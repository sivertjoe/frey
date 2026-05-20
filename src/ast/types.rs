// AST nodes carry `id` and `span` fields that aren't all consumed by every
// pass; they're kept for diagnostics, visitor passes, and future tooling.
#![allow(dead_code)]

use crate::lexer::types::Span;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

#[derive(Default)]
pub struct NodeIdGen {
    next: u32,
}

impl NodeIdGen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh(&mut self) -> NodeId {
        let id = NodeId(self.next);
        self.next += 1;
        id
    }
}

pub struct Program {
    pub span: Span,
    pub declarations: Vec<Declaration>,
}

pub struct Declaration {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    pub value: Expr,
}

pub struct TypeExpr {
    pub id: NodeId,
    pub span: Span,
    pub kind: TypeExprKind,
}

#[derive(Debug)]
pub enum TypeExprKind {
    Int,
    Function {
        params: Vec<TypeExpr>,
        return_ty: Box<TypeExpr>,
    },
}

pub struct Block {
    pub id: NodeId,
    pub span: Span,
    pub items: Vec<BlockItem>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug)]
pub enum BlockItem {
    Declaration(Declaration),
    Statement(Statement),
}

pub struct Statement {
    pub id: NodeId,
    pub span: Span,
    pub kind: StatementKind,
}

#[derive(Debug)]
pub enum StatementKind {
    Return(Option<Expr>),
    Expr(Expr),
}

pub struct Expr {
    pub id: NodeId,
    pub span: Span,
    pub kind: ExprKind,
}

#[derive(Debug)]
pub enum ExprKind {
    Const(Const),
    Identifier(String),
    Function {
        params: Vec<Param>,
        return_ty: Option<TypeExpr>,
        body: Block,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    Unary {
        op: UnaryOperator,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOperator,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Block(Block),
    If {
        condition: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitXor,
    BitOr,
    And,
    Or,
}

pub struct Param {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug)]
pub enum Const {
    Int(i32),
}

#[derive(Debug)]
pub enum UnaryOperator {
    Not,
    Minus,
}

impl fmt::Debug for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Debug for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Debug for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
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
            .field("name", &self.name)
            .field("value", &self.value)
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

impl fmt::Debug for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Param")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish()
    }
}
