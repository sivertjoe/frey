use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        UnaryOperator,
        coerce::{coerce_int_literal, coerce_through_tails},
        error::{Error, ErrorKind},
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, FunctionCall, LocalId,
            LocalIdGen, Param, Program, Statement, StatementKind, StructDef, Ty,
        },
    },
};

pub struct Lower {
    scopes: Vec<HashMap<String, LocalId>>,
    bindings: HashMap<LocalId, Ty>,
    structs: HashMap<String, StructDef>,
    id_gen: LocalIdGen,
}

impl Lower {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::default()], // Add the global scope
            bindings: HashMap::default(),
            structs: HashMap::default(),
            id_gen: LocalIdGen::new(),
        }
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
        // since they're already recorded in self.structs.
        let mut decls = Vec::new();
        for decl in p.declarations {
            if matches!(decl.value.kind, ast::ExprKind::StructDef { .. }) {
                continue;
            }
            decls.push(self.lower_declaration(decl)?);
        }

        Ok(Program {
            span,
            declarations: decls,
            structs: std::mem::take(&mut self.structs),
        })
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

        let param_tys = params
            .iter()
            .map(|p| self.lower_type(&p.ty))
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = Box::new(match return_ty {
            Some(t) => self.lower_type(t)?,
            None => Ty::Unit,
        });
        let ty = Ty::Function {
            params: param_tys,
            return_ty,
        };

        let id = self.id_gen.fresh();
        self.current_scope_mut().insert(d.name.clone(), id);
        self.bindings.insert(id, ty);
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

        let value = self.lower_expr(d.value)?;
        let ty = value.ty.clone();

        let id = if pre_registered {
            *self.current_scope().get(&d.name).unwrap()
        } else {
            let id = self.id_gen.fresh();
            self.current_scope_mut().insert(d.name.clone(), id);
            self.bindings.insert(id, ty.clone());
            id
        };

        Ok(Declaration {
            id,
            span: d.span,
            mutable: d.mutable,
            name: d.name,
            ty,
            value,
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
                if body.tail.ty != return_ty
                    && return_ty.is_number()
                    && return_ty != Ty::Int
                {
                    let tail = std::mem::replace(&mut body.tail, Box::new(unit_expr(body.span)));
                    body.tail = Box::new(coerce_through_tails(*tail, &return_ty)?);
                }

                self.leave_scope();

                let ty = Ty::Function {
                    params: param_tys,
                    return_ty: Box::new(return_ty.clone()),
                };

                Ok(Expr {
                    span: e.span,
                    ty,
                    kind: ExprKind::Function(Function {
                        params: hir_params,
                        return_ty,
                        body,
                    }),
                })
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
                let result_ty = *return_ty;

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
                            kind: ErrorKind::NotDereferencable { found: other.clone() },
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
                if self.structs.contains_key(name) {
                    Ok(Ty::Struct(name.clone()))
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
