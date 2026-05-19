use crate::hir::types::{
    Block, BlockItem, Declaration, Expr, ExprKind, Function, FunctionCall, Program, Statement,
    StatementKind, Ty,
};
use crate::semantics::error::{Error, ErrorKind};

pub fn type_check(program: &Program) -> Result<(), Error> {
    Typechecker::new().check_program(program)
}

struct Typechecker {
    expected_return: Vec<Ty>,
}

impl Typechecker {
    fn new() -> Self {
        Self {
            expected_return: Vec::new(),
        }
    }

    fn check_program(&mut self, p: &Program) -> Result<(), Error> {
        for decl in &p.declarations {
            self.check_declaration(decl)?;
        }
        Ok(())
    }

    fn check_declaration(&mut self, d: &Declaration) -> Result<(), Error> {
        self.check_expr(&d.value)
    }

    fn check_expr(&mut self, e: &Expr) -> Result<(), Error> {
        match &e.kind {
            ExprKind::Const(_) => Ok(()),
            ExprKind::Local(_) => Ok(()),
            ExprKind::Function(func) => self.check_function(func),
            ExprKind::Call(call) => self.check_call(call, e),
            ExprKind::Unary { operand, .. } => {
                self.check_expr(operand)?;
                if operand.ty != Ty::Int {
                    return Err(Error {
                        span: operand.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: Ty::Int,
                            found: operand.ty.clone(),
                        },
                    });
                }
                Ok(())
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.check_expr(lhs)?;
                self.check_expr(rhs)?;
                if lhs.ty != Ty::Int {
                    return Err(Error {
                        span: lhs.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: Ty::Int,
                            found: lhs.ty.clone(),
                        },
                    });
                }
                if rhs.ty != Ty::Int {
                    return Err(Error {
                        span: rhs.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: Ty::Int,
                            found: rhs.ty.clone(),
                        },
                    });
                }
                Ok(())
            }
        }
    }

    fn check_function(&mut self, f: &Function) -> Result<(), Error> {
        self.expected_return.push(f.return_ty.clone());
        self.check_body(&f.body, &f.return_ty)?;
        self.expected_return.pop();
        Ok(())
    }

    fn check_body(&mut self, body: &Block, return_ty: &Ty) -> Result<(), Error> {
        for item in &body.items {
            match item {
                BlockItem::Declaration(d) => self.check_declaration(d)?,
                BlockItem::Statement(s) => self.check_statement(s)?,
            }
        }
        if let Some(tail) = &body.tail {
            self.check_expr(tail)?;
            if &tail.ty != return_ty {
                return Err(Error {
                    span: tail.span,
                    kind: ErrorKind::TypeMismatch {
                        expected: return_ty.clone(),
                        found: tail.ty.clone(),
                    },
                });
            }
        } else {
            let ends_with_return = matches!(
                body.items.last(),
                Some(BlockItem::Statement(stmt))
                    if matches!(stmt.kind, StatementKind::Return(_))
            );
            if !ends_with_return {
                return Err(Error {
                    span: body.span,
                    kind: ErrorKind::MissingReturn {
                        expected: return_ty.clone(),
                    },
                });
            }
        }
        Ok(())
    }

    fn check_statement(&mut self, s: &Statement) -> Result<(), Error> {
        match &s.kind {
            StatementKind::Return(e) => {
                self.check_expr(e)?;
                let expected = self
                    .expected_return
                    .last()
                    .expect("return outside any function body")
                    .clone();
                if e.ty != expected {
                    return Err(Error {
                        span: e.span,
                        kind: ErrorKind::TypeMismatch {
                            expected,
                            found: e.ty.clone(),
                        },
                    });
                }
                Ok(())
            }
            StatementKind::Expr(e) => self.check_expr(e),
        }
    }

    fn check_call(&mut self, call: &FunctionCall, call_expr: &Expr) -> Result<(), Error> {
        self.check_expr(&call.callee)?;
        for arg in &call.args {
            self.check_expr(arg)?;
        }

        let Ty::Function { params, .. } = &call.callee.ty else {
            unreachable!("lowering ensures the callee has a function type");
        };

        if call.args.len() != params.len() {
            return Err(Error {
                span: call_expr.span,
                kind: ErrorKind::ArityMismatch {
                    expected: params.len(),
                    found: call.args.len(),
                },
            });
        }

        for (arg, expected) in call.args.iter().zip(params.iter()) {
            if &arg.ty != expected {
                return Err(Error {
                    span: arg.span,
                    kind: ErrorKind::TypeMismatch {
                        expected: expected.clone(),
                        found: arg.ty.clone(),
                    },
                });
            }
        }
        Ok(())
    }
}
