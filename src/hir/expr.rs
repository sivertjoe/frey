//! Expression lowering: the recursive walk that turns AST expressions
//! into HIR `Expr`s. Holds the giant `lower_expr_with_hint` match.

use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        TypeVarId, UnaryOperator,
        coerce::{coerce_int_literal, coerce_through_tails},
        error::{Error, ErrorKind},
        generics::{collect_typevars, ty_has_typevars, unify},
        lower::{ GenericTemplate,
            Lower, end_of,
            unit_expr,
        },
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function,
            IntrinsicKind, LocalId, Param, Statement, StatementKind,
            StructDef, Ty,
        },
    },
    lexer::types::Span,
};

impl Lower {
    pub(super) fn lower_expr(&mut self, e: ast::Expr) -> Result<Expr, Error> {
        self.lower_expr_with_hint(e, None)
    }

    /// `hint` is the surrounding expected type, used today only as a tiebreaker
    /// for nullary variants like `None` where neither explicit type args nor
    /// argument unification can pin the enum's type parameters.
    pub(super) fn lower_expr_with_hint(
        &mut self,
        e: ast::Expr,
        hint: Option<&Ty>,
    ) -> Result<Expr, Error> {
        self.lower_expr_with_hint_inner(e, hint)
    }

    fn lower_expr_with_hint_inner(
        &mut self,
        e: ast::Expr,
        hint: Option<&Ty>,
    ) -> Result<Expr, Error> {
        match e.kind {
            ast::ExprKind::Const(ast::Const::Int(n)) => Ok(Expr {
                span: e.span,
                ty: Ty::Int,
                kind: ExprKind::Const(Const::Int(n)),
            }),
            ast::ExprKind::Const(ast::Const::Float(f)) => Ok(Expr {
                span: e.span,
                ty: Ty::Float,
                kind: ExprKind::Const(Const::Float(f)),
            }),
            ast::ExprKind::Const(ast::Const::Str(s)) => Ok(Expr {
                span: e.span,
                ty: Ty::Ptr(Box::new(Ty::U8)),
                kind: ExprKind::Const(Const::Str(s)),
            }),
            ast::ExprKind::Const(ast::Const::Char(b)) => Ok(Expr {
                span: e.span,
                ty: Ty::U8,
                kind: ExprKind::Const(Const::Char(b)),
            }),
            ast::ExprKind::Identifier(name) => {
                // Inside a `#comptime` body, a bare type-variable name used in
                // expression position (e.g. `T` in `T == Int`) denotes a
                // reified type value rather than a runtime binding.
                if self.in_comptime
                    && self.resolve(&name).is_none()
                    && let Some(tv) = self.lookup_type_var(&name)
                {
                    return Ok(Expr {
                        span: e.span,
                        ty: Ty::Unit,
                        kind: ExprKind::TypeValue(Ty::TypeVar(tv)),
                    });
                }

                // Variant names shadow locals — `None` is reserved.
                if self.variant_constructors.contains_key(&name) {
                    if let Some(expr) =
                        self.try_lower_variant_call(&name, &[], Vec::new(), e.span, hint)?
                    {
                        return Ok(expr);
                    }
                }

                let Some(local_id) = self.resolve(&name) else {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::NameNotFound { name },
                    });
                };

                let ty = self
                    .bindings
                    .get(&local_id)
                    .expect("resolved name must have a recorded type")
                    .clone();

                // Inside a closure body: if `local_id` was bound in a
                // non-global enclosing scope, it's a free variable — record
                // it as a capture. The Local expr is left in place; a
                // post-walk in `lower_closure` rewrites it to an env-field
                // load once the env tuple's layout is known.
                let depth = self
                    .scopes
                    .iter()
                    .enumerate()
                    .find_map(|(d, s)| s.values().any(|&v| v == local_id).then_some(d));
                if let Some(ctx) = self.closure_capture_ctx.as_mut()
                    && let Some(depth) = depth
                    && depth > 0
                    && depth < ctx.threshold_depth
                {
                    ctx.captures_by_id.entry(local_id).or_insert_with(|| {
                        let idx = ctx.captures.len();
                        ctx.captures.push((local_id, ty.clone()));
                        idx
                    });
                }

                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Local(local_id),
                })
            }
            ast::ExprKind::TypeValue(type_expr) => {
                if !self.in_comptime {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::ComptimeError {
                            message:
                                "a type can only be used as a value inside a #comptime function"
                                    .to_string(),
                        },
                    });
                }
                let ty = self.lower_type(&type_expr)?;
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Unit,
                    kind: ExprKind::TypeValue(ty),
                })
            }
            ast::ExprKind::Function {
                type_params,
                params,
                return_ty,
                body,
            } => {
                // Inline (named or anonymous) function literals: get their
                // own type-var scope. `lower_function_value` lifts the result
                // to a synthesized top-level function and returns a Local ref.
                self.push_type_var_scope();
                let result = self
                    .declare_type_params(&type_params, e.span)
                    .and_then(|()| self.lower_function_value(params, return_ty, body, e.span));
                self.pop_type_var_scope();
                result
            }
            ast::ExprKind::Closure { params, body } => {
                self.lower_closure(params, *body, hint, e.span)
            }
            ast::ExprKind::ExternFunction { .. } => Err(Error {
                span: e.span,
                kind: ErrorKind::ExternMustBeTopLevel,
            }),
            ast::ExprKind::Null => {
                let ty = match hint {
                    Some(t @ Ty::Ptr(_)) => t.clone(),
                    _ => {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::CannotInferNullType,
                        });
                    }
                };
                Ok(Expr {
                    span: e.span,
                    ty: ty.clone(),
                    kind: ExprKind::ZeroInit(ty),
                })
            }
            ast::ExprKind::TypedFunctionRef { name, type_args } => {
                let template_id = self.resolve(&name).ok_or_else(|| Error {
                    span: e.span,
                    kind: ErrorKind::NameNotFound { name: name.clone() },
                })?;
                let lowered_args: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.lower_type(t))
                    .collect::<Result<_, _>>()?;

                // The template may already be fully lowered (in self.templates)
                // or may only have its signature registered (in pending_fn_sigs)
                // if its body hasn't been lowered yet. Either source gives us
                // the arity + signature info we need.
                let (template_name, type_var_ids, param_tys, return_ty) =
                    if let Some(t) = self.templates.get(&template_id) {
                        (
                            t.name.clone(),
                            t.type_var_ids.clone(),
                            t.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>(),
                            t.return_ty.clone(),
                        )
                    } else if let Some(sig) = self.pending_fn_sigs.get(&template_id) {
                        (
                            name.clone(),
                            sig.type_var_ids.clone(),
                            sig.param_tys.clone(),
                            sig.return_ty.clone(),
                        )
                    } else {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::NotAGenericFunction { name },
                        });
                    };
                if lowered_args.len() != type_var_ids.len() {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::TypeArgArityMismatch {
                            name: template_name,
                            expected: type_var_ids.len(),
                            found: lowered_args.len(),
                        },
                    });
                }
                // Build the function type with the type args substituted in,
                // so the resulting expression's type is correct even in the
                // deferred case.
                let mut subst = HashMap::new();
                for (tv, t) in type_var_ids.iter().zip(lowered_args.iter()) {
                    subst.insert(*tv, t.clone());
                }
                let result_param_tys: Vec<Ty> =
                    param_tys.iter().map(|p| self.substitute_ty(p, &subst)).collect();
                let result_return_ty = self.substitute_ty(&return_ty, &subst);
                let fn_ty = Ty::Function {
                    params: result_param_tys,
                    return_ty: Box::new(result_return_ty),
                    varargs: false,
                };
                // Specialize now only when both (a) the args are concrete and
                // (b) the template's body is available. Otherwise emit a
                // DeferredFunctionRef the substitution pass will resolve.
                let deferable = lowered_args.iter().any(ty_has_typevars)
                    || !self.templates.contains_key(&template_id);
                if deferable {
                    return Ok(Expr {
                        span: e.span,
                        ty: fn_ty,
                        kind: ExprKind::DeferredFunctionRef {
                            template_id,
                            type_args: lowered_args,
                        },
                    });
                }
                let specialized =
                    self.specialize_call(template_id, &[], &lowered_args, e.span)?;
                let final_ty = self.bindings[&specialized].clone();
                Ok(Expr {
                    span: e.span,
                    ty: final_ty,
                    kind: ExprKind::Local(specialized),
                })
            }
            ast::ExprKind::Call {
                callee,
                type_args,
                args,
            } => {
                // `comperror("msg")` inside a comptime body lowers to a
                // CompError node that aborts compilation if reached during
                // comptime evaluation.
                if self.in_comptime
                    && let ast::ExprKind::Identifier(n) = &callee.kind
                    && n == "comperror"
                {
                    let message = match args.first().map(|a| &a.kind) {
                        Some(ast::ExprKind::Const(ast::Const::Str(s))) => s.clone(),
                        _ => {
                            return Err(Error {
                                span: e.span,
                                kind: ErrorKind::ComptimeError {
                                    message: "comperror expects a single string-literal argument"
                                        .to_string(),
                                },
                            });
                        }
                    };
                    return Ok(Expr {
                        span: e.span,
                        ty: Ty::Unit,
                        kind: ExprKind::CompError(message),
                    });
                }

                // Heap intrinsics: `alloc<T>(n)`, `realloc<T>(p, n)`, `free(p)`.
                if let ast::ExprKind::Identifier(n) = &callee.kind {
                    let intrinsic = match n.as_str() {
                        "alloc" => Some(IntrinsicKind::Alloc),
                        "realloc" => Some(IntrinsicKind::Realloc),
                        "free" => Some(IntrinsicKind::Free),
                        _ => None,
                    };
                    if let Some(kind) = intrinsic {
                        return self.lower_intrinsic(kind, type_args, args, e.span);
                    }
                }

                // Enum variant constructor: `Some(x)`, `Ok<Int, String>(42)`.
                if let ast::ExprKind::Identifier(n) = &callee.kind
                    && self.variant_constructors.contains_key(n)
                {
                    let name = n.clone();
                    if let Some(expr) =
                        self.try_lower_variant_call(&name, &type_args, args, e.span, hint)?
                    {
                        return Ok(expr);
                    }
                    unreachable!("variant_constructors entry guarantees Ok(Some)");
                }

                // Overloaded direct call: pick the overload matching the args.
                if let ast::ExprKind::Identifier(name) = &callee.kind
                    && self.overloads.get(name).is_some_and(|c| c.len() > 1)
                {
                    let name = name.clone();
                    let has_closures = args
                        .iter()
                        .any(|a| matches!(a.kind, ast::ExprKind::Closure { .. }));
                    let lowered_args: Vec<Expr> = if has_closures {
                        self.lower_overloaded_call_args_with_closures(&name, args)?
                    } else {
                        args.into_iter()
                            .map(|a| self.lower_expr(a))
                            .collect::<Result<_, _>>()?
                    };
                    let arg_tys: Vec<Ty> = lowered_args.iter().map(|a| a.ty.clone()).collect();
                    let explicit_type_args: Vec<Ty> = type_args
                        .iter()
                        .map(|t| self.lower_type(t))
                        .collect::<Result<_, _>>()?;
                    let fn_id = self.resolve_overload(&name, &arg_tys, e.span)?;
                    let callee = Expr {
                        span: e.span,
                        ty: self.bindings[&fn_id].clone(),
                        kind: ExprKind::Local(fn_id),
                    };
                    return self.finish_call(callee, lowered_args, explicit_type_args, e.span);
                }

                // Method call / UFCS: `recv.name(args)` becomes a field call
                // (when `name` is a field holding a function) or the uniform
                // form `name(recv, args)` otherwise.
                if matches!(&callee.kind, ast::ExprKind::Field { .. }) {
                    return self.lower_method_call(*callee, type_args, args, e.span);
                }

                let callee = self.lower_expr(*callee)?;
                let explicit_type_args: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.lower_type(t))
                    .collect::<Result<_, _>>()?;
                let args = self.lower_call_args(&callee, args, &explicit_type_args, e.span)?;
                self.finish_call(callee, args, explicit_type_args, e.span)
            }
            ast::ExprKind::Unary { op, expr } => {
                let operand = self.lower_expr(*expr)?;
                let op = self.lower_unary(op);
                // `!x` always yields an Int (0/1). `-x` preserves the operand type.
                let ty = match op {
                    UnaryOperator::Not => Ty::Int,
                    UnaryOperator::Minus => operand.ty.clone(),
                };
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Unary {
                        op,
                        operand: Box::new(operand),
                    },
                })
            }
            ast::ExprKind::Binary { op, lhs, rhs } => {
                use crate::ast::BinaryOperator as B;
                // For `==`/`!=`, lower LHS first; if it's a pointer, push its
                // type to the RHS as a hint so `null` picks up the right
                // pointer type.
                let lhs = self.lower_expr(*lhs)?;
                let rhs_hint =
                    matches!(op, B::Eq | B::Ne).then(|| lhs.ty.clone());
                let rhs = self.lower_expr_with_hint(*rhs, rhs_hint.as_ref())?;
                let (lhs, rhs) = if lhs.ty != rhs.ty {
                    if rhs.ty.is_number() && rhs.ty != Ty::Int {
                        let target = rhs.ty.clone();
                        (coerce_int_literal(lhs, &target)?, rhs)
                    } else if lhs.ty.is_number() && lhs.ty != Ty::Int {
                        let target = lhs.ty.clone();
                        (lhs, coerce_int_literal(rhs, &target)?)
                    } else {
                        (lhs, rhs)
                    }
                } else {
                    (lhs, rhs)
                };
                let ty = match op {
                    // Arithmetic preserves the operand type (operands must match —
                    // typechecker enforces this).
                    B::Add | B::Sub | B::Mul | B::Div | B::Mod => lhs.ty.clone(),
                    // Shifts and bitwise are Int-only; result is Int.
                    B::Shl | B::Shr | B::BitAnd | B::BitOr | B::BitXor => Ty::Int,
                    // Comparisons and logical produce Int (0/1) as boolean.
                    B::Lt | B::Le | B::Gt | B::Ge | B::Eq | B::Ne => Ty::Int,
                    B::And | B::Or => Ty::Int,
                };
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                })
            }

            ast::ExprKind::Block(block) => {
                let b = self.lower_block_with_hint(block, hint)?;
                let ty = b.tail.ty.clone();
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Block(b),
                })
            }
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.lower_expr(*condition)?;

                let then_block = self.lower_block_with_hint(then_branch, hint)?;
                let then_span = then_block.span;
                let then_expr = Expr {
                    span: then_span,
                    ty: then_block.tail.ty.clone(),
                    kind: ExprKind::Block(then_block),
                };

                let else_expr = match else_branch {
                    Some(else_branch) => self.lower_expr_with_hint(*else_branch, hint)?,
                    None => unit_expr(end_of(e.span)),
                };

                let (then_expr, else_expr) = if then_expr.ty != else_expr.ty {
                    if else_expr.ty.is_number() && else_expr.ty != Ty::Int {
                        let target = else_expr.ty.clone();
                        (coerce_through_tails(then_expr, &target)?, else_expr)
                    } else if then_expr.ty.is_number() && then_expr.ty != Ty::Int {
                        let target = then_expr.ty.clone();
                        (then_expr, coerce_through_tails(else_expr, &target)?)
                    } else {
                        (then_expr, else_expr)
                    }
                } else {
                    (then_expr, else_expr)
                };

                let if_ty = then_expr.ty.clone();
                Ok(Expr {
                    span: e.span,
                    ty: if_ty,
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(then_expr),
                        else_branch: Box::new(else_expr),
                    },
                })
            }
            ast::ExprKind::Cast { expr, target } => {
                let ty = self.lower_type(&target)?;
                Ok(Expr {
                    span: e.span,
                    ty: ty.clone(),
                    kind: ExprKind::Cast {
                        target: ty.clone(),
                        expr: Box::new(self.lower_expr(*expr)?),
                    },
                })
            }
            ast::ExprKind::Assign { target, value } => {
                // The parser guarantees `target` is a place expression
                // (Identifier or Subscript). Lower it like any other expr
                // — Identifier becomes Local, Subscript becomes Subscript.
                let target = self.lower_expr(*target)?;
                // Push the target's type as a hint into the value, so
                // `x = Empty;` (etc.) can infer from the target.
                let value_hint = target.ty.clone();
                let value = self.lower_expr_with_hint(*value, Some(&value_hint))?;
                let value = coerce_int_literal(value, &target.ty)?;
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Unit,
                    kind: ExprKind::Assign {
                        target: Box::new(target),
                        value: Box::new(value),
                    },
                })
            }
            ast::ExprKind::Subscript { expr, index } => {
                let target = self.lower_expr(*expr)?;
                let index = self.lower_expr(*index)?;
                // Indexing works on both fixed arrays and raw pointers.
                let element = match target.ty.clone() {
                    Ty::Array { element, .. } => *element,
                    Ty::Ptr(element) => *element,
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotIndexable { found: other },
                        });
                    }
                };
                Ok(Expr {
                    span: e.span,
                    ty: element,
                    kind: ExprKind::Subscript {
                        expr: Box::new(target),
                        index: Box::new(index),
                    },
                })
            }
            ast::ExprKind::Array(items) => {
                if items.is_empty() {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::EmptyArrayLiteral,
                    });
                }
                let lowered: Vec<Expr> = items
                    .into_iter()
                    .map(|x| self.lower_expr(x))
                    .collect::<Result<_, _>>()?;
                let element = Box::new(lowered[0].ty.clone());
                let count = lowered.len();
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Array { element, count },
                    kind: ExprKind::Array(lowered),
                })
            }
            ast::ExprKind::Ref(target) => {
                let target = self.lower_expr(*target)?;
                let ty = Ty::Ptr(Box::new(target.ty.clone()));
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Ref(Box::new(target)),
                })
            }
            ast::ExprKind::Deref(target) => {
                let target = self.lower_expr(*target)?;
                // `*u8` in a `#comptime` body — the unary `*` parses as Deref
                // but semantically wraps the inner type. Rewrite to a TypeValue
                // so `T == *u8` works.
                if let ExprKind::TypeValue(inner_ty) = target.kind {
                    return Ok(Expr {
                        span: e.span,
                        ty: Ty::Unit,
                        kind: ExprKind::TypeValue(Ty::Ptr(Box::new(inner_ty))),
                    });
                }
                let ty = match &target.ty {
                    Ty::Ptr(inner) => (**inner).clone(),
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotDereferencable {
                                found: other.clone(),
                            },
                        });
                    }
                };
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Deref(Box::new(target)),
                })
            }
            ast::ExprKind::StructDef { .. } => {
                // Struct definitions at top level are consumed by lower_program
                // and never reach here. Inside an expression context they have
                // no runtime value.
                Err(Error {
                    span: e.span,
                    kind: ErrorKind::StructDefNotAllowedHere,
                })
            }
            ast::ExprKind::StructLiteral {
                name,
                type_args,
                fields,
            } => {
                // Explicit type arguments: `Map<K, V> { ... }`.
                if !type_args.is_empty() {
                    if let Some(template) = self.struct_templates.get(&name).cloned() {
                        return self.lower_explicit_generic_struct_literal(
                            template, name, type_args, fields, e.span,
                        );
                    }
                    if self.structs.contains_key(&name) {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::UnexpectedTypeArguments { name },
                        });
                    }
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::UnknownType { name },
                    });
                }
                // Concrete-struct path: existing behavior.
                if let Some(def) = self.structs.get(&name).cloned() {
                    return self.lower_struct_literal(def, name, fields, e.span);
                }
                // Generic-struct path: infer type args from the provided field
                // values, then specialize.
                if let Some(template) = self.struct_templates.get(&name).cloned() {
                    return self.lower_generic_struct_literal(template, name, fields, e.span);
                }
                Err(Error {
                    span: e.span,
                    kind: ErrorKind::UnknownType { name: name.clone() },
                })
            }
            ast::ExprKind::While { condition, body } => {
                let condition = self.lower_expr(*condition)?;
                let body = self.lower_block(body)?;
                Ok(Expr {
                    span: e.span,
                    ty: Ty::Unit,
                    kind: ExprKind::While {
                        condition: Box::new(condition),
                        body,
                    },
                })
            }
            ast::ExprKind::Field { target, name } => {
                let mut target = self.lower_expr(*target)?;
                // Auto-deref through any chain of pointers so `p.x` works
                // when `p: *T`, `**T`, etc. If the chain doesn't end in a
                // struct, the NotAStruct check below catches it.
                while let Ty::Ptr(inner) = target.ty.clone() {
                    let pointee_ty = *inner;
                    target = Expr {
                        span: target.span,
                        ty: pointee_ty,
                        kind: ExprKind::Deref(Box::new(target)),
                    };
                }
                let (display_name, fields) = match &target.ty {
                    Ty::Struct(n) => {
                        let def = self
                            .structs
                            .get(n)
                            .expect("typed Struct must be registered");
                        (n.clone(), def.fields.clone())
                    }
                    Ty::GenericStruct { name, args } => {
                        let template = self
                            .struct_templates
                            .get(name)
                            .cloned()
                            .expect("GenericStruct must reference a known template");
                        let subst: HashMap<TypeVarId, Ty> = template
                            .type_var_ids
                            .iter()
                            .zip(args.iter())
                            .map(|(tv, ty)| (*tv, ty.clone()))
                            .collect();
                        let substituted_fields: Vec<(String, Ty)> = template
                            .fields
                            .iter()
                            .map(|(fname, fty)| {
                                let ty = self.substitute_ty(fty, &subst);
                                (fname.clone(), ty)
                            })
                            .collect();
                        (name.clone(), substituted_fields)
                    }
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotAStruct {
                                found: other.clone(),
                            },
                        });
                    }
                };

                let (index, field_ty) = fields
                    .iter()
                    .enumerate()
                    .find_map(|(i, (n, t))| (n == &name).then_some((i, t.clone())))
                    .ok_or_else(|| Error {
                        span: e.span,
                        kind: ErrorKind::UnknownField {
                            struct_name: display_name,
                            field: name.clone(),
                        },
                    })?;

                Ok(Expr {
                    span: e.span,
                    ty: field_ty,
                    kind: ExprKind::Field {
                        target: Box::new(target),
                        name,
                        index,
                    },
                })
            }
            ast::ExprKind::Tuple(elems) => {
                let lowered: Vec<Expr> = elems
                    .into_iter()
                    .map(|el| self.lower_expr(el))
                    .collect::<Result<_, _>>()?;
                let ty = Ty::Tuple(lowered.iter().map(|el| el.ty.clone()).collect());
                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Tuple(lowered),
                })
            }
            ast::ExprKind::TupleField { target, index } => {
                let mut target = self.lower_expr(*target)?;
                // Auto-deref through any pointer chain so `p.0` works when
                // `p: *T`, `**T`, ...
                while let Ty::Ptr(inner) = target.ty.clone() {
                    let pointee_ty = *inner;
                    target = Expr {
                        span: target.span,
                        ty: pointee_ty,
                        kind: ExprKind::Deref(Box::new(target)),
                    };
                }
                let elems = match &target.ty {
                    Ty::Tuple(elems) => elems.clone(),
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotATuple {
                                found: other.clone(),
                            },
                        });
                    }
                };
                if index >= elems.len() {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::TupleIndexOutOfRange {
                            len: elems.len(),
                            index,
                        },
                    });
                }
                let field_ty = elems[index].clone();
                Ok(Expr {
                    span: e.span,
                    ty: field_ty,
                    kind: ExprKind::TupleField {
                        target: Box::new(target),
                        index,
                    },
                })
            }
            ast::ExprKind::EnumDef { .. } => {
                // Enum definitions at top level are consumed by lower_program
                // and never reach here. Inside an expression context they have
                // no runtime value.
                Err(Error {
                    span: e.span,
                    kind: ErrorKind::StructDefNotAllowedHere,
                })
            }
            ast::ExprKind::Match { scrutinee, arms } => {
                self.lower_match(*scrutinee, arms, e.span, hint)
            }
        }
    }

    /// Finishes a call once the callee and arguments are lowered: coerces
    /// int-literal args against the parameter types, then either specializes a
    /// generic template (when every argument is concrete) or builds the call.

    pub(super) fn lower_function_value(
        &mut self,
        params: Vec<ast::Param>,
        return_ty: Option<ast::TypeExpr>,
        body: ast::Block,
        span: crate::lexer::types::Span,
    ) -> Result<Expr, Error> {
        let return_ty = match return_ty {
            Some(t) => self.lower_type(&t)?,
            None => Ty::Unit,
        };

        self.enter_scope();

        let mut hir_params = Vec::new();
        let mut param_tys = Vec::new();
        for p in params {
            let ty = self.lower_type(&p.ty)?;
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(p.name.clone(), id);
            self.bindings.insert(id, ty.clone());
            param_tys.push(ty.clone());
            hir_params.push(Param {
                id,
                span: p.span,
                name: p.name,
                ty,
            });
        }

        let return_hint = (!ty_has_typevars(&return_ty)).then(|| return_ty.clone());
        self.return_ty_stack.push(return_ty.clone());
        let body_result = self.lower_block_with_hint(body, return_hint.as_ref());
        self.return_ty_stack.pop();
        let mut body = body_result?;
        if body.tail.ty != return_ty && return_ty.is_number() && return_ty != Ty::Int {
            let tail = std::mem::replace(&mut body.tail, Box::new(unit_expr(body.span)));
            body.tail = Box::new(coerce_through_tails(*tail, &return_ty)?);
        }

        self.leave_scope();

        let ty = Ty::Function {
            params: param_tys,
            return_ty: Box::new(return_ty.clone()),
            varargs: false,
        };

        let func = Function {
            params: hir_params,
            return_ty,
            body,
            env_param_id: None,
        };
        Ok(self.lift_function_value(func, ty, span))
    }

    /// Lowers a call's argument list with hints derived from the callee's
    /// parameter types. Closure args are deferred to a second pass so that
    /// non-closure args can extend the substitution first — without this
    /// step, `map<Int, Int>(Some(10), {x: x*2})` wouldn't see `Int` for `x`.

    /// Coerces a Frey-function value (`Ty::Function`) into a closure value
    /// (`Ty::Closure`) — wraps it as `{env: null, code: fn}`. Under the
    /// uniform calling convention, every Frey fn takes `env: *u8` as its
    /// first LLVM param, so the fn pointer is directly usable as a
    /// closure's code field without a trampoline. No-op for non-Function
    /// values or non-Closure targets.
    pub(super) fn coerce_to_closure(&mut self, value: Expr, target_ty: &Ty, span: Span) -> Expr {
        if !matches!(target_ty, Ty::Closure { .. }) {
            return value;
        }
        // Take the source function's concrete params/return — using the
        // target's would erase the inferred types (e.g., coercing a
        // `(Int)->Int` to a `(T)->U` hint must produce `Ty::Closure { (Int)->Int }`,
        // not `Ty::Closure { (T)->U }`, so the caller's unification still
        // learns T=Int and U=Int).
        let (src_params, src_return) = match &value.ty {
            Ty::Function {
                params,
                return_ty,
                varargs: false,
            } => (params.clone(), return_ty.clone()),
            _ => return value,
        };
        let env_ty = Ty::Ptr(Box::new(Ty::U8));
        let null_env = Expr {
            span,
            ty: env_ty.clone(),
            kind: ExprKind::ZeroInit(env_ty),
        };
        Expr {
            span,
            ty: Ty::Closure {
                params: src_params,
                return_ty: src_return,
            },
            kind: ExprKind::MakeClosure {
                env: Box::new(null_env),
                code: Box::new(value),
            },
        }
    }

    pub(super) fn lift_function_value(&mut self, func: Function, fn_ty: Ty, span: Span) -> Expr {
        let synth_id = self.id_gen.fresh();
        let synth_name = format!("__lambda_{}", synth_id.raw());
        self.bindings.insert(synth_id, fn_ty.clone());

        // Closure code fns can have typevars in the body (via captured-tuple
        // casts) that aren't in the signature. Walk the body too so the
        // template's type_var_ids covers every typevar specialization needs.
        let body_has_tv = block_has_typevars(&func.body);
        let is_generic = func.params.iter().any(|p| ty_has_typevars(&p.ty))
            || ty_has_typevars(&func.return_ty)
            || body_has_tv;
        if is_generic {
            let mut type_var_ids = Vec::new();
            for p in &func.params {
                collect_typevars(&p.ty, &mut type_var_ids);
            }
            collect_typevars(&func.return_ty, &mut type_var_ids);
            collect_typevars_in_block(&func.body, &mut type_var_ids);
            self.templates.insert(
                synth_id,
                GenericTemplate {
                    name: synth_name,
                    span,
                    type_var_ids,
                    params: func.params,
                    return_ty: func.return_ty,
                    body: func.body,
                    env_param_id: func.env_param_id,
                },
            );
        } else {
            self.pending_specializations.push(Declaration {
                id: synth_id,
                span,
                name: synth_name,
                ty: fn_ty.clone(),
                value: Expr {
                    span,
                    ty: fn_ty.clone(),
                    kind: ExprKind::Function(func),
                },
            });
        }

        Expr {
            span,
            ty: fn_ty,
            kind: ExprKind::Local(synth_id),
        }
    }

    /// Lowers a closure literal `{x, y : body}` using `hint` as the expected
    /// closure type. The body lowers with enclosing scopes visible so free
    /// variables can be captured. The synthesized code fn takes
    /// `env: *u8` as its first LLVM param (added uniformly by codegen);
    /// captures live in a heap-allocated env tuple and are rewritten in
    /// the body as loads from `*env`.
    pub(super) fn lower_closure(
        &mut self,
        params: Vec<String>,
        body: ast::Expr,
        hint: Option<&Ty>,
        span: Span,
    ) -> Result<Expr, Error> {
        // Zero-param closures don't need a hint — there's nothing to infer
        // for params, and the return type falls out of the body.
        let (hint_params, hint_return): (&[Ty], Option<&Ty>) = match hint {
            Some(Ty::Closure { params, return_ty }) => (params.as_slice(), Some(return_ty.as_ref())),
            Some(Ty::Function {
                params, return_ty, ..
            }) => (params.as_slice(), Some(return_ty.as_ref())),
            _ if params.is_empty() => (&[][..], None),
            _ => {
                return Err(Error {
                    span,
                    kind: ErrorKind::ClosureTypeUnknown,
                });
            }
        };
        if hint_params.len() != params.len() {
            return Err(Error {
                span,
                kind: ErrorKind::ClosureArityMismatch {
                    expected: hint_params.len(),
                    found: params.len(),
                },
            });
        }
        if hint_params.iter().any(ty_has_typevars) {
            return Err(Error {
                span,
                kind: ErrorKind::ClosureTypeUnknown,
            });
        }

        // Synthesize the env param up front so the body can refer to it
        // after capture rewriting.
        let env_ty = Ty::Ptr(Box::new(Ty::U8));
        let env_param_id = self.id_gen.fresh();
        self.bindings.insert(env_param_id, env_ty.clone());

        // Push a scope for the closure's own params. The body will see
        // this AND all enclosing scopes — captures fall out naturally.
        self.enter_scope();

        let mut hir_params = Vec::with_capacity(params.len());
        for (name, ty) in params.into_iter().zip(hint_params.iter()) {
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(name.clone(), id);
            self.bindings.insert(id, ty.clone());
            hir_params.push(Param {
                id,
                span,
                name,
                ty: ty.clone(),
            });
        }

        // Activate capture detection — the Identifier-lowering path checks
        // here and records free vars from depths 1..threshold_depth.
        // `threshold_depth` is the closure's own param scope's depth, so
        // strictly-less-than excludes the closure's own params from
        // capture (they're depth == threshold_depth).
        let prev_ctx = self.closure_capture_ctx.take();
        self.closure_capture_ctx = Some(crate::hir::lower::ClosureCaptureCtx {
            threshold_depth: self.scopes.len() - 1,
            captures: Vec::new(),
            captures_by_id: HashMap::new(),
        });

        let return_hint = match hint_return {
            Some(t) if !ty_has_typevars(t) => Some(t.clone()),
            _ => None,
        };
        let body_result = self.lower_expr_with_hint(body, return_hint.as_ref());

        let ctx = std::mem::replace(&mut self.closure_capture_ctx, prev_ctx)
            .expect("ctx was set above");
        self.leave_scope();
        let body_expr = body_result?;

        let return_ty = body_expr.ty.clone();

        // Build the env tuple type from captures (empty captures → unit-ish,
        // but we keep env = null in that case to skip allocation entirely).
        let captures = ctx.captures;
        let captures_by_id = ctx.captures_by_id;
        let has_captures = !captures.is_empty();
        let env_tuple_ty = if has_captures {
            Ty::Tuple(captures.iter().map(|(_, t)| t.clone()).collect())
        } else {
            Ty::Unit
        };

        // Rewrite the body: each Local(captured_id) becomes a load from
        // `*(env as *EnvTuple)`'s tuple field at the capture's index.
        let mut body_expr = body_expr;
        if has_captures {
            rewrite_captures_in_expr(
                &mut body_expr,
                &captures_by_id,
                env_param_id,
                &env_tuple_ty,
                span,
            );
        }

        // Wrap body as the synth code fn's body block.
        let body_block = Block {
            span,
            items: Vec::new(),
            tail: Box::new(body_expr),
        };
        let mut all_params: Vec<Param> = Vec::with_capacity(hir_params.len());
        all_params.extend(hir_params.iter().cloned());
        let code_fn_ty = Ty::Function {
            params: all_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(return_ty.clone()),
            varargs: false,
        };
        // Only flag env_param_id when there are captures — capture-free
        // closures don't read env, so codegen can skip registering it.
        let env_param_id_for_func = if has_captures { Some(env_param_id) } else { None };
        let func = Function {
            params: all_params,
            return_ty: return_ty.clone(),
            body: body_block,
            env_param_id: env_param_id_for_func,
        };
        let code_expr = self.lift_function_value(func, code_fn_ty, span);

        let closure_ty = Ty::Closure {
            params: hint_params.to_vec(),
            return_ty: Box::new(return_ty),
        };

        // env construction expression: null for capture-free, otherwise a
        // block that allocs the env tuple on the heap, writes captures
        // into each field, and casts the *EnvTuple back to *u8.
        let env_expr = if has_captures {
            self.build_env_alloc_expr(&captures, &env_tuple_ty, span)
        } else {
            Expr {
                span,
                ty: env_ty.clone(),
                kind: ExprKind::ZeroInit(env_ty),
            }
        };

        Ok(Expr {
            span,
            ty: closure_ty,
            kind: ExprKind::MakeClosure {
                env: Box::new(env_expr),
                code: Box::new(code_expr),
            },
        })
    }

    /// Builds the env-allocation expression for a capturing closure:
    /// ```text
    /// {
    ///     let __env = alloc<EnvTuple>(1);
    ///     (*__env).0 = cap_0;
    ///     (*__env).1 = cap_1;
    ///     ...
    ///     __env as *u8
    /// }
    /// ```
    fn build_env_alloc_expr(
        &mut self,
        captures: &[(LocalId, Ty)],
        env_tuple_ty: &Ty,
        span: Span,
    ) -> Expr {
        let env_ptr_ty = Ty::Ptr(Box::new(env_tuple_ty.clone()));
        let u8_ptr_ty = Ty::Ptr(Box::new(Ty::U8));

        // let __env = alloc<EnvTuple>(1);
        let env_local_id = self.id_gen.fresh();
        self.bindings.insert(env_local_id, env_ptr_ty.clone());
        let alloc_call = Expr {
            span,
            ty: env_ptr_ty.clone(),
            kind: ExprKind::Intrinsic {
                kind: crate::hir::types::IntrinsicKind::Alloc,
                elem_ty: env_tuple_ty.clone(),
                args: vec![Expr {
                    span,
                    ty: Ty::Int,
                    kind: ExprKind::Const(Const::Int(1)),
                }],
            },
        };
        let env_decl = Declaration {
            id: env_local_id,
            span,
            name: "__env".to_string(),
            ty: env_ptr_ty.clone(),
            value: alloc_call,
        };

        // (*__env).i = cap_i  for each capture
        let mut items: Vec<BlockItem> = Vec::with_capacity(captures.len() + 1);
        items.push(BlockItem::Declaration(env_decl));
        for (i, (cap_id, cap_ty)) in captures.iter().enumerate() {
            let env_local_expr = Expr {
                span,
                ty: env_ptr_ty.clone(),
                kind: ExprKind::Local(env_local_id),
            };
            let env_deref = Expr {
                span,
                ty: env_tuple_ty.clone(),
                kind: ExprKind::Deref(Box::new(env_local_expr)),
            };
            let field = Expr {
                span,
                ty: cap_ty.clone(),
                kind: ExprKind::TupleField {
                    target: Box::new(env_deref),
                    index: i,
                },
            };
            let value = Expr {
                span,
                ty: cap_ty.clone(),
                kind: ExprKind::Local(*cap_id),
            };
            let assign = Expr {
                span,
                ty: Ty::Unit,
                kind: ExprKind::Assign {
                    target: Box::new(field),
                    value: Box::new(value),
                },
            };
            items.push(BlockItem::Statement(Statement {
                span,
                kind: StatementKind::Expr(assign),
            }));
        }

        // tail: __env as *u8
        let cast_tail = Expr {
            span,
            ty: u8_ptr_ty.clone(),
            kind: ExprKind::Cast {
                target: u8_ptr_ty,
                expr: Box::new(Expr {
                    span,
                    ty: env_ptr_ty,
                    kind: ExprKind::Local(env_local_id),
                }),
            },
        };
        Expr {
            span,
            ty: Ty::Ptr(Box::new(Ty::U8)),
            kind: ExprKind::Block(Block {
                span,
                items,
                tail: Box::new(cast_tail),
            }),
        }
    }


    pub(super) fn lower_block(&mut self, b: ast::Block) -> Result<Block, Error> {
        self.lower_block_with_hint(b, None)
    }

    /// `hint` flows into the block's tail expression only — intermediate
    /// statements don't see it.
    pub(super) fn lower_block_with_hint(
        &mut self,
        b: ast::Block,
        hint: Option<&Ty>,
    ) -> Result<Block, Error> {
        let mut items = Vec::new();

        for item in b.items {
            match item {
                ast::BlockItem::Declaration(d) => {
                    // Nested function literals alias their name to a lifted
                    // top-level synth id and produce no block item.
                    if let Some(d) = self.lower_declaration(d)? {
                        items.push(BlockItem::Declaration(d));
                    }
                }
                ast::BlockItem::Statement(statement) => {
                    let s = self.lower_statement(statement)?;
                    items.push(BlockItem::Statement(s));
                }
            }
        }

        let tail = match b.tail {
            Some(expr) => Box::new(self.lower_expr_with_hint(*expr, hint)?),
            None => Box::new(unit_expr(end_of(b.span))),
        };

        Ok(Block {
            span: b.span,
            items,
            tail,
        })
    }

    pub(super) fn lower_statement(&mut self, s: ast::Statement) -> Result<Statement, Error> {
        let kind = match s.kind {
            ast::StatementKind::Return(expr) => {
                let expr = match expr {
                    Some(e) => {
                        // Push the current function's return type as a hint
                        // so `return None;` and friends infer correctly. Even
                        // a TypeVar-bearing hint (e.g. `Option<V>` inside a
                        // generic body) is useful — variant constructors can
                        // defer their specialization through it.
                        let hint = self.return_ty_stack.last().cloned();
                        self.lower_expr_with_hint(e, hint.as_ref())?
                    }
                    None => unit_expr(end_of(s.span)),
                };
                StatementKind::Return(expr)
            }
            ast::StatementKind::Expr(expr) => {
                let expr = self.lower_expr(expr)?;
                StatementKind::Expr(expr)
            }
            ast::StatementKind::Break => StatementKind::Break,
            ast::StatementKind::Defer(expr) => {
                let expr = self.lower_expr(expr)?;
                StatementKind::Defer(expr)
            }
        };

        Ok(Statement { span: s.span, kind })
    }


    pub(super) fn lower_intrinsic(
        &mut self,
        kind: IntrinsicKind,
        type_args: Vec<ast::TypeExpr>,
        args: Vec<ast::Expr>,
        span: Span,
    ) -> Result<Expr, Error> {
        // `alloc`/`realloc` need the element type for `sizeof`; `free` doesn't.
        let elem_ty = match type_args.first() {
            Some(t) => self.lower_type(t)?,
            None => {
                if matches!(kind, IntrinsicKind::Alloc | IntrinsicKind::Realloc) {
                    let name = match kind {
                        IntrinsicKind::Alloc => "alloc",
                        IntrinsicKind::Realloc => "realloc",
                        IntrinsicKind::Free => "free",
                    };
                    return Err(Error {
                        span,
                        kind: ErrorKind::MissingTypeArguments {
                            name: name.to_string(),
                        },
                    });
                }
                Ty::Unit
            }
        };
        let args: Vec<Expr> = args
            .into_iter()
            .map(|a| self.lower_expr(a))
            .collect::<Result<_, _>>()?;
        let ty = match kind {
            IntrinsicKind::Alloc | IntrinsicKind::Realloc => Ty::Ptr(Box::new(elem_ty.clone())),
            IntrinsicKind::Free => Ty::Unit,
        };
        Ok(Expr {
            span,
            ty,
            kind: ExprKind::Intrinsic {
                kind,
                elem_ty,
                args,
            },
        })
    }


    pub(super) fn lower_struct_literal(
        &mut self,
        def: StructDef,
        name: String,
        fields: Vec<ast::StructLiteralField>,
        span: Span,
    ) -> Result<Expr, Error> {
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut lowered_by_name: HashMap<String, Expr> = HashMap::new();
        for f in fields {
            if seen.insert(f.name.clone(), ()).is_some() {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::DuplicateField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            let target_ty = def
                .fields
                .iter()
                .find(|(n, _)| n == &f.name)
                .map(|(_, t)| t.clone());
            let Some(target_ty) = target_ty else {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::UnknownField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            };
            let value = self.lower_expr_with_hint(f.value, Some(&target_ty))?;
            let value = coerce_int_literal(value, &target_ty)?;
            let value = self.coerce_to_closure(value, &target_ty, f.span);
            lowered_by_name.insert(f.name, value);
        }

        let mut missing = Vec::new();
        let mut ordered = Vec::with_capacity(def.fields.len());
        for (fname, _) in &def.fields {
            match lowered_by_name.remove(fname) {
                Some(v) => ordered.push((fname.clone(), v)),
                None => missing.push(fname.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Error {
                span,
                kind: ErrorKind::MissingFields {
                    struct_name: name,
                    missing,
                },
            });
        }

        Ok(Expr {
            span,
            ty: Ty::Struct(def.name),
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

    /// Lowers `Name<T, U> { ... }` — a generic struct literal with explicit
    /// type arguments. Unlike the inferred path, the type args come from the
    /// `<...>` list, so this works even when no field's value pins them (e.g.
    /// a phantom parameter). When an arg is still a type var (inside a generic
    /// body) the literal stays a `GenericStruct` and specializes later.
    pub(super) fn lower_explicit_generic_struct_literal(
        &mut self,
        template: StructDef,
        name: String,
        type_args: Vec<ast::TypeExpr>,
        fields: Vec<ast::StructLiteralField>,
        span: Span,
    ) -> Result<Expr, Error> {
        let args: Vec<Ty> = type_args
            .iter()
            .map(|t| self.lower_type(t))
            .collect::<Result<_, _>>()?;
        if args.len() != template.type_var_ids.len() {
            return Err(Error {
                span,
                kind: ErrorKind::TypeArgArityMismatch {
                    name,
                    expected: template.type_var_ids.len(),
                    found: args.len(),
                },
            });
        }
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        for (tv, ty) in template.type_var_ids.iter().zip(args.iter()) {
            subst.insert(*tv, ty.clone());
        }

        // Lower and coerce each field against its (substituted) declared type.
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut lowered_by_name: HashMap<String, Expr> = HashMap::new();
        for f in fields {
            if seen.insert(f.name.clone(), ()).is_some() {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::DuplicateField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            let Some((_, template_field_ty)) = template.fields.iter().find(|(n, _)| n == &f.name)
            else {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::UnknownField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            };
            let field_ty = template_field_ty.clone();
            let target_ty = self.substitute_ty(&field_ty, &subst);
            let value = self.lower_expr_with_hint(f.value, Some(&target_ty))?;
            let value = coerce_int_literal(value, &target_ty)?;
            let value = self.coerce_to_closure(value, &target_ty, f.span);
            lowered_by_name.insert(f.name, value);
        }

        let mut missing = Vec::new();
        let mut ordered = Vec::with_capacity(template.fields.len());
        for (fname, _) in &template.fields {
            match lowered_by_name.remove(fname) {
                Some(v) => ordered.push((fname.clone(), v)),
                None => missing.push(fname.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Error {
                span,
                kind: ErrorKind::MissingFields {
                    struct_name: name,
                    missing,
                },
            });
        }

        // Defer while any type argument is still a type var (we're inside a
        // generic body); otherwise specialize the struct now.
        let still_generic = args
            .iter()
            .any(|a| ty_has_typevars(a) || matches!(a, Ty::GenericStruct { .. }));
        let ty = if still_generic {
            Ty::GenericStruct { name, args }
        } else {
            Ty::Struct(self.specialize_struct(&template, args, span)?)
        };

        Ok(Expr {
            span,
            ty,
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

    pub(super) fn lower_generic_struct_literal(
        &mut self,
        template: StructDef,
        name: String,
        fields: Vec<ast::StructLiteralField>,
        span: Span,
    ) -> Result<Expr, Error> {
        // First pass: lower each provided field value (no coercion yet — the
        // template field type may be a TypeVar so we don't know what to coerce
        // to). Detect duplicate and unknown field names while we're here.
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut lowered_by_name: HashMap<String, Expr> = HashMap::new();
        let mut spans_by_name: HashMap<String, Span> = HashMap::new();
        for f in fields {
            if seen.insert(f.name.clone(), ()).is_some() {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::DuplicateField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            if !template.fields.iter().any(|(n, _)| n == &f.name) {
                return Err(Error {
                    span: f.span,
                    kind: ErrorKind::UnknownField {
                        struct_name: name.clone(),
                        field: f.name.clone(),
                    },
                });
            }
            // Coerce the value against the template's field type up front.
            // For TypeVar fields this is a no-op (TypeVar isn't a number);
            // for concrete fields it handles literal-to-numeric coercion so
            // unify doesn't have to reason about `Int → UInt`.
            let template_field_ty = template
                .fields
                .iter()
                .find(|(n, _)| n == &f.name)
                .map(|(_, t)| t.clone())
                .expect("checked above");
            let value = self.lower_expr(f.value)?;
            let value = coerce_int_literal(value, &template_field_ty)?;
            spans_by_name.insert(f.name.clone(), value.span);
            lowered_by_name.insert(f.name, value);
        }

        // Check no fields are missing — we need every value provided to
        // potentially help infer the type args.
        let mut missing = Vec::new();
        for (fname, _) in &template.fields {
            if !lowered_by_name.contains_key(fname) {
                missing.push(fname.clone());
            }
        }
        if !missing.is_empty() {
            return Err(Error {
                span,
                kind: ErrorKind::MissingFields {
                    struct_name: name,
                    missing,
                },
            });
        }

        // Second pass: unify each provided value's type against the template
        // field's type to deduce type arguments.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        for (fname, template_ty) in &template.fields {
            let value = lowered_by_name.get(fname).expect("checked above");
            let field_span = spans_by_name.get(fname).copied().unwrap_or(span);
            unify(
                template_ty,
                &value.ty,
                field_span,
                &mut subst,
                &self.struct_template_origin,
                &self.enum_template_origin,
            )?;
        }
        for tv in &template.type_var_ids {
            if !subst.contains_key(tv) {
                let tv_name = self
                    .type_vars
                    .get(tv.0 as usize)
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| format!("T{}", tv.0));
                return Err(Error {
                    span,
                    kind: ErrorKind::CannotInferTypeArg { name: tv_name },
                });
            }
        }

        // Specialize and coerce values against the concrete field types.
        let args: Vec<Ty> = template
            .type_var_ids
            .iter()
            .map(|id| subst[id].clone())
            .collect();
        let specialized_name = self.specialize_struct(&template, args, span)?;
        let def = self
            .structs
            .get(&specialized_name)
            .cloned()
            .expect("just inserted by specialize_struct");

        let mut ordered = Vec::with_capacity(def.fields.len());
        for (fname, target_ty) in &def.fields {
            let value = lowered_by_name.remove(fname).expect("checked above");
            let value = coerce_int_literal(value, target_ty)?;
            let field_span = value.span;
            let value = self.coerce_to_closure(value, target_ty, field_span);
            ordered.push((fname.clone(), value));
        }

        Ok(Expr {
            span,
            ty: Ty::Struct(specialized_name),
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

}

/// Whether any expression in `block` has a type containing a TypeVar.
/// Used by `lift_function_value` to detect closure code fns whose body
/// references parent typevars (via captured-tuple casts) even when the
/// signature doesn't.
fn block_has_typevars(block: &Block) -> bool {
    for item in &block.items {
        match item {
            BlockItem::Declaration(d) => {
                if ty_has_typevars(&d.ty) || expr_has_typevars(&d.value) {
                    return true;
                }
            }
            BlockItem::Statement(s) => match &s.kind {
                StatementKind::Expr(e)
                | StatementKind::Return(e)
                | StatementKind::Defer(e) => {
                    if expr_has_typevars(e) {
                        return true;
                    }
                }
                StatementKind::Break => {}
            },
        }
    }
    expr_has_typevars(&block.tail)
}

fn expr_has_typevars(e: &Expr) -> bool {
    if ty_has_typevars(&e.ty) {
        return true;
    }
    match &e.kind {
        ExprKind::Cast { target, expr } => {
            ty_has_typevars(target) || expr_has_typevars(expr)
        }
        ExprKind::Unary { operand, .. } => expr_has_typevars(operand),
        ExprKind::Binary { lhs, rhs, .. } => expr_has_typevars(lhs) || expr_has_typevars(rhs),
        ExprKind::Block(b) => block_has_typevars(b),
        ExprKind::If { condition, then_branch, else_branch } => {
            expr_has_typevars(condition)
                || expr_has_typevars(then_branch)
                || expr_has_typevars(else_branch)
        }
        ExprKind::While { condition, body } => {
            expr_has_typevars(condition) || block_has_typevars(body)
        }
        ExprKind::Assign { target, value } => {
            expr_has_typevars(target) || expr_has_typevars(value)
        }
        ExprKind::Array(items) => items.iter().any(expr_has_typevars),
        ExprKind::Subscript { expr, index } => {
            expr_has_typevars(expr) || expr_has_typevars(index)
        }
        ExprKind::Ref(t) | ExprKind::Deref(t) => expr_has_typevars(t),
        ExprKind::StructLiteral { fields } => fields.iter().any(|(_, v)| expr_has_typevars(v)),
        ExprKind::Field { target, .. } | ExprKind::TupleField { target, .. } => expr_has_typevars(target),
        ExprKind::Tuple(es) => es.iter().any(expr_has_typevars),
        ExprKind::Call(call) => {
            expr_has_typevars(&call.callee) || call.args.iter().any(expr_has_typevars)
        }
        ExprKind::Intrinsic { args, elem_ty, .. } => {
            ty_has_typevars(elem_ty) || args.iter().any(expr_has_typevars)
        }
        ExprKind::EnumConstruct { args, .. } => args.iter().any(expr_has_typevars),
        ExprKind::Match { scrutinee, arms } => {
            expr_has_typevars(scrutinee) || arms.iter().any(|a| expr_has_typevars(&a.body))
        }
        ExprKind::MakeClosure { env, code } => {
            expr_has_typevars(env) || expr_has_typevars(code)
        }
        ExprKind::ZeroInit(ty) => ty_has_typevars(ty),
        _ => false,
    }
}

fn collect_typevars_in_block(block: &Block, out: &mut Vec<TypeVarId>) {
    for item in &block.items {
        match item {
            BlockItem::Declaration(d) => {
                collect_typevars(&d.ty, out);
                collect_typevars_in_expr(&d.value, out);
            }
            BlockItem::Statement(s) => match &s.kind {
                StatementKind::Expr(e)
                | StatementKind::Return(e)
                | StatementKind::Defer(e) => collect_typevars_in_expr(e, out),
                StatementKind::Break => {}
            },
        }
    }
    collect_typevars_in_expr(&block.tail, out);
}

/// Walks an expression collecting type-vars from explicit type annotations
/// (cast targets, `ZeroInit` types, intrinsic element types) and recurses
/// through children — but DOES NOT walk `e.ty` itself. The type of a
/// `Local(template_id)` expression is the template's signature, whose
/// type-vars belong to the template, not to the surrounding closure body.
/// Including them would leak nested-template type-vars into the closure
/// template's signature, blocking specialization when the outer
/// substitution doesn't know about them.
fn collect_typevars_in_expr(e: &Expr, out: &mut Vec<TypeVarId>) {
    match &e.kind {
        ExprKind::Cast { target, expr } => {
            collect_typevars(target, out);
            collect_typevars_in_expr(expr, out);
        }
        ExprKind::Unary { operand, .. } => collect_typevars_in_expr(operand, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_typevars_in_expr(lhs, out);
            collect_typevars_in_expr(rhs, out);
        }
        ExprKind::Block(b) => collect_typevars_in_block(b, out),
        ExprKind::If { condition, then_branch, else_branch } => {
            collect_typevars_in_expr(condition, out);
            collect_typevars_in_expr(then_branch, out);
            collect_typevars_in_expr(else_branch, out);
        }
        ExprKind::While { condition, body } => {
            collect_typevars_in_expr(condition, out);
            collect_typevars_in_block(body, out);
        }
        ExprKind::Assign { target, value } => {
            collect_typevars_in_expr(target, out);
            collect_typevars_in_expr(value, out);
        }
        ExprKind::Array(items) => items.iter().for_each(|i| collect_typevars_in_expr(i, out)),
        ExprKind::Subscript { expr, index } => {
            collect_typevars_in_expr(expr, out);
            collect_typevars_in_expr(index, out);
        }
        ExprKind::Ref(t) | ExprKind::Deref(t) => collect_typevars_in_expr(t, out),
        ExprKind::StructLiteral { fields } => {
            for (_, v) in fields { collect_typevars_in_expr(v, out); }
        }
        ExprKind::Field { target, .. } | ExprKind::TupleField { target, .. } => {
            collect_typevars_in_expr(target, out);
        }
        ExprKind::Tuple(es) => es.iter().for_each(|e| collect_typevars_in_expr(e, out)),
        ExprKind::Call(call) => {
            collect_typevars_in_expr(&call.callee, out);
            for a in &call.args { collect_typevars_in_expr(a, out); }
        }
        ExprKind::Intrinsic { args, elem_ty, .. } => {
            collect_typevars(elem_ty, out);
            for a in args { collect_typevars_in_expr(a, out); }
        }
        ExprKind::EnumConstruct { args, .. } => {
            for a in args { collect_typevars_in_expr(a, out); }
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_typevars_in_expr(scrutinee, out);
            for a in arms { collect_typevars_in_expr(&a.body, out); }
        }
        ExprKind::MakeClosure { env, code } => {
            collect_typevars_in_expr(env, out);
            collect_typevars_in_expr(code, out);
        }
        ExprKind::ZeroInit(ty) => collect_typevars(ty, out),
        _ => {}
    }
}

/// Walks `expr` and replaces every `Local(id)` whose `id` appears in
/// `captures_by_id` with an env-field load. The replacement reads from
/// `(*(env as *EnvTuple)).index` where `env` is the synth code fn's env
/// param (`env_param_id`).
fn rewrite_captures_in_expr(
    expr: &mut Expr,
    captures_by_id: &HashMap<LocalId, usize>,
    env_param_id: LocalId,
    env_tuple_ty: &Ty,
    span: Span,
) {
    match &mut expr.kind {
        ExprKind::Local(id) => {
            if let Some(&index) = captures_by_id.get(id) {
                let env_ptr_ty = Ty::Ptr(Box::new(env_tuple_ty.clone()));
                let cap_ty = expr.ty.clone();
                let env_local = Expr {
                    span,
                    ty: Ty::Ptr(Box::new(Ty::U8)),
                    kind: ExprKind::Local(env_param_id),
                };
                let env_cast = Expr {
                    span,
                    ty: env_ptr_ty.clone(),
                    kind: ExprKind::Cast {
                        target: env_ptr_ty,
                        expr: Box::new(env_local),
                    },
                };
                let env_deref = Expr {
                    span,
                    ty: env_tuple_ty.clone(),
                    kind: ExprKind::Deref(Box::new(env_cast)),
                };
                *expr = Expr {
                    span,
                    ty: cap_ty,
                    kind: ExprKind::TupleField {
                        target: Box::new(env_deref),
                        index,
                    },
                };
            }
        }
        ExprKind::Const(_)
        | ExprKind::ZeroInit(_)
        | ExprKind::CompError(_)
        | ExprKind::TypeValue(_)
        | ExprKind::ExternFunction { .. }
        | ExprKind::DeferredFunctionRef { .. }
        | ExprKind::Function(_) => {}
        ExprKind::Unary { operand, .. } => {
            rewrite_captures_in_expr(operand, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            rewrite_captures_in_expr(lhs, captures_by_id, env_param_id, env_tuple_ty, span);
            rewrite_captures_in_expr(rhs, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Cast { expr: inner, .. } => {
            rewrite_captures_in_expr(inner, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Call(call) => {
            rewrite_captures_in_expr(&mut call.callee, captures_by_id, env_param_id, env_tuple_ty, span);
            for a in &mut call.args {
                rewrite_captures_in_expr(a, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::Block(b) => {
            for item in &mut b.items {
                match item {
                    BlockItem::Declaration(d) => {
                        rewrite_captures_in_expr(&mut d.value, captures_by_id, env_param_id, env_tuple_ty, span);
                    }
                    BlockItem::Statement(s) => match &mut s.kind {
                        StatementKind::Expr(e)
                        | StatementKind::Return(e)
                        | StatementKind::Defer(e) => {
                            rewrite_captures_in_expr(e, captures_by_id, env_param_id, env_tuple_ty, span);
                        }
                        StatementKind::Break => {}
                    },
                }
            }
            rewrite_captures_in_expr(&mut b.tail, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            rewrite_captures_in_expr(condition, captures_by_id, env_param_id, env_tuple_ty, span);
            rewrite_captures_in_expr(then_branch, captures_by_id, env_param_id, env_tuple_ty, span);
            rewrite_captures_in_expr(else_branch, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::While { condition, body } => {
            rewrite_captures_in_expr(condition, captures_by_id, env_param_id, env_tuple_ty, span);
            for item in &mut body.items {
                match item {
                    BlockItem::Declaration(d) => {
                        rewrite_captures_in_expr(&mut d.value, captures_by_id, env_param_id, env_tuple_ty, span);
                    }
                    BlockItem::Statement(s) => match &mut s.kind {
                        StatementKind::Expr(e)
                        | StatementKind::Return(e)
                        | StatementKind::Defer(e) => {
                            rewrite_captures_in_expr(e, captures_by_id, env_param_id, env_tuple_ty, span);
                        }
                        StatementKind::Break => {}
                    },
                }
            }
            rewrite_captures_in_expr(&mut body.tail, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Assign { target, value } => {
            rewrite_captures_in_expr(target, captures_by_id, env_param_id, env_tuple_ty, span);
            rewrite_captures_in_expr(value, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Array(items) => {
            for it in items {
                rewrite_captures_in_expr(it, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::Subscript { expr: arr, index } => {
            rewrite_captures_in_expr(arr, captures_by_id, env_param_id, env_tuple_ty, span);
            rewrite_captures_in_expr(index, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Ref(inner) | ExprKind::Deref(inner) => {
            rewrite_captures_in_expr(inner, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::StructLiteral { fields } => {
            for (_, v) in fields {
                rewrite_captures_in_expr(v, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::Field { target, .. } | ExprKind::TupleField { target, .. } => {
            rewrite_captures_in_expr(target, captures_by_id, env_param_id, env_tuple_ty, span);
        }
        ExprKind::Tuple(elems) => {
            for e in elems {
                rewrite_captures_in_expr(e, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::Intrinsic { args, .. } => {
            for a in args {
                rewrite_captures_in_expr(a, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::EnumConstruct { args, .. } => {
            for a in args {
                rewrite_captures_in_expr(a, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            rewrite_captures_in_expr(scrutinee, captures_by_id, env_param_id, env_tuple_ty, span);
            for arm in arms {
                rewrite_captures_in_expr(&mut arm.body, captures_by_id, env_param_id, env_tuple_ty, span);
            }
        }
        ExprKind::MakeClosure { env, code } => {
            rewrite_captures_in_expr(env, captures_by_id, env_param_id, env_tuple_ty, span);
            rewrite_captures_in_expr(code, captures_by_id, env_param_id, env_tuple_ty, span);
        }
    }
}
