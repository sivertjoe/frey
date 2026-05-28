/*
# Sample code:

let main = () -> Int {
    return 0;
}

This should turn into

program ::= { <declaration> }
declaration ::= "let" [ "mut" ] <ident> "=" <expr> ";"

block ::= "{" { <block-item> } [<expr>] "}"
block-item ::=
    <declaration>
    | <statement>

statement ::=
    "return" [<expr>] ";"
    | "break" ";"
    | <expr> ";"

<expr> ::=
    <const>
    | <ident>
    | <function-literal>
    | <block>
    | "(" <expr> ")"
    | <expr> "(" [ <expr> { "," <expr> } ] ")"
    | <expr> "[" <expr> "]"
    | "[" [ <expr> { "," <expr> } ] "]"
    | <unary-op> <expr>
    | <expr> "as" <type>
    | <expr> <binary-op> <expr>
    | <expr> "=" <expr>
    | if <expr> <block> [ "else" <expr> ]
    | while <expr> <block>

<const> ::= <integer-literal> | <float-literal>

<unary-op> ::= "-" | "!" | "&" | "*"

<binary-op> ::=
      "+" | "-" | "*" | "/" | "%"
    | "<<" | ">>"
    | "<" | "<=" | ">" | ">="
    | "==" | "!="
    | "&" | "^" | "|"
    | "&&" | "||"

`|>` is the pipe operator: `a |> f(b, c)` desugars to `f(a, b, c)`.
Left-associative, lowest precedence among binary operators. The right
operand must syntactically be a call.

<function-literal> ::= "(" [<params>] ")" [ "->" <type> ] <block>
<params> ::= <param> { "," <param> }
<param>  ::= <ident> ":" <type>

<type> ::=
    "Int"
    | "UInt"
    | "Float"
    | "i8"
    | "i32"
    | "i64"
    | "u8"
    | "u32"
    | "u64"
    | "f32"
    | "f64"
    | <function-type>
    | <array-type>
    | <ptr-type>

<ptr-type> ::= "*" <type>

<array-type> ::= "[" <type> ";" <integer-literal> "]"

<function-type> ::= "(" [<type-list>] ")" "->" <type>
<type-list> ::= <type> { "," <type> }

<ident> is any identifier matching [A-Za-z_][A-Za-z0-9_]*.
<integer-literal> and <float-literal> are decimal numeric literals; the
integer form is parsed as i32.
*/

use crate::{
    ast::{
        UnaryOperator,
        error::Error,
        token_iter::TokenIter,
        types::{
            BinaryOperator, Block, BlockItem, Const, Declaration, Expr, ExprKind, ImportDecl,
            NodeIdGen, Param, Program, Statement, StatementKind, StructLiteralField,
            StructTypeField, TypeExpr, TypeExprKind,
        },
    },
    lexer::types::{Literal, Token, TokenKind},
};

pub(super) struct Parser {
    iter: TokenIter,
    id_gen: NodeIdGen,
}

