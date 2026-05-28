use std::collections::{HashMap, HashSet};

use crate::{
    ast,
    hir::{
        TypeVar, TypeVarId, UnaryOperator,
        coerce::{coerce_int_literal, coerce_through_tails},
        error::{Error, ErrorKind},
        generics::{collect_typevars, ty_has_typevars, unify},
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, FunctionCall,
            IntrinsicKind, LocalId, LocalIdGen, Param, Program, Statement, StatementKind,
            StructDef, Ty,
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

    struct_templates: HashMap<String, StructDef>,
    struct_specialization_cache: HashMap<(String, Vec<Ty>), String>,
    struct_template_origin: HashMap<String, (String, Vec<Ty>)>,

    /// Ids of generic templates that came from `#comptime` declarations. When
    /// one is specialized, the substitution pass folds comptime constructs
    /// (type comparisons, `static if`) and drops unreachable code.
    comptime_template_ids: HashSet<LocalId>,
    /// True while lowering a `#comptime` function body — enables lowering of
    /// `TypeValue` / `comperror` constructs that are illegal elsewhere.
    in_comptime: bool,
    /// True while substituting a comptime template's body — enables folding.
    in_comptime_subst: bool,

    /// Top-level function names → every declaration sharing that name (an
    /// overload set, resolved by argument types at the call site).
    overloads: HashMap<String, Vec<LocalId>>,
    /// AST declaration node id → its pre-registered LocalId, so a function body
    /// finds its own id even when the name is overloaded.
    fn_id_by_node: HashMap<crate::ast::NodeId, LocalId>,
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
            struct_templates: HashMap::default(),
            struct_specialization_cache: HashMap::default(),
            struct_template_origin: HashMap::default(),
            comptime_template_ids: HashSet::default(),
            in_comptime: false,
            in_comptime_subst: false,
            overloads: HashMap::default(),
            fn_id_by_node: HashMap::default(),
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

    /// Declares each name from a `<$K, $V>` clause as a fresh type variable in
    /// the current type-var scope, so later bare references (`K`, `V`) resolve.
    /// A type-var scope must already be pushed.
    fn declare_type_params(&mut self, names: &[String], span: Span) -> Result<(), Error> {
        for name in names {
            if self.structs.contains_key(name) || self.struct_templates.contains_key(name) {
                return Err(Error {
                    span,
                    kind: ErrorKind::GenericIsAlsoAStruct { name: name.clone() },
                });
            }
            if self
                .type_var_scopes
                .last()
                .expect("declare_type_params requires a pushed scope")
                .contains_key(name)
            {
                return Err(Error {
                    span,
                    kind: ErrorKind::GenericAlreadyDefined { name: name.clone() },
                });
            }
            let id = self.fresh_type_var_id(name.clone());
            self.type_var_scopes
                .last_mut()
                .unwrap()
                .insert(name.clone(), id);
        }
        Ok(())
    }
    pub fn lower_program(&mut self, p: ast::Program) -> Result<Program, Error> {
        let span = p
            .declarations
            .first()
            .unwrap()
            .span
            .join(p.declarations.last().unwrap().span);

        // Pass 1: register every struct name. Non-generic structs go into
        // `structs` with an empty fields list (filled in pass 2); generic
        // structs go into `struct_templates` similarly. This lets a struct's
        // body reference its own name (via `*T`) and other structs by name
        // before their bodies are lowered.
        for decl in &p.declarations {
            if let ast::ExprKind::StructDef { type_params, .. } = &decl.value.kind {
                if self.structs.contains_key(&decl.name)
                    || self.struct_templates.contains_key(&decl.name)
                {
                    return Err(Error {
                        span: decl.span,
                        kind: ErrorKind::AlreadyDefined {
                            name: decl.name.clone(),
                        },
                    });
                }
                if type_params.is_empty() {
                    self.structs.insert(
                        decl.name.clone(),
                        StructDef {
                            name: decl.name.clone(),
                            type_var_ids: Vec::new(),
                            fields: Vec::new(),
                        },
                    );
                } else {
                    self.struct_templates.insert(
                        decl.name.clone(),
                        StructDef {
                            name: decl.name.clone(),
                            type_var_ids: Vec::new(),
                            fields: Vec::new(),
                        },
                    );
                }
            }
        }

        // Pass 2: lower struct bodies. For generic structs, push a fresh
        // type-var scope, register each type param as a TypeVarId, then
        // lower fields. Field types may contain TypeVars and stay as
        // templates until a concrete use site triggers specialization.
        for decl in &p.declarations {
            if let ast::ExprKind::StructDef {
                type_params,
                fields,
            } = &decl.value.kind
            {
                if type_params.is_empty() {
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
                } else {
                    // Generic struct: type params are introduced from the
                    // explicit `<$K, $V>` list.
                    self.push_type_var_scope();
                    let mut tv_ids = Vec::with_capacity(type_params.len());
                    for name in type_params {
                        if self.structs.contains_key(name)
                            || self.struct_templates.contains_key(name)
                        {
                            self.pop_type_var_scope();
                            return Err(Error {
                                span: decl.span,
                                kind: ErrorKind::GenericIsAlsoAStruct { name: name.clone() },
                            });
                        }
                        let id = self.fresh_type_var_id(name.clone());
                        self.type_var_scopes
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), id);
                        tv_ids.push(id);
                    }

                    let mut lowered_fields = Vec::with_capacity(fields.len());
                    for f in fields {
                        let ty = self.lower_type(&f.ty)?;
                        if let Ty::Struct(other) = &ty
                            && other == &decl.name
                        {
                            self.pop_type_var_scope();
                            return Err(Error {
                                span: f.span,
                                kind: ErrorKind::DirectStructRecursion {
                                    name: decl.name.clone(),
                                },
                            });
                        }
                        lowered_fields.push((f.name.clone(), ty));
                    }
                    self.pop_type_var_scope();

                    let entry = self
                        .struct_templates
                        .get_mut(&decl.name)
                        .expect("registered in pass 1");
                    entry.type_var_ids = tv_ids;
                    entry.fields = lowered_fields;
                }
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
            let is_comptime = decl.comptime;
            self.in_comptime = is_comptime;
            let lowered = self.lower_declaration(decl)?;
            self.in_comptime = false;
            if self.register_if_generic_template(&lowered) {
                if is_comptime {
                    self.comptime_template_ids.insert(lowered.id);
                }
            } else {
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
            type_params,
            params,
            return_ty,
            ..
        } = &d.value.kind
        else {
            return Ok(());
        };

        // Each top-level function gets its own type-variable scope. After
        // the signature is lowered the scope is saved on the side so the
        // body's $T / T references resolve to the same TypeVarIds.
        self.push_type_var_scope();
        let lowered_sig = (|| -> Result<(Vec<Ty>, Ty), Error> {
            self.declare_type_params(type_params, d.span)?;
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
        // Functions may share a name (overloading); the scope keeps the most
        // recent for bare references, while `overloads` keeps the full set and
        // `fn_id_by_node` ties each declaration to its own id.
        self.current_scope_mut().insert(d.name.clone(), id);
        self.overloads.entry(d.name.clone()).or_default().push(id);
        self.fn_id_by_node.insert(d.id, id);
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
        // Top-level functions were pre-registered (by node id, so overloaded
        // names still find the right id). Everything else is a fresh binding.
        let pre_registered_id = if matches!(d.value.kind, ast::ExprKind::Function { .. }) {
            self.fn_id_by_node.get(&d.id).copied()
        } else {
            None
        };

        if pre_registered_id.is_none() && self.current_scope().contains_key(&d.name) {
            return Err(Error {
                span: d.span,
                kind: ErrorKind::AlreadyDefined { name: d.name },
            });
        }

        // Pre-registered top-level functions: lower the body using the
        // already-lowered signature (don't re-call lower_type on `$T`).
        if let Some(id) = pre_registered_id {
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
                // Inline function literals: get their own type-var scope.
                // Top-level functions go through lower_declaration directly
                // (and don't hit this arm).
                self.push_type_var_scope();
                let result = self
                    .declare_type_params(&type_params, e.span)
                    .and_then(|()| self.lower_function_value(params, return_ty, body, e.span));
                self.pop_type_var_scope();
                result
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

                // Overloaded direct call: pick the overload matching the args.
                if let ast::ExprKind::Identifier(name) = &callee.kind
                    && self.overloads.get(name).is_some_and(|c| c.len() > 1)
                {
                    let name = name.clone();
                    let lowered_args: Vec<Expr> = args
                        .into_iter()
                        .map(|a| self.lower_expr(a))
                        .collect::<Result<_, _>>()?;
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
                let args: Vec<Expr> = args
                    .into_iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<Result<_, _>>()?;
                let explicit_type_args: Vec<Ty> = type_args
                    .iter()
                    .map(|t| self.lower_type(t))
                    .collect::<Result<_, _>>()?;
                self.finish_call(callee, args, explicit_type_args, e.span)
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
        }
    }

    /// Finishes a call once the callee and arguments are lowered: coerces
    /// int-literal args against the parameter types, then either specializes a
    /// generic template (when every argument is concrete) or builds the call.
    fn finish_call(
        &mut self,
        callee: Expr,
        args: Vec<Expr>,
        explicit_type_args: Vec<Ty>,
        span: Span,
    ) -> Result<Expr, Error> {
        let Ty::Function { params, return_ty } = callee.ty.clone() else {
            return Err(Error {
                span: callee.span,
                kind: ErrorKind::NotCallable {
                    found: callee.ty.clone(),
                },
            });
        };

        // Coerce int literals against the (possibly generic) param type;
        // coerce_int_literal is a no-op when the target is a TypeVar.
        let mut coerced = Vec::with_capacity(args.len());
        for (i, arg) in args.into_iter().enumerate() {
            let arg = match params.get(i) {
                Some(pty) => coerce_int_literal(arg, pty)?,
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

        let result_ty = *return_ty;
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
    fn lower_method_call(
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

        if let Some((index, field_ty)) = self.resolve_field(&recv.ty, &name) {
            // `recv.field(args)` — the field holds the callable value.
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
            let args: Vec<Expr> = args
                .into_iter()
                .map(|a| self.lower_expr(a))
                .collect::<Result<_, _>>()?;
            return self.finish_call(callee, args, explicit_type_args, span);
        }

        // UFCS: resolve `name` (possibly overloaded) against the receiver and
        // arguments, then call it as `name(recv, args)`.
        let lowered_args: Vec<Expr> = args
            .into_iter()
            .map(|a| self.lower_expr(a))
            .collect::<Result<_, _>>()?;
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
    fn resolve_field(&mut self, ty: &Ty, name: &str) -> Option<(usize, Ty)> {
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
    fn autoderef_to_struct(&self, mut target: Expr) -> Expr {
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
    fn maybe_autoref(&self, recv: Expr, fn_ty: &Ty) -> Expr {
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
    fn fn_params(&self, id: LocalId) -> Vec<Ty> {
        match self.bindings.get(&id) {
            Some(Ty::Function { params, .. }) => params.clone(),
            _ => Vec::new(),
        }
    }

    /// Whether `params` accept `arg_tys`: arity matches and each pair unifies,
    /// treating the parameters' type vars as inference holes.
    fn overload_matches(&self, params: &[Ty], arg_tys: &[Ty]) -> bool {
        if params.len() != arg_tys.len() {
            return false;
        }
        let mut subst = HashMap::new();
        for (p, a) in params.iter().zip(arg_tys.iter()) {
            if unify(p, a, Span::default(), &mut subst, &self.struct_template_origin).is_err() {
                return false;
            }
        }
        true
    }

    /// Picks the unique overload of `name` whose parameters accept `arg_tys`.
    fn resolve_overload(&self, name: &str, arg_tys: &[Ty], span: Span) -> Result<LocalId, Error> {
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
    fn resolve_method(
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
    fn receiver_forms(&self, recv: &Expr) -> Vec<(Ty, RecvAdjust)> {
        let mut forms = vec![(recv.ty.clone(), RecvAdjust::AsIs)];
        if is_place_kind(&recv.kind) {
            forms.push((Ty::Ptr(Box::new(recv.ty.clone())), RecvAdjust::Ref));
        }
        if let Ty::Ptr(inner) = &recv.ty {
            forms.push(((**inner).clone(), RecvAdjust::Deref));
        }
        forms
    }

    fn apply_recv_adjust(&self, recv: Expr, adjust: RecvAdjust) -> Expr {
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
            ast::StatementKind::Defer(expr) => {
                let expr = self.lower_expr(expr)?;
                StatementKind::Defer(expr)
            }
        };

        Ok(Statement { span: s.span, kind })
    }

    fn lower_intrinsic(
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
                } else if self.struct_templates.contains_key(name) {
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
                // Concrete struct? Then `Foo<...>` is wrong — Foo isn't generic.
                if self.structs.contains_key(name) {
                    return Err(Error {
                        span: t.span,
                        kind: ErrorKind::UnexpectedTypeArguments { name: name.clone() },
                    });
                }
                let template = self
                    .struct_templates
                    .get(name)
                    .cloned()
                    .ok_or_else(|| Error {
                        span: t.span,
                        kind: ErrorKind::UnknownType { name: name.clone() },
                    })?;
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
                    Ok(Ty::GenericStruct {
                        name: name.clone(),
                        args: lowered_args,
                    })
                } else {
                    let specialized = self.specialize_struct(&template, lowered_args, t.span)?;
                    Ok(Ty::Struct(specialized))
                }
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

    fn lower_struct_literal(
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
    fn lower_explicit_generic_struct_literal(
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
            let value = self.lower_expr(f.value)?;
            let value = coerce_int_literal(value, &target_ty)?;
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

    fn lower_generic_struct_literal(
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
            ordered.push((fname.clone(), value));
        }

        Ok(Expr {
            span,
            ty: Ty::Struct(specialized_name),
            kind: ExprKind::StructLiteral { fields: ordered },
        })
    }

    /// Generates (or returns the cached) specialization of a generic struct
    /// `template` for the given concrete type arguments. The specialized
    /// struct lives in `self.structs` with a mangled name, ready for codegen.
    fn specialize_struct(
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

    fn substitute_ty(&mut self, ty: &Ty, subst: &HashMap<TypeVarId, Ty>) -> Ty {
        match ty {
            Ty::TypeVar(id) => subst.get(id).cloned().unwrap_or(Ty::TypeVar(*id)),
            Ty::Ptr(inner) => Ty::Ptr(Box::new(self.substitute_ty(inner, subst))),
            Ty::Array { element, count } => Ty::Array {
                element: Box::new(self.substitute_ty(element, subst)),
                count: *count,
            },
            Ty::Function { params, return_ty } => {
                let params: Vec<Ty> = params
                    .iter()
                    .map(|p| self.substitute_ty(p, subst))
                    .collect();
                let return_ty = Box::new(self.substitute_ty(return_ty, subst));
                Ty::Function { params, return_ty }
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
            _ => ty.clone(),
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
            unify(
                &param.ty,
                arg_ty,
                call_span,
                &mut subst,
                &self.struct_template_origin,
            )?;
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

    fn substitute_block_item(
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

    fn substitute_expr(
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

/// How a UFCS receiver is adjusted to match the chosen overload's first param.
#[derive(Clone, Copy)]
enum RecvAdjust {
    AsIs,
    Ref,
    Deref,
}

/// Whether an expression denotes an addressable place (so `&expr` is valid).
fn is_place_kind(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Local(_)
            | ExprKind::Field { .. }
            | ExprKind::Subscript { .. }
            | ExprKind::Deref(_)
    )
}

fn mangle_specialization(name: &str, tys: &[Ty]) -> String {
    let mut s = String::from(name);
    for t in tys {
        s.push('$');
        s.push_str(&format!("{t:?}"));
    }
    s
}

fn mangle_struct_specialization(name: &str, tys: &[Ty]) -> String {
    mangle_specialization(name, tys)
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
