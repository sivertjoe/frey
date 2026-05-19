/*
# Sample code:

let main = () -> Int {
    return 0;
}

This should turn into

program ::= { <declaration> }
declaration ::= "let" <ident> "=" <expr>

block ::= "{" { <block-item> } [<expr>] "}"
block-item ::=
    <declaration>
    | <statement>

statement ::=
    "return" <expr> ";"
    | <expr> ";"

<expr> ::=
    <const>
    | <ident>
    | <function-literal>
    | <identifier> "(" [ <expr> [ { "," <expr> } ] ] ")"
    | <unary-op> <expr>
    | if <expr> <block> [ "else" <expr> ]

<function-literal> ::= "(" [<params>] ")" [ "->" <type> ] <block>
<params> ::= <param> { "," <param> }
<param>  ::= <ident> ":" <type>

<type> ::=
    "Int"
    | <function-type>

<function-type> ::= "(" [<type-list>] ")" "->" <type>
<type-list> ::= <type> { "," <type> }
*/

use crate::{
    ast::{
        UnaryOperator,
        error::Error,
        token_iter::TokenIter,
        types::{
            BinaryOperator, Block, BlockItem, Const, Declaration, Expr, ExprKind, NodeIdGen, Param,
            Program, Statement, StatementKind, TypeExpr, TypeExprKind,
        },
    },
    lexer::types::{Literal, Token, TokenKind},
};

pub(super) struct Parser {
    iter: TokenIter,
    id_gen: NodeIdGen,
}

impl Parser {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        Self {
            iter: TokenIter::new(tokens),
            id_gen: NodeIdGen::new(),
        }
    }

    pub(super) fn parse(&mut self) -> Result<Program, Error> {
        self.parse_program()
    }

    pub(super) fn parse_declaration(&mut self) -> Result<Declaration, Error> {
        let start = self.expect(TokenKind::Let)?.span;
        let name = self.ident()?;
        self.expect(TokenKind::Equal)?;
        let expr = self.parse_expr()?;
        self.expect(TokenKind::Semicolon)?;

        Ok(Declaration {
            id: self.id_gen.fresh(),
            span: start.join(expr.span),
            name,
            value: expr,
        })
    }

    pub(super) fn parse_program(&mut self) -> Result<Program, Error> {
        let mut decls = Vec::new();

        while !self.eof() {
            decls.push(self.parse_declaration()?);
        }

        if decls.is_empty() {
            let eof = self.iter.peek().expect("lexer emits eof");
            return Err(Error::unexpected(eof, "declaration"));
        }

        let span = decls.first().unwrap().span.join(decls.last().unwrap().span);
        Ok(Program {
            span,
            declarations: decls,
        })
    }

    pub(super) fn parse_type(&mut self) -> Result<TypeExpr, Error> {
        let tok = self.iter.peek().expect("lexer always emits Eof");
        match &tok.kind {
            TokenKind::Int => {
                let span = self.iter.consume().unwrap().span;
                Ok(TypeExpr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: TypeExprKind::Int,
                })
            }
            TokenKind::LeftParen => self.parse_function_type(),
            _ => Err(Error::unexpected(tok, "type")),
        }
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
                TokenKind::Let => {
                    let decl = self.parse_declaration()?;
                    items.push(BlockItem::Declaration(decl));
                }
                TokenKind::Return => {
                    let start = self.expect(TokenKind::Return)?.span;
                    let expr = self.parse_expr()?;
                    let end = self.expect(TokenKind::Semicolon)?.span;
                    items.push(BlockItem::Statement(Statement {
                        id: self.id_gen.fresh(),
                        span: start.join(end),
                        kind: StatementKind::Return(expr),
                    }));
                }
                _ => {
                    // Bare expression: either followed by `;` (statement)
                    // or `}` (this is the block's tail expression).
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
        self.parse_binary_expr(0)
    }

    fn parse_binary_expr(&mut self, min_prec: i32) -> Result<Expr, Error> {
        let mut lhs = self.parse_unary()?;

        loop {
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

    fn parse_unary(&mut self) -> Result<Expr, Error> {
        if !self.check_is_unop() {
            return self.parse_postfix();
        }
        let tok = self.iter.consume().unwrap();
        let op = match tok.kind {
            TokenKind::Not => UnaryOperator::Not,
            TokenKind::Minus => UnaryOperator::Minus,
            _ => unreachable!(),
        };
        let start = tok.span;
        let operand = self.parse_unary()?;
        let span = start.join(operand.span);
        Ok(Expr {
            id: self.id_gen.fresh(),
            span,
            kind: ExprKind::Unary {
                op,
                expr: Box::new(operand),
            },
        })
    }

    fn parse_postfix(&mut self) -> Result<Expr, Error> {
        let mut e = self.parse_primary()?;
        while self.check(TokenKind::LeftParen) {
            e = self.parse_call_suffix(e)?;
        }
        Ok(e)
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
            TokenKind::Identifier(_) => {
                let span = tok.span;
                let name = self.ident()?;
                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind: ExprKind::Identifier(name),
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
                    let left = self.expect(TokenKind::LeftParen)?;

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

                    let span = left.span.join(body.span);
                    Ok(Expr {
                        id: self.id_gen.fresh(),
                        span,
                        kind: ExprKind::Function {
                            params,
                            return_ty,
                            body,
                        },
                    })
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
            _ => Err(Error::unexpected(tok, "expression")),
        }
    }

    fn parse_call_suffix(&mut self, callee: Expr) -> Result<Expr, Error> {
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
                args,
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
            TokenKind::Minus | TokenKind::Not => true,
            _ => false,
        }
    }
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
