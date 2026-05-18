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
    TypeMismatch { expected: Ty, found: Ty },
    MissingReturn { expected: Ty },
    AlreadyDefined { name: String },
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
            ErrorKind::MissingReturn { expected } => {
                write!(f, "missing return, expected {:?}", expected)
            }
            ErrorKind::NameNotFound { name } => {
                write!(f, "name not found: {name}")
            }
            ErrorKind::TypeMismatch { expected, found } => {
                write!(
                    f,
                    "type mismatch\nexpected: {:?}\nfound:{:?}",
                    expected, found
                )
            }
            ErrorKind::AlreadyDefined { name } => {
                write!(f, "name `{name}` already define")
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
