use crate::hir::error::{Error, ErrorKind};
use crate::hir::types::{Const, Expr, ExprKind, Ty, UnaryOperator};

pub fn const_int_literal(e: &Expr) -> Option<i32> {
    if e.ty != Ty::Int {
        return None;
    }
    match &e.kind {
        ExprKind::Const(Const::Int(n)) => Some(*n),
        ExprKind::Unary { op, operand } if matches!(op, UnaryOperator::Minus) => {
            match &operand.kind {
                ExprKind::Const(Const::Int(n)) => n.checked_neg(),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn fits_in_ty(v: i32, ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::I32 | Ty::I64 => true,
        Ty::I8 => i8::try_from(v).is_ok(),
        Ty::U8 => u8::try_from(v).is_ok(),
        Ty::UInt | Ty::U32 | Ty::U64 => v >= 0,
        Ty::Float | Ty::F32 | Ty::F64 => true,
        _ => false,
    }
}

pub fn coerce_int_literal(expr: Expr, target: &Ty) -> Result<Expr, Error> {
    if expr.ty == *target || *target == Ty::Int || !target.is_number() {
        return Ok(expr);
    }
    let Some(value) = const_int_literal(&expr) else {
        return Ok(expr);
    };
    if !fits_in_ty(value, target) {
        return Err(Error {
            span: expr.span,
            kind: ErrorKind::LiteralOutOfRange {
                value,
                target: target.clone(),
            },
        });
    }
    let span = expr.span;
    Ok(Expr {
        span,
        ty: target.clone(),
        kind: ExprKind::Cast {
            target: target.clone(),
            expr: Box::new(expr),
        },
    })
}

pub fn coerce_through_tails(expr: Expr, target: &Ty) -> Result<Expr, Error> {
    if expr.ty == *target {
        return Ok(expr);
    }
    match expr.kind {
        ExprKind::Block(mut b) => {
            let new_tail = coerce_through_tails(*b.tail, target)?;
            let block_ty = new_tail.ty.clone();
            b.tail = Box::new(new_tail);
            Ok(Expr {
                span: expr.span,
                ty: block_ty,
                kind: ExprKind::Block(b),
            })
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let then_branch = coerce_through_tails(*then_branch, target)?;
            let else_branch = coerce_through_tails(*else_branch, target)?;
            let if_ty = then_branch.ty.clone();
            Ok(Expr {
                span: expr.span,
                ty: if_ty,
                kind: ExprKind::If {
                    condition,
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
            })
        }
        _ => coerce_int_literal(expr, target),
    }
}
