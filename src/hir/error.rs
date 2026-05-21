use std::fmt;

use crate::{hir::types::Ty, lexer::types::Span};

#[derive(Debug, PartialEq)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, PartialEq)]
pub enum ErrorKind {
    NameNotFound { name: String },
    AlreadyDefined { name: String },
    NotCallable { found: Ty },
    NotIndexable { found: Ty },
    EmptyArrayLiteral,
    LiteralOutOfRange { value: i32, target: Ty },
    NotDereferencable { found: Ty },
    UnknownType { name: String },
    UnknownField { struct_name: String, field: String },
    MissingFields { struct_name: String, missing: Vec<String> },
    DuplicateField { struct_name: String, field: String },
    NotAStruct { found: Ty },
    DirectStructRecursion { name: String },
    StructDefNotAllowedHere,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.kind, self.span.start.line, self.span.start.column
        )
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::NameNotFound { name } => {
                write!(f, "name not found: {name}")
            }
            ErrorKind::AlreadyDefined { name } => {
                write!(f, "name `{name}` already defined")
            }
            ErrorKind::NotCallable { found } => {
                write!(f, "cannot call non-function value of type {found:?}")
            }
            ErrorKind::NotIndexable { found } => {
                write!(f, "cannot subscript non-array value of type {found:?}")
            }
            ErrorKind::EmptyArrayLiteral => {
                write!(
                    f,
                    "empty array literals are not supported (element type cannot be inferred)"
                )
            }
            ErrorKind::LiteralOutOfRange { value, target } => {
                write!(f, "literal {value} is out of range for type {target:?}")
            }
            ErrorKind::NotDereferencable { found } => {
                write!(f, "cannot dereference non-pointer value of type {found:?}")
            }
            ErrorKind::UnknownType { name } => {
                write!(f, "unknown type `{name}`")
            }
            ErrorKind::UnknownField { struct_name, field } => {
                write!(f, "struct `{struct_name}` has no field `{field}`")
            }
            ErrorKind::MissingFields { struct_name, missing } => {
                write!(
                    f,
                    "struct literal for `{struct_name}` is missing fields: {}",
                    missing.join(", ")
                )
            }
            ErrorKind::DuplicateField { struct_name, field } => {
                write!(
                    f,
                    "field `{field}` is specified more than once in `{struct_name}` literal"
                )
            }
            ErrorKind::NotAStruct { found } => {
                write!(f, "field access requires a struct, got {found:?}")
            }
            ErrorKind::DirectStructRecursion { name } => {
                write!(
                    f,
                    "struct `{name}` directly contains itself (infinite size); use `*{name}` for self-references"
                )
            }
            ErrorKind::StructDefNotAllowedHere => {
                write!(f, "struct definitions are only allowed as `let X = struct {{...}};`")
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
