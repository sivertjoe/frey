use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        TypeVar, TypeVarId, UnaryOperator,
        coerce::{coerce_int_literal, coerce_through_tails},
        error::{Error, ErrorKind},
        generics::{collect_typevars, substitute_ty, ty_has_typevars, unify},
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, FunctionCall, LocalId,
            LocalIdGen, Param, Program, Statement, StatementKind, StructDef, Ty,
        },
    },
    lexer::types::Span,
};

struct PendingFnSig {
    scope: HashMap<String, TypeVarId>,
    param_tys: Vec<Ty>,
    return_ty: Ty,
}

#[derive(Clone)]
pub(crate) struct GenericTemplate {
    pub name: String,
    pub span: Span,
    pub type_var_ids: Vec<TypeVarId>,
    pub params: Vec<Param>,
    pub return_ty: Ty,
    pub body: Block,
}

pub struct Lower {
    scopes: Vec<HashMap<String, LocalId>>,
    bindings: HashMap<LocalId, Ty>,
    structs: HashMap<String, StructDef>,
    id_gen: LocalIdGen,

    type_vars: Vec<TypeVar>,
    type_var_scopes: Vec<HashMap<String, TypeVarId>>,
    pending_fn_sigs: HashMap<LocalId, PendingFnSig>,

    pub(crate) templates: HashMap<LocalId, GenericTemplate>,

    // Cache: (template id, concrete arg types in TypeVarId order) →
    // specialization LocalId. The id is reserved BEFORE the specialization's
    // body is processed, so recursive calls find the in-progress entry.
    specialization_cache: HashMap<(LocalId, Vec<Ty>), LocalId>,
    pending_specializations: Vec<Declaration>,
}

