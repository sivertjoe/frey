use crate::{
    ast::{error::Error, parser::Parser},
    lexer::types::Token,
};

pub use types::*;

pub mod error;
mod parser;
mod token_iter;
pub mod types;

pub fn parse(tokens: Vec<Token>) -> Result<Program, Error> {
    Parser::new(tokens).parse()
}

#[cfg(test)]
mod tests;
