use crate::lexer::error::ErrorKind;

use super::{error::Error, types::*};

pub struct Cursor<'a> {
    src: &'a str,
    offset: usize,
    line: usize,
    column: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn position(&self) -> Position {
        Position {
            offset: self.offset,
            line: self.line,
            column: self.column,
        }
    }

    pub fn peek(&self) -> Option<char> {
        self.src[self.offset..].chars().next()
    }

    pub fn peek_second(&self) -> Option<char> {
        let mut chars = self.src[self.offset..].chars();
        chars.next();
        chars.next()
    }

    pub fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;

        self.offset += ch.len_utf8();

        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }

        Some(ch)
    }

    pub fn single(&mut self, kind: TokenKind) -> Token {
        let start = self.position();
        self.bump();
        let end = self.position();

        Token {
            kind,
            span: Span { start, end },
        }
    }

    pub fn double(&mut self, kind: TokenKind) -> Token {
        let start = self.position();
        self.bump();
        self.bump();
        let end = self.position();

        Token {
            kind,
            span: Span { start, end },
        }
    }

    pub fn take_while<F>(&mut self, mut predicate: F) -> &'a str
    where
        F: FnMut(char) -> bool,
    {
        let start = self.offset;

        while let Some(ch) = self.peek() {
            if !predicate(ch) {
                break;
            }

            self.bump();
        }

        &self.src[start..self.offset]
    }

    pub fn int(&mut self) -> Result<Token, Error> {
        let start = self.position();

        let digits = self.take_while(|ch| ch.is_ascii_digit()).to_string();

        if self
            .peek()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        {
            let suffix = self
                .take_while(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                .to_string();
            return Err(Error {
                kind: ErrorKind::InvalidIntegerSuffix(suffix),
                span: Span {
                    start,
                    end: self.position(),
                },
            });
        }

        // @TODO: handle long later
        let Ok(value) = digits.parse::<i32>() else {
            return Err(Error {
                kind: ErrorKind::InvalidInt(digits),
                span: Span {
                    start,
                    end: self.position(),
                },
            });
        };

        let end = self.position();

        Ok(Token {
            kind: TokenKind::Literal(Literal::Int(value)),
            span: Span { start, end },
        })
    }

    pub fn identifier_or_keyword(&mut self) -> Token {
        let start = self.position();

        let raw = self.take_while(|ch| ch.is_ascii_alphanumeric() || ch == '_');

        let kind = match raw {
            "let" => TokenKind::Let,
            "Int" => TokenKind::Int,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "return" => TokenKind::Return,
            _ => TokenKind::Identifier(raw.to_string()),
        };

        let end = self.position();

        Token {
            kind,
            span: Span { start, end },
        }
    }
}
