use crate::codegen::{Codegen, Error};
use crate::hir::types::{Block, BlockItem, Declaration, ExprKind};

impl<'ctx> Codegen<'ctx> {
    pub fn declare_top_level(&mut self, decl: &Declaration) {
        if let ExprKind::Function(func) = &decl.value.kind {
            let fn_type = self.lower_fn_type(&func.params, &func.return_ty);
            let llvm_fn = self.module.add_function(&decl.name, fn_type, None);
            self.functions.insert(decl.id, llvm_fn);
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
        for item in block.items {
            match item {
                BlockItem::Declaration(d) => self.lower_local_decl(d)?,
                BlockItem::Statement(s) => self.lower_statement(s)?,
            }
        }
        if let Some(tail) = block.tail {
            let value = self.lower_expr(*tail)?;
            self.builder.build_return(Some(&value))?;
        }
        Ok(())
    }

    fn lower_local_decl(&mut self, decl: Declaration) -> Result<(), Error> {
        let value = self.lower_expr(decl.value)?;
        let llvm_ty = self.lower_ty(&decl.ty);
        let slot = self.builder.build_alloca(llvm_ty, &decl.name)?;
        self.builder.build_store(slot, value)?;
        self.locals.insert(decl.id, slot);
        Ok(())
    }
}
