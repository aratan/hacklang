// src/evaluador.rs
// Versión: rewrite applied
use crate::ast::{Expression, Program, Statement, LetStatement, PrintStatement};
use crate::lexer::Token;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    String(String),
}

impl Value {
    fn as_bool(&self) -> bool {
        match self {
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
        }
    }
}

#[derive(Debug)]
pub struct Evaluator {
    variables: HashMap<String, Value>,
}

impl Evaluator {
    pub fn new() -> Self { Evaluator { variables: HashMap::new() } }

    pub fn evaluate(&mut self, program: &Program) -> Result<(), String> {
        for stmt in &program.statements {
            self.run_stmt(stmt)?;
        }
        Ok(())
    }

    fn run_stmt(&mut self, stmt: &Statement) -> Result<(), String> {
        match stmt {
            Statement::Let(l) => self.eval_let(l),
            Statement::Print(p) => self.eval_print(p),
            Statement::Block(ss) => {
                for s in ss { self.run_stmt(s)?; }
                Ok(())
            }
            Statement::While(ws) => {
                while self.condition_true(&ws.condition) {
                    for s in &ws.body {
                        self.run_stmt(s)?;
                    }
                }
                Ok(())
            }
            Statement::If(ifst) => {
                let cond_val = self.eval_expression(&ifst.condition)?;
                if cond_val.as_bool() {
                    for s in &ifst.true_body { self.run_stmt(s)?; }
                } else if let Some(ref fb) = ifst.false_body {
                    for s in fb { self.run_stmt(s)?; }
                }
                Ok(())
            }
        }
    }

    fn condition_true(&self, condition: &Expression) -> bool {
        match self.eval_expression(condition) {
            Ok(Value::Number(n)) => n != 0.0,
            Ok(Value::String(s)) => !s.is_empty(),
            _ => false,
        }
    }

    fn eval_let(&mut self, stmt: &LetStatement) -> Result<(), String> {
        let val = self.eval_expression(&stmt.value)?;
        self.variables.insert(stmt.name.clone(), val);
        Ok(())
    }

    fn eval_print(&mut self, stmt: &PrintStatement) -> Result<(), String> {
        let val = self.eval_expression(&stmt.value)?;
        match &val {
            Value::Number(n) => println!("{}", n),
            Value::String(s) => println!("{}", s),
        }
        Ok(())
    }

    /// Public accessor for testing — returns a variable's value if it exists.
    pub fn get_var(&self, name: &str) -> Option<&Value> {
        self.variables.get(name)
    }

    fn eval_expression(&self, expr: &Expression) -> Result<Value, String> {
        match expr {
            Expression::NumberLiteral(num) => Ok(Value::Number(*num)),
            Expression::StringLiteral(s) => Ok(Value::String(s.clone())),
            Expression::Identifier(name) => {
                self.variables.get(name).cloned().ok_or_else(
                    || format!("Variable '{}' no definida", name))
            }
            Expression::Infix { left, operator, right } => {
                let lval = self.eval_expression(left)?;
                let rval = self.eval_expression(right)?;
                self.apply_binop(lval, rval, operator)
            }
            Expression::Ternary { condition, true_branch, false_branch } => {
                let c = self.eval_expression(condition)?;
                if c.as_bool() {
                    self.eval_expression(true_branch)
                } else {
                    self.eval_expression(false_branch)
                }
            }
            Expression::Lambda { params: _, body: _ } => {
                Err("Lambda not implemented".to_string())
            }
            Expression::FunctionCall { func: _, args: _ } => {
                Err("FunctionCall not implemented".to_string())
            }
            Expression::Array(_) => Err("Array not implemented".to_string()),
            Expression::Map(_) => Err("Map not implemented".to_string()),
            Expression::Index { target: _, index: _ } => {
                Err("Index not implemented".to_string())
            }
        }
    }

