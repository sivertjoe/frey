use crate::lexer::error::ErrorKind;

use super::{error::Error, types::*};

pub struct Cursor<'a> {
    src: &'a str,
    /// Byte offset added to local positions so that `Position::offset` is unique
    /// across files (used by the multi-file source map). `offset` below stays a
    /// local index into `src`.
    base: usize,
    offset: usize,
    line: usize,
    column: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(src: &'a str, base: usize) -> Self {
        Self {
            src,
            base,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn position(&self) -> Position {
        Position {
            offset: self.base + self.offset,
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

    pub fn peek_nth(&self, n: usize) -> Option<char> {
        self.src[self.offset..].chars().nth(n)
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
            "extern" => TokenKind::Extern,
            "null" => TokenKind::Null,
            "Int" => TokenKind::Int,
            "UInt" => TokenKind::UInt,
            "Float" => TokenKind::Float,
            "i8" => TokenKind::I8,
            "i32" => TokenKind::I32,
            "i64" => TokenKind::I64,
            "u8" => TokenKind::U8,
            "u32" => TokenKind::U32,
            "u64" => TokenKind::U64,
            "f32" => TokenKind::F32,
            "f64" => TokenKind::F64,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "return" => TokenKind::Return,
            "as" => TokenKind::As,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "match" => TokenKind::Match,
            "while" => TokenKind::While,
            "break" => TokenKind::Break,
            "defer" => TokenKind::Defer,
            "import" => TokenKind::Import,
            _ => TokenKind::Identifier(raw.to_string()),
        };

        let end = self.position();

        Token {
            kind,
            span: Span { start, end },
        }
    }

    /// Skips a `// ...` line comment (up to, but not including, the newline).
    pub fn line_comment(&mut self) {
        self.bump(); // first `/`
        self.bump(); // second `/`
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Skips a `/* ... */` block comment. Nesting is supported, so
    /// `/* a /* b */ c */` is a single comment. Errors if unterminated.
    pub fn block_comment(&mut self) -> Result<(), Error> {
        let start = self.position();
        self.bump(); // `/`
        self.bump(); // `*`
        let mut depth = 1usize;
        while depth > 0 {
            match (self.peek(), self.peek_second()) {
                (Some('/'), Some('*')) => {
                    self.bump();
                    self.bump();
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    self.bump();
                    self.bump();
                    depth -= 1;
                }
                (Some(_), _) => {
                    self.bump();
                }
                (None, _) => {
                    return Err(Error {
                        kind: ErrorKind::UnterminatedComment,
                        span: Span {
                            start,
                            end: self.position(),
                        },
                    });
                }
            }
        }
        Ok(())
    }

    pub fn number(&mut self, integer_only: bool) -> Result<Token, Error> {
        let start = self.position();
        let start_offset = self.offset;
        let mut is_float = false;

        self.take_while(|c| c.is_ascii_digit());

        let started_with_dot = self.offset == start_offset;
        if !integer_only
            && self.peek() == Some('.')
            && (started_with_dot || matches!(self.peek_second(), Some(c) if c.is_ascii_digit()))
        {
            is_float = true;
            self.bump();
            self.take_while(|c| c.is_ascii_digit());
        }

        // Optional exponent: `e[+-]?digits`.
        if !integer_only && matches!(self.peek(), Some('e' | 'E')) {
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

    /// `'A'`, `'\n'`, `'\0'`, etc. — a single byte. Bytes outside ASCII
    /// (e.g. `'é'` which is two UTF-8 bytes) are rejected.
    pub fn char_literal(&mut self) -> Result<Token, Error> {
        let start = self.position();
        self.bump(); // opening '

        let byte: u8 = match self.peek() {
            None | Some('\n') => {
                return Err(Error {
                    kind: ErrorKind::UnterminatedChar,
                    span: Span {
                        start,
                        end: self.position(),
                    },
                });
            }
            Some('\'') => {
                self.bump();
                return Err(Error {
                    kind: ErrorKind::EmptyChar,
                    span: Span {
                        start,
                        end: self.position(),
                    },
                });
            }
            Some('\\') => {
                self.bump();
                let esc_start = self.position();
                let b = match self.peek() {
                    Some('n') => b'\n',
                    Some('t') => b'\t',
                    Some('r') => b'\r',
                    Some('0') => 0u8,
                    Some('\\') => b'\\',
                    Some('\'') => b'\'',
                    Some('"') => b'"',
                    Some(other) => {
                        self.bump();
                        return Err(Error {
                            kind: ErrorKind::InvalidEscape(other),
                            span: Span {
                                start: esc_start,
                                end: self.position(),
                            },
                        });
                    }
                    None => {
                        return Err(Error {
                            kind: ErrorKind::UnterminatedChar,
                            span: Span {
                                start,
                                end: self.position(),
                            },
                        });
                    }
                };
                self.bump();
                b
            }
            Some(ch) => {
                let mut buf = [0u8; 4];
                let encoded = ch.encode_utf8(&mut buf);
                if encoded.len() != 1 {
                    let bad_start = self.position();
                    self.bump();
                    return Err(Error {
                        kind: ErrorKind::NonAsciiChar(ch),
                        span: Span {
                            start: bad_start,
                            end: self.position(),
                        },
                    });
                }
                self.bump();
                buf[0]
            }
        };

        match self.peek() {
            Some('\'') => self.bump(),
            _ => {
                return Err(Error {
                    kind: ErrorKind::UnterminatedChar,
                    span: Span {
                        start,
                        end: self.position(),
                    },
                });
            }
        };

        Ok(Token {
            kind: TokenKind::Literal(Literal::Char(byte)),
            span: Span {
                start,
                end: self.position(),
            },
        })
    }

    pub fn string(&mut self) -> Result<Token, Error> {
        let start = self.position();
        self.bump(); // opening "

        let mut s = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    return Err(Error {
                        kind: ErrorKind::UnterminatedString,
                        span: Span {
                            start,
                            end: self.position(),
                        },
                    });
                }
                Some('"') => {
                    self.bump();
                    break;
                }
                Some('\\') => {
                    self.bump();
                    let esc_start = self.position();
                    let ch = match self.peek() {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some('0') => '\0',
                        Some('\\') => '\\',
                        Some('"') => '"',
                        Some(other) => {
                            self.bump();
                            return Err(Error {
                                kind: ErrorKind::InvalidEscape(other),
                                span: Span {
                                    start: esc_start,
                                    end: self.position(),
                                },
                            });
                        }
                        None => {
                            return Err(Error {
                                kind: ErrorKind::UnterminatedString,
                                span: Span {
                                    start,
                                    end: self.position(),
                                },
                            });
                        }
                    };
                    self.bump();
                    s.push(ch);
                }
                Some(ch) => {
                    self.bump();
                    s.push(ch);
                }
            }
        }

        Ok(Token {
            kind: TokenKind::Literal(Literal::Str(s)),
            span: Span {
                start,
                end: self.position(),
            },
        })
    }
}
