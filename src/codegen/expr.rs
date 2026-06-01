use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue, ValueKind,
};

use crate::codegen::{Codegen, Error};
use crate::hir::types::{Const, Expr, ExprKind, IntrinsicKind, Statement, StatementKind};
use crate::hir::{BinaryOperator, FunctionCall, Ty, UnaryOperator};

impl<'ctx> Codegen<'ctx> {
    pub fn lower_expr(&mut self, expr: Expr) -> Result<BasicValueEnum<'ctx>, Error> {
        match expr.kind {
            ExprKind::Const(Const::Int(n)) => {
                let i32_ty = self.context.i32_type();
                Ok(i32_ty.const_int(n as u64, true).into())
            }
            ExprKind::Cast { target, expr } => {
                let source_ty = expr.ty.clone();
                let target_ty = target;
                let source_val = self.lower_expr(*expr)?;
                // Pointer → pointer is a no-op under opaque-pointer LLVM —
                // the value is already a `ptr`, so we just forward it.
                if matches!(source_ty, Ty::Ptr(_)) && matches!(target_ty, Ty::Ptr(_)) {
                    return Ok(source_val);
                }
                self.lower_numeric_cast(source_val, &source_ty, &target_ty)
            }
            ExprKind::Const(Const::Float(f)) => {
                let f32_ty = self.context.f32_type();
                Ok(f32_ty.const_float(f as f64).into())
            }
            ExprKind::Const(Const::Str(s)) => Ok(self.string_global_for(&s).into()),
            ExprKind::Const(Const::Char(b)) => {
                Ok(self.context.i8_type().const_int(b as u64, false).into())
            }
            ExprKind::Const(Const::Unit) => Ok(self.context.bool_type().const_zero().into()),
            ExprKind::Local(id) => {
                if let Some(func) = self.functions.get(&id) {
                    return Ok(func.as_global_value().as_pointer_value().into());
                }
                let ptr = *self.locals.get(&id).expect("local binding exists");
                let llvm_ty = self.lower_ty(&expr.ty);
                Ok(self.builder.build_load(llvm_ty, ptr, "")?)
            }
            ExprKind::Function(_) => {
                unreachable!(
                    "function literals are lifted to top-level declarations during HIR lowering"
                )
            }
            ExprKind::Unary { operand, op } => {
                let operand_is_ptr = matches!(operand.ty, Ty::Ptr(_));
                let value = self.lower_expr(*operand)?;
                match op {
                    UnaryOperator::Minus => {
                        // Typechecker guarantees Int or Float.
                        if value.is_int_value() {
                            let v = value.into_int_value();
                            Ok(self.builder.build_int_neg(v, "")?.into())
                        } else {
                            let v = value.into_float_value();
                            Ok(self.builder.build_float_neg(v, "")?.into())
                        }
                    }
                    UnaryOperator::Not => {
                        // `!ptr` → "is null", `!int` → "is zero". Both produce
                        // a 0/1 i32.
                        let is_zero = if operand_is_ptr {
                            let p = value.into_pointer_value();
                            let null_p = p.get_type().const_null();
                            self.builder.build_int_compare(
                                inkwell::IntPredicate::EQ,
                                p,
                                null_p,
                                "",
                            )?
                        } else {
                            let v = value.into_int_value();
                            let zero = v.get_type().const_zero();
                            self.builder.build_int_compare(
                                inkwell::IntPredicate::EQ,
                                v,
                                zero,
                                "",
                            )?
                        };
                        Ok(self
                            .builder
                            .build_int_z_extend(is_zero, self.context.i32_type(), "")?
                            .into())
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                if matches!(op, BinaryOperator::And | BinaryOperator::Or) {
                    let result = self.build_short_circuit(op, *lhs, *rhs)?;
                    return Ok(result.into());
                }

                // Capture operand type before lowering — LLVM doesn't track
                // signedness in types, so we need the HIR type for Div/Mod/Shr
                // and comparison predicate selection.
                let operand_ty = lhs.ty.clone();
                let lhs_val = self.lower_expr(*lhs)?;
                let rhs_val = self.lower_expr(*rhs)?;
                let is_float = operand_ty.is_float();
                let is_unsigned = operand_ty.is_uint();
                let i32_ty = self.context.i32_type();

                let result: BasicValueEnum<'ctx> = match op {
                    BinaryOperator::Add => {
                        if is_float {
                            self.builder
                                .build_float_add(
                                    lhs_val.into_float_value(),
                                    rhs_val.into_float_value(),
                                    "",
                                )?
                                .into()
                        } else {
                            self.builder
                                .build_int_add(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        }
                    }
                    BinaryOperator::Sub => {
                        if is_float {
                            self.builder
                                .build_float_sub(
                                    lhs_val.into_float_value(),
                                    rhs_val.into_float_value(),
                                    "",
                                )?
                                .into()
                        } else {
                            self.builder
                                .build_int_sub(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        }
                    }
                    BinaryOperator::Mul => {
                        if is_float {
                            self.builder
                                .build_float_mul(
                                    lhs_val.into_float_value(),
                                    rhs_val.into_float_value(),
                                    "",
                                )?
                                .into()
                        } else {
                            self.builder
                                .build_int_mul(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        }
                    }
                    BinaryOperator::Div => {
                        if is_float {
                            self.builder
                                .build_float_div(
                                    lhs_val.into_float_value(),
                                    rhs_val.into_float_value(),
                                    "",
                                )?
                                .into()
                        } else if is_unsigned {
                            self.builder
                                .build_int_unsigned_div(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        } else {
                            self.builder
                                .build_int_signed_div(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        }
                    }
                    BinaryOperator::Mod => {
                        if is_float {
                            self.builder
                                .build_float_rem(
                                    lhs_val.into_float_value(),
                                    rhs_val.into_float_value(),
                                    "",
                                )?
                                .into()
                        } else if is_unsigned {
                            self.builder
                                .build_int_unsigned_rem(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        } else {
                            self.builder
                                .build_int_signed_rem(
                                    lhs_val.into_int_value(),
                                    rhs_val.into_int_value(),
                                    "",
                                )?
                                .into()
                        }
                    }
                    // Shifts and bitwise are Int-only (typechecker enforces this).
                    BinaryOperator::Shl => self
                        .builder
                        .build_left_shift(lhs_val.into_int_value(), rhs_val.into_int_value(), "")?
                        .into(),
                    BinaryOperator::Shr => self
                        .builder
                        .build_right_shift(
                            lhs_val.into_int_value(),
                            rhs_val.into_int_value(),
                            !is_unsigned, // signed = arithmetic shift; unsigned = logical
                            "",
                        )?
                        .into(),
                    BinaryOperator::BitAnd => self
                        .builder
                        .build_and(lhs_val.into_int_value(), rhs_val.into_int_value(), "")?
                        .into(),
                    BinaryOperator::BitOr => self
                        .builder
                        .build_or(lhs_val.into_int_value(), rhs_val.into_int_value(), "")?
                        .into(),
                    BinaryOperator::BitXor => self
                        .builder
                        .build_xor(lhs_val.into_int_value(), rhs_val.into_int_value(), "")?
                        .into(),
                    BinaryOperator::Lt
                    | BinaryOperator::Le
                    | BinaryOperator::Gt
                    | BinaryOperator::Ge
                    | BinaryOperator::Eq
                    | BinaryOperator::Ne => {
                        let is_pointer = matches!(operand_ty, Ty::Ptr(_));
                        let cmp = if is_pointer {
                            let predicate = match op {
                                BinaryOperator::Eq => inkwell::IntPredicate::EQ,
                                BinaryOperator::Ne => inkwell::IntPredicate::NE,
                                _ => unreachable!(
                                    "type-checker only permits Eq/Ne on pointers"
                                ),
                            };
                            self.builder.build_int_compare(
                                predicate,
                                lhs_val.into_pointer_value(),
                                rhs_val.into_pointer_value(),
                                "",
                            )?
                        } else if is_float {
                            let predicate = match op {
                                BinaryOperator::Lt => inkwell::FloatPredicate::OLT,
                                BinaryOperator::Le => inkwell::FloatPredicate::OLE,
                                BinaryOperator::Gt => inkwell::FloatPredicate::OGT,
                                BinaryOperator::Ge => inkwell::FloatPredicate::OGE,
                                BinaryOperator::Eq => inkwell::FloatPredicate::OEQ,
                                BinaryOperator::Ne => inkwell::FloatPredicate::ONE,
                                _ => unreachable!(),
                            };
                            self.builder.build_float_compare(
                                predicate,
                                lhs_val.into_float_value(),
                                rhs_val.into_float_value(),
                                "",
                            )?
                        } else {
                            let predicate = if is_unsigned {
                                match op {
                                    BinaryOperator::Lt => inkwell::IntPredicate::ULT,
                                    BinaryOperator::Le => inkwell::IntPredicate::ULE,
                                    BinaryOperator::Gt => inkwell::IntPredicate::UGT,
                                    BinaryOperator::Ge => inkwell::IntPredicate::UGE,
                                    BinaryOperator::Eq => inkwell::IntPredicate::EQ,
                                    BinaryOperator::Ne => inkwell::IntPredicate::NE,
                                    _ => unreachable!(),
                                }
                            } else {
                                match op {
                                    BinaryOperator::Lt => inkwell::IntPredicate::SLT,
                                    BinaryOperator::Le => inkwell::IntPredicate::SLE,
                                    BinaryOperator::Gt => inkwell::IntPredicate::SGT,
                                    BinaryOperator::Ge => inkwell::IntPredicate::SGE,
                                    BinaryOperator::Eq => inkwell::IntPredicate::EQ,
                                    BinaryOperator::Ne => inkwell::IntPredicate::NE,
                                    _ => unreachable!(),
                                }
                            };
                            self.builder.build_int_compare(
                                predicate,
                                lhs_val.into_int_value(),
                                rhs_val.into_int_value(),
                                "",
                            )?
                        };
                        self.builder.build_int_z_extend(cmp, i32_ty, "")?.into()
                    }
                    BinaryOperator::And | BinaryOperator::Or => unreachable!(),
                };
                Ok(result)
            }
            ExprKind::Assign { target, value } => {
                let val = self.lower_expr(*value)?;
                let ptr = self.lower_place(*target)?;
                self.builder.build_store(ptr, val)?;
                // Assignment returns Unit.
                Ok(self.context.bool_type().const_zero().into())
            }
            ExprKind::Block(block) => self.lower_block_value(block),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.lower_if(*condition, *then_branch, *else_branch),
            ExprKind::While { condition, body } => self.lower_while(*condition, body),
            ExprKind::Array(items) => {
                // Build the array value by repeatedly inserting each element
                // into an undef array of the right LLVM type.
                let arr_llvm_ty = self.lower_ty(&expr.ty).into_array_type();
                let mut agg: inkwell::values::AggregateValueEnum<'ctx> =
                    arr_llvm_ty.get_undef().into();
                for (i, item) in items.into_iter().enumerate() {
                    let v = self.lower_expr(item)?;
                    agg = self.builder.build_insert_value(agg, v, i as u32, "")?;
                }
                Ok(agg.into_array_value().into())
            }
            ExprKind::Subscript { expr: arr, index } => {
                // Element value: GEP to the element's storage and load it.
                // If the array source is a place, GEP directly; otherwise
                // spill the rvalue array into an alloca first.
                let elem_llvm_ty = self.lower_ty(&expr.ty);
                let ptr = self.build_subscript_ptr(*arr, *index)?;
                Ok(self.builder.build_load(elem_llvm_ty, ptr, "")?)
            }
            ExprKind::Ref(target) => Ok(self.lower_place(*target)?.into()),
            ExprKind::Deref(target) => {
                let elem_llvm_ty = self.lower_ty(&expr.ty);
                let ptr = self.lower_expr(*target)?.into_pointer_value();
                Ok(self.builder.build_load(elem_llvm_ty, ptr, "")?)
            }
            ExprKind::StructLiteral { fields } => {
                let struct_llvm_ty = self.lower_ty(&expr.ty).into_struct_type();
                let mut agg: inkwell::values::AggregateValueEnum<'ctx> =
                    struct_llvm_ty.get_undef().into();
                for (i, (_, value)) in fields.into_iter().enumerate() {
                    let v = self.lower_expr(value)?;
                    agg = self.builder.build_insert_value(agg, v, i as u32, "")?;
                }
                Ok(agg.into_struct_value().into())
            }
            ExprKind::Field { target, index, .. } => {
                let field_llvm_ty = self.lower_ty(&expr.ty);
                if is_place(&target) {
                    let struct_name = match &target.ty {
                        crate::hir::types::Ty::Struct(n) => n.clone(),
                        _ => unreachable!("Field target has Struct type from lowering"),
                    };
                    let struct_llvm_ty = self.struct_llvm[&struct_name];
                    let struct_ptr = self.lower_place(*target)?;
                    let field_ptr = self.builder.build_struct_gep(
                        struct_llvm_ty,
                        struct_ptr,
                        index as u32,
                        "",
                    )?;
                    Ok(self.builder.build_load(field_llvm_ty, field_ptr, "")?)
                } else {
                    // Field on an rvalue struct (e.g. function-call result):
                    // just extract from the value.
                    let agg = self.lower_expr(*target)?;
                    Ok(self.builder.build_extract_value(
                        agg.into_struct_value(),
                        index as u32,
                        "",
                    )?)
                }
            }
            ExprKind::Call(FunctionCall { callee, args }) => {
                // For varargs (extern C) calls, args past the fixed param
                // count must obey the C default argument promotions: f32 → f64.
                // (i32 stays i32; i8/i16 → i32 would apply if Frey had those
                // smaller types — it doesn't widen i8 today.)
                let (fixed_arity, is_vararg) = if let Ty::Function {
                    params, varargs, ..
                } = &callee.ty
                {
                    (params.len(), *varargs)
                } else {
                    (args.len(), false)
                };

                let mut arg_vals: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(args.len());
                for (i, a) in args.into_iter().enumerate() {
                    let in_vararg_slot = is_vararg && i >= fixed_arity;
                    // C's default argument promotions for `...`: any integer
                    // narrower than `int` is widened to i32 (signed-extended
                    // for signed types, zero-extended for unsigned); `float`
                    // is widened to `double`.
                    let arg_ty = a.ty.clone();
                    let v = self.lower_expr(a)?;
                    let v = if in_vararg_slot {
                        match arg_ty {
                            Ty::Float | Ty::F32 => self
                                .builder
                                .build_float_ext(
                                    v.into_float_value(),
                                    self.context.f64_type(),
                                    "",
                                )?
                                .into(),
                            Ty::I8 => self
                                .builder
                                .build_int_s_extend(
                                    v.into_int_value(),
                                    self.context.i32_type(),
                                    "",
                                )?
                                .into(),
                            Ty::U8 => self
                                .builder
                                .build_int_z_extend(
                                    v.into_int_value(),
                                    self.context.i32_type(),
                                    "",
                                )?
                                .into(),
                            _ => v,
                        }
                    } else {
                        v
                    };
                    arg_vals.push(v);
                }

                let arg_metadata: Vec<BasicMetadataValueEnum<'ctx>> =
                    arg_vals.iter().map(|v| (*v).into()).collect();

                let direct_target = match &callee.kind {
                    ExprKind::Local(id) => self.functions.get(id).copied(),
                    _ => None,
                };

                let call_site = match direct_target {
                    Some(func) => self.builder.build_direct_call(func, &arg_metadata, "")?,
                    None => {
                        let fn_ty = self.fn_type_for_function_ty(&callee.ty);
                        let fn_ptr = self.lower_expr(*callee)?.into_pointer_value();
                        self.builder
                            .build_indirect_call(fn_ty, fn_ptr, &arg_metadata, "")?
                    }
                };

                match call_site.try_as_basic_value() {
                    ValueKind::Basic(v) => Ok(v),
                    ValueKind::Instruction(_) => {
                        panic!("call returned no value, but Frey functions always return")
                    }
                }
            }
            ExprKind::Intrinsic {
                kind,
                elem_ty,
                args,
            } => self.lower_intrinsic(kind, elem_ty, args),
            ExprKind::Tuple(elems) => {
                let elem_tys: Vec<Ty> = elems.iter().map(|e| e.ty.clone()).collect();
                let tuple_llvm_ty = self.tuple_llvm_type(&elem_tys);
                let mut agg: inkwell::values::AggregateValueEnum<'ctx> =
                    tuple_llvm_ty.get_undef().into();
                for (i, el) in elems.into_iter().enumerate() {
                    let v = self.lower_expr(el)?;
                    agg = self.builder.build_insert_value(agg, v, i as u32, "")?;
                }
                Ok(agg.into_struct_value().into())
            }
            ExprKind::TupleField { target, index } => {
                let field_llvm_ty = self.lower_ty(&expr.ty);
                let elem_tys = match &target.ty {
                    Ty::Tuple(elems) => elems.clone(),
                    _ => unreachable!("TupleField target has Tuple type from lowering"),
                };
                if is_place(&target) {
                    let tuple_llvm_ty = self.tuple_llvm_type(&elem_tys);
                    let tuple_ptr = self.lower_place(*target)?;
                    let field_ptr = self.builder.build_struct_gep(
                        tuple_llvm_ty,
                        tuple_ptr,
                        index as u32,
                        "",
                    )?;
                    Ok(self.builder.build_load(field_llvm_ty, field_ptr, "")?)
                } else {
                    let agg = self.lower_expr(*target)?;
                    Ok(self.builder.build_extract_value(
                        agg.into_struct_value(),
                        index as u32,
                        "",
                    )?)
                }
            }
            ExprKind::EnumConstruct {
                enum_name,
                variant_index,
                args,
            } => self.lower_enum_construct(&enum_name, variant_index, args),
            ExprKind::Match { scrutinee, arms } => self.lower_match(*scrutinee, arms, expr.ty),
            ExprKind::ZeroInit(ty) => Ok(zero_value(self.lower_ty(&ty))),
            ExprKind::ExternFunction { .. } => {
                unreachable!("extern function declarations are top-level, not expressions")
            }
            ExprKind::DeferredFunctionRef { .. } => unreachable!(
                "deferred function references are resolved during specialization"
            ),
            ExprKind::TypeValue(_) | ExprKind::CompError(_) => {
                unreachable!("comptime-only nodes are eliminated during specialization")
            }
        }
    }

    /// `Variant(args...)` → `{tag, payload}`. Payload is the variant's fields
    /// packed into a tuple struct, stored over the enum's byte buffer.
    fn lower_enum_construct(
        &mut self,
        enum_name: &str,
        variant_index: usize,
        args: Vec<Expr>,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        let enum_llvm_ty = self.enum_llvm[enum_name];
        let slot = self.builder.build_alloca(enum_llvm_ty, "")?;

        let tag_ptr = self
            .builder
            .build_struct_gep(enum_llvm_ty, slot, 0, "")?;
        let tag_val = self
            .context
            .i32_type()
            .const_int(variant_index as u64, false);
        self.builder.build_store(tag_ptr, tag_val)?;

        if !args.is_empty() {
            let payload_ptr = self
                .builder
                .build_struct_gep(enum_llvm_ty, slot, 1, "")?;
            let elem_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
            let variant_struct = self.tuple_llvm_type(&elem_tys);
            let mut agg: inkwell::values::AggregateValueEnum<'ctx> =
                variant_struct.get_undef().into();
            for (i, a) in args.into_iter().enumerate() {
                let v = self.lower_expr(a)?;
                agg = self.builder.build_insert_value(agg, v, i as u32, "")?;
            }
            self.builder
                .build_store(payload_ptr, agg.into_struct_value())?;
        }

        Ok(self.builder.build_load(enum_llvm_ty, slot, "")?)
    }

    /// `match` lowers to a `switch` on the tag with one BB per arm and a
    /// `phi` at the exit collecting arm values.
    fn lower_match(
        &mut self,
        scrutinee: Expr,
        arms: Vec<crate::hir::types::MatchArm>,
        result_ty: Ty,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        let enum_name = match &scrutinee.ty {
            Ty::Enum(n) => n.clone(),
            _ => unreachable!("typechecker guarantees match scrutinee is Enum"),
        };
        let enum_llvm_ty = self.enum_llvm[&enum_name];

        // Spill so GEP-into works.
        let scrutinee_val = self.lower_expr(scrutinee)?;
        let slot = self.builder.build_alloca(enum_llvm_ty, "")?;
        self.builder.build_store(slot, scrutinee_val)?;

        let tag_ptr = self
            .builder
            .build_struct_gep(enum_llvm_ty, slot, 0, "")?;
        let tag_val = self
            .builder
            .build_load(self.context.i32_type(), tag_ptr, "")?
            .into_int_value();
        // GEP the payload once, before branching — arms reuse the pointer.
        let payload_ptr = self
            .builder
            .build_struct_gep(enum_llvm_ty, slot, 1, "")?;

        let function = self
            .builder
            .get_insert_block()
            .expect("inside a function")
            .get_parent()
            .expect("bb has parent");

        let exit_bb = self.context.append_basic_block(function, "match.exit");

        let mut variant_targets: Vec<(u64, inkwell::basic_block::BasicBlock<'ctx>, usize)> =
            Vec::new();
        let mut wildcard_bb: Option<inkwell::basic_block::BasicBlock<'ctx>> = None;
        let mut wildcard_arm_idx: Option<usize> = None;
        for (i, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                crate::hir::types::HirPattern::Variant { variant_index, .. } => {
                    let bb = self
                        .context
                        .append_basic_block(function, &format!("match.arm{i}"));
                    variant_targets.push((*variant_index as u64, bb, i));
                }
                crate::hir::types::HirPattern::Wildcard => {
                    let bb = self.context.append_basic_block(function, "match.wild");
                    wildcard_bb = Some(bb);
                    wildcard_arm_idx = Some(i);
                }
            }
        }

        // Without a wildcard, exhaustiveness has already ruled this out — but
        // LLVM still needs a switch default, so wire an unreachable block.
        let default_bb = wildcard_bb.unwrap_or_else(|| {
            self.context
                .append_basic_block(function, "match.unreachable")
        });

        let cases: Vec<(IntValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> = variant_targets
            .iter()
            .map(|(tag, bb, _)| {
                (
                    self.context.i32_type().const_int(*tag, false),
                    *bb,
                )
            })
            .collect();
        self.builder.build_switch(tag_val, default_bb, &cases)?;

        if wildcard_bb.is_none() {
            self.builder.position_at_end(default_bb);
            self.builder.build_unreachable()?;
        }

        let mut incoming: Vec<(BasicValueEnum<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            Vec::with_capacity(arms.len());

        for (tag, bb, arm_idx) in &variant_targets {
            self.builder.position_at_end(*bb);
            let arm = &arms[*arm_idx];
            let crate::hir::types::HirPattern::Variant { bindings, .. } = &arm.pattern else {
                unreachable!();
            };
            if !bindings.is_empty() {
                let elem_tys: Vec<Ty> =
                    bindings.iter().map(|(_, _, t)| t.clone()).collect();
                let variant_struct = self.tuple_llvm_type(&elem_tys);
                for (i, (_, local_id, field_ty)) in bindings.iter().enumerate() {
                    let field_ptr = self.builder.build_struct_gep(
                        variant_struct,
                        payload_ptr,
                        i as u32,
                        "",
                    )?;
                    let field_llvm_ty = self.lower_ty(field_ty);
                    let value_slot = self.builder.build_alloca(field_llvm_ty, "")?;
                    let v = self.builder.build_load(field_llvm_ty, field_ptr, "")?;
                    self.builder.build_store(value_slot, v)?;
                    self.locals.insert(*local_id, value_slot);
                }
            }
            let _ = tag;

            let value = self.lower_expr(arm.body.clone())?;
            let end_bb = self
                .builder
                .get_insert_block()
                .expect("arm body produced a basic block");
            // Skip the implicit branch if the arm already terminated (return/break).
            if end_bb.get_terminator().is_none() {
                incoming.push((value, end_bb));
                self.builder.build_unconditional_branch(exit_bb)?;
            }
        }

        if let Some(arm_idx) = wildcard_arm_idx {
            self.builder.position_at_end(default_bb);
            let arm = &arms[arm_idx];
            let value = self.lower_expr(arm.body.clone())?;
            let end_bb = self
                .builder
                .get_insert_block()
                .expect("arm body produced a basic block");
            if end_bb.get_terminator().is_none() {
                incoming.push((value, end_bb));
                self.builder.build_unconditional_branch(exit_bb)?;
            }
        }

        self.builder.position_at_end(exit_bb);
        // If every arm terminated, exit_bb has no predecessors — emit an
        // unreachable and synthesize a dummy value of the result type.
        if incoming.is_empty() {
            self.builder.build_unreachable()?;
            let result_llvm_ty = self.lower_ty(&result_ty);
            return Ok(match result_llvm_ty {
                BasicTypeEnum::IntType(t) => t.const_zero().into(),
                BasicTypeEnum::FloatType(t) => t.const_zero().into(),
                BasicTypeEnum::PointerType(t) => t.const_null().into(),
                BasicTypeEnum::StructType(t) => t.const_zero().into(),
                BasicTypeEnum::ArrayType(t) => t.const_zero().into(),
                BasicTypeEnum::VectorType(t) => t.const_zero().into(),
                BasicTypeEnum::ScalableVectorType(t) => t.const_zero().into(),
            });
        }
        let result_llvm_ty = self.lower_ty(&result_ty);
        let phi = self.builder.build_phi(result_llvm_ty, "match.value")?;
        let phi_incoming: Vec<(&dyn inkwell::values::BasicValue<'ctx>, _)> = incoming
            .iter()
            .map(|(v, bb)| {
                let bv: &dyn inkwell::values::BasicValue<'ctx> = match v {
                    BasicValueEnum::IntValue(i) => i,
                    BasicValueEnum::FloatValue(f) => f,
                    BasicValueEnum::PointerValue(p) => p,
                    BasicValueEnum::ArrayValue(a) => a,
                    BasicValueEnum::StructValue(s) => s,
                    BasicValueEnum::VectorValue(v) => v,
                    BasicValueEnum::ScalableVectorValue(v) => v,
                };
                (bv, *bb)
            })
            .collect();
        phi.add_incoming(&phi_incoming);
        Ok(phi.as_basic_value())
    }

    /// Emits a heap intrinsic as a call to libc `malloc`/`realloc`/`free`.
    /// `alloc`/`realloc` size their allocation as `count * sizeof(elem_ty)`.
    fn lower_intrinsic(
        &mut self,
        kind: IntrinsicKind,
        elem_ty: Ty,
        args: Vec<Expr>,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let mut args = args.into_iter();
        match kind {
            IntrinsicKind::Alloc => {
                let count = self
                    .lower_expr(args.next().expect("alloc takes a count"))?
                    .into_int_value();
                let bytes = self.alloc_byte_size(count, &elem_ty)?;
                let malloc = self.libc_function("malloc", ptr_ty.fn_type(&[i64_ty.into()], false));
                let call = self
                    .builder
                    .build_direct_call(malloc, &[bytes.into()], "")?;
                match call.try_as_basic_value() {
                    ValueKind::Basic(v) => Ok(v),
                    ValueKind::Instruction(_) => panic!("malloc returns a pointer"),
                }
            }
            IntrinsicKind::Realloc => {
                let ptr = self
                    .lower_expr(args.next().expect("realloc takes a pointer"))?
                    .into_pointer_value();
                let count = self
                    .lower_expr(args.next().expect("realloc takes a count"))?
                    .into_int_value();
                let bytes = self.alloc_byte_size(count, &elem_ty)?;
                let realloc = self.libc_function(
                    "realloc",
                    ptr_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false),
                );
                let call =
                    self.builder
                        .build_direct_call(realloc, &[ptr.into(), bytes.into()], "")?;
                match call.try_as_basic_value() {
                    ValueKind::Basic(v) => Ok(v),
                    ValueKind::Instruction(_) => panic!("realloc returns a pointer"),
                }
            }
            IntrinsicKind::Free => {
                let ptr = self
                    .lower_expr(args.next().expect("free takes a pointer"))?
                    .into_pointer_value();
                let free = self.libc_function(
                    "free",
                    self.context.void_type().fn_type(&[ptr_ty.into()], false),
                );
                self.builder.build_direct_call(free, &[ptr.into()], "")?;
                Ok(self.context.bool_type().const_zero().into())
            }
        }
    }

    /// `count * sizeof(elem_ty)` as an i64, for sizing an allocation.
    fn alloc_byte_size(
        &self,
        count: inkwell::values::IntValue<'ctx>,
        elem_ty: &Ty,
    ) -> Result<inkwell::values::IntValue<'ctx>, Error> {
        let i64_ty = self.context.i64_type();
        let count64 = if count.get_type().get_bit_width() == 64 {
            count
        } else {
            self.builder.build_int_s_extend(count, i64_ty, "")?
        };
        let elem_size = self
            .lower_ty(elem_ty)
            .size_of()
            .expect("sized element type");
        Ok(self.builder.build_int_mul(count64, elem_size, "")?)
    }

    fn libc_function(
        &self,
        name: &str,
        ty: inkwell::types::FunctionType<'ctx>,
    ) -> FunctionValue<'ctx> {
        self.module.get_function(name).unwrap_or_else(|| {
            self.module
                .add_function(name, ty, Some(inkwell::module::Linkage::External))
        })
    }

    /// Lowers a place expression to its storage pointer. Valid places are
    /// `Local`, `Subscript`, and `Deref` (the pointer value itself is the
    /// storage address).
    fn lower_place(&mut self, expr: Expr) -> Result<PointerValue<'ctx>, Error> {
        match expr.kind {
            ExprKind::Local(id) => Ok(*self
                .locals
                .get(&id)
                .expect("assignable local is in the locals table")),
            ExprKind::Subscript { expr: arr, index } => self.build_subscript_ptr(*arr, *index),
            ExprKind::Deref(target) => Ok(self.lower_expr(*target)?.into_pointer_value()),
            ExprKind::Field { target, index, .. } => {
                let struct_name = match &target.ty {
                    crate::hir::types::Ty::Struct(n) => n.clone(),
                    _ => unreachable!("Field target has Struct type from lowering"),
                };
                let struct_llvm_ty = self.struct_llvm[&struct_name];
                let struct_ptr = self.lower_place(*target)?;
                Ok(self
                    .builder
                    .build_struct_gep(struct_llvm_ty, struct_ptr, index as u32, "")?)
            }
            ExprKind::TupleField { target, index } => {
                let elem_tys = match &target.ty {
                    Ty::Tuple(elems) => elems.clone(),
                    _ => unreachable!("TupleField target has Tuple type from lowering"),
                };
                let tuple_llvm_ty = self.tuple_llvm_type(&elem_tys);
                let tuple_ptr = self.lower_place(*target)?;
                Ok(self
                    .builder
                    .build_struct_gep(tuple_llvm_ty, tuple_ptr, index as u32, "")?)
            }
            _ => unreachable!("assignment target must be a place expression"),
        }
    }

    /// Computes a pointer to the indexed element of an array. If the array
    /// expression is itself a place, GEP off its existing pointer; otherwise
    /// (e.g. a call result), spill the rvalue array to a fresh alloca first
    /// so we have a pointer to GEP into.
    fn build_subscript_ptr(&mut self, arr: Expr, index: Expr) -> Result<PointerValue<'ctx>, Error> {
        // Raw-pointer indexing: `p[i]` is `gep elem, p_value, i`. The base
        // expression evaluates to the pointer value itself (not a place).
        if let Ty::Ptr(elem) = arr.ty.clone() {
            let elem_llvm_ty = self.lower_ty(&elem);
            let base_ptr = self.lower_expr(arr)?.into_pointer_value();
            let index_val = self.lower_expr(index)?.into_int_value();
            let elem_ptr = unsafe {
                self.builder
                    .build_in_bounds_gep(elem_llvm_ty, base_ptr, &[index_val], "")?
            };
            return Ok(elem_ptr);
        }
        let array_llvm_ty = self.lower_ty(&arr.ty);
        let array_ptr = if is_place(&arr) {
            self.lower_place(arr)?
        } else {
            let arr_val = self.lower_expr(arr)?;
            let slot = self.builder.build_alloca(array_llvm_ty, "")?;
            self.builder.build_store(slot, arr_val)?;
            slot
        };
        let index_val = self.lower_expr(index)?.into_int_value();
        let zero = self.context.i32_type().const_zero();
        // [N x T]* -> T*  via  gep [N x T], ptr, 0, idx
        let elem_ptr = unsafe {
            self.builder
                .build_in_bounds_gep(array_llvm_ty, array_ptr, &[zero, index_val], "")?
        };
        Ok(elem_ptr)
    }

    fn lower_numeric_cast(
        &mut self,
        source_val: BasicValueEnum<'ctx>,
        source_ty: &Ty,
        target_ty: &Ty,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        if source_ty == target_ty {
            return Ok(source_val);
        }

        let source_is_float = source_ty.is_float();
        let target_is_float = target_ty.is_float();
        let source_width = source_ty.bit_width().expect("numeric source");
        let target_width = target_ty.bit_width().expect("numeric target");

        match (source_is_float, target_is_float) {
            // float -> float
            (true, true) => {
                let v = source_val.into_float_value();
                let target_llvm = self.float_llvm_type(target_ty);
                if source_width < target_width {
                    Ok(self.builder.build_float_ext(v, target_llvm, "")?.into())
                } else if source_width > target_width {
                    Ok(self.builder.build_float_trunc(v, target_llvm, "")?.into())
                } else {
                    Ok(source_val)
                }
            }
            // float -> int
            (true, false) => {
                let v = source_val.into_float_value();
                let target_llvm = self.int_llvm_type(target_ty);
                if target_ty.is_uint() {
                    Ok(self
                        .builder
                        .build_float_to_unsigned_int(v, target_llvm, "")?
                        .into())
                } else {
                    Ok(self
                        .builder
                        .build_float_to_signed_int(v, target_llvm, "")?
                        .into())
                }
            }
            // int -> float
            (false, true) => {
                let v = source_val.into_int_value();
                let target_llvm = self.float_llvm_type(target_ty);
                if source_ty.is_uint() {
                    Ok(self
                        .builder
                        .build_unsigned_int_to_float(v, target_llvm, "")?
                        .into())
                } else {
                    Ok(self
                        .builder
                        .build_signed_int_to_float(v, target_llvm, "")?
                        .into())
                }
            }
            // int -> int
            (false, false) => {
                let v = source_val.into_int_value();
                let target_llvm = self.int_llvm_type(target_ty);
                if source_width < target_width {
                    // Widening: zero-extend for unsigned source, sign-extend for signed.
                    if source_ty.is_uint() {
                        Ok(self.builder.build_int_z_extend(v, target_llvm, "")?.into())
                    } else {
                        Ok(self.builder.build_int_s_extend(v, target_llvm, "")?.into())
                    }
                } else if source_width > target_width {
                    Ok(self.builder.build_int_truncate(v, target_llvm, "")?.into())
                } else {
                    // Same width — Int<->I32, UInt<->U32, signed<->unsigned of same width.
                    Ok(source_val)
                }
            }
        }
    }

    fn int_llvm_type(&self, ty: &Ty) -> inkwell::types::IntType<'ctx> {
        match ty.bit_width().expect("integer type") {
            8 => self.context.i8_type(),
            32 => self.context.i32_type(),
            64 => self.context.i64_type(),
            _ => unreachable!("unsupported integer width"),
        }
    }

    fn float_llvm_type(&self, ty: &Ty) -> inkwell::types::FloatType<'ctx> {
        match ty.bit_width().expect("float type") {
            32 => self.context.f32_type(),
            64 => self.context.f64_type(),
            _ => unreachable!("unsupported float width"),
        }
    }

    fn lower_block_value(
        &mut self,
        block: crate::hir::types::Block,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        self.defer_scopes.push(Vec::new());
        for item in block.items {
            match item {
                crate::hir::types::BlockItem::Declaration(d) => self.lower_local_decl(d)?,
                crate::hir::types::BlockItem::Statement(s) => self.lower_statement(s)?,
            }
        }
        let result = if self.current_block_terminated() {
            // A `return`/`break` inside already ran the relevant defers and
            // terminated the block; the tail (and a placeholder value) is dead.
            Ok(self.context.bool_type().const_zero().into())
        } else {
            let value = self.lower_expr(*block.tail)?;
            // Normal exit: run this block's own defers after computing its value.
            self.run_top_defer_scope()?;
            Ok(value)
        };
        self.defer_scopes.pop();
        result
    }

    /// Lowers a truthy condition (integer or pointer) to an i1. Integers
    /// are truthy if non-zero; pointers if non-null.
    fn lower_condition(
        &mut self,
        condition: Expr,
    ) -> Result<inkwell::values::IntValue<'ctx>, Error> {
        let is_pointer = matches!(condition.ty, Ty::Ptr(_));
        let value = self.lower_expr(condition)?;
        if is_pointer {
            let p = value.into_pointer_value();
            let null_p = p.get_type().const_null();
            Ok(self.builder.build_int_compare(
                inkwell::IntPredicate::NE,
                p,
                null_p,
                "",
            )?)
        } else {
            let v = value.into_int_value();
            let zero = v.get_type().const_zero();
            Ok(self
                .builder
                .build_int_compare(inkwell::IntPredicate::NE, v, zero, "")?)
        }
    }

