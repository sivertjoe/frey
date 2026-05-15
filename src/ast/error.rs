use crate::lexer::types::{Span, Token, TokenKind};
use std::fmt;

#[derive(Debug, PartialEq)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, PartialEq)]
pub enum ErrorKind {
    Unexpected { expected: String, found: TokenKind },
    UnknownType(String),
}

impl Error {
    pub fn unexpected<S: AsRef<str>>(token: &Token, expected: S) -> Self {
        Self {
            span: token.span,
            kind: ErrorKind::Unexpected {
                expected: expected.as_ref().to_string(),
                found: token.kind.clone(),
            },
        }
    }
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
            ErrorKind::Unexpected { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            ErrorKind::UnknownType(name) => {
                write!(f, "unknown type: `{name}`")
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
