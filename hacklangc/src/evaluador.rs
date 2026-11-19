// src/evaluador.rs
// Versión: rewrite applied
use crate::ast::{AssignStatement, Expression, Program, Statement, LetStatement, PrintStatement};
use crate::lexer::Token;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Function { params: Vec<String>, body: Box<Expression>, captured_env: usize },
}

impl Value {
    fn as_bool(&self) -> bool {
        match self {
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Map(_) | Value::Function { .. } => true,
        }
    }
}

#[derive(Debug, Clone)]
struct Environment {
    parent: Option<usize>,
    values: HashMap<String, Value>,
}

#[derive(Debug)]
pub struct Evaluator {
    // Arena de entornos: las closures conservan IDs estables, no copias de variables.
    // Los frames permanecen vivos durante la vida del Evaluator.
    environments: Vec<Environment>,
    current_env: usize,
}

impl Evaluator {
    pub fn new() -> Self {
        Evaluator {
            environments: vec![Environment { parent: None, values: HashMap::new() }],
            current_env: 0,
        }
    }

    fn push_environment(&mut self, parent: usize) -> usize {
        let id = self.environments.len();
        self.environments.push(Environment { parent: Some(parent), values: HashMap::new() });
        id
    }

    fn with_environment<R>(
        &mut self,
        parent: usize,
        operation: impl FnOnce(&mut Self) -> Result<R, String>,
    ) -> Result<R, String> {
        let previous = self.current_env;
        self.current_env = self.push_environment(parent);
        let result = operation(self);
        self.current_env = previous;
        result
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        let mut environment = Some(self.current_env);
        while let Some(id) = environment {
            let frame = &self.environments[id];
            if let Some(value) = frame.values.get(name) {
                return Some(value);
            }
            environment = frame.parent;
        }
        None
    }

    fn assign_existing(&mut self, name: &str, value: Value) -> Result<(), String> {
        let mut environment = Some(self.current_env);
        while let Some(id) = environment {
            let parent = self.environments[id].parent;
            if self.environments[id].values.contains_key(name) {
                self.environments[id].values.insert(name.to_string(), value);
                return Ok(());
            }
            environment = parent;
        }
        Err(format!("Variable '{}' no definida para asignación", name))
    }

    pub fn evaluate(&mut self, program: &Program) -> Result<(), String> {
        for stmt in &program.statements {
            self.run_stmt(stmt)?;
        }
        Ok(())
    }

    fn run_stmt(&mut self, stmt: &Statement) -> Result<(), String> {
        match stmt {
            Statement::Let(l) => self.eval_let(l),
            Statement::Assign(a) => self.eval_assign(a),
            Statement::Print(p) => self.eval_print(p),
            Statement::Block(statements) => {
                let parent = self.current_env;
                self.with_environment(parent, |this| {
                    for statement in statements { this.run_stmt(statement)?; }
                    Ok(())
                })
            }
            Statement::While(ws) => {
                let parent = self.current_env;
                self.with_environment(parent, |this| {
                    loop {
                        if !this.condition_true(&ws.condition)? { break; }
                        for statement in &ws.body { this.run_stmt(statement)?; }
                    }
                    Ok(())
                })
            }
            Statement::If(ifst) => {
                let condition = self.eval_expression(&ifst.condition)?;
                let parent = self.current_env;
                self.with_environment(parent, |this| {
                    if condition.as_bool() {
                        for statement in &ifst.true_body { this.run_stmt(statement)?; }
                    } else if let Some(false_body) = &ifst.false_body {
                        for statement in false_body { this.run_stmt(statement)?; }
                    }
                    Ok(())
                })
            }
        }
    }

    fn condition_true(&mut self, condition: &Expression) -> Result<bool, String> {
        Ok(self.eval_expression(condition)?.as_bool())
    }

    fn eval_let(&mut self, stmt: &LetStatement) -> Result<(), String> {
        let value = self.eval_expression(&stmt.value)?;
        self.environments[self.current_env].values.insert(stmt.name.clone(), value);
        Ok(())
    }

    fn eval_assign(&mut self, stmt: &AssignStatement) -> Result<(), String> {
        let value = self.eval_expression(&stmt.value)?;
        self.assign_existing(&stmt.name, value)
    }

    fn eval_print(&mut self, stmt: &PrintStatement) -> Result<(), String> {
        let val = self.eval_expression(&stmt.value)?;
        match &val {
            Value::Number(n) => println!("{}", n),
            Value::String(s) => println!("{}", s),
            Value::Array(values) => println!("{:?}", values),
            Value::Map(entries) => println!("{:?}", entries),
            Value::Function { .. } => println!("<function>"),
        }
        Ok(())
    }

    /// Public accessor for testing — returns a variable's value if it exists.
    pub fn get_var(&self, name: &str) -> Option<&Value> {
        self.lookup(name)
    }

