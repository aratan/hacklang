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
    /// Monotonic event index at which the binding is declared.
    pub declaration_position: Option<u32>,
    pub first_use_position: Option<u32>,
    pub last_use_position: Option<u32>,
    pub last_write_position: Option<u32>,
    /// True when a loop may read/write this binding across iterations.
    pub loop_carried: bool,
    /// Lexical scopes in which this binding is read, sorted and deduplicated.
    pub read_scopes: Vec<ScopeId>,
}


/// Basic block in a control-flow graph built from structured HIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFlowBlock {
    pub id: u32,
    pub uses: std::collections::HashSet<BindingId>,
    pub defs: std::collections::HashSet<BindingId>,
    pub successors: Vec<u32>,
    pub live_in: std::collections::HashSet<BindingId>,
    pub live_out: std::collections::HashSet<BindingId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControlFlowGraph {
    pub blocks: Vec<ControlFlowBlock>,
    pub entry: u32,
    pub exit: u32,
}

/// Physical slot assignment within a planned data stack. Metadata only:
/// the runtime does not consume these offsets yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSlotPlan {
    pub stack: DataStackId,
    pub slot: u32,
}

/// Why a dynamic value must outlive the scope where it is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionReason {
    /// A value produced in a nested scope is assigned to an outer binding.
    AssignedToLongerLivedBinding,
    /// A value read from a nested-scope binding is assigned to an outer binding.
    ReadFromShorterLivedBinding,
}

/// Backend-neutral memory operation shared by the Rust and C backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryOperationKind { Promote }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryOperation {
    pub kind: MemoryOperationKind,
    pub destination: BindingId,
    pub source: Option<BindingId>,
    pub produced_in: ScopeId,
    pub destination_scope: ScopeId,
    pub reason: PromotionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPlan {
    pub bindings: HashMap<BindingId, StorageClass>,
    /// Stack assignments for scopes that own at least one scope-region value.
    /// This is planning metadata only; the current C runtime does not consume it.
    pub scope_stacks: HashMap<ScopeId, ScopeStackPlan>,
    /// Read/write facts for diagnostics and future escape analysis.
    pub usage: HashMap<BindingId, BindingUsage>,
    /// Structured control-flow graph and solved live-in/live-out sets.
    pub cfg: ControlFlowGraph,
    /// Interference graph for bindings eligible for frame/region slot reuse.
    pub interference: HashMap<BindingId, std::collections::HashSet<BindingId>>,
    /// Greedy, deterministic slot assignment, partitioned by data stack.
    pub binding_slots: HashMap<BindingId, BindingSlotPlan>,
    /// Number of slots reserved in each data stack.
    pub stack_slot_counts: HashMap<DataStackId, u32>,
    /// Proven lifetime extensions discovered in assignments. Not yet consumed by C.
    pub operations: Vec<MemoryOperation>,
}