impl Parser {
    #[cfg(test)]
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        Self {
            iter: TokenIter::new(tokens),
            id_gen: NodeIdGen::new(),
        }
    }

    /// Like `new`, but starts node numbering at `node_base` so ids are unique
    /// across the files merged by the module loader.
    pub(super) fn new_at(tokens: Vec<Token>, node_base: u32) -> Self {
        Self {
            iter: TokenIter::new(tokens),
            id_gen: NodeIdGen::with_next(node_base),
        }
    }

    /// The next node id this parser would hand out (for chaining across files).
    pub(super) fn node_id_count(&self) -> u32 {
        self.id_gen.next_value()
    }

    pub(super) fn parse(&mut self) -> Result<Program, Error> {
        self.parse_program()
    }

    pub(super) fn parse_declaration(&mut self) -> Result<Declaration, Error> {
        // Optional `#comptime` attribute: `#comptime let ... = ...;`.
        let (comptime, attr_span) = if self.check(TokenKind::Hash) {
            let hash = self.expect(TokenKind::Hash)?;
            let name_tok = self.iter.consume().expect("lexer emits eof");
            match &name_tok.kind {
                TokenKind::Identifier(n) if n == "comptime" => {}
                _ => return Err(Error::unexpected(&name_tok, "`comptime`")),
            }
            (true, Some(hash.span))
        } else {
            (false, None)
        };

        let let_span = self.expect(TokenKind::Let)?.span;
        let start = attr_span.unwrap_or(let_span);
        let mutable = if self.check(TokenKind::Mut) {
            self.expect(TokenKind::Mut)?;
            true
        } else {
            false
        };
        let name = self.ident()?;
        self.expect(TokenKind::Equal)?;
        let expr = self.parse_expr()?;
        self.expect(TokenKind::Semicolon)?;

        Ok(Declaration {
            id: self.id_gen.fresh(),
            span: start.join(expr.span),
            mutable,
            comptime,
            name,
            value: expr,
        })
    }

    pub(super) fn parse_program(&mut self) -> Result<Program, Error> {
        let mut declarations = Vec::new();
        let mut imports = Vec::new();

        while !self.eof() {
            if self.check(TokenKind::Import) {
                imports.push(self.parse_import()?);
            } else {
                declarations.push(self.parse_declaration()?);
            }
        }

        if declarations.is_empty() && imports.is_empty() {
            let eof = self.iter.peek().expect("lexer emits eof");
            return Err(Error::unexpected(eof, "declaration"));
        }

        let span = if let (Some(first), Some(last)) = (declarations.first(), declarations.last()) {
            first.span.join(last.span)
        } else {
            // declarations is empty, so imports is not (checked above).
            imports.first().unwrap().span.join(imports.last().unwrap().span)
        };
        Ok(Program {
            span,
            declarations,
            imports,
        })
    }

    fn parse_import(&mut self) -> Result<ImportDecl, Error> {
        let start = self.expect(TokenKind::Import)?.span;
        let tok = self.iter.consume().expect("lexer emits eof");
        let TokenKind::Literal(Literal::Str(path)) = tok.kind else {
            return Err(Error::unexpected(&tok, "a module path string"));
        };
        let end = self.expect(TokenKind::Semicolon)?.span;
        Ok(ImportDecl {
            span: start.join(end),
            path,
        })
    }

    pub(super) fn parse_type(&mut self) -> Result<TypeExpr, Error> {
        let tok = self.iter.peek().expect("lexer always emits Eof");
        let kind = match &tok.kind {
            TokenKind::Int => TypeExprKind::Int,
            TokenKind::UInt => TypeExprKind::UInt,
            TokenKind::Float => TypeExprKind::Float,
            TokenKind::I8 => TypeExprKind::I8,
            TokenKind::I32 => TypeExprKind::I32,
            TokenKind::I64 => TypeExprKind::I64,
            TokenKind::U8 => TypeExprKind::U8,
            TokenKind::U32 => TypeExprKind::U32,
            TokenKind::U64 => TypeExprKind::U64,
            TokenKind::F32 => TypeExprKind::F32,
            TokenKind::F64 => TypeExprKind::F64,
            TokenKind::Identifier(name) => {
                let name = name.clone();
                // `Identifier<` in type position is a generic application
                // like `Vec<Int>`. The angle brackets are unambiguous here
                // because comparisons don't occur in type positions.
                if matches!(
                    self.iter.peek_nth(1).map(|t| &t.kind),
                    Some(TokenKind::LessThan)
                ) {
                    return self.parse_named_generic_type(name);
                }
                TypeExprKind::Named(name)
            }
            TokenKind::Star => return self.parse_ptr_type(),
            TokenKind::LeftParen => return self.parse_function_type(),
            TokenKind::LeftBracket => return self.parse_array_type(),
            TokenKind::Dollar => return self.parse_generic_type(),
            _ => return Err(Error::unexpected(tok, "type")),
        };
        let span = self.iter.consume().unwrap().span;
        Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span,
            kind,
        })
    }

    fn parse_named_generic_type(&mut self, name: String) -> Result<TypeExpr, Error> {
        let start = self.iter.consume().unwrap().span; // consume the Identifier
        self.expect(TokenKind::LessThan)?;
        let mut args = Vec::new();
        while !self.check(TokenKind::GreaterThan) {
            if !args.is_empty() {
                self.expect(TokenKind::Comma)?;
                if self.check(TokenKind::GreaterThan) {
                    break;
                }
            }
            args.push(self.parse_type()?);
        }
        let end = self.expect(TokenKind::GreaterThan)?.span;
        Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span: start.join(end),
            kind: TypeExprKind::NamedGeneric { name, args },
        })
    }

    pub(super) fn parse_generic_type(&mut self) -> Result<TypeExpr, Error> {
        let tok = self.iter.consume().unwrap();
        let next = self.iter.peek().expect("lexer always emits Eof");
        let TokenKind::Identifier(_) = &next.kind else {
            return Err(Error::unexpected(next, "named type"));
        };
        let next = self.iter.consume().unwrap();
        let TokenKind::Identifier(name) = &next.kind else {
            unreachable!();
        };

        let span = tok.span.join(next.span);
        return Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span,
            kind: TypeExprKind::Named(format!("${name}")),
        });
    }

    pub(super) fn parse_ptr_type(&mut self) -> Result<TypeExpr, Error> {
        let start = self.expect(TokenKind::Star)?.span;
        let target = Box::new(self.parse_type()?);
        let span = start.join(target.span);
        Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span,
            kind: TypeExprKind::Ptr(target),
        })
    }

    pub(super) fn parse_array_type(&mut self) -> Result<TypeExpr, Error> {
        let start = self.expect(TokenKind::LeftBracket)?.span;
        let element_ty = Box::new(self.parse_type()?);
        self.expect(TokenKind::Semicolon)?;

        let count_tok = self.iter.consume().expect("lexer emits eof");
        let count = match count_tok.kind {
            TokenKind::Literal(Literal::Int(n)) if n >= 0 => n as usize,
            _ => {
                return Err(Error::unexpected(
                    &count_tok,
                    "non-negative integer literal",
                ));
            }
        };

        let end = self.expect(TokenKind::RightBracket)?.span;
        let span = start.join(end);
        Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span,
            kind: TypeExprKind::Array { element_ty, count },
        })
    }

    pub(super) fn parse_function_type(&mut self) -> Result<TypeExpr, Error> {
        let left = self.expect(TokenKind::LeftParen)?.span;

        let mut params = Vec::new();
        while !matches!(self.iter.peek(), Some(t) if t.kind == TokenKind::RightParen) {
            if !params.is_empty() {
                self.expect(TokenKind::Comma)?;
            }
            params.push(self.parse_type()?);
        }

        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::Minus)?;
        self.expect(TokenKind::GreaterThan)?;
        let return_ty = Box::new(self.parse_type()?);

        let span = left.join(return_ty.span);
        let kind = TypeExprKind::Function { params, return_ty };
        Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span,
            kind,
        })
    }

    pub(super) fn parse_param(&mut self) -> Result<Param, Error> {
        let start = self.iter.peek().expect("lexer emits eof").span;
        let name = self.ident()?;
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type()?;
        let span = start.join(ty.span);
        Ok(Param {
            id: self.id_gen.fresh(),
            span,
            name,
            ty,
        })
    }

    pub(super) fn parse_block(&mut self) -> Result<Block, Error> {
        let left = self.expect(TokenKind::LeftBrace)?.span;
        let mut items = Vec::new();
        let mut tail = None;

        while !matches!(self.iter.peek(), Some(t) if t.kind == TokenKind::RightBrace) {
            match &self.iter.peek().expect("lexer emits eof").kind {
                TokenKind::Let | TokenKind::Hash => {
                    let decl = self.parse_declaration()?;
                    items.push(BlockItem::Declaration(decl));
                }
                TokenKind::Return => {
                    let start = self.expect(TokenKind::Return)?.span;
                    let expr = if self.check(TokenKind::Semicolon) {
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    let end = self.expect(TokenKind::Semicolon)?.span;
                    items.push(BlockItem::Statement(Statement {
                        id: self.id_gen.fresh(),
                        span: start.join(end),
                        kind: StatementKind::Return(expr),
                    }));
                }
                TokenKind::Break => {
                    let start = self.expect(TokenKind::Break)?.span;
                    let end = self.expect(TokenKind::Semicolon)?.span;
                    items.push(BlockItem::Statement(Statement {
                        id: self.id_gen.fresh(),
                        span: start.join(end),
                        kind: StatementKind::Break,
                    }));
                }
                TokenKind::Defer => {
                    let start = self.expect(TokenKind::Defer)?.span;
                    let expr = self.parse_expr()?;
                    let end = self.expect(TokenKind::Semicolon)?.span;
                    items.push(BlockItem::Statement(Statement {
                        id: self.id_gen.fresh(),
                        span: start.join(end),
                        kind: StatementKind::Defer(expr),
                    }));
                }
                _ => {
                    // Bare expression at statement position. Outcomes:
                    //   - followed by `;`  → expression statement
                    //   - followed by `}`  → tail expression
                    //   - block-like expr  → statement with no `;` required
                    //     (since `}` already terminates it visually)
                    //   - anything else    → error
                    let expr = self.parse_expr()?;
                    let after_kind = self.iter.peek().expect("lexer emits eof").kind.clone();
                    match after_kind {
                        TokenKind::Semicolon => {
                            let end = self.expect(TokenKind::Semicolon)?.span;
                            items.push(BlockItem::Statement(Statement {
                                id: self.id_gen.fresh(),
                                span: expr.span.join(end),
                                kind: StatementKind::Expr(expr),
                            }));
                        }
                        TokenKind::RightBrace => {
                            tail = Some(Box::new(expr));
                            break;
                        }
                        _ if is_block_like(&expr) => {
                            let span = expr.span;
                            items.push(BlockItem::Statement(Statement {
                                id: self.id_gen.fresh(),
                                span,
                                kind: StatementKind::Expr(expr),
                            }));
                        }
                        _ => {
                            let next = self.iter.peek().expect("lexer emits eof");
                            return Err(Error::unexpected(next, "`;` or `}`"));
                        }
                    }
                }
            }
        }

        let right = self.expect(TokenKind::RightBrace)?.span;

        let span = left.join(right);
        Ok(Block {
            id: self.id_gen.fresh(),
            span,
            items,
            tail,
        })
    }

    pub(super) fn parse_expr(&mut self) -> Result<Expr, Error> {
        let lhs = self.parse_binary_expr(0)?;
        // Assignment: lowest precedence, right-associative, LHS must be a
        // place expression (identifier or subscript).
        if self.check(TokenKind::Equal) {
            self.expect(TokenKind::Equal)?;
            if !is_place_expr(&lhs) {
                return Err(Error::unexpected(
                    &Token {
                        kind: TokenKind::Equal,
                        span: lhs.span,
                    },
                    "assignable place expression on the left of `=`",
                ));
            }
            let value = self.parse_expr()?;
            let span = lhs.span.join(value.span);
            return Ok(Expr {
                id: self.id_gen.fresh(),
                span,
                kind: ExprKind::Assign {
                    target: Box::new(lhs),
                    value: Box::new(value),
                },
            });
        }
        Ok(lhs)
    }

    fn parse_binary_expr(&mut self, min_prec: i32) -> Result<Expr, Error> {
        let mut lhs = self.parse_unary_with_cast()?;

        loop {
            // `|>` desugars to a function call. It sits at the lowest binary
            // precedence (below `||`), is left-associative, and the RHS must
            // syntactically be a call.
            if self.check(TokenKind::PipeArrow) {
                const PIPE_PREC: i32 = 1;
                if PIPE_PREC < min_prec {
                    break;
                }
                self.expect(TokenKind::PipeArrow)?;
                let rhs = self.parse_binary_expr(PIPE_PREC + 1)?;
                lhs = self.splice_pipe(lhs, rhs)?;
                continue;
            }

            let Some((prec, op)) = self.iter.peek().and_then(|t| binary_precedence(&t.kind)) else {
                break;
            };
            if prec < min_prec {
                break;
            }
            self.iter.consume();
            let rhs = self.parse_binary_expr(prec + 1)?;
            let span = lhs.span.join(rhs.span);
            lhs = Expr {
                id: self.id_gen.fresh(),
                span,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            };
        }
        Ok(lhs)
    }

    fn splice_pipe(&mut self, lhs: Expr, rhs: Expr) -> Result<Expr, Error> {
        let span = lhs.span.join(rhs.span);
        match rhs.kind {
            ExprKind::Call {
                callee,
                type_args,
                mut args,
            } => {
                args.insert(0, lhs);
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Call {
                        callee,
                        type_args,
                        args,
                    },
                })
            }
            _ => Err(Error {
                span: rhs.span,
                kind: crate::ast::error::ErrorKind::PipeRhsNotCall,
            }),
        }
    }

    fn parse_unary_with_cast(&mut self) -> Result<Expr, Error> {
        let mut e = self.parse_unary()?;
        while self.check(TokenKind::As) {
            self.expect(TokenKind::As)?;
            let target = self.parse_type()?;
            let span = e.span.join(target.span);
            e = Expr {
                id: self.id_gen.fresh(),
                span,
                kind: ExprKind::Cast {
                    expr: Box::new(e),
                    target,
                },
            };
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<Expr, Error> {
        if !self.check_is_unop() {
            return self.parse_postfix();
        }
        let tok = self.iter.consume().unwrap();
        let start = tok.span;
        let operand = self.parse_unary()?;
        let span = start.join(operand.span);
        let kind = match tok.kind {
            TokenKind::Not => ExprKind::Unary {
                op: UnaryOperator::Not,
                expr: Box::new(operand),
            },
            TokenKind::Minus => ExprKind::Unary {
                op: UnaryOperator::Minus,
                expr: Box::new(operand),
            },
            TokenKind::Ampersand => ExprKind::Ref(Box::new(operand)),
            TokenKind::Star => ExprKind::Deref(Box::new(operand)),
            _ => unreachable!(),
        };
        Ok(Expr {
            id: self.id_gen.fresh(),
            span,
            kind,
        })
    }

    fn parse_postfix(&mut self) -> Result<Expr, Error> {
        let mut e = self.parse_primary()?;
        loop {
            if self.check(TokenKind::LeftParen) {
                e = self.parse_call_suffix(e, Vec::new())?;
            } else if self.check(TokenKind::LessThan) && self.generic_call_ahead() {
                // `callee<T, U>(args)` — explicit type arguments on a call.
                let type_args = self.parse_type_args()?;
                e = self.parse_call_suffix(e, type_args)?;
            } else if self.check(TokenKind::LeftBracket) {
                e = self.parse_subscript_suffix(e)?;
            } else if self.check(TokenKind::Dot) {
                e = self.parse_field_suffix(e)?;
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_field_suffix(&mut self, target: Expr) -> Result<Expr, Error> {
        self.expect(TokenKind::Dot)?;
        let name_tok = self.iter.consume().expect("lexer emits eof");
        let TokenKind::Identifier(name) = name_tok.kind else {
            return Err(Error::unexpected(&name_tok, "field name"));
        };
        let span = target.span.join(name_tok.span);
        Ok(Expr {
            id: self.id_gen.fresh(),
            span,
            kind: ExprKind::Field {
                target: Box::new(target),
                name,
            },
        })
    }

    /// `Identifier { Identifier :` or `Identifier { }` — disambiguates a
    /// struct literal from an identifier followed by a block. Also handles an
    /// explicit type-argument list: `Identifier<...> { ... }`.
    fn looks_like_struct_literal(&self) -> bool {
        // Find where the `{` would be: right after the name, or after a
        // `<...>` type-argument list if one is present.
        let brace_at = if matches!(
            self.iter.peek_nth(1).map(|t| &t.kind),
            Some(TokenKind::LessThan)
        ) {
            match self.type_arg_list_end(1) {
                Some(after) => after,
                None => return false,
            }
        } else {
            1
        };
        if !matches!(
            self.iter.peek_nth(brace_at).map(|t| &t.kind),
            Some(TokenKind::LeftBrace)
        ) {
            return false;
        }
        match (
            self.iter.peek_nth(brace_at + 1).map(|t| &t.kind),
            self.iter.peek_nth(brace_at + 2).map(|t| &t.kind),
        ) {
            (Some(TokenKind::RightBrace), _) => true,
            (Some(TokenKind::Identifier(_)), Some(TokenKind::Colon)) => true,
            _ => false,
        }
    }

    fn parse_struct_literal(&mut self) -> Result<Expr, Error> {
        let name_tok = self.iter.consume().expect("checked by caller");
        let start = name_tok.span;
        let TokenKind::Identifier(name) = name_tok.kind else {
            unreachable!("caller verified identifier");
        };
        let type_args = if self.check(TokenKind::LessThan) {
            self.parse_type_args()?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::LeftBrace)?;
        let mut fields = Vec::new();
        while !self.check(TokenKind::RightBrace) {
            if !fields.is_empty() {
                self.expect(TokenKind::Comma)?;
                if self.check(TokenKind::RightBrace) {
                    break;
                }
            }
            let field_start = self.iter.peek().expect("lexer emits eof").span;
            let field_name = self.ident()?;
            self.expect(TokenKind::Colon)?;
            let value = self.parse_expr()?;
            let span = field_start.join(value.span);
            fields.push(StructLiteralField {
                id: self.id_gen.fresh(),
                span,
                name: field_name,
                value,
            });
        }
        let end = self.expect(TokenKind::RightBrace)?.span;
        Ok(Expr {
            id: self.id_gen.fresh(),
            span: start.join(end),
            kind: ExprKind::StructLiteral {
                name,
                type_args,
                fields,
            },
        })
    }

    fn parse_struct_def(&mut self) -> Result<Expr, Error> {
        let start = self.expect(TokenKind::Struct)?.span;

        // Optional type parameter list: struct<$K, $V> { ... }
        let type_params = if self.check(TokenKind::LessThan) {
            self.parse_type_param_list()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LeftBrace)?;
        let mut fields = Vec::new();
        while !self.check(TokenKind::RightBrace) {
            if !fields.is_empty() {
                self.expect(TokenKind::Comma)?;
                if self.check(TokenKind::RightBrace) {
                    break;
                }
            }
            let field_start = self.iter.peek().expect("lexer emits eof").span;
            let name = self.ident()?;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type()?;
            let span = field_start.join(ty.span);
            fields.push(StructTypeField {
                id: self.id_gen.fresh(),
                span,
                name,
                ty,
            });
        }
        let end = self.expect(TokenKind::RightBrace)?.span;
        Ok(Expr {
            id: self.id_gen.fresh(),
            span: start.join(end),
            kind: ExprKind::StructDef {
                type_params,
                fields,
            },
        })
    }

    /// Parses a `<$K, $V>` generic parameter list, returning the names without
    /// the `$`. The opening `<` must be the current token.
    fn parse_type_param_list(&mut self) -> Result<Vec<String>, Error> {
        self.expect(TokenKind::LessThan)?;
        let mut type_params: Vec<String> = Vec::new();
        while !self.check(TokenKind::GreaterThan) {
            if !type_params.is_empty() {
                self.expect(TokenKind::Comma)?;
                if self.check(TokenKind::GreaterThan) {
                    break;
                }
            }
            self.expect(TokenKind::Dollar)?;
            let name_tok = self.iter.consume().expect("lexer emits eof");
            let TokenKind::Identifier(name) = name_tok.kind else {
                return Err(Error::unexpected(&name_tok, "generic type name"));
            };
            if type_params.iter().any(|n| n == &name) {
                return Err(Error::unexpected(
                    &Token {
                        kind: TokenKind::Identifier(name.clone()),
                        span: name_tok.span,
                    },
                    "unique generic type name",
                ));
            }
            type_params.push(name);
        }
        self.expect(TokenKind::GreaterThan)?;
        Ok(type_params)
    }

    /// Parses the `(params) [-> ret] { body }` tail of a function literal,
    /// given an already-parsed (possibly empty) generic parameter list. The
    /// opening `(` must be the current token.
    fn finish_function_literal(
        &mut self,
        start: crate::lexer::types::Span,
        type_params: Vec<String>,
    ) -> Result<Expr, Error> {
        self.expect(TokenKind::LeftParen)?;
        let mut params = Vec::new();
        while !matches!(self.iter.peek(), Some(t) if t.kind == TokenKind::RightParen) {
            if !params.is_empty() {
                self.expect(TokenKind::Comma)?;
            }
            params.push(self.parse_param()?);
        }
        self.expect(TokenKind::RightParen)?;

        let return_ty = if self.check(TokenKind::Minus) {
            self.expect(TokenKind::Minus)?;
            self.expect(TokenKind::GreaterThan)?;
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        let span = start.join(body.span);
        Ok(Expr {
            id: self.id_gen.fresh(),
            span,
            kind: ExprKind::Function {
                type_params,
                params,
                return_ty,
                body,
            },
        })
    }

    fn parse_primary(&mut self) -> Result<Expr, Error> {
        let tok = self.iter.peek().expect("lexer emits eof");

        match &tok.kind {
            TokenKind::Literal(Literal::Int(int)) => {
                let value = *int;
                let span = tok.span;
                self.iter.consume();
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Const(Const::Int(value)),
                })
            }
            TokenKind::Literal(Literal::Float(float)) => {
                let value = *float;
                let span = tok.span;
                self.iter.consume();
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Const(Const::Float(value)),
                })
            }
            TokenKind::Literal(Literal::Str(s)) => {
                let value = s.clone();
                let span = tok.span;
                self.iter.consume();
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Const(Const::Str(value)),
                })
            }
            TokenKind::Identifier(_) => {
                if self.looks_like_struct_literal() {
                    return self.parse_struct_literal();
                }
                let span = tok.span;
                let name = self.ident()?;
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Identifier(name),
                })
            }
            TokenKind::Struct => return self.parse_struct_def(),
            // A leading generic parameter list introduces a generic function
            // literal: `<$K, $V>(params) -> ret { body }`.
            TokenKind::LessThan => {
                let start = tok.span;
                let type_params = self.parse_type_param_list()?;
                return self.finish_function_literal(start, type_params);
            }
            TokenKind::LeftBracket => {
                let start = self.expect(TokenKind::LeftBracket)?.span;

                let mut exprs = Vec::new();
                while !self.check(TokenKind::RightBracket) {
                    if !exprs.is_empty() {
                        self.expect(TokenKind::Comma)?;
                    }
                    let expr = self.parse_expr()?;
                    exprs.push(expr);
                }

                let end = self.expect(TokenKind::RightBracket)?.span;

                let span = start.join(end);

                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Array(exprs),
                })
            }
            TokenKind::LeftParen => {
                // Disambiguate: function literal vs parenthesized expression.
                // `()` and `(ident : ...)` are function literals; everything
                // else inside `( ... )` is a grouped expression.
                let is_function_literal = matches!(
                    self.iter.peek_nth(1).map(|t| &t.kind),
                    Some(TokenKind::RightParen)
                ) || matches!(
                    (
                        self.iter.peek_nth(1).map(|t| &t.kind),
                        self.iter.peek_nth(2).map(|t| &t.kind),
                    ),
                    (Some(TokenKind::Identifier(_)), Some(TokenKind::Colon))
                );

                if is_function_literal {
                    let start = self.iter.peek().expect("lexer emits eof").span;
                    self.finish_function_literal(start, Vec::new())
                } else {
                    let left = self.expect(TokenKind::LeftParen)?.span;
                    let mut expr = self.parse_expr()?;
                    let right = self.expect(TokenKind::RightParen)?.span;
                    expr.span = left.join(right);
                    Ok(expr)
                }
            }
            TokenKind::LeftBrace => {
                let block = self.parse_block()?;
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span: block.span,
                    kind: ExprKind::Block(block),
                })
            }
            TokenKind::If => {
                let start = self.expect(TokenKind::If)?.span;

                let condition = self.parse_expr()?;
                let then_branch = self.parse_block()?;

                let else_branch = if self.check(TokenKind::Else) {
                    self.expect(TokenKind::Else)?;
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };

                let span = if let Some(e) = &else_branch {
                    start.join(e.span)
                } else {
                    start.join(then_branch.span)
                };

                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        then_branch,
                        else_branch,
                    },
                })
            }
            TokenKind::While => {
                let start = self.expect(TokenKind::While)?.span;
                let condition = self.parse_expr()?;
                let body = self.parse_block()?;
                let span = start.join(body.span);
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::While {
                        condition: Box::new(condition),
                        body,
                    },
                })
            }
            // Type keywords in expression position are only meaningful inside
            // a `#comptime` function (e.g. the `Int` in `T == Int`). The parser
            // accepts them as a `TypeValue`; lowering rejects them outside
            // comptime.
            TokenKind::Int
            | TokenKind::UInt
            | TokenKind::Float
            | TokenKind::I8
            | TokenKind::I32
            | TokenKind::I64
            | TokenKind::U8
            | TokenKind::U32
            | TokenKind::U64
            | TokenKind::F32
            | TokenKind::F64 => {
                let ty = self.parse_type()?;
                let span = ty.span;
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::TypeValue(ty),
                })
            }
            _ => Err(Error::unexpected(tok, "expression")),
        }
    }

    fn parse_call_suffix(&mut self, callee: Expr, type_args: Vec<TypeExpr>) -> Result<Expr, Error> {
        self.expect(TokenKind::LeftParen)?;

        let mut args = Vec::new();
        while !self.check(TokenKind::RightParen) {
            if !args.is_empty() {
                self.expect(TokenKind::Comma)?;
            }
            args.push(self.parse_expr()?);
        }

        let end = self.expect(TokenKind::RightParen)?.span;
        let span = callee.span.join(end);
        Ok(Expr {
            id: self.id_gen.fresh(),
            span,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                type_args,
                args,
            },
        })
    }

    /// Consumes a `<T, U>` type-argument list. The opening `<` must be current.
    fn parse_type_args(&mut self) -> Result<Vec<TypeExpr>, Error> {
        self.expect(TokenKind::LessThan)?;
        let mut args = Vec::new();
        while !self.check(TokenKind::GreaterThan) {
            if !args.is_empty() {
                self.expect(TokenKind::Comma)?;
                if self.check(TokenKind::GreaterThan) {
                    break;
                }
            }
            args.push(self.parse_type()?);
        }
        self.expect(TokenKind::GreaterThan)?;
        Ok(args)
    }

    /// Non-consuming lookahead to resolve the `<` ambiguity: starting at the
    /// `<` token at offset `at`, scan a balanced angle-bracket group made only
    /// of tokens that can appear in a type-argument list. Returns the offset of
    /// the token just past the closing `>` (or `>>`), or None if it doesn't
    /// look like a type-argument list.
    fn type_arg_list_end(&self, mut at: usize) -> Option<usize> {
        if !matches!(
            self.iter.peek_nth(at).map(|t| &t.kind),
            Some(TokenKind::LessThan)
        ) {
            return None;
        }
        let mut depth: i32 = 0;
        loop {
            let kind = &self.iter.peek_nth(at)?.kind;
            match kind {
                TokenKind::LessThan => depth += 1,
                TokenKind::GreaterThan => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(at + 1);
                    }
                    if depth < 0 {
                        return None;
                    }
                }
                // A `>>` token closes two levels at once (`Map<Vec<Int>>`).
                TokenKind::ShiftRight => {
                    depth -= 2;
                    if depth == 0 {
                        return Some(at + 1);
                    }
                    if depth < 0 {
                        return None;
                    }
                }
                // Tokens that may legitimately appear inside a type argument.
                TokenKind::Identifier(_)
                | TokenKind::Int
                | TokenKind::UInt
                | TokenKind::Float
                | TokenKind::I8
                | TokenKind::I32
                | TokenKind::I64
                | TokenKind::U8
                | TokenKind::U32
                | TokenKind::U64
                | TokenKind::F32
                | TokenKind::F64
                | TokenKind::Comma
                | TokenKind::Star
                | TokenKind::LeftBracket
                | TokenKind::RightBracket
                | TokenKind::Semicolon
                | TokenKind::Literal(_)
                | TokenKind::Dollar => {}
                // Anything else means this `<` was a comparison, not type args.
                _ => return None,
            }
            at += 1;
        }
    }

    /// True when the upcoming `<...>` is a call's type-argument list (i.e. the
    /// matching `>` is immediately followed by `(`).
    fn generic_call_ahead(&self) -> bool {
        match self.type_arg_list_end(0) {
            Some(after) => matches!(
                self.iter.peek_nth(after).map(|t| &t.kind),
                Some(TokenKind::LeftParen)
            ),
            None => false,
        }
    }

    fn parse_subscript_suffix(&mut self, target: Expr) -> Result<Expr, Error> {
        self.expect(TokenKind::LeftBracket)?;
        let index = self.parse_expr()?;
        let end = self.expect(TokenKind::RightBracket)?.span;
        let span = target.span.join(end);
        Ok(Expr {
            id: self.id_gen.fresh(),
            span,
            kind: ExprKind::Subscript {
                expr: Box::new(target),
                index: Box::new(index),
            },
        })
    }
}

