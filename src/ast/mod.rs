use crate::{
    ast::{error::Error, parser::Parser, types::Program},
    lexer::types::Token,
};

pub mod error;
mod parser;
mod token_iter;
pub mod ty;
pub mod types;

pub fn parse(tokens: Vec<Token>) -> Result<Program, Error> {
    Parser::new(tokens).parse()
}

#[cfg(test)]
mod tests;
