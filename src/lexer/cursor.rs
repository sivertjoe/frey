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

    pub fn identifier_or_keyword(&mut self) -> Token {
        let start = self.position();

        let raw = self.take_while(|ch| ch.is_ascii_alphanumeric() || ch == '_');

        let kind = match raw {
            "let" => TokenKind::Let,
            "Int" => TokenKind::Int,
            "UInt" => TokenKind::UInt,
            "Float" => TokenKind::Float,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "return" => TokenKind::Return,
            "as" => TokenKind::As,
            _ => TokenKind::Identifier(raw.to_string()),
        };

        let end = self.position();

        Token {
            kind,
            span: Span { start, end },
        }
    }

    pub fn number(&mut self) -> Result<Token, Error> {
        let start = self.position();
        let start_offset = self.offset;
        let mut is_float = false;

        self.take_while(|c| c.is_ascii_digit());

        let started_with_dot = self.offset == start_offset;
        if self.peek() == Some('.')
            && (started_with_dot || matches!(self.peek_second(), Some(c) if c.is_ascii_digit()))
        {
            is_float = true;
            self.bump();
            self.take_while(|c| c.is_ascii_digit());
        }

        // Optional exponent: `e[+-]?digits`.
        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            let exp_digits_start = self.offset;
            self.take_while(|c| c.is_ascii_digit());
            if self.offset == exp_digits_start {
                return Err(Error {
                    span: Span {
                        start,
                        end: self.position(),
                    },
                    kind: ErrorKind::UnexpectedText(
                        self.src[start_offset..self.offset].to_string(),
                    ),
                });
            }
        }

        if self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            self.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
            return Err(Error {
                kind: ErrorKind::UnexpectedText(self.src[start_offset..self.offset].to_string()),
                span: Span {
                    start,
                    end: self.position(),
                },
            });
        }

        let text = &self.src[start_offset..self.offset];
        let span = Span {
            start,
            end: self.position(),
        };

        if is_float {
            let value = text.parse::<f32>().map_err(|_| Error {
                kind: ErrorKind::UnexpectedText(text.to_string()),
                span,
            })?;
            Ok(Token {
                span,
                kind: TokenKind::Literal(Literal::Float(value)),
            })
        } else {
            let value = text.parse::<i32>().map_err(|_| Error {
                kind: ErrorKind::InvalidInt(text.to_string()),
                span,
            })?;
            Ok(Token {
                span,
                kind: TokenKind::Literal(Literal::Int(value)),
            })
        }
    }
}