    fn lower_if(
        &mut self,
        condition: Expr,
        then_branch: Expr,
        else_branch: Expr,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        let cond_i1 = self.lower_condition(condition)?;

        let function = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let then_bb = self.context.append_basic_block(function, "if.then");
        let else_bb = self.context.append_basic_block(function, "if.else");
        let merge_bb = self.context.append_basic_block(function, "if.merge");

        self.builder
            .build_conditional_branch(cond_i1, then_bb, else_bb)?;

        // Then branch
        self.builder.position_at_end(then_bb);
        let then_val = self.lower_expr(then_branch)?;
        let then_end_bb = self.builder.get_insert_block().unwrap();
        let then_reaches_merge = then_end_bb.get_terminator().is_none();
        if then_reaches_merge {
            self.builder.build_unconditional_branch(merge_bb)?;
        }

        // Else branch
        self.builder.position_at_end(else_bb);
        let else_val = self.lower_expr(else_branch)?;
        let else_end_bb = self.builder.get_insert_block().unwrap();
        let else_reaches_merge = else_end_bb.get_terminator().is_none();
        if else_reaches_merge {
            self.builder.build_unconditional_branch(merge_bb)?;
        }

        // Merge
        self.builder.position_at_end(merge_bb);
        match (then_reaches_merge, else_reaches_merge) {
            (true, true) => {
                let phi_ty = then_val.get_type();
                let phi = self.builder.build_phi(phi_ty, "")?;
                phi.add_incoming(&[(&then_val, then_end_bb), (&else_val, else_end_bb)]);
                Ok(phi.as_basic_value())
            }
            (true, false) => Ok(then_val),
            (false, true) => Ok(else_val),
            (false, false) => {
                self.builder.build_unreachable()?;
                Ok(then_val)
            }
        }
    }