    fn apply_binop(&self, lval: Value, rval: Value, op: &Token) -> Result<Value, String> {
        match (&lval, &rval) {
            (&Value::Number(l), &Value::Number(r)) => self.num_op(l, r, op),
            (Value::String(l), &Value::Number(r)) => self.str_num_op(l, r, op),
            (Value::Number(l), Value::String(r)) => self.num_str_op(*l, r, op),
            _ => Err(format!(
                "Type mismatch: {:?} y {:?} no son compatibles con el operador {:?}",
                lval, rval, op
            )),
        }
    }

    fn num_op(&self, l: f64, r: f64, op: &Token) -> Result<Value, String> {
        match op {
            Token::Plus => Ok(Value::Number(l + r)),
            Token::Minus => Ok(Value::Number(l - r)),
            Token::Star => Ok(Value::Number(l * r)),
            Token::Slash => {
                if r != 0.0 {
                    Ok(Value::Number(l / r))
                } else {
                    Err("Division by zero".to_string())
                }
            }
            Token::Less => Ok(Value::Number(if l < r {1.0} else {0.0})),
            Token::Greater => Ok(Value::Number(if l > r {1.0} else {0.0})),
            Token::Tilde => Ok(Value::Number(if l == r {1.0} else {0.0})),
            Token::Caret => Ok(Value::Number(if (l - r).abs() > f64::EPSILON { 1.0 } else { 0.0 })),
            _ => Ok(Value::Number(0.0)),
        }
    }

    fn num_str_op(&self, l: f64, r: &str, op: &Token) -> Result<Value, String> {
        match op {
            Token::Plus => Ok(Value::String(format!("{}{}", l, r))),
            _ => Ok(Value::String(format!("{}({})", l, r))),
        }
    }

