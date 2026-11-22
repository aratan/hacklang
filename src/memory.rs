//! Conservative storage planning for HIR.
//!
//! This module is analysis-only: it does not allocate memory or change runtime
//! semantics. Unknown lifetimes are promoted to an owned region, never shortened.

use crate::hir::{BindingId, BinaryOp, ExprKind, Expression, Program, ScopeId, StmtKind, Statement};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    /// Immediate scalar with no dynamic payload.
    Inline,
    /// Dynamic payload is confined to the binding's lexical scope.
    ScopeRegion(ScopeId),
    /// Payload may outlive its declaring scope; use a longer-lived owner.
    OwnedRegion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPlan {
    pub bindings: HashMap<BindingId, StorageClass>,
}

fn is_dynamic_string(expr: &Expression) -> Option<bool> {
    match &expr.kind {
        ExprKind::String(_) => Some(true),
        ExprKind::Number(_) | ExprKind::Bool(_) => Some(false),
        // String concatenation can allocate. Other binary operations are scalar
        // in the currently supported HIR subset.
        ExprKind::Binary { op: BinaryOp::Add, left, right } => {
            let left = is_dynamic_string(left)?;
            let right = is_dynamic_string(right)?;
            Some(left || right)
        }
        ExprKind::Binary { .. } => Some(false),
        ExprKind::Ternary { when_true, when_false, .. } => {
            let left = is_dynamic_string(when_true)?;
            let right = is_dynamic_string(when_false)?;
            Some(left || right)
        }
        ExprKind::Read(_) => None,
    }
}

fn mark_assignments(statements: &[Statement], classes: &mut HashMap<BindingId, StorageClass>) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Assign { binding, value } => {
                match is_dynamic_string(value) {
                    Some(true) | None => { classes.insert(*binding, StorageClass::OwnedRegion); }
                    Some(false) => {}
                }
            }
            StmtKind::Block { statements, .. } => mark_assignments(statements, classes),
            StmtKind::If { then_body, else_body, .. } => {
                mark_assignments(then_body, classes);
                mark_assignments(else_body, classes);
            }
            StmtKind::While { body, .. } => mark_assignments(body, classes),
            StmtKind::Let { .. } | StmtKind::Print(_) | StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

pub fn plan(program: &Program) -> MemoryPlan {
    let mut bindings = HashMap::new();
    fn visit(statements: &[Statement], program: &Program, result: &mut HashMap<BindingId, StorageClass>) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Let { binding, value } => {
                    let scope = program.bindings[binding.0 as usize].scope;
                    let class = match is_dynamic_string(value) {
                        Some(false) => StorageClass::Inline,
                        Some(true) => StorageClass::ScopeRegion(scope),
                        None => StorageClass::OwnedRegion,
                    };
                    result.insert(*binding, class);
                }
                StmtKind::Block { statements, .. } => visit(statements, program, result),
                StmtKind::If { then_body, else_body, .. } => {
                    visit(then_body, program, result);
                    visit(else_body, program, result);
                }
                StmtKind::While { body, .. } => visit(body, program, result),
                StmtKind::Assign { .. } | StmtKind::Print(_) | StmtKind::Break | StmtKind::Continue => {}
            }
        }
    }
    visit(&program.statements, program, &mut bindings);
    mark_assignments(&program.statements, &mut bindings);
    MemoryPlan { bindings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ast::{Expression as A, LetStatement, Program as AP, Statement as S}, hir};

    fn lower(statements: Vec<S>) -> Program { hir::lower(&AP { statements }).unwrap() }

    #[test]
    fn scalar_bindings_are_inline_and_strings_start_in_their_scope_region() {
        let p = lower(vec![
            S::Let(LetStatement { name: "n".into(), value: A::NumberLiteral(7.0) }),
            S::Let(LetStatement { name: "s".into(), value: A::StringLiteral("hello".into()) }),
        ]);
        let plan = plan(&p);
        assert_eq!(plan.bindings[&BindingId(0)], StorageClass::Inline);
        assert_eq!(plan.bindings[&BindingId(1)], StorageClass::ScopeRegion(ScopeId(0)));
    }

    #[test]
    fn string_values_assigned_to_existing_bindings_are_conservatively_promoted() {
        let p = lower(vec![
            S::Let(LetStatement { name: "message".into(), value: A::StringLiteral("outer".into()) }),
            S::Block(vec![
                S::Assign(crate::ast::AssignStatement { name: "message".into(), value: A::StringLiteral("inner".into()) }),
            ]),
        ]);
        let plan = plan(&p);
        assert_eq!(plan.bindings[&BindingId(0)], StorageClass::OwnedRegion);
    }

    #[test]
    fn string_concatenation_is_treated_as_dynamic_storage() {
        let p = lower(vec![S::Let(LetStatement {
            name: "message".into(),
            value: A::Infix {
                left: Box::new(A::StringLiteral("value=".into())),
                operator: crate::lexer::Token::Plus,
                right: Box::new(A::NumberLiteral(3.0)),
            },
        })]);
        let plan = plan(&p);
        assert_eq!(plan.bindings[&BindingId(0)], StorageClass::ScopeRegion(ScopeId(0)));
    }
}
