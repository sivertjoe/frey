use crate::lexer::types::Span;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorKind {
    IdentifierStartsWithDigit(String),
    UnexpectedText(String),
    UnexpectedChar(char),
    InvalidInt(String),
    InvalidIntegerSuffix(String),
    InvalidCharConst,
    InvalidStringConst,
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
            ErrorKind::IdentifierStartsWithDigit(text) => {
                write!(f, "identifier cannot start with digit: `{}`", text)
            }
            ErrorKind::UnexpectedText(text) => {
                write!(f, "unexpected text: `{}`", text)
            }
            ErrorKind::UnexpectedChar(text) => {
                write!(f, "unexpected text: `{}`", text)
            }
            ErrorKind::InvalidIntegerSuffix(suffix) => {
                write!(f, "invalid integer suffix: `{}`", suffix)
            }
            ErrorKind::InvalidInt(int) => {
                write!(f, "invalid integer: `{}`", int)
            }
            ErrorKind::InvalidCharConst => {
                write!(f, "invalid character constant")
            }
            ErrorKind::InvalidStringConst => {
                write!(f, "invalid string constant")
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
