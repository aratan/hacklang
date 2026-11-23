//! Deterministic lexical capture analysis for lambda expressions.
//!
//! This pass is deliberately metadata-only: it does not enable native closure
//! code generation or change interpreter semantics. Binding and function IDs
//! are assigned in AST traversal order so later HIR/runtime phases can consume
//! stable identities without re-resolving names.

use crate::ast::{Expression, Program, Statement};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CaptureBindingId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureBinding {
    pub id: CaptureBindingId,
    pub name: String,
    /// The lambda activation that declares this binding; None means module scope.
    pub function_owner: Option<FunctionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCaptures {
    pub id: FunctionId,
    pub parent: Option<FunctionId>,
    pub params: Vec<CaptureBindingId>,
    /// Direct free-variable references, sorted by stable binding ID.
    pub captures: Vec<CaptureBindingId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureAnalysis {
    pub bindings: Vec<CaptureBinding>,
    pub functions: Vec<FunctionCaptures>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureError {
    pub message: String,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CaptureError {}

#[derive(Debug)]
struct FunctionBuilder {
    id: FunctionId,
    parent: Option<FunctionId>,
    params: Vec<CaptureBindingId>,
    captures: HashSet<CaptureBindingId>,
}

struct Analyzer {
    bindings: Vec<CaptureBinding>,
    functions: Vec<FunctionBuilder>,
    scopes: Vec<HashMap<String, CaptureBindingId>>,
    function_stack: Vec<FunctionId>,
}

impl Analyzer {
    fn new() -> Self {
        Self {
            bindings: Vec::new(),
            functions: Vec::new(),
            scopes: vec![HashMap::new()],
            function_stack: Vec::new(),
        }
    }

    fn current_function(&self) -> Option<FunctionId> {
        self.function_stack.last().copied()
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn leave_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn declare(&mut self, name: &str) -> CaptureBindingId {
        let id = CaptureBindingId(self.bindings.len() as u32);
        self.bindings.push(CaptureBinding {
            id,
            name: name.to_string(),
            function_owner: self.current_function(),
        });
        self.scopes.last_mut().expect("module scope exists").insert(name.to_string(), id);
        id
    }

    fn resolve(&self, name: &str) -> Result<CaptureBindingId, CaptureError> {
        self.scopes.iter().rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| CaptureError {
                message: format!("Variable no resuelta durante análisis de capturas: '{}'", name),
            })
    }

    fn reference(&mut self, name: &str) -> Result<(), CaptureError> {
        let binding = self.resolve(name)?;
        if let Some(function) = self.current_function() {
            let owner = self.bindings[binding.0 as usize].function_owner;
            if owner != Some(function) {
                self.functions[function.0 as usize].captures.insert(binding);
            }
        }
        Ok(())
    }

    fn lambda(&mut self, params: &[String], body: &Expression) -> Result<(), CaptureError> {
        let id = FunctionId(self.functions.len() as u32);
        let parent = self.current_function();
        self.functions.push(FunctionBuilder {
            id,
            parent,
            params: Vec::with_capacity(params.len()),
            captures: HashSet::new(),
        });

        self.function_stack.push(id);
        self.enter_scope();
        for param in params {
            let binding = self.declare(param);
            self.functions[id.0 as usize].params.push(binding);
        }

        let result = self.expression(body);
        self.leave_scope();
        self.function_stack.pop();
        result
    }

    fn expression(&mut self, expression: &Expression) -> Result<(), CaptureError> {
        match expression {
            Expression::Identifier(name) => self.reference(name),
            Expression::NumberLiteral(_) | Expression::BoolLiteral(_) | Expression::StringLiteral(_) => Ok(()),
            Expression::Infix { left, right, .. } => {
                self.expression(left)?;
                self.expression(right)
            }
            Expression::Ternary { condition, true_branch, false_branch } => {
                self.expression(condition)?;
                self.expression(true_branch)?;
                self.expression(false_branch)
            }
            Expression::Lambda { params, body } => self.lambda(params, body),
            Expression::Block(statements) => {
                self.enter_scope();
                let result = self.statements(statements);
                self.leave_scope();
                result
            }
            Expression::FunctionCall { func, args } => {
                self.expression(func)?;
                for arg in args {
                    self.expression(arg)?;
                }
                Ok(())
            }
            Expression::Array(items) => {
                for item in items {
                    self.expression(item)?;
                }
                Ok(())
            }
            Expression::Map(items) => {
                for (key, value) in items {
                    self.expression(key)?;
                    self.expression(value)?;
                }
                Ok(())
            }
            Expression::Index { target, index } => {
                self.expression(target)?;
                self.expression(index)
            }
        }
    }

    fn statements(&mut self, statements: &[Statement]) -> Result<(), CaptureError> {
        for statement in statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    fn statement(&mut self, statement: &Statement) -> Result<(), CaptureError> {
        match statement {
            Statement::Let(binding) => {
                // Match the interpreter/HIR declaration rule: the initializer
                // is resolved before the new name enters the current scope.
                self.expression(&binding.value)?;
                self.declare(&binding.name);
                Ok(())
            }
            Statement::Assign(assignment) => {
                self.expression(&assignment.value)?;
                self.reference(&assignment.name)
            }
            Statement::IndexAssign(assignment) => {
                self.reference(&assignment.name)?;
                for index in &assignment.indices {
                    self.expression(index)?;
                }
                self.expression(&assignment.value)
            }
            Statement::Print(print) => self.expression(&print.value),
            Statement::Return(value) => self.expression(value),
            Statement::Break | Statement::Continue => Ok(()),
            Statement::Block(statements) => {
                self.enter_scope();
                let result = self.statements(statements);
                self.leave_scope();
                result
            }
            Statement::While(loop_statement) => {
                self.enter_scope();
                let result = (|| {
                    self.expression(&loop_statement.condition)?;
                    self.statements(&loop_statement.body)
                })();
                self.leave_scope();
                result
            }
            Statement::If(branch) => {
                self.expression(&branch.condition)?;
                self.enter_scope();
                let then_result = self.statements(&branch.true_body);
                self.leave_scope();
                then_result?;
                if let Some(body) = &branch.false_body {
                    self.enter_scope();
                    let else_result = self.statements(body);
                    self.leave_scope();
                    else_result?;
                }
                Ok(())
            }
        }
    }

    fn finish(self) -> CaptureAnalysis {
        let functions = self.functions.into_iter().map(|function| {
            let mut captures: Vec<_> = function.captures.into_iter().collect();
            captures.sort_unstable();
            FunctionCaptures {
                id: function.id,
                parent: function.parent,
                params: function.params,
                captures,
            }
        }).collect();
        CaptureAnalysis { bindings: self.bindings, functions }
    }
}

/// Analyze every lambda in a parsed program without changing its execution path.
pub fn analyze(program: &Program) -> Result<CaptureAnalysis, CaptureError> {
    let mut analyzer = Analyzer::new();
    analyzer.statements(&program.statements)?;
    Ok(analyzer.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_source(source: &str) -> CaptureAnalysis {
        let mut lexer = crate::lexer::Lexer::new(source.to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        analyze(&program).unwrap()
    }

    #[test]
    fn captures_outer_binding_by_identity_and_not_parameter_name() {
        let result = analyze_source("let outside = 7; let add = |x| x + outside;");
        assert_eq!(result.functions.len(), 1);
        let outside = result.bindings.iter().find(|binding| binding.name == "outside").unwrap().id;
        assert_eq!(result.functions[0].captures, vec![outside]);
        assert_eq!(result.functions[0].params.len(), 1);
    }

    #[test]
    fn nested_lambda_captures_parent_parameter_not_its_own_parameter() {
        let result = analyze_source("let factory = |x| |y| x + y;");
        assert_eq!(result.functions.len(), 2);
        let x = result.bindings.iter().find(|binding| binding.name == "x").unwrap().id;
        assert_eq!(result.functions[0].parent, None);
        assert_eq!(result.functions[0].captures, Vec::<CaptureBindingId>::new());
        assert_eq!(result.functions[1].parent, Some(FunctionId(0)));
        assert_eq!(result.functions[1].captures, vec![x]);
    }

    #[test]
    fn shadowing_resolves_to_inner_binding_without_false_capture() {
        let result = analyze_source("let value = 1; let identity = |value| value;");
        assert_eq!(result.functions.len(), 1);
        assert!(result.functions[0].captures.is_empty());
    }

    #[test]
    fn block_lambda_captures_outer_binding_through_returned_inner_closure() {
        let result = analyze_source("let factory = |x| (let inner = |y| x + y; return inner;);");
        assert_eq!(result.functions.len(), 2);
        let x = result.bindings.iter().find(|binding| binding.name == "x").unwrap().id;
        assert_eq!(result.functions[1].captures, vec![x]);
    }

    #[test]
    fn captured_assignment_is_a_capture_too() {
        let result = analyze_source("let total = 0; let update = |x| (total = total + x; return total;);");
        let total = result.bindings.iter().find(|binding| binding.name == "total").unwrap().id;
        assert_eq!(result.functions[0].captures, vec![total]);
    }

    #[test]
    fn unresolved_names_fail_instead_of_becoming_accidental_captures() {
        let mut lexer = crate::lexer::Lexer::new("let f = |x| x + missing;".to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let program = parser.parse_program().unwrap();
        assert!(analyze(&program).unwrap_err().message.contains("missing"));
    }
}
