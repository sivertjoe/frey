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
            Ty::Array { element, count } => self.lower_ty(element).array_type(*count as u32).into(),
            Ty::Struct(name) => self.struct_llvm[name].into(),
            Ty::Tuple(elems) => self.tuple_llvm_type(elems).into(),
            Ty::Enum(name) => self.enum_llvm[name].into(),

            Ty::TypeVar(_) | Ty::GenericStruct { .. } | Ty::GenericEnum { .. } => {
                unreachable!(
                    "specialization should have eliminated TypeVars, GenericStructs, and GenericEnums"
                )
            }
        }
    }

    /// Conservative byte-size upper bound (no alignment padding), used only
    /// to size the enum payload buffer.
    pub fn approx_size_bytes(&self, ty: &Ty) -> usize {
        match ty {
            Ty::Unit => 1,
            Ty::I8 | Ty::U8 => 1,
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 | Ty::Float | Ty::F32 => 4,
            Ty::I64 | Ty::U64 | Ty::F64 => 8,
            Ty::Ptr(_) | Ty::Function { .. } => 8,
            Ty::Array { element, count } => self.approx_size_bytes(element) * count,
            Ty::Tuple(elems) => elems.iter().map(|e| self.approx_size_bytes(e)).sum(),
            Ty::Struct(name) => {
                let def = self
                    .struct_defs
                    .get(name)
                    .expect("struct def registered before sizing");
                def.fields
                    .iter()
                    .map(|(_, t)| self.approx_size_bytes(t))
                    .sum::<usize>()
                    .max(1)
            }
            Ty::Enum(name) => {
                let def = self
                    .enum_defs
                    .get(name)
                    .expect("enum def registered before sizing");
                let payload = def
                    .variants
                    .iter()
                    .map(|v| v.fields.iter().map(|t| self.approx_size_bytes(t)).sum())
                    .max()
                    .unwrap_or(0);
                4 + payload
            }
            Ty::TypeVar(_) | Ty::GenericStruct { .. } | Ty::GenericEnum { .. } => {
                unreachable!("approx_size_bytes called on unspecialized type")
            }
        }
    }

    /// Structural LLVM type for a Frey tuple — same element types share
    /// the same anonymous struct, no name needed.
    pub fn tuple_llvm_type(&self, elems: &[Ty]) -> inkwell::types::StructType<'ctx> {
        let field_types: Vec<BasicTypeEnum<'ctx>> =
            elems.iter().map(|e| self.lower_ty(e)).collect();
        self.context.struct_type(&field_types, false)
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
            Ty::Struct(name) => self.struct_llvm[name].fn_type(param_types, false),
            Ty::Tuple(elems) => self.tuple_llvm_type(elems).fn_type(param_types, false),
            Ty::Enum(name) => self.enum_llvm[name].fn_type(param_types, false),
            Ty::TypeVar(_) | Ty::GenericStruct { .. } | Ty::GenericEnum { .. } => {
                unreachable!(
                    "specialization should have eliminated TypeVars, GenericStructs, and GenericEnums"
                )
            }
        }
    }
}
