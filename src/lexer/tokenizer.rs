use super::{
    cursor::Cursor,
    error::{Error, ErrorKind},
    types::{Span, Token, TokenKind},
};

pub fn tokenize(src: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let mut cursor = Cursor::new(src);

    while let Some(ch) = cursor.peek() {
        match ch {
            ' ' | '\t' | '\r' => {
                cursor.bump();
            }
            '\n' => {
                cursor.bump();
            }
            ';' => {
                tokens.push(cursor.single(TokenKind::Semicolon));
            }
            ',' => {
                tokens.push(cursor.single(TokenKind::Comma));
            }
            ':' => {
                tokens.push(cursor.single(TokenKind::Colon));
            }
            '{' => {
                tokens.push(cursor.single(TokenKind::LeftBrace));
            }
            '}' => {
                tokens.push(cursor.single(TokenKind::RightBrace));
            }
            '(' => {
                tokens.push(cursor.single(TokenKind::LeftParen));
            }
            ')' => {
                tokens.push(cursor.single(TokenKind::RightParen));
            }
            '=' => match cursor.peek_second() {
                Some('=') => tokens.push(cursor.double(TokenKind::EqualEqual)),
                _ => tokens.push(cursor.single(TokenKind::Equal)),
            },
            '-' => {
                tokens.push(cursor.single(TokenKind::Minus));
            }
            '+' => {
                tokens.push(cursor.single(TokenKind::Plus));
            }
            '*' => {
                tokens.push(cursor.single(TokenKind::Star));
            }
            '/' => {
                tokens.push(cursor.single(TokenKind::Slash));
            }
            '%' => {
                tokens.push(cursor.single(TokenKind::Percent));
            }
            '^' => {
                tokens.push(cursor.single(TokenKind::Caret));
            }
            '!' => match cursor.peek_second() {
                Some('=') => tokens.push(cursor.double(TokenKind::NotEqual)),
                _ => tokens.push(cursor.single(TokenKind::Not)),
            },
            '<' => match cursor.peek_second() {
                Some('=') => tokens.push(cursor.double(TokenKind::LessEqual)),
                Some('<') => tokens.push(cursor.double(TokenKind::ShiftLeft)),
                _ => tokens.push(cursor.single(TokenKind::LessThan)),
            },
            '>' => match cursor.peek_second() {
                Some('=') => tokens.push(cursor.double(TokenKind::GreaterEqual)),
                Some('>') => tokens.push(cursor.double(TokenKind::ShiftRight)),
                _ => tokens.push(cursor.single(TokenKind::GreaterThan)),
            },
            '&' => match cursor.peek_second() {
                Some('&') => tokens.push(cursor.double(TokenKind::AmpAmp)),
                _ => tokens.push(cursor.single(TokenKind::Ampersand)),
            },
            '|' => match cursor.peek_second() {
                Some('|') => tokens.push(cursor.double(TokenKind::PipePipe)),
                _ => tokens.push(cursor.single(TokenKind::Pipe)),
            },
            'a'..='z' | 'A'..='Z' | '_' => {
                tokens.push(cursor.identifier_or_keyword());
            }
            ch if ch.is_ascii_digit()
                || (ch == '.'
                    && matches!(cursor.peek_second(), Some(ch) if ch.is_ascii_digit())) =>
            {
                let tok = cursor.number()?;
                tokens.push(tok);
            }
            _ => {
                let start = cursor.position();
                cursor.bump();
                let end = cursor.position();

                return Err(Error {
                    kind: ErrorKind::UnexpectedChar(ch),
                    span: Span { start, end },
                });
            }
        }
    }

    let position = cursor.position();

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            start: position,
            end: position,
        },
    });

    Ok(tokens)
}
