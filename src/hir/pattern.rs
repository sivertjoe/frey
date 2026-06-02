//! Pattern matching: `match` expressions and variant pattern lowering.

use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        TypeVarId,
        coerce::coerce_through_tails,
        error::{Error, ErrorKind},
        lower::Lower,
        types::{
            Expr, ExprKind, LocalId, Ty,
        },
    },
    lexer::types::Span,
};

impl Lower {
    pub(super) fn lower_match(
        &mut self,
        scrutinee: ast::Expr,
        arms: Vec<ast::MatchArm>,
        span: Span,
        hint: Option<&Ty>,
    ) -> Result<Expr, Error> {
        let scrutinee = self.lower_expr(scrutinee)?;
        // Inside a generic body the scrutinee is `GenericEnum<T>`; we match
        // against the template's variants with payload types substituted.
        // Specialization (later) updates binding types through substitute_expr.
        let (enum_def, subst): (crate::hir::types::EnumDef, HashMap<TypeVarId, Ty>) =
            match &scrutinee.ty {
                Ty::Enum(n) => (
                    self.enums
                        .get(n)
                        .cloned()
                        .expect("Ty::Enum must reference a known enum"),
                    HashMap::new(),
                ),
                Ty::GenericEnum { name, args } => {
                    let template = self
                        .enum_templates
                        .get(name)
                        .cloned()
                        .expect("GenericEnum must reference a known template");
                    let mut subst = HashMap::new();
                    for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
                        subst.insert(*tv, ty.clone());
                    }
                    let mut substituted = template.clone();
                    for v in &mut substituted.variants {
                        v.fields = v
                            .fields
                            .iter()
                            .map(|t| self.substitute_ty(t, &subst))
                            .collect();
                    }
                    (substituted, subst)
                }
                other => {
                    return Err(Error {
                        span: scrutinee.span,
                        kind: ErrorKind::MatchOnNonEnum {
                            found: other.clone(),
                        },
                    });
                }
            };
        let _ = subst; // Already applied to enum_def above.

        let mut hir_arms: Vec<crate::hir::types::MatchArm> = Vec::with_capacity(arms.len());
        let mut covered: Vec<bool> = vec![false; enum_def.variants.len()];
        let mut has_wildcard = false;
        let mut result_ty: Option<Ty> = None;

        for arm in arms {
            let arm_span = arm.span;
            let (pattern, binding_locals) =
                self.lower_pattern(arm.pattern, &enum_def, &mut covered, &mut has_wildcard)?;
            self.enter_scope();
            for (name, local_id, ty) in &binding_locals {
                self.current_scope_mut().insert(name.clone(), *local_id);
                self.bindings.insert(*local_id, ty.clone());
            }
            // Arm bodies see (in priority order): the first arm's type once
            // we have it, then the outer hint.
            let arm_hint = result_ty.as_ref().or(hint);
            let body = self.lower_expr_with_hint(arm.body, arm_hint);
            self.leave_scope();
            let body = body?;

            // Widen int-literal arms to match the first arm's type.
            let body = if let Some(rt) = &result_ty {
                coerce_through_tails(body, rt)?
            } else {
                body
            };

            if let Some(rt) = &result_ty {
                if body.ty != *rt {
                    return Err(Error {
                        span: body.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: rt.clone(),
                            found: body.ty.clone(),
                        },
                    });
                }
            } else {
                result_ty = Some(body.ty.clone());
            }

            hir_arms.push(crate::hir::types::MatchArm {
                span: arm_span,
                pattern,
                body,
            });
        }

        if !has_wildcard {
            let missing: Vec<String> = enum_def
                .variants
                .iter()
                .zip(covered.iter())
                .filter_map(|(v, &c)| (!c).then(|| v.name.clone()))
                .collect();
            if !missing.is_empty() {
                return Err(Error {
                    span,
                    kind: ErrorKind::NonExhaustiveMatch { missing },
                });
            }
        }

        let ty = result_ty.unwrap_or(Ty::Unit);
        Ok(Expr {
            span,
            ty,
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms: hir_arms,
            },
        })
    }

    pub(super) fn lower_pattern(
        &mut self,
        pattern: ast::Pattern,
        enum_def: &crate::hir::types::EnumDef,
        covered: &mut [bool],
        has_wildcard: &mut bool,
    ) -> Result<(crate::hir::types::HirPattern, Vec<(String, LocalId, Ty)>), Error> {
        let p_span = pattern.span;
        match pattern.kind {
            ast::PatternKind::Wildcard => {
                if *has_wildcard {
                    return Err(Error {
                        span: p_span,
                        kind: ErrorKind::DuplicateMatchArm {
                            name: "_".to_string(),
                        },
                    });
                }
                *has_wildcard = true;
                Ok((crate::hir::types::HirPattern::Wildcard, Vec::new()))
            }
            ast::PatternKind::Binding(name) => {
                // A bare identifier names a variant; catch-all bindings use `_`.
                self.resolve_variant_pattern(&name, Vec::new(), enum_def, covered, p_span)
            }
            ast::PatternKind::Variant { name, bindings } => {
                self.resolve_variant_pattern(&name, bindings, enum_def, covered, p_span)
            }
        }
    }

    pub(super) fn resolve_variant_pattern(
        &mut self,
        name: &str,
        bindings: Vec<String>,
        enum_def: &crate::hir::types::EnumDef,
        covered: &mut [bool],
        p_span: Span,
    ) -> Result<(crate::hir::types::HirPattern, Vec<(String, LocalId, Ty)>), Error> {
        let (idx, variant) = enum_def
            .variants
            .iter()
            .enumerate()
            .find(|(_, v)| v.name == name)
            .ok_or_else(|| Error {
                span: p_span,
                kind: ErrorKind::UnknownVariant {
                    name: name.to_string(),
                },
            })?;
        if variant.fields.len() != bindings.len() {
            return Err(Error {
                span: p_span,
                kind: ErrorKind::VariantArityMismatch {
                    name: name.to_string(),
                    expected: variant.fields.len(),
                    found: bindings.len(),
                },
            });
        }
        if covered[idx] {
            return Err(Error {
                span: p_span,
                kind: ErrorKind::DuplicateMatchArm {
                    name: name.to_string(),
                },
            });
        }
        covered[idx] = true;
        let mut binding_locals = Vec::with_capacity(bindings.len());
        for (binding_name, field_ty) in bindings.iter().zip(variant.fields.iter()) {
            let local_id = self.id_gen.fresh();
            binding_locals.push((binding_name.clone(), local_id, field_ty.clone()));
        }
        let bindings_for_hir: Vec<(String, LocalId, Ty)> = binding_locals.clone();
        Ok((
            crate::hir::types::HirPattern::Variant {
                enum_name: enum_def.name.clone(),
                variant_index: idx,
                bindings: bindings_for_hir,
            },
            binding_locals,
        ))
    }

}
