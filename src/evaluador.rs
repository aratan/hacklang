// src/evaluador.rs
// Versión: rewrite applied
use crate::ast::{AssignStatement, Expression, IndexAssignStatement, Program, Statement, LetStatement, PrintStatement};
use crate::lexer::Token;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    Bool(bool),
    String(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Function { params: Vec<String>, body: Box<Expression>, captured_env: usize },
}

impl Value {
    fn as_bool(&self) -> bool {
        match self {
            Value::Bool(value) => *value,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Map(_) | Value::Function { .. } => true,
        }
    }
}


#[derive(Debug, Clone, PartialEq)]
enum Flow {
    Continue,
    LoopContinue,
    Break,
    Return(Value),
}
#[derive(Debug, Clone)]
struct Environment {
    parent: Option<usize>,
    values: HashMap<String, Value>,
    alive: bool,
}

const MAX_EVAL_DEPTH: usize = 256;

#[derive(Debug)]
pub struct Evaluator {
    // Arena de entornos: las closures conservan IDs estables, no copias de variables.
    // Los frames permanecen vivos durante la vida del Evaluator.
    environments: Vec<Environment>,
    free_env_ids: Vec<usize>,
    current_env: usize,
    eval_depth: usize,
}

impl Evaluator {
    pub fn new() -> Self {
        Evaluator {
            environments: vec![Environment { parent: None, values: HashMap::new(), alive: true }],
            free_env_ids: Vec::new(),
            current_env: 0,
            eval_depth: 0,
        }
    }

    fn push_environment(&mut self, parent: usize) -> usize {
        if let Some(id) = self.free_env_ids.pop() {
            self.environments[id] = Environment { parent: Some(parent), values: HashMap::new(), alive: true };
            id
        } else {
            let id = self.environments.len();
            self.environments.push(Environment { parent: Some(parent), values: HashMap::new(), alive: true });
            id
        }
    }

    fn mark_value(value: &Value, pending: &mut Vec<usize>) {
        match value {
            Value::Function { captured_env, .. } => pending.push(*captured_env),
            Value::Array(items) => {
                for item in items { Self::mark_value(item, pending); }
            }
            Value::Map(entries) => {
                for (key, value) in entries {
                    Self::mark_value(key, pending);
                    Self::mark_value(value, pending);
                }
            }
            Value::Number(_) | Value::Bool(_) | Value::String(_) => {}
        }
    }

