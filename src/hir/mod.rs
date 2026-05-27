mod coerce;
pub mod comptime;
pub mod error;
mod generics;
mod lower;
pub mod types;

pub use error::Error;
pub use types::*;

pub fn lower(p: crate::ast::Program) -> Result<Program, Error> {
    lower::Lower::new().lower_program(p)
}
