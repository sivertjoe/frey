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
        let ExprKind::Cast {
            expr: inner,
            target,
        } = expr.kind
        else {
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
        let ExprKind::Cast {
            expr: inner,
            target,
        } = expr.kind
        else {
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
        let ExprKind::Cast {
            expr: inner,
            target,
        } = expr.kind
        else {
            panic!("expected outer cast");
        };
        assert!(matches!(target.kind, TypeExprKind::Int));
        let ExprKind::Cast {
            target: inner_target,
            ..
        } = &inner.kind
        else {
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
        let ExprKind::Cast {
            expr: inner,
            target,
        } = &decl.value.kind
        else {
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

    fn assert_assign_target_named(target: &Expr, expected: &str) {
        let ExprKind::Identifier(name) = &target.kind else {
            panic!(
                "expected identifier assignment target, got {:?}",
                target.kind
            );
        };
        assert_eq!(name, expected);
    }

    #[test]
    fn parses_assignment() {
        let expr = parser("x = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected assignment");
        };
        assert_assign_target_named(&target, "x");
        assert!(matches!(value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn assignment_is_right_associative() {
        // `x = y = 5` → Assign(x, Assign(y, 5))
        let expr = parser("x = y = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected outer assignment");
        };
        assert_assign_target_named(&target, "x");
        let ExprKind::Assign { target: inner, .. } = &value.kind else {
            panic!("expected inner assignment");
        };
        assert_assign_target_named(inner, "y");
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

    // ----------------------------------------------------------------------
    // Array tests — array literals, subscript, and `[T; N]` types.
    // ----------------------------------------------------------------------

    // ---- Array types ----

    #[test]
    fn parses_array_type() {
        // `[Int; 3]` → Array { element_ty: Int, count: 3 }
        let ty = parser("[Int; 3]").parse_type().unwrap();
        let TypeExprKind::Array { element_ty, count } = ty.kind else {
            panic!("expected array type");
        };
        assert!(matches!(element_ty.kind, TypeExprKind::Int));
        assert_eq!(count, 3);
    }

    #[test]
    fn parses_array_type_with_float_element() {
        let ty = parser("[Float; 5]").parse_type().unwrap();
        let TypeExprKind::Array { element_ty, count } = ty.kind else {
            panic!("expected array type");
        };
        assert!(matches!(element_ty.kind, TypeExprKind::Float));
        assert_eq!(count, 5);
    }

    #[test]
    fn parses_nested_array_type() {
        // `[[Int; 2]; 3]` — array-of-arrays
        let ty = parser("[[Int; 2]; 3]").parse_type().unwrap();
        let TypeExprKind::Array { element_ty, count } = ty.kind else {
            panic!("expected outer array type");
        };
        assert_eq!(count, 3);
        let TypeExprKind::Array {
            element_ty: inner,
            count: inner_n,
        } = element_ty.kind
        else {
            panic!("expected inner array type");
        };
        assert!(matches!(inner.kind, TypeExprKind::Int));
        assert_eq!(inner_n, 2);
    }

    #[test]
    fn parses_function_returning_array() {
        // `() -> [Float; 5]` — array as a function's return type
        let ty = parser("() -> [Float; 5]").parse_type().unwrap();
        let TypeExprKind::Function { return_ty, .. } = ty.kind else {
            panic!("expected function type");
        };
        let TypeExprKind::Array { element_ty, count } = return_ty.kind else {
            panic!("expected array return type");
        };
        assert!(matches!(element_ty.kind, TypeExprKind::Float));
        assert_eq!(count, 5);
    }

    // ---- Array literals ----

    #[test]
    fn parses_array_literal() {
        // `[1, 2, 3]` → ExprKind::Array(3 elements)
        let expr = parser("[1, 2, 3]").parse_expr().unwrap();
        let ExprKind::Array(items) = expr.kind else {
            panic!("expected array literal");
        };
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0].kind, ExprKind::Const(Const::Int(1))));
        assert!(matches!(items[1].kind, ExprKind::Const(Const::Int(2))));
        assert!(matches!(items[2].kind, ExprKind::Const(Const::Int(3))));
    }

    #[test]
    fn parses_singleton_array_literal() {
        let expr = parser("[42]").parse_expr().unwrap();
        let ExprKind::Array(items) = expr.kind else {
            panic!("expected array literal");
        };
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0].kind, ExprKind::Const(Const::Int(42))));
    }

    #[test]
    fn parses_array_literal_in_declaration() {
        // `let xs = [1, 2, 3];`
        let decl = parser("let xs = [1, 2, 3];").parse_declaration().unwrap();
        assert_eq!(decl.name, "xs");
        let ExprKind::Array(items) = &decl.value.kind else {
            panic!("expected array literal as declaration value");
        };
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn parses_heterogeneous_array_literal_at_parse_time() {
        // `[1, 2.0]` — parser accepts it; typechecker will reject later.
        let expr = parser("[1, 2.0]").parse_expr().unwrap();
        let ExprKind::Array(items) = expr.kind else {
            panic!("expected array literal");
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0].kind, ExprKind::Const(Const::Int(_))));
        assert!(matches!(items[1].kind, ExprKind::Const(Const::Float(_))));
    }

    // ---- Subscript ----

    #[test]
    fn parses_subscript() {
        // `foo[0]` → Subscript(foo, 0)
        let expr = parser("foo[0]").parse_expr().unwrap();
        let ExprKind::Subscript { expr: inner, index } = expr.kind else {
            panic!("expected subscript, got {:?}", expr.kind);
        };
        let ExprKind::Identifier(name) = &inner.kind else {
            panic!("expected identifier inside subscript");
        };
        assert_eq!(name, "foo");
        assert!(matches!(index.kind, ExprKind::Const(Const::Int(0))));
    }

    #[test]
    fn parses_nested_subscript() {
        // `m[i][j]` → Subscript(Subscript(m, i), j)
        let expr = parser("m[i][j]").parse_expr().unwrap();
        let ExprKind::Subscript {
            expr: outer_target,
            index: outer_idx,
        } = expr.kind
        else {
            panic!("expected outer subscript");
        };
        let ExprKind::Identifier(j) = &outer_idx.kind else {
            panic!("expected identifier `j`");
        };
        assert_eq!(j, "j");
        let ExprKind::Subscript {
            expr: inner_target,
            index: inner_idx,
        } = &outer_target.kind
        else {
            panic!("expected inner subscript");
        };
        let ExprKind::Identifier(m) = &inner_target.kind else {
            panic!("expected identifier `m`");
        };
        assert_eq!(m, "m");
        let ExprKind::Identifier(i) = &inner_idx.kind else {
            panic!("expected identifier `i`");
        };
        assert_eq!(i, "i");
    }

    #[test]
    fn subscript_binds_tighter_than_unary() {
        // `-arr[0]` should parse as `-(arr[0])`, NOT `(-arr)[0]`.
        let expr = parser("-arr[0]").parse_expr().unwrap();
        let ExprKind::Unary { op, expr: inner } = expr.kind else {
            panic!("expected unary at top, got {:?}", expr.kind);
        };
        assert!(matches!(op, UnaryOperator::Minus));
        assert!(matches!(inner.kind, ExprKind::Subscript { .. }));
    }

    #[test]
    fn subscript_on_call_result() {
        // `foo()[0]` — postfix chain: subscript over call result.
        let expr = parser("foo()[0]").parse_expr().unwrap();
        let ExprKind::Subscript { expr: target, .. } = expr.kind else {
            panic!("expected subscript at top");
        };
        assert!(matches!(target.kind, ExprKind::Call { .. }));
    }

    #[test]
    fn call_on_subscript_result() {
        // `funcs[0]()` — postfix chain: call over subscript result.
        let expr = parser("funcs[0]()").parse_expr().unwrap();
        let ExprKind::Call { callee, .. } = expr.kind else {
            panic!("expected call at top");
        };
        assert!(matches!(callee.kind, ExprKind::Subscript { .. }));
    }

    #[test]
    fn subscript_binds_tighter_than_binary() {
        // `arr[0] + 1` parses as `(arr[0]) + 1`.
        let expr = parser("arr[0] + 1").parse_expr().unwrap();
        let ExprKind::Binary { op, lhs, .. } = expr.kind else {
            panic!("expected binary at top");
        };
        assert_eq!(op, BinaryOperator::Add);
        assert!(matches!(lhs.kind, ExprKind::Subscript { .. }));
    }

    #[test]
    fn subscript_index_can_be_expression() {
        // `arr[i + 1]` — index is an arbitrary expression.
        let expr = parser("arr[i + 1]").parse_expr().unwrap();
        let ExprKind::Subscript { index, .. } = expr.kind else {
            panic!("expected subscript");
        };
        assert!(matches!(index.kind, ExprKind::Binary { .. }));
    }

    // ---- Indexed assignment ----

    #[test]
    fn parses_subscript_assignment() {
        // `arr[0] = 5` — assignment to a subscript place expression.
        let expr = parser("arr[0] = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected assignment");
        };
        let ExprKind::Subscript { expr: arr, index } = &target.kind else {
            panic!("expected subscript on lhs of `=`");
        };
        let ExprKind::Identifier(name) = &arr.kind else {
            panic!("expected identifier inside subscript");
        };
        assert_eq!(name, "arr");
        assert!(matches!(index.kind, ExprKind::Const(Const::Int(0))));
        assert!(matches!(value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn parses_nested_subscript_assignment() {
        // `m[i][j] = 0` — assignment to a chained subscript.
        let expr = parser("m[i][j] = 0").parse_expr().unwrap();
        let ExprKind::Assign { target, .. } = expr.kind else {
            panic!("expected assignment");
        };
        let ExprKind::Subscript { expr: outer, .. } = &target.kind else {
            panic!("expected outer subscript as lhs");
        };
        assert!(matches!(outer.kind, ExprKind::Subscript { .. }));
    }

    #[test]
    fn assignment_to_call_result_errors() {
        // `foo() = 5` — call result isn't a place expression.
        let result = parser("foo() = 5").parse_expr();
        assert!(result.is_err());
    }

    // ---- Pointer types ----

    #[test]
    fn parses_ptr_type() {
        let ty = parser("*Int").parse_type().unwrap();
        let TypeExprKind::Ptr(inner) = ty.kind else {
            panic!("expected pointer type, got {:?}", ty.kind);
        };
        assert!(matches!(inner.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_nested_ptr_type() {
        let ty = parser("**u8").parse_type().unwrap();
        let TypeExprKind::Ptr(inner) = ty.kind else {
            panic!("expected outer pointer");
        };
        let TypeExprKind::Ptr(innermost) = inner.kind else {
            panic!("expected inner pointer");
        };
        assert!(matches!(innermost.kind, TypeExprKind::U8));
    }

    #[test]
    fn parses_ptr_to_array_type() {
        let ty = parser("*[Int; 4]").parse_type().unwrap();
        let TypeExprKind::Ptr(inner) = ty.kind else {
            panic!("expected pointer");
        };
        let TypeExprKind::Array { element_ty, count } = inner.kind else {
            panic!("expected array as pointee");
        };
        assert_eq!(count, 4);
        assert!(matches!(element_ty.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_function_taking_ptr() {
        let ty = parser("(*u8) -> Int").parse_type().unwrap();
        let TypeExprKind::Function { params, return_ty } = ty.kind else {
            panic!("expected function type");
        };
        assert_eq!(params.len(), 1);
        let TypeExprKind::Ptr(inner) = &params[0].kind else {
            panic!("expected pointer param");
        };
        assert!(matches!(inner.kind, TypeExprKind::U8));
        assert!(matches!(return_ty.kind, TypeExprKind::Int));
    }

    // ---- Address-of (&) ----

    #[test]
    fn parses_ref_of_identifier() {
        let expr = parser("&x").parse_expr().unwrap();
        let ExprKind::Ref(target) = expr.kind else {
            panic!("expected ref, got {:?}", expr.kind);
        };
        let ExprKind::Identifier(name) = &target.kind else {
            panic!("expected identifier inside ref");
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn parses_ref_of_subscript() {
        let expr = parser("&a[0]").parse_expr().unwrap();
        let ExprKind::Ref(target) = expr.kind else {
            panic!("expected ref");
        };
        assert!(matches!(target.kind, ExprKind::Subscript { .. }));
    }

    #[test]
    fn binary_bitand_still_parses() {
        // Make sure adding unary `&` didn't break `a & b`.
        let expr = parser("a & b").parse_expr().unwrap();
        let ExprKind::Binary { op, .. } = expr.kind else {
            panic!("expected binary, got {:?}", expr.kind);
        };
        assert_eq!(op, BinaryOperator::BitAnd);
    }

    // ---- Dereference (*) ----

    #[test]
    fn parses_deref_of_identifier() {
        let expr = parser("*p").parse_expr().unwrap();
        let ExprKind::Deref(target) = expr.kind else {
            panic!("expected deref, got {:?}", expr.kind);
        };
        let ExprKind::Identifier(name) = &target.kind else {
            panic!("expected identifier inside deref");
        };
        assert_eq!(name, "p");
    }

    #[test]
    fn parses_double_deref() {
        let expr = parser("**p").parse_expr().unwrap();
        let ExprKind::Deref(outer) = expr.kind else {
            panic!("expected outer deref");
        };
        let ExprKind::Deref(_) = outer.kind else {
            panic!("expected inner deref");
        };
    }

    #[test]
    fn parses_deref_in_arithmetic() {
        // `*p + 1` — deref binds tighter than `+`.
        let expr = parser("*p + 1").parse_expr().unwrap();
        let ExprKind::Binary { op, lhs, .. } = expr.kind else {
            panic!("expected binary");
        };
        assert_eq!(op, BinaryOperator::Add);
        assert!(matches!(lhs.kind, ExprKind::Deref(_)));
    }

    #[test]
    fn parses_assign_through_deref() {
        // `*p = 5` is a valid assignment (deref is a place).
        let expr = parser("*p = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected assignment, got {:?}", expr.kind);
        };
        assert!(matches!(target.kind, ExprKind::Deref(_)));
        assert!(matches!(value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn binary_mul_still_parses() {
        // Make sure adding unary `*` didn't break `a * b`.
        let expr = parser("a * b").parse_expr().unwrap();
        let ExprKind::Binary { op, .. } = expr.kind else {
            panic!("expected binary, got {:?}", expr.kind);
        };
        assert_eq!(op, BinaryOperator::Mul);
    }

    #[test]
    fn ref_and_deref_round_trip() {
        // `*&x` — taking the address and dereferencing right back.
        let expr = parser("*&x").parse_expr().unwrap();
        let ExprKind::Deref(inner) = expr.kind else {
            panic!("expected outer deref");
        };
        assert!(matches!(inner.kind, ExprKind::Ref(_)));
    }

    // ---- Structs ----

    #[test]
    fn parses_named_type() {
        let ty = parser("Point").parse_type().unwrap();
        let TypeExprKind::Named(name) = ty.kind else {
            panic!("expected named type, got {:?}", ty.kind);
        };
        assert_eq!(name, "Point");
    }

    #[test]
    fn parses_struct_def() {
        let decl = parser("let Point = struct { x: Int, y: Int };")
            .parse_declaration()
            .unwrap();
        assert_eq!(decl.name, "Point");
        let ExprKind::StructDef { fields, .. } = decl.value.kind else {
            panic!("expected struct def, got {:?}", decl.value.kind);
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert!(matches!(fields[0].ty.kind, TypeExprKind::Int));
        assert_eq!(fields[1].name, "y");
        assert!(matches!(fields[1].ty.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_empty_struct_def() {
        let decl = parser("let Empty = struct {};")
            .parse_declaration()
            .unwrap();
        let ExprKind::StructDef { fields, .. } = decl.value.kind else {
            panic!("expected struct def");
        };
        assert!(fields.is_empty());
    }

    #[test]
    fn parses_struct_def_with_trailing_comma() {
        let decl = parser("let Point = struct { x: Int, y: Int, };")
            .parse_declaration()
            .unwrap();
        let ExprKind::StructDef { fields, .. } = decl.value.kind else {
            panic!("expected struct def");
        };
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn parses_recursive_struct_type() {
        // The parser doesn't validate type names; that's the lowering pass.
        let decl = parser("let Node = struct { value: Int, next: *Node };")
            .parse_declaration()
            .unwrap();
        let ExprKind::StructDef { fields, .. } = decl.value.kind else {
            panic!("expected struct def");
        };
        assert_eq!(fields[1].name, "next");
        let TypeExprKind::Ptr(inner) = &fields[1].ty.kind else {
            panic!("expected pointer type for next");
        };
        assert!(matches!(inner.kind, TypeExprKind::Named(ref n) if n == "Node"));
    }

    #[test]
    fn parses_struct_literal() {
        let expr = parser("Point { x: 1, y: 2 }").parse_expr().unwrap();
        let ExprKind::StructLiteral { name, fields } = expr.kind else {
            panic!("expected struct literal, got {:?}", expr.kind);
        };
        assert_eq!(name, "Point");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert!(matches!(
            fields[0].value.kind,
            ExprKind::Const(Const::Int(1))
        ));
        assert_eq!(fields[1].name, "y");
        assert!(matches!(
            fields[1].value.kind,
            ExprKind::Const(Const::Int(2))
        ));
    }

    #[test]
    fn parses_empty_struct_literal() {
        let expr = parser("Empty {}").parse_expr().unwrap();
        let ExprKind::StructLiteral { name, fields } = expr.kind else {
            panic!("expected struct literal");
        };
        assert_eq!(name, "Empty");
        assert!(fields.is_empty());
    }

    #[test]
    fn parses_struct_literal_with_trailing_comma() {
        let expr = parser("Point { x: 1, y: 2, }").parse_expr().unwrap();
        let ExprKind::StructLiteral { fields, .. } = expr.kind else {
            panic!("expected struct literal");
        };
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn parses_field_access() {
        let expr = parser("p.x").parse_expr().unwrap();
        let ExprKind::Field { target, name } = expr.kind else {
            panic!("expected field access, got {:?}", expr.kind);
        };
        assert_eq!(name, "x");
        let ExprKind::Identifier(target_name) = &target.kind else {
            panic!("expected identifier target");
        };
        assert_eq!(target_name, "p");
    }

    #[test]
    fn parses_nested_field_access() {
        // `p.pos.x` → (p.pos).x
        let expr = parser("p.pos.x").parse_expr().unwrap();
        let ExprKind::Field { target, name } = expr.kind else {
            panic!("expected outer field");
        };
        assert_eq!(name, "x");
        assert!(matches!(target.kind, ExprKind::Field { .. }));
    }

    #[test]
    fn parses_field_access_in_arithmetic() {
        // `p.x + 1` — field access binds tighter than `+`.
        let expr = parser("p.x + 1").parse_expr().unwrap();
        let ExprKind::Binary { op, lhs, .. } = expr.kind else {
            panic!("expected binary");
        };
        assert_eq!(op, BinaryOperator::Add);
        assert!(matches!(lhs.kind, ExprKind::Field { .. }));
    }

    #[test]
    fn parses_field_of_call_result() {
        // `foo().x` — field access on a non-place is still valid syntax.
        let expr = parser("foo().x").parse_expr().unwrap();
        let ExprKind::Field { target, name } = expr.kind else {
            panic!("expected field access");
        };
        assert_eq!(name, "x");
        assert!(matches!(target.kind, ExprKind::Call { .. }));
    }

    #[test]
    fn parses_assign_to_field() {
        let expr = parser("p.x = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, value } = expr.kind else {
            panic!("expected assign");
        };
        assert!(matches!(target.kind, ExprKind::Field { .. }));
        assert!(matches!(value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn parses_nested_struct_literal() {
        let expr = parser("Outer { inner: Inner { a: 1 } }")
            .parse_expr()
            .unwrap();
        let ExprKind::StructLiteral { name, fields } = expr.kind else {
            panic!("expected outer struct literal");
        };
        assert_eq!(name, "Outer");
        assert_eq!(fields.len(), 1);
        assert!(matches!(
            fields[0].value.kind,
            ExprKind::StructLiteral { .. }
        ));
    }

    #[test]
    fn struct_literal_with_complex_field_values() {
        // Each field value can be any expression.
        let expr = parser("Point { x: 1 + 2, y: foo() }").parse_expr().unwrap();
        let ExprKind::StructLiteral { fields, .. } = expr.kind else {
            panic!("expected struct literal");
        };
        assert!(matches!(fields[0].value.kind, ExprKind::Binary { .. }));
        assert!(matches!(fields[1].value.kind, ExprKind::Call { .. }));
    }

    #[test]
    fn identifier_not_followed_by_struct_lit_pattern() {
        // `Point { x }` — there's no `:` after the identifier, so this is
        // NOT a struct literal. The `{` is a block start, which would only
        // be valid in places that accept a block. As an expression in
        // isolation this should still NOT eat the `{` into a struct lit.
        // We verify by parsing just the identifier — `Point` alone.
        let mut p = parser("Point { x }");
        let expr = p.parse_expr().unwrap();
        assert!(
            matches!(expr.kind, ExprKind::Identifier(ref n) if n == "Point"),
            "expected bare identifier (no struct lit), got {:?}",
            expr.kind
        );
    }

    #[test]
    fn parses_field_assign_through_subscript() {
        // `arr[0].x = 5` — chain places.
        let expr = parser("arr[0].x = 5").parse_expr().unwrap();
        let ExprKind::Assign { target, .. } = expr.kind else {
            panic!("expected assign");
        };
        let ExprKind::Field { target: inner, .. } = &target.kind else {
            panic!("expected field on subscript");
        };
        assert!(matches!(inner.kind, ExprKind::Subscript { .. }));
    }

    #[test]
    fn parses_function_taking_named_struct() {
        let ty = parser("(Point) -> Int").parse_type().unwrap();
        let TypeExprKind::Function { params, .. } = ty.kind else {
            panic!("expected function type");
        };
        assert_eq!(params.len(), 1);
        assert!(matches!(params[0].kind, TypeExprKind::Named(ref n) if n == "Point"));
    }

    // ---- Generic struct syntax ----

    #[test]
    fn parses_generic_struct_def() {
        let decl = parser("let Box = struct<$T> { value: T };")
            .parse_declaration()
            .unwrap();
        let ExprKind::StructDef {
            type_params,
            fields,
        } = decl.value.kind
        else {
            panic!("expected struct def");
        };
        assert_eq!(type_params, vec!["T".to_string()]);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "value");
        assert!(matches!(fields[0].ty.kind, TypeExprKind::Named(ref n) if n == "T"));
    }

    #[test]
    fn parses_multi_param_generic_struct() {
        let decl = parser("let Pair = struct<$A, $B> { fst: A, snd: B };")
            .parse_declaration()
            .unwrap();
        let ExprKind::StructDef { type_params, .. } = decl.value.kind else {
            panic!("expected struct def");
        };
        assert_eq!(type_params, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn parses_generic_type_use_site() {
        let ty = parser("Vec<Int>").parse_type().unwrap();
        let TypeExprKind::NamedGeneric { name, args } = ty.kind else {
            panic!("expected NamedGeneric, got {:?}", ty.kind);
        };
        assert_eq!(name, "Vec");
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0].kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_multi_arg_generic_type_use() {
        let ty = parser("Map<Int, u8>").parse_type().unwrap();
        let TypeExprKind::NamedGeneric { name, args } = ty.kind else {
            panic!("expected NamedGeneric");
        };
        assert_eq!(name, "Map");
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0].kind, TypeExprKind::Int));
        assert!(matches!(args[1].kind, TypeExprKind::U8));
    }

    #[test]
    fn parses_pointer_to_named_generic_type() {
        let ty = parser("*Vec<Int>").parse_type().unwrap();
        let TypeExprKind::Ptr(inner) = ty.kind else {
            panic!("expected pointer");
        };
        assert!(matches!(inner.kind, TypeExprKind::NamedGeneric { .. }));
    }

    // ---- Pipe operator ----

    #[test]
    fn pipe_no_arg_call() {
        // `5 |> foo()` desugars to `foo(5)`.
        let expr = parser("5 |> foo()").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call, got {:?}", expr.kind);
        };
        assert_callee_named(&callee, "foo");
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0].kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn pipe_with_existing_args() {
        // `5 |> add(3)` desugars to `add(5, 3)`.
        let expr = parser("5 |> add(3)").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call");
        };
        assert_callee_named(&callee, "add");
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0].kind, ExprKind::Const(Const::Int(5))));
        assert!(matches!(args[1].kind, ExprKind::Const(Const::Int(3))));
    }

    #[test]
    fn pipe_chains_left_associative() {
        // `5 |> f() |> g()` desugars to `g(f(5))`.
        let expr = parser("5 |> f() |> g()").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected outer call");
        };
        assert_callee_named(&callee, "g");
        assert_eq!(args.len(), 1);
        let ExprKind::Call {
            callee: inner_callee,
            args: inner_args,
        } = &args[0].kind
        else {
            panic!("expected inner call");
        };
        assert_callee_named(inner_callee, "f");
        assert_eq!(inner_args.len(), 1);
        assert!(matches!(inner_args[0].kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn pipe_lhs_takes_full_arithmetic() {
        // `2 + 3 |> f()` desugars to `f(2 + 3)`, not `2 + f(3)`.
        let expr = parser("2 + 3 |> f()").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call");
        };
        assert_callee_named(&callee, "f");
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0].kind, ExprKind::Binary { .. }));
    }

    #[test]
    fn pipe_rhs_must_be_call() {
        let result = parser("5 |> 7").parse_expr();
        assert!(result.is_err());
    }

    #[test]
    fn pipe_rhs_bare_identifier_errors() {
        let result = parser("5 |> foo").parse_expr();
        assert!(result.is_err());
    }

    // ---- While + break ----

    #[test]
    fn parses_while_expression() {
        let expr = parser("while 1 { }").parse_expr().unwrap();
        let ExprKind::While { condition, body } = expr.kind else {
            panic!("expected while, got {:?}", expr.kind);
        };
        assert!(matches!(condition.kind, ExprKind::Const(Const::Int(1))));
        assert!(body.items.is_empty());
    }

    #[test]
    fn parses_while_with_body_and_break() {
        let expr = parser("while 1 { break; }").parse_expr().unwrap();
        let ExprKind::While { body, .. } = expr.kind else {
            panic!("expected while");
        };
        assert_eq!(body.items.len(), 1);
        let BlockItem::Statement(s) = &body.items[0] else {
            panic!("expected statement");
        };
        assert!(matches!(s.kind, StatementKind::Break));
    }

    #[test]
    fn parses_while_with_condition_using_comparison() {
        // Make sure `while i < 10 { }` parses with `<` as comparison, not as
        // some lexer weirdness.
        let expr = parser("while i < 10 { }").parse_expr().unwrap();
        let ExprKind::While { condition, .. } = expr.kind else {
            panic!("expected while");
        };
        assert!(matches!(condition.kind, ExprKind::Binary { .. }));
    }

    // ---- String literals ----

    #[test]
    fn parses_string_literal() {
        let expr = parser("\"hello\"").parse_expr().unwrap();
        let ExprKind::Const(Const::Str(s)) = expr.kind else {
            panic!("expected string literal, got {:?}", expr.kind);
        };
        assert_eq!(s, "hello");
    }

    #[test]
    fn parses_string_in_declaration() {
        let decl = parser("let s = \"hi\";").parse_declaration().unwrap();
        let ExprKind::Const(Const::Str(s)) = decl.value.kind else {
            panic!("expected string literal");
        };
        assert_eq!(s, "hi");
    }

    #[test]
    fn pipe_into_method_chain() {
        // `5 |> foo.bar(3)` desugars to `foo.bar(5, 3)` — the call we splice
        // into is the outermost one, even if the callee itself is a field
        // access expression.
        let expr = parser("5 |> foo.bar(3)").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call");
        };
        // callee is foo.bar, a Field expression
        assert!(matches!(callee.kind, ExprKind::Field { .. }));
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn parses_cast_to_generic_type() {
        let expr = parser("x as $T").parse_expr().unwrap();

        let ExprKind::Cast {
            expr: inner,
            target,
        } = expr.kind
        else {
            panic!("expected cast");
        };

        assert!(matches!(inner.kind, ExprKind::Identifier(ref n) if n == "x"));
        assert!(matches!(target.kind, TypeExprKind::Named(ref n) if n == "$T"));
    }

    #[test]
    fn parses_array_of_generic_type() {
        let ty = parser("[$T; 3]").parse_type().unwrap();

        let TypeExprKind::Array { element_ty, count } = ty.kind else {
            panic!("expected array type");
        };

        assert_eq!(count, 3);
        assert!(matches!(element_ty.kind, TypeExprKind::Named(ref n) if n == "$T"));
    }

    #[test]
    fn parses_pointer_to_generic_type() {
        let ty = parser("*$T").parse_type().unwrap();

        let TypeExprKind::Ptr(inner) = ty.kind else {
            panic!("expected pointer type");
        };

        assert!(matches!(inner.kind, TypeExprKind::Named(ref n) if n == "$T"));
    }
    #[test]
    fn parses_declaration_with_multiple_generic_function_params() {
        let decl = parser("let bar = (foo: $T, baz: $U, foz: T) { };")
            .parse_declaration()
            .unwrap();

        assert_eq!(decl.name, "bar");

        let ExprKind::Function {
            params, return_ty, ..
        } = decl.value.kind
        else {
            panic!("expected function literal");
        };

        assert_eq!(params.len(), 3);

        assert_eq!(params[0].name, "foo");
        assert!(matches!(params[0].ty.kind, TypeExprKind::Named(ref n) if n == "$T"));

        assert_eq!(params[1].name, "baz");
        assert!(matches!(params[1].ty.kind, TypeExprKind::Named(ref n) if n == "$U"));

        assert_eq!(params[2].name, "foz");
        assert!(matches!(params[2].ty.kind, TypeExprKind::Named(ref n) if n == "T"));

        assert!(return_ty.is_none());
    }
}