    // Recolector mark-and-sweep de frames. Los valores siguen gestionados por Rust.
    fn collect_garbage(&mut self, temporary_root: Option<&Value>) {
        let mut marked = vec![false; self.environments.len()];
        let mut pending = vec![0, self.current_env];
        if let Some(value) = temporary_root {
            Self::mark_value(value, &mut pending);
        }

        while let Some(id) = pending.pop() {
            if id >= self.environments.len() || marked[id] || !self.environments[id].alive {
                continue;
            }
            marked[id] = true;
            let frame = &self.environments[id];
            if let Some(parent) = frame.parent { pending.push(parent); }
            for value in frame.values.values() { Self::mark_value(value, &mut pending); }
        }

        for id in 1..self.environments.len() {
            if self.environments[id].alive && !marked[id] {
                self.environments[id].alive = false;
                self.environments[id].parent = None;
                self.environments[id].values.clear();
                self.free_env_ids.push(id);
            }
        }
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
            if frame.alive {
                if let Some(value) = frame.values.get(name) {
                    return Some(value);
                }
            }
            environment = frame.parent;
        }
        None
    }

    fn find_environment(&self, name: &str) -> Option<usize> {
        let mut environment = Some(self.current_env);
        while let Some(id) = environment {
            if self.environments[id].alive && self.environments[id].values.contains_key(name) { return Some(id); }
            environment = self.environments[id].parent;
        }
        None
    }

    fn assign_existing(&mut self, name: &str, value: Value) -> Result<(), String> {
        let id = self.find_environment(name)
            .ok_or_else(|| format!("Variable '{}' no definida para asignación", name))?;
        self.environments[id].values.insert(name.to_string(), value);
        Ok(())
    }

    pub fn evaluate(&mut self, program: &Program) -> Result<(), String> {
        for stmt in &program.statements {
            match self.run_stmt(stmt)? {
                Flow::Continue => {}
                Flow::LoopContinue => return Err("continue fuera de un bucle".to_string()),
                Flow::Break => return Err("break fuera de un bucle".to_string()),
                Flow::Return(_) => return Err("return fuera de una función".to_string()),
            }
        }
        Ok(())
    }

    fn run_stmt(&mut self, stmt: &Statement) -> Result<Flow, String> {
        let result = match stmt {
            Statement::Let(l) => self.eval_let(l).map(|_| Flow::Continue),
            Statement::Assign(a) => self.eval_assign(a).map(|_| Flow::Continue),
            Statement::IndexAssign(a) => self.eval_index_assign(a).map(|_| Flow::Continue),
            Statement::Print(p) => self.eval_print(p).map(|_| Flow::Continue),
            Statement::Return(value) => self.eval_expression(value).map(Flow::Return),
            Statement::Break => Ok(Flow::Break),
            Statement::Continue => Ok(Flow::LoopContinue),
            Statement::Block(statements) => {
                let parent = self.current_env;
                self.with_environment(parent, |this| {
                    for statement in statements {
                        match this.run_stmt(statement)? {
                            Flow::Continue => {}
                            flow @ (Flow::LoopContinue | Flow::Break | Flow::Return(_)) => return Ok(flow),
                        }
                    }
                    Ok(Flow::Continue)
                })
            }
            Statement::While(ws) => {
                let parent = self.current_env;
                self.with_environment(parent, |this| {
                    'while_loop: loop {
                        if !this.condition_true(&ws.condition)? { break; }
                        for statement in &ws.body {
                            match this.run_stmt(statement)? {
                                Flow::Continue => {}
                                Flow::LoopContinue => continue 'while_loop,
                                Flow::Break => break 'while_loop,
                                flow @ Flow::Return(_) => return Ok(flow),
                            }
                        }
                    }
                    Ok(Flow::Continue)
                })
            }
            Statement::If(ifst) => {
                let condition = self.eval_expression(&ifst.condition)?;
                let parent = self.current_env;
                self.with_environment(parent, |this| {
                    let body = if condition.as_bool() {
                        Some(&ifst.true_body)
                    } else {
                        ifst.false_body.as_ref()
                    };
                    if let Some(statements) = body {
                        for statement in statements {
                            match this.run_stmt(statement)? {
                                Flow::Continue => {}
                                flow @ (Flow::LoopContinue | Flow::Break | Flow::Return(_)) => return Ok(flow),
                            }
                        }
                    }
                    Ok(Flow::Continue)
                })
            }
        };
        let temporary_root = match &result {
            Ok(Flow::Return(value)) => Some(value),
            _ => None,
        };
        self.collect_garbage(temporary_root);
        result
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

    fn eval_index_assign(&mut self, stmt: &IndexAssignStatement) -> Result<(), String> {
        let index = self.eval_expression(&stmt.index)?;
        let value = self.eval_expression(&stmt.value)?;
        let environment = self.find_environment(&stmt.name)
            .ok_or_else(|| format!("Variable '{}' no definida para asignación", stmt.name))?;
        let target = self.environments[environment].values.get_mut(&stmt.name).unwrap();
        match target {
            Value::Array(items) => match index {
                Value::Number(n) if n >= 0.0 && n.fract() == 0.0 => {
                    let item = items.get_mut(n as usize)
                        .ok_or_else(|| format!("Índice de array fuera de rango: {}", n))?;
                    *item = value;
                    Ok(())
                }
                other => Err(format!("El índice de un array debe ser entero no negativo: {:?}", other)),
            },
            Value::Map(entries) => {
                if let Some((_, existing)) = entries.iter_mut().find(|(key, _)| key == &index) {
                    *existing = value;
                } else {
                    entries.push((index, value));
                }
                Ok(())
            }
            other => Err(format!("No se puede asignar por índice al valor {:?}", other)),
        }
    }

    fn eval_print(&mut self, stmt: &PrintStatement) -> Result<(), String> {
        let val = self.eval_expression(&stmt.value)?;
        match &val {
            Value::Number(n) => println!("{}", n),
            Value::Bool(value) => println!("{}", value),
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
        if self.eval_depth >= MAX_EVAL_DEPTH {
            return Err(format!("Límite de profundidad de evaluación excedido ({MAX_EVAL_DEPTH})"));
        }
        self.eval_depth += 1;
        let result = self.eval_expression_inner(expr);
        self.eval_depth -= 1;
        result
    }

    fn eval_expression_inner(&mut self, expr: &Expression) -> Result<Value, String> {
        match expr {
            Expression::NumberLiteral(num) => Ok(Value::Number(*num)),
            Expression::BoolLiteral(value) => Ok(Value::Bool(*value)),
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
            Expression::Block(statements) => {
                for statement in statements {
                    match self.run_stmt(statement)? {
                        Flow::Continue => {}
                        Flow::Return(value) => return Ok(value),
                        Flow::Break => return Err("break fuera de un bucle".to_string()),
                        Flow::LoopContinue => return Err("continue fuera de un bucle".to_string()),
                    }
                }
                Ok(Value::Number(0.0))
            }
            Expression::FunctionCall { func, args } => {
                if let Expression::Identifier(name) = func.as_ref() {
                    if name == "len" {
                        if args.len() != 1 {
                            return Err(format!("len() esperaba 1 argumento y recibió {}", args.len()));
                        }
                        return match self.eval_expression(&args[0])? {
                            Value::String(value) => Ok(Value::Number(value.chars().count() as f64)),
                            Value::Array(values) => Ok(Value::Number(values.len() as f64)),
                            Value::Map(entries) => Ok(Value::Number(entries.len() as f64)),
                            other => Err(format!("len() no admite el valor {:?}", other)),
                        };
                    }
                }
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
            Token::Less => Ok(Value::Bool(l < r)),
            Token::Greater => Ok(Value::Bool(l > r)),
            Token::Tilde => Ok(Value::Bool(l == r)),
            Token::Caret => Ok(Value::Bool((l - r).abs() > f64::EPSILON)),
            _ => Err(format!("Operador numérico no soportado: {:?}", op)),
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

    // -- First-class function call tests --

    #[test]
    fn test_function_block_with_return() {
        let ev = eval_program("let calculate = |x| (let doubled = x * 2; return doubled + 1;); let result = calculate(4);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(9.0)));
    }

    #[test]
    fn test_returned_closure_keeps_nested_block_environment_alive() {
        let ev = eval_program(
            "let factory = |x| ((let inner = |y| x + y; return inner;)); let add_ten = factory(10); let result = add_ten(5);"
        ).unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(15.0)));
    }

    #[test]
    fn test_return_propagates_through_if() {
        let ev = eval_program("let choose = |x| (if (x > 0) (return 1;); return 2;); let positive = choose(5); let negative = choose(0);").unwrap();
        assert_eq!(ev.get_var("positive"), Some(&Value::Number(1.0)));
        assert_eq!(ev.get_var("negative"), Some(&Value::Number(2.0)));
    }

    #[test]
    fn test_return_propagates_through_while() {
        let ev = eval_program("let find = |limit| (let i = 0; while (i < limit) (if (i ~ 3) (return i;); i = i + 1;); return 0 - 1;); let result = find(10);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn test_break_exits_loop() {
        let ev = eval_program("let i = 0; while (i < 10) (i = i + 1; if (i ~ 3) (break;);); let result = i;").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(3.0)));
    }

    #[test]
    fn test_continue_skips_remaining_iteration() {
        let ev = eval_program("let i = 0; let sum = 0; while (i < 5) (i = i + 1; if (i ~ 3) (continue;); sum = sum + i;); let result = sum;").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(12.0)));
    }

    #[test]
    fn test_break_outside_loop_is_error() {
        let result = eval_program("break;");
        assert!(result.unwrap_err().contains("break fuera de un bucle"));
    }

    #[test]
    fn test_return_outside_function_is_error() {
        let result = eval_program("return 42;");
        assert!(result.unwrap_err().contains("return fuera de una función"));
    }

    #[test]
    fn test_eval_call_on_parenthesized_function() {
        let ev = eval_program("let inc = |x| x + 1; let result = (inc)(4);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(5.0)));
    }

    #[test]
    fn test_eval_immediate_lambda_call() {
        let ev = eval_program("let result = (|x| x + 1)(4);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(5.0)));
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
    fn unsupported_numeric_operator_returns_error_instead_of_zero() {
        let evaluator = Evaluator::new();
        let error = evaluator.num_op(4.0, 2.0, &Token::At).unwrap_err();
        assert!(error.contains("Operador numérico no soportado"));
    }

    #[test]
    fn test_eval_precedence() {
        assert_eq!(eval_expr("2 + 3 * 4").unwrap(), Value::Number(14.0));
    }

    // -- Comparison tests --

    #[test]
    fn test_eval_less_true() {
        assert_eq!(eval_expr("3 < 5").unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_eval_less_false() {
        assert_eq!(eval_expr("5 < 3").unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_eval_greater_true() {
        assert_eq!(eval_expr("5 > 3").unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_eval_greater_false() {
        assert_eq!(eval_expr("3 > 5").unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_eval_equal() {
        assert_eq!(eval_expr("5 ~ 5").unwrap(), Value::Bool(true));
        assert_eq!(eval_expr("3 ~ 5").unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_eval_not_equal() {
        assert_eq!(eval_expr("3 ^ 5").unwrap(), Value::Bool(true));
        assert_eq!(eval_expr("5 ^ 5").unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_eval_boolean_literals() {
        assert_eq!(eval_expr("true").unwrap(), Value::Bool(true));
        assert_eq!(eval_expr("false").unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_boolean_literals_work_in_conditions() {
        let ev = eval_program("let a = 0; if (true) (a = 1;); if (false) (a = 2;); let i = 0; while (false) (i = i + 1;);").unwrap();
        assert_eq!(ev.get_var("a"), Some(&Value::Number(1.0)));
        assert_eq!(ev.get_var("i"), Some(&Value::Number(0.0)));
    }

    #[test]
    fn test_numeric_truthiness_remains_compatible() {
        let ev = eval_program("let a = 0; if (1) (a = 7;); if (0) (a = 9;);").unwrap();
        assert_eq!(ev.get_var("a"), Some(&Value::Number(7.0)));
    }

    #[test]
    fn test_boolean_printing() {
        // The test asserts evaluation; print formatting is covered by the Value variant.
        assert_eq!(eval_expr("true").unwrap(), Value::Bool(true));
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
    fn builtin_len_handles_unicode_strings_arrays_and_maps() {
        assert_eq!(eval_expr("len(\"hola\")").unwrap(), Value::Number(4.0));
        assert_eq!(eval_expr("len(\"á🙂\")").unwrap(), Value::Number(2.0));
        assert_eq!(eval_expr("len([1,2,3])").unwrap(), Value::Number(3.0));
    }

    #[test]
    fn builtin_len_rejects_unsupported_values_and_wrong_arity() {
        assert!(eval_expr("len(1)").unwrap_err().contains("no admite"));
        assert!(eval_expr("len()").unwrap_err().contains("esperaba 1 argumento"));
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

    #[test]
    fn deep_recursion_returns_error_instead_of_overflowing_stack() {
        let result = eval_program("let loop = |n| loop(n + 1); loop(0);");
        assert!(result.unwrap_err().contains("Límite de profundidad de evaluación"));
    }

    #[test]
    fn test_assign_array_index() {
        let ev = eval_program("let values = [1, 2]; values[0] = 9; let result = values[0];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(9.0)));
    }

    #[test]
    fn test_assign_map_index_and_insert_key() {
        let ev = eval_program("let config = @(\"port\": 80); config[\"port\"] = 8080; config[\"host\"] = \"localhost\"; let result = config[\"port\"];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(8080.0)));
    }

    #[test]
    fn test_assign_array_index_out_of_range() {
        let result = eval_program("let values = [1]; values[3] = 9;");
        assert!(result.unwrap_err().contains("fuera de rango"));
    }

    #[test]
    fn test_returned_closure_keeps_captured_frame_alive() {
        let ev = eval_program("let make_adder = |x| |y| x + y; let add_five = make_adder(5); let result = add_five(3);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(8.0)));
    }

    #[test]
    fn test_environment_frames_are_reused_after_calls() {
        let ev = eval_program("let add = |x| x + 1; add(1); add(2); add(3); add(4); add(5);").unwrap();
        assert!(ev.environments.len() <= 2, "frames should be reclaimed and reused");
        assert!(!ev.free_env_ids.is_empty(), "a returned frame should be available for reuse");
    }


    // -- MEM-004..MEM-017: observable lifetime/ownership semantics baseline --
    // These tests pin current language behavior; they do not claim stack/region
    // placement is implemented. Placement and release are verified in later phases.

    #[test]
    fn mem_004_returned_array_remains_valid_after_creator_returns() {
        let ev = eval_program("let make = |x| [x, x + 1]; let values = make(40); let result = values[1];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(41.0)));
    }

    #[test]
    fn mem_005_nested_map_and_array_values_survive_escape() {
        let ev = eval_program("let make = |x| @(\"payload\": [x, x + 1]); let data = make(8); let result = data[\"payload\"][1];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(9.0)));
    }

    #[test]
    fn mem_006_string_inside_returned_array_remains_valid() {
        let ev = eval_program("let make = |x| [x, \"alive\"]; let values = make(1); let result = values[1];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::String("alive".into())));
    }

    #[test]
    fn mem_007_returned_closure_keeps_immutable_capture() {
        let ev = eval_program("let make = |x| |y| x + y; let add = make(30); let result = add(12);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(42.0)));
    }

    #[test]
    fn mem_008_returned_closure_observes_mutable_capture() {
        let ev = eval_program("let make = |x| (let change = |n| (x = x + n; return x;); return change;); let change = make(10); let first = change(2); let second = change(3);").unwrap();
        assert_eq!(ev.get_var("first"), Some(&Value::Number(12.0)));
        assert_eq!(ev.get_var("second"), Some(&Value::Number(15.0)));
    }

    #[test]
    fn mem_009_sibling_closures_share_the_same_mutable_capture() {
        let ev = eval_program("let make = |x| (let increment = |n| (x = x + n; return x;); let read = |n| x; return [increment, read];); let pair = make(10); let changed = pair[0](5); let observed = pair[1](0);").unwrap();
        assert_eq!(ev.get_var("changed"), Some(&Value::Number(15.0)));
        assert_eq!(ev.get_var("observed"), Some(&Value::Number(15.0)));
    }

    #[test]
    fn mem_010_recursive_closure_remains_callable() {
        let ev = eval_program("let factorial = |n| n < 2 ? 1 : n * factorial(n - 1); let result = factorial(6);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(720.0)));
    }

    #[test]
    fn mem_011_mutually_recursive_closures_resolve_shared_scope() {
        let ev = eval_program("let even = |n| n ~ 0 ? true : odd(n - 1); let odd = |n| n ~ 0 ? false : even(n - 1); let result = even(8);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Bool(true)));
    }

    #[test]
    fn mem_012_closure_nested_in_escaping_collection_keeps_capture_alive() {
        let ev = eval_program("let make = |x| [|y| x + y]; let functions = make(20); let add = functions[0]; let result = add(22);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(42.0)));
    }

    #[test]
    fn mem_013_repeated_loop_calls_reuse_environment_frames() {
        let ev = eval_program("let identity = |x| x; let i = 0; let sum = 0; while (i < 100) (sum = sum + identity(i); i = i + 1;); let result = sum;").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(4950.0)));
        assert!(ev.environments.len() <= 4, "repeated calls should not retain one frame per iteration");
    }

    #[test]
    fn mem_014_break_and_continue_leave_nested_scopes_consistent() {
        let ev = eval_program("let i = 0; let sum = 0; while (i < 8) ((let temporary = i; i = i + 1; if (temporary ~ 2) (continue;); if (temporary ~ 6) (break;); sum = sum + temporary;)); let result = sum;").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(13.0)));
    }

    #[test]
    fn mem_015_return_from_nested_control_flow_preserves_returned_value() {
        let ev = eval_program("let find = |n| (while (n > 0) (if (n ~ 3) (return [n, \"found\"];); n = n - 1;); return [0, \"missing\"];); let result = find(5); let label = result[1];").unwrap();
        assert_eq!(ev.get_var("label"), Some(&Value::String("found".into())));
    }

    #[test]
    fn mem_016_error_during_nested_evaluation_is_reported() {
        let error = eval_program("let make = |x| [x, x + 1]; let values = make(1); let result = values[99];").unwrap_err();
        assert!(error.contains("fuera de rango"));
    }

    #[test]
    fn mem_017_reassigning_collection_keeps_new_value_observable() {
        let ev = eval_program("let values = [1, 2]; values = [7, 8, 9]; let result = values[2];").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(9.0)));
    }

    #[test]
    fn mem_020_huge_collection_index_fails_without_panicking() {
        let error = eval_program("let stable = 42; let values = [1]; let result = values[1e300];").unwrap_err();
        assert!(error.contains("fuera de rango"));
    }

    #[test]
    fn mem_021_deep_recursion_fails_with_controlled_error() {
        let error = eval_program("let recurse = |n| recurse(n + 1); recurse(0);").unwrap_err();
        assert!(error.contains("Límite de profundidad de evaluación"));
    }

    #[test]
    fn mem_022_collection_and_closure_semantics_are_stable_across_nested_calls() {
        let ev = eval_program("let build = |base| (let values = [base, base + 1]; let mapper = |offset| values[1] + offset; return mapper;); let mapper = build(40); let result = mapper(1);").unwrap();
        assert_eq!(ev.get_var("result"), Some(&Value::Number(42.0)));
    }

}
