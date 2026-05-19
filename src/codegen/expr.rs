use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, ValueKind};

use crate::codegen::{Codegen, Error};
use crate::hir::types::{Const, Expr, ExprKind, Statement, StatementKind};
use crate::hir::{BinaryOperator, FunctionCall, UnaryOperator};

impl<'ctx> Codegen<'ctx> {
    pub fn lower_expr(&mut self, expr: Expr) -> Result<BasicValueEnum<'ctx>, Error> {
        match expr.kind {
            ExprKind::Const(Const::Int(n)) => {
                let i32_ty = self.context.i32_type();
                Ok(i32_ty.const_int(n as u64, true).into())
            }
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
                let value = self.lower_expr(*operand)?.into_int_value();
                let result = match op {
                    UnaryOperator::Minus => self.builder.build_int_neg(value, "")?,
                    UnaryOperator::Not => {
                        let zero = self.context.i32_type().const_zero();
                        let is_zero = self.builder.build_int_compare(
                            inkwell::IntPredicate::EQ,
                            value,
                            zero,
                            "",
                        )?;
                        self.builder
                            .build_int_z_extend(is_zero, self.context.i32_type(), "")?
                    }
                };
                Ok(result.into())
            }
            ExprKind::Binary { op, lhs, rhs } => {
                if matches!(op, BinaryOperator::And | BinaryOperator::Or) {
                    let result = self.build_short_circuit(op, *lhs, *rhs)?;
                    return Ok(result.into());
                }

                let lhs = self.lower_expr(*lhs)?.into_int_value();
                let rhs = self.lower_expr(*rhs)?.into_int_value();
                let i32_ty = self.context.i32_type();
                let result = match op {
                    BinaryOperator::Add => self.builder.build_int_add(lhs, rhs, "")?,
                    BinaryOperator::Sub => self.builder.build_int_sub(lhs, rhs, "")?,
                    BinaryOperator::Mul => self.builder.build_int_mul(lhs, rhs, "")?,
                    BinaryOperator::Div => self.builder.build_int_signed_div(lhs, rhs, "")?,
                    BinaryOperator::Mod => self.builder.build_int_signed_rem(lhs, rhs, "")?,
                    BinaryOperator::Shl => self.builder.build_left_shift(lhs, rhs, "")?,
                    BinaryOperator::Shr => self.builder.build_right_shift(lhs, rhs, true, "")?,
                    BinaryOperator::BitAnd => self.builder.build_and(lhs, rhs, "")?,
                    BinaryOperator::BitOr => self.builder.build_or(lhs, rhs, "")?,
                    BinaryOperator::BitXor => self.builder.build_xor(lhs, rhs, "")?,
                    BinaryOperator::Lt
                    | BinaryOperator::Le
                    | BinaryOperator::Gt
                    | BinaryOperator::Ge
                    | BinaryOperator::Eq
                    | BinaryOperator::Ne => {
                        let predicate = match op {
                            BinaryOperator::Lt => inkwell::IntPredicate::SLT,
                            BinaryOperator::Le => inkwell::IntPredicate::SLE,
                            BinaryOperator::Gt => inkwell::IntPredicate::SGT,
                            BinaryOperator::Ge => inkwell::IntPredicate::SGE,
                            BinaryOperator::Eq => inkwell::IntPredicate::EQ,
                            BinaryOperator::Ne => inkwell::IntPredicate::NE,
                            _ => unreachable!(),
                        };
                        let cmp = self.builder.build_int_compare(predicate, lhs, rhs, "")?;
                        self.builder.build_int_z_extend(cmp, i32_ty, "")?
                    }
                    BinaryOperator::And | BinaryOperator::Or => unreachable!(),
                };
                Ok(result.into())
            }
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
        phi.add_incoming(&[
            (&rhs_bool, rhs_end_bb),
            (&short_circuit_value, lhs_end_bb),
        ]);

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
