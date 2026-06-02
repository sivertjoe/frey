//! Generic template specialization: substituting type vars and emitting
//! concrete declarations for `Foo<Int>`-style use sites.

use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        TypeVarId,
        coerce::coerce_int_literal,
        error::{Error, ErrorKind},
        generics::{ty_has_typevars, unify},
        lower::{ Lower, mangle_specialization, mangle_struct_specialization,
            unit_expr,
        },
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, LocalId, Param, StatementKind,
            StructDef, Ty,
        },
    },
    lexer::types::Span,
};

impl Lower {
    pub(super) fn lower_type(&mut self, t: &ast::TypeExpr) -> Result<Ty, Error> {
        match &t.kind {
            ast::TypeExprKind::Int => Ok(Ty::Int),
            ast::TypeExprKind::UInt => Ok(Ty::UInt),
            ast::TypeExprKind::Float => Ok(Ty::Float),
            ast::TypeExprKind::I8 => Ok(Ty::I8),
            ast::TypeExprKind::I32 => Ok(Ty::I32),
            ast::TypeExprKind::I64 => Ok(Ty::I64),
            ast::TypeExprKind::U8 => Ok(Ty::U8),
            ast::TypeExprKind::U32 => Ok(Ty::U32),
            ast::TypeExprKind::U64 => Ok(Ty::U64),
            ast::TypeExprKind::F32 => Ok(Ty::F32),
            ast::TypeExprKind::F64 => Ok(Ty::F64),
            ast::TypeExprKind::Array { element_ty, count } => {
                let element = Box::new(self.lower_type(element_ty)?);
                Ok(Ty::Array {
                    element,
                    count: *count,
                })
            }
            ast::TypeExprKind::Function { params, return_ty } => {
                // User-written `(T) -> U` is the fat closure type. Extern
                // declarations build a thin `Ty::Function` directly elsewhere.
                let params = params
                    .iter()
                    .map(|p| self.lower_type(p))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_ty = Box::new(self.lower_type(return_ty)?);
                Ok(Ty::Closure { params, return_ty })
            }
            ast::TypeExprKind::Ptr(target) => {
                let target = Box::new(self.lower_type(target)?);
                Ok(Ty::Ptr(target))
            }
            ast::TypeExprKind::Named(name) => {
                if let Some(stripped) = name.strip_prefix('$') {
                    // `$T` — declares a fresh type variable in the current
                    // function's type-var scope. The same name appearing as
                    // bare `T` elsewhere in the same signature/body resolves
                    // to this same TypeVarId.
                    if self.structs.contains_key(stripped) {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::GenericIsAlsoAStruct {
                                name: stripped.to_string(),
                            },
                        });
                    }
                    let scope = self.type_var_scopes.last_mut().ok_or_else(|| Error {
                        span: t.span,
                        kind: ErrorKind::GenericOutsideFunctionSignature {
                            name: stripped.to_string(),
                        },
                    })?;
                    if scope.contains_key(stripped) {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::GenericAlreadyDefined {
                                name: stripped.to_string(),
                            },
                        });
                    }
                    let id = self.fresh_type_var_id(stripped.to_string());
                    self.type_var_scopes
                        .last_mut()
                        .unwrap()
                        .insert(stripped.to_string(), id);
                    Ok(Ty::TypeVar(id))
                } else if self.structs.contains_key(name) {
                    Ok(Ty::Struct(name.clone()))
                } else if self.enums.contains_key(name) {
                    Ok(Ty::Enum(name.clone()))
                } else if self.struct_templates.contains_key(name)
                    || self.enum_templates.contains_key(name)
                {
                    Err(Error {
                        span: t.span,
                        kind: ErrorKind::MissingTypeArguments { name: name.clone() },
                    })
                } else if let Some(id) = self.lookup_type_var(name) {
                    Ok(Ty::TypeVar(id))
                } else {
                    Err(Error {
                        span: t.span,
                        kind: ErrorKind::UnknownType { name: name.clone() },
                    })
                }
            }
            ast::TypeExprKind::NamedGeneric { name, args } => {
                // Concrete struct/enum? Then `Foo<...>` is wrong — Foo isn't generic.
                if self.structs.contains_key(name) || self.enums.contains_key(name) {
                    return Err(Error {
                        span: t.span,
                        kind: ErrorKind::UnexpectedTypeArguments { name: name.clone() },
                    });
                }
                if let Some(template) = self.struct_templates.get(name).cloned() {
                    if template.type_var_ids.len() != args.len() {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::TypeArgArityMismatch {
                                name: name.clone(),
                                expected: template.type_var_ids.len(),
                                found: args.len(),
                            },
                        });
                    }
                    let lowered_args: Vec<Ty> = args
                        .iter()
                        .map(|a| self.lower_type(a))
                        .collect::<Result<_, _>>()?;
                    let still_generic = lowered_args
                        .iter()
                        .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericStruct { .. }));
                    if still_generic {
                        return Ok(Ty::GenericStruct {
                            name: name.clone(),
                            args: lowered_args,
                        });
                    }
                    let specialized = self.specialize_struct(&template, lowered_args, t.span)?;
                    return Ok(Ty::Struct(specialized));
                }
                if let Some(template) = self.enum_templates.get(name).cloned() {
                    if template.type_var_ids.len() != args.len() {
                        return Err(Error {
                            span: t.span,
                            kind: ErrorKind::TypeArgArityMismatch {
                                name: name.clone(),
                                expected: template.type_var_ids.len(),
                                found: args.len(),
                            },
                        });
                    }
                    let lowered_args: Vec<Ty> = args
                        .iter()
                        .map(|a| self.lower_type(a))
                        .collect::<Result<_, _>>()?;
                    let still_generic = lowered_args
                        .iter()
                        .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericEnum { .. }));
                    if still_generic {
                        return Ok(Ty::GenericEnum {
                            name: name.clone(),
                            args: lowered_args,
                        });
                    }
                    let specialized = self.specialize_enum(&template, lowered_args, t.span)?;
                    return Ok(Ty::Enum(specialized));
                }
                Err(Error {
                    span: t.span,
                    kind: ErrorKind::UnknownType { name: name.clone() },
                })
            }
            ast::TypeExprKind::Tuple(elems) => {
                let lowered: Vec<Ty> = elems
                    .iter()
                    .map(|e| self.lower_type(e))
                    .collect::<Result<_, _>>()?;
                Ok(Ty::Tuple(lowered))
            }
        }
    }


    pub(super) fn specialize_struct(
        &mut self,
        template: &StructDef,
        args: Vec<Ty>,
        _span: Span,
    ) -> Result<String, Error> {
        let cache_key = (template.name.clone(), args.clone());
        if let Some(cached) = self.struct_specialization_cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let mangled = mangle_struct_specialization(&template.name, &args);
        // Reserve the cache slot BEFORE substituting so a self-referential
        // pointer field (`*Self<T>`) finds the in-progress entry.
        self.struct_specialization_cache
            .insert(cache_key, mangled.clone());
        self.struct_template_origin
            .insert(mangled.clone(), (template.name.clone(), args.clone()));

        let mut subst = HashMap::new();
        for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
            subst.insert(*tv, ty.clone());
        }

        let fields = template.fields.clone();
        let mut new_fields = Vec::with_capacity(fields.len());
        for (n, ty) in fields {
            new_fields.push((n, self.substitute_ty(&ty, &subst)));
        }

        self.structs.insert(
            mangled.clone(),
            StructDef {
                name: mangled.clone(),
                type_var_ids: Vec::new(),
                fields: new_fields,
            },
        );

        Ok(mangled)
    }

    /// Enum analogue of `specialize_struct`.
    pub(super) fn specialize_enum(
        &mut self,
        template: &crate::hir::types::EnumDef,
        args: Vec<Ty>,
        _span: Span,
    ) -> Result<String, Error> {
        let cache_key = (template.name.clone(), args.clone());
        if let Some(cached) = self.enum_specialization_cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let mangled = mangle_struct_specialization(&template.name, &args);
        self.enum_specialization_cache
            .insert(cache_key, mangled.clone());
        self.enum_template_origin
            .insert(mangled.clone(), (template.name.clone(), args.clone()));

        let mut subst = HashMap::new();
        for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
            subst.insert(*tv, ty.clone());
        }

        let variants = template.variants.clone();
        let mut new_variants = Vec::with_capacity(variants.len());
        for v in variants {
            let fields: Vec<Ty> = v
                .fields
                .iter()
                .map(|t| self.substitute_ty(t, &subst))
                .collect();
            new_variants.push(crate::hir::types::EnumVariant {
                name: v.name,
                fields,
            });
        }

        self.enums.insert(
            mangled.clone(),
            crate::hir::types::EnumDef {
                name: mangled.clone(),
                type_var_ids: Vec::new(),
                variants: new_variants,
            },
        );

        Ok(mangled)
    }

    /// Returns `Ok(None)` if `name` isn't a variant constructor — the caller
    /// then falls through to regular call lowering.

    pub(super) fn try_lower_variant_call(
        &mut self,
        name: &str,
        type_args: &[ast::TypeExpr],
        args: Vec<ast::Expr>,
        span: Span,
        hint: Option<&Ty>,
    ) -> Result<Option<Expr>, Error> {
        let Some(&(ref enum_template_name, variant_index)) = self.variant_constructors.get(name)
        else {
            return Ok(None);
        };
        let enum_template_name = enum_template_name.clone();

        // Non-generic enum: variant fields are already concrete.
        if let Some(enum_def) = self.enums.get(&enum_template_name).cloned() {
            if !type_args.is_empty() {
                return Err(Error {
                    span,
                    kind: ErrorKind::UnexpectedTypeArguments {
                        name: enum_template_name.clone(),
                    },
                });
            }
            let variant = &enum_def.variants[variant_index];
            if variant.fields.len() != args.len() {
                return Err(Error {
                    span,
                    kind: ErrorKind::VariantArityMismatch {
                        name: name.to_string(),
                        expected: variant.fields.len(),
                        found: args.len(),
                    },
                });
            }
            let mut lowered_args = Vec::with_capacity(args.len());
            for (arg, field_ty) in args.into_iter().zip(variant.fields.iter()) {
                let v = self.lower_expr_with_hint(arg, Some(field_ty))?;
                let v = coerce_int_literal(v, field_ty)?;
                if v.ty != *field_ty {
                    return Err(Error {
                        span: v.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: field_ty.clone(),
                            found: v.ty.clone(),
                        },
                    });
                }
                lowered_args.push(v);
            }
            return Ok(Some(Expr {
                span,
                ty: Ty::Enum(enum_def.name.clone()),
                kind: ExprKind::EnumConstruct {
                    enum_name: enum_def.name,
                    variant_index,
                    args: lowered_args,
                },
            }));
        }

        // Generic enum: unify args + explicit type args to infer the
        // substitution, then specialize.
        let template = self
            .enum_templates
            .get(&enum_template_name)
            .cloned()
            .expect("variant_constructors points to a known template");
        let variant = &template.variants[variant_index];
        if variant.fields.len() != args.len() {
            return Err(Error {
                span,
                kind: ErrorKind::VariantArityMismatch {
                    name: name.to_string(),
                    expected: variant.fields.len(),
                    found: args.len(),
                },
            });
        }

        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        if !type_args.is_empty() {
            if type_args.len() != template.type_var_ids.len() {
                return Err(Error {
                    span,
                    kind: ErrorKind::TypeArgArityMismatch {
                        name: enum_template_name.clone(),
                        expected: template.type_var_ids.len(),
                        found: type_args.len(),
                    },
                });
            }
            for (tv, t) in template.type_var_ids.iter().zip(type_args.iter()) {
                let ty = self.lower_type(t)?;
                subst.insert(*tv, ty);
            }
        }
        // Hint seeding is only used as a last resort below (when no args are
        // present to drive unification, or when args left some TypeVars
        // unbound). Seeding eagerly here would block unify from later
        // refining a TypeVar hint (e.g. `Some(v.get(0))` inside a `map(...)`).

        // Lower args, coerce against (currently-substituted) field type, unify.
        let mut lowered_args = Vec::with_capacity(args.len());
        for (arg, field_ty) in args.into_iter().zip(variant.fields.iter()) {
            let coerce_target = self.substitute_ty(field_ty, &subst);
            let arg_hint = (!ty_has_typevars(&coerce_target)).then(|| coerce_target.clone());
            let v = self.lower_expr_with_hint(arg, arg_hint.as_ref())?;
            let v = coerce_int_literal(v, &coerce_target)?;
            crate::hir::generics::unify(
                field_ty,
                &v.ty,
                v.span,
                &mut subst,
                &self.struct_template_origin,
                &self.enum_template_origin,
            )?;
            lowered_args.push(v);
        }

        // Fall back to the hint for any slot that arg-unification didn't bind.
        let hint_args = enum_type_args_from_hint(
            &enum_template_name,
            hint,
            &self.enum_template_origin,
        );
        let mut resolved_args = Vec::with_capacity(template.type_var_ids.len());
        for (i, tv) in template.type_var_ids.iter().enumerate() {
            let t = subst.get(tv).cloned().or_else(|| {
                hint_args
                    .as_ref()
                    .filter(|args| args.len() == template.type_var_ids.len())
                    .map(|args| args[i].clone())
            });
            let Some(t) = t else {
                return Err(Error {
                    span,
                    kind: ErrorKind::CannotInferEnumTypeArg {
                        variant: name.to_string(),
                    },
                });
            };
            resolved_args.push(t);
        }

        // Inside a generic function body, type args may still be TypeVars.
        // Defer specialization to substitute_expr: emit an EnumConstruct
        // tagged with the template name and a generic enum type. The
        // post-specialization walk re-mangles it once args are concrete.
        if resolved_args.iter().any(ty_has_typevars) {
            return Ok(Some(Expr {
                span,
                ty: Ty::GenericEnum {
                    name: enum_template_name.clone(),
                    args: resolved_args,
                },
                kind: ExprKind::EnumConstruct {
                    enum_name: enum_template_name,
                    variant_index,
                    args: lowered_args,
                },
            }));
        }

        let mangled = self.specialize_enum(&template, resolved_args, span)?;
        let def = self
            .enums
            .get(&mangled)
            .expect("specialize_enum inserts the definition");
        let coerced_args: Vec<Expr> = lowered_args
            .into_iter()
            .zip(def.variants[variant_index].fields.iter())
            .map(|(v, target)| coerce_int_literal(v, target))
            .collect::<Result<_, _>>()?;
        Ok(Some(Expr {
            span,
            ty: Ty::Enum(mangled.clone()),
            kind: ExprKind::EnumConstruct {
                enum_name: mangled,
                variant_index,
                args: coerced_args,
            },
        }))
    }


    pub(super) fn substitute_ty(&mut self, ty: &Ty, subst: &HashMap<TypeVarId, Ty>) -> Ty {
        match ty {
            Ty::TypeVar(id) => subst.get(id).cloned().unwrap_or(Ty::TypeVar(*id)),
            Ty::Ptr(inner) => Ty::Ptr(Box::new(self.substitute_ty(inner, subst))),
            Ty::Array { element, count } => Ty::Array {
                element: Box::new(self.substitute_ty(element, subst)),
                count: *count,
            },
            Ty::Function {
                params,
                return_ty,
                varargs,
            } => {
                let params: Vec<Ty> = params
                    .iter()
                    .map(|p| self.substitute_ty(p, subst))
                    .collect();
                let return_ty = Box::new(self.substitute_ty(return_ty, subst));
                Ty::Function {
                    params,
                    return_ty,
                    varargs: *varargs,
                }
            }
            Ty::Closure { params, return_ty } => {
                let params: Vec<Ty> = params
                    .iter()
                    .map(|p| self.substitute_ty(p, subst))
                    .collect();
                let return_ty = Box::new(self.substitute_ty(return_ty, subst));
                Ty::Closure { params, return_ty }
            }
            Ty::GenericStruct { name, args } => {
                let new_args: Vec<Ty> = args.iter().map(|a| self.substitute_ty(a, subst)).collect();
                let still_generic = new_args
                    .iter()
                    .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericStruct { .. }));
                if still_generic {
                    Ty::GenericStruct {
                        name: name.clone(),
                        args: new_args,
                    }
                } else {
                    let template = self
                        .struct_templates
                        .get(name)
                        .cloned()
                        .expect("GenericStruct must reference a known template");
                    let specialized = self
                        .specialize_struct(&template, new_args, Span::default())
                        .expect("specialize_struct cannot fail on concrete args");
                    Ty::Struct(specialized)
                }
            }
            Ty::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.substitute_ty(e, subst)).collect())
            }
            Ty::GenericEnum { name, args } => {
                let new_args: Vec<Ty> = args.iter().map(|a| self.substitute_ty(a, subst)).collect();
                let still_generic = new_args
                    .iter()
                    .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericEnum { .. }));
                if still_generic {
                    Ty::GenericEnum {
                        name: name.clone(),
                        args: new_args,
                    }
                } else {
                    let template = self
                        .enum_templates
                        .get(name)
                        .cloned()
                        .expect("GenericEnum must reference a known template");
                    let specialized = self
                        .specialize_enum(&template, new_args, Span::default())
                        .expect("specialize_enum cannot fail on concrete args");
                    Ty::Enum(specialized)
                }
            }
            _ => ty.clone(),
        }
    }


    pub(super) fn specialize_call(
        &mut self,
        template_id: LocalId,
        arg_tys: &[Ty],
        explicit_type_args: &[Ty],
        call_span: crate::lexer::types::Span,
    ) -> Result<LocalId, Error> {
        let template = self
            .templates
            .get(&template_id)
            .expect("specialize_call: template_id is not a generic template")
            .clone();

        // Build subst by unifying each (param, arg) pair.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();

        // Seed bindings from explicit type arguments, if provided.
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != template.type_var_ids.len() {
                return Err(Error {
                    span: call_span,
                    kind: ErrorKind::TypeArgArityMismatch {
                        name: template.name.clone(),
                        expected: template.type_var_ids.len(),
                        found: explicit_type_args.len(),
                    },
                });
            }
            for (tv, ty) in template.type_var_ids.iter().zip(explicit_type_args.iter()) {
                subst.insert(*tv, ty.clone());
            }
        }

        // For a call site we unify each arg against the corresponding param to
        // extend the substitution. For a function-reference specialization
        // there are no args — the explicit type args alone must cover all
        // type vars, and the check below catches it. So skip arg-unification
        // entirely when arg_tys is empty.
        if !arg_tys.is_empty() {
            if template.params.len() != arg_tys.len() {
                return Err(Error {
                    span: call_span,
                    kind: ErrorKind::TypeMismatch {
                        expected: Ty::Function {
                            params: template.params.iter().map(|p| p.ty.clone()).collect(),
                            return_ty: Box::new(template.return_ty.clone()),
                            varargs: false,
                        },
                        found: Ty::Function {
                            params: arg_tys.to_vec(),
                            return_ty: Box::new(Ty::Unit),
                            varargs: false,
                        },
                    },
                });
            }
            for (param, arg_ty) in template.params.iter().zip(arg_tys.iter()) {
                unify(
                    &param.ty,
                    arg_ty,
                    call_span,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                )?;
            }
        }

        // Make sure every type var declared by the template is bound.
        for tv_id in &template.type_var_ids {
            if !subst.contains_key(tv_id) {
                let name = self
                    .type_vars
                    .get(tv_id.0 as usize)
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| format!("T{}", tv_id.0));
                return Err(Error {
                    span: call_span,
                    kind: ErrorKind::CannotInferTypeArg { name },
                });
            }
        }

        let key_tys: Vec<Ty> = template
            .type_var_ids
            .iter()
            .map(|id| subst[id].clone())
            .collect();
        let cache_key = (template_id, key_tys.clone());
        if let Some(&cached) = self.specialization_cache.get(&cache_key) {
            return Ok(cached);
        }

        // Reserve the new id BEFORE walking the body so self-recursive calls
        // find the in-progress entry instead of re-specializing forever.
        let new_id = self.id_gen.fresh();
        self.specialization_cache.insert(cache_key, new_id);

        // Build the specialization signature.
        let mut new_params = Vec::with_capacity(template.params.len());
        let template_params = template.params.clone();
        for p in &template_params {
            let new_param_id = self.id_gen.fresh();
            let ty = self.substitute_ty(&p.ty, &subst);
            new_params.push(Param {
                id: new_param_id,
                span: p.span,
                name: p.name.clone(),
                ty,
            });
        }
        let new_return_ty = self.substitute_ty(&template.return_ty, &subst);
        let new_fn_ty = Ty::Function {
            params: new_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(new_return_ty.clone()),
            varargs: false,
        };
        self.bindings.insert(new_id, new_fn_ty.clone());

        // Walk and substitute the body. References to template params (by
        // old LocalId) get remapped to the new param LocalIds.
        let mut new_body = template.body.clone();
        let mut local_map: HashMap<LocalId, LocalId> = HashMap::new();
        for (p_old, p_new) in template_params.iter().zip(new_params.iter()) {
            local_map.insert(p_old.id, p_new.id);
        }
        // Comptime templates fold their type-introspection during this walk.
        let is_comptime = self.comptime_template_ids.contains(&template_id);
        let prev_comptime_subst = self.in_comptime_subst;
        self.in_comptime_subst = is_comptime;
        let subst_result = self.substitute_block(&mut new_body, &subst, &mut local_map);
        self.in_comptime_subst = prev_comptime_subst;
        subst_result?;

        // Build the specialization's Declaration. Compute the mangled name
        // up front so we don't keep borrowing `template.name` while
        // mutably calling substitute_block.
        let template_name = template.name.clone();
        let template_span = template.span;
        let template_env_param_id = template.env_param_id;
        drop(template);

        let mangled = mangle_specialization(&template_name, &key_tys);
        let value = Expr {
            span: template_span,
            ty: new_fn_ty.clone(),
            kind: ExprKind::Function(Function {
                params: new_params,
                return_ty: new_return_ty,
                body: new_body,
                // Closure-body code fns carry their env_param_id forward
                // unchanged — the body's references to it still resolve
                // since substitute_expr leaves the id alone (it's not in
                // `local_map`, which only covers template params).
                env_param_id: template_env_param_id,
            }),
        };
        let decl = Declaration {
            id: new_id,
            span: template_span,
            name: mangled,
            ty: new_fn_ty,
            value,
        };
        self.pending_specializations.push(decl);

        Ok(new_id)
    }


    pub(super) fn substitute_block(
        &mut self,
        block: &mut Block,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        if self.in_comptime_subst {
            // Process items until one diverges, then drop everything after it
            // (including the tail) as unreachable. This is what stops a
            // fall-through `comperror` from firing once an earlier static-if
            // branch has been selected and returns.
            let mut new_items = Vec::new();
            let mut diverged = false;
            for mut item in std::mem::take(&mut block.items) {
                self.substitute_block_item(&mut item, subst, local_map)?;
                let d = crate::hir::comptime::item_diverges(&item);
                new_items.push(item);
                if d {
                    diverged = true;
                    break;
                }
            }
            block.items = new_items;
            if diverged {
                block.tail = Box::new(unit_expr(block.span));
            } else {
                self.substitute_expr(&mut block.tail, subst, local_map)?;
            }
            return Ok(());
        }

        for item in &mut block.items {
            self.substitute_block_item(item, subst, local_map)?;
        }
        self.substitute_expr(&mut block.tail, subst, local_map)?;
        Ok(())
    }

    pub(super) fn substitute_block_item(
        &mut self,
        item: &mut BlockItem,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        match item {
            BlockItem::Declaration(d) => {
                d.ty = self.substitute_ty(&d.ty, subst);
                self.substitute_expr(&mut d.value, subst, local_map)?;
                // Local declarations introduce a fresh LocalId in the
                // specialization to keep ids distinct from the template's.
                let new_id = self.id_gen.fresh();
                local_map.insert(d.id, new_id);
                d.id = new_id;
                self.bindings.insert(new_id, d.ty.clone());
            }
            BlockItem::Statement(s) => match &mut s.kind {
                StatementKind::Return(e) => self.substitute_expr(e, subst, local_map)?,
                StatementKind::Expr(e) => self.substitute_expr(e, subst, local_map)?,
                StatementKind::Break => {}
                StatementKind::Defer(e) => self.substitute_expr(e, subst, local_map)?,
            },
        }
        Ok(())
    }

    pub(super) fn substitute_expr(
        &mut self,
        e: &mut Expr,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        let span = e.span;

        // Comptime `static if`: when the condition is comptime-known, fold to
        // the taken branch and discard the other, so its dead code (including
        // any `comperror`) is never evaluated.
        if self.in_comptime_subst {
            let mut replacement: Option<Expr> = None;
            if let ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } = &mut e.kind
            {
                self.substitute_expr(condition, subst, local_map)?;
                if let ExprKind::Const(Const::Int(n)) = &condition.kind {
                    let n = *n;
                    let taken = if n != 0 { then_branch } else { else_branch };
                    replacement = Some(std::mem::replace(taken.as_mut(), unit_expr(span)));
                }
            }
            if let Some(mut taken) = replacement {
                self.substitute_expr(&mut taken, subst, local_map)?;
                *e = taken;
                return Ok(());
            }
        }

        e.ty = self.substitute_ty(&e.ty, subst);
        // When a nested call is re-specialized, we need to update the Call
        // expression's result type to match the new specialization's return
        // type (its old ty was the template's return_ty, which may still
        // contain TypeVars that this substitution didn't bind).
        let mut updated_call_ty: Option<Ty> = None;
        // When a comptime comparison folds to a constant, stash it and rewrite
        // `e` once the `&mut e.kind` borrow below has ended.
        let mut comptime_const_fold: Option<i32> = None;
        match &mut e.kind {
            ExprKind::Const(_) => {}
            ExprKind::Local(id) => {
                if let Some(&new) = local_map.get(id) {
                    *id = new;
                } else if self.templates.contains_key(id) {
                    let template = self.templates.get(id).cloned().unwrap();
                    let template_args: Vec<Ty> = template
                        .type_var_ids
                        .iter()
                        .map(|tv| {
                            subst.get(tv).cloned().unwrap_or(Ty::TypeVar(*tv))
                        })
                        .collect();
                    if !template_args.iter().any(ty_has_typevars)
                        && template_args.len() == template.type_var_ids.len()
                    {
                        let new_id = self.specialize_call(*id, &[], &template_args, span)?;
                        *id = new_id;
                        if let Some(new_ty) = self.bindings.get(&new_id).cloned() {
                            e.ty = new_ty;
                        }
                    }
                }
            }
            ExprKind::Function(_) => {
                // Nested function literals aren't supported as generic
                // values; leave alone. If their body uses outer TypeVars
                // they'd need their own walk, but that's not on the table.
            }
            ExprKind::Call(call) => {
                self.substitute_expr(&mut call.callee, subst, local_map)?;
                for arg in &mut call.args {
                    self.substitute_expr(arg, subst, local_map)?;
                }
                // If the callee resolves to a generic template AND every arg
                // is concrete now, re-specialize.
                if let ExprKind::Local(callee_id) = call.callee.kind
                    && self.templates.contains_key(&callee_id)
                {
                    let arg_tys: Vec<Ty> = call.args.iter().map(|a| a.ty.clone()).collect();
                    if !arg_tys.iter().any(ty_has_typevars) {
                        let new_id = self.specialize_call(callee_id, &arg_tys, &[], e.span)?;
                        let new_fn_ty = self.bindings[&new_id].clone();
                        if let Ty::Function { return_ty, .. } = &new_fn_ty {
                            updated_call_ty = Some((**return_ty).clone());
                        }
                        call.callee.kind = ExprKind::Local(new_id);
                        call.callee.ty = new_fn_ty;
                    }
                }
            }
            ExprKind::Cast { target, expr } => {
                *target = self.substitute_ty(target, subst);
                self.substitute_expr(expr, subst, local_map)?;
            }
            ExprKind::Unary { operand, .. } => self.substitute_expr(operand, subst, local_map)?,
            ExprKind::Block(b) => self.substitute_block(b, subst, local_map)?,
            ExprKind::Binary { op, lhs, rhs } => {
                self.substitute_expr(lhs, subst, local_map)?;
                self.substitute_expr(rhs, subst, local_map)?;
                if self.in_comptime_subst
                    && let Some(crate::hir::comptime::CtValue::Bool(b)) =
                        crate::hir::comptime::eval_binary(*op, lhs, rhs)
                {
                    comptime_const_fold = Some(if b { 1 } else { 0 });
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.substitute_expr(condition, subst, local_map)?;
                self.substitute_expr(then_branch, subst, local_map)?;
                self.substitute_expr(else_branch, subst, local_map)?;
            }
            ExprKind::While { condition, body } => {
                self.substitute_expr(condition, subst, local_map)?;
                self.substitute_block(body, subst, local_map)?;
            }
            ExprKind::Assign { target, value } => {
                self.substitute_expr(target, subst, local_map)?;
                self.substitute_expr(value, subst, local_map)?;
            }
            ExprKind::Array(items) => {
                for item in items {
                    self.substitute_expr(item, subst, local_map)?;
                }
            }
            ExprKind::Subscript { expr, index } => {
                self.substitute_expr(expr, subst, local_map)?;
                self.substitute_expr(index, subst, local_map)?;
            }
            ExprKind::Ref(t) => self.substitute_expr(t, subst, local_map)?,
            ExprKind::Deref(t) => self.substitute_expr(t, subst, local_map)?,
            ExprKind::StructLiteral { fields } => {
                for (_, value) in fields {
                    self.substitute_expr(value, subst, local_map)?;
                }
            }
            ExprKind::Field { target, .. } => self.substitute_expr(target, subst, local_map)?,
            ExprKind::TypeValue(ty) => {
                *ty = self.substitute_ty(ty, subst);
            }
            ExprKind::CompError(message) => {
                return Err(Error {
                    span,
                    kind: ErrorKind::ComptimeError {
                        message: message.clone(),
                    },
                });
            }
            ExprKind::Intrinsic { elem_ty, args, .. } => {
                *elem_ty = self.substitute_ty(elem_ty, subst);
                for arg in args {
                    self.substitute_expr(arg, subst, local_map)?;
                }
            }
            ExprKind::Tuple(elems) => {
                for el in elems {
                    self.substitute_expr(el, subst, local_map)?;
                }
            }
            ExprKind::TupleField { target, .. } => {
                self.substitute_expr(target, subst, local_map)?;
            }
            ExprKind::EnumConstruct {
                enum_name,
                variant_index,
                args,
            } => {
                for arg in args.iter_mut() {
                    self.substitute_expr(arg, subst, local_map)?;
                }
                // If e.ty resolved to a concrete Ty::Enum (substitute_ty above
                // handles GenericEnum→Enum), point enum_name at the
                // specialization and coerce int-literal args against the now-
                // concrete variant field types.
                if let Ty::Enum(mangled) = &e.ty {
                    *enum_name = mangled.clone();
                    let variant_fields = self
                        .enums
                        .get(mangled)
                        .expect("substitute_ty registered this enum")
                        .variants[*variant_index]
                        .fields
                        .clone();
                    for (arg, target) in args.iter_mut().zip(variant_fields.iter()) {
                        let taken = std::mem::replace(arg, unit_expr(arg.span));
                        *arg = coerce_int_literal(taken, target)?;
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.substitute_expr(scrutinee, subst, local_map)?;
                for arm in arms {
                    if let crate::hir::types::HirPattern::Variant { bindings, .. } =
                        &mut arm.pattern
                    {
                        for (_, _, ty) in bindings {
                            *ty = self.substitute_ty(ty, subst);
                        }
                    }
                    self.substitute_expr(&mut arm.body, subst, local_map)?;
                }
            }
            ExprKind::MakeClosure { env, code } => {
                self.substitute_expr(env, subst, local_map)?;
                self.substitute_expr(code, subst, local_map)?;
            }
            ExprKind::ZeroInit(ty) => {
                *ty = self.substitute_ty(ty, subst);
            }
            ExprKind::ExternFunction { .. } => {
                // Extern decls never appear inside a generic body — they're
                // always top-level — so substitution can't reach them.
                unreachable!("ExternFunction never substitutes");
            }
            ExprKind::DeferredFunctionRef {
                template_id,
                type_args,
            } => {
                let substituted: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.substitute_ty(t, subst))
                    .collect();
                if substituted.iter().any(ty_has_typevars) {
                    *type_args = substituted;
                } else {
                    let span = e.span;
                    let specialized = self.specialize_call(
                        *template_id,
                        &[],
                        &substituted,
                        span,
                    )?;
                    let fn_ty = self.bindings[&specialized].clone();
                    e.kind = ExprKind::Local(specialized);
                    e.ty = fn_ty;
                }
            }
        }
        if let Some(ty) = updated_call_ty {
            e.ty = ty;
        }
        if let Some(n) = comptime_const_fold {
            e.kind = ExprKind::Const(Const::Int(n));
            e.ty = Ty::Int;
        }
        Ok(())
    }
}

fn enum_type_args_from_hint(
    template_name: &str,
    hint: Option<&Ty>,
    enum_origins: &HashMap<String, (String, Vec<Ty>)>,
) -> Option<Vec<Ty>> {
    let hint = hint?;
    match hint {
        Ty::Enum(spec) => enum_origins
            .get(spec)
            .filter(|(tname, _)| tname == template_name)
            .map(|(_, args)| args.clone()),
        Ty::GenericEnum { name, args } if name == template_name => Some(args.clone()),
        _ => None,
    }
}

