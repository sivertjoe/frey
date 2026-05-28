use std::collections::HashMap;

use crate::hir::types::{
    BinaryOperator, Block, BlockItem, Declaration, Expr, ExprKind, Function, FunctionCall, LocalId,
    Program, Statement, StatementKind, Ty, UnaryOperator,
};
use crate::semantics::error::{Error, ErrorKind};

#[derive(Clone)]
struct BindingInfo {
    name: String,
    mutable: bool,
}

pub fn type_check(program: &Program) -> Result<(), Error> {
    Typechecker::new().check_program(program)
}

struct Typechecker {
    expected_return: Vec<Ty>,
    bindings: HashMap<LocalId, BindingInfo>,
    loop_depth: u32,
}

impl Typechecker {
    fn new() -> Self {
        Self {
            expected_return: Vec::new(),
            bindings: HashMap::new(),
            loop_depth: 0,
        }
    }

    fn check_program(&mut self, p: &Program) -> Result<(), Error> {
        // Pre-register top-level bindings so nested function bodies that
        // assign to outer mutable bindings can resolve them.
        for decl in &p.declarations {
            self.bindings.insert(
                decl.id,
                BindingInfo {
                    name: decl.name.clone(),
                    mutable: decl.mutable,
                },
            );
        }
        for decl in &p.declarations {
            self.check_declaration(decl)?;
        }
        Ok(())
    }

