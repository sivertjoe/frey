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

        assert_eq!(body.items.len(), 1);
        let BlockItem::Statement(stmt) = &body.items[0] else {
            panic!("expected statement");
        };
        let StatementKind::Expr(expr) = &stmt.kind else {
            panic!("expected bare-expression statement");
        };
        assert!(matches!(expr.kind, ExprKind::Const(Const::Int(0))));
    }
}
