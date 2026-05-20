use std::fmt;

use crate::hir::types::Ty;
use crate::lexer::types::Span;

#[derive(Debug, PartialEq)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, PartialEq)]
pub enum ErrorKind {
    TypeMismatch {
        expected: Ty,
        found: Ty,
    },
    ArityMismatch {
        expected: usize,
        found: usize,
    },
    #[allow(dead_code)] // reserved for when CFG analysis lands
    MissingReturn {
        expected: Ty,
    },
    IllegalCast {
        ty: Ty,
    },
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
            ErrorKind::TypeMismatch { expected, found } => {
                write!(f, "type mismatch\nexpected: {expected:?}\nfound: {found:?}")
            }
            ErrorKind::IllegalCast { ty } => {
                write!(f, "type error\nillegal cast to : {ty:?}")
            }
            ErrorKind::ArityMismatch { expected, found } => {
                write!(
                    f,
                    "wrong number of arguments: expected {expected}, found {found}"
                )
            }
            ErrorKind::MissingReturn { expected } => {
                write!(
                    f,
                    "function body does not return a value; expected {expected:?}"
                )
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