impl Parser {
    fn expect(&mut self, kind: TokenKind) -> Result<Token, Error> {
        let Some(tok) = self.iter.consume() else {
            todo!()
        };

        if tok.kind == kind {
            Ok(tok)
        } else {
            Err(Error::unexpected(&tok, &kind.to_string()))
        }
    }

    fn ident(&mut self) -> Result<String, Error> {
        let Some(tok) = self.iter.consume() else {
            todo!()
        };

        match tok.kind {
            TokenKind::Identifier(ident) => Ok(ident),
            _ => Err(Error::unexpected(&tok, "identifier")),
        }
    }

    fn eof(&self) -> bool {
        matches!(self.iter.peek(), Some(tok) if tok.kind == TokenKind::Eof)
    }

    fn check(&self, kind: TokenKind) -> bool {
        matches!(self.iter.peek().unwrap(), tok if tok.kind == kind)
    }

    fn check_is_unop(&self) -> bool {
        match self.iter.peek().expect("should end in eof").kind {
            TokenKind::Minus | TokenKind::Not | TokenKind::Ampersand | TokenKind::Star => true,
            _ => false,
        }
    }
}

fn is_block_like(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Block(_) | ExprKind::If { .. } | ExprKind::While { .. }
    )
}

fn is_place_expr(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Identifier(_)
            | ExprKind::Subscript { .. }
            | ExprKind::Deref(_)
            | ExprKind::Field { .. }
    )
}

