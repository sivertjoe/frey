mod error;
mod expr;
mod function;
mod ty;

use std::collections::HashMap;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::StructType;
use inkwell::values::{FunctionValue, PointerValue};

use crate::hir::types::{LocalId, Program, StructDef};

pub use error::Error;

pub struct Codegen<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    functions: HashMap<LocalId, FunctionValue<'ctx>>,
    pub(crate) struct_defs: HashMap<String, StructDef>,
    pub(crate) struct_llvm: HashMap<String, StructType<'ctx>>,
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
            struct_defs: HashMap::new(),
            struct_llvm: HashMap::new(),
        }
    }

    pub fn lower(&mut self, program: Program) -> Result<(), Error> {
        // Two-step struct setup so self-referential pointers work: first
        // declare every struct as an opaque LLVM type, then fill in bodies
        // referencing those opaque types.
        for (name, def) in &program.structs {
            let llvm_ty = self.context.opaque_struct_type(name);
            self.struct_llvm.insert(name.clone(), llvm_ty);
            self.struct_defs.insert(name.clone(), def.clone());
        }
        for (name, def) in &program.structs {
            let body: Vec<_> = def
                .fields
                .iter()
                .map(|(_, ty)| self.lower_ty(ty))
                .collect();
            let llvm_ty = self.struct_llvm[name];
            llvm_ty.set_body(&body, false);
        }

        for decl in &program.declarations {
            self.declare_top_level(decl);
        }
        for decl in program.declarations {
            self.lower_top_level(decl)?;
        }
        Ok(())
    }

    pub fn write_ir_to_file(&self, path: &std::path::Path) -> Result<(), Error> {
        self.module
            .print_to_file(path)
            .map_err(|e| Error::IrWrite(e.to_string()))
    }
}
