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
            '=' => {
                tokens.push(cursor.single(TokenKind::Equal));
            }
            '-' => {
                tokens.push(cursor.single(TokenKind::Minus));
            }
            '>' => {
                tokens.push(cursor.single(TokenKind::GreaterThan));
            }
            '0'..='9' => {
                tokens.push(cursor.int()?);
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                tokens.push(cursor.identifier_or_keyword());
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
