pub use crate::ast::{BinaryOperator, UnaryOperator};
use crate::lexer::types::Span;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(u32);

#[derive(Default)]
pub struct LocalIdGen {
    next: u32,
}

impl LocalIdGen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh(&mut self) -> LocalId {
        let id = LocalId(self.next);
        self.next += 1;
        id
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Function { params: Vec<Ty>, return_ty: Box<Ty> },
}

pub struct Program {
    pub span: Span,
    pub declarations: Vec<Declaration>,
}

pub struct Declaration {
    pub id: LocalId,
    pub span: Span,
    pub name: String,
    pub ty: Ty,
    pub value: Expr,
}

pub struct Expr {
    pub span: Span,
    pub ty: Ty,
    pub kind: ExprKind,
}

pub enum ExprKind {
    Const(Const),
    Local(LocalId),
    Function(Function),
    Call(FunctionCall),
    Unary {
        op: UnaryOperator,
        operand: Box<Expr>,
    },
    Block(Block),
    Binary {
        op: BinaryOperator,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

pub struct FunctionCall {
    pub callee: Box<Expr>,
    pub args: Vec<Expr>,
}

pub enum Const {
    Int(i32),
}

pub struct Function {
    pub params: Vec<Param>,
    pub return_ty: Ty,
    pub body: Block,
}

pub struct Param {
    pub id: LocalId,
    pub span: Span,
    pub name: String,
    pub ty: Ty,
}

pub struct Block {
    pub span: Span,
    pub items: Vec<BlockItem>,
    pub tail: Option<Box<Expr>>,
}

pub enum BlockItem {
    Declaration(Declaration),
    Statement(Statement),
}

pub struct Statement {
    pub span: Span,
    pub kind: StatementKind,
}

pub enum StatementKind {
    Return(Expr),
    Expr(Expr),
}

impl fmt::Debug for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::Function { params, return_ty } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p:?}")?;
                }
                write!(f, ") -> {return_ty:?}")
            }
        }
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Program")
            .field("declarations", &self.declarations)
            .finish()
    }
}

impl fmt::Debug for Declaration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Declaration")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("ty", &self.ty)
            .field("value", &self.value)
            .finish()
    }
}

impl fmt::Debug for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} : {:?}", self.kind, self.ty)
    }
}

impl fmt::Debug for ExprKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprKind::Const(c) => write!(f, "{c:?}"),
            ExprKind::Local(id) => write!(f, "Local({})", id.0),
            ExprKind::Function(func) => write!(f, "{func:?}"),
            ExprKind::Call(call) => write!(f, "call {:?}{:?}", call.callee, call.args),
            ExprKind::Unary { op, operand } => write!(f, "{op:?}({operand:?})"),
            ExprKind::Binary { op, lhs, rhs } => write!(f, "{op:?}({lhs:?}, {rhs:?})"),
        }
    }
}

impl fmt::Debug for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::Int(n) => write!(f, "Int({n})"),
        }
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Function")
            .field("params", &self.params)
            .field("return_ty", &self.return_ty)
            .field("body", &self.body)
            .finish()
    }
}

impl fmt::Debug for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Param")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish()
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("items", &self.items)
            .field("tail", &self.tail)
            .finish()
    }
}

impl fmt::Debug for BlockItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockItem::Declaration(d) => write!(f, "{d:?}"),
            BlockItem::Statement(s) => write!(f, "{s:?}"),
        }
    }
}

impl fmt::Debug for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Debug for StatementKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatementKind::Return(e) => write!(f, "Return({e:?})"),
            StatementKind::Expr(e) => write!(f, "Expr({e:?})"),
        }
    }
}
