#[cfg(test)]
mod tests {
    use super::super::tokenize;
    use super::super::types::{Literal, Position, Span, Token, TokenKind};

    fn pos(offset: usize, line: usize, column: usize) -> Position {
        Position {
            offset,
            line,
            column,
        }
    }

    fn span(
        start_offset: usize,
        start_line: usize,
        start_column: usize,
        end_offset: usize,
        end_line: usize,
        end_column: usize,
    ) -> Span {
        Span {
            start: pos(start_offset, start_line, start_column),
            end: pos(end_offset, end_line, end_column),
        }
    }

    fn eof(offset: usize, line: usize, column: usize) -> Token {
        Token {
            kind: TokenKind::Eof,
            span: Span {
                start: pos(offset, line, column),
                end: pos(offset, line, column),
            },
        }
    }

    #[test]
    fn tokenizes_single_character_tokens() {
        let src = "{}()=-<";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::LeftBrace,
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::RightBrace,
                    span: span(1, 1, 2, 2, 1, 3),
                },
                Token {
                    kind: TokenKind::LeftParen,
                    span: span(2, 1, 3, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::RightParen,
                    span: span(3, 1, 4, 4, 1, 5),
                },
                Token {
                    kind: TokenKind::Equal,
                    span: span(4, 1, 5, 5, 1, 6),
                },
                Token {
                    kind: TokenKind::Minus,
                    span: span(5, 1, 6, 6, 1, 7),
                },
                Token {
                    kind: TokenKind::GreaterThan,
                    span: span(6, 1, 7, 7, 1, 8),
                },
                eof(7, 1, 8),
            ]
        );
    }

    #[test]
    fn tokenizes_keywords() {
        let src = "let int";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Let,
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Int,
                    span: span(4, 1, 5, 7, 1, 8),
                },
                eof(7, 1, 8),
            ]
        );
    }

    #[test]
    fn tokenizes_identifiers() {
        let src = "x foo bar_123 _temp";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Identifier("x".to_string()),
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::Identifier("foo".to_string()),
                    span: span(2, 1, 3, 5, 1, 6),
                },
                Token {
                    kind: TokenKind::Identifier("bar_123".to_string()),
                    span: span(6, 1, 7, 13, 1, 14),
                },
                Token {
                    kind: TokenKind::Identifier("_temp".to_string()),
                    span: span(14, 1, 15, 19, 1, 20),
                },
                eof(19, 1, 20),
            ]
        );
    }

    #[test]
    fn tokenizes_integer_literals() {
        let src = "0 42 12345";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Literal(Literal::Int(0)),
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(42)),
                    span: span(2, 1, 3, 4, 1, 5),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(12345)),
                    span: span(5, 1, 6, 10, 1, 11),
                },
                eof(10, 1, 11),
            ]
        );
    }

    #[test]
    fn ignores_spaces_tabs_and_carriage_returns() {
        let src = "let\t\r int";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Let,
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Int,
                    span: span(6, 1, 7, 9, 1, 10),
                },
                eof(9, 1, 10),
            ]
        );
    }

    #[test]
    fn tracks_lines_and_columns() {
        let src = "let x\nint y";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Let,
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Identifier("x".to_string()),
                    span: span(4, 1, 5, 5, 1, 6),
                },
                Token {
                    kind: TokenKind::Int,
                    span: span(6, 2, 1, 9, 2, 4),
                },
                Token {
                    kind: TokenKind::Identifier("y".to_string()),
                    span: span(10, 2, 5, 11, 2, 6),
                },
                eof(11, 2, 6),
            ]
        );
    }

    #[test]
    fn tokenizes_small_declaration() {
        let src = "let int x = 42";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Let,
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Int,
                    span: span(4, 1, 5, 7, 1, 8),
                },
                Token {
                    kind: TokenKind::Identifier("x".to_string()),
                    span: span(8, 1, 9, 9, 1, 10),
                },
                Token {
                    kind: TokenKind::Equal,
                    span: span(10, 1, 11, 11, 1, 12),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(42)),
                    span: span(12, 1, 13, 14, 1, 15),
                },
                eof(14, 1, 15),
            ]
        );
    }

    #[test]
    fn keyword_prefixes_are_identifiers() {
        let src = "letter integer";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Identifier("letter".to_string()),
                    span: span(0, 1, 1, 6, 1, 7),
                },
                Token {
                    kind: TokenKind::Identifier("integer".to_string()),
                    span: span(7, 1, 8, 14, 1, 15),
                },
                eof(14, 1, 15),
            ]
        );
    }

    #[test]
    fn unicode_error_span_uses_byte_offsets_but_character_columns() {
        let src = "let æ = 1";

        let err = tokenize(src).unwrap_err();

        assert_eq!(err.span, span(4, 1, 5, 6, 1, 6));
    }

    #[test]
    fn rejects_integer_overflow() {
        let src = "999999999999999999999999999999";

        let err = tokenize(src).unwrap_err();

        assert_eq!(
            err.span,
            span(0, 1, 1, src.len(), 1, src.chars().count() + 1),
        );
    }
}
