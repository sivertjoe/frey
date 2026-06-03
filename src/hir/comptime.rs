//! Compile-time evaluation support for `#comptime` functions.
//!
//! A `#comptime` function is lowered to a generic template like any other,
//! but its body may contain type-introspection nodes (`ExprKind::TypeValue`,
//! `ExprKind::CompError`). When the template is specialized for concrete
//! types, the substitution pass in `lower.rs` uses the helpers here to:
//!
//!   * evaluate comptime-known expressions (e.g. `T == Int`) to a [`CtValue`],
//!     so they can be folded to constants and drive `static if` selection, and
//!   * decide which statements diverge, so unreachable code (including a dead
//!     `comperror`) is dropped rather than evaluated.
//!
//! Everything here is pure: it reads HIR and reports facts. The actual
//! rewriting (folding, truncation) is done by the substitution pass.

use crate::hir::BinaryOperator;
use crate::hir::generics::ty_has_typevars;
use crate::hir::types::{Block, BlockItem, Const, Expr, ExprKind, Statement, StatementKind};

/// `Int` is currently unused, but reserved as the extension point for future
/// comptime constants (e.g. a `comptime n: Int` parameter).
#[allow(dead_code)]
pub enum CtValue {
    Type(crate::hir::types::Ty),
    Int(i32),
    Bool(bool),
    Str(String),
}

pub fn eval(e: &Expr) -> Option<CtValue> {
    match &e.kind {
        ExprKind::TypeValue(ty) if !ty_has_typevars(ty) => Some(CtValue::Type(ty.clone())),
        ExprKind::Const(Const::Int(n)) => Some(CtValue::Int(*n)),
        ExprKind::Const(Const::Str(s)) => Some(CtValue::Str(s.clone())),
        _ => None,
    }
}

pub fn eval_binary(op: BinaryOperator, lhs: &Expr, rhs: &Expr) -> Option<CtValue> {
    let l = eval(lhs)?;
    let r = eval(rhs)?;
    match (op, l, r) {
        (BinaryOperator::Eq, CtValue::Type(a), CtValue::Type(b)) => Some(CtValue::Bool(a == b)),
        (BinaryOperator::Ne, CtValue::Type(a), CtValue::Type(b)) => Some(CtValue::Bool(a != b)),
        (BinaryOperator::Eq, CtValue::Str(a), CtValue::Str(b)) => Some(CtValue::Bool(a == b)),
        (BinaryOperator::Ne, CtValue::Str(a), CtValue::Str(b)) => Some(CtValue::Bool(a != b)),
        _ => None,
    }
}

pub fn item_diverges(item: &BlockItem) -> bool {
    match item {
        BlockItem::Declaration(_) => false,
        BlockItem::Statement(s) => stmt_diverges(s),
    }
}

fn stmt_diverges(s: &Statement) -> bool {
    match &s.kind {
        StatementKind::Return(_) => true,
        StatementKind::Break => false,
        StatementKind::Defer(_) => false,
        StatementKind::Expr(e) => expr_diverges(e),
    }
}

pub fn expr_diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::CompError(_) => true,
        ExprKind::Block(b) => block_diverges(b),
        ExprKind::If {
            then_branch,
            else_branch,
            ..
        } => expr_diverges(then_branch) && expr_diverges(else_branch),
        _ => false,
    }
}

fn block_diverges(b: &Block) -> bool {
    b.items.iter().any(item_diverges) || expr_diverges(&b.tail)
}
