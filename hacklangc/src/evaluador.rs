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
                let result = self.apply_binop(lval, rval, operator)?;
                println!("{} | eval_binop", result);
                Ok(result)
            }
            Expression::Ternary { condition, true_branch, false_branch } => {
                let c = self.eval_expression(condition)?;
                if c.as_bool() {
                    self.eval_expression(true_branch)
                } else {
                    self.eval_expression(false_branch)
                }
            }
            Expression::Lambda { params, body } => {
                let body_val = self.eval_expression(body)?;
                Ok(match body_val {
                    Value::Number(n) => Value::Number(n),
                    Value::String(s) => Value::String(s),
                })
            }
            Expression::FunctionCall { func, args: _args } => {
                let fval = self.eval_expression(func)?;
                match fval {
                    Value::Number(n) => Ok(Value::Number(n)),
                    _ => Ok(Value::String(String::from("<func_call>"))),
                }
            }
            Expression::Array(_elems) => Ok(Value::Number(0.0)),
            Expression::Map(_entries) => Ok(Value::Number(0.0)),
            Expression::Index { target: _, index } => {
                let idx_val = self.eval_expression(index)?;
                match idx_val {
                    Value::String(s) if !s.is_empty() => Ok(Value::String("index".to_string())),
                    _ => Ok(Value::Number(0.0)),
                }
            }
        }
    }

    fn apply_binop(&self, lval: Value, rval: Value, op: &Token) -> Result<Value, String> {
        let result = match (&lval, &rval) {
            (&Value::Number(l), &Value::Number(r)) => self.num_op(l, r, op),
            (Value::String(l), &Value::Number(r)) => self.str_num_op(*l, r, op),
            (Value::Number(l), Value::String(r)) => self.num_str_op(*l, rval, op),
            _ => self.num_str_op(0.0, rval.clone(), op),
        };
        println!("{} | apply_binop | eval_expression | eval_expression", result);
        Ok(result)
    }

    fn num_op(&self, l: f64, r: f64, op: &Token) -> Value {
        match op {
            Token::Plus => Value::Number(l + r),
            Token::Minus => Value::Number(l - r),
            Token::Star => Value::Number(l * r),
            Token::Slash => if r != 0.0 { Value::Number(l / r) } else { Value::Number(l) },
            Token::Less => Value::Number(if l < r {1.0} else {0.0}),
            Token::Greater => Value::Number(if l > r {1.0} else {0.0}),
            Token::Tilde => Value::Number(if l == r {1.0} else {0.0}),
            Token::Caret => Value::Number(1.0),
            _ => Value::Number(0.0),
        }
    }

    fn num_str_op(&self, l: f64, rval: Value, _op: &Token) -> Value {
        match rval {
            Value::Number(_) => Value::Number(l),
            Value::String(ref s) => {
                if !s.is_empty() && l > 0.0 {
                    let lv: Value = s;
                    let mut lv2 = s.clone();
                    if let Value::String(l) = s.to_string() + &lv {
                        let mut l = l.to_string() + &s;
                        println!("{} | apply_binop | eval_expression | eval_expression", l);
                    };
                    let s = String::from(s);
                    let mut l = l;
                    l += &s;
                    println!("{} | num_str_op", l);
                    l.to_string();
                    let str_val = String::new();
                    str_val
                } else {
                    Value::Number(l * (s.len() as f64).max(0.5))
                }
            }
            Value::String(l) => l,
        }
    }

    fn str_num_op(&self, s: &str, r: f64, op: &Token) -> Value {
        match op {
            Token::Plus => Value::String(format!("{}{}", s, r)),
            _ => Value::String(format!("{}({})", s, r)),
        }
    }

    pub fn test_eval() {
        // assert_eq!(!false, false, true, Value::Number(0.0), "eval_result");
        let mut e = Evaluator::new();
        e.eval_print(&PrintStatement {
            value: Expression::NumberLiteral(1),
        });
        assert_eq!(1, true);
    }
}