fn new_cfg_block(cfg: &mut ControlFlowGraph) -> u32 {
    let id = cfg.blocks.len() as u32;
    cfg.blocks.push(ControlFlowBlock { id, uses: Default::default(), defs: Default::default(), successors: Vec::new(), live_in: Default::default(), live_out: Default::default() });
    id
}
fn add_edge(cfg: &mut ControlFlowGraph, from: u32, to: u32) {
    let successors = &mut cfg.blocks[from as usize].successors;
    if !successors.contains(&to) { successors.push(to); }
}
fn expr_uses(expr: &Expression, uses: &mut std::collections::HashSet<BindingId>) {
    match &expr.kind {
        ExprKind::Read(id) => { uses.insert(*id); }
        ExprKind::Binary { left, right, .. } => { expr_uses(left, uses); expr_uses(right, uses); }
        ExprKind::Ternary { condition, when_true, when_false } => { expr_uses(condition, uses); expr_uses(when_true, uses); expr_uses(when_false, uses); }
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_) => {}
    }
}
fn build_cfg_sequence(
    statements: &[Statement], incoming: Vec<u32>, cfg: &mut ControlFlowGraph,
    breaks: &mut Vec<u32>, continues: &mut Vec<u32>,
) -> Vec<u32> {
    let mut current = incoming;
    for statement in statements {
        match &statement.kind {
            StmtKind::Let { binding, value } | StmtKind::Assign { binding, value } => {
                let id = new_cfg_block(cfg);
                for prev in current.drain(..) { add_edge(cfg, prev, id); }
                expr_uses(value, &mut cfg.blocks[id as usize].uses);
                cfg.blocks[id as usize].defs.insert(*binding);
                current.push(id);
            }
            StmtKind::Print(value) => {
                let id = new_cfg_block(cfg);
                for prev in current.drain(..) { add_edge(cfg, prev, id); }
                expr_uses(value, &mut cfg.blocks[id as usize].uses);
                current.push(id);
            }
            StmtKind::Block { statements, .. } => current = build_cfg_sequence(statements, current, cfg, breaks, continues),
            StmtKind::If { condition, then_body, else_body, .. } => {
                let branch = new_cfg_block(cfg);
                for prev in current.drain(..) { add_edge(cfg, prev, branch); }
                expr_uses(condition, &mut cfg.blocks[branch as usize].uses);
                let then_exits = build_cfg_sequence(then_body, vec![branch], cfg, breaks, continues);
                let else_exits = if else_body.is_empty() { vec![branch] } else { build_cfg_sequence(else_body, vec![branch], cfg, breaks, continues) };
                let join = new_cfg_block(cfg);
                for exit in then_exits.into_iter().chain(else_exits) { add_edge(cfg, exit, join); }
                current.push(join);
            }
            StmtKind::While { condition, body, .. } => {
                let header = new_cfg_block(cfg);
                for prev in current.drain(..) { add_edge(cfg, prev, header); }
                expr_uses(condition, &mut cfg.blocks[header as usize].uses);
                let after = new_cfg_block(cfg);
                add_edge(cfg, header, after);
                breaks.push(after); continues.push(header);
                let body_exits = build_cfg_sequence(body, vec![header], cfg, breaks, continues);
                breaks.pop(); continues.pop();
                for exit in body_exits { add_edge(cfg, exit, header); }
                current.push(after);
            }
            StmtKind::Break | StmtKind::Continue => {
                let id = new_cfg_block(cfg);
                for prev in current.drain(..) { add_edge(cfg, prev, id); }
                let target = if matches!(&statement.kind, StmtKind::Break) { breaks.last() } else { continues.last() };
                if let Some(target) = target { add_edge(cfg, id, *target); }
            }
        }
    }
    current
}
fn build_control_flow_graph(program: &Program) -> ControlFlowGraph {
    let mut cfg = ControlFlowGraph::default();
    let entry = new_cfg_block(&mut cfg); cfg.entry = entry;
    let exits = build_cfg_sequence(&program.statements, vec![entry], &mut cfg, &mut Vec::new(), &mut Vec::new());
    let exit = new_cfg_block(&mut cfg);
    for from in exits { add_edge(&mut cfg, from, exit); }
    cfg.exit = exit;
    cfg
}
fn solve_liveness(cfg: &mut ControlFlowGraph) {
    let mut changed = true;
    while changed {
        changed = false;
        for index in (0..cfg.blocks.len()).rev() {
            let mut out = std::collections::HashSet::new();
            for successor in cfg.blocks[index].successors.clone() { out.extend(cfg.blocks[successor as usize].live_in.iter().copied()); }
            let mut input = cfg.blocks[index].uses.clone();
            input.extend(out.difference(&cfg.blocks[index].defs).copied());
            if out != cfg.blocks[index].live_out || input != cfg.blocks[index].live_in {
                cfg.blocks[index].live_out = out; cfg.blocks[index].live_in = input; changed = true;
            }
        }
    }
}


fn stack_for_binding(
    binding: BindingId,
    class: StorageClass,
    program: &Program,
    scope_stacks: &HashMap<ScopeId, ScopeStackPlan>,
) -> Option<DataStackId> {
    match class {
        StorageClass::Inline => Some(DataStackId(u32::MAX)),
        StorageClass::ScopeRegion(scope) => scope_stacks.get(&scope).map(|plan| plan.stack),
        StorageClass::OwnedRegion => {
            let _ = (binding, program);
            None
        }
    }
}

