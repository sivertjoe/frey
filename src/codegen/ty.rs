use inkwell::AddressSpace;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType};

use crate::codegen::Codegen;
use crate::hir::types::{Param, Ty};

impl<'ctx> Codegen<'ctx> {
    pub fn lower_ty(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        match ty {
            Ty::Int => self.context.i32_type().into(),
            Ty::Function { .. } => self
                .context
                .ptr_type(AddressSpace::default())
                .into(),
        }
    }

    pub fn lower_fn_type(&self, params: &[Param], return_ty: &Ty) -> FunctionType<'ctx> {
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|p| self.lower_ty(&p.ty).into()).collect();

        match return_ty {
            Ty::Int => self.context.i32_type().fn_type(&param_types, false),
            Ty::Function { .. } => self
                .context
                .ptr_type(AddressSpace::default())
                .fn_type(&param_types, false),
        }
    }
}