    fn eval_expression(&mut self, expr: &Expression) -> Result<Value, String> {
        match expr {
            Expression::NumberLiteral(num) => Ok(Value::Number(*num)),
            Expression::StringLiteral(s) => Ok(Value::String(s.clone())),
            Expression::Identifier(name) => {
                self.lookup(name).cloned().ok_or_else(
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
            Expression::Lambda { params, body } => Ok(Value::Function {
                params: params.clone(), body: body.clone(), captured_env: self.current_env,
            }),
            Expression::FunctionCall { func, args } => {
                let function = self.eval_expression(func)?;
                let (params, body, captured_env) = match function {
                    Value::Function { params, body, captured_env } => (params, body, captured_env),
                    other => return Err(format!("El valor {:?} no es una función", other)),
                };
                if params.len() != args.len() {
                    return Err(format!("La función esperaba {} argumentos y recibió {}", params.len(), args.len()));
                }
                let mut values = Vec::with_capacity(args.len());
                for arg in args { values.push(self.eval_expression(arg)?); }
                self.with_environment(captured_env, |this| {
                    for (name, value) in params.iter().cloned().zip(values) {
                        this.environments[this.current_env].values.insert(name, value);
                    }
                    this.eval_expression(&body)
                })
            }
            Expression::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items { values.push(self.eval_expression(item)?); }
                Ok(Value::Array(values))
            }
            Expression::Map(items) => {
                let mut entries = Vec::with_capacity(items.len());
                for (key, value) in items {
                    entries.push((self.eval_expression(key)?, self.eval_expression(value)?));
                }
                Ok(Value::Map(entries))
            }
            Expression::Index { target, index } => {
                let target_value = self.eval_expression(target)?;
                let index_value = self.eval_expression(index)?;
                match target_value {
                    Value::Array(items) => match index_value {
                        Value::Number(n) if n >= 0.0 && n.fract() == 0.0 => items.get(n as usize).cloned().ok_or_else(|| format!("Índice de array fuera de rango: {}", n)),
                        other => Err(format!("El índice de un array debe ser entero no negativo: {:?}", other)),
                    },
                    Value::Map(entries) => entries.into_iter().find(|(key, _)| key == &index_value).map(|(_, value)| value).ok_or_else(|| format!("Clave no encontrada en mapa: {:?}", index_value)),
                    Value::String(value) => match index_value {
                        Value::Number(n) if n >= 0.0 && n.fract() == 0.0 => value.chars().nth(n as usize).map(|c| Value::String(c.to_string())).ok_or_else(|| format!("Índice de cadena fuera de rango: {}", n)),
                        other => Err(format!("El índice de una cadena debe ser entero no negativo: {:?}", other)),
                    },
                    other => Err(format!("No se puede indexar el valor {:?}", other)),
                }
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
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        let mut evaluator = Evaluator::new();
        evaluator.evaluate(&program)?;
        Ok(evaluator)
    }

    fn eval_expr(source: &str) -> Result<Value, String> {
        // Wrap the expression in a let and inspect the variable
        let mut lexer = Lexer::new(format!("let __x = {};", source));
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
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
    #[test]
    fn test_lambda_function_call() {
        let ev = eval_program("let add = |a, b| a + b; let result = add(3, 4);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(7.0)));
    }

    #[test]
    fn test_lambda_captures_outer_scope() {
        let ev = eval_program("let base = 10; let add_base = |n| n + base; let result = add_base(5);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(15.0)));
    }

    #[test]
    fn test_function_arity_error() {
        let result = eval_program("let add = |a, b| a + b; let result = add(1);");
        assert!(result.unwrap_err().contains("esperaba 2 argumentos"));
    }

    #[test]
    fn test_array_literal_and_index() {
        let ev = eval_program("let values = [10, 20, 30]; let result = values[1];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(20.0)));
    }

    #[test]
    fn test_map_literal_and_index() {
        let ev = eval_program("let config = @(\"port\": 8080); let result = config[\"port\"];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(8080.0)));
    }

    #[test]
    fn test_while_condition_error_is_propagated() {
        let result = eval_program("while (missing < 3) ( print 1; )");
        assert!(result.unwrap_err().contains("missing"));
    }

    #[test]
    fn test_closure_sees_later_mutation() {
        let ev = eval_program("let base = 10; let add = |n| n + base; base = 20; let result = add(1);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(21.0)));
    }

    #[test]
    fn test_lexical_block_scope() {
        let ev = eval_program("let value = 1; (let value = 2;); let result = value;").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(1.0)));
    }

    #[test]
    fn test_assignment_updates_outer_scope() {
        let ev = eval_program("let value = 1; (value = value + 2;); let result = value;").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn test_recursive_lambda() {
        let ev = eval_program("let factorial = |n| n < 2 ? 1 : n * factorial(n - 1); let result = factorial(5);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(120.0)));
    }

}