fn build_interference(
    cfg: &ControlFlowGraph,
    bindings: &HashMap<BindingId, StorageClass>,
    program: &Program,
    scope_stacks: &HashMap<ScopeId, ScopeStackPlan>,
) -> HashMap<BindingId, std::collections::HashSet<BindingId>> {
    let eligible: std::collections::HashSet<BindingId> = bindings.iter()
        .filter_map(|(id, class)| stack_for_binding(*id, *class, program, scope_stacks).map(|_| *id))
        .collect();
    let mut graph: HashMap<BindingId, std::collections::HashSet<BindingId>> =
        eligible.iter().map(|id| (*id, Default::default())).collect();

    let mut connect = |a: BindingId, b: BindingId| {
        if a == b || !eligible.contains(&a) || !eligible.contains(&b) { return; }
        let same_stack = stack_for_binding(a, bindings[&a], program, scope_stacks)
            == stack_for_binding(b, bindings[&b], program, scope_stacks);
        if same_stack {
            graph.get_mut(&a).unwrap().insert(b);
            graph.get_mut(&b).unwrap().insert(a);
        }
    };

    for block in &cfg.blocks {
        let mut simultaneous = block.live_in.clone();
        simultaneous.extend(block.live_out.iter().copied());
        for defined in &block.defs {
            for live in &block.live_out { connect(*defined, *live); }
        }
        let ids: Vec<_> = simultaneous.into_iter().collect();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() { connect(ids[i], ids[j]); }
        }
    }
    graph
}

fn assign_binding_slots(
    graph: &HashMap<BindingId, std::collections::HashSet<BindingId>>,
    bindings: &HashMap<BindingId, StorageClass>,
    program: &Program,
    scope_stacks: &HashMap<ScopeId, ScopeStackPlan>,
) -> (HashMap<BindingId, BindingSlotPlan>, HashMap<DataStackId, u32>) {
    let mut ordered: Vec<_> = graph.keys().copied().collect();
    ordered.sort_unstable_by_key(|id| {
        (std::cmp::Reverse(graph.get(id).map_or(0, |edges| edges.len())), id.0)
    });
    let mut assigned: HashMap<BindingId, BindingSlotPlan> = HashMap::new();
    let mut counts: HashMap<DataStackId, u32> = HashMap::new();

    for binding in ordered {
        let stack = stack_for_binding(binding, bindings[&binding], program, scope_stacks)
            .expect("only eligible bindings have interference nodes");
        let unavailable: std::collections::HashSet<u32> = graph[&binding].iter()
            .filter_map(|neighbor| assigned.get(neighbor))
            .filter(|slot| slot.stack == stack)
            .map(|slot| slot.slot)
            .collect();
        let mut slot = 0;
        while unavailable.contains(&slot) { slot += 1; }
        assigned.insert(binding, BindingSlotPlan { stack, slot });
        let count = counts.entry(stack).or_insert(0);
        *count = (*count).max(slot + 1);
    }
    (assigned, counts)
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


fn touch_binding(
    binding: BindingId,
    position: u32,
    usage: &mut HashMap<BindingId, BindingUsage>,
    loop_touched: &mut [std::collections::HashSet<BindingId>],
) {
    let facts = usage.entry(binding).or_default();
    facts.first_use_position = Some(facts.first_use_position.map_or(position, |old| old.min(position)));
    facts.last_use_position = Some(facts.last_use_position.map_or(position, |old| old.max(position)));
    for touched in loop_touched {
        touched.insert(binding);
    }
}

fn collect_expr_usage(
    expr: &Expression,
    scope: ScopeId,
    usage: &mut HashMap<BindingId, BindingUsage>,
    position: &mut u32,
    loop_touched: &mut [std::collections::HashSet<BindingId>],
) {
    match &expr.kind {
        ExprKind::Read(binding) => {
            *position += 1;
            let facts = usage.entry(*binding).or_default();
            facts.read_count += 1;
            facts.read_scopes.push(scope);
            touch_binding(*binding, *position, usage, loop_touched);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_expr_usage(left, scope, usage, position, loop_touched);
            collect_expr_usage(right, scope, usage, position, loop_touched);
        }
        ExprKind::Ternary { condition, when_true, when_false } => {
            collect_expr_usage(condition, scope, usage, position, loop_touched);
            collect_expr_usage(when_true, scope, usage, position, loop_touched);
            collect_expr_usage(when_false, scope, usage, position, loop_touched);
        }
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_) => {}
    }
}

