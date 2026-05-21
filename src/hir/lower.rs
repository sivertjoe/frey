use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        UnaryOperator,
        error::{Error, ErrorKind},
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, FunctionCall, LocalId,
            LocalIdGen, Param, Program, Statement, StatementKind, Ty,
        },
    },
};

pub struct Lower {
    scopes: Vec<HashMap<String, LocalId>>,
    bindings: HashMap<LocalId, Ty>,
    id_gen: LocalIdGen,
}

impl Lower {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::default()], // Add the global scope
            bindings: HashMap::default(),
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

        for decl in &p.declarations {
            self.pre_register_top_level(decl)?;
        }

        let mut decls = Vec::new();
        for decl in p.declarations {
            decls.push(self.lower_declaration(decl)?);
        }

        Ok(Program {
            span,
            declarations: decls,
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

                let body = self.lower_block(body)?;

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
                let then_ty = then_block.tail.ty.clone();
                let then_span = then_block.span;
                let then_expr = Expr {
                    span: then_span,
                    ty: then_ty.clone(),
                    kind: ExprKind::Block(then_block),
                };

                let else_expr = match else_branch {
                    Some(else_branch) => self.lower_expr(*else_branch)?,
                    None => unit_expr(end_of(e.span)),
                };

                Ok(Expr {
                    span: e.span,
                    ty: then_ty,
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

fn const_int_literal(e: &Expr) -> Option<i32> {
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

fn fits_in_ty(v: i32, ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::I32 | Ty::I64 => true,
        Ty::I8 => i8::try_from(v).is_ok(),
        Ty::U8 => u8::try_from(v).is_ok(),
        Ty::UInt | Ty::U32 | Ty::U64 => v >= 0,
        Ty::Float | Ty::F32 | Ty::F64 => true,
        _ => false,
    }
}

fn coerce_int_literal(expr: Expr, target: &Ty) -> Result<Expr, Error> {
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
