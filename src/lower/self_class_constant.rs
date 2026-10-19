//! `self.class::NAME` the typer could not place on one declaring class
//! — a concern's read under several includers that each declare their
//! own — is the runtime lookup it was written as. The typer leaves it
//! the bare path with `SELF_CLASS_CONST_REF` (see
//! `BodyTyper::self_class_constant`); this pass spells it
//! `self.class.const_get(:NAME)`, an IR shape every emitter prints as
//! written rather than a bare `NAME` naming the wrong constant. The
//! site is already on the strict targets' residue ledger
//! (`lower_residue`); a read the typer refused carries its unsupported
//! stub and renders as that.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub(crate) fn rewrite_node(expr: &mut Expr) {
    if expr.decisions & crate::expr::SELF_CLASS_CONST_REF == 0 {
        return;
    }
    let ExprNode::Const { path } = &*expr.node else { return };
    let span = expr.span;
    let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let self_class = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from("class"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    expr.node = Box::new(ExprNode::Send {
        recv: Some(self_class),
        method: Symbol::from("const_get"),
        args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } })],
        block: None,
        parenthesized: true,
    });
    expr.decisions &= !crate::expr::SELF_CLASS_CONST_REF;
}
