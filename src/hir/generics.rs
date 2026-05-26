use std::collections::HashMap;

use crate::hir::error::{Error, ErrorKind};
use crate::hir::types::{Ty, TypeVarId};

/// Recurses through a `Ty` looking for `Ty::TypeVar` anywhere inside it.
pub fn ty_has_typevars(ty: &Ty) -> bool {
    match ty {
        Ty::TypeVar(_) => true,
        Ty::Ptr(inner) => ty_has_typevars(inner),
        Ty::Array { element, .. } => ty_has_typevars(element),
        Ty::Function { params, return_ty } => {
            params.iter().any(ty_has_typevars) || ty_has_typevars(return_ty)
        }
        _ => false,
    }
}

/// Appends every distinct `TypeVarId` found in `ty` (in encounter order) to
/// `out`. Used to collect the type parameters of a generic function's
/// signature in a stable order.
pub fn collect_typevars(ty: &Ty, out: &mut Vec<TypeVarId>) {
    match ty {
        Ty::TypeVar(id) => {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        Ty::Ptr(inner) => collect_typevars(inner, out),
        Ty::Array { element, .. } => collect_typevars(element, out),
        Ty::Function { params, return_ty } => {
            for p in params {
                collect_typevars(p, out);
            }
            collect_typevars(return_ty, out);
        }
        _ => {}
    }
}

/// Walks `ty` and substitutes any `Ty::TypeVar(id)` for `subst[id]` (when
/// present), recursively. Leaves unmapped TypeVars in place.
pub fn substitute_ty(ty: &Ty, subst: &HashMap<TypeVarId, Ty>) -> Ty {
    match ty {
        Ty::TypeVar(id) => subst.get(id).cloned().unwrap_or(Ty::TypeVar(*id)),
        Ty::Ptr(inner) => Ty::Ptr(Box::new(substitute_ty(inner, subst))),
        Ty::Array { element, count } => Ty::Array {
            element: Box::new(substitute_ty(element, subst)),
            count: *count,
        },
        Ty::Function { params, return_ty } => Ty::Function {
            params: params.iter().map(|p| substitute_ty(p, subst)).collect(),
            return_ty: Box::new(substitute_ty(return_ty, subst)),
        },
        _ => ty.clone(),
    }
}

/// Tries to bind type variables in `param_ty` to concrete bits of `arg_ty`,
/// updating `subst`. Errors if the structures disagree or if the same
/// TypeVar would have to bind to two different concrete types.
pub fn unify(
    param_ty: &Ty,
    arg_ty: &Ty,
    span: crate::lexer::types::Span,
    subst: &mut HashMap<TypeVarId, Ty>,
) -> Result<(), Error> {
    match (param_ty, arg_ty) {
        (Ty::TypeVar(id), concrete) => {
            if let Some(existing) = subst.get(id) {
                if existing != concrete {
                    return Err(Error {
                        span,
                        kind: ErrorKind::TypeMismatch {
                            expected: existing.clone(),
                            found: concrete.clone(),
                        },
                    });
                }
                Ok(())
            } else {
                subst.insert(*id, concrete.clone());
                Ok(())
            }
        }
        (Ty::Ptr(p), Ty::Ptr(a)) => unify(p, a, span, subst),
        (
            Ty::Array {
                element: ep,
                count: cp,
            },
            Ty::Array {
                element: ea,
                count: ca,
            },
        ) if cp == ca => unify(ep, ea, span, subst),
        (
            Ty::Function {
                params: ps,
                return_ty: rp,
            },
            Ty::Function {
                params: as_,
                return_ty: ra,
            },
        ) if ps.len() == as_.len() => {
            for (p, a) in ps.iter().zip(as_.iter()) {
                unify(p, a, span, subst)?;
            }
            unify(rp, ra, span, subst)
        }
        (a, b) if a == b => Ok(()),
        (a, b) => Err(Error {
            span,
            kind: ErrorKind::TypeMismatch {
                expected: a.clone(),
                found: b.clone(),
            },
        }),
    }
}
