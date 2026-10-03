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
//! finder has to evaluate on the record it found. Each purpose gets a
//! payload method (`__token_data_for_email_change`), which
//! `generate_token_for` calls on `self` and the finder on `record`:
//!
//! ```ruby
//! def generate_token_for(purpose)
//!   case purpose
//!   when :email_change
//!     ActiveRecord::TokenFor.generate(__token_data_for_email_change, "User\\nemail_change\\n3600", 3600)
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
//! Claimed: a Symbol purpose, an optional `expires_in:` that folds to
//! seconds, and an optional block without parameters. Anything else — a
//! computed purpose or expiry, `expires_at:`, a block taking the record
//! as a parameter — stays unclaimed and keeps its unsupported warning:
//! half an expansion is worse than none. `token_for_decls` is the one
//! place that decides, and `report_unclaimed_unknowns` asks it by span.

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

/// The declarations in `body` this pass expands.
pub(crate) fn token_for_decls(body: &[ModelBodyItem]) -> Vec<TokenForDecl> {
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
        if let (true, Some(purpose)) = (ok, purpose) {
            out.push(TokenForDecl { purpose, expires_in, value, span: expr.span });
        }
    }
    out
}

/// Synthesize the model's token methods from its declarations and
/// append them to `methods`; a method the model writes itself wins.
pub(crate) fn push_token_for_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let decls = token_for_decls(&model.body);
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
            "__verified_token_data" => Some(fn_sig(
                vec![(Symbol::from("purpose"), Ty::Sym), (Symbol::from("token"), Ty::Str)],
                Ty::Str,
            )),
            "__token_data_matches?" => Some(fn_sig(
                vec![(Symbol::from("purpose"), Ty::Sym), (Symbol::from("data"), Ty::Str)],
                Ty::Bool,
            )),
            _ => Some(fn_sig(vec![], Ty::Str)),
        };
        let own = model.body.iter().any(|item| {
            matches!(item, ModelBodyItem::Method { method, .. }
                if method.name == m.name && method.receiver == m.receiver)
        }) || methods.iter().any(|x| x.name == m.name && x.receiver == m.receiver);
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

fn data_method(purpose: &Symbol) -> String {
    format!("__token_data_for_{}", purpose.as_str())
}

fn synthesized_source(model: &Model, decls: &[TokenForDecl]) -> String {
    use crate::emit::ruby::emit_expr;
    let class = model.name.0.as_str();
    let mut body = String::new();

    // One payload method per purpose: `[id]`, or `[id, value]` with the
    // block's value in its String form.
    for d in decls {
        let data = match &d.value {
            Some(e) => format!(
                "value = ({})\n    ActiveRecord::TokenFor.value_data(id, value.nil? ? nil : value.to_s)",
                emit_expr(e)
            ),
            None => "ActiveRecord::TokenFor.id_data(id)".to_string(),
        };
        body.push_str(&format!("  def {}\n    {data}\n  end\n\n", data_method(&d.purpose)));
    }

    body.push_str("  def generate_token_for(purpose)\n    case purpose\n");
    for d in decls {
        body.push_str(&format!(
            "    when :{p}\n      ActiveRecord::TokenFor.generate({dm}, \"{purpose}\", {expires})\n",
            p = d.purpose.as_str(),
            dm = data_method(&d.purpose),
            purpose = full_purpose(model, d),
            expires = d.expires_in,
        ));
    }
    body.push_str("    else\n      raise \"unknown token purpose\"\n    end\n  end\n\n");

    // The verified payload for `purpose`, "" for every rejection.
    body.push_str("  def self.__verified_token_data(purpose, token)\n    data = \"\"\n    case purpose\n");
    for d in decls {
        body.push_str(&format!(
            "    when :{p}\n      data = ActiveRecord::TokenFor.verified_data(token, \"{purpose}\")\n",
            p = d.purpose.as_str(),
            purpose = full_purpose(model, d),
        ));
    }
    body.push_str("    else\n      raise \"unknown token purpose\"\n    end\n    data\n  end\n\n");

    // Whether `record` still produces the payload the token carries.
    body.push_str("  def __token_data_matches?(purpose, data)\n    current = \"\"\n    case purpose\n");
    for d in decls {
        body.push_str(&format!(
            "    when :{p}\n      current = {dm}\n",
            p = d.purpose.as_str(),
            dm = data_method(&d.purpose),
        ));
    }
    body.push_str("    end\n    current == data\n  end\n\n");

    body.push_str(&format!(
        "  def self.find_by_token_for(purpose, token)\n    data = {class}.__verified_token_data(purpose, token)\n    return nil if data == \"\"\n    record = {class}.find_by(id: ActiveRecord::TokenFor.data_id(data))\n    return nil if record.nil?\n    record.__token_data_matches?(purpose, data) ? record : nil\n  end\n\n"
    ));

    // Rails: a token that does not verify, or whose value no longer
    // matches, is InvalidSignature; one naming a row that is gone is
    // `find`'s RecordNotFound.
    body.push_str(&format!(
        "  def self.find_by_token_for!(purpose, token)\n    data = {class}.__verified_token_data(purpose, token)\n    raise ActiveSupport::MessageVerifier::InvalidSignature if data == \"\"\n    record = {class}.find(ActiveRecord::TokenFor.data_id(data))\n    raise ActiveSupport::MessageVerifier::InvalidSignature unless record.__token_data_matches?(purpose, data)\n    record\n  end\n"
    ));

    format!("class {class}\n{body}end\n")
}
