use inkwell::AddressSpace;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType};

use crate::codegen::Codegen;
use crate::hir::types::{Param, Ty};

impl<'ctx> Codegen<'ctx> {
    pub fn lower_ty(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        match ty {
            Ty::Unit => self.context.bool_type().into(),
            // LLVM has no signedness in its types; that only matters per-op.
            Ty::I8 | Ty::U8 => self.context.i8_type().into(),
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 => self.context.i32_type().into(),
            Ty::I64 | Ty::U64 => self.context.i64_type().into(),
            Ty::Float | Ty::F32 => self.context.f32_type().into(),
            Ty::F64 => self.context.f64_type().into(),
            Ty::Function { .. } | Ty::Ptr(_) => {
                self.context.ptr_type(AddressSpace::default()).into()
            }
            Ty::Array { element, count } => {
                self.lower_ty(element).array_type(*count as u32).into()
            }
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
            Ty::I8 | Ty::U8 => self.context.i8_type().fn_type(param_types, false),
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 => {
                self.context.i32_type().fn_type(param_types, false)
            }
            Ty::I64 | Ty::U64 => self.context.i64_type().fn_type(param_types, false),
            Ty::Float | Ty::F32 => self.context.f32_type().fn_type(param_types, false),
            Ty::F64 => self.context.f64_type().fn_type(param_types, false),
            Ty::Function { .. } | Ty::Ptr(_) => self
                .context
                .ptr_type(AddressSpace::default())
                .fn_type(param_types, false),
            Ty::Array { element, count } => self
                .lower_ty(element)
                .array_type(*count as u32)
                .fn_type(param_types, false),
        }
    }
}
