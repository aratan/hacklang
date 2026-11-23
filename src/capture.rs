//! Capture analysis over resolved HIR bindings.
//!
//! Names are resolved exactly once by AST -> HIR lowering. This pass consumes
//! BindingId identities and never maintains a second symbol table.

use crate::hir::{BindingId, Capture, ExprKind, Expression, Function, Program, StmtKind, Statement};
use std::collections::HashMap;

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

#[derive(Default)]
struct Access {
    reads: bool,
    writes: bool,
}

fn record(
    program: &Program,
    function: usize,
    binding: BindingId,
    write: bool,
    captures: &mut HashMap<BindingId, Access>,
) -> Result<(), CaptureError> {
    let metadata = program.bindings.get(binding.0 as usize).ok_or_else(|| CaptureError {
        message: format!("BindingId inválido durante análisis de capturas: {}", binding.0),
    })?;
    if metadata.function_owner != Some(program.functions[function].id) {
        let access = captures.entry(binding).or_default();
        if write { access.writes = true; } else { access.reads = true; }
    }
    Ok(())
}

fn expression(
    program: &Program,
    function: usize,
    value: &Expression,
    captures: &mut HashMap<BindingId, Access>,
) -> Result<(), CaptureError> {
    match &value.kind {
        ExprKind::Read(binding) => record(program, function, *binding, false, captures)?,
        ExprKind::Binary { left, right, .. } => {
            expression(program, function, left, captures)?;
            expression(program, function, right, captures)?;
        }
        ExprKind::Ternary { condition, when_true, when_false } => {
            expression(program, function, condition, captures)?;
            expression(program, function, when_true, captures)?;
            expression(program, function, when_false, captures)?;
        }
        ExprKind::Array(items) => {
            for item in items { expression(program, function, item, captures)?; }
        }
        ExprKind::Map(items) => {
            for (key, value) in items {
                expression(program, function, key, captures)?;
                expression(program, function, value, captures)?;
            }
        }
        ExprKind::Length(value) => expression(program, function, value, captures)?,
        ExprKind::Index { target, index } => {
            expression(program, function, target, captures)?;
            expression(program, function, index, captures)?;
        }
        ExprKind::Call { callee, args } => {
            expression(program, function, callee, captures)?;
            for arg in args { expression(program, function, arg, captures)?; }
        }
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_) | ExprKind::Closure(_) => {}
    }
    Ok(())
}

fn statements(
    program: &Program,
    function: usize,
    body: &[Statement],
    captures: &mut HashMap<BindingId, Access>,
) -> Result<(), CaptureError> {
    for statement in body {
        match &statement.kind {
            StmtKind::Let { value, .. } | StmtKind::Print(value) | StmtKind::Return(value) =>
                expression(program, function, value, captures)?,
            StmtKind::Assign { binding, value } => {
                expression(program, function, value, captures)?;
                record(program, function, *binding, true, captures)?;
            }
            StmtKind::IndexAssign { binding, indices, value } => {
                record(program, function, *binding, false, captures)?;
                record(program, function, *binding, true, captures)?;
                for index in indices { expression(program, function, index, captures)?; }
                expression(program, function, value, captures)?;
            }
            StmtKind::Block { statements: nested, .. } =>
                statements(program, function, nested, captures)?,
            StmtKind::If { condition, then_body, else_body, .. } => {
                expression(program, function, condition, captures)?;
                statements(program, function, then_body, captures)?;
                statements(program, function, else_body, captures)?;
            }
            StmtKind::While { condition, body, .. } => {
                expression(program, function, condition, captures)?;
                statements(program, function, body, captures)?;
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
    Ok(())
}

/// Fill each HIR function's direct free-variable set, sorted by BindingId.
pub fn analyze(program: &Program) -> Result<Vec<Function>, CaptureError> {
    let mut functions = program.functions.clone();
    for index in 0..functions.len() {
        let mut captures = HashMap::<BindingId, Access>::new();
        if let Some(value) = &functions[index].body.expression {
            expression(program, index, value, &mut captures)?;
        }
        statements(program, index, &functions[index].body.statements, &mut captures)?;
        let mut ordered: Vec<_> = captures.into_iter().map(|(binding, access)| Capture {
            binding, reads: access.reads, writes: access.writes,
        }).collect();
        ordered.sort_by_key(|capture| capture.binding);
        functions[index].captures = ordered;
    }
    Ok(functions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir;

    fn lower(source: &str) -> Program {
        let mut lexer = crate::lexer::Lexer::new(source.to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let ast = parser.parse_program().unwrap();
        hir::lower(&ast).unwrap()
    }

    #[test]
    fn captures_use_hir_binding_ids_and_ignore_shadowed_parameters() {
        let p = lower("let outside = 7; let add = |x| x + outside;");
        assert_eq!(p.functions.len(), 1);
        let outside = p.bindings.iter().find(|binding| binding.name == "outside").unwrap().id;
        assert_eq!(p.functions[0].captures, vec![Capture { binding: outside, reads: true, writes: false }]);
    }

    #[test]
    fn nested_closure_capture_is_owned_by_the_nested_function() {
        let p = lower("let factory = |x| |y| x + y;");
        assert_eq!(p.functions.len(), 2);
        let x = p.bindings.iter().find(|binding| binding.name == "x").unwrap().id;
        assert!(p.functions[0].captures.is_empty());
        assert_eq!(p.functions[1].parent, Some(p.functions[0].id));
        assert_eq!(p.functions[1].captures, vec![Capture { binding: x, reads: true, writes: false }]);
    }

    #[test]
    fn mutable_outer_capture_records_reads_and_writes() {
        let p = lower("let total = 0; let update = |x| (total = total + x; return total;);");
        let total = p.bindings.iter().find(|binding| binding.name == "total").unwrap().id;
        assert_eq!(p.functions[0].captures, vec![Capture { binding: total, reads: true, writes: true }]);
    }

    #[test]
    fn indexed_capture_is_a_read_modify_write() {
        let p = lower("let values = [1]; let update = |x| (values[0] = x; return 0;);");
        let values = p.bindings.iter().find(|binding| binding.name == "values").unwrap().id;
        assert_eq!(p.functions[0].captures, vec![Capture { binding: values, reads: true, writes: true }]);
    }

    #[test]
    fn function_return_is_lowered_and_capture_analysis_sees_return_expression() {
        let p = lower("let make = |x| (let inner = |y| x + y; return inner;);");
        assert_eq!(p.functions.len(), 2);
        assert!(matches!(p.functions[0].body.statements.last().unwrap().kind, StmtKind::Return(_)));
        let x = p.bindings.iter().find(|binding| binding.name == "x").unwrap().id;
        assert_eq!(p.functions[1].captures, vec![Capture { binding: x, reads: true, writes: false }]);
    }

    #[test]
    fn top_level_return_is_rejected_by_hir() {
        let mut lexer = crate::lexer::Lexer::new("return 1;".to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let ast = parser.parse_program().unwrap();
        assert!(hir::lower(&ast).unwrap_err().message.contains("return fuera"));
    }

    #[test]
    fn builtin_len_is_not_a_function_capture() {
        let p = lower("let values = [1, 2]; let size = |x| len(values) + x;");
        let values = p.bindings.iter().find(|binding| binding.name == "values").unwrap().id;
        assert_eq!(p.functions[0].captures, vec![Capture { binding: values, reads: true, writes: false }]);
    }
}
