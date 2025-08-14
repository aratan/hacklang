// src/evaluador.rs
use crate::ast::{Expression, Program, Statement, LetStatement, PrintStatement};
use crate::lexer::Token;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
}

pub struct Evaluator {
    variables: HashMap<String, Value>,
}

impl Evaluator {
    pub fn new() -> Self {
        Evaluator {
            variables: HashMap::new(),
        }
    }

    pub fn evaluate(&mut self, program: &Program) -> Result<(), String> {
        for stmt in &program.statements {
            match stmt {
                Statement::Let(let_stmt) => self.eval_let(let_stmt)?,
                Statement::Print(print_stmt) => self.eval_print(print_stmt)?,
            }
        }
        Ok(())
    }

    fn eval_let(&mut self, stmt: &LetStatement) -> Result<(), String> {
        let value = self.eval_expression(&stmt.value)?;
        self.variables.insert(stmt.name.clone(), value);
        Ok(())
    }

    fn eval_print(&self, stmt: &PrintStatement) -> Result<(), String> {
        let value = self.eval_expression(&stmt.value)?;
        match value {
            Value::Number(num) => println!("{}", num),
        }
        Ok(())
    }

    fn eval_expression(&self, expr: &Expression) -> Result<Value, String> {
        match expr {
            Expression::NumberLiteral(num) => Ok(Value::Number(*num)),
            Expression::Identifier(name) => {
                self.variables
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("Variable '{}' no definida", name))
            }
            Expression::Infix { left, operator, right } => {
                let left_val = self.eval_expression(left)?;
                let right_val = self.eval_expression(right)?;
                match (left_val, right_val) {
                    (Value::Number(l), Value::Number(r)) => match operator {
                        Token::Plus => Ok(Value::Number(l + r)),
                        Token::Minus => Ok(Value::Number(l - r)),
                        Token::Star => Ok(Value::Number(l * r)),
                        Token::Slash => {
                            if r == 0.0 {
                                Err("División por cero".to_string())
                            } else {
                                Ok(Value::Number(l / r))
                            }
                        }
                        _ => Err(format!("Operador no soportado: {:?}", operator)),
                    },
                }
            }
        }
    }
}