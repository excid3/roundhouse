//! `store_accessor :preferences, :theme` — ActiveRecord::Store over a
//! schema-less JSON column: named keys of an untyped JSON object.
//!
//! Rails defines, per key, a reader answering `preferences["theme"]`
//! (nil when absent) and a writer assigning into the same Hash. Nothing
//! about either is dynamic once the declaration is read, so the DSL
//! EXPANDS, the way `lower::has_json` does (see its header for the
//! posture), into one reader/writer pair per key on the model:
//!
//!   def theme          = JsonColumn.read_key(@preferences, "theme")
//!   def theme=(value)  = @preferences = JsonColumn.write_key(@preferences, "theme", value)
//!
//! The calls land on the column's existing storage seam (`JsonColumn`,
//! `runtime/spinel/json_column.rb` and its overlay twin) with one key
//! picked out of `load`/`dump`. The values are UNTYPED, as Rails' are:
//! the declaration names keys, not types. `has_json` is the typed
//! alternative, and a different declaration.
//!
//! **Only a column `column_slot` reads through `JsonColumn` is
//! claimed** (`t.json`/`t.jsonb`, or `serialize …, coder: JSON`).
//! `store_accessor` also works over `store :settings`, a text column
//! with a YAML coder, which the JSON seam cannot read; that declaration
//! stays unclaimed and keeps warning. So does a splatted key list
//! (`store_accessor :roles, *ROLES`) and any option other than a literal
//! `prefix:`/`suffix:`. The ActiveModel::Dirty methods Rails adds per
//! key (`theme_changed?`, …) are not synthesized; a call to one stays a
//! dispatch failure, honestly.

use crate::dialect::{AccessorKind, MethodDef, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::schema::Table;
use crate::span::Span;
use crate::ty::Ty;

use super::column_slot::{is_json_column, json_column};
use super::model_to_library::push_synth_instance_method;
use super::typing::lit_str;

pub(crate) struct StoreAccessorKey {
    /// The accessor's name: the key with any `prefix:`/`suffix:` applied.
    pub(crate) accessor: Symbol,
    /// The JSON object key — Rails stringifies the symbol.
    pub(crate) key: Symbol,
}

pub(crate) struct StoreAccessorDecl {
    /// The call's own span; see `HasJsonDecl::span`.
    pub(crate) span: Span,
    pub(crate) column: Symbol,
    pub(crate) keys: Vec<StoreAccessorKey>,
}

/// Every `store_accessor :<col>, :<key>, …` in a model body that this
/// lowering can fully expand (see the module docs). A declaration with
/// anything it cannot read is omitted WHOLE.
pub(crate) fn store_accessor_decls(model: &Model, table: Option<&Table>) -> Vec<StoreAccessorDecl> {
    model
        .body
        .iter()
        .filter_map(|item| match item {
            ModelBodyItem::Unknown { expr, .. } => parse_decl(expr, model, table),
            _ => None,
        })
        .collect()
}

fn parse_decl(expr: &Expr, model: &Model, table: Option<&Table>) -> Option<StoreAccessorDecl> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "store_accessor" {
        return None;
    }
    let column = sym_lit(args.first()?)?;
    if !is_json_column(model, table, &column) {
        return None;
    }
    let mut names = Vec::new();
    let mut prefix: Option<String> = None;
    let mut suffix: Option<String> = None;
    for arg in &args[1..] {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => names.push(value.clone()),
            ExprNode::Hash { entries, .. } => {
                for (option, value) in entries {
                    // `true` affixes the column name; a Symbol or String
                    // names the affix itself; `false` is the default.
                    let text = match &*value.node {
                        ExprNode::Lit { value: Literal::Bool { value: true } } => {
                            Some(column.as_str().to_string())
                        }
                        ExprNode::Lit { value: Literal::Bool { value: false } } => None,
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                        _ => return None,
                    };
                    match sym_lit(option)?.as_str() {
                        "prefix" => prefix = text,
                        "suffix" => suffix = text,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    if names.is_empty() {
        return None;
    }
    let keys = names
        .into_iter()
        .map(|key| {
            let accessor = [prefix.as_deref(), Some(key.as_str()), suffix.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("_");
            StoreAccessorKey { accessor: Symbol::from(accessor), key }
        })
        .collect();
    Some(StoreAccessorDecl { span: expr.span, column, keys })
}

/// The accessor names every claimed declaration on `model` synthesizes
/// — what the permit filter and the form-field reader lookup count as
/// the model's own attributes.
pub(crate) fn store_accessor_names(model: &Model, table: Option<&Table>) -> Vec<Symbol> {
    store_accessor_decls(model, table)
        .into_iter()
        .flat_map(|decl| decl.keys.into_iter().map(|k| k.accessor))
        .collect()
}

/// Synthesize the reader/writer pair for every key.
///
/// A custom method in the model body must win, and `push_user_methods`
/// runs after this — `push_synth_instance_method` does that check
/// itself, same dance as `push_typed_store_methods`.
pub(crate) fn push_store_accessor_methods(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    table: Option<&Table>,
) {
    let value = Symbol::from("value");
    for decl in store_accessor_decls(model, table) {
        for k in &decl.keys {
            push_synth_instance_method(
                methods,
                model,
                k.accessor.clone(),
                Vec::new(),
                read_body(&decl.column, &k.key),
                Some(super::model_to_library::fn_sig(vec![], Ty::Untyped)),
                AccessorKind::Method,
                false,
            );
            push_synth_instance_method(
                methods,
                model,
                Symbol::from(format!("{}=", k.accessor.as_str())),
                vec![Param::positional(value.clone())],
                write_body(&decl.column, &k.key),
                Some(super::model_to_library::fn_sig(vec![(value.clone(), Ty::Untyped)], Ty::Nil)),
                AccessorKind::Method,
                true,
            );
        }
    }
}

fn sym_lit(expr: &Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

fn ivar(col: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.clone() })
}

/// `JsonColumn.read_key(@<col>, "<key>")`.
fn read_body(col: &Symbol, key: &Symbol) -> Expr {
    json_column(
        Span::synthetic(),
        "read_key",
        vec![ivar(col), lit_str(key.as_str().to_string())],
        Ty::Untyped,
    )
}

/// `@<col> = JsonColumn.write_key(@<col>, "<key>", value)`.
fn write_body(col: &Symbol, key: &Symbol) -> Expr {
    let value = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: Symbol::from("value") },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: col.clone() },
            value: json_column(
                Span::synthetic(),
                "write_key",
                vec![ivar(col), lit_str(key.as_str().to_string()), value],
                Ty::Untyped,
            ),
        },
    )
}
