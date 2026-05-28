use crate::{
    ast::{error::Error, parser::Parser},
    lexer::types::Token,
};

pub use types::*;

pub mod error;
mod parser;
mod token_iter;
pub mod types;

/// Parses with node ids starting at `node_base`, returning the program and the
/// next free id. The module loader chains these so ids stay unique across the
/// files it merges.
pub fn parse_at(tokens: Vec<Token>, node_base: u32) -> Result<(Program, u32), Error> {
    let mut parser = Parser::new_at(tokens, node_base);
    let program = parser.parse()?;
    Ok((program, parser.node_id_count()))
}

#[cfg(test)]
mod tests;
