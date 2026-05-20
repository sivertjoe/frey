use crate::hir::types::{
    BinaryOperator, Block, BlockItem, Declaration, Expr, ExprKind, Function, FunctionCall, Program,
    Statement, StatementKind, Ty, UnaryOperator,
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
            ExprKind::Unary { operand, op } => {
                self.check_expr(operand)?;
                match op {
                    // `-x` works on any number type (Int or Float).
                    UnaryOperator::Minus => {
                        if !operand.ty.is_number() {
                            return Err(Error {
                                span: operand.span,
                                kind: ErrorKind::TypeMismatch {
                                    expected: Ty::Int,
                                    found: operand.ty.clone(),
                                },
                            });
                        }
                    }
                    // `!x` is "is zero"; meaningful on any integer (Int or UInt).
                    UnaryOperator::Not => {
                        if !operand.ty.is_integer() {
                            return Err(Error {
                                span: operand.span,
                                kind: ErrorKind::TypeMismatch {
                                    expected: Ty::Int,
                                    found: operand.ty.clone(),
                                },
                            });
                        }
                    }
                }
                Ok(())
            }
            ExprKind::Cast { target, expr } => {
                self.check_expr(expr)?;
                if !expr.ty.is_number() || !target.is_number() {
                    return Err(Error {
                        span: expr.span,
                        kind: ErrorKind::IllegalCast { ty: target.clone() },
                    });
                }
                Ok(())
            }
            ExprKind::Binary { lhs, rhs, op } => {
                self.check_expr(lhs)?;
                self.check_expr(rhs)?;
                self.check_binary_op(*op, lhs, rhs)?;
                Ok(())
            }
            ExprKind::Block(block) => {
                for item in &block.items {
                    match item {
                        BlockItem::Declaration(d) => self.check_declaration(d)?,
                        BlockItem::Statement(s) => self.check_statement(s)?,
                    }
                }
                self.check_expr(&block.tail)?;
                Ok(())
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.check_expr(condition)?;
                if !condition.ty.is_integer() {
                    return Err(Error {
                        span: condition.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: Ty::Int,
                            found: condition.ty.clone(),
                        },
                    });
                }
                self.check_expr(then_branch)?;
                self.check_expr(else_branch)?;
                if then_branch.ty != else_branch.ty {
                    return Err(Error {
                        span: else_branch.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: then_branch.ty.clone(),
                            found: else_branch.ty.clone(),
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
        self.check_expr(&body.tail)?;

        // If the body terminates via a `return` statement at the end, the
        // synthesized Unit tail is unreachable — skip the type check on it.
        let ends_with_return = matches!(
            body.items.last(),
            Some(BlockItem::Statement(stmt))
                if matches!(stmt.kind, StatementKind::Return(_))
        );
        if !ends_with_return && &body.tail.ty != return_ty {
            return Err(Error {
                span: body.tail.span,
                kind: ErrorKind::TypeMismatch {
                    expected: return_ty.clone(),
                    found: body.tail.ty.clone(),
                },
            });
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

    fn check_binary_op(&self, op: BinaryOperator, lhs: &Expr, rhs: &Expr) -> Result<(), Error> {
        use BinaryOperator as B;
        match op {
            // Arithmetic: both operands must be the same numeric type.
            B::Add | B::Sub | B::Mul | B::Div | B::Mod => {
                self.require_number(lhs)?;
                self.require_matching(lhs, rhs)?;
            }
            // Comparisons: same numeric type. Result is Int (0/1).
            B::Lt | B::Le | B::Gt | B::Ge | B::Eq | B::Ne => {
                self.require_number(lhs)?;
                self.require_matching(lhs, rhs)?;
            }
            // Shifts: both integer (signed or unsigned), same type.
            B::Shl | B::Shr => {
                self.require_integer(lhs)?;
                self.require_matching(lhs, rhs)?;
            }
            // Bitwise: both integer, same type.
            B::BitAnd | B::BitOr | B::BitXor => {
                self.require_integer(lhs)?;
                self.require_matching(lhs, rhs)?;
            }
            // Logical: any integer (truthy semantics; no Bool yet).
            B::And | B::Or => {
                self.require_integer(lhs)?;
                self.require_matching(lhs, rhs)?;
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn require_int(&self, e: &Expr) -> Result<(), Error> {
        if !e.ty.is_int() {
            return Err(Error {
                span: e.span,
                kind: ErrorKind::TypeMismatch {
                    expected: Ty::Int,
                    found: e.ty.clone(),
                },
            });
        }
        Ok(())
    }

    fn require_integer(&self, e: &Expr) -> Result<(), Error> {
        if !e.ty.is_integer() {
            return Err(Error {
                span: e.span,
                kind: ErrorKind::TypeMismatch {
                    expected: Ty::Int,
                    found: e.ty.clone(),
                },
            });
        }
        Ok(())
    }

    fn require_number(&self, e: &Expr) -> Result<(), Error> {
        if !e.ty.is_number() {
            return Err(Error {
                span: e.span,
                kind: ErrorKind::TypeMismatch {
                    expected: Ty::Int,
                    found: e.ty.clone(),
                },
            });
        }
        Ok(())
    }

    fn require_matching(&self, lhs: &Expr, rhs: &Expr) -> Result<(), Error> {
        if lhs.ty != rhs.ty {
            return Err(Error {
                span: rhs.span,
                kind: ErrorKind::TypeMismatch {
                    expected: lhs.ty.clone(),
                    found: rhs.ty.clone(),
                },
            });
        }
        Ok(())
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
