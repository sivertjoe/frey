// HIR nodes carry `id` and `span` fields that aren't all consumed by every
// pass; they're kept for diagnostics and future tooling.
#![allow(dead_code)]

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
pub struct TypeVar {
    pub name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeVarId(pub u32);

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Unit,
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
    Function { params: Vec<Ty>, return_ty: Box<Ty> },
    Array { element: Box<Ty>, count: usize },
    Ptr(Box<Ty>),
    Struct(String),
    TypeVar(TypeVarId),
    GenericStruct { name: String, args: Vec<Ty> },
}

impl Ty {
    pub fn is_pointer(&self) -> bool {
        matches!(self, Ty::Ptr(_))
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Ty::Int | Ty::I8 | Ty::I32 | Ty::I64)
    }

    pub fn is_uint(&self) -> bool {
        matches!(self, Ty::UInt | Ty::U8 | Ty::U32 | Ty::U64)
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::Float | Ty::F32 | Ty::F64)
    }

    pub fn is_integer(&self) -> bool {
        self.is_int() || self.is_uint()
    }

    pub fn is_number(&self) -> bool {
        self.is_integer() || self.is_float()
    }

    pub fn is_generic(&self) -> bool {
        matches!(self, Ty::TypeVar(_))
    }

    pub fn bit_width(&self) -> Option<u32> {
        match self {
            Ty::I8 | Ty::U8 => Some(8),
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 | Ty::Float | Ty::F32 => Some(32),
            Ty::I64 | Ty::U64 | Ty::F64 => Some(64),
            _ => None,
        }
    }
}

pub struct Program {
    pub span: Span,
    pub declarations: Vec<Declaration>,
    pub structs: std::collections::HashMap<String, StructDef>,
}

#[derive(Clone)]
pub struct StructDef {
    pub name: String,
    pub type_var_ids: Vec<TypeVarId>,
    pub fields: Vec<(String, Ty)>,
}

impl fmt::Debug for StructDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructDef")
            .field("name", &self.name)
            .field("fields", &self.fields)
            .finish()
    }
}

#[derive(Clone)]
pub struct Declaration {
    pub id: LocalId,
    pub span: Span,
    pub mutable: bool,
    pub name: String,
    pub ty: Ty,
    pub value: Expr,
}

#[derive(Clone)]
pub struct Expr {
    pub span: Span,
    pub ty: Ty,
    pub kind: ExprKind,
}

