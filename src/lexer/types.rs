#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    pub fn join(self, end: Span) -> Span {
        Span {
            start: self.start,
            end: end.end,
        }
    }
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
    Return,

    Int,
    Identifier(String),

    Equal,
    Minus,
    GreaterThan,
    Not,

    Literal(Literal),

    Semicolon,
    Comma,
    Colon,

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i32),
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKind::LeftBrace => write!(f, "`{{`"),
            TokenKind::RightBrace => write!(f, "`}}`"),
            TokenKind::LeftParen => write!(f, "`(`"),
            TokenKind::RightParen => write!(f, "`)`"),
            TokenKind::Let => write!(f, "`let`"),
            TokenKind::Int => write!(f, "`Int`"),
            TokenKind::Identifier(name) => write!(f, "`{name}`"),
            TokenKind::Equal => write!(f, "`=`"),
            TokenKind::Minus => write!(f, "`-`"),
            TokenKind::Not => write!(f, "`!`"),
            TokenKind::GreaterThan => write!(f, "`>`"),
            TokenKind::Literal(lit) => write!(f, "{lit}"),
            TokenKind::Semicolon => write!(f, "`;`"),
            TokenKind::Comma => write!(f, "`,`"),
            TokenKind::Colon => write!(f, "`:`"),
            TokenKind::Return => write!(f, "`return`"),

            TokenKind::Eof => write!(f, "end of input"),
        }
    }
}

impl std::fmt::Display for Literal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Literal::Int(n) => write!(f, "`{n}`"),
        }
    }
}
