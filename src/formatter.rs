use crate::ast::{Expression, Program, Statement};
use crate::lexer::Token;

/// Render an AST using HackLang's compact canonical syntax.
pub fn format_program(program: &Program) -> String {
    program.statements.iter().map(format_statement).collect::<Vec<_>>().join("\n")
}

fn format_block(statements: &[Statement]) -> String {
    if statements.is_empty() {
        return "{}".to_string();
    }
    let body = statements.iter()
        .map(|statement| format!("  {}", format_statement(statement).replace("\n", "\n  ")))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{{\n{}\n}}", body)
}

fn format_statement(statement: &Statement) -> String {
    match statement {
        Statement::Let(stmt) => format!("{}:={}", stmt.name, format_expression(&stmt.value)),
        Statement::Assign(stmt) => format!("{}={}", stmt.name, format_expression(&stmt.value)),
        Statement::IndexAssign(stmt) => {
            let path = stmt.indices.iter().map(|idx| format!("[{}]", format_expression(idx))).collect::<Vec<_>>().join("");
            format!("{}{}={}", stmt.name, path, format_expression(&stmt.value))
        }
        Statement::Print(stmt) => format!("print {}", format_expression(&stmt.value)),
        Statement::Return(ret) => format!("return {}", format_expression(&ret.value)),
        Statement::Break(_) => "break".to_string(),
        Statement::Continue(_) => "continue".to_string(),
        Statement::Block(block) => format_block(&block.statements),
        Statement::While(stmt) => format!("while {}{}", format_expression(&stmt.condition), format_block(&stmt.body)),
        Statement::If(stmt) => {
            let mut result = format!("if {}{}", format_expression(&stmt.condition), format_block(&stmt.true_body));
            if let Some(body) = &stmt.false_body {
                result.push_str("else");
                result.push_str(&format_block(body));
            }
            result
        }
    }
}

fn format_expression(expression: &Expression) -> String {
    match expression {
        Expression::Identifier { name, .. } => name.clone(),
        Expression::NumberLiteral { value, .. } => value.to_string(),
        Expression::BoolLiteral { value, .. } => value.to_string(),
        Expression::StringLiteral { value, .. } => format!("\"{}\"", value),
        Expression::Infix { left, operator, right, .. } => {
            format!("({}{}{})", format_expression(left), operator_text(operator), format_expression(right))
        }
        Expression::Ternary { condition, true_branch, false_branch, .. } => {
            format!("({}?{}:{})", format_expression(condition), format_expression(true_branch), format_expression(false_branch))
        }
        Expression::Lambda { params, body, .. } => {
            format!("|{}|{}", params.join(","), format_expression(body))
        }
        Expression::Block { statements, .. } => format_block(statements),
        Expression::FunctionCall { func, args, .. } => {
            format!("{}({})", format_expression(func), args.iter().map(format_expression).collect::<Vec<_>>().join(","))
        }
        Expression::Array { elements, .. } => {
            format!("[{}]", elements.iter().map(format_expression).collect::<Vec<_>>().join(","))
        }
        Expression::Map { entries, .. } => {
            let entries = entries.iter().map(|(key, value)| format!("{}:{}", format_expression(key), format_expression(value))).collect::<Vec<_>>().join(",");
            format!("@{{{}}}", entries)
        }
        Expression::Index { target, index, .. } => {
            format!("{}[{}]", format_expression(target), format_expression(index))
        }
    }
}

fn operator_text(operator: &Token) -> &'static str {
    match operator {
        Token::Plus => "+",
        Token::Minus => "-",
        Token::Star => "*",
        Token::Slash => "/",
        Token::Less => "<",
        Token::Greater => ">",
        Token::Tilde => "~",
        Token::Caret => "^",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(source: &str) -> String {
        let mut lexer = crate::lexer::Lexer::new(source.to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new_compact(tokens);
        let program = parser.parse_program().unwrap();
        let formatted = format_program(&program);
        let mut lexer = crate::lexer::Lexer::new(formatted.clone());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new_compact(tokens);
        parser.parse_program().unwrap();
        formatted
    }

    #[test]
    fn formats_compact_declarations_and_assignments() {
        let formatted = round_trip("n:=2 n=n+1");
        assert_eq!(formatted, "n:=2\nn=(n+1)");
    }

    #[test]
    fn formats_compact_control_flow_and_lambda_blocks() {
        let formatted = round_trip("f:=|x|{if x>0{return x}else{return 0}}");
        assert!(formatted.contains("if (x>0)"));
        assert!(formatted.contains("return x"));
    }

    #[test]
    fn formats_compact_arrays_and_maps() {
        let formatted = round_trip("xs:=[1,2] data:=@{\"a\":1}");
        assert!(formatted.contains("[1,2]"));
        assert!(formatted.contains("@{\"a\":1}"));
    }
}