fn collect_usage(
    statements: &[Statement],
    scope: ScopeId,
    usage: &mut HashMap<BindingId, BindingUsage>,
    position: &mut u32,
    loop_touched: &mut Vec<std::collections::HashSet<BindingId>>,
) {
    for statement in statements {
        *position += 1;
        match &statement.kind {
            StmtKind::Let { binding, value } => {
                usage.entry(*binding).or_default().declaration_position = Some(*position);
                collect_expr_usage(value, scope, usage, position, loop_touched);
            }
            StmtKind::Print(value) => {
                collect_expr_usage(value, scope, usage, position, loop_touched);
            }
            StmtKind::Assign { binding, value } => {
                collect_expr_usage(value, scope, usage, position, loop_touched);
                *position += 1;
                let facts = usage.entry(*binding).or_default();
                facts.assignment_count += 1;
                facts.last_write_position = Some(*position);
                touch_binding(*binding, *position, usage, loop_touched);
            }
            StmtKind::Block { scope: child, statements } => {
                collect_usage(statements, *child, usage, position, loop_touched);
            }
            StmtKind::If { condition, then_scope, then_body, else_scope, else_body } => {
                collect_expr_usage(condition, scope, usage, position, loop_touched);
                collect_usage(then_body, *then_scope, usage, position, loop_touched);
                if let Some(child) = else_scope {
                    collect_usage(else_body, *child, usage, position, loop_touched);
                }
            }
            StmtKind::While { scope: child, condition, body } => {
                let mut touched = std::collections::HashSet::new();
                loop_touched.push(std::mem::take(&mut touched));
                collect_expr_usage(condition, *child, usage, position, loop_touched);
                collect_usage(body, *child, usage, position, loop_touched);
                let touched = loop_touched.pop().expect("loop tracking stack is balanced");
                *position += 1; // Conservative loop-exit boundary.
                for binding in touched {
                    let facts = usage.entry(binding).or_default();
                    facts.loop_carried = true;
                    facts.last_use_position = Some(facts.last_use_position.map_or(*position, |old| old.max(*position)));
                }
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

fn is_scope_ancestor(program: &Program, ancestor: ScopeId, mut child: ScopeId) -> bool {
    loop {
        if ancestor == child { return true; }
        match program.scopes[child.0 as usize].parent {
            Some(parent) => child = parent,
            None => return false,
        }
    }
}

fn collect_operations(
    statements: &[Statement],
    scope: ScopeId,
    program: &Program,
    bindings: &HashMap<BindingId, StorageClass>,
    operations: &mut Vec<MemoryOperation>,
) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Assign { binding, value } => {
                let destination_scope = program.bindings[binding.0 as usize].scope;
                let source = match &value.kind { ExprKind::Read(source) => Some(*source), _ => None };
                let dynamic = match source {
                    Some(source) => !matches!(bindings.get(&source), Some(StorageClass::Inline)),
                    None => is_dynamic_string(value) == Some(true),
                };
                let source_is_shorter = source.is_some_and(|id| {
                    let source_scope = program.bindings[id.0 as usize].scope;
                    !is_scope_ancestor(program, source_scope, destination_scope)
                });
                if dynamic && is_scope_ancestor(program, destination_scope, scope)
                    && destination_scope != scope
                    && (source.is_none() || source_is_shorter)
                {
                    operations.push(MemoryOperation {
                        kind: MemoryOperationKind::Promote,
                        destination: *binding,
                        source,
                        produced_in: scope,
                        destination_scope,
                        reason: if source_is_shorter {
                            PromotionReason::ReadFromShorterLivedBinding
                        } else {
                            PromotionReason::AssignedToLongerLivedBinding
                        },
                    });
                }
            }
            StmtKind::Block { scope: child, statements } => collect_operations(statements, *child, program, bindings, operations),
            StmtKind::If { then_scope, then_body, else_scope, else_body, .. } => {
                collect_operations(then_body, *then_scope, program, bindings, operations);
                if let Some(child) = else_scope { collect_operations(else_body, *child, program, bindings, operations); }
            }
            StmtKind::While { scope: child, body, .. } => collect_operations(body, *child, program, bindings, operations),
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
    let scope_stacks = plan_scope_stacks(program, &bindings);
    let mut usage = HashMap::new();
    let mut position = 0;
    let mut loop_touched = Vec::new();
    collect_usage(&program.statements, ScopeId(0), &mut usage, &mut position, &mut loop_touched);
    normalize_usage(&mut usage);
    let mut cfg = build_control_flow_graph(program);
    solve_liveness(&mut cfg);
    let interference = build_interference(&cfg, &bindings, program, &scope_stacks);
    let (binding_slots, stack_slot_counts) =
        assign_binding_slots(&interference, &bindings, program, &scope_stacks);
    let mut operations = Vec::new();
    collect_operations(&program.statements, ScopeId(0), program, &bindings, &mut operations);
    MemoryPlan { bindings, scope_stacks, usage, cfg, interference, binding_slots, stack_slot_counts, operations }
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
    fn records_promotion_when_inner_dynamic_value_is_assigned_to_outer_binding() {
        let p = lower(vec![
            S::Let(LetStatement { name: "outer".into(), value: A::StringLiteral("initial".into()) }),
            S::Block(vec![S::Assign(crate::ast::AssignStatement {
                name: "outer".into(), value: A::StringLiteral("inner".into()),
            })]),
        ]);
        let memory = plan(&p);
        assert_eq!(memory.operations.len(), 1);
        let decision = memory.operations[0];
        assert_eq!(decision.destination, BindingId(0));
        assert_eq!(decision.source, None);
        assert_eq!(decision.destination_scope, ScopeId(0));
        assert_eq!(decision.kind, MemoryOperationKind::Promote);
        assert_eq!(decision.reason, PromotionReason::AssignedToLongerLivedBinding);
        assert_eq!(memory.bindings[&BindingId(0)], StorageClass::OwnedRegion);
    }

    #[test]
    fn records_source_binding_when_shorter_lived_value_flows_to_outer_owner() {
        let p = lower(vec![
            S::Let(LetStatement { name: "outer".into(), value: A::StringLiteral("initial".into()) }),
            S::Block(vec![
                S::Let(LetStatement { name: "inner".into(), value: A::StringLiteral("temporary".into()) }),
                S::Assign(crate::ast::AssignStatement {
                    name: "outer".into(), value: A::Identifier("inner".into()),
                }),
            ]),
        ]);
        let memory = plan(&p);
        assert_eq!(memory.operations.len(), 1);
        let decision = memory.operations[0];
        assert_eq!(decision.destination, BindingId(0));
        assert_eq!(decision.source, Some(BindingId(1)));
        assert_eq!(decision.reason, PromotionReason::ReadFromShorterLivedBinding);
    }


    #[test]
    fn does_not_promote_value_read_from_an_outer_scope() {
        let p = lower(vec![
            S::Let(LetStatement { name: "outer".into(), value: A::StringLiteral("stable".into()) }),
            S::Block(vec![S::Assign(crate::ast::AssignStatement {
                name: "outer".into(), value: A::Identifier("outer".into()),
            })]),
        ]);
        assert!(plan(&p).operations.is_empty());
    }

    #[test]
    fn does_not_emit_promotion_for_same_scope_assignment() {
        let p = lower(vec![
            S::Let(LetStatement { name: "value".into(), value: A::StringLiteral("initial".into()) }),
            S::Assign(crate::ast::AssignStatement { name: "value".into(), value: A::StringLiteral("next".into()) }),
        ]);
        assert!(plan(&p).operations.is_empty());
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

    #[test]
    fn usage_analysis_builds_conservative_live_interval_for_reads_after_nested_scope() {
        let p = lower(vec![
            S::Let(LetStatement { name: "value".into(), value: A::NumberLiteral(1.0) }),
            S::Block(vec![S::Print(PrintStatement { value: A::Identifier("value".into()) })]),
            S::Print(PrintStatement { value: A::Identifier("value".into()) }),
        ]);
        let facts = &plan(&p).usage[&BindingId(0)];
        assert!(facts.declaration_position.is_some());
        assert!(facts.first_use_position.unwrap() > facts.declaration_position.unwrap());
        assert!(facts.last_use_position.unwrap() >= facts.first_use_position.unwrap());
        assert!(!facts.loop_carried);
    }

    #[test]
    fn usage_inside_loop_is_kept_live_through_the_loop_exit() {
        let p = lower(vec![
            S::Let(LetStatement { name: "counter".into(), value: A::NumberLiteral(3.0) }),
            S::While(crate::ast::WhileStatement {
                condition: A::Infix {
                    left: Box::new(A::Identifier("counter".into())),
                    operator: crate::lexer::Token::Greater,
                    right: Box::new(A::NumberLiteral(0.0)),
                },
                body: vec![S::Print(PrintStatement { value: A::Identifier("counter".into()) })],
            }),
            S::Print(PrintStatement { value: A::Identifier("counter".into()) }),
        ]);
        let facts = &plan(&p).usage[&BindingId(0)];
        assert!(facts.loop_carried);
        assert!(facts.last_use_position.unwrap() > facts.first_use_position.unwrap());
        assert!(facts.last_use_position.unwrap() >= facts.last_write_position.unwrap_or(0));
    }


    #[test]
    fn cfg_liveness_flows_through_if_branches_and_joins() {
        let p = lower(vec![
            S::Let(LetStatement { name: "x".into(), value: A::NumberLiteral(1.0) }),
            S::If(crate::ast::IfStatement {
                condition: A::BoolLiteral(true),
                true_body: vec![S::Assign(crate::ast::AssignStatement { name: "x".into(), value: A::NumberLiteral(2.0) })],
                false_body: Some(vec![S::Assign(crate::ast::AssignStatement { name: "x".into(), value: A::NumberLiteral(3.0) })]),
            }),
            S::Print(PrintStatement { value: A::Identifier("x".into()) }),
        ]);
        let cfg = &plan(&p).cfg;
        assert!(cfg.blocks.iter().any(|block| block.successors.len() > 1));
        assert!(cfg.blocks.iter().any(|block| block.live_out.contains(&BindingId(0))));
    }

    #[test]
    fn cfg_liveness_creates_back_edge_for_while_loop() {
        let p = lower(vec![
            S::Let(LetStatement { name: "x".into(), value: A::NumberLiteral(1.0) }),
            S::While(crate::ast::WhileStatement {
                condition: A::Infix {
                    left: Box::new(A::Identifier("x".into())),
                    operator: crate::lexer::Token::Greater,
                    right: Box::new(A::NumberLiteral(0.0)),
                },
                body: vec![S::Assign(crate::ast::AssignStatement { name: "x".into(), value: A::NumberLiteral(0.0) })],
            }),
            S::Print(PrintStatement { value: A::Identifier("x".into()) }),
        ]);
        let cfg = &plan(&p).cfg;
        assert!(cfg.blocks.iter().any(|block| block.successors.iter().any(|next| (*next as usize) <= block.id as usize)));
        assert!(cfg.blocks.iter().any(|block| block.live_in.contains(&BindingId(0))));
    }

    #[test]
    fn non_overlapping_scalar_lifetimes_reuse_the_same_frame_slot() {
        let p = lower(vec![
            S::Let(LetStatement { name: "first".into(), value: A::NumberLiteral(1.0) }),
            S::Print(PrintStatement { value: A::Identifier("first".into()) }),
            S::Let(LetStatement { name: "second".into(), value: A::NumberLiteral(2.0) }),
            S::Print(PrintStatement { value: A::Identifier("second".into()) }),
        ]);
        let memory = plan(&p);
        assert!(!memory.interference[&BindingId(0)].contains(&BindingId(1)));
        assert_eq!(memory.binding_slots[&BindingId(0)].stack, memory.binding_slots[&BindingId(1)].stack);
        assert_eq!(memory.binding_slots[&BindingId(0)].slot, memory.binding_slots[&BindingId(1)].slot);
        assert_eq!(memory.stack_slot_counts[&DataStackId(u32::MAX)], 1);
    }

    #[test]
    fn simultaneously_live_scalar_bindings_receive_distinct_slots() {
        let p = lower(vec![
            S::Let(LetStatement { name: "first".into(), value: A::NumberLiteral(1.0) }),
            S::Let(LetStatement { name: "second".into(), value: A::NumberLiteral(2.0) }),
            S::Print(PrintStatement { value: A::Identifier("first".into()) }),
            S::Print(PrintStatement { value: A::Identifier("second".into()) }),
        ]);
        let memory = plan(&p);
        assert!(memory.interference[&BindingId(0)].contains(&BindingId(1)));
        assert_ne!(memory.binding_slots[&BindingId(0)].slot, memory.binding_slots[&BindingId(1)].slot);
        assert_eq!(memory.stack_slot_counts[&DataStackId(u32::MAX)], 2);
    }

    #[test]
    fn mutually_exclusive_branch_temporaries_can_reuse_a_frame_slot() {
        let p = lower(vec![
            S::If(crate::ast::IfStatement {
                condition: A::BoolLiteral(true),
                true_body: vec![
                    S::Let(LetStatement { name: "left".into(), value: A::NumberLiteral(1.0) }),
                    S::Print(PrintStatement { value: A::Identifier("left".into()) }),
                ],
                false_body: Some(vec![
                    S::Let(LetStatement { name: "right".into(), value: A::NumberLiteral(2.0) }),
                    S::Print(PrintStatement { value: A::Identifier("right".into()) }),
                ]),
            }),
        ]);
        let memory = plan(&p);
        assert_eq!(memory.binding_slots[&BindingId(0)].slot, memory.binding_slots[&BindingId(1)].slot);
        assert!(!memory.interference[&BindingId(0)].contains(&BindingId(1)));
    }

}
