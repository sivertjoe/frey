mod error;
mod expr;
mod function;
mod ty;

use std::collections::HashMap;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::{FunctionValue, PointerValue};

use crate::hir::types::{LocalId, Program};

pub use error::Error;

pub struct Codegen<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    functions: HashMap<LocalId, FunctionValue<'ctx>>,
}

impl<'ctx> Codegen<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        Self {
            context,
            module,
            builder: context.create_builder(),
            locals: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub fn lower(&mut self, program: Program) -> Result<(), Error> {
        for decl in &program.declarations {
            self.declare_top_level(decl);
        }
        for decl in program.declarations {
            self.lower_top_level(decl)?;
        }
        Ok(())
    }

    pub fn module_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }
}
