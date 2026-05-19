#[cfg(test)]
mod tests {
    use crate::ast::parser::Parser;
    use crate::ast::types::{
        BlockItem, Const, Declaration, Expr, ExprKind, StatementKind, TypeExpr, TypeExprKind,
    };
    use crate::lexer::tokenize;

    fn parser(src: &str) -> Parser {
        let tokens = tokenize(src).unwrap();
        Parser::new(tokens)
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
    fn parses_function_literal_with_named_params() {
        let expr = parser("(x: Int, y: Int) -> Int { return 0; }")
            .parse_expr()
            .unwrap();
        let ExprKind::Function {
            params,
            return_ty,
            ..
        } = expr.kind
        else {
            panic!("expected function literal");
        };
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "x");
        assert!(matches!(params[0].ty.kind, TypeExprKind::Int));
        assert_eq!(params[1].name, "y");
        assert!(matches!(params[1].ty.kind, TypeExprKind::Int));
        assert!(matches!(return_ty.kind, TypeExprKind::Int));
    }

    #[test]
    fn parses_function_literal_with_higher_order_param() {
        // param type is itself a function type
        let expr = parser("(f: (Int) -> Int) -> Int { return 0; }")
            .parse_expr()
            .unwrap();
        let ExprKind::Function { params, .. } = expr.kind else {
            panic!("expected function literal");
        };
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "f");
        let TypeExprKind::Function {
            params: inner_params,
            return_ty,
        } = &params[0].ty.kind
        else {
            panic!("expected function type for param");
        };
        assert_eq!(inner_params.len(), 1);
        assert!(matches!(inner_params[0].kind, TypeExprKind::Int));
        assert!(matches!(return_ty.kind, TypeExprKind::Int));
    }

    #[test]
    fn param_missing_colon_is_error() {
        let err = parser("(x Int) -> Int { return 0; }")
            .parse_expr()
            .unwrap_err();
        let msg = err.kind.to_string();
        assert!(msg.contains("`:`"), "got: {msg}");
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
    fn type_error_on_unexpected_token() {
        let err = parser("=").parse_type().unwrap_err();
        let msg = err.kind.to_string();
        assert!(msg.contains("expected type"), "got: {msg}");
    }

    fn assert_callee_named(callee: &Expr, expected: &str) {
        let ExprKind::Identifier(name) = &callee.kind else {
            panic!("expected identifier callee, got {:?}", callee.kind);
        };
        assert_eq!(name, expected);
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
    fn parses_call_multiple_args() {
        let expr = parser("f(1, 2, 3)").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected call");
        };
        assert_callee_named(&callee, "f");
        assert_eq!(args.len(), 3);
        assert!(matches!(args[0].kind, ExprKind::Const(Const::Int(1))));
        assert!(matches!(args[1].kind, ExprKind::Const(Const::Int(2))));
        assert!(matches!(args[2].kind, ExprKind::Const(Const::Int(3))));
    }

    #[test]
    fn parses_call_with_identifier_arg() {
        let expr = parser("f(x)").parse_expr().unwrap();
        let ExprKind::Call { args, .. } = expr.kind else {
            panic!("expected call");
        };
        assert_eq!(args.len(), 1);
        let ExprKind::Identifier(arg_name) = &args[0].kind else {
            panic!("expected identifier arg");
        };
        assert_eq!(arg_name, "x");
    }

    #[test]
    fn parses_nested_call() {
        let expr = parser("f(g())").parse_expr().unwrap();
        let ExprKind::Call { callee, args } = expr.kind else {
            panic!("expected outer call");
        };
        assert_callee_named(&callee, "f");
        assert_eq!(args.len(), 1);
        let ExprKind::Call { callee: inner_callee, args: inner_args } = &args[0].kind else {
            panic!("expected inner call as arg");
        };
        assert_callee_named(inner_callee, "g");
        assert!(inner_args.is_empty());
    }

    #[test]
    fn parses_chained_call() {
        // foo()() — call the result of foo() with no args
        let expr = parser("foo()()").parse_expr().unwrap();
        let ExprKind::Call { callee: outer_callee, args: outer_args } = expr.kind else {
            panic!("expected outer call");
        };
        assert!(outer_args.is_empty());

        // The outer callee is itself a Call(foo, [])
        let ExprKind::Call { callee: inner_callee, args: inner_args } = &outer_callee.kind else {
            panic!("expected inner call as the outer callee");
        };
        assert_callee_named(inner_callee, "foo");
        assert!(inner_args.is_empty());
    }

    #[test]
    fn identifier_without_parens_is_not_a_call() {
        let expr = parser("x").parse_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::Identifier(_)));
    }

    #[test]
    fn parses_identifier_expression() {
        let expr = parser("x").parse_expr().unwrap();
        let ExprKind::Identifier(name) = expr.kind else {
            panic!("expected identifier expression");
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn parses_return_of_identifier() {
        let block = parser("{ return x; }").parse_block().unwrap();
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Return(expr) = &stmt.kind else {
            panic!("expected return");
        };
        let ExprKind::Identifier(name) = &expr.kind else {
            panic!("expected identifier in return");
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn parses_declaration_with_identifier_value() {
        let decl = parser("let y = x;").parse_declaration().unwrap();
        assert_eq!(decl.name, "y");
        let ExprKind::Identifier(name) = &decl.value.kind else {
            panic!("expected identifier value");
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn parses_int_literal_expression() {
        let expr = parser("42").parse_expr().unwrap();
        assert!(matches!(expr.kind, ExprKind::Const(Const::Int(42))));
    }

    #[test]
    fn parses_declaration() {
        let decl = parser("let x = 5;").parse_declaration().unwrap();
        assert_eq!(decl.name, "x");
        assert!(matches!(decl.value.kind, ExprKind::Const(Const::Int(5))));
    }

    #[test]
    fn parses_block_with_return() {
        let block = parser("{ return 7; }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Return(expr) = &stmt.kind else {
            panic!("expected return statement");
        };
        assert!(matches!(expr.kind, ExprKind::Const(Const::Int(7))));
    }

    #[test]
    fn parses_block_with_nested_let() {
        let block = parser("{ let x = 1; return 2; }").parse_block().unwrap();
        assert_eq!(block.items.len(), 2);
        let BlockItem::Declaration(Declaration { name, .. }) = &block.items[0] else {
            panic!("expected declaration");
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn declaration_errors_on_missing_semicolon() {
        let err = parser("let x = 5").parse_declaration().unwrap_err();
        let msg = err.kind.to_string();
        assert!(msg.contains("`;`"), "got: {msg}");
    }

    #[test]
    fn span_of_declaration_covers_let_through_value() {
        let decl = parser("let x = 5;").parse_declaration().unwrap();
        // "let x = 5;" — span runs from `let` (col 1) through the value `5` (col 9)
        assert_eq!(decl.span.start.column, 1);
        assert_eq!(decl.span.end.column, 10);
    }

    #[test]
    fn parse_expr_int_carries_token_span() {
        let expr: Expr = parser("42").parse_expr().unwrap();
        assert_eq!(expr.span.start.column, 1);
        assert_eq!(expr.span.end.column, 3);
    }

    #[test]
    fn parse_type_assigns_fresh_node_ids() {
        let ty: TypeExpr = parser("() -> () -> Int").parse_type().unwrap();
        let TypeExprKind::Function { return_ty, .. } = &ty.kind else {
            unreachable!();
        };
        assert_ne!(ty.id, return_ty.id);
    }

    #[test]
    fn empty_program_is_error() {
        let err = parser("").parse_program().unwrap_err();
        let msg = err.kind.to_string();
        assert!(msg.contains("expected declaration"), "got: {msg}");
    }

    #[test]
    fn parses_function_literal_returning_a_function() {
        let src = "() -> () -> Int { return () -> Int { return 0; }; }";
        let expr = parser(src).parse_expr().unwrap();

        // outer: () -> () -> int { ... }
        let ExprKind::Function {
            params,
            return_ty,
            body,
        } = expr.kind
        else {
            panic!("expected outer function literal");
        };
        assert!(params.is_empty());

        // outer return type: () -> int
        let TypeExprKind::Function {
            return_ty: outer_ret_ret,
            ..
        } = return_ty.kind
        else {
            panic!("expected outer return type to be a function type");
        };
        assert!(matches!(outer_ret_ret.kind, TypeExprKind::Int));

        // outer body: single `return <function-literal>;`
        assert_eq!(body.items.len(), 1);
        let BlockItem::Statement(stmt) = &body.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Return(ret_expr) = &stmt.kind else {
            panic!("expected return statement");
        };

        // returned value: () -> int { return 0; }
        let ExprKind::Function {
            return_ty: inner_ret_ty,
            body: inner_body,
            ..
        } = &ret_expr.kind
        else {
            panic!("expected inner function literal");
        };
        assert!(matches!(inner_ret_ty.kind, TypeExprKind::Int));

        // inner body: return 0;
        assert_eq!(inner_body.items.len(), 1);
        let BlockItem::Statement(inner_stmt) = &inner_body.items[0] else {
            panic!("expected inner statement");
        };
        let StatementKind::Return(inner_ret) = &inner_stmt.kind else {
            panic!("expected inner return");
        };
        assert!(matches!(inner_ret.kind, ExprKind::Const(Const::Int(0))));
    }

    #[test]
    fn test_main_function_with_return() {
        let src = "let main = () -> Int { return 0; };";
        let program = parser(src).parse_program().unwrap();

        assert_eq!(program.declarations.len(), 1);
        let decl = &program.declarations[0];
        assert_eq!(decl.name, "main");

        let ExprKind::Function {
            params,
            return_ty,
            body,
        } = &decl.value.kind
        else {
            panic!("expected function literal as the value of `main`");
        };
        assert!(params.is_empty());
        assert!(matches!(return_ty.kind, TypeExprKind::Int));

        assert_eq!(body.items.len(), 1);
        let BlockItem::Statement(stmt) = &body.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Return(expr) = &stmt.kind else {
            panic!("expected return statement");
        };
        assert!(matches!(expr.kind, ExprKind::Const(Const::Int(0))));
    }

    #[test]
    fn test_main_function() {
        let src = "let main = () -> Int { 0 };";
        let program = parser(src).parse_program().unwrap();

        assert_eq!(program.declarations.len(), 1);
        let decl = &program.declarations[0];
        assert_eq!(decl.name, "main");

        let ExprKind::Function {
            params,
            return_ty,
            body,
        } = &decl.value.kind
        else {
            panic!("expected function literal as the value of `main`");
        };
        assert!(params.is_empty());
        assert!(matches!(return_ty.kind, TypeExprKind::Int));

        // `{ 0 }` — `0` is the tail expression, not a regular item.
        assert!(body.items.is_empty());
        let tail = body.tail.as_ref().expect("expected tail expression");
        assert!(matches!(tail.kind, ExprKind::Const(Const::Int(0))));
    }

    #[test]
    fn parses_block_with_tail_only() {
        let block = parser("{ 7 }").parse_block().unwrap();
        assert!(block.items.is_empty());
        let tail = block.tail.as_ref().expect("expected tail");
        assert!(matches!(tail.kind, ExprKind::Const(Const::Int(7))));
    }

    #[test]
    fn parses_block_with_statement_and_tail() {
        // `let x = 1; x` — one declaration item, then `x` as tail
        let block = parser("{ let x = 1; x }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        let BlockItem::Declaration(Declaration { name, .. }) = &block.items[0] else {
            panic!("expected declaration");
        };
        assert_eq!(name, "x");

        let tail = block.tail.as_ref().expect("expected tail");
        let ExprKind::Identifier(tail_name) = &tail.kind else {
            panic!("expected identifier as tail");
        };
        assert_eq!(tail_name, "x");
    }

    #[test]
    fn parses_bare_expr_statement_with_semicolon() {
        // `x;` — bare expression *with* `;` is a discard statement, not a tail.
        let block = parser("{ x; }").parse_block().unwrap();
        assert_eq!(block.items.len(), 1);
        assert!(block.tail.is_none());
        let BlockItem::Statement(stmt) = &block.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Expr(expr) = &stmt.kind else {
            panic!("expected expression statement");
        };
        assert!(matches!(expr.kind, ExprKind::Identifier(_)));
    }

    #[test]
    fn rejects_two_consecutive_bare_expressions() {
        // `0 a` — `0` is not followed by `;` or `}`, so it can't be a statement
        // or a tail. Should error with `expected \`;\` or \`}\``.
        let err = parser("{ 0 a }").parse_block().unwrap_err();
        let msg = err.kind.to_string();
        assert!(msg.contains("`;`"), "got: {msg}");
        assert!(msg.contains("`}`"), "got: {msg}");
    }
}
