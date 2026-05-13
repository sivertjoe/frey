use crate::lexer::types::Span;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorKind {
    UnexpectedChar(char),
    InvalidInt(String),
    InvalidIntegerSuffix(String),
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
            ErrorKind::UnexpectedChar(ch) => {
                write!(f, "unexpected character: `{}`", ch)
            }
            ErrorKind::InvalidInt(int) => {
                write!(f, "invalid integer: `{}`", int)
            }
            ErrorKind::InvalidIntegerSuffix(suffix) => {
                write!(f, "invalid integer suffix: `{}`", suffix)
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
