use crate::hir::{BinaryOp, ExprKind, Expression, Program, ScopeId, StmtKind, Statement};
use crate::memory::{MemoryPlan, StorageClass};
use std::collections::HashMap;

#[derive(Default)]
struct Generator {
    temp: usize,
    scope: usize,
    loop_id: usize,
    scopes: Vec<String>,
    loops: Vec<(String, String, usize)>,
    inline_slots: HashMap<crate::hir::BindingId, u32>,
    owned_bindings: std::collections::HashSet<crate::hir::BindingId>,
    scope_lanes: HashMap<ScopeId, u32>,
}

impl Generator {
    fn line(out: &mut String, value: impl AsRef<str>) { out.push_str(value.as_ref()); out.push('\n'); }
    fn temp(&mut self) -> String { let value = format!("v{}", self.temp); self.temp += 1; value }
    fn c_string(value: &str) -> String {
        let mut out = String::from("\"");
        for byte in value.bytes() {
            match byte {
                b'"' => out.push_str("\\\""), b'\\' => out.push_str("\\\\"),
                b'\n' => out.push_str("\\n"), b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"), 0x20..=0x7e => out.push(byte as char),
                other => out.push_str(&format!("\\{:03o}", other)),
            }
        }
        out.push('"'); out
    }
    fn binding(program: &Program, id: crate::hir::BindingId) -> Result<String, String> {
        program.bindings.get(id.0 as usize).map(|b| Self::c_string(&b.name))
            .ok_or_else(|| format!("BindingId inválido: {}", id.0))
    }
    fn expr(&mut self, p: &Program, e: &Expression, out: &mut String) -> Result<String, String> {
        let v = self.temp();
        let rhs = match &e.kind {
            ExprKind::Number(n) if n.is_finite() => format!("hl_number({n:.17})"),
            ExprKind::Number(_) => return Err("NaN e infinito no backend nativo aún no están soportados".into()),
            ExprKind::Bool(b) => format!("hl_bool({})", if *b { 1 } else { 0 }),
            ExprKind::String(s) => format!("hl_string(env, {})", Self::c_string(s)),
            ExprKind::Read(id) => match self.inline_slots.get(id) {
                Some(slot) => format!("hl_frame_read_or_fail(env, &frame, {slot})"),
                None => format!("hl_get(env, {})", Self::binding(p, *id)?),
            },
            ExprKind::Binary { op, left, right } => {
                let a = self.expr(p, left, out)?; let b = self.expr(p, right, out)?;
                let f = match op { BinaryOp::Add=>"hl_add", BinaryOp::Sub=>"hl_sub", BinaryOp::Mul=>"hl_mul",
                    BinaryOp::Div=>"hl_div", BinaryOp::Less=>"hl_less", BinaryOp::Greater=>"hl_greater",
                    BinaryOp::Equal=>"hl_equal", BinaryOp::NotEqual=>"hl_not_equal" };
                format!("{f}(env, {a}, {b})")
            }
            ExprKind::Ternary { condition, when_true, when_false } => {
                let c = self.expr(p, condition, out)?;
                Self::line(out, format!("HlValue {v};"));
                Self::line(out, format!("if (hl_truthy({c})) {{"));
                let a = self.expr(p, when_true, out)?; Self::line(out, format!("{v} = {a};"));
                Self::line(out, "} else {");
                let b = self.expr(p, when_false, out)?; Self::line(out, format!("{v} = {b};"));
                Self::line(out, "}"); return Ok(v);
            }
        };
        Self::line(out, format!("HlValue {v} = {rhs};")); Ok(v)
    }
    fn enter_scope_env(&self, scope: ScopeId, out: &mut String) {
        if let Some(lane) = self.scope_lanes.get(&scope) {
            Self::line(out, format!("env = hl_env_new_in_lane(env, &region_lanes[{lane}]);"));
        } else {
            Self::line(out, "env = hl_env_new(env);");
        }
    }
    fn block(&mut self, p: &Program, scope: ScopeId, statements: &[Statement], out: &mut String) -> Result<(), String> {
        let id = self.scope; self.scope += 1; let prev = format!("prev_{id}");
        Self::line(out, "{"); Self::line(out, format!("HlEnv *{prev} = env;"));
        self.enter_scope_env(scope, out); self.scopes.push(prev.clone());
        for s in statements { self.stmt(p, s, out)?; }
        self.scopes.pop(); Self::line(out, "hl_env_free(env);"); Self::line(out, format!("env = {prev};")); Self::line(out, "}"); Ok(())
    }
    fn restore(&self, out: &mut String, depth: usize) {
        for prev in self.scopes[depth..].iter().rev() { Self::line(out, "hl_env_free(env);"); Self::line(out, format!("env = {prev};")); }
    }
    fn stmt(&mut self, p: &Program, s: &Statement, out: &mut String) -> Result<(), String> {
        match &s.kind {
            StmtKind::Let { binding, value } => {
                let v = self.expr(p, value, out)?;
                if let Some(slot) = self.inline_slots.get(binding) {
                    Self::line(out, format!("hl_frame_write_or_fail(env, &frame, {slot}, {v});"));
                } else {
                    let storage = if self.owned_bindings.contains(binding) { "HL_STORAGE_OWNED" } else { "HL_STORAGE_SCOPE" };
                    Self::line(out, format!("hl_define_planned(env, {}, {v}, {storage});", Self::binding(p, *binding)?));
                }
            }
            StmtKind::Assign { binding, value } => {
                let v = self.expr(p, value, out)?;
                if let Some(slot) = self.inline_slots.get(binding) {
                    Self::line(out, format!("hl_frame_write_or_fail(env, &frame, {slot}, {v});"));
                } else {
                    let storage = if self.owned_bindings.contains(binding) { "HL_STORAGE_OWNED" } else { "HL_STORAGE_SCOPE" };
                    Self::line(out, format!("hl_assign_planned(env, {}, {v}, {storage});", Self::binding(p, *binding)?));
                }
            }
            StmtKind::Print(e) => { let v = self.expr(p, e, out)?; Self::line(out, format!("hl_print({v});")); }
            StmtKind::Block { scope, statements } => self.block(p, *scope, statements, out)?,
            StmtKind::If { condition, then_scope, then_body, else_scope, else_body } => {
                let c = self.expr(p, condition, out)?;
                Self::line(out, format!("if (hl_truthy({c}))")); self.block(p, *then_scope, then_body, out)?;
                if !else_body.is_empty() {
                    Self::line(out, "else");
                    let scope = (*else_scope).ok_or("else_body sin ScopeId en HIR")?;
                    self.block(p, scope, else_body, out)?;
                }
            }
            StmtKind::While { scope, condition, body } => {
                let id = self.loop_id; self.loop_id += 1;
                let prev = format!("loop_prev_{id}"); let loop_env = format!("loop_env_{id}");
                let iter_prev = format!("iter_prev_{id}"); let cond = format!("loop_cond_{id}"); let end = format!("loop_end_{id}");
                Self::line(out, format!("HlEnv *{prev} = env;")); Self::line(out, "env = hl_env_new(env);");
                Self::line(out, format!("HlEnv *{loop_env} = env;"));
                self.scopes.push(prev.clone()); let depth = self.scopes.len();
                self.loops.push((cond.clone(), end.clone(), depth));
                Self::line(out, format!("{cond}: ;"));
                Self::line(out, format!("HlEnv *{iter_prev} = env;"));
                self.enter_scope_env(*scope, out); self.scopes.push(iter_prev.clone());
                let c = self.expr(p, condition, out)?;
                Self::line(out, format!("if (!hl_truthy({c})) {{ hl_env_free(env); env = {iter_prev}; goto {end}; }}"));
                for child in body { self.stmt(p, child, out)?; }
                self.scopes.pop();
                Self::line(out, "hl_env_free(env);"); Self::line(out, format!("env = {iter_prev};"));
                Self::line(out, format!("goto {cond};")); Self::line(out, format!("{end}: ;"));
                self.loops.pop(); self.scopes.pop(); Self::line(out, format!("hl_env_free({loop_env});"));
                Self::line(out, format!("env = {prev};"));
            }
            StmtKind::Break => {
                let (_, end, depth) = self.loops.last().cloned().ok_or("break fuera de bucle en HIR")?;
                self.restore(out, depth); Self::line(out, format!("goto {end};"));
            }
            StmtKind::Continue => {
                let (cond, _, depth) = self.loops.last().cloned().ok_or("continue fuera de bucle en HIR")?;
                self.restore(out, depth); Self::line(out, format!("goto {cond};"));
            }
        }
        Ok(())
    }
}