    fn check_declaration(&mut self, d: &Declaration) -> Result<(), Error> {
        // Top-level decls were registered in check_program; for nested decls
        // (block items), this insert is what makes them visible to later
        // assignments in the same block.
        self.bindings.insert(
            d.id,
            BindingInfo {
                name: d.name.clone(),
                mutable: d.mutable,
            },
        );
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
            ExprKind::Assign { target, value } => {
                self.check_expr(target)?;
                self.check_expr(value)?;
                // For chains rooted in a local (a, a[i], a[i][j]), require the
                // local to be mut. For chains rooted in a deref (*p, (*p)[i]),
                // the pointer itself is the gate — writing through a pointer
                // doesn't require its binding to be mut.
                if let Some(root_id) = assignment_local_root(target) {
                    let binding = self
                        .bindings
                        .get(&root_id)
                        .expect("resolved LocalId must be in the bindings table")
                        .clone();
                    if !binding.mutable {
                        return Err(Error {
                            span: e.span,
                            kind: ErrorKind::AssignToImmutable { name: binding.name },
                        });
                    }
                }
                if value.ty != target.ty {
                    return Err(Error {
                        span: value.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: target.ty.clone(),
                            found: value.ty.clone(),
                        },
                    });
                }
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
            ExprKind::Array(items) => {
                // Lowering already picked the first element's type as the
                // array's element type; here we just verify the rest match.
                let Ty::Array { element, .. } = &e.ty else {
                    unreachable!("array literal has Array type from lowering");
                };
                for item in items {
                    self.check_expr(item)?;
                    if &item.ty != element.as_ref() {
                        return Err(Error {
                            span: item.span,
                            kind: ErrorKind::TypeMismatch {
                                expected: (**element).clone(),
                                found: item.ty.clone(),
                            },
                        });
                    }
                }
                Ok(())
            }
            ExprKind::Subscript { expr, index } => {
                self.check_expr(expr)?;
                self.check_expr(index)?;
                // Lowering only accepts arrays and pointers as index targets.
                if !matches!(expr.ty, Ty::Array { .. } | Ty::Ptr(_)) {
                    unreachable!("subscript target must be an array or pointer from lowering");
                }
                if !index.ty.is_integer() {
                    return Err(Error {
                        span: index.span,
                        kind: ErrorKind::TypeMismatch {
                            expected: Ty::Int,
                            found: index.ty.clone(),
                        },
                    });
                }
                Ok(())
            }
            ExprKind::Ref(target) => {
                self.check_expr(target)?;
                if !is_addressable(target) {
                    return Err(Error {
                        span: target.span,
                        kind: ErrorKind::NotAddressable,
                    });
                }
                Ok(())
            }
            ExprKind::Deref(target) => {
                self.check_expr(target)?;
                // Lowering already rejected non-pointer operands; assert
                // defensively in case the HIR is fed from elsewhere.
                if !matches!(target.ty, Ty::Ptr(_)) {
                    unreachable!("deref operand has Ptr type from lowering");
                }
                Ok(())
            }
            ExprKind::StructLiteral { fields } => {
                for (_, v) in fields {
                    self.check_expr(v)?;
                }
                Ok(())
            }
            ExprKind::Field { target, .. } => {
                self.check_expr(target)?;
                Ok(())
            }
            ExprKind::While { condition, body } => {
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
                self.loop_depth += 1;
                let result = self.check_body_block(body, &Ty::Unit);
                self.loop_depth -= 1;
                result
            }
            // Comptime-only nodes are folded away during specialization and
            // never reach a function that is actually emitted.
            ExprKind::TypeValue(_) | ExprKind::CompError(_) => Ok(()),
            ExprKind::Intrinsic { args, .. } => {
                for arg in args {
                    self.check_expr(arg)?;
                }
                Ok(())
            }
            ExprKind::Tuple(elems) => {
                for el in elems {
                    self.check_expr(el)?;
                }
                Ok(())
            }
            ExprKind::TupleField { target, .. } => {
                self.check_expr(target)?;
                Ok(())
            }
        }
    }

    fn check_body_block(&mut self, body: &Block, expected_tail_ty: &Ty) -> Result<(), Error> {
        for item in &body.items {
            match item {
                BlockItem::Declaration(d) => self.check_declaration(d)?,
                BlockItem::Statement(s) => self.check_statement(s)?,
            }
        }
        self.check_expr(&body.tail)?;
        if &body.tail.ty != expected_tail_ty {
            return Err(Error {
                span: body.tail.span,
                kind: ErrorKind::TypeMismatch {
                    expected: expected_tail_ty.clone(),
                    found: body.tail.ty.clone(),
                },
            });
        }
        Ok(())
    }

    fn check_function(&mut self, f: &Function) -> Result<(), Error> {
        self.expected_return.push(f.return_ty.clone());
        // Register params in the bindings table. Params are always immutable
        // and passed by value — there's no syntactic way to mark them `mut`.
        for p in &f.params {
            self.bindings.insert(
                p.id,
                BindingInfo {
                    name: p.name.clone(),
                    mutable: false,
                },
            );
        }
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

        // If control flow always diverts before reaching the synthesized Unit
        // tail — a trailing `return`, or (after comptime folding) a block / if
        // that returns — the tail is unreachable, so skip its type check.
        let diverges = body.items.iter().any(crate::hir::comptime::item_diverges)
            || crate::hir::comptime::expr_diverges(&body.tail);
        if !diverges && &body.tail.ty != return_ty {
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
            StatementKind::Break => {
                if self.loop_depth == 0 {
                    return Err(Error {
                        span: s.span,
                        kind: ErrorKind::BreakOutsideLoop,
                    });
                }
                Ok(())
            }
            StatementKind::Defer(e) => self.check_expr(e),
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

/// Whether `&e` is valid: e is a place we can take the address of.
fn is_addressable(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Local(_)
            | ExprKind::Subscript { .. }
            | ExprKind::Deref(_)
            | ExprKind::Field { .. }
    )
}

/// Walks through a place expression chain to find the local whose mutability
/// gates the assignment. Returns None when the chain is rooted in a deref —
/// `*p = v` doesn't need `p` to be mut, only the pointer's pointee to be
/// writable, which the type system doesn't currently distinguish.
fn assignment_local_root(target: &Expr) -> Option<LocalId> {
    match &target.kind {
        ExprKind::Local(id) => Some(*id),
        ExprKind::Subscript { expr, .. } => {
            // Indexing through a pointer writes to the pointee, so — like a
            // deref — it doesn't require the pointer binding itself to be mut.
            if matches!(expr.ty, Ty::Ptr(_)) {
                None
            } else {
                assignment_local_root(expr)
            }
        }
        ExprKind::Field { target, .. } => assignment_local_root(target),
        ExprKind::TupleField { target, .. } => assignment_local_root(target),
        ExprKind::Deref(_) => None,
        _ => unreachable!("assignment target must be a place expression"),
    }
}
