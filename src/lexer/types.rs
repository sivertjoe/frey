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
    LeftBracket,
    RightBracket,

    Let,
    Mut,
    Return,
    As,
    If,
    Else,
    Struct,

    Int,
    UInt,
    Float,
    I8,
    I32,
    I64,
    U8,
    U32,
    U64,
    F32,
    F64,
    Identifier(String),

    Equal,
    Minus,
    Plus,
    Star,
    Slash,
    Percent,
    LessThan,
    GreaterThan,
    LessEqual,
    GreaterEqual,
    EqualEqual,
    NotEqual,
    ShiftLeft,
    ShiftRight,
    Ampersand,
    Pipe,
    Caret,
    AmpAmp,
    PipePipe,
    PipeArrow,
    Not,

    Literal(Literal),

    Semicolon,
    Comma,
    Colon,
    Dot,

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i32),
    Float(f32),
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKind::LeftBrace => write!(f, "`{{`"),
            TokenKind::RightBrace => write!(f, "`}}`"),
            TokenKind::LeftParen => write!(f, "`(`"),
            TokenKind::RightParen => write!(f, "`)`"),
            TokenKind::LeftBracket => write!(f, "`[`"),
            TokenKind::RightBracket => write!(f, "`]`"),
            TokenKind::Let => write!(f, "`let`"),
            TokenKind::Mut => write!(f, "`mut`"),
            TokenKind::Int => write!(f, "`Int`"),
            TokenKind::UInt => write!(f, "`UInt`"),
            TokenKind::Float => write!(f, "`Float`"),
            TokenKind::I8 => write!(f, "`i8`"),
            TokenKind::I32 => write!(f, "`i32`"),
            TokenKind::I64 => write!(f, "`i64`"),
            TokenKind::U8 => write!(f, "`u8`"),
            TokenKind::U32 => write!(f, "`u32`"),
            TokenKind::U64 => write!(f, "`u64`"),
            TokenKind::F32 => write!(f, "`f32`"),
            TokenKind::F64 => write!(f, "`f64`"),
            TokenKind::Identifier(name) => write!(f, "`{name}`"),
            TokenKind::Equal => write!(f, "`=`"),
            TokenKind::Minus => write!(f, "`-`"),
            TokenKind::Plus => write!(f, "`+`"),
            TokenKind::Star => write!(f, "`*`"),
            TokenKind::Slash => write!(f, "`/`"),
            TokenKind::Percent => write!(f, "`%`"),
            TokenKind::LessThan => write!(f, "`<`"),
            TokenKind::GreaterThan => write!(f, "`>`"),
            TokenKind::LessEqual => write!(f, "`<=`"),
            TokenKind::GreaterEqual => write!(f, "`>=`"),
            TokenKind::EqualEqual => write!(f, "`==`"),
            TokenKind::NotEqual => write!(f, "`!=`"),
            TokenKind::ShiftLeft => write!(f, "`<<`"),
            TokenKind::ShiftRight => write!(f, "`>>`"),
            TokenKind::Ampersand => write!(f, "`&`"),
            TokenKind::Pipe => write!(f, "`|`"),
            TokenKind::Caret => write!(f, "`^`"),
            TokenKind::AmpAmp => write!(f, "`&&`"),
            TokenKind::PipePipe => write!(f, "`||`"),
            TokenKind::PipeArrow => write!(f, "`|>`"),
            TokenKind::Not => write!(f, "`!`"),
            TokenKind::Literal(lit) => write!(f, "{lit}"),
            TokenKind::Semicolon => write!(f, "`;`"),
            TokenKind::Comma => write!(f, "`,`"),
            TokenKind::Colon => write!(f, "`:`"),
            TokenKind::Dot => write!(f, "`.`"),
            TokenKind::Return => write!(f, "`return`"),
            TokenKind::If => write!(f, "`if`"),
            TokenKind::Else => write!(f, "`else`"),
            TokenKind::As => write!(f, "`as`"),
            TokenKind::Struct => write!(f, "`struct`"),

            TokenKind::Eof => write!(f, "end of input"),
        }
    }
}

impl std::fmt::Display for Literal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Literal::Int(n) => write!(f, "`{n}`"),
            Literal::Float(fl) => write!(f, "`{fl}`"),
        }
    }
}
