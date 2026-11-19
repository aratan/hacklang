//! Non-blocking static analysis for HackLang.
//!
//! This first checker deliberately uses Unknown for values whose type cannot be
//! established safely. It reports definite contradictions only; it never changes
//! runtime behavior or prevents a program from executing.

use crate::ast::{Expression, Program, Statement};
use crate::lexer::Token;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Number,
    Bool,
    String,
    Array,
    Map,
    Function { arity: usize, returns: Box<Type> },
    Unknown,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Number => write!(f, "number"),
            Type::Bool => write!(f, "bool"),
            Type::String => write!(f, "string"),
            Type::Array => write!(f, "array"),
            Type::Map => write!(f, "map"),
            Type::Function { arity, returns } => write!(f, "function/{arity} -> {returns}"),
            Type::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

type Scope = HashMap<String, Type>;

pub struct TypeChecker {
    scopes: Vec<Scope>,
    diagnostics: Vec<Diagnostic>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::new()],
            diagnostics: Vec::new(),
        }
    }

    pub fn check(program: &Program) -> Vec<Diagnostic> {
        let mut checker = Self::new();
        checker.check_statements(&program.statements);
        checker.diagnostics
    }

    fn report(&mut self, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic { message: message.into() });
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::new());
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn define(&mut self, name: &str, ty: Type) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup(&self, name: &str) -> Type {
        self.scopes.iter().rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or(Type::Unknown)
    }

    fn check_scoped_statements(&mut self, statements: &[Statement]) {
        self.push_scope();
        self.check_statements(statements);
        self.pop_scope();
    }

    fn check_statements(&mut self, statements: &[Statement]) {
        for statement in statements {
            match statement {
                Statement::Let(stmt) => {
                    let ty = self.infer(&stmt.value);
                    self.define(&stmt.name, ty);
                }
                Statement::Assign(stmt) => {
                    let expected = self.lookup(&stmt.name);
                    let actual = self.infer(&stmt.value);
                    self.check_compatible(&format!("asignación a '{}'", stmt.name), &expected, &actual);
                }
                Statement::IndexAssign(stmt) => {
                    let target = self.lookup(&stmt.name);
                    let index = self.infer(&stmt.index);
                    let value = self.infer(&stmt.value);
                    match target {
                        Type::Array if index != Type::Unknown && index != Type::Number =>
                            self.report(format!("el índice de '{}' debe ser number, no {}", stmt.name, index)),
                        Type::Array | Type::Map | Type::Unknown => { let _ = value; }
                        other => self.report(format!("no se puede asignar por índice a '{}' de tipo {}", stmt.name, other)),
                    }
                }
                Statement::Print(stmt) => { self.infer(&stmt.value); }
                Statement::Return(expr) => { self.infer(expr); }
                Statement::Break | Statement::Continue => {}
                Statement::Block(statements) => self.check_scoped_statements(statements),
                Statement::While(stmt) => {
                    self.infer(&stmt.condition);
                    self.check_scoped_statements(&stmt.body);
                }
                Statement::If(stmt) => {
                    self.infer(&stmt.condition);
                    self.check_scoped_statements(&stmt.true_body);
                    if let Some(body) = &stmt.false_body {
                        self.check_scoped_statements(body);
                    }
                }
            }
        }
    }

    fn check_compatible(&mut self, context: &str, expected: &Type, actual: &Type) {
        if *expected != Type::Unknown && *actual != Type::Unknown && expected != actual {
            self.report(format!("{context}: se esperaba {expected}, se obtuvo {actual}"));
        }
    }

    fn infer(&mut self, expr: &Expression) -> Type {
        match expr {
            Expression::NumberLiteral(_) => Type::Number,
            Expression::BoolLiteral(_) => Type::Bool,
            Expression::StringLiteral(_) => Type::String,
            Expression::Identifier(name) => self.lookup(name),
            Expression::Array(items) => {
                for item in items { self.infer(item); }
                Type::Array
            }
            Expression::Map(items) => {
                for (key, value) in items {
                    self.infer(key);
                    self.infer(value);
                }
                Type::Map
            }
            Expression::Index { target, index } => {
                let target_type = self.infer(target);
                let index_type = self.infer(index);
                match target_type {
                    Type::Array | Type::String if index_type != Type::Unknown && index_type != Type::Number => {
                        self.report(format!("el índice requiere number, se obtuvo {index_type}"));
                        Type::Unknown
                    }
                    Type::Array => Type::Unknown,
                    Type::String => Type::String,
                    Type::Map => Type::Unknown,
                    Type::Unknown => Type::Unknown,
                    other => {
                        self.report(format!("el tipo {other} no admite indexación"));
                        Type::Unknown
                    }
                }
            }
            Expression::Infix { left, operator, right } => {
                let lhs = self.infer(left);
                let rhs = self.infer(right);
                self.infer_binary(operator, &lhs, &rhs)
            }
            Expression::Ternary { condition, true_branch, false_branch } => {
                self.infer(condition);
                let yes = self.infer(true_branch);
                let no = self.infer(false_branch);
                if yes == no { yes } else { Type::Unknown }
            }
            Expression::Lambda { params, body } => {
                self.push_scope();
                for param in params { self.define(param, Type::Unknown); }
                let returns = self.infer(body);
                self.pop_scope();
                Type::Function { arity: params.len(), returns: Box::new(returns) }
            }
            Expression::Block(statements) => {
                self.push_scope();
                self.check_statements(statements);
                self.pop_scope();
                Type::Unknown
            }
            Expression::FunctionCall { func, args } => {
                let function_type = self.infer(func);
                for arg in args { self.infer(arg); }
                match function_type {
                    Type::Function { arity, returns } => {
                        if arity != args.len() {
                            self.report(format!("llamada a función: se esperaban {arity} argumentos y se recibieron {}", args.len()));
                        }
                        *returns
                    }
                    Type::Unknown => Type::Unknown,
                    other => {
                        self.report(format!("se intenta llamar un valor de tipo {other}, no una función"));
                        Type::Unknown
                    }
                }
            }
        }
    }

    fn infer_binary(&mut self, op: &Token, lhs: &Type, rhs: &Type) -> Type {
        use Token::*;
        match op {
            Plus if *lhs == Type::String || *rhs == Type::String => {
                if matches!(lhs, Type::String | Type::Number | Type::Unknown)
                    && matches!(rhs, Type::String | Type::Number | Type::Unknown) {
                    Type::String
                } else {
                    self.report(format!("'+' no admite operandos {lhs} y {rhs}"));
                    Type::Unknown
                }
            }
            Plus | Minus | Star | Slash => {
                self.require_pair(op, lhs, rhs, Type::Number, Type::Number)
            }
            Less | Greater | Tilde | Caret => {
                self.require_pair(op, lhs, rhs, Type::Number, Type::Bool)
            }
            _ => Type::Unknown,
        }
    }

    fn require_pair(&mut self, op: &Token, lhs: &Type, rhs: &Type, required: Type, result: Type) -> Type {
        for (side, actual) in [("izquierdo", lhs), ("derecho", rhs)] {
            if *actual != Type::Unknown && *actual != required {
                self.report(format!("operador {op:?}: el operando {side} debe ser {required}, no {actual}"));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn check_source(source: &str) -> Vec<Diagnostic> {
        let mut lexer = Lexer::new(source.to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        TypeChecker::check(&program)
    }

    #[test]
    fn infers_primitive_types_without_diagnostics() {
        assert!(check_source("let n = 1 + 2; let flag = true; let text = \"hi\";").is_empty());
    }

    #[test]
    fn detects_definite_numeric_type_mismatch() {
        let diagnostics = check_source("let result = true + 2;");
        assert!(diagnostics.iter().any(|d| d.message.contains("operando")));
    }

    #[test]
    fn detects_incompatible_assignment() {
        let diagnostics = check_source("let count = 1; count = false;");
        assert!(diagnostics.iter().any(|d| d.message.contains("asignación")));
    }

    #[test]
    fn detects_call_arity_mismatch() {
        let diagnostics = check_source("let add = |a, b| a + b; let result = add(1);");
        assert!(diagnostics.iter().any(|d| d.message.contains("argumentos")));
    }

    #[test]
    fn keeps_unknown_types_non_blocking() {
        assert!(check_source("let result = missing_name + 1;").is_empty());
    }

    #[test]
    fn detects_non_callable_values() {
        let diagnostics = check_source("let n = 42; let result = n();");
        assert!(diagnostics.iter().any(|d| d.message.contains("no una función")));
    }

    #[test]
    fn accepts_legacy_string_number_concatenation() {
        assert!(check_source("let result = \"count=\" + 3;").is_empty());
    }
}