    fn build_short_circuit(
        &mut self,
        op: BinaryOperator,
        lhs: Expr,
        rhs: Expr,
    ) -> Result<IntValue<'ctx>, Error> {
        let is_and = matches!(op, BinaryOperator::And);
        let i32_ty = self.context.i32_type();
        let i1_ty = self.context.bool_type();
        let zero = i32_ty.const_zero();

        // Evaluate lhs in the current block, convert to i1.
        let lhs_val = self.lower_expr(lhs)?.into_int_value();
        let lhs_bool =
            self.builder
                .build_int_compare(inkwell::IntPredicate::NE, lhs_val, zero, "")?;
        let lhs_end_bb = self.builder.get_insert_block().unwrap();

        // Get the enclosing function so we can append new blocks.
        let function = lhs_end_bb.get_parent().unwrap();
        let eval_rhs_bb = self.context.append_basic_block(function, "sc.rhs");
        let merge_bb = self.context.append_basic_block(function, "sc.merge");

        // For `&&`: branch to rhs when lhs is true, to merge (short-circuit) when false.
        // For `||`: branch to merge (short-circuit) when lhs is true, to rhs when false.
        if is_and {
            self.builder
                .build_conditional_branch(lhs_bool, eval_rhs_bb, merge_bb)?;
        } else {
            self.builder
                .build_conditional_branch(lhs_bool, merge_bb, eval_rhs_bb)?;
        }

