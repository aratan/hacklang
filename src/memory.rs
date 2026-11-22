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

/// Stable index of a compiler-planned data stack within one activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DataStackId(pub u32);

/// Lexical scope to data-stack assignment. Sibling scopes at the same depth
/// reuse a lane; nested scopes receive a distinct lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeStackPlan {
    pub scope: ScopeId,
    pub stack: DataStackId,
    pub depth: u32,
}

/// Conservative usage facts collected from HIR. These are not yet a
/// control-flow liveness proof and must not be used to shorten storage lives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BindingUsage {
    pub read_count: u32,
    pub assignment_count: u32,
    /// Lexical scopes in which this binding is read, sorted and deduplicated.
    pub read_scopes: Vec<ScopeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPlan {
    pub bindings: HashMap<BindingId, StorageClass>,
    /// Stack assignments for scopes that own at least one scope-region value.
    /// This is planning metadata only; the current C runtime does not consume it.
    pub scope_stacks: HashMap<ScopeId, ScopeStackPlan>,
    /// Read/write facts for future liveness and escape analysis.
    pub usage: HashMap<BindingId, BindingUsage>,
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

fn scope_depth(program: &Program, scope: ScopeId) -> u32 {
    let mut depth = 0;
    let mut current = program.scopes[scope.0 as usize].parent;
    while let Some(parent) = current {
        depth += 1;
        current = program.scopes[parent.0 as usize].parent;
    }
    depth
}

fn plan_scope_stacks(program: &Program, bindings: &HashMap<BindingId, StorageClass>) -> HashMap<ScopeId, ScopeStackPlan> {
    let mut scopes: Vec<ScopeId> = bindings.values().filter_map(|class| match class {
        StorageClass::ScopeRegion(scope) => Some(*scope),
        StorageClass::Inline | StorageClass::OwnedRegion => None,
    }).collect();
    scopes.sort_unstable_by_key(|scope| scope.0);
    scopes.dedup();

    let mut result = HashMap::new();
    for scope in scopes {
        let depth = scope_depth(program, scope);
        // One lane per lexical depth: parent/child scopes never share storage,
        // while sibling scopes can safely reuse it after their scope ends.
        result.insert(scope, ScopeStackPlan {
            scope,
            stack: DataStackId(depth),
            depth,
        });
    }
    result
}


fn collect_expr_usage(expr: &Expression, scope: ScopeId, usage: &mut HashMap<BindingId, BindingUsage>) {
    match &expr.kind {
        ExprKind::Read(binding) => {
            let facts = usage.entry(*binding).or_default();
            facts.read_count += 1;
            facts.read_scopes.push(scope);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_expr_usage(left, scope, usage);
            collect_expr_usage(right, scope, usage);
        }
        ExprKind::Ternary { condition, when_true, when_false } => {
            collect_expr_usage(condition, scope, usage);
            collect_expr_usage(when_true, scope, usage);
            collect_expr_usage(when_false, scope, usage);
        }
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_) => {}
    }
}