fn binary_precedence(kind: &TokenKind) -> Option<(i32, BinaryOperator)> {
    Some(match kind {
        TokenKind::Star => (50, BinaryOperator::Mul),
        TokenKind::Slash => (50, BinaryOperator::Div),
        TokenKind::Percent => (50, BinaryOperator::Mod),
        TokenKind::Plus => (45, BinaryOperator::Add),
        TokenKind::Minus => (45, BinaryOperator::Sub),
        TokenKind::ShiftLeft => (40, BinaryOperator::Shl),
        TokenKind::ShiftRight => (40, BinaryOperator::Shr),
        TokenKind::LessThan => (35, BinaryOperator::Lt),
        TokenKind::LessEqual => (35, BinaryOperator::Le),
        TokenKind::GreaterThan => (35, BinaryOperator::Gt),
        TokenKind::GreaterEqual => (35, BinaryOperator::Ge),
        TokenKind::EqualEqual => (30, BinaryOperator::Eq),
        TokenKind::NotEqual => (30, BinaryOperator::Ne),
        TokenKind::Ampersand => (25, BinaryOperator::BitAnd),
        TokenKind::Caret => (20, BinaryOperator::BitXor),
        TokenKind::Pipe => (15, BinaryOperator::BitOr),
        TokenKind::AmpAmp => (10, BinaryOperator::And),
        TokenKind::PipePipe => (5, BinaryOperator::Or),
        _ => return None,
    })
}