#[derive(Clone)]
pub enum ExprKind {
    Const(Const),
    Local(LocalId),
    Function(Function),
    Call(FunctionCall),
    Cast {
        target: Ty,
        expr: Box<Expr>,
    },
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
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    While {
        condition: Box<Expr>,
        body: Block,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Array(Vec<Expr>),
    Subscript {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    Ref(Box<Expr>),
    Deref(Box<Expr>),
    StructLiteral {
        fields: Vec<(String, Expr)>,
    },
    Field {
        target: Box<Expr>,
        name: String,
        index: usize,
    },
    /// A reified type value, used only inside `#comptime` function bodies.
    /// Folded away during specialization; never reaches codegen.
    TypeValue(Ty),
    /// A `comperror(msg)` call. If reached during comptime evaluation it
    /// aborts compilation; otherwise it is discarded by static-if folding.
    CompError(String),
    /// A heap-allocation intrinsic (`alloc<T>`, `realloc<T>`, `free`). `elem_ty`
    /// is the element type `T` (used for `sizeof`); it is resolved to a
    /// concrete type during specialization before codegen.
    Intrinsic {
        kind: IntrinsicKind,
        elem_ty: Ty,
        args: Vec<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntrinsicKind {
    Alloc,
    Realloc,
    Free,
}

#[derive(Clone)]
pub struct FunctionCall {
    pub callee: Box<Expr>,
    pub args: Vec<Expr>,
}

#[derive(Clone)]
pub enum Const {
    Int(i32),
    Float(f32),
    Str(String),
    Unit,
}

#[derive(Clone)]
pub struct Function {
    pub params: Vec<Param>,
    pub return_ty: Ty,
    pub body: Block,
}

#[derive(Clone)]
pub struct Param {
    pub id: LocalId,
    pub span: Span,
    pub name: String,
    pub ty: Ty,
}

#[derive(Clone)]
pub struct Block {
    pub span: Span,
    pub items: Vec<BlockItem>,
    pub tail: Box<Expr>,
}

#[derive(Clone)]
pub enum BlockItem {
    Declaration(Declaration),
    Statement(Statement),
}

#[derive(Clone)]
pub struct Statement {
    pub span: Span,
    pub kind: StatementKind,
}

#[derive(Clone)]
pub enum StatementKind {
    Return(Expr),
    Expr(Expr),
    Break,
    /// `defer <expr>;` — runs `expr` (LIFO) when the enclosing block exits,
    /// including via early `return`/`break`.
    Defer(Expr),
}

impl fmt::Debug for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unit => write!(f, "Unit"),
            Ty::Int => write!(f, "Int"),
            Ty::UInt => write!(f, "UInt"),
            Ty::Float => write!(f, "Float"),
            Ty::I8 => write!(f, "i8"),
            Ty::I32 => write!(f, "i32"),
            Ty::I64 => write!(f, "i64"),
            Ty::U8 => write!(f, "u8"),
            Ty::U32 => write!(f, "u32"),
            Ty::U64 => write!(f, "u64"),
            Ty::F32 => write!(f, "f32"),
            Ty::F64 => write!(f, "f64"),
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
            Ty::Array { element, count } => write!(f, "[{element:?}; {count}]"),
            Ty::Ptr(target) => write!(f, "*{target:?}"),
            Ty::Struct(name) => write!(f, "{name}"),
            Ty::TypeVar(id) => write!(f, "type var id {}", id.0),
            Ty::GenericStruct { name, args } => {
                write!(f, "{name}<")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a:?}")?;
                }
                write!(f, ">")
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
            .field("mutable", &self.mutable)
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
            ExprKind::Block(block) => write!(f, "{block:?}"),
            ExprKind::Cast { target, expr } => write!(f, "Cast({target:?}) {expr:?}"),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => write!(f, "If({condition:?}, {then_branch:?}, {else_branch:?})"),
            ExprKind::While { condition, body } => write!(f, "While({condition:?}, {body:?})"),
            ExprKind::Assign { target, value } => {
                write!(f, "Assign({target:?}, {value:?})")
            }
            ExprKind::Array(items) => write!(f, "Array{items:?}"),
            ExprKind::Subscript { expr, index } => write!(f, "Subscript({expr:?}, {index:?})"),
            ExprKind::Ref(target) => write!(f, "Ref({target:?})"),
            ExprKind::Deref(target) => write!(f, "Deref({target:?})"),
            ExprKind::StructLiteral { fields } => {
                write!(f, "StructLit{{")?;
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {value:?}")?;
                }
                write!(f, "}}")
            }
            ExprKind::Field { target, name, .. } => write!(f, "{target:?}.{name}"),
            ExprKind::TypeValue(ty) => write!(f, "TypeValue({ty:?})"),
            ExprKind::CompError(msg) => write!(f, "CompError({msg:?})"),
            ExprKind::Intrinsic {
                kind,
                elem_ty,
                args,
            } => write!(f, "{kind:?}<{elem_ty:?}>{args:?}"),
        }
    }
}

impl fmt::Debug for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::Int(n) => write!(f, "Int({n})"),
            Const::Float(n) => write!(f, "Float({n})"),
            Const::Str(s) => write!(f, "Str({s:?})"),
            Const::Unit => write!(f, "Unit"),
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
            StatementKind::Break => write!(f, "Break"),
            StatementKind::Defer(e) => write!(f, "Defer({e:?})"),
        }
    }
}
