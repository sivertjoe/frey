use crate::lexer::types::Token;
use std::collections::VecDeque;

pub struct TokenIter {
    tokens: VecDeque<Token>,
}

impl TokenIter {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens: tokens.into(),
        }
    }

    pub fn peek(&self) -> Option<&Token> {
        self.tokens.front()
    }

    pub fn peek_nth(&self, n: usize) -> Option<&Token> {
        self.tokens.get(n)
    }

    pub fn consume(&mut self) -> Option<Token> {
        self.tokens.pop_front()
    }
}
