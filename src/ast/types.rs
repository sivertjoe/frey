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

    /// Starts numbering at `next` instead of 0, so ids stay unique across the
    /// several files merged by the module loader.
    pub fn with_next(next: u32) -> Self {
        Self { next }
    }

    /// The next id that would be handed out — used to chain numbering between
    /// files.
    pub fn next_value(&self) -> u32 {
        self.next
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
    pub imports: Vec<ImportDecl>,
}

/// `import "path";` — pulls another file's top-level declarations into the
/// program (resolved by the module loader before lowering).
pub struct ImportDecl {
    pub span: Span,
    pub path: String,
}

pub struct Declaration {
    pub id: NodeId,
    pub span: Span,
    pub mutable: bool,
    pub comptime: bool,
    pub name: String,
    /// Optional `: T` annotation. Int literals coerce to a numeric `T`.
    pub ty: Option<TypeExpr>,
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
    UInt,
    Float,
    I8,
    I32,
    I64,
    U8,
    U32,
    U64,
    F32,
    F64,
    Array {
        element_ty: Box<TypeExpr>,
        count: usize,
    },
    Function {
        params: Vec<TypeExpr>,
        return_ty: Box<TypeExpr>,
    },
    Ptr(Box<TypeExpr>),
    Named(String),
    NamedGeneric {
        name: String,
        args: Vec<TypeExpr>,
    },
    /// An anonymous tuple type `(T1, T2, ...)` with at least two elements.
    Tuple(Vec<TypeExpr>),
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
    Break,
    /// `defer <expr>;` — runs `expr` when the enclosing block exits.
    Defer(Expr),
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
    /// A type used in expression position (only meaningful inside a
    /// `#comptime` function), e.g. the `Int` in `T == Int`.
    TypeValue(TypeExpr),
    Function {
        type_params: Vec<String>,
        params: Vec<Param>,
        return_ty: Option<TypeExpr>,
        body: Block,
    },
    /// `{x : expr}` / `{x, y : expr}` — anonymous function literal. Param
    /// and return types come from the expected-type hint at the call site;
    /// captures aren't supported (the body lowers in a globals-only scope).
    Closure {
        params: Vec<String>,
        body: Box<Expr>,
    },
    Array(Vec<Expr>),
    Cast {
        expr: Box<Expr>,
        target: TypeExpr,
    },
    Call {
        callee: Box<Expr>,
        type_args: Vec<TypeExpr>,
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
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Block(Block),
    If {
        condition: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
    },
    While {
        condition: Box<Expr>,
        body: Block,
    },
    Subscript {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    Ref(Box<Expr>),
    Deref(Box<Expr>),
    StructDef {
        type_params: Vec<String>,
        fields: Vec<StructTypeField>,
    },
    /// `enum<$T> { Variant, Variant(T1, T2), ... }`.
    EnumDef {
        type_params: Vec<String>,
        variants: Vec<EnumVariantDef>,
    },
    /// `match scrutinee { Pat -> arm_expr, ... }`.
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    StructLiteral {
        name: String,
        type_args: Vec<TypeExpr>,
        fields: Vec<StructLiteralField>,
    },
    Field {
        target: Box<Expr>,
        name: String,
    },
    /// A tuple value `(a, b, ...)` with at least two elements.
    Tuple(Vec<Expr>),
    /// Tuple field access by index: `t.0`, `t.1`, …
    TupleField {
        target: Box<Expr>,
        index: usize,
    },
}

#[derive(Debug)]
pub struct StructTypeField {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug)]
pub struct EnumVariantDef {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    /// Positional payload types; empty for nullary variants like `None`.
    pub fields: Vec<TypeExpr>,
}

#[derive(Debug)]
pub struct MatchArm {
    pub id: NodeId,
    pub span: Span,
    pub pattern: Pattern,
    pub body: Expr,
}

pub struct Pattern {
    pub id: NodeId,
    pub span: Span,
    pub kind: PatternKind,
}

#[derive(Debug)]
pub enum PatternKind {
    /// `_` — matches anything, binds nothing.
    Wildcard,
    /// `Some(x, y)` — bindings is empty for nullary variants in the parser;
    /// bare identifiers parse as `Binding` and disambiguate at lowering.
    Variant {
        name: String,
        bindings: Vec<String>,
    },
    /// A bare identifier in pattern position; the lowerer decides if it's a
    /// nullary variant or a fresh binding.
    Binding(String),
}

impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

#[derive(Debug)]
pub struct StructLiteralField {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    pub value: Expr,
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
    Float(f32),
    Str(String),
}

#[derive(Debug, Clone, Copy)]
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
            .field("mutable", &self.mutable)
            .field("comptime", &self.comptime)
            .field("name", &self.name)
            .field("ty", &self.ty)
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
