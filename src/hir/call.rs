//! Call lowering: regular calls, UFCS method calls, overload resolution,
//! receiver auto-ref/auto-deref, and the closure-hint two-pass.

use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        TypeVarId,
        coerce::coerce_int_literal,
        error::{Error, ErrorKind},
        generics::{ty_has_typevars, unify},
        lower::{ Lower, is_place_kind,
        },
        types::{
            Expr, ExprKind, FunctionCall, LocalId, Ty,
        },
    },
    lexer::types::Span,
};

impl Lower {
    pub(super) fn finish_call(
        &mut self,
        callee: Expr,
        args: Vec<Expr>,
        explicit_type_args: Vec<Ty>,
        span: Span,
    ) -> Result<Expr, Error> {
        let (params, return_ty) = match callee.ty.clone() {
            Ty::Function {
                params, return_ty, ..
            } => (params, return_ty),
            Ty::Closure { params, return_ty } => (params, return_ty),
            _ => {
                return Err(Error {
                    span: callee.span,
                    kind: ErrorKind::NotCallable {
                        found: callee.ty.clone(),
                    },
                });
            }
        };

        // Coerce int literals against the (possibly generic) param type;
        // coerce_int_literal is a no-op when the target is a TypeVar.
        // Also wrap raw function values as closures when the param expects a
        // closure (e.g. an `(Int)->Int` arg flowing into a `(T)->U` closure
        // param): the dispatch path for multi-overload calls lowers args
        // without hints, so coercion has to happen here once we know which
        // overload was picked.
        let mut coerced = Vec::with_capacity(args.len());
        for (i, arg) in args.into_iter().enumerate() {
            let arg = match params.get(i) {
                Some(pty) => coerce_int_literal(arg, pty)?,
                None => arg,
            };
            let arg = match params.get(i) {
                Some(pty) => {
                    let span = arg.span;
                    self.coerce_to_closure(arg, pty, span)
                }
                None => arg,
            };
            coerced.push(arg);
        }
        let args = coerced;

        let callee_local = if let ExprKind::Local(id) = callee.kind {
            Some(id)
        } else {
            None
        };
        if let Some(callee_id) = callee_local
            && self.templates.contains_key(&callee_id)
        {
            let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
            let concrete = !arg_tys.iter().any(ty_has_typevars)
                && !explicit_type_args.iter().any(ty_has_typevars);
            if concrete {
                let new_id = self.specialize_call(callee_id, &arg_tys, &explicit_type_args, span)?;
                let new_fn_ty = self.bindings[&new_id].clone();
                let Ty::Function {
                    return_ty: spec_ret,
                    ..
                } = &new_fn_ty
                else {
                    unreachable!("specialized function must have function type");
                };
                let result_ty = (**spec_ret).clone();
                return Ok(Expr {
                    span,
                    ty: result_ty,
                    kind: ExprKind::Call(FunctionCall {
                        callee: Box::new(Expr {
                            span: callee.span,
                            ty: new_fn_ty,
                            kind: ExprKind::Local(new_id),
                        }),
                        args,
                    }),
                });
            }
        }

        // Couldn't fully specialize, but if the callee is a generic template
        // we can still unify the partial info — any TypeVars we *do* learn
        // get substituted into the return type. Without this, the call's
        // result type stays as the template's bare TypeVar(T), which means
        // `match self.vec.get(i)` sees `T` instead of the actual `Bucket<K, V>`.
        let result_ty = if let Some(callee_id) = callee_local
            && let Some(template) = self.templates.get(&callee_id).cloned()
            && template.params.len() == args.len()
        {
            let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
            if explicit_type_args.len() == template.type_var_ids.len() {
                for (tv, t) in template
                    .type_var_ids
                    .iter()
                    .zip(explicit_type_args.iter())
                {
                    subst.insert(*tv, t.clone());
                }
            }
            for (param, arg) in template.params.iter().zip(args.iter()) {
                let _ = unify(
                    &param.ty,
                    &arg.ty,
                    span,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                );
            }
            self.substitute_ty(&template.return_ty, &subst)
        } else {
            *return_ty
        };
        Ok(Expr {
            span,
            ty: result_ty,
            kind: ExprKind::Call(FunctionCall {
                callee: Box::new(callee),
                args,
            }),
        })
    }