fn collect_usage(statements: &[Statement], scope: ScopeId, usage: &mut HashMap<BindingId, BindingUsage>) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Let { value, .. } | StmtKind::Print(value) => {
                collect_expr_usage(value, scope, usage);
            }
            StmtKind::Assign { binding, value } => {
                collect_expr_usage(value, scope, usage);
                usage.entry(*binding).or_default().assignment_count += 1;
            }
            StmtKind::Block { scope: child, statements } => collect_usage(statements, *child, usage),
            StmtKind::If { condition, then_scope, then_body, else_scope, else_body } => {
                collect_expr_usage(condition, scope, usage);
                collect_usage(then_body, *then_scope, usage);
                if let Some(child) = else_scope {
                    collect_usage(else_body, *child, usage);
                }
            }
            StmtKind::While { scope: child, condition, body } => {
                collect_expr_usage(condition, *child, usage);
                collect_usage(body, *child, usage);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn normalize_usage(usage: &mut HashMap<BindingId, BindingUsage>) {
    for facts in usage.values_mut() {
        facts.read_scopes.sort_unstable_by_key(|scope| scope.0);
        facts.read_scopes.dedup();
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
    let scope_stacks = plan_scope_stacks(program, &bindings);
    let mut usage = HashMap::new();
    collect_usage(&program.statements, ScopeId(0), &mut usage);
    normalize_usage(&mut usage);
    MemoryPlan { bindings, scope_stacks, usage }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ast::{Expression as A, LetStatement, PrintStatement, Program as AP, Statement as S}, hir};

    fn lower(statements: Vec<S>) -> Program { hir::lower(&AP { statements }).unwrap() }

    #[test]
    fn sibling_scopes_reuse_a_stack_lane_but_nested_scopes_do_not() {
        let p = lower(vec![
            S::Block(vec![S::Let(LetStatement {
                name: "left".into(),
                value: A::StringLiteral("left".into()),
            })]),
            S::Block(vec![S::Let(LetStatement {
                name: "right".into(),
                value: A::StringLiteral("right".into()),
            })]),
            S::Block(vec![
                S::Let(LetStatement {
                    name: "outer".into(),
                    value: A::StringLiteral("outer".into()),
                }),
                S::Block(vec![S::Let(LetStatement {
                    name: "inner".into(),
                    value: A::StringLiteral("inner".into()),
                })]),
            ]),
        ]);
        let memory = plan(&p);
        let left = match memory.bindings[&BindingId(0)] { StorageClass::ScopeRegion(s) => s, _ => panic!("expected scope storage") };
        let right = match memory.bindings[&BindingId(1)] { StorageClass::ScopeRegion(s) => s, _ => panic!("expected scope storage") };
        let outer = match memory.bindings[&BindingId(2)] { StorageClass::ScopeRegion(s) => s, _ => panic!("expected scope storage") };
        let inner = match memory.bindings[&BindingId(3)] { StorageClass::ScopeRegion(s) => s, _ => panic!("expected scope storage") };
        assert_eq!(memory.scope_stacks[&left].stack, memory.scope_stacks[&right].stack);
        assert_ne!(memory.scope_stacks[&outer].stack, memory.scope_stacks[&inner].stack);
        assert_eq!(memory.scope_stacks[&left].depth, memory.scope_stacks[&right].depth);
        assert_eq!(memory.scope_stacks[&inner].depth, memory.scope_stacks[&outer].depth + 1);
    }

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
    #[test]
    fn unknown_read_assignment_is_promoted_instead_of_assumed_stack_safe() {
        let p = lower(vec![
            S::Let(LetStatement { name: "source".into(), value: A::StringLiteral("value".into()) }),
            S::Let(LetStatement { name: "target".into(), value: A::NumberLiteral(0.0) }),
            S::Assign(crate::ast::AssignStatement {
                name: "target".into(),
                value: A::Identifier("source".into()),
            }),
        ]);
        let plan = plan(&p);
        assert_eq!(plan.bindings[&BindingId(1)], StorageClass::OwnedRegion);
    }

    #[test]
    fn conditional_string_assignment_is_promoted_across_all_control_flow_paths() {
        let p = lower(vec![
            S::Let(LetStatement { name: "target".into(), value: A::NumberLiteral(0.0) }),
            S::If(crate::ast::IfStatement {
                condition: A::BoolLiteral(true),
                true_body: vec![S::Assign(crate::ast::AssignStatement {
                    name: "target".into(),
                    value: A::StringLiteral("escaped".into()),
                })],
                false_body: Some(vec![S::Assign(crate::ast::AssignStatement {
                    name: "target".into(),
                    value: A::NumberLiteral(1.0),
                })]),
            }),
        ]);
        let plan = plan(&p);
        assert_eq!(plan.bindings[&BindingId(0)], StorageClass::OwnedRegion);
    }

    #[test]
    fn string_temporary_from_outer_binding_is_never_confined_to_inner_scope() {
        let p = lower(vec![
            S::Let(LetStatement { name: "source".into(), value: A::StringLiteral("outer".into()) }),
            S::Block(vec![
                S::Let(LetStatement {
                    name: "copy".into(),
                    value: A::Identifier("source".into()),
                }),
            ]),
        ]);
        let plan = plan(&p);
        assert_eq!(plan.bindings[&BindingId(0)], StorageClass::ScopeRegion(ScopeId(0)));
        assert_eq!(plan.bindings[&BindingId(1)], StorageClass::OwnedRegion);
    }

    #[test]
    fn usage_analysis_counts_reads_and_records_nested_lexical_scopes() {
        let p = lower(vec![
            S::Let(LetStatement { name: "value".into(), value: A::NumberLiteral(1.0) }),
            S::Block(vec![
                S::Print(PrintStatement { value: A::Identifier("value".into()) }),
                S::Print(PrintStatement { value: A::Identifier("value".into()) }),
            ]),
            S::Print(PrintStatement { value: A::Identifier("value".into()) }),
        ]);
        let memory = plan(&p);
        let facts = &memory.usage[&BindingId(0)];
        assert_eq!(facts.read_count, 3);
        assert_eq!(facts.read_scopes.len(), 2);
        assert_eq!(facts.read_scopes[0], ScopeId(0));
        assert_eq!(facts.read_scopes[1], ScopeId(1));
        assert_eq!(facts.assignment_count, 0);
    }

    #[test]
    fn usage_analysis_counts_assignments_without_treating_them_as_reads() {
        let p = lower(vec![
            S::Let(LetStatement { name: "value".into(), value: A::NumberLiteral(1.0) }),
            S::Assign(crate::ast::AssignStatement { name: "value".into(), value: A::NumberLiteral(2.0) }),
        ]);
        let facts = &plan(&p).usage[&BindingId(0)];
        assert_eq!(facts.read_count, 0);
        assert_eq!(facts.assignment_count, 1);
    }

}
