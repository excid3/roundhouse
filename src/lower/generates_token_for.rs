//! `generates_token_for :purpose, expires_in: D do <value> end` — Rails
//! 7.1's single-use-ish tokens, synthesized in the shared model lowering
//! (all targets): `generate_token_for(purpose)`, `Model.find_by_token_for
//! (purpose, token)` and its bang form.
//!
//! The analyzer already types the three methods
//! (`register_generates_token_for`); this pass defines them.
//!
//! ## What Rails does, and what this does
//!
//! A token is signed for one purpose, optionally expires, and carries
//! the record id plus the VALUE of the declaration block evaluated on
//! the record — so the token stops verifying when that value changes
//! (a password-reset token dies when the password does). `find_by_token_for`
//! verifies, finds the record by id, re-evaluates the block on it and
//! compares; any failure is nil. The bang form raises
//! `ActiveSupport::MessageVerifier::InvalidSignature` instead, and
//! `RecordNotFound` for a token naming a row that is gone.
//!
//! The wire half is `ActiveRecord::TokenFor`
//! (runtime/ruby/active_record/token_for.rb), the runtime
//! `has_secure_password`'s reset token already stands on, and the
//! format is Rails' own: the `[id]` or `[id, value]` payload under the
//! purpose `"<Model>\n<purpose>\n<expires_in seconds>"`. The purpose is
//! a compile-time fact, so `expires_in:` must fold to seconds here — an
//! Integer or `N.<unit>` literal.
//!
//! ## Synthesis
//!
//! Ruby source, re-ingested — the same route `ingest::current_attributes`
//! takes — because the block body is an instance-level expression the
//! finder has to evaluate on the record it found. Two dispatchers on
//! the purpose carry everything per-declaration: `__token_data(purpose)`,
//! the payload the record produces now, which `generate_token_for` signs
//! and the finders compare against; and `__token_purpose(purpose)`, the
//! purpose string the finders verify under:
//!
//! ```ruby
//! def __token_data(purpose)
//!   case purpose
//!   when :email_change
//!     ActiveRecord::TokenFor.value_data(id, (unconfirmed_email)&.to_s)
//!   else
//!     raise "unknown token purpose"
//!   end
//! end
//! ```
//!
//! A block value goes into the payload as its String form (nil as
//! `null`). For a String value — the corpus' case — that is Rails'
//! JSON; a number or a Time would be quoted where Rails writes it
//! bare, so such a token verifies in the emitted app but not across
//! to Rails.
//!
//! ## Claimed and declined
//!
//! Claimed: a plain Symbol purpose (`:email_change`, not `:"a-b"`), an
//! optional `expires_in:` that folds to seconds, and an optional block
//! without parameters. Anything else — a computed expiry,
//! `expires_at:`, a block taking the record as a parameter — stays
//! unclaimed and keeps its unsupported warning: half an expansion is
//! worse than none. The methods dispatch on a purpose passed at
//! runtime, so it is all or nothing per model: a purpose whose LAST
//! declaration is unclaimed (Rails keeps the last), a computed purpose,
//! or a key that is not an Integer `id` declines the whole model, and
//! every declaration on it warns.
//! `token_for_decls` is the one place that decides, and
//! `report_unclaimed_unknowns` asks `claims` by span.