    /// Lowers `recv.name(args)`. If `name` is a (function-typed) field of the
    /// receiver's struct it's a field call; otherwise it desugars to the
    /// uniform-call form `name(recv, args)` (Nim-style UFCS), auto-referencing
    /// the receiver when the function expects a pointer.
    pub(super) fn lower_method_call(
        &mut self,
        callee: ast::Expr,
        type_args: Vec<ast::TypeExpr>,
        args: Vec<ast::Expr>,
        span: Span,
    ) -> Result<Expr, Error> {
        let ast::ExprKind::Field { target, name } = callee.kind else {
            unreachable!("lower_method_call requires a field callee");
        };
        let recv = self.lower_expr(*target)?;
        let explicit_type_args: Vec<Ty> = type_args
            .iter()
            .map(|t| self.lower_type(t))
            .collect::<Result<_, _>>()?;

        // `recv.field(args)` — only when the field actually holds a function
        // value. A non-callable field (e.g. `s.len` is an `Int`) falls
        // through to UFCS instead, so `s.len()` can call a top-level `len`
        // that takes `s` as its receiver.
        if let Some((index, field_ty)) = self.resolve_field(&recv.ty, &name)
            && matches!(field_ty, Ty::Function { .. } | Ty::Closure { .. })
        {
            let target = self.autoderef_to_struct(recv);
            let callee = Expr {
                span: target.span,
                ty: field_ty,
                kind: ExprKind::Field {
                    target: Box::new(target),
                    name,
                    index,
                },
            };
            let args = self.lower_call_args(&callee, args, &explicit_type_args, span)?;
            return self.finish_call(callee, args, explicit_type_args, span);
        }

        // UFCS: resolve `name` (possibly overloaded) against the receiver and
        // arguments, then call it as `name(recv, args)`. When an arg is a
        // closure literal, route through the two-pass helper so it can pick
        // up its parameter types from the narrowed signature.
        let has_closures = args
            .iter()
            .any(|a| matches!(a.kind, ast::ExprKind::Closure { .. }));
        let lowered_args: Vec<Expr> = if has_closures {
            self.lower_method_call_args_with_closures(&name, &recv, args)?
        } else {
            args.into_iter()
                .map(|a| self.lower_expr(a))
                .collect::<Result<_, _>>()?
        };
        let other_arg_tys: Vec<Ty> = lowered_args.iter().map(|a| a.ty.clone()).collect();
        let (fn_id, recv) = self.resolve_method(&name, recv, &other_arg_tys, span)?;
        let callee = Expr {
            span,
            ty: self.bindings[&fn_id].clone(),
            kind: ExprKind::Local(fn_id),
        };
        let mut all_args = Vec::with_capacity(lowered_args.len() + 1);
        all_args.push(recv);
        all_args.extend(lowered_args);
        self.finish_call(callee, all_args, explicit_type_args, span)
    }

    /// Looks up `name` as a field of `ty` (after auto-dereferencing pointers to
    /// a struct), returning its index and substituted type. None if `ty` is not
    /// a struct or has no such field.
    pub(super) fn resolve_field(&mut self, ty: &Ty, name: &str) -> Option<(usize, Ty)> {
        let mut t = ty.clone();
        while let Ty::Ptr(inner) = t {
            t = *inner;
        }
        let fields: Vec<(String, Ty)> = match t {
            Ty::Struct(n) => self.structs.get(&n)?.fields.clone(),
            Ty::GenericStruct { name: sname, args } => {
                let template = self.struct_templates.get(&sname)?.clone();
                let subst: HashMap<TypeVarId, Ty> = template
                    .type_var_ids
                    .iter()
                    .zip(args.iter())
                    .map(|(tv, ty)| (*tv, ty.clone()))
                    .collect();
                template
                    .fields
                    .iter()
                    .map(|(fname, fty)| (fname.clone(), self.substitute_ty(fty, &subst)))
                    .collect()
            }
            _ => return None,
        };
        fields
            .iter()
            .enumerate()
            .find_map(|(i, (n, t))| (n.as_str() == name).then(|| (i, t.clone())))
    }

