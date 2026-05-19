use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, ValueKind};

use crate::codegen::{Codegen, Error};
use crate::hir::types::{Const, Expr, ExprKind, Statement, StatementKind};
use crate::hir::{FunctionCall, UnaryOperator};

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