pub fn generate_c(program: &Program, memory: &MemoryPlan) -> Result<String, String> {
    // Reject stale/incomplete plans instead of silently compiling without the
    // frontend's storage decisions. This is metadata consumption, not yet an
    // instruction to the runtime to allocate from physical regions.
    for binding in &program.bindings {
        if !memory.bindings.contains_key(&binding.id) {
            return Err(format!("MemoryPlan incompleto: falta BindingId {}", binding.id.0));
        }
    }
    // Validate stack counts, stack ownership, and interference constraints before codegen.
    crate::memory::validate_slot_plan(program, memory)?;
    let layout = crate::memory::frame_layout(program, memory)?;
    let inline_stack = crate::memory::DataStackId(u32::MAX);
    let inline_count = layout.stacks.get(&inline_stack).map_or(0, |slots| slots.len());
    let inline_slots: HashMap<_, _> = memory.binding_slots.iter()
        .filter(|(_, assignment)| assignment.stack == inline_stack)
        .map(|(binding, assignment)| (*binding, assignment.slot))
        .collect();
    let owned_bindings = memory.bindings.iter()
        .filter_map(|(binding, class)| matches!(class, StorageClass::OwnedRegion).then_some(*binding))
        .collect();
    let scope_lanes: HashMap<_, _> = memory.scope_stacks.iter()
        .map(|(scope, plan)| (*scope, plan.stack.0))
        .collect();
    let lane_count = scope_lanes.values().copied().max().map_or(0, |max| max + 1);
    let mut body = String::new();
    let mut generator = Generator { inline_slots, owned_bindings, scope_lanes: scope_lanes.clone(), ..Generator::default() };
    for stmt in &program.statements { generator.stmt(program, stmt, &mut body)?; }
    let mut source = include_str!("native_runtime.c").to_string();
    source.push_str("\n/* Compiler MemoryPlan: physical scope lanes. */\n");
    for binding in &program.bindings {
        let class = match memory.bindings.get(&binding.id).expect("validated above") {
            StorageClass::Inline => "inline",
            StorageClass::ScopeRegion(_) => "scope-region",
            StorageClass::OwnedRegion => "owned-region",
        };
        source.push_str(&format!("/* binding {} ({}) => {} */\n", binding.id.0, binding.name, class));
        if let Some(slot) = memory.binding_slots.get(&binding.id) {
            source.push_str(&format!("/* binding {} => data-stack {} slot {} */\n",
                binding.id.0, slot.stack.0, slot.slot));
        }
    }
    source.push_str("\nint main(void) {\n");
    if lane_count > 0 {
        source.push_str(&format!("    HlArenaLane region_lanes[{lane_count}] = {{0}};\n"));
    }
    if let Some(root_lane) = scope_lanes.get(&ScopeId(0)) {
        source.push_str(&format!("    HlEnv *env = hl_env_new_in_lane(NULL, &region_lanes[{root_lane}]);\n"));
    } else {
        source.push_str("    HlEnv *env = hl_env_new(NULL);\n");
    }
    if inline_count > 0 {
        source.push_str(&format!("    HlFrameSlot frame_slots[{inline_count}];\n    HlFrame frame;\n    hl_frame_init(&frame, frame_slots, {inline_count});\n"));
    } else {
        source.push_str("    HlFrame frame = { NULL, 0 };\n");
    }
    source.push_str(&body); source.push_str("    hl_env_free(env);\n");
    if lane_count > 0 {
        for lane in 0..lane_count {
            source.push_str(&format!("    hl_lane_destroy(&region_lanes[{lane}]);\n"));
        }
    }
    source.push_str("    return 0;\n}\n"); Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir;
    use crate::ast::{Program as AstProgram, Statement as AstStmt, LetStatement, PrintStatement};
    #[test]
    fn generated_runtime_exposes_single_lifo_cleanup_registry() {
        let ast = AstProgram { statements: vec![] };
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("HlAllocation *hl_env_defer("));
        assert!(c.contains("int hl_env_transfer("));
        assert!(c.contains("int hl_env_cancel("));
        assert!(c.contains("allocation->cleanup(allocation->context);"));
        assert!(c.contains("entry->next = environment->allocations;"));
    }

    #[test]
    fn generated_runtime_uses_aligned_bump_arenas_per_environment() {
        let ast = AstProgram { statements: vec![] };
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("typedef struct HlArenaBlock"));
        assert!(c.contains("_Alignof(max_align_t)"));
        assert!(c.contains("environment->arena = block"));
        assert!(c.contains("free(block);"));
        assert!(!c.contains("hl_env_defer(environment, hl_cleanup_free"));
    }

    #[test]
    fn generated_c_consumes_memory_plan_binding_classes() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement { name: "n".into(), value: crate::ast::Expression::NumberLiteral(2.0) }),
            AstStmt::Let(LetStatement { name: "s".into(), value: crate::ast::Expression::StringLiteral("x".into()) }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("binding 0 (n) => inline"));
        assert!(c.contains("binding 1 (s) => scope-region"));
        let generated_main = c.split("int main(void) {").last().unwrap();
        assert!(generated_main.contains("hl_define_planned(env,"));
        assert!(generated_main.contains("HL_STORAGE_SCOPE"));
        assert!(!generated_main.contains("HL_STORAGE_OWNED"));
    }

    #[test]
    fn generated_c_promotes_reassigned_dynamic_binding_to_owned_storage() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "s".into(),
                value: crate::ast::Expression::StringLiteral("first".into()),
            }),
            AstStmt::Assign(crate::ast::AssignStatement {
                name: "s".into(),
                value: crate::ast::Expression::StringLiteral("second".into()),
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        assert_eq!(memory.bindings[&crate::hir::BindingId(0)], StorageClass::OwnedRegion);
        let c = generate_c(&lowered, &memory).unwrap();
        let generated_main = c.split("int main(void) {").last().unwrap();
        assert!(generated_main.contains("hl_define_planned(env,"));
        assert!(generated_main.contains("hl_assign_planned(env,"));
        assert!(generated_main.contains("HL_STORAGE_OWNED"));
    }

    #[test]
    fn generated_c_promotes_outer_binding_written_from_nested_scope() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "outer".into(),
                value: crate::ast::Expression::StringLiteral("before".into()),
            }),
            AstStmt::Block(vec![
                AstStmt::Assign(crate::ast::AssignStatement {
                    name: "outer".into(),
                    value: crate::ast::Expression::StringLiteral("from-inner".into()),
                }),
            ]),
            AstStmt::Print(PrintStatement {
                value: crate::ast::Expression::Identifier("outer".into()),
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        assert_eq!(memory.bindings[&crate::hir::BindingId(0)], StorageClass::OwnedRegion);
        let c = generate_c(&lowered, &memory).unwrap();
        let generated_main = c.split("int main(void) {").last().unwrap();
        assert!(generated_main.contains("hl_assign_planned(env,"));
        assert!(generated_main.contains("HL_STORAGE_OWNED"));
    }

    #[test]
    fn generated_c_uses_memory_plan_slots_for_inline_bindings() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement { name: "x".into(), value: crate::ast::Expression::NumberLiteral(7.0) }),
            AstStmt::Print(PrintStatement { value: crate::ast::Expression::Identifier("x".into()) }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("HlFrameSlot frame_slots["));
        assert!(c.contains("hl_frame_write_or_fail(env, &frame,"));
        assert!(c.contains("hl_frame_read_or_fail(env, &frame,"));
        assert!(!c.contains("hl_define(env, \"x\""));
        assert!(!c.contains("hl_get(env, \"x\""));
    }

    #[test]
    fn generated_c_emits_physical_slot_assignments() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement { name: "n".into(), value: crate::ast::Expression::NumberLiteral(2.0) }),
            AstStmt::Let(LetStatement { name: "s".into(), value: crate::ast::Expression::StringLiteral("x".into()) }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        for (binding, slot) in &memory.binding_slots {
            assert!(c.contains(&format!("binding {} => data-stack {} slot {}", binding.0, slot.stack.0, slot.slot)));
        }
    }

    #[test]
    fn generated_c_routes_scope_bindings_through_planned_physical_lanes() {
        let ast = AstProgram { statements: vec![
            AstStmt::Block(vec![AstStmt::Let(LetStatement {
                name: "first".into(), value: crate::ast::Expression::StringLiteral("one".into()),
            })]),
            AstStmt::Block(vec![AstStmt::Let(LetStatement {
                name: "second".into(), value: crate::ast::Expression::StringLiteral("two".into()),
            })]),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("HlArenaLane region_lanes[2] = {0};"));
        assert_eq!(c.matches("hl_env_new_in_lane(env, &region_lanes[1])").count(), 2);
        assert!(c.contains("hl_lane_destroy(&region_lanes[1]);"));
    }

    #[test]
    fn generated_c_rejects_orphan_slot_assignments() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement { name: "n".into(), value: crate::ast::Expression::NumberLiteral(2.0) }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let mut memory = crate::memory::plan(&lowered);
        memory.binding_slots.insert(crate::hir::BindingId(9999), crate::memory::BindingSlotPlan {
            stack: crate::memory::DataStackId(0), slot: 0,
        });
        assert!(generate_c(&lowered, &memory).unwrap_err().contains("slot para BindingId inexistente"));
    }

    #[test]
    fn generated_c_rejects_incomplete_memory_plan() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement { name: "s".into(), value: crate::ast::Expression::StringLiteral("x".into()) }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let mut memory = crate::memory::plan(&lowered);
        memory.bindings.clear();
        assert!(generate_c(&lowered, &memory).unwrap_err().contains("MemoryPlan incompleto"));
    }

    #[test]
    fn emits_c_from_resolved_hir_bindings() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement { name:"x".into(), value:crate::ast::Expression::NumberLiteral(7.0) }),
            AstStmt::Print(PrintStatement { value:crate::ast::Expression::Identifier("x".into()) })
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("hl_frame_write_or_fail(env, &frame,"));
        assert!(c.contains("hl_frame_read_or_fail(env, &frame,"));
    }
}
