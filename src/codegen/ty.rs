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
            // Closure value: `{env: ptr, code: ptr}` (16 bytes). Calling
            // does `code(env, args)`. The code pointer's underlying fn
            // always takes env as its first parameter.
            Ty::Closure { .. } => self.closure_llvm_type().into(),
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

    /// LLVM struct layout for a closure value, shared by every closure
    /// regardless of signature: `{env: ptr, code: ptr}`. The `code` field's
    /// actual fn type is recovered from `Ty::Closure { params, return_ty }`
    /// at the call site (env is prepended).
    pub fn closure_llvm_type(&self) -> inkwell::types::StructType<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.context.struct_type(&[ptr.into(), ptr.into()], false)
    }

    /// Natural alignment of a type, matching LLVM's default layout rules.
    pub fn align_bytes(&self, ty: &Ty) -> usize {
        match ty {
            Ty::Unit => 1,
            Ty::I8 | Ty::U8 => 1,
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 | Ty::Float | Ty::F32 => 4,
            Ty::I64 | Ty::U64 | Ty::F64 => 8,
            Ty::Ptr(_) | Ty::Function { .. } => 8,
            Ty::Closure { .. } => 8,
            Ty::Array { element, .. } => self.align_bytes(element),
            Ty::Tuple(elems) => elems.iter().map(|e| self.align_bytes(e)).max().unwrap_or(1),
            Ty::Struct(name) => {
                let def = self
                    .struct_defs
                    .get(name)
                    .expect("struct def registered before sizing");
                def.fields
                    .iter()
                    .map(|(_, t)| self.align_bytes(t))
                    .max()
                    .unwrap_or(1)
            }
            Ty::Enum(_) => 4,
            Ty::TypeVar(_) | Ty::GenericStruct { .. } | Ty::GenericEnum { .. } => {
                unreachable!("align_bytes called on unspecialized type")
            }
        }
    }

    /// Aligned byte size including trailing padding, matching LLVM's default
    /// struct layout. Used to size the enum payload buffer correctly.
    pub fn approx_size_bytes(&self, ty: &Ty) -> usize {
        match ty {
            Ty::Unit => 1,
            Ty::I8 | Ty::U8 => 1,
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 | Ty::Float | Ty::F32 => 4,
            Ty::I64 | Ty::U64 | Ty::F64 => 8,
            Ty::Ptr(_) | Ty::Function { .. } => 8,
            // Closure: env + code = 16 bytes.
            Ty::Closure { .. } => 16,
            Ty::Array { element, count } => self.approx_size_bytes(element) * count,
            Ty::Tuple(elems) => self.aligned_fields_size(elems),
            Ty::Struct(name) => {
                let def = self
                    .struct_defs
                    .get(name)
                    .expect("struct def registered before sizing");
                let field_tys: Vec<Ty> = def.fields.iter().map(|(_, t)| t.clone()).collect();
                self.aligned_fields_size(&field_tys).max(1)
            }
            Ty::Enum(name) => {
                let def = self
                    .enum_defs
                    .get(name)
                    .expect("enum def registered before sizing");
                let payload = def
                    .variants
                    .iter()
                    .map(|v| self.aligned_fields_size(&v.fields))
                    .max()
                    .unwrap_or(0);
                // Tag (i32) + 4 bytes padding for 8-aligned payloads + payload.
                // Padding doesn't matter for the buffer size — we just need the
                // payload array large enough that storing the variant struct
                // doesn't overrun.
                payload + 4
            }
            Ty::TypeVar(_) | Ty::GenericStruct { .. } | Ty::GenericEnum { .. } => {
                unreachable!("approx_size_bytes called on unspecialized type")
            }
        }
    }

    /// Lays out a sequence of fields with natural alignment, returning the
    /// final aligned size (including trailing padding to struct alignment).
    pub fn aligned_fields_size_pub(&self, fields: &[Ty]) -> usize {
        self.aligned_fields_size(fields)
    }

    fn aligned_fields_size(&self, fields: &[Ty]) -> usize {
        let mut offset: usize = 0;
        let mut max_align: usize = 1;
        for t in fields {
            let a = self.align_bytes(t);
            max_align = max_align.max(a);
            // pad offset up to a
            if a > 0 {
                offset = (offset + a - 1) / a * a;
            }
            offset += self.approx_size_bytes(t);
        }
        // pad final offset to struct alignment
        if max_align > 0 {
            offset = (offset + max_align - 1) / max_align * max_align;
        }
        offset
    }

    /// Structural LLVM type for a Frey tuple — same element types share
    /// the same anonymous struct, no name needed.
    pub fn tuple_llvm_type(&self, elems: &[Ty]) -> inkwell::types::StructType<'ctx> {
        let field_types: Vec<BasicTypeEnum<'ctx>> =
            elems.iter().map(|e| self.lower_ty(e)).collect();
        self.context.struct_type(&field_types, false)
    }

    pub fn lower_fn_type(
        &self,
        params: &[Param],
        return_ty: &Ty,
        varargs: bool,
    ) -> FunctionType<'ctx> {
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|p| self.lower_ty(&p.ty).into()).collect();
        self.fn_type_with_return(&param_types, return_ty, varargs)
    }

    /// Like `lower_fn_type` but prepends `env: *u8` as the first parameter.
    /// Used for every Frey fn under the closure-uniform calling convention.
    pub fn lower_fn_type_with_env(
        &self,
        params: &[Param],
        return_ty: &Ty,
    ) -> FunctionType<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            Vec::with_capacity(params.len() + 1);
        param_types.push(ptr.into());
        for p in params {
            param_types.push(self.lower_ty(&p.ty).into());
        }
        self.fn_type_with_return(&param_types, return_ty, false)
    }

    pub fn fn_type_for_function_ty(&self, fn_ty: &Ty) -> FunctionType<'ctx> {
        let Ty::Function {
            params,
            return_ty,
            varargs,
        } = fn_ty
        else {
            panic!("expected function type, got {fn_ty:?}");
        };
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|p| self.lower_ty(p).into()).collect();
        self.fn_type_with_return(&param_types, return_ty, *varargs)
    }

    fn fn_type_with_return(
        &self,
        param_types: &[BasicMetadataTypeEnum<'ctx>],
        return_ty: &Ty,
        varargs: bool,
    ) -> FunctionType<'ctx> {
        match return_ty {
            Ty::Unit => self.context.bool_type().fn_type(param_types, varargs),
            Ty::I8 | Ty::U8 => self.context.i8_type().fn_type(param_types, varargs),
            Ty::Int | Ty::UInt | Ty::I32 | Ty::U32 => {
                self.context.i32_type().fn_type(param_types, varargs)
            }
            Ty::I64 | Ty::U64 => self.context.i64_type().fn_type(param_types, varargs),
            Ty::Float | Ty::F32 => self.context.f32_type().fn_type(param_types, varargs),
            Ty::F64 => self.context.f64_type().fn_type(param_types, varargs),
            Ty::Function { .. } | Ty::Ptr(_) => self
                .context
                .ptr_type(AddressSpace::default())
                .fn_type(param_types, varargs),
            Ty::Closure { .. } => self
                .closure_llvm_type()
                .fn_type(param_types, varargs),
            Ty::Array { element, count } => self
                .lower_ty(element)
                .array_type(*count as u32)
                .fn_type(param_types, varargs),
            Ty::Struct(name) => self.struct_llvm[name].fn_type(param_types, varargs),
            Ty::Tuple(elems) => self.tuple_llvm_type(elems).fn_type(param_types, varargs),
            Ty::Enum(name) => self.enum_llvm[name].fn_type(param_types, varargs),
            Ty::TypeVar(_) | Ty::GenericStruct { .. } | Ty::GenericEnum { .. } => {
                unreachable!(
                    "specialization should have eliminated TypeVars, GenericStructs, and GenericEnums"
                )
            }
        }
    }
}
