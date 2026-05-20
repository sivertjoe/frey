use inkwell::AddressSpace;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType};

use crate::codegen::Codegen;
use crate::hir::types::{Param, Ty};

impl<'ctx> Codegen<'ctx> {
    pub fn lower_ty(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        match ty {
            Ty::Unit => self.context.bool_type().into(),
            Ty::Int => self.context.i32_type().into(),
            Ty::Float => self.context.f32_type().into(),
            Ty::Function { .. } => self.context.ptr_type(AddressSpace::default()).into(),
        }
    }

    pub fn lower_fn_type(&self, params: &[Param], return_ty: &Ty) -> FunctionType<'ctx> {
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|p| self.lower_ty(&p.ty).into()).collect();
        self.fn_type_with_return(&param_types, return_ty)
    }

    pub fn fn_type_for_function_ty(&self, fn_ty: &Ty) -> FunctionType<'ctx> {
        let Ty::Function { params, return_ty } = fn_ty else {
            panic!("expected function type, got {fn_ty:?}");
        };
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|p| self.lower_ty(p).into()).collect();
        self.fn_type_with_return(&param_types, return_ty)
    }

    fn fn_type_with_return(
        &self,
        param_types: &[BasicMetadataTypeEnum<'ctx>],
        return_ty: &Ty,
    ) -> FunctionType<'ctx> {
        match return_ty {
            Ty::Unit => self.context.bool_type().fn_type(param_types, false),
            Ty::Int => self.context.i32_type().fn_type(param_types, false),
            Ty::Float => self.context.f32_type().fn_type(param_types, false),
            Ty::Function { .. } => self
                .context
                .ptr_type(AddressSpace::default())
                .fn_type(param_types, false),
        }
    }
}