use super::model_to_library::fn_sig;
use crate::dialect::{MethodDef, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// One `generates_token_for` declaration this pass claims.
pub(crate) struct TokenForDecl {
    pub(crate) purpose: Symbol,
    /// `expires_in:` in seconds; 0 means the token never expires.
    pub(crate) expires_in: i64,
    /// The block body; `None` means the token carries only the id.
    pub(crate) value: Option<Expr>,
    pub(crate) span: Span,
}

/// The declarations `model` gets token methods for, one per purpose:
/// Rails keeps the LAST declaration of a purpose
/// (`token_definitions.merge`). All or nothing. The methods dispatch on
/// a purpose the caller passes at runtime, so a purpose whose last
/// declaration this pass cannot expand would leave a typed
/// `generate_token_for(:that)` raising in the emitted app. An earlier
/// form must not stand in for it either. That declines the whole model,
/// as do a computed purpose (it could replace any of them) and a key
/// that is not an Integer `id`: the payload is `[id, …]` with the id as
/// a JSON number, and Rails writes a uuid or other String key as a JSON
/// string the runtime does not read back.
pub(crate) fn token_for_decls(model: &Model) -> Vec<TokenForDecl> {
    model_decls(model).unwrap_or_default()
}

/// Whether the declaration at `span` is one this pass handles: an
/// expandable form on a model it does not decline, including one a
/// later declaration supersedes, which Rails discards too.
/// `report_unclaimed_unknowns` asks.
pub(crate) fn claims(model: &Model, span: Span) -> bool {
    model_decls(model).is_some()
        && parse_decls(&model.body).iter().any(|d| matches!(d, Ok(decl) if decl.span == span))
}

/// The last declaration of each purpose, or `None` when the model is
/// declined whole (see `token_for_decls`).
fn model_decls(model: &Model) -> Option<Vec<TokenForDecl>> {
    if !integer_id(model) {
        return None;
    }
    let mut last: Vec<(Symbol, Option<TokenForDecl>)> = Vec::new();
    for d in parse_decls(&model.body) {
        let (purpose, decl) = match d {
            Ok(decl) => (decl.purpose.clone(), Some(decl)),
            Err(purpose) => (purpose?, None),
        };
        last.retain(|(p, _)| *p != purpose);
        last.push((purpose, decl));
    }
    last.into_iter().map(|(_, decl)| decl).collect()
}

fn integer_id(model: &Model) -> bool {
    let named_id = model.primary_key.as_ref().is_none_or(|k| k.as_str() == "id");
    named_id && model.attributes.fields.get(&Symbol::from("id")).is_none_or(|t| *t == Ty::Int)
}

/// A purpose the synthesized source can spell as a bare Symbol literal
/// (`when :email_change`). A quoted one (`:"share-link"`) would come out
/// as other Ruby, so it stays unlowered.
fn plain_purpose(purpose: &Symbol) -> bool {
    let mut chars = purpose.as_str().chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Every `generates_token_for` call in `body`, in source order: the
/// declaration when this pass can expand it, otherwise the purpose it
/// names (`None` for a computed one).
fn parse_decls(body: &[ModelBodyItem]) -> Vec<Result<TokenForDecl, Option<Symbol>>> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "generates_token_for" {
            continue;
        }
        let mut purpose: Option<Symbol> = None;
        let mut expires_in: i64 = 0;
        let mut ok = true;
        for (i, arg) in args.iter().enumerate() {
            match &*arg.node {
                ExprNode::Lit { value: Literal::Sym { value } } if i == 0 => {
                    purpose = Some(value.clone());
                }
                ExprNode::Hash { entries, .. } if i == 1 => {
                    for (k, v) in entries {
                        match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value: key } }
                                if key.as_str() == "expires_in" =>
                            {
                                match duration_seconds(v) {
                                    Some(secs) if secs > 0 => expires_in = secs,
                                    _ => ok = false,
                                }
                            }
                            _ => ok = false,
                        }
                    }
                }
                _ => ok = false,
            }
        }
        let value = match block {
            None => None,
            Some(b) => match &*b.node {
                ExprNode::Lambda { params, rest_param, block_param, body, .. }
                    if params.is_empty() && rest_param.is_none() && block_param.is_none() =>
                {
                    Some(body.clone())
                }
                _ => {
                    ok = false;
                    None
                }
            },
        };
        out.push(match purpose {
            Some(purpose) if ok && plain_purpose(&purpose) => {
                Ok(TokenForDecl { purpose, expires_in, value, span: expr.span })
            }
            purpose => Err(purpose),
        });
    }
    out
}

/// Synthesize the model's token methods from its declarations and
/// append them to `methods`; a method the model writes itself wins.
pub(crate) fn push_token_for_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let decls = token_for_decls(model);
    if decls.is_empty() {
        return;
    }
    let src = synthesized_source(model, &decls);
    let (parsed, diags) = crate::ingest::prism::scope(|| {
        crate::ingest::ingest_library_classes(src.as_bytes(), "<generates_token_for>")
    });
    let synthesized: Vec<MethodDef> = match parsed {
        Ok(classes) if diags.is_empty() => classes.into_iter().flat_map(|c| c.methods).collect(),
        Ok(_) => {
            crate::ingest::survey::record_synthesis_failure(
                "<generates_token_for>",
                &format!("generates_token_for methods for `{}`", model.name.0.as_str()),
                &diags,
            );
            return;
        }
        Err(err) => {
            crate::ingest::survey::record(&err);
            return;
        }
    };
    let record = Ty::Class { id: model.name.clone(), args: vec![] };
    let nilable_record = Ty::Union { variants: vec![record.clone(), Ty::Nil] };
    for mut m in synthesized {
        // Declared signatures, so the sidecar the strict targets compile
        // from says what the registry already says (`register_generates_
        // token_for`) instead of `untyped`.
        m.signature = match m.name.as_str() {
            "generate_token_for" => Some(fn_sig(vec![(Symbol::from("purpose"), Ty::Sym)], Ty::Str)),
            "find_by_token_for" => Some(fn_sig(
                vec![(Symbol::from("purpose"), Ty::Sym), (Symbol::from("token"), Ty::Str)],
                nilable_record.clone(),
            )),
            "find_by_token_for!" => Some(fn_sig(
                vec![(Symbol::from("purpose"), Ty::Sym), (Symbol::from("token"), Ty::Str)],
                record.clone(),
            )),
            "__token_purpose" => Some(fn_sig(vec![(Symbol::from("purpose"), Ty::Sym)], Ty::Str)),
            _ => Some(fn_sig(vec![(Symbol::from("purpose"), Ty::Sym)], Ty::Str)),
        };
        // `methods` already holds the model's own (push_user_methods
        // runs first), and one it writes itself wins.
        let own = methods.iter().any(|x| x.name == m.name && x.receiver == m.receiver);
        if !own {
            methods.push(m);
        }
    }
}

