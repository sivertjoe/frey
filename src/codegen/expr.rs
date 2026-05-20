use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, ValueKind};

use crate::codegen::{Codegen, Error};
use crate::hir::types::{Const, Expr, ExprKind, Statement, StatementKind};
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
                self.lower_numeric_cast(source_val, &source_ty, &target_ty)
            }
            ExprKind::Const(Const::Float(f)) => {
                let f32_ty = self.context.f32_type();
                Ok(f32_ty.const_float(f as f64).into())
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
                todo!("nested function literals require closure support")
            }
            ExprKind::Unary { operand, op } => {
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
                        // Typechecker guarantees Int.
                        let v = value.into_int_value();
                        let zero = self.context.i32_type().const_zero();
                        let is_zero = self.builder.build_int_compare(
                            inkwell::IntPredicate::EQ,
                            v,
                            zero,
                            "",
                        )?;
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
                        let cmp = if is_float {
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
            ExprKind::Block(block) => self.lower_block_value(block),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.lower_if(*condition, *then_branch, *else_branch),
            ExprKind::Call(FunctionCall { callee, args }) => {
                let arg_vals: Vec<BasicValueEnum<'ctx>> = args
                    .into_iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<Result<_, _>>()?;

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
        }
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
        for item in block.items {
            match item {
                crate::hir::types::BlockItem::Declaration(d) => self.lower_local_decl(d)?,
                crate::hir::types::BlockItem::Statement(s) => self.lower_statement(s)?,
            }
        }
        // If a `return` inside the block already terminated this basic block,
        // there's no way to actually evaluate the tail — emit a placeholder
        // value that the caller won't use (since it's unreachable code).
        if self.current_block_terminated() {
            return Ok(self.context.bool_type().const_zero().into());
        }
        self.lower_expr(*block.tail)
    }

    fn lower_if(
        &mut self,
        condition: Expr,
        then_branch: Expr,
        else_branch: Expr,
    ) -> Result<BasicValueEnum<'ctx>, Error> {
        let cond_val = self.lower_expr(condition)?.into_int_value();
        let zero = self.context.i32_type().const_zero();
        let cond_i1 =
            self.builder
                .build_int_compare(inkwell::IntPredicate::NE, cond_val, zero, "")?;

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
                self.builder.build_return(Some(&value))?;
            }
            StatementKind::Expr(expr) => {
                let _ = self.lower_expr(expr)?;
            }
        }
        Ok(())
    }
}
