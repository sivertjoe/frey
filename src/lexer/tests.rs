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
        let src = "{}()=->,:";

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
                Token {
                    kind: TokenKind::Comma,
                    span: span(7, 1, 8, 8, 1, 9),
                },
                Token {
                    kind: TokenKind::Colon,
                    span: span(8, 1, 9, 9, 1, 10),
                },
                eof(9, 1, 10),
            ]
        );
    }

    #[test]
    fn tokenizes_keywords() {
        let src = "let Int return";

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
                    kind: TokenKind::Return,
                    span: span(8, 1, 9, 14, 1, 15),
                },
                eof(14, 1, 15),
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
        let src = "let\t\r Int";

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
        let src = "let x\nInt y";

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
        let src = "let Int x = 42";

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

    #[test]
    fn tokenizes_if_else_keywords() {
        let src = "if else";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::If,
                    span: span(0, 1, 1, 2, 1, 3),
                },
                Token {
                    kind: TokenKind::Else,
                    span: span(3, 1, 4, 7, 1, 8),
                },
                eof(7, 1, 8),
            ]
        );
    }

    #[test]
    fn tokenizes_arithmetic_operators() {
        let src = "+ - * / %";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Plus,
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::Minus,
                    span: span(2, 1, 3, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Star,
                    span: span(4, 1, 5, 5, 1, 6),
                },
                Token {
                    kind: TokenKind::Slash,
                    span: span(6, 1, 7, 7, 1, 8),
                },
                Token {
                    kind: TokenKind::Percent,
                    span: span(8, 1, 9, 9, 1, 10),
                },
                eof(9, 1, 10),
            ]
        );
    }

    #[test]
    fn tokenizes_comparison_operators() {
        let src = "< <= > >= == !=";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::LessThan,
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::LessEqual,
                    span: span(2, 1, 3, 4, 1, 5),
                },
                Token {
                    kind: TokenKind::GreaterThan,
                    span: span(5, 1, 6, 6, 1, 7),
                },
                Token {
                    kind: TokenKind::GreaterEqual,
                    span: span(7, 1, 8, 9, 1, 10),
                },
                Token {
                    kind: TokenKind::EqualEqual,
                    span: span(10, 1, 11, 12, 1, 13),
                },
                Token {
                    kind: TokenKind::NotEqual,
                    span: span(13, 1, 14, 15, 1, 16),
                },
                eof(15, 1, 16),
            ]
        );
    }

    #[test]
    fn tokenizes_bitwise_and_logical_operators() {
        let src = "& && | || ^ ! << >>";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Ampersand,
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::AmpAmp,
                    span: span(2, 1, 3, 4, 1, 5),
                },
                Token {
                    kind: TokenKind::Pipe,
                    span: span(5, 1, 6, 6, 1, 7),
                },
                Token {
                    kind: TokenKind::PipePipe,
                    span: span(7, 1, 8, 9, 1, 10),
                },
                Token {
                    kind: TokenKind::Caret,
                    span: span(10, 1, 11, 11, 1, 12),
                },
                Token {
                    kind: TokenKind::Not,
                    span: span(12, 1, 13, 13, 1, 14),
                },
                Token {
                    kind: TokenKind::ShiftLeft,
                    span: span(14, 1, 15, 16, 1, 17),
                },
                Token {
                    kind: TokenKind::ShiftRight,
                    span: span(17, 1, 18, 19, 1, 20),
                },
                eof(19, 1, 20),
            ]
        );
    }

    #[test]
    fn prefers_longest_matching_operator() {
        let src = "<< <= && || ==";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::ShiftLeft,
                    span: span(0, 1, 1, 2, 1, 3),
                },
                Token {
                    kind: TokenKind::LessEqual,
                    span: span(3, 1, 4, 5, 1, 6),
                },
                Token {
                    kind: TokenKind::AmpAmp,
                    span: span(6, 1, 7, 8, 1, 9),
                },
                Token {
                    kind: TokenKind::PipePipe,
                    span: span(9, 1, 10, 11, 1, 12),
                },
                Token {
                    kind: TokenKind::EqualEqual,
                    span: span(12, 1, 13, 14, 1, 15),
                },
                eof(14, 1, 15),
            ]
        );
    }

    #[test]
    fn tokenizes_if_expression() {
        let src = "if x { return 1; } else { 2 }";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::If,
                    span: span(0, 1, 1, 2, 1, 3),
                },
                Token {
                    kind: TokenKind::Identifier("x".to_string()),
                    span: span(3, 1, 4, 4, 1, 5),
                },
                Token {
                    kind: TokenKind::LeftBrace,
                    span: span(5, 1, 6, 6, 1, 7),
                },
                Token {
                    kind: TokenKind::Return,
                    span: span(7, 1, 8, 13, 1, 14),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(1)),
                    span: span(14, 1, 15, 15, 1, 16),
                },
                Token {
                    kind: TokenKind::Semicolon,
                    span: span(15, 1, 16, 16, 1, 17),
                },
                Token {
                    kind: TokenKind::RightBrace,
                    span: span(17, 1, 18, 18, 1, 19),
                },
                Token {
                    kind: TokenKind::Else,
                    span: span(19, 1, 20, 23, 1, 24),
                },
                Token {
                    kind: TokenKind::LeftBrace,
                    span: span(24, 1, 25, 25, 1, 26),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(2)),
                    span: span(26, 1, 27, 27, 1, 28),
                },
                Token {
                    kind: TokenKind::RightBrace,
                    span: span(28, 1, 29, 29, 1, 30),
                },
                eof(29, 1, 30),
            ]
        );
    }

    #[test]
    fn tokenizes_nested_parentheses_and_calls() {
        let src = "foo(bar(1, 2))";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Identifier("foo".to_string()),
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::LeftParen,
                    span: span(3, 1, 4, 4, 1, 5),
                },
                Token {
                    kind: TokenKind::Identifier("bar".to_string()),
                    span: span(4, 1, 5, 7, 1, 8),
                },
                Token {
                    kind: TokenKind::LeftParen,
                    span: span(7, 1, 8, 8, 1, 9),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(1)),
                    span: span(8, 1, 9, 9, 1, 10),
                },
                Token {
                    kind: TokenKind::Comma,
                    span: span(9, 1, 10, 10, 1, 11),
                },
                Token {
                    kind: TokenKind::Literal(Literal::Int(2)),
                    span: span(11, 1, 12, 12, 1, 13),
                },
                Token {
                    kind: TokenKind::RightParen,
                    span: span(12, 1, 13, 13, 1, 14),
                },
                Token {
                    kind: TokenKind::RightParen,
                    span: span(13, 1, 14, 14, 1, 15),
                },
                eof(14, 1, 15),
            ]
        );
    }

    #[test]
    fn empty_input_only_emits_eof() {
        let tokens = tokenize("").unwrap();

        assert_eq!(tokens, vec![eof(0, 1, 1)]);
    }

    #[test]
    fn tokenizes_float_literal() {
        let tokens = tokenize("3.14").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Literal(Literal::Float(3.14)),
                    span: span(0, 1, 1, 4, 1, 5),
                },
                eof(4, 1, 5),
            ]
        );
    }

    #[test]
    fn tokenizes_float_with_leading_dot() {
        let tokens = tokenize(".5").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Literal(Literal::Float(0.5)),
                    span: span(0, 1, 1, 2, 1, 3),
                },
                eof(2, 1, 3),
            ]
        );
    }

    #[test]
    fn tokenizes_float_with_exponent() {
        let tokens = tokenize("1e10 2.5E-3 1.5e+2").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert!(
            matches!(kinds[0], TokenKind::Literal(Literal::Float(v)) if (*v - 1e10).abs() < 1.0)
        );
        assert!(
            matches!(kinds[1], TokenKind::Literal(Literal::Float(v)) if (*v - 2.5e-3).abs() < 1e-9)
        );
        assert!(
            matches!(kinds[2], TokenKind::Literal(Literal::Float(v)) if (*v - 150.0).abs() < 1e-3)
        );
    }

    #[test]
    fn float_span_tracks_columns_after_number() {
        // After a number, the next token's column should reflect the number's full width.
        let tokens = tokenize("3.14 x").unwrap();
        assert_eq!(tokens[0].span.end.column, 5);
        // `x` should be at column 6.
        assert_eq!(tokens[1].span.start.column, 6);
    }

    #[test]
    fn integer_followed_by_alpha_is_error() {
        // `123abc` is not a valid integer/float — alphanumeric suffix.
        let err = tokenize("123abc").unwrap_err();
        assert_eq!(err.span.start.column, 1);
        assert_eq!(err.span.end.column, 7);
    }

    #[test]
    fn missing_exponent_digits_is_error() {
        // `1e` without digits in the exponent is invalid.
        assert!(tokenize("1e").is_err());
    }

    #[test]
    fn tokenizes_as_keyword() {
        let tokens = tokenize("as").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::As,
                    span: span(0, 1, 1, 2, 1, 3),
                },
                eof(2, 1, 3),
            ]
        );
    }

    #[test]
    fn as_in_a_cast_expression() {
        let tokens = tokenize("x as Int").unwrap();
        let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).take(3).collect();
        assert_eq!(kinds[0], &TokenKind::Identifier("x".to_string()));
        assert_eq!(kinds[1], &TokenKind::As);
        assert_eq!(kinds[2], &TokenKind::Int);
    }

    #[test]
    fn as_prefix_is_an_identifier() {
        // `asx`, `as_foo`, `assert` — anything where `as` is part of a longer
        // identifier should NOT be tokenized as the `as` keyword.
        for ident in ["asx", "as_foo", "assert", "asd"] {
            let tokens = tokenize(ident).unwrap();
            assert_eq!(
                tokens[0].kind,
                TokenKind::Identifier(ident.to_string()),
                "expected `{ident}` to be an identifier, not a keyword"
            );
        }
    }

    #[test]
    fn as_keyword_in_declaration_context() {
        // `let y = 1.5 as Int;`
        let tokens = tokenize("let y = 1.5 as Int;").unwrap();
        let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(kinds[0], &TokenKind::Let);
        assert_eq!(kinds[1], &TokenKind::Identifier("y".to_string()));
        assert_eq!(kinds[2], &TokenKind::Equal);
        assert!(matches!(kinds[3], TokenKind::Literal(Literal::Float(_))));
        assert_eq!(kinds[4], &TokenKind::As);
        assert_eq!(kinds[5], &TokenKind::Int);
        assert_eq!(kinds[6], &TokenKind::Semicolon);
    }

    // ---- String literals ----

    #[test]
    fn tokenizes_simple_string() {
        let tokens = tokenize("\"hello\"").unwrap();
        assert!(matches!(
            &tokens[0].kind,
            TokenKind::Literal(Literal::Str(s)) if s == "hello"
        ));
    }

    #[test]
    fn tokenizes_empty_string() {
        let tokens = tokenize("\"\"").unwrap();
        assert!(matches!(
            &tokens[0].kind,
            TokenKind::Literal(Literal::Str(s)) if s.is_empty()
        ));
    }

    #[test]
    fn string_escapes_newline_and_tab() {
        let tokens = tokenize("\"a\\nb\\tc\"").unwrap();
        let TokenKind::Literal(Literal::Str(s)) = &tokens[0].kind else {
            panic!("expected string literal");
        };
        assert_eq!(s, "a\nb\tc");
    }

    #[test]
    fn string_escape_null_and_backslash() {
        let tokens = tokenize("\"\\0\\\\\"").unwrap();
        let TokenKind::Literal(Literal::Str(s)) = &tokens[0].kind else {
            panic!("expected string literal");
        };
        assert_eq!(s, "\0\\");
    }

    #[test]
    fn string_escape_quote() {
        let tokens = tokenize("\"\\\"\"").unwrap();
        let TokenKind::Literal(Literal::Str(s)) = &tokens[0].kind else {
            panic!("expected string literal");
        };
        assert_eq!(s, "\"");
    }

    #[test]
    fn unterminated_string_is_error() {
        let result = tokenize("\"oops");
        assert!(result.is_err());
    }

    #[test]
    fn invalid_escape_is_error() {
        let result = tokenize("\"\\q\"");
        assert!(result.is_err());
    }

    #[test]
    fn newline_in_string_is_error() {
        let result = tokenize("\"line1\nline2\"");
        assert!(result.is_err());
    }

    #[test]
    fn tokenizes_dollar_token() {
        let src = "$";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Dollar,
                    span: span(0, 1, 1, 1, 1, 2),
                },
                eof(1, 1, 2),
            ]
        );
    }

    #[test]
    fn tokenizes_generic_parameter_type() {
        let src = "$T";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Dollar,
                    span: span(0, 1, 1, 1, 1, 2),
                },
                Token {
                    kind: TokenKind::Identifier("T".to_string()),
                    span: span(1, 1, 2, 2, 1, 3),
                },
                eof(2, 1, 3),
            ]
        );
    }

    #[test]
    fn tokenizes_generic_parameter_in_function_argument() {
        let src = "let foo = (arg: $T) { }";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Let,
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Identifier("foo".to_string()),
                    span: span(4, 1, 5, 7, 1, 8),
                },
                Token {
                    kind: TokenKind::Equal,
                    span: span(8, 1, 9, 9, 1, 10),
                },
                Token {
                    kind: TokenKind::LeftParen,
                    span: span(10, 1, 11, 11, 1, 12),
                },
                Token {
                    kind: TokenKind::Identifier("arg".to_string()),
                    span: span(11, 1, 12, 14, 1, 15),
                },
                Token {
                    kind: TokenKind::Colon,
                    span: span(14, 1, 15, 15, 1, 16),
                },
                Token {
                    kind: TokenKind::Dollar,
                    span: span(16, 1, 17, 17, 1, 18),
                },
                Token {
                    kind: TokenKind::Identifier("T".to_string()),
                    span: span(17, 1, 18, 18, 1, 19),
                },
                Token {
                    kind: TokenKind::RightParen,
                    span: span(18, 1, 19, 19, 1, 20),
                },
                Token {
                    kind: TokenKind::LeftBrace,
                    span: span(20, 1, 21, 21, 1, 22),
                },
                Token {
                    kind: TokenKind::RightBrace,
                    span: span(22, 1, 23, 23, 1, 24),
                },
                eof(23, 1, 24),
            ]
        );
    }

    #[test]
    fn tokenizes_multiple_generic_parameters_in_function_arguments() {
        let src = "let bar = (foo: $T, baz: $U, foz: T) { }";

        let tokens = tokenize(src).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token {
                    kind: TokenKind::Let,
                    span: span(0, 1, 1, 3, 1, 4),
                },
                Token {
                    kind: TokenKind::Identifier("bar".to_string()),
                    span: span(4, 1, 5, 7, 1, 8),
                },
                Token {
                    kind: TokenKind::Equal,
                    span: span(8, 1, 9, 9, 1, 10),
                },
                Token {
                    kind: TokenKind::LeftParen,
                    span: span(10, 1, 11, 11, 1, 12),
                },
                Token {
                    kind: TokenKind::Identifier("foo".to_string()),
                    span: span(11, 1, 12, 14, 1, 15),
                },
                Token {
                    kind: TokenKind::Colon,
                    span: span(14, 1, 15, 15, 1, 16),
                },
                Token {
                    kind: TokenKind::Dollar,
                    span: span(16, 1, 17, 17, 1, 18),
                },
                Token {
                    kind: TokenKind::Identifier("T".to_string()),
                    span: span(17, 1, 18, 18, 1, 19),
                },
                Token {
                    kind: TokenKind::Comma,
                    span: span(18, 1, 19, 19, 1, 20),
                },
                Token {
                    kind: TokenKind::Identifier("baz".to_string()),
                    span: span(20, 1, 21, 23, 1, 24),
                },
                Token {
                    kind: TokenKind::Colon,
                    span: span(23, 1, 24, 24, 1, 25),
                },
                Token {
                    kind: TokenKind::Dollar,
                    span: span(25, 1, 26, 26, 1, 27),
                },
                Token {
                    kind: TokenKind::Identifier("U".to_string()),
                    span: span(26, 1, 27, 27, 1, 28),
                },
                Token {
                    kind: TokenKind::Comma,
                    span: span(27, 1, 28, 28, 1, 29),
                },
                Token {
                    kind: TokenKind::Identifier("foz".to_string()),
                    span: span(29, 1, 30, 32, 1, 33),
                },
                Token {
                    kind: TokenKind::Colon,
                    span: span(32, 1, 33, 33, 1, 34),
                },
                Token {
                    kind: TokenKind::Identifier("T".to_string()),
                    span: span(34, 1, 35, 35, 1, 36),
                },
                Token {
                    kind: TokenKind::RightParen,
                    span: span(35, 1, 36, 36, 1, 37),
                },
                Token {
                    kind: TokenKind::LeftBrace,
                    span: span(37, 1, 38, 38, 1, 39),
                },
                Token {
                    kind: TokenKind::RightBrace,
                    span: span(39, 1, 40, 40, 1, 41),
                },
                eof(40, 1, 41),
            ]
        );
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn skips_line_comments() {
        assert_eq!(
            kinds("let // a comment\nx"),
            vec![
                TokenKind::Let,
                TokenKind::Identifier("x".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_block_comments() {
        assert_eq!(
            kinds("let /* a\n comment */ x"),
            vec![
                TokenKind::Let,
                TokenKind::Identifier("x".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn block_comments_nest() {
        assert_eq!(
            kinds("/* outer /* inner */ still */ x"),
            vec![TokenKind::Identifier("x".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn slash_is_still_division() {
        assert_eq!(
            kinds("a / b"),
            vec![
                TokenKind::Identifier("a".to_string()),
                TokenKind::Slash,
                TokenKind::Identifier("b".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unterminated_block_comment_errors() {
        assert!(tokenize("/* never closed").is_err());
    }

    #[test]
    fn mut_is_no_longer_a_keyword() {
        // After mut was removed, `mut` is just an identifier.
        assert_eq!(
            kinds("mut"),
            vec![TokenKind::Identifier("mut".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn tokenizes_enum_and_match_keywords() {
        assert_eq!(
            kinds("enum match"),
            vec![TokenKind::Enum, TokenKind::Match, TokenKind::Eof,]
        );
    }

    #[test]
    fn tuple_field_access_lexes_as_int_dot_int() {
        // `t.0.1` must lex as identifier, dot, int(0), dot, int(1) — not as
        // identifier, dot, float(0.1) — otherwise `.0.1` would be a single
        // float literal and nested tuple indexing would break.
        assert_eq!(
            kinds("t.0.1"),
            vec![
                TokenKind::Identifier("t".to_string()),
                TokenKind::Dot,
                TokenKind::Literal(Literal::Int(0)),
                TokenKind::Dot,
                TokenKind::Literal(Literal::Int(1)),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn leading_dot_float_still_works_in_expression_position() {
        // `let x = .5;` — `.5` is a float literal because the previous token
        // (`=`) doesn't end an expression.
        let ks = kinds("let x = .5;");
        assert!(matches!(
            ks[3],
            TokenKind::Literal(Literal::Float(f)) if (f - 0.5).abs() < 1e-6
        ));
    }
}