        // Lower rhs in its own block; capture the block we end up in
        // (rhs might itself contain branches).
        self.builder.position_at_end(eval_rhs_bb);
        let rhs_val = self.lower_expr(rhs)?.into_int_value();
        let rhs_bool =
            self.builder
                .build_int_compare(inkwell::IntPredicate::NE, rhs_val, zero, "")?;
        let rhs_end_bb = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb)?;

        // Merge: phi of rhs_bool (came from rhs path) vs the short-circuit constant.
        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(i1_ty, "")?;
        let short_circuit_value = if is_and {
            i1_ty.const_zero()
        } else {
            i1_ty.const_int(1, false)
        };
        phi.add_incoming(&[(&rhs_bool, rhs_end_bb), (&short_circuit_value, lhs_end_bb)]);

        let phi_i1 = phi.as_basic_value().into_int_value();
        Ok(self.builder.build_int_z_extend(phi_i1, i32_ty, "")?)
    }

    pub fn lower_statement(&mut self, stmt: Statement) -> Result<(), Error> {
        match stmt.kind {
            StatementKind::Return(expr) => {
                let value = self.lower_expr(expr)?;
                // Run every enclosing scope's defers before leaving the function.
                self.run_all_defers()?;
                self.builder.build_return(Some(&value))?;
            }
            StatementKind::Expr(expr) => {
                let _ = self.lower_expr(expr)?;
            }
            StatementKind::Break => {
                // Run defers registered inside the loop before exiting it.
                self.run_break_defers()?;
                let exit = *self
                    .loop_exit_stack
                    .last()
                    .expect("typechecker guarantees break is inside a loop");
                self.builder.build_unconditional_branch(exit)?;
            }
            StatementKind::Defer(expr) => {
                // Register for the current block scope; emitted at scope exit.
                self.defer_scopes
                    .last_mut()
                    .expect("defer is always inside a block scope")
                    .push(expr);
            }
        }
        Ok(())
    }

    /// Emits the given defer scopes in order (callers pass them innermost-first);
    /// each scope's expressions run LIFO.
    fn run_defer_scopes(&mut self, scopes: Vec<Vec<Expr>>) -> Result<(), Error> {
        for scope in scopes {
            for e in scope.into_iter().rev() {
                self.lower_expr(e)?;
            }
        }
        Ok(())
    }

    /// All pending defers, innermost scope first — run before a `return`.
    fn run_all_defers(&mut self) -> Result<(), Error> {
        let scopes: Vec<Vec<Expr>> = self.defer_scopes.iter().rev().cloned().collect();
        self.run_defer_scopes(scopes)
    }

    /// Just the current block's defers — run on a normal (fall-through) exit.
    pub(super) fn run_top_defer_scope(&mut self) -> Result<(), Error> {
        let scope = self.defer_scopes.last().cloned().unwrap_or_default();
        self.run_defer_scopes(vec![scope])
    }

    /// Defers registered inside the current loop — run before a `break`.
    fn run_break_defers(&mut self) -> Result<(), Error> {
        let boundary = self.loop_defer_boundary.last().copied().unwrap_or(0);
        let scopes: Vec<Vec<Expr>> = self.defer_scopes[boundary..]
            .iter()
            .rev()
            .cloned()
            .collect();
        self.run_defer_scopes(scopes)
    }

    fn lower_while(
        &mut self,
        condition: Expr,
        body: crate::hir::types::Block,
    ) -> Result<inkwell::values::BasicValueEnum<'ctx>, Error> {
        let function = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let header_bb = self.context.append_basic_block(function, "while.cond");
        let body_bb = self.context.append_basic_block(function, "while.body");
        let exit_bb = self.context.append_basic_block(function, "while.exit");

        self.builder.build_unconditional_branch(header_bb)?;

        // Header: evaluate condition, branch to body or exit.
        self.builder.position_at_end(header_bb);
        let cond_i1 = self.lower_condition(condition)?;
        self.builder
            .build_conditional_branch(cond_i1, body_bb, exit_bb)?;

        // Body: lower with the exit block pushed so `break` can find it.
        self.builder.position_at_end(body_bb);
        self.loop_exit_stack.push(exit_bb);
        self.loop_defer_boundary.push(self.defer_scopes.len());
        let _ = self.lower_block_value(body)?;
        self.loop_defer_boundary.pop();
        self.loop_exit_stack.pop();
        if !self.current_block_terminated() {
            self.builder.build_unconditional_branch(header_bb)?;
        }

        self.builder.position_at_end(exit_bb);
        // While expressions evaluate to Unit.
        Ok(self.context.bool_type().const_zero().into())
    }
}

fn is_place(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Local(_)
            | ExprKind::Subscript { .. }
            | ExprKind::Deref(_)
            | ExprKind::Field { .. }
            | ExprKind::TupleField { .. }
    )
}

/// All-zero constant for any basic LLVM type. Used to materialize `let x: T;`.
fn zero_value(ty: inkwell::types::BasicTypeEnum<'_>) -> BasicValueEnum<'_> {
    use inkwell::types::BasicTypeEnum::*;
    match ty {
        IntType(t) => t.const_zero().into(),
        FloatType(t) => t.const_zero().into(),
        PointerType(t) => t.const_null().into(),
        ArrayType(t) => t.const_zero().into(),
        StructType(t) => t.const_zero().into(),
        VectorType(t) => t.const_zero().into(),
        ScalableVectorType(t) => t.const_zero().into(),
    }
}
