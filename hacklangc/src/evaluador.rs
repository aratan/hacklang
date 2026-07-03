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