    /// Wraps `target` in a chain of `Deref`s until it names a struct value,
    /// mirroring field-access auto-deref.
    pub(super) fn autoderef_to_struct(&self, mut target: Expr) -> Expr {
        while let Ty::Ptr(inner) = target.ty.clone() {
            let pointee_ty = *inner;
            target = Expr {
                span: target.span,
                ty: pointee_ty,
                kind: ExprKind::Deref(Box::new(target)),
            };
        }
        target
    }

    /// For a UFCS receiver: if the function's first parameter is a pointer and
    /// the receiver is an addressable non-pointer, take its address.
    pub(super) fn maybe_autoref(&self, recv: Expr, fn_ty: &Ty) -> Expr {
        if let Ty::Function { params, .. } = fn_ty
            && matches!(params.first(), Some(Ty::Ptr(_)))
            && !matches!(recv.ty, Ty::Ptr(_))
            && is_place_kind(&recv.kind)
        {
            let ty = Ty::Ptr(Box::new(recv.ty.clone()));
            let span = recv.span;
            return Expr {
                span,
                ty,
                kind: ExprKind::Ref(Box::new(recv)),
            };
        }
        recv
    }

    /// The parameter types of function `id`, or empty if it isn't a function.

    pub(super) fn lower_method_call_args_with_closures(
        &mut self,
        name: &str,
        recv: &Expr,
        args: Vec<ast::Expr>,
    ) -> Result<Vec<Expr>, Error> {
        let n = args.len();
        let is_closure: Vec<bool> = args
            .iter()
            .map(|a| matches!(a.kind, ast::ExprKind::Closure { .. }))
            .collect();
        let mut slots: Vec<Option<Expr>> = (0..n).map(|_| None).collect();
        let mut pending: Vec<(usize, ast::Expr)> = Vec::new();
        for (i, a) in args.into_iter().enumerate() {
            if is_closure[i] {
                pending.push((i, a));
            } else {
                slots[i] = Some(self.lower_expr(a)?);
            }
        }
        let known: Vec<Option<Ty>> = slots
            .iter()
            .map(|s| s.as_ref().map(|e| e.ty.clone()))
            .collect();
        let hints = self.arg_hints_from_receiver(name, recv, &known);
        for (i, a) in pending {
            let hint = hints.as_ref().and_then(|h| h.get(i));
            slots[i] = Some(self.lower_expr_with_hint(a, hint)?);
        }
        Ok(slots.into_iter().map(|s| s.expect("filled")).collect())
    }

    /// Two-pass lowering of arguments for an overloaded direct call when some
    /// of them are closure literals: lower the non-closure args first, narrow
    /// candidates from their types via `arg_hints_from_args`, then lower
    /// the closures with the resulting per-position hints.
    pub(super) fn lower_overloaded_call_args_with_closures(
        &mut self,
        name: &str,
        args: Vec<ast::Expr>,
    ) -> Result<Vec<Expr>, Error> {
        let n = args.len();
        let is_closure: Vec<bool> = args
            .iter()
            .map(|a| matches!(a.kind, ast::ExprKind::Closure { .. }))
            .collect();
        let mut slots: Vec<Option<Expr>> = (0..n).map(|_| None).collect();
        let mut pending: Vec<(usize, ast::Expr)> = Vec::new();
        for (i, a) in args.into_iter().enumerate() {
            if is_closure[i] {
                pending.push((i, a));
            } else {
                slots[i] = Some(self.lower_expr(a)?);
            }
        }
        let known: Vec<Option<Ty>> = slots
            .iter()
            .map(|s| s.as_ref().map(|e| e.ty.clone()))
            .collect();
        let hints = self.arg_hints_from_args(name, &known);
        for (i, a) in pending {
            let hint = hints.as_ref().and_then(|h| h.get(i));
            slots[i] = Some(self.lower_expr_with_hint(a, hint)?);
        }
        Ok(slots.into_iter().map(|s| s.expect("filled")).collect())
    }