/// `expires_in:` as seconds, when it is a compile-time fact: an
/// Integer literal or `N.<unit>` for the fixed-length units. A month
/// or a year has no fixed length, and anything computed is unknown.
/// The analyzer asks before `lower::duration` runs and the model
/// lowering after it, so the grounded `ActiveSupport::Duration.<units>(N)`
/// reads the same as the `N.<unit>` it came from.
fn duration_seconds(e: &Expr) -> Option<i64> {
    let int = |e: &Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Int { value } } => Some(*value),
        _ => None,
    };
    let unit_seconds = |unit: &str| match unit {
        "second" | "seconds" => Some(1),
        "minute" | "minutes" => Some(60),
        "hour" | "hours" => Some(3_600),
        "day" | "days" => Some(86_400),
        "week" | "weeks" => Some(604_800),
        _ => None,
    };
    match &*e.node {
        ExprNode::Lit { .. } => int(e),
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. } => {
            let (n, unit) = match (&*recv.node, args.as_slice()) {
                (ExprNode::Const { path }, [n])
                    if path.len() == 2
                        && path[0].as_str() == "ActiveSupport"
                        && path[1].as_str() == "Duration" =>
                {
                    (int(n)?, method.as_str())
                }
                (_, []) => (int(recv)?, method.as_str()),
                _ => return None,
            };
            n.checked_mul(unit_seconds(unit)?)
        }
        _ => None,
    }
}

/// Rails' `TokenDefinition#full_purpose`, `[class, purpose,
/// expires_in].join("\n")` — a nil expiry joins as "" — JSON-escaped
/// the way `TokenFor.verified_data` compares it, and escaped once more
/// to sit in a Ruby string literal: `"User\\nemail_change\\n3600"`.
fn full_purpose(model: &Model, d: &TokenForDecl) -> String {
    let expires = if d.expires_in > 0 { d.expires_in.to_string() } else { String::new() };
    format!("{}\\\\n{}\\\\n{expires}", model.name.0.as_str(), d.purpose.as_str())
}

fn synthesized_source(model: &Model, decls: &[TokenForDecl]) -> String {
    use crate::emit::ruby::emit_expr;
    let class = model.name.0.as_str();
    let case = |arms: String| format!("    case purpose\n{arms}    else\n      raise \"unknown token purpose\"\n    end\n");
    let arms = |arm: &dyn Fn(&TokenForDecl) -> String| {
        decls.iter().map(|d| format!("    when :{}\n      {}\n", d.purpose.as_str(), arm(d))).collect::<String>()
    };

    // The payload for `purpose` on this record: `[id]`, or `[id, value]`
    // with the block's value in its String form.
    let data = case(arms(&|d| match &d.value {
        Some(e) => format!("ActiveRecord::TokenFor.value_data(id, ({})&.to_s)", emit_expr(e)),
        None => "ActiveRecord::TokenFor.id_data(id)".to_string(),
    }));
    let purposes = case(arms(&|d| format!("\"{}\"", full_purpose(model, d))));
    let generate = case(arms(&|d| {
        format!("ActiveRecord::TokenFor.generate(data, {class}.__token_purpose(purpose), {})", d.expires_in)
    }));

    // Rails: the finder answers nil for a token that does not verify,
    // names no row, or whose payload the record no longer produces. The
    // bang form raises InvalidSignature for the first and last, and
    // `find`'s RecordNotFound for a row that is gone.
    format!(
        "class {class}
  def __token_data(purpose)
{data}  end

  def self.__token_purpose(purpose)
{purposes}  end

  def generate_token_for(purpose)
    data = __token_data(purpose)
{generate}  end

  def self.find_by_token_for(purpose, token)
    data = ActiveRecord::TokenFor.verified_data(token, {class}.__token_purpose(purpose))
    return nil if data == \"\"
    record = {class}.find_by(id: ActiveRecord::TokenFor.data_id(data))
    return nil if record.nil?
    record.__token_data(purpose) == data ? record : nil
  end

  def self.find_by_token_for!(purpose, token)
    data = ActiveRecord::TokenFor.verified_data(token, {class}.__token_purpose(purpose))
    raise ActiveSupport::MessageVerifier::InvalidSignature if data == \"\"
    record = {class}.find(ActiveRecord::TokenFor.data_id(data))
    raise ActiveSupport::MessageVerifier::InvalidSignature unless record.__token_data(purpose) == data
    record
  end
end
"
    )
}
