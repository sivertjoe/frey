use inkwell::types::{ArrayType, BasicTypeEnum};
use inkwell::values::{ArrayValue, BasicValueEnum};

use crate::codegen::{Codegen, Error};
use crate::hir::types::{Block, BlockItem, Const, Declaration, Expr, ExprKind};

impl<'ctx> Codegen<'ctx> {
    pub fn declare_top_level(&mut self, decl: &Declaration) {
        match &decl.value.kind {
            ExprKind::Function(func) => {
                let fn_type = self.lower_fn_type(&func.params, &func.return_ty);
                let llvm_fn = self.module.add_function(&decl.name, fn_type, None);
                self.functions.insert(decl.id, llvm_fn);
            }
            // Top-level non-function declarations become LLVM globals.
            // The initializer must be a constant literal — complex expressions
            // would need a static-init function (LLVM `@llvm.global_ctors`).
            _ => {
                let llvm_ty = self.lower_ty(&decl.ty);
                let global = self.module.add_global(llvm_ty, None, &decl.name);
                let init = self
                    .const_initializer(&decl.value)
                    .expect("global initializer must be a constant literal");
                global.set_initializer(&init);
                // Immutable globals get marked `constant` so LLVM can place
                // them in `.rodata` and inline reads aggressively.
                if !decl.mutable {
                    global.set_constant(true);
                }
                // Store the global's pointer in `locals` so `Local(id)` reads
                // and `Assign { target: id }` writes go through the same code
                // path as function locals.
                self.locals.insert(decl.id, global.as_pointer_value());
            }
        }
    }

    fn const_initializer(&mut self, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        match &expr.kind {
            ExprKind::Const(Const::Int(n)) => {
                Some(self.context.i32_type().const_int(*n as u64, true).into())
            }
            ExprKind::Const(Const::Float(f)) => {
                Some(self.context.f32_type().const_float(*f as f64).into())
            }
            ExprKind::Const(Const::Unit) => Some(self.context.bool_type().const_zero().into()),
            ExprKind::Const(Const::Str(s)) => {
                let s = s.clone();
                Some(self.string_global_for(&s).into())
            }
            ExprKind::Array(items) => {
                // An array literal whose every element is itself a constant
                // can become a constant LLVM aggregate.
                let elem_vals: Option<Vec<BasicValueEnum<'ctx>>> =
                    items.iter().map(|it| self.const_initializer(it)).collect();
                let elem_vals = elem_vals?;
                let arr_llvm_ty = self.lower_ty(&expr.ty).into_array_type();
                Some(const_array_from(arr_llvm_ty, &elem_vals).into())
            }
            _ => None,
        }
    }

    pub fn lower_top_level(&mut self, decl: Declaration) -> Result<(), Error> {
        let ExprKind::Function(func) = decl.value.kind else {
            return Ok(());
        };

        let llvm_fn = *self
            .functions
            .get(&decl.id)
            .expect("function declared in pass 1");

        let entry = self.context.append_basic_block(llvm_fn, "entry");
        self.builder.position_at_end(entry);

        for (i, param) in func.params.iter().enumerate() {
            let llvm_ty = self.lower_ty(&param.ty);
            let slot = self.builder.build_alloca(llvm_ty, &param.name)?;
            let arg = llvm_fn.get_nth_param(i as u32).unwrap();
            self.builder.build_store(slot, arg)?;
            self.locals.insert(param.id, slot);
        }

        self.lower_function_body(func.body)?;
        Ok(())
    }

    fn lower_function_body(&mut self, block: Block) -> Result<(), Error> {
        self.defer_scopes.push(Vec::new());
        for item in block.items {
            match item {
                BlockItem::Declaration(d) => self.lower_local_decl(d)?,
                BlockItem::Statement(s) => self.lower_statement(s)?,
            }
        }
        // The body may already be terminated by a `return` statement (which
        // ran the defers itself); otherwise emit the implicit tail-return,
        // running the function-level defers just before it.
        if !self.current_block_terminated() {
            let value = self.lower_expr(*block.tail)?;
            self.run_top_defer_scope()?;
            self.builder.build_return(Some(&value))?;
        }
        self.defer_scopes.pop();
        Ok(())
    }

    pub(super) fn current_block_terminated(&self) -> bool {
        self.builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
    }

    pub(super) fn lower_local_decl(&mut self, decl: Declaration) -> Result<(), Error> {
        let value = self.lower_expr(decl.value)?;
        let llvm_ty = self.lower_ty(&decl.ty);
        let slot = self.builder.build_alloca(llvm_ty, &decl.name)?;
        self.builder.build_store(slot, value)?;
        self.locals.insert(decl.id, slot);
        Ok(())
    }
}

fn const_array_from<'ctx>(
    arr_ty: ArrayType<'ctx>,
    elems: &[BasicValueEnum<'ctx>],
) -> ArrayValue<'ctx> {
    match arr_ty.get_element_type() {
        BasicTypeEnum::IntType(t) => {
            let vs: Vec<_> = elems.iter().map(|v| v.into_int_value()).collect();
            t.const_array(&vs)
        }
        BasicTypeEnum::FloatType(t) => {
            let vs: Vec<_> = elems.iter().map(|v| v.into_float_value()).collect();
            t.const_array(&vs)
        }
        BasicTypeEnum::ArrayType(t) => {
            let vs: Vec<_> = elems.iter().map(|v| v.into_array_value()).collect();
            t.const_array(&vs)
        }
        _ => unreachable!("unsupported const array element type"),
    }
}