    /// Whether `params` accept `arg_tys`: arity matches and each pair unifies,
    /// treating the parameters' type vars as inference holes.
    pub(super) fn overload_matches(&self, params: &[Ty], arg_tys: &[Ty]) -> bool {
        if params.len() != arg_tys.len() {
            return false;
        }
        let mut subst = HashMap::new();
        for (p, a) in params.iter().zip(arg_tys.iter()) {
            // `unify(param, arg)` binds TypeVars on the param side. Inside a
            // generic function body the arg's *own* TypeVars also need to be
            // allowed to "match anything" — swap so unify always sees the
            // typevar-bearing side on the left.
            let (lhs, rhs) = if !ty_has_typevars(p) && ty_has_typevars(a) {
                (a, p)
            } else {
                (p, a)
            };
            if unify(
                lhs,
                rhs,
                Span::default(),
                &mut subst,
                &self.struct_template_origin,
                &self.enum_template_origin,
            )
            .is_err()
            {
                return false;
            }
        }
        true
    }

    /// Like `arg_hints_from_receiver` but without a receiver — pre-narrows
    /// overloads from already-lowered arg types alone. `None` entries mark
    /// not-yet-lowered positions (closures) and skip unification there.
    pub(super) fn arg_hints_from_args(
        &mut self,
        name: &str,
        known_arg_tys: &[Option<Ty>],
    ) -> Option<Vec<Ty>> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        if candidates.is_empty() {
            return None;
        }
        let n_args = known_arg_tys.len();
        let mut matched: Vec<(Vec<Ty>, HashMap<TypeVarId, Ty>)> = Vec::new();
        for id in &candidates {
            let params = self.fn_params(*id);
            if params.len() != n_args {
                continue;
            }
            let mut subst = HashMap::new();
            let mut ok = true;
            for (param, known) in params.iter().zip(known_arg_tys.iter()) {
                let Some(arg_ty) = known else { continue };
                if !try_unify_one(
                    arg_ty,
                    param,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                ) {
                    ok = false;
                    break;
                }
            }
            if ok {
                matched.push((params.clone(), subst));
            }
        }
        if matched.len() != 1 {
            return None;
        }
        let (params, subst) = matched.into_iter().next().unwrap();
        Some(
            params
                .iter()
                .map(|p| self.substitute_ty(p, &subst))
                .collect(),
        )
    }

    /// Pre-narrows overloads of `name` using the receiver and (optionally)
    /// already-lowered other args. If exactly one candidate matches, returns
    /// its non-receiver param types (positions 1..N) with all derived
    /// TypeVars substituted. Holes in `known_other_arg_tys` (None) skip that
    /// position for unification — used for args not yet lowered, e.g. closures.
    pub(super) fn arg_hints_from_receiver(
        &mut self,
        name: &str,
        recv: &Expr,
        known_other_arg_tys: &[Option<Ty>],
    ) -> Option<Vec<Ty>> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        if candidates.is_empty() {
            return None;
        }
        let forms = self.receiver_forms(recv);
        let n_args = known_other_arg_tys.len();
        let mut matched: Vec<(Vec<Ty>, HashMap<TypeVarId, Ty>)> = Vec::new();
        for id in &candidates {
            let params = self.fn_params(*id);
            if params.len() != n_args + 1 {
                continue;
            }
            for (form_ty, _adjust) in &forms {
                let mut subst = HashMap::new();
                if !try_unify_one(
                    form_ty,
                    &params[0],
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                ) {
                    continue;
                }
                let mut ok = true;
                for (param, known) in params.iter().skip(1).zip(known_other_arg_tys.iter()) {
                    let Some(arg_ty) = known else { continue };
                    if !try_unify_one(
                        arg_ty,
                        param,
                        &mut subst,
                        &self.struct_template_origin,
                        &self.enum_template_origin,
                    ) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    matched.push((params.clone(), subst));
                    break;
                }
            }
        }
        if matched.len() != 1 {
            return None;
        }
        let (params, subst) = matched.into_iter().next().unwrap();
        Some(
            params
                .iter()
                .skip(1)
                .map(|p| self.substitute_ty(p, &subst))
                .collect(),
        )
    }

    /// Picks the unique overload of `name` whose parameters accept `arg_tys`.
    pub(super) fn resolve_overload(&self, name: &str, arg_tys: &[Ty], span: Span) -> Result<LocalId, Error> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        let mut matched: Vec<LocalId> = Vec::new();
        for id in candidates {
            if self.overload_matches(&self.fn_params(id), arg_tys) {
                matched.push(id);
            }
        }
        match matched.as_slice() {
            [id] => Ok(*id),
            [] => Err(Error {
                span,
                kind: ErrorKind::NoMatchingOverload {
                    name: name.to_string(),
                },
            }),
            _ => Err(Error {
                span,
                kind: ErrorKind::AmbiguousOverload {
                    name: name.to_string(),
                },
            }),
        }
    }

    /// Resolves a UFCS method `name`, choosing the overload that accepts the
    /// receiver (as-is, auto-referenced, or auto-dereferenced) plus the other
    /// arguments. Returns the chosen function and the adjusted receiver.
    pub(super) fn resolve_method(
        &mut self,
        name: &str,
        recv: Expr,
        other_arg_tys: &[Ty],
        span: Span,
    ) -> Result<(LocalId, Expr), Error> {
        let candidates = self.overloads.get(name).cloned().unwrap_or_default();
        if candidates.is_empty() {
            // Not a top-level function — maybe a local holding a function value.
            let Some(id) = self.resolve(name) else {
                return Err(Error {
                    span,
                    kind: ErrorKind::NameNotFound {
                        name: name.to_string(),
                    },
                });
            };
            let fn_ty = self
                .bindings
                .get(&id)
                .cloned()
                .expect("resolved name must have a recorded type");
            let recv = self.maybe_autoref(recv, &fn_ty);
            return Ok((id, recv));
        }

        let forms = self.receiver_forms(&recv);
        let mut matched: Vec<(LocalId, RecvAdjust)> = Vec::new();
        for id in &candidates {
            let params = self.fn_params(*id);
            if params.is_empty() {
                continue;
            }
            for (form_ty, adjust) in &forms {
                let mut full = Vec::with_capacity(other_arg_tys.len() + 1);
                full.push(form_ty.clone());
                full.extend_from_slice(other_arg_tys);
                if self.overload_matches(&params, &full) {
                    matched.push((*id, *adjust));
                    break;
                }
            }
        }
        match matched.as_slice() {
            [(id, adjust)] => Ok((*id, self.apply_recv_adjust(recv, *adjust))),
            [] => Err(Error {
                span,
                kind: ErrorKind::NoMatchingOverload {
                    name: name.to_string(),
                },
            }),
            _ => Err(Error {
                span,
                kind: ErrorKind::AmbiguousOverload {
                    name: name.to_string(),
                },
            }),
        }
    }

    /// Receiver forms to try for UFCS dispatch, in preferred order: as-is, a
    /// reference (if addressable), then a dereference (if it's a pointer).
    pub(super) fn receiver_forms(&self, recv: &Expr) -> Vec<(Ty, RecvAdjust)> {
        let mut forms = vec![(recv.ty.clone(), RecvAdjust::AsIs)];
        if is_place_kind(&recv.kind) {
            forms.push((Ty::Ptr(Box::new(recv.ty.clone())), RecvAdjust::Ref));
        }
        if let Ty::Ptr(inner) = &recv.ty {
            forms.push(((**inner).clone(), RecvAdjust::Deref));
        }
        forms
    }

    pub(super) fn apply_recv_adjust(&self, recv: Expr, adjust: RecvAdjust) -> Expr {
        match adjust {
            RecvAdjust::AsIs => recv,
            RecvAdjust::Ref => {
                let ty = Ty::Ptr(Box::new(recv.ty.clone()));
                let span = recv.span;
                Expr {
                    span,
                    ty,
                    kind: ExprKind::Ref(Box::new(recv)),
                }
            }
            RecvAdjust::Deref => {
                let Ty::Ptr(inner) = recv.ty.clone() else {
                    unreachable!("Deref adjustment requires a pointer receiver");
                };
                let span = recv.span;
                Expr {
                    span,
                    ty: *inner,
                    kind: ExprKind::Deref(Box::new(recv)),
                }
            }
        }
    }


    pub(super) fn lower_call_args(
        &mut self,
        callee: &Expr,
        args: Vec<ast::Expr>,
        explicit_type_args: &[Ty],
        _span: Span,
    ) -> Result<Vec<Expr>, Error> {
        let param_types: Vec<Ty> = match &callee.ty {
            Ty::Function { params, .. } => params.clone(),
            Ty::Closure { params, .. } => params.clone(),
            _ => return self.lower_args_no_hints(args),
        };

        // Seed substitution from the callee template (if any) and explicit
        // type args. This lets `map<Int, Int>(...)` immediately substitute T
        // and U in the param types before we hand them down as hints.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        if let ExprKind::Local(id) = &callee.kind
            && let Some(template) = self.templates.get(id)
            && explicit_type_args.len() == template.type_var_ids.len()
        {
            for (tv, ty) in template
                .type_var_ids
                .iter()
                .zip(explicit_type_args.iter())
            {
                subst.insert(*tv, ty.clone());
            }
        }

        let n = args.len();
        let mut args_opts: Vec<Option<ast::Expr>> = args.into_iter().map(Some).collect();
        let mut lowered: Vec<Option<Expr>> = (0..n).map(|_| None).collect();

        // Pass 1: lower non-closure args, extending subst as we go.
        for i in 0..n {
            let is_closure = matches!(
                args_opts[i].as_ref().map(|a| &a.kind),
                Some(ast::ExprKind::Closure { .. })
            );
            if is_closure {
                continue;
            }
            let arg = args_opts[i].take().unwrap();
            let param_ty = param_types.get(i).cloned();
            let hint = param_ty.as_ref().map(|p| self.substitute_ty(p, &subst));
            // Forward the hint even if it still has TypeVars — variant
            // constructors can emit a deferred (GenericEnum-typed)
            // EnumConstruct, and everyone else ignores the hint anyway.
            let v = self.lower_expr_with_hint(arg, hint.as_ref())?;
            let v = if let Some(p) = hint.as_ref() {
                let span = v.span;
                self.coerce_to_closure(v, p, span)
            } else {
                v
            };
            if let Some(p) = &param_ty {
                let _ = crate::hir::generics::unify(
                    p,
                    &v.ty,
                    v.span,
                    &mut subst,
                    &self.struct_template_origin,
                    &self.enum_template_origin,
                );
            }
            lowered[i] = Some(v);
        }

        // Pass 2: closures, with hints refined by pass 1's substitution.
        for i in 0..n {
            if lowered[i].is_some() {
                continue;
            }
            let arg = args_opts[i].take().unwrap();
            let param_ty = param_types.get(i).cloned();
            let hint = param_ty.as_ref().map(|p| self.substitute_ty(p, &subst));
            let v = self.lower_expr_with_hint(arg, hint.as_ref())?;
            let v = if let Some(p) = hint.as_ref() {
                let span = v.span;
                self.coerce_to_closure(v, p, span)
            } else {
                v
            };
            lowered[i] = Some(v);
        }

        Ok(lowered.into_iter().map(Option::unwrap).collect())
    }

    pub(super) fn lower_args_no_hints(&mut self, args: Vec<ast::Expr>) -> Result<Vec<Expr>, Error> {
        args.into_iter().map(|a| self.lower_expr(a)).collect()
    }
}

/// How a UFCS receiver is adjusted to match the chosen overload's first param.
#[derive(Clone, Copy)]
pub(super) enum RecvAdjust {
    AsIs,
    Ref,
    Deref,
}

/// Unifies `arg_ty` against `param_ty`, growing `subst` in place. Same
/// orientation-swap trick as `overload_matches`: whichever side has TypeVars
/// is treated as the inference side.
fn try_unify_one(
    arg_ty: &Ty,
    param_ty: &Ty,
    subst: &mut HashMap<TypeVarId, Ty>,
    struct_template_origin: &HashMap<String, (String, Vec<Ty>)>,
    enum_template_origin: &HashMap<String, (String, Vec<Ty>)>,
) -> bool {
    let (lhs, rhs) = if !ty_has_typevars(param_ty) && ty_has_typevars(arg_ty) {
        (arg_ty, param_ty)
    } else {
        (param_ty, arg_ty)
    };
    unify(
        lhs,
        rhs,
        Span::default(),
        subst,
        struct_template_origin,
        enum_template_origin,
    )
    .is_ok()
}

