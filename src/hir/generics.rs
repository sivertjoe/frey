use std::collections::HashMap;

use crate::hir::error::{Error, ErrorKind};
use crate::hir::types::{Ty, TypeVarId};

pub fn ty_has_typevars(ty: &Ty) -> bool {
    match ty {
        Ty::TypeVar(_) => true,
        Ty::Ptr(inner) => ty_has_typevars(inner),
        Ty::Array { element, .. } => ty_has_typevars(element),
        Ty::Function { params, return_ty, .. } => {
            params.iter().any(ty_has_typevars) || ty_has_typevars(return_ty)
        }
        Ty::Closure { params, return_ty } => {
            params.iter().any(ty_has_typevars) || ty_has_typevars(return_ty)
        }
        Ty::GenericStruct { args, .. } => args.iter().any(ty_has_typevars),
        Ty::GenericEnum { args, .. } => args.iter().any(ty_has_typevars),
        Ty::Tuple(elems) => elems.iter().any(ty_has_typevars),
        _ => false,
    }
}

pub fn collect_typevars(ty: &Ty, out: &mut Vec<TypeVarId>) {
    match ty {
        Ty::TypeVar(id) => {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        Ty::Ptr(inner) => collect_typevars(inner, out),
        Ty::Array { element, .. } => collect_typevars(element, out),
        Ty::Function { params, return_ty, .. } => {
            for p in params {
                collect_typevars(p, out);
            }
            collect_typevars(return_ty, out);
        }
        Ty::Closure { params, return_ty } => {
            for p in params {
                collect_typevars(p, out);
            }
            collect_typevars(return_ty, out);
        }
        Ty::GenericStruct { args, .. } => {
            for a in args {
                collect_typevars(a, out);
            }
        }
        Ty::GenericEnum { args, .. } => {
            for a in args {
                collect_typevars(a, out);
            }
        }
        Ty::Tuple(elems) => {
            for e in elems {
                collect_typevars(e, out);
            }
        }
        _ => {}
    }
}

pub fn unify(
    param_ty: &Ty,
    arg_ty: &Ty,
    span: crate::lexer::types::Span,
    subst: &mut HashMap<TypeVarId, Ty>,
    struct_origins: &HashMap<String, (String, Vec<Ty>)>,
    enum_origins: &HashMap<String, (String, Vec<Ty>)>,
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
        (Ty::Ptr(p), Ty::Ptr(a)) => unify(p, a, span, subst, struct_origins, enum_origins),
        (
            Ty::Array {
                element: ep,
                count: cp,
            },
            Ty::Array {
                element: ea,
                count: ca,
            },
        ) if cp == ca => unify(ep, ea, span, subst, struct_origins, enum_origins),
        (
            Ty::Function {
                params: ps,
                return_ty: rp,
                ..
            },
            Ty::Function {
                params: as_,
                return_ty: ra,
                ..
            },
        ) if ps.len() == as_.len() => {
            for (p, a) in ps.iter().zip(as_.iter()) {
                unify(p, a, span, subst, struct_origins, enum_origins)?;
            }
            unify(rp, ra, span, subst, struct_origins, enum_origins)
        }
        (
            Ty::Closure {
                params: ps,
                return_ty: rp,
            },
            Ty::Closure {
                params: as_,
                return_ty: ra,
            },
        ) if ps.len() == as_.len() => {
            for (p, a) in ps.iter().zip(as_.iter()) {
                unify(p, a, span, subst, struct_origins, enum_origins)?;
            }
            unify(rp, ra, span, subst, struct_origins, enum_origins)
        }
        // A plain function type unifies with a closure type — the function
        // value is wrappable as `MakeClosure { env: null, code: fn }` at the
        // call boundary via `coerce_to_closure`.
        (
            Ty::Closure {
                params: ps,
                return_ty: rp,
            },
            Ty::Function {
                params: as_,
                return_ty: ra,
                ..
            },
        )
        | (
            Ty::Function {
                params: as_,
                return_ty: ra,
                ..
            },
            Ty::Closure {
                params: ps,
                return_ty: rp,
            },
        ) if ps.len() == as_.len() => {
            for (p, a) in ps.iter().zip(as_.iter()) {
                unify(p, a, span, subst, struct_origins, enum_origins)?;
            }
            unify(rp, ra, span, subst, struct_origins, enum_origins)
        }
        (Ty::GenericStruct { name: n1, args: a1 }, Ty::GenericStruct { name: n2, args: a2 })
            if n1 == n2 && a1.len() == a2.len() =>
        {
            for (x, y) in a1.iter().zip(a2.iter()) {
                unify(x, y, span, subst, struct_origins, enum_origins)?;
            }
            Ok(())
        }
        (Ty::GenericStruct { name, args }, Ty::Struct(spec))
        | (Ty::Struct(spec), Ty::GenericStruct { name, args }) => match struct_origins.get(spec) {
            Some((tname, targs)) if tname == name && targs.len() == args.len() => {
                for (x, y) in args.iter().zip(targs.iter()) {
                    unify(x, y, span, subst, struct_origins, enum_origins)?;
                }
                Ok(())
            }
            _ => Err(Error {
                span,
                kind: ErrorKind::TypeMismatch {
                    expected: param_ty.clone(),
                    found: arg_ty.clone(),
                },
            }),
        },
        (Ty::GenericEnum { name: n1, args: a1 }, Ty::GenericEnum { name: n2, args: a2 })
            if n1 == n2 && a1.len() == a2.len() =>
        {
            for (x, y) in a1.iter().zip(a2.iter()) {
                unify(x, y, span, subst, struct_origins, enum_origins)?;
            }
            Ok(())
        }
        (Ty::GenericEnum { name, args }, Ty::Enum(spec))
        | (Ty::Enum(spec), Ty::GenericEnum { name, args }) => match enum_origins.get(spec) {
            Some((tname, targs)) if tname == name && targs.len() == args.len() => {
                for (x, y) in args.iter().zip(targs.iter()) {
                    unify(x, y, span, subst, struct_origins, enum_origins)?;
                }
                Ok(())
            }
            _ => Err(Error {
                span,
                kind: ErrorKind::TypeMismatch {
                    expected: param_ty.clone(),
                    found: arg_ty.clone(),
                },
            }),
        },
        (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() => {
            for (x, y) in a.iter().zip(b.iter()) {
                unify(x, y, span, subst, struct_origins, enum_origins)?;
            }
            Ok(())
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
