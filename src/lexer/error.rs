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
    UnexpectedText(String),
    InvalidInt(String),
    UnterminatedString,
    InvalidEscape(char),
    UnterminatedComment,
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
            ErrorKind::UnexpectedText(ch) => {
                write!(f, "unexpected text: `{}`", ch)
            }
            ErrorKind::InvalidInt(int) => {
                write!(f, "invalid integer: `{}`", int)
            }
            ErrorKind::UnterminatedString => {
                write!(f, "unterminated string literal")
            }
            ErrorKind::InvalidEscape(ch) => {
                write!(f, "invalid escape sequence: `\\{ch}`")
            }
            ErrorKind::UnterminatedComment => {
                write!(f, "unterminated block comment")
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
