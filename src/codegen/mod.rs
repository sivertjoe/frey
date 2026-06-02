mod error;
mod expr;
mod function;
mod ty;

use std::collections::HashMap;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::StructType;
use inkwell::values::{FunctionValue, PointerValue};

use crate::hir::types::{EnumDef, Expr, LocalId, Program, StructDef};

pub use error::Error;

pub struct Codegen<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    locals: HashMap<LocalId, PointerValue<'ctx>>,
    pub(crate) functions: HashMap<LocalId, FunctionValue<'ctx>>,
    /// Function ids whose LLVM signature does NOT include the implicit
    /// `env: *u8` first param — extern C ABI functions only. Every other
    /// Frey function carries env as its first LLVM param so that any fn
    /// value can flow into a `Ty::Closure` slot without a trampoline.
    pub(crate) extern_fn_ids: std::collections::HashSet<LocalId>,
    pub(crate) struct_defs: HashMap<String, StructDef>,
    pub(crate) struct_llvm: HashMap<String, StructType<'ctx>>,
    pub(crate) enum_defs: HashMap<String, EnumDef>,
    /// `{ i32 tag, [N x i8] payload }`, N sized for the largest variant.
    pub(crate) enum_llvm: HashMap<String, StructType<'ctx>>,
    pub(crate) string_constants: HashMap<String, PointerValue<'ctx>>,
    pub(crate) loop_exit_stack: Vec<BasicBlock<'ctx>>,
    /// One list of pending `defer` expressions per active block scope (LIFO).
    pub(crate) defer_scopes: Vec<Vec<Expr>>,
    /// `defer_scopes` length at each enclosing loop's body, so `break` runs
    /// only the defers registered inside the loop.
    pub(crate) loop_defer_boundary: Vec<usize>,
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
            extern_fn_ids: std::collections::HashSet::new(),
            struct_defs: HashMap::new(),
            struct_llvm: HashMap::new(),
            enum_defs: HashMap::new(),
            enum_llvm: HashMap::new(),
            string_constants: HashMap::new(),
            loop_exit_stack: Vec::new(),
            defer_scopes: Vec::new(),
            loop_defer_boundary: Vec::new(),
        }
    }

    pub(crate) fn string_global_for(&mut self, s: &str) -> PointerValue<'ctx> {
        if let Some(&p) = self.string_constants.get(s) {
            return p;
        }
        let const_array = self.context.const_string(s.as_bytes(), true);
        let name = format!(".str.{}", self.string_constants.len());
        let global = self.module.add_global(const_array.get_type(), None, &name);
        global.set_initializer(&const_array);
        global.set_constant(true);
        global.set_linkage(inkwell::module::Linkage::Private);
        let ptr = global.as_pointer_value();
        self.string_constants.insert(s.to_string(), ptr);
        ptr
    }

    pub fn lower(&mut self, program: Program) -> Result<(), Error> {
        // Opaque-first so a struct can hold a pointer to itself. Enums get
        // pre-registered too so a struct field can be Ty::Enum and vice versa.
        for (name, def) in &program.structs {
            let llvm_ty = self.context.opaque_struct_type(name);
            self.struct_llvm.insert(name.clone(), llvm_ty);
            self.struct_defs.insert(name.clone(), def.clone());
        }
        for (name, def) in &program.enums {
            self.enum_defs.insert(name.clone(), def.clone());
        }
        for (name, def) in &program.enums {
            let payload_bytes = def
                .variants
                .iter()
                .map(|v| self.aligned_fields_size_pub(&v.fields))
                .max()
                .unwrap_or(0)
                .max(1);
            let payload_ty = self
                .context
                .i8_type()
                .array_type(payload_bytes as u32);
            let tag_ty = self.context.i32_type();
            let llvm_ty = self.context.struct_type(
                &[tag_ty.into(), payload_ty.into()],
                false,
            );
            self.enum_llvm.insert(name.clone(), llvm_ty);
        }
        for (name, def) in &program.structs {
            let body: Vec<_> = def.fields.iter().map(|(_, ty)| self.lower_ty(ty)).collect();
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