    fn str_num_op(&self, s: &str, r: f64, op: &Token) -> Result<Value, String> {
        match op {
            Token::Plus => Ok(Value::String(format!("{}{}", s, r))),
            _ => Ok(Value::String(format!("{}({})", s, r))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    /// Helper: lex + parse + eval, returns the evaluator so we can inspect state.
    /// Panics if parsing fails (parse_program panics on errors).
    fn eval_program(source: &str) -> Result<Evaluator, String> {
        let mut lexer = Lexer::new(source.to_string());
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program();
        let mut evaluator = Evaluator::new();
        evaluator.evaluate(&program)?;
        Ok(evaluator)
    }

    fn eval_expr(source: &str) -> Result<Value, String> {
        // Wrap the expression in a let and inspect the variable
        let mut lexer = Lexer::new(format!("let __x = {};", source));
        let tokens = lexer.tokenize();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program();
        let mut evaluator = Evaluator::new();
        evaluator.evaluate(&program)?;
        evaluator.get_var("__x").cloned().ok_or("__x not set".to_string())
    }

    // -- Number literal tests --

    #[test]
    fn test_eval_number() {
        assert_eq!(eval_expr("42").unwrap(), Value::Number(42.0));
    }

    #[test]
    fn test_eval_negative() {
        // Parse -42 as 0 - 42 (binary minus by our precedence)
        let result = eval_expr("0 - 42");
        assert!(result.is_ok());
    }

    // -- String literal tests --

    #[test]
    fn test_eval_string() {
        assert_eq!(eval_expr(r#""hello""#).unwrap(), Value::String("hello".to_string()));
    }

    // -- Variable / Let tests --

    #[test]
    fn test_eval_let_and_use() {
        let ev = eval_program("let x = 10; let y = x + 5;").unwrap();
        assert_eq!(ev.get_var("x"), Some(&Value::Number(10.0)));
        assert_eq!(ev.get_var("y"), Some(&Value::Number(15.0)));
    }

    #[test]
    fn test_eval_undefined_variable() {
        let result = eval_program("print x;");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no definida"));
    }

    // -- Arithmetic tests --

    #[test]
    fn test_eval_add() {
        assert_eq!(eval_expr("2 + 3").unwrap(), Value::Number(5.0));
    }

    #[test]
    fn test_eval_sub() {
        assert_eq!(eval_expr("10 - 4").unwrap(), Value::Number(6.0));
    }

    #[test]
    fn test_eval_mul() {
        assert_eq!(eval_expr("3 * 7").unwrap(), Value::Number(21.0));
    }

    #[test]
    fn test_eval_div() {
        assert_eq!(eval_expr("10 / 2").unwrap(), Value::Number(5.0));
    }

    #[test]
    fn test_eval_div_by_zero() {
        let result = eval_expr("10 / 0");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Division by zero"));
    }

    #[test]
    fn test_eval_precedence() {
        assert_eq!(eval_expr("2 + 3 * 4").unwrap(), Value::Number(14.0));
    }

    // -- Comparison tests --

    #[test]
    fn test_eval_less_true() {
        assert_eq!(eval_expr("3 < 5").unwrap(), Value::Number(1.0));
    }

    #[test]
    fn test_eval_less_false() {
        assert_eq!(eval_expr("5 < 3").unwrap(), Value::Number(0.0));
    }

    #[test]
    fn test_eval_greater_true() {
        assert_eq!(eval_expr("5 > 3").unwrap(), Value::Number(1.0));
    }

    #[test]
    fn test_eval_greater_false() {
        assert_eq!(eval_expr("3 > 5").unwrap(), Value::Number(0.0));
    }

    #[test]
    fn test_eval_equal() {
        assert_eq!(eval_expr("5 ~ 5").unwrap(), Value::Number(1.0));
        assert_eq!(eval_expr("3 ~ 5").unwrap(), Value::Number(0.0));
    }

    #[test]
    fn test_eval_not_equal() {
        assert_eq!(eval_expr("3 ^ 5").unwrap(), Value::Number(1.0));
        assert_eq!(eval_expr("5 ^ 5").unwrap(), Value::Number(0.0));
    }

    // -- Ternary tests --

    #[test]
    fn test_eval_ternary_true() {
        assert_eq!(eval_expr("(1 < 2) ? 10 : 20").unwrap(), Value::Number(10.0));
    }

    #[test]
    fn test_eval_ternary_false() {
        assert_eq!(eval_expr("(1 > 2) ? 10 : 20").unwrap(), Value::Number(20.0));
    }

    // -- String + Number tests --

    #[test]
    fn test_eval_string_plus_number() {
        assert_eq!(eval_expr(r#""x" + 42"#).unwrap(), Value::String("x42".to_string()));
    }

    #[test]
    fn test_eval_number_plus_string() {
        assert_eq!(eval_expr("42 + \"x\"").unwrap(), Value::String("42x".to_string()));
    }

    // -- While loop tests --

    #[test]
    fn test_eval_while_basic() {
        let ev = eval_program("let i = 0; while (i < 3) ( i = i + 1; )").unwrap();
        assert_eq!(ev.get_var("i"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn test_eval_while_zero_iterations() {
        let ev = eval_program("let i = 10; while (i < 5) ( i = i + 1; )").unwrap();
        assert_eq!(ev.get_var("i"), Some(&Value::Number(10.0)));
    }

    // -- If / Else tests --

    #[test]
    fn test_eval_if_true() {
        let ev = eval_program("let x = 0; if (1 < 2) ( x = 10; )").unwrap();
        assert_eq!(ev.get_var("x"), Some(&Value::Number(10.0)));
    }

    #[test]
    fn test_eval_if_false() {
        let ev = eval_program("let x = 0; if (1 > 2) ( x = 10; )").unwrap();
        assert_eq!(ev.get_var("x"), Some(&Value::Number(0.0)));
    }

    #[test]
    fn test_eval_if_else() {
        let ev = eval_program("let x = 0; if (1 > 2) ( x = 10; ) else ( x = 20; )").unwrap();
        assert_eq!(ev.get_var("x"), Some(&Value::Number(20.0)));
    }
}