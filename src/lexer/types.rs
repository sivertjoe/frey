#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub offset: usize,
    pub line: usize,   // 1-based
    pub column: usize, // 1-based
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,

    Let,

    Int,
    Identifier(String),

    Equal,
    Minus,
    GreaterThan,

    Literal(Literal),

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i32),
}
