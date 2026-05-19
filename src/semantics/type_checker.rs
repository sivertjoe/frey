use crate::hir;

#[derive(Debug, Clone)]
pub enum Error {}

pub fn type_check(program: &hir::Program) -> Result<(), Error> {
    Typechecker::new().typecheck_program(program)
}

struct Typechecker {}

impl Typechecker {
    fn new() -> Self {
        Self {}
    }
    fn typecheck_program(&self, program: &hir::Program) -> Result<(), Error> {
        Ok(())
    }
}