impl Lower {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::default()], // Add the global scope
            bindings: HashMap::default(),
            structs: HashMap::default(),
            id_gen: LocalIdGen::new(),
            type_vars: Vec::default(),
            type_var_scopes: Vec::default(),
            pending_fn_sigs: HashMap::default(),
            templates: HashMap::default(),
            specialization_cache: HashMap::default(),
            pending_specializations: Vec::default(),
        }
    }

    fn push_type_var_scope(&mut self) {
        self.type_var_scopes.push(HashMap::new());
    }
    fn pop_type_var_scope(&mut self) -> HashMap<String, TypeVarId> {
        self.type_var_scopes
            .pop()
            .expect("type_var_scopes underflow")
    }
    fn lookup_type_var(&self, name: &str) -> Option<TypeVarId> {
        for s in self.type_var_scopes.iter().rev() {
            if let Some(&id) = s.get(name) {
                return Some(id);
            }
        }
        None
    }
    fn fresh_type_var_id(&mut self, name: String) -> TypeVarId {
        let id = TypeVarId(self.type_vars.len() as u32);
        self.type_vars.push(TypeVar { name });
        id
    }
    pub fn lower_program(&mut self, p: ast::Program) -> Result<Program, Error> {
        let span = p
            .declarations
            .first()
            .unwrap()
            .span
            .join(p.declarations.last().unwrap().span);

        // Pass 1: register every struct name with empty fields. This lets a
        // struct's body (and any other struct's body) reference it via `*T`
        // or by name before its own body is lowered.
        for decl in &p.declarations {
            if let ast::ExprKind::StructDef { .. } = &decl.value.kind {
                if self.structs.contains_key(&decl.name) {
                    return Err(Error {
                        span: decl.span,
                        kind: ErrorKind::AlreadyDefined {
                            name: decl.name.clone(),
                        },
                    });
                }
                self.structs.insert(
                    decl.name.clone(),
                    StructDef {
                        name: decl.name.clone(),
                        fields: Vec::new(),
                    },
                );
            }
        }

        // Pass 2: lower struct bodies. Each field type is resolved; struct
        // names referenced via *T resolve to the placeholder from pass 1.
        for decl in &p.declarations {
            if let ast::ExprKind::StructDef { fields } = &decl.value.kind {
                let mut lowered_fields = Vec::with_capacity(fields.len());
                for f in fields {
                    let ty = self.lower_type(&f.ty)?;
                    if let Ty::Struct(other) = &ty
                        && other == &decl.name
                    {
                        return Err(Error {
                            span: f.span,
                            kind: ErrorKind::DirectStructRecursion {
                                name: decl.name.clone(),
                            },
                        });
                    }
                    lowered_fields.push((f.name.clone(), ty));
                }
                self.structs
                    .get_mut(&decl.name)
                    .expect("registered in pass 1")
                    .fields = lowered_fields;
            }
        }

        // Pass 3: function signatures.
        for decl in &p.declarations {
            self.pre_register_top_level(decl)?;
        }

        // Pass 4: lower the remaining declarations. Struct defs are skipped
        // since they're already recorded in self.structs. Generic functions
        // are detected after lowering and routed into the templates registry
        // instead of being emitted as declarations.
        let mut decls = Vec::new();
        for decl in p.declarations {
            if matches!(decl.value.kind, ast::ExprKind::StructDef { .. }) {
                continue;
            }
            let lowered = self.lower_declaration(decl)?;
            if !self.register_if_generic_template(&lowered) {
                decls.push(lowered);
            }
        }

        // Pass 5: drain specializations produced during body lowering. Each
        // specialization may itself produce more specializations (recursive
        // case), so we loop until quiescent.
        while !self.pending_specializations.is_empty() {
            let batch = std::mem::take(&mut self.pending_specializations);
            for spec in batch {
                decls.push(spec);
            }
        }

        Ok(Program {
            span,
            declarations: decls,
            structs: std::mem::take(&mut self.structs),
        })
    }

    /// If `decl` is a function whose signature contains a TypeVar, move it
    /// out of regular declarations into the generic-templates registry.
    /// Returns true if the declaration was consumed.
    fn register_if_generic_template(&mut self, decl: &Declaration) -> bool {
        let ExprKind::Function(func) = &decl.value.kind else {
            return false;
        };
        let is_generic =
            func.params.iter().any(|p| ty_has_typevars(&p.ty)) || ty_has_typevars(&func.return_ty);
        if !is_generic {
            return false;
        }
        // Collect TypeVarIds in declaration order.
        let mut seen = Vec::new();
        for p in &func.params {
            collect_typevars(&p.ty, &mut seen);
        }
        collect_typevars(&func.return_ty, &mut seen);
        self.templates.insert(
            decl.id,
            GenericTemplate {
                name: decl.name.clone(),
                span: decl.span,
                type_var_ids: seen,
                params: func.params.clone(),
                return_ty: func.return_ty.clone(),
                body: func.body.clone(),
            },
        );
        true
    }

    fn pre_register_top_level(&mut self, d: &ast::Declaration) -> Result<(), Error> {
        let ast::ExprKind::Function {
            params, return_ty, ..
        } = &d.value.kind
        else {
            return Ok(());
        };

        if self.current_scope().contains_key(&d.name) {
            return Err(Error {
                span: d.span,
                kind: ErrorKind::AlreadyDefined {
                    name: d.name.clone(),
                },
            });
        }

        // Each top-level function gets its own type-variable scope. After
        // the signature is lowered the scope is saved on the side so the
        // body's $T / T references resolve to the same TypeVarIds.
        self.push_type_var_scope();
        let lowered_sig = (|| -> Result<(Vec<Ty>, Ty), Error> {
            let param_tys = params
                .iter()
                .map(|p| self.lower_type(&p.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let ret = match return_ty {
                Some(t) => self.lower_type(t)?,
                None => Ty::Unit,
            };
            Ok((param_tys, ret))
        })();
        let scope = self.pop_type_var_scope();
        let (param_tys, return_ty) = lowered_sig?;

        let ty = Ty::Function {
            params: param_tys.clone(),
            return_ty: Box::new(return_ty.clone()),
        };

        let id = self.id_gen.fresh();
        self.current_scope_mut().insert(d.name.clone(), id);
        self.bindings.insert(id, ty);
        self.pending_fn_sigs.insert(
            id,
            PendingFnSig {
                scope,
                param_tys,
                return_ty,
            },
        );

        Ok(())
    }

    fn lower_declaration(&mut self, d: ast::Declaration) -> Result<Declaration, Error> {
        let pre_registered = matches!(d.value.kind, ast::ExprKind::Function { .. })
            && self.current_scope().contains_key(&d.name);

        if !pre_registered && self.current_scope().contains_key(&d.name) {
            return Err(Error {
                span: d.span,
                kind: ErrorKind::AlreadyDefined { name: d.name },
            });
        }

        // Pre-registered top-level functions: lower the body using the
        // already-lowered signature (don't re-call lower_type on `$T`).
        if pre_registered {
            let id = *self.current_scope().get(&d.name).unwrap();
            let sig = self
                .pending_fn_sigs
                .remove(&id)
                .expect("pre_register_top_level must save a sig for every function");

            let ast::ExprKind::Function { params, body, .. } = d.value.kind else {
                unreachable!("pre_registered guarantees Function");
            };

            let value = self.lower_top_level_function_body(params, body, sig, d.value.span)?;
            let ty = value.ty.clone();
            return Ok(Declaration {
                id,
                span: d.span,
                mutable: d.mutable,
                name: d.name,
                ty,
                value,
            });
        }

        let value = self.lower_expr(d.value)?;
        let ty = value.ty.clone();
        let id = self.id_gen.fresh();
        self.current_scope_mut().insert(d.name.clone(), id);
        self.bindings.insert(id, ty.clone());

        Ok(Declaration {
            id,
            span: d.span,
            mutable: d.mutable,
            name: d.name,
            ty,
            value,
        })
    }

    fn lower_top_level_function_body(
        &mut self,
        params: Vec<ast::Param>,
        body: ast::Block,
        sig: PendingFnSig,
        span: crate::lexer::types::Span,
    ) -> Result<Expr, Error> {
        // Re-push the type-var scope so any `T` references inside the body
        // resolve to the TypeVarIds chosen during pre-register.
        self.type_var_scopes.push(sig.scope);

        self.enter_scope();
        let mut hir_params = Vec::with_capacity(params.len());
        for (p, ty) in params.into_iter().zip(sig.param_tys.iter()) {
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(p.name.clone(), id);
            self.bindings.insert(id, ty.clone());
            hir_params.push(Param {
                id,
                span: p.span,
                name: p.name,
                ty: ty.clone(),
            });
        }

        let body_result = self.lower_block(body);
        self.leave_scope();
        self.pop_type_var_scope();

        let mut body = body_result?;
        if body.tail.ty != sig.return_ty && sig.return_ty.is_number() && sig.return_ty != Ty::Int {
            let tail = std::mem::replace(&mut body.tail, Box::new(unit_expr(body.span)));
            body.tail = Box::new(coerce_through_tails(*tail, &sig.return_ty)?);
        }

        let fn_ty = Ty::Function {
            params: hir_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(sig.return_ty.clone()),
        };
        Ok(Expr {
            span,
            ty: fn_ty,
            kind: ExprKind::Function(Function {
                params: hir_params,
                return_ty: sig.return_ty,
                body,
            }),
        })
    }
    fn lower_expr(&mut self, e: ast::Expr) -> Result<Expr, Error> {
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
            ast::ExprKind::Identifier(name) => {
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

                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Local(local_id),
                })
            }
            ast::ExprKind::Function {
                params,
                return_ty,
                body,
            } => {
                // Inline function literals: get their own type-var scope.
                // Top-level functions go through lower_declaration directly
                // (and don't hit this arm).
                self.push_type_var_scope();
                let result = self.lower_function_value(params, return_ty, body, e.span);
                self.pop_type_var_scope();
                result
            }
            ast::ExprKind::Call { callee, args } => {
                let callee = self.lower_expr(*callee)?;

                let Ty::Function { params, return_ty } = callee.ty.clone() else {
                    return Err(Error {
                        span: callee.span,
                        kind: ErrorKind::NotCallable {
                            found: callee.ty.clone(),
                        },
                    });
                };

                // Lower args. We coerce int literals against the (possibly
                // generic) param type; coerce_int_literal is a no-op when the
                // target is a TypeVar.
                let mut lowered_args = Vec::with_capacity(args.len());
                for (i, a) in args.into_iter().enumerate() {
                    let arg = self.lower_expr(a)?;
                    let arg = match params.get(i) {
                        Some(pty) => coerce_int_literal(arg, pty)?,
                        None => arg,
                    };
                    lowered_args.push(arg);
                }
                let args = lowered_args;

                // Specialize generic calls when every arg is concrete.
                // Generic calls inside other generic bodies (args still have
                // TypeVars) are left as-is; the outer specialization pass
                // will re-visit and specialize them with concrete arg types.
                let callee_local = if let ExprKind::Local(id) = callee.kind {
                    Some(id)
                } else {
                    None
                };
                if let Some(callee_id) = callee_local
                    && self.templates.contains_key(&callee_id)
                {
                    let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
                    if !arg_tys.iter().any(ty_has_typevars) {
                        let new_id = self.specialize_call(callee_id, &arg_tys, e.span)?;
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
                            span: e.span,
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

                let result_ty = *return_ty;
                Ok(Expr {
                    span: e.span,
                    ty: result_ty,
                    kind: ExprKind::Call(FunctionCall {
                        callee: Box::new(callee),
                        args,
                    }),
                })
            }
            ast::ExprKind::Unary { op, expr } => {
                let operand = self.lower_expr(*expr)?;
                let ty = operand.ty.clone();
                let op = self.lower_unary(op);
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
                let lhs = self.lower_expr(*lhs)?;
                let rhs = self.lower_expr(*rhs)?;
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
                let b = self.lower_block(block)?;
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

                let then_block = self.lower_block(then_branch)?;
                let then_span = then_block.span;
                let then_expr = Expr {
                    span: then_span,
                    ty: then_block.tail.ty.clone(),
                    kind: ExprKind::Block(then_block),
                };

                let else_expr = match else_branch {
                    Some(else_branch) => self.lower_expr(*else_branch)?,
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
                let value = self.lower_expr(*value)?;
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
                let Ty::Array { element, .. } = target.ty.clone() else {
                    return Err(Error {
                        span: target.span,
                        kind: ErrorKind::NotIndexable {
                            found: target.ty.clone(),
                        },
                    });
                };
                Ok(Expr {
                    span: e.span,
                    ty: *element,
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
            ast::ExprKind::StructLiteral { name, fields } => {
                let def = self.structs.get(&name).cloned().ok_or_else(|| Error {
                    span: e.span,
                    kind: ErrorKind::UnknownType { name: name.clone() },
                })?;

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
                    let value = self.lower_expr(f.value)?;
                    let value = coerce_int_literal(value, &target_ty)?;
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
                        span: e.span,
                        kind: ErrorKind::MissingFields {
                            struct_name: name,
                            missing,
                        },
                    });
                }

                Ok(Expr {
                    span: e.span,
                    ty: Ty::Struct(def.name),
                    kind: ExprKind::StructLiteral { fields: ordered },
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
                let struct_name = match &target.ty {
                    Ty::Struct(n) => n.clone(),
                    other => {
                        return Err(Error {
                            span: target.span,
                            kind: ErrorKind::NotAStruct {
                                found: other.clone(),
                            },
                        });
                    }
                };
                let def = self
                    .structs
                    .get(&struct_name)
                    .expect("typed Struct must be registered");
                let (index, field_ty) = def
                    .fields
                    .iter()
                    .enumerate()
                    .find_map(|(i, (n, t))| (n == &name).then_some((i, t.clone())))
                    .ok_or_else(|| Error {
                        span: e.span,
                        kind: ErrorKind::UnknownField {
                            struct_name: struct_name.clone(),
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
        }
    }

    fn lower_function_value(
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

        let mut body = self.lower_block(body)?;
        if body.tail.ty != return_ty && return_ty.is_number() && return_ty != Ty::Int {
            let tail = std::mem::replace(&mut body.tail, Box::new(unit_expr(body.span)));
            body.tail = Box::new(coerce_through_tails(*tail, &return_ty)?);
        }

        self.leave_scope();

        let ty = Ty::Function {
            params: param_tys,
            return_ty: Box::new(return_ty.clone()),
        };

        Ok(Expr {
            span,
            ty,
            kind: ExprKind::Function(Function {
                params: hir_params,
                return_ty,
                body,
            }),
        })
    }

    fn lower_block(&mut self, b: ast::Block) -> Result<Block, Error> {
        let mut items = Vec::new();

        for item in b.items {
            match item {
                ast::BlockItem::Declaration(d) => {
                    let d = self.lower_declaration(d)?;
                    items.push(BlockItem::Declaration(d));
                }
                ast::BlockItem::Statement(statement) => {
                    let s = self.lower_statement(statement)?;
                    items.push(BlockItem::Statement(s));
                }
            }
        }

        let tail = match b.tail {
            Some(expr) => Box::new(self.lower_expr(*expr)?),
            None => Box::new(unit_expr(end_of(b.span))),
        };

        Ok(Block {
            span: b.span,
            items,
            tail,
        })
    }

    fn lower_statement(&mut self, s: ast::Statement) -> Result<Statement, Error> {
        let kind = match s.kind {
            ast::StatementKind::Return(expr) => {
                let expr = match expr {
                    Some(e) => self.lower_expr(e)?,
                    None => unit_expr(end_of(s.span)),
                };
                StatementKind::Return(expr)
            }
            ast::StatementKind::Expr(expr) => {
                let expr = self.lower_expr(expr)?;
                StatementKind::Expr(expr)
            }
            ast::StatementKind::Break => StatementKind::Break,
        };

        Ok(Statement { span: s.span, kind })
    }

    fn lower_type(&mut self, t: &ast::TypeExpr) -> Result<Ty, Error> {
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
                let params = params
                    .iter()
                    .map(|p| self.lower_type(p))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_ty = Box::new(self.lower_type(return_ty)?);
                Ok(Ty::Function { params, return_ty })
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
                } else if let Some(id) = self.lookup_type_var(name) {
                    Ok(Ty::TypeVar(id))
                } else {
                    Err(Error {
                        span: t.span,
                        kind: ErrorKind::UnknownType { name: name.clone() },
                    })
                }
            }
        }
    }

    fn lower_unary(&self, op: ast::UnaryOperator) -> UnaryOperator {
        match op {
            ast::UnaryOperator::Not => UnaryOperator::Not,
            ast::UnaryOperator::Minus => UnaryOperator::Minus,
        }
    }

    /// Produces a specialization of the template identified by `template_id`
    /// for the given concrete argument types, returning the LocalId of the
    /// specialized function. The id is reserved up front and cached so
    /// recursive calls inside the template body resolve to the same id.
    fn specialize_call(
        &mut self,
        template_id: LocalId,
        arg_tys: &[Ty],
        call_span: crate::lexer::types::Span,
    ) -> Result<LocalId, Error> {
        let template = self
            .templates
            .get(&template_id)
            .expect("specialize_call: template_id is not a generic template")
            .clone();

        // Build subst by unifying each (param, arg) pair.
        let mut subst: HashMap<TypeVarId, Ty> = HashMap::new();
        if template.params.len() != arg_tys.len() {
            return Err(Error {
                span: call_span,
                kind: ErrorKind::TypeMismatch {
                    expected: Ty::Function {
                        params: template.params.iter().map(|p| p.ty.clone()).collect(),
                        return_ty: Box::new(template.return_ty.clone()),
                    },
                    found: Ty::Function {
                        params: arg_tys.to_vec(),
                        return_ty: Box::new(Ty::Unit),
                    },
                },
            });
        }
        for (param, arg_ty) in template.params.iter().zip(arg_tys.iter()) {
            unify(&param.ty, arg_ty, call_span, &mut subst)?;
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
        for p in &template.params {
            let new_param_id = self.id_gen.fresh();
            new_params.push(Param {
                id: new_param_id,
                span: p.span,
                name: p.name.clone(),
                ty: substitute_ty(&p.ty, &subst),
            });
        }
        let new_return_ty = substitute_ty(&template.return_ty, &subst);
        let new_fn_ty = Ty::Function {
            params: new_params.iter().map(|p| p.ty.clone()).collect(),
            return_ty: Box::new(new_return_ty.clone()),
        };
        self.bindings.insert(new_id, new_fn_ty.clone());

        // Walk and substitute the body. References to template params (by
        // old LocalId) get remapped to the new param LocalIds.
        let mut new_body = template.body.clone();
        let mut local_map: HashMap<LocalId, LocalId> = HashMap::new();
        for (p_old, p_new) in template.params.iter().zip(new_params.iter()) {
            local_map.insert(p_old.id, p_new.id);
        }
        self.substitute_block(&mut new_body, &subst, &mut local_map)?;

        // Build the specialization's Declaration. Compute the mangled name
        // up front so we don't keep borrowing `template.name` while
        // mutably calling substitute_block.
        let template_name = template.name.clone();
        let template_span = template.span;
        drop(template);

        let mangled = mangle_specialization(&template_name, &key_tys);
        let value = Expr {
            span: template_span,
            ty: new_fn_ty.clone(),
            kind: ExprKind::Function(Function {
                params: new_params,
                return_ty: new_return_ty,
                body: new_body,
            }),
        };
        let decl = Declaration {
            id: new_id,
            span: template_span,
            mutable: false,
            name: mangled,
            ty: new_fn_ty,
            value,
        };
        self.pending_specializations.push(decl);

        Ok(new_id)
    }

    fn substitute_block(
        &mut self,
        block: &mut Block,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        for item in &mut block.items {
            match item {
                BlockItem::Declaration(d) => {
                    d.ty = substitute_ty(&d.ty, subst);
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
                },
            }
        }
        self.substitute_expr(&mut block.tail, subst, local_map)?;
        Ok(())
    }

    fn substitute_expr(
        &mut self,
        e: &mut Expr,
        subst: &HashMap<TypeVarId, Ty>,
        local_map: &mut HashMap<LocalId, LocalId>,
    ) -> Result<(), Error> {
        e.ty = substitute_ty(&e.ty, subst);
        // When a nested call is re-specialized, we need to update the Call
        // expression's result type to match the new specialization's return
        // type (its old ty was the template's return_ty, which may still
        // contain TypeVars that this substitution didn't bind).
        let mut updated_call_ty: Option<Ty> = None;
        match &mut e.kind {
            ExprKind::Const(_) => {}
            ExprKind::Local(id) => {
                if let Some(&new) = local_map.get(id) {
                    *id = new;
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
                        let new_id = self.specialize_call(callee_id, &arg_tys, e.span)?;
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
                *target = substitute_ty(target, subst);
                self.substitute_expr(expr, subst, local_map)?;
            }
            ExprKind::Unary { operand, .. } => self.substitute_expr(operand, subst, local_map)?,
            ExprKind::Block(b) => self.substitute_block(b, subst, local_map)?,
            ExprKind::Binary { lhs, rhs, .. } => {
                self.substitute_expr(lhs, subst, local_map)?;
                self.substitute_expr(rhs, subst, local_map)?;
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
        }
        if let Some(ty) = updated_call_ty {
            e.ty = ty;
        }
        Ok(())
    }
}

fn mangle_specialization(name: &str, tys: &[Ty]) -> String {
    let mut s = String::from(name);
    for t in tys {
        s.push('$');
        s.push_str(&format!("{t:?}"));
    }
    s
}

impl Lower {
    fn current_scope(&self) -> &HashMap<String, LocalId> {
        self.scopes.last().unwrap()
    }
    fn current_scope_mut(&mut self) -> &mut HashMap<String, LocalId> {
        self.scopes.last_mut().unwrap()
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn leave_scope(&mut self) {
        self.scopes.pop();
    }
    fn resolve(&self, name: &str) -> Option<LocalId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }
}

fn unit_expr(span: crate::lexer::types::Span) -> Expr {
    Expr {
        span,
        ty: Ty::Unit,
        kind: ExprKind::Const(Const::Unit),
    }
}

fn end_of(span: crate::lexer::types::Span) -> crate::lexer::types::Span {
    crate::lexer::types::Span {
        start: span.end,
        end: span.end,
    }
}
