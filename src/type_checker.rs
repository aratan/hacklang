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
    Array(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Function { arity: usize, returns: Box<Type> },
    Unknown,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Number => write!(f, "number"),
            Type::Bool => write!(f, "bool"),
            Type::String => write!(f, "string"),
            Type::Array(element) => write!(f, "array<{element}>"),
            Type::Map(key, value) => write!(f, "map<{key}, {value}>"),
            Type::Function { arity, returns } => write!(f, "function/{arity} -> {returns}"),
            Type::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable machine-readable identifier for this diagnostic family.
    pub code: &'static str,
    /// Static analysis is advisory and never blocks execution.
    pub severity: &'static str,
    pub message: String,
}

impl Diagnostic {
    /// Serialize this diagnostic as one JSON object.
    /// Source ranges are null until the AST preserves lexer spans.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"code\":\"{}\",\"severity\":\"{}\",\"message\":\"{}\",\"range\":null}}",
            self.code,
            self.severity,
            json_escape(&self.message)
        )
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch <= '\u{1f}' => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
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
    return_types: Vec<Vec<Type>>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::new()],
            diagnostics: Vec::new(),
            return_types: Vec::new(),
        }
    }

    pub fn check(program: &Program) -> Vec<Diagnostic> {
        let mut checker = Self::new();
        checker.check_statements(&program.statements);
        checker.diagnostics
    }

    fn report(&mut self, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: "HL-TYPE-001",
            severity: "warning",
            message: message.into(),
        });
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
                    let mut target = self.lookup(&stmt.name);
                    for (position, expression) in stmt.indices.iter().enumerate() {
                        let index = self.infer(expression);
                        let is_last = position + 1 == stmt.indices.len();
                        match target {
                            Type::Array(element) => {
                                if index != Type::Unknown && index != Type::Number {
                                    self.report(format!("el índice {} de '{}' debe ser number, no {}", position, stmt.name, index));
                                }
                                if is_last {
                                    let value = self.infer(&stmt.value);
                                    self.check_compatible(&format!("elemento de '{}'", stmt.name), &element, &value);
                                }
                                target = *element;
                            }
                            Type::Map(key, map_value) if is_last => {
                                self.check_compatible(&format!("clave de '{}'", stmt.name), &key, &index);
                                let value = self.infer(&stmt.value);
                                self.check_compatible(&format!("valor de '{}'", stmt.name), &map_value, &value);
                                target = *map_value;
                            }
                            Type::Map(_, _) => {
                                self.report(format!("la asignación anidada a través de mapas no está soportada para '{}'", stmt.name));
                                if is_last { self.infer(&stmt.value); }
                                target = Type::Unknown;
                            }
                            Type::Unknown => {
                                if is_last { self.infer(&stmt.value); }
                                target = Type::Unknown;
                            }
                            other => {
                                self.report(format!("no se puede indexar el nivel {} de '{}' de tipo {}", position, stmt.name, other));
                                if is_last { self.infer(&stmt.value); }
                                target = Type::Unknown;
                            }
                        }
                    }
                }
                Statement::Print(stmt) => { self.infer(&stmt.value); }
                Statement::Return(expr) => {
                    let ty = self.infer(expr);
                    if let Some(returns) = self.return_types.last_mut() {
                        returns.push(ty);
                    }
                }
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
                let mut element_type = Type::Unknown;
                for item in items {
                    let item_type = self.infer(item);
                    element_type = self.merge_collection_type("elementos de array", element_type, item_type);
                }
                Type::Array(Box::new(element_type))
            }
            Expression::Map(items) => {
                let mut key_type = Type::Unknown;
                let mut value_type = Type::Unknown;
                for (key, value) in items {
                    let current_key = self.infer(key);
                    let current_value = self.infer(value);
                    key_type = self.merge_collection_type("claves de map", key_type, current_key);
                    value_type = self.merge_collection_type("valores de map", value_type, current_value);
                }
                Type::Map(Box::new(key_type), Box::new(value_type))
            }
            Expression::Index { target, index } => {
                let target_type = self.infer(target);
                let index_type = self.infer(index);
                match target_type {
                    Type::Array(element) if index_type != Type::Unknown && index_type != Type::Number => {
                        self.report(format!("el índice requiere number, se obtuvo {index_type}"));
                        Type::Unknown
                    }
                    Type::Array(element) => *element,
                    Type::String if index_type != Type::Unknown && index_type != Type::Number => {
                        self.report(format!("el índice requiere number, se obtuvo {index_type}"));
                        Type::Unknown
                    }
                    Type::String => Type::String,
                    Type::Map(key, value) => {
                        self.check_compatible("clave de map", &key, &index_type);
                        *value
                    }
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
                self.return_types.push(Vec::new());
                for param in params { self.define(param, Type::Unknown); }
                let body_type = self.infer(body);
                let explicit_returns = self.return_types.pop().unwrap_or_default();
                self.pop_scope();
                let returns = if explicit_returns.is_empty() {
                    body_type
                } else {
                    self.merge_return_types(explicit_returns)
                };
                Type::Function { arity: params.len(), returns: Box::new(returns) }
            }
            Expression::Block(statements) => {
                self.push_scope();
                self.check_statements(statements);
                self.pop_scope();
                Type::Unknown
            }
            Expression::FunctionCall { func, args } => {
                if matches!(func.as_ref(), Expression::Identifier(name) if name == "len") {
                    if args.len() != 1 {
                        self.report(format!("len(): se esperaba 1 argumento y se recibieron {}", args.len()));
                        for arg in args { self.infer(arg); }
                        return Type::Unknown;
                    }
                    let argument = self.infer(&args[0]);
                    match argument {
                        Type::String | Type::Array(_) | Type::Map(_, _) | Type::Unknown => Type::Number,
                        other => {
                            self.report(format!("len() requiere string, array o map; se obtuvo {other}"));
                            Type::Unknown
                        }
                    }
                } else {
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
    }

    fn merge_collection_type(&mut self, context: &str, current: Type, next: Type) -> Type {
        match (current, next) {
            (Type::Unknown, other) | (other, Type::Unknown) => other,
            (left, right) if left == right => left,
            (left, right) => {
                self.report(format!("{context}: tipos incompatibles {left} y {right}"));
                Type::Unknown
            }
        }
    }

    fn merge_return_types(&mut self, returns: Vec<Type>) -> Type {
        let Some(first) = returns.first().cloned() else { return Type::Unknown; };
        if returns.iter().all(|ty| *ty == first) {
            first
        } else {
            self.report("una función devuelve tipos incompatibles en distintas ramas");
            Type::Unknown
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
    fn builtin_len_has_numeric_type_for_supported_inputs() {
        assert!(check_source("let a = len(\"hello\"); let b = len([1,2]); let c = len(@(\"x\":1));").is_empty());
    }

    #[test]
    fn builtin_len_reports_invalid_argument_type_and_arity() {
        let diagnostics = check_source("let a = len(42); let b = len();");
        assert!(diagnostics.iter().any(|d| d.message.contains("len() requiere")));
        assert!(diagnostics.iter().any(|d| d.message.contains("len(): se esperaba 1 argumento")));
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
    fn infers_explicit_return_type_from_function_block() {
        let diagnostics = check_source("let make = |x| (return 7;); let n = make(1); n = false;");
        assert!(diagnostics.iter().any(|d| d.message.contains("asignación") && d.message.contains("number")));
    }

    #[test]
    fn detects_incompatible_function_return_branches() {
        let diagnostics = check_source("let choose = |x| (if (x) (return 1;) else (return false;));");
        assert!(diagnostics.iter().any(|d| d.message.contains("tipos incompatibles")));
    }

    #[test]
    fn infers_array_element_type_through_indexing() {
        let diagnostics = check_source("let items = [1, 2, 3]; let value = items[0]; value = false;");
        assert!(diagnostics.iter().any(|d| d.message.contains("asignación") && d.message.contains("number")));
    }

    #[test]
    fn infers_map_key_and_value_types_through_indexing() {
        let diagnostics = check_source("let data = @(\"count\": 1); let value = data[\"count\"]; value = false;");
        assert!(diagnostics.iter().any(|d| d.message.contains("asignación") && d.message.contains("number")));
    }

    #[test]
    fn detects_incompatible_array_element_types() {
        let diagnostics = check_source("let items = [1, false];");
        assert!(diagnostics.iter().any(|d| d.message.contains("elementos de array") && d.message.contains("incompatibles")));
    }

    #[test]
    fn checks_array_element_type_on_index_assignment() {
        let diagnostics = check_source("let items = [1, 2]; items[0] = false;");
        assert!(diagnostics.iter().any(|d| d.message.contains("elemento de 'items'") && d.message.contains("number")));
    }

    #[test]
    fn checks_map_key_and_value_types_on_index_assignment() {
        let diagnostics = check_source("let data = @(\"count\": 1); data[1] = false;");
        assert!(diagnostics.iter().any(|d| d.message.contains("clave de 'data'") && d.message.contains("string")));
        assert!(diagnostics.iter().any(|d| d.message.contains("valor de 'data'") && d.message.contains("number")));
    }

    #[test]
    fn accepts_legacy_string_number_concatenation() {
        assert!(check_source("let result = \"count=\" + 3;").is_empty());
    }

    #[test]
    fn diagnostics_have_stable_machine_readable_metadata() {
        let diagnostics = check_source("let result = true + 2;");
        let diagnostic = diagnostics.first().unwrap();
        assert_eq!(diagnostic.code, "HL-TYPE-001");
        assert_eq!(diagnostic.severity, "warning");
        assert!(diagnostic.to_json().contains("\"range\":null"));
    }

    #[test]
    fn diagnostic_json_escapes_quotes_slashes_and_control_characters() {
        assert_eq!(json_escape("quote: \" slash: \\\\ newline:\n tab:\t"), r#"quote: \" slash: \\\\ newline:\n tab:\t"#);
    }

}
