use std::collections::HashMap;

use crate::{
    ast,
    hir::{
        error::{Error, ErrorKind},
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, Function, LocalId, LocalIdGen,
            Param, Program, Statement, StatementKind, Ty,
        },
    },
};

pub struct Lower {
    scopes: Vec<HashMap<String, LocalId>>,
    bindings: HashMap<LocalId, Ty>,
    id_gen: LocalIdGen,
    expected_return: Vec<Ty>, // enclosing function return types
}

impl Lower {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::default()], // Add the global scope
            bindings: HashMap::default(),
            id_gen: LocalIdGen::new(),
            expected_return: Vec::new(),
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
                kind: ErrorKind::AlreadyDefined { name: d.name.clone() },
            });
        }

        let param_tys = params
            .iter()
            .map(|p| self.lower_type(&p.ty))
            .collect::<Result<Vec<_>, _>>()?;
        let return_ty = Box::new(self.lower_type(return_ty)?);
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
                let return_ty = self.lower_type(&return_ty)?;

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

                self.expected_return.push(return_ty.clone());
                let body = self.lower_block(body)?;
                self.expected_return.pop();

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
            Some(expr) => Some(Box::new(self.lower_expr(*expr)?)),
            None => None,
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
                let expr = self.lower_expr(expr)?;
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
