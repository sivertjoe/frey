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
        }
    }
}

impl std::error::Error for ErrorKind {}
