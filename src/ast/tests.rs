#[cfg(test)]
mod tests {
    use crate::ast::parser::Parser;
    use crate::ast::types::{
        BinaryOperator, BlockItem, Const, Expr, ExprKind, StatementKind, TypeExprKind,
        UnaryOperator,
    };
    use crate::lexer::tokenize;

    fn parser(src: &str) -> Parser {
        let tokens = tokenize(src).unwrap();
        Parser::new(tokens)
    }

    fn assert_callee_named(callee: &Expr, expected: &str) {
        let ExprKind::Identifier(name) = &callee.kind else {
            panic!("expected identifier callee, got {:?}", callee.kind);
        };
        assert_eq!(name, expected);
    }

    #[test]
    fn parses_int_type() {
        let ty = parser("Int").parse_type().unwrap();
        assert!(matches!(ty.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_function_type() {
        let ty = parser("() -> Int").parse_type().unwrap();
        match ty.kind {
            TypeExprKind::Function { params, return_ty } => {
                assert!(params.is_empty());
                assert!(matches!(return_ty.kind, TypeExprKind::Int));
            }
            _ => panic!("expected function type"),
        }
    }

    #[test]
    fn parses_function_type_with_params() {
        let ty = parser("(Int, Int) -> Int").parse_type().unwrap();
        let TypeExprKind::Function { params, return_ty } = ty.kind else {
            panic!("expected function type");
        };
        assert_eq!(params.len(), 2);
        assert!(matches!(params[0].kind, TypeExprKind::Int));
        assert!(matches!(params[1].kind, TypeExprKind::Int));
        assert!(matches!(return_ty.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_nested_function_type() {
        let ty = parser("() -> () -> Int").parse_type().unwrap();
        let TypeExprKind::Function { return_ty, .. } = ty.kind else {
            panic!("expected outer function type");
        };
        let TypeExprKind::Function {
            return_ty: inner_ret,
            ..
        } = return_ty.kind
        else {
            panic!("expected nested function type");
        };
        assert!(matches!(inner_ret.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_call_no_args() {
        let expr = parser("f()").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call");
        };
        assert_callee_named(&callee, "f");
        assert!(args.is_empty());
    }

    #[test]
    fn parses_call_one_arg() {
        let expr = parser("f(5)").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call");
        };
        assert_callee_named(&callee, "f");
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0].kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn parses_nested_call() {
        let expr = parser("f(g())").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected outer call");
        };
        assert_callee_named(&callee, "f");
        assert_eq!(args.len(), 1);
        let ExprKind::Call { callee: inner, .. } = &args[0].kind else {
            panic!("expected inner call");
        };
        assert_callee_named(inner, "g");
    }

    #[test]
    fn parses_unary_minus() {
        let expr = parser("-5").parse_expr().unwrap();
        let ExprKind::Unary { op, expr: inner } = expr.kind else {
            panic!("expected unary");
        };
        assert!(matches!(op, UnaryOperator::Minus));
        assert!(matches!(inner.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn precedence_mul_binds_tighter_than_add() {
        let expr = parser("1 + 2 * 3").parse_expr().unwrap();
        let ExprKind::Binary { op, rhs, .. } = expr.kind else {
            panic!("expected binary");
        };
        assert_eq!(op, BinaryOperator::Add);
        let ExprKind::Binary { op: inner, .. } = &rhs.kind else {
            panic!("expected inner binary");
        };
        assert_eq!(*inner, BinaryOperator::Mul);
    }

    #[test]
    fn parses_block_with_tail_only() {
        let block = parser("{ 7 }").parse_block().unwrap();
        assert!(block.items.is_empty());
        let tail = block.tail.as_ref().unwrap();
        assert!(matches!(tail.kind, ExprKind::Const(Const::Int(7))));
    }

    #[test]
    fn parses_declaration() {
        let decl = parser("let x = 5;").parse_declaration().unwrap();
        assert_eq!(decl.name, "x");
        assert!(matches!(decl.value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn parses_if_without_else() {
        let expr = parser("if x { 1 }").parse_expr().unwrap();

        let ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } = expr.kind
        else {
            panic!("expected if expression");
        };

        assert!(else_branch.is_none());

        let ExprKind::Identifier(name) = &condition.kind else {
            panic!("expected identifier condition");
        };
        assert_eq!(name, "x");

        let tail = then_branch.tail.as_ref().unwrap();
        assert!(matches!(tail.kind, ExprKind::Const(Const::Int(1))));
    }
    #[test]
    fn parses_if_else_expression() {
        let expr = parser("if x { 1 } else { 2 }").parse_expr().unwrap();

        let ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } = expr.kind
        else {
            panic!("expected if");
        };

        let ExprKind::Identifier(name) = &condition.kind else {
            panic!("expected condition");
        };
        assert_eq!(name, "x");

        let then_tail = then_branch.tail.as_ref().unwrap();
        assert!(matches!(then_tail.kind, ExprKind::Const(Const::Int(1))));

        let else_expr = else_branch.expect("expected else branch");

        let ExprKind::Block(block) = &else_expr.kind else {
            panic!("expected block in else branch");
        };

        let else_tail = block.tail.as_ref().unwrap();
        assert!(matches!(else_tail.kind, ExprKind::Const(Const::Int(2))));
    }

    #[test]
    fn parses_else_if_as_nested_if() {
        let expr = parser("if a { 1 } else if b { 2 } else { 3 }")
            .parse_expr()
            .unwrap();

        let ExprKind::If { else_branch, .. } = expr.kind else {
            panic!("expected outer if");
        };

        let else_expr = else_branch.unwrap();

        let ExprKind::If {
            condition,
            else_branch: nested_else,
            ..
        } = &else_expr.kind
        else {
            panic!("expected nested if");
        };

        let ExprKind::Identifier(name) = &condition.kind else {
            panic!("expected condition");
        };
        assert_eq!(name, "b");

        assert!(nested_else.is_some());
    }

    #[test]
    fn if_binds_as_expression_in_binary() {
        let expr = parser("1 + if x { 2 } else { 3 }").parse_expr().unwrap();

        let ExprKind::Binary { rhs, .. } = expr.kind else {
            panic!("expected binary");
        };

        assert!(matches!(rhs.kind, ExprKind::If { .. }));
    }

    #[test]
    fn if_inside_block_tail() {
        let block = parser("{ if x { 1 } else { 2 } }").parse_block().unwrap();
        assert!(block.items.is_empty());
        assert!(matches!(
            block.tail.as_ref().unwrap().kind,
            ExprKind::If { .. }
        ));
    }

    #[test]
    fn dangling_else_binds_to_nearest_if() {
        let expr = parser("if a { if b { 1 } else { 2 } }")
            .parse_expr()
            .unwrap();

        let ExprKind::If { then_branch, .. } = expr.kind else {
            panic!("expected outer if");
        };

        let tail = then_branch.tail.as_ref().unwrap();

        let ExprKind::If { else_branch, .. } = &tail.kind else {
            panic!("expected inner if");
        };

        assert!(else_branch.is_some());
    }

    #[test]
    fn if_statement_without_trailing_semicolon() {
        // `{ if a { return 1; } return 0; }` — the `if` is a statement,
        // followed directly by another statement, with no `;` after `}`.
        let block = parser("{ if a { return 1; } return 0; }")
            .parse_block()
            .unwrap();
        assert_eq!(block.items.len(), 2);

        // First item: the `if` as an expression statement
        let BlockItem::Statement(stmt0) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Expr(expr) = &stmt0.kind else {
            panic!("expected expression statement");
        };
        assert!(matches!(expr.kind, ExprKind::If { .. }));

        // Second item: the trailing return statement
        let BlockItem::Statement(stmt1) = &block.items[1] else {
            panic!("expected statement");
        };
        assert!(matches!(stmt1.kind, StatementKind::Return(_)));
    }

    #[test]
    fn nested_block_as_statement_without_semicolon() {
        // `{ { 1 } 2 }` — the inner `{ 1 }` is a block-statement, then `2` is the tail
        let block = parser("{ { 1 } 2 }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Expr(expr) = &stmt.kind else {
            panic!("expected expression statement");
        };
        assert!(matches!(expr.kind, ExprKind::Block(_)));

        let tail = block.tail.as_ref().unwrap();
        assert!(matches!(tail.kind, ExprKind::Const(Const::Int(2))));
    }

    #[test]
    fn if_can_still_be_followed_by_explicit_semicolon() {
        // `{ if a { 1 }; }` — semicolon still accepted (and consumed)
        let block = parser("{ if a { 1 }; }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Expr(_) = &stmt.kind else {
            panic!("expected expression statement");
        };
    }

    #[test]
    fn parses_bare_return() {
        // `{ return; }` — return without an expression, for unit-returning functions
        let block = parser("{ return; }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Return(expr) = &stmt.kind else {
            panic!("expected return statement");
        };
        assert!(expr.is_none(), "expected bare return (no value)");
    }

    #[test]
    fn parses_return_with_value_still_works() {
        let block = parser("{ return 5; }").parse_block().unwrap();
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Return(Some(expr)) = &stmt.kind else {
            panic!("expected return with value");
        };
        assert!(matches!(expr.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn function_literal_without_return_type_annotation() {
        // `() { }` — no `->` means implicit Unit return
        let expr = parser("() { }").parse_expr().unwrap();
        let ExprKind::Function { return_ty, .. } = expr.kind else {
            panic!("expected function literal");
        };
        assert!(return_ty.is_none());
    }

    #[test]
    fn function_literal_with_return_type_annotation() {
        // `() -> Int { 0 }` — explicit return type still parses
        let expr = parser("() -> Int { 0 }").parse_expr().unwrap();
        let ExprKind::Function { return_ty, .. } = expr.kind else {
            panic!("expected function literal");
        };
        let ret = return_ty.expect("return type should be present");
        assert!(matches!(ret.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_identifier_expression() {
        let expr = parser("x").parse_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::Identifier(_)));
    }

    #[test]
    fn parses_simple_cast() {
        let expr = parser("x as Int").parse_expr().unwrap();
        let ExprKind::Cast { expr: inner, target } = expr.kind else {
            panic!("expected cast");
        };
        let ExprKind::Identifier(name) = &inner.kind else {
            panic!("expected identifier inside cast");
        };
        assert_eq!(name, "x");
        assert!(matches!(target.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_cast_to_float() {
        let expr = parser("5 as Float").parse_expr().unwrap();
        let ExprKind::Cast { expr: inner, target } = expr.kind else {
            panic!("expected cast");
        };
        assert!(matches!(inner.kind, ExprKind::Const(Const::Int(5))));
        assert!(matches!(target.kind, TypeExprKind::Float));
    }

    #[test]
    fn cast_binds_tighter_than_addition() {
        // `1 + 2 as Int` should parse as `1 + (2 as Int)`
        let expr = parser("1 + 2 as Int").parse_expr().unwrap();
        let ExprKind::Binary { op, lhs, rhs } = expr.kind else {
            panic!("expected binary at top");
        };
        assert_eq!(op, BinaryOperator::Add);
        assert!(matches!(lhs.kind, ExprKind::Const(Const::Int(1))));
        // RHS should be the cast, NOT another binary
        let ExprKind::Cast { expr: inner, .. } = &rhs.kind else {
            panic!("expected cast on rhs, got {:?}", rhs.kind);
        };
        assert!(matches!(inner.kind, ExprKind::Const(Const::Int(2))));
    }

    #[test]
    fn parens_override_cast_precedence() {
        // `(1 + 2) as Int` should cast the Add result
        let expr = parser("(1 + 2) as Int").parse_expr().unwrap();
        let ExprKind::Cast { expr: inner, .. } = expr.kind else {
            panic!("expected cast at top");
        };
        let ExprKind::Binary { op, .. } = &inner.kind else {
            panic!("expected binary inside cast");
        };
        assert_eq!(*op, BinaryOperator::Add);
    }

    #[test]
    fn unary_binds_tighter_than_cast() {
        // `-x as Int` should parse as `(-x) as Int`, NOT `-(x as Int)`
        let expr = parser("-x as Int").parse_expr().unwrap();
        let ExprKind::Cast { expr: inner, .. } = expr.kind else {
            panic!("expected cast at top, got {:?}", expr.kind);
        };
        assert!(matches!(inner.kind, ExprKind::Unary { .. }));
    }

    #[test]
    fn chained_casts() {
        // `x as Float as Int` parses left-associatively: (x as Float) as Int
        let expr = parser("x as Float as Int").parse_expr().unwrap();
        let ExprKind::Cast { expr: inner, target } = expr.kind else {
            panic!("expected outer cast");
        };
        assert!(matches!(target.kind, TypeExprKind::Int));
        let ExprKind::Cast { target: inner_target, .. } = &inner.kind else {
            panic!("expected inner cast");
        };
        assert!(matches!(inner_target.kind, TypeExprKind::Float));
    }

    #[test]
    fn cast_on_call_result() {
        // `foo() as Int` — the call's result is what's cast
        let expr = parser("foo() as Int").parse_expr().unwrap();
        let ExprKind::Cast { expr: inner, .. } = expr.kind else {
            panic!("expected cast");
        };
        assert!(matches!(inner.kind, ExprKind::Call { .. }));
    }

    #[test]
    fn cast_in_declaration() {
        let decl = parser("let x = 1.5 as Int;").parse_declaration().unwrap();
        assert_eq!(decl.name, "x");
        let ExprKind::Cast { expr: inner, target } = &decl.value.kind else {
            panic!("expected cast as declaration value");
        };
        assert!(matches!(inner.kind, ExprKind::Const(Const::Float(_))));
        assert!(matches!(target.kind, TypeExprKind::Int));
    }

    #[test]
    fn cast_to_function_type() {
        // Casts to function types should at least parse, even if the typechecker
        // later rejects them.
        let expr = parser("x as () -> Int").parse_expr().unwrap();
        let ExprKind::Cast { target, .. } = expr.kind else {
            panic!("expected cast");
        };
        assert!(matches!(target.kind, TypeExprKind::Function { .. }));
    }

    #[test]
    fn parses_let_mut() {
        let decl = parser("let mut x = 5;").parse_declaration().unwrap();
        assert!(decl.mutable);
        assert_eq!(decl.name, "x");
    }

    #[test]
    fn parses_let_immutable_by_default() {
        let decl = parser("let x = 5;").parse_declaration().unwrap();
        assert!(!decl.mutable);
    }

    #[test]
    fn parses_assignment() {
        let expr = parser("x = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected assignment");
        };
        assert_eq!(target, "x");
        assert!(matches!(value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn assignment_is_right_associative() {
        // `x = y = 5` → Assign(x, Assign(y, 5))
        let expr = parser("x = y = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected outer assignment");
        };
        assert_eq!(target, "x");
        let ExprKind::Assign { target: inner, .. } = &value.kind else {
            panic!("expected inner assignment");
        };
        assert_eq!(inner, "y");
    }

    #[test]
    fn assignment_lower_precedence_than_arithmetic() {
        // `x = 1 + 2` → Assign(x, Add(1, 2)), NOT (x = 1) + 2
        let expr = parser("x = 1 + 2").parse_expr().unwrap();
        let ExprKind::Assign { value, .. } = expr.kind else {
            panic!("expected assignment");
        };
        assert!(matches!(value.kind, ExprKind::Binary { .. }));
    }

    #[test]
    fn assign_in_block_statement() {
        let block = parser("{ x = 5; }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Expr(expr) = &stmt.kind else {
            panic!("expected expression statement");
        };
        assert!(matches!(expr.kind, ExprKind::Assign { .. }));
    }

    #[test]
    fn assignment_to_non_identifier_errors() {
        // `1 + 2 = 3` should error — LHS isn't an identifier
        let result = parser("1 + 2 = 3").parse_expr();
        assert!(result.is_err());
    }
}
