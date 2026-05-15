/*
# Sample code:

let main = () -> Int {
    return 0;
}

This should turn into

program ::= { <declaration> }
declaration ::= "let" <ident> "=" <expr>

block ::= "{" { <block-item> } "}"
block-item ::=
    <statement>

statement ::=
    "return" <expr> ";"
    <expr>

<expr> ::=
    <const>
    | <function-literal>

<function-literal> ::= "(" ")" "->" <type> <block>

<type> ::=
    "Int"
    | <function-type>

<function-type> ::= "(" ")" "->" <type>
*/

use crate::{
    ast::{
        error::Error,
        token_iter::TokenIter,
        types::{
            Block, BlockItem, Const, Declaration, Expr, ExprKind, NodeIdGen, Program, Statement,
            StatementKind, TypeExpr, TypeExprKind,
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
        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::Minus)?;
        self.expect(TokenKind::GreaterThan)?;
        let return_ty = Box::new(self.parse_type()?);

        let span = left.join(return_ty.span);
        let kind = TypeExprKind::Function {
            params: Vec::new(),
            return_ty,
        };
        Ok(TypeExpr {
            id: self.id_gen.fresh(),
            span,
            kind,
        })
    }

    pub(super) fn parse_block_item(&mut self) -> Result<BlockItem, Error> {
        match &self.iter.peek().expect("lexer emits eof").kind {
            TokenKind::Return => {
                let start = self.expect(TokenKind::Return)?.span;
                let expr = self.parse_expr()?;
                let end = self.expect(TokenKind::Semicolon)?.span;
                Ok(BlockItem::Statement(Statement {
                    id: self.id_gen.fresh(),
                    span: start.join(end),
                    kind: StatementKind::Return(expr),
                }))
            }
            TokenKind::Let => {
                let decl = self.parse_declaration()?;
                Ok(BlockItem::Declaration(decl))
            }
            _ => {
                let expr = self.parse_expr()?;
                Ok(BlockItem::Statement(Statement {
                    id: self.id_gen.fresh(),
                    span: expr.span,
                    kind: StatementKind::Expr(expr),
                }))
            }
        }
    }

    pub(super) fn parse_block(&mut self) -> Result<Block, Error> {
        let left = self.expect(TokenKind::LeftBrace)?.span;
        let mut items = Vec::new();

        while !matches!(self.iter.peek(), Some(tok) if tok.kind == TokenKind::RightBrace) {
            items.push(self.parse_block_item()?);
        }

        let right = self.expect(TokenKind::RightBrace)?.span;

        let span = left.join(right);
        Ok(Block {
            id: self.id_gen.fresh(),
            span,
            items,
        })
    }

    pub(super) fn parse_expr(&mut self) -> Result<Expr, Error> {
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
            TokenKind::LeftParen => {
                let left = self.expect(TokenKind::LeftParen)?;
                self.expect(TokenKind::RightParen)?;
                self.expect(TokenKind::Minus)?;
                self.expect(TokenKind::GreaterThan)?;
                let return_ty = self.parse_type()?;
                let body = self.parse_block()?;

                let span = left.span.join(body.span);
                let kind = ExprKind::Function {
                    params: Vec::new(),
                    return_ty,
                    body,
                };

                Ok(Expr {
                    id: self.id_gen.fresh(),
                    span,
                    kind,
                })
            }
            _ => Err(Error::unexpected(tok, "expression")),
        }
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
}
