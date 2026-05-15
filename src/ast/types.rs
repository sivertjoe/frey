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
    Return(Expr),
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
    Function {
        params: Vec<Param>,
        return_ty: TypeExpr,
        body: Block,
    },
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

// Debug impls omit `id` and `span` to keep printed ASTs readable.
// Wrapper types (TypeExpr, Statement, Expr) delegate directly to their `kind`.

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
