use crate::hir::{BinaryOp, ExprKind, Expression, Program, ScopeId, StmtId, StmtKind, Statement};
use crate::memory::{MemoryPlan, StorageClass};
use std::collections::HashMap;

#[derive(Default)]
struct Generator {
    temp: usize,
    temp_declarations: Vec<String>,
    temp_roots: Vec<String>,
    frame_owner: Option<String>,
    scope: usize,
    loop_id: usize,
    scopes: Vec<String>,
    loops: Vec<(String, String, usize)>,
    inline_slots: HashMap<crate::hir::BindingId, u32>,
    owned_bindings: std::collections::HashSet<crate::hir::BindingId>,
    ownership_sites: HashMap<crate::hir::BindingId, u64>,
    scope_lanes: HashMap<ScopeId, u32>,
    promotion_sites: HashMap<StmtId, (crate::hir::BindingId, u64)>,
    function_return: Option<(String, String)>,
    /// Bindings whose storage is a shared capture cell. They are predeclared
    /// before their initializer so a self-recursive closure can capture them.
    cell_bindings: std::collections::HashSet<crate::hir::BindingId>,
    /// Trace closure cycles at statement boundaries when the program can create
    /// closure values.
    collect: bool,
}

impl Generator {
    fn line(out: &mut String, value: impl AsRef<str>) { out.push_str(value.as_ref()); out.push('\n'); }
    fn temp(&mut self) -> String {
        let value = format!("v{}", self.temp);
        self.temp += 1;
        self.temp_declarations.push(format!("    HlValue {value};\n"));
        value
    }
    fn release_temps(&mut self, out: &mut String) {
        for value in self.temp_roots.drain(..).rev() {
            Self::line(out, format!("hl_root_value_pop(env, &{value});"));
        }
    }
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
            ExprKind::String(s) => {
                let literal = Self::c_string(s);
                format!("hl_string_n(env, {literal}, sizeof({literal}) - 1)")
            },
            ExprKind::Read(id) => match self.inline_slots.get(id) {
                Some(slot) => format!("hl_frame_read_or_fail(env, &frame, {slot})"),
                // Cell-backed bindings live in an environment binding; hl_get
                // reads through the shared cell when one is installed.
                None => format!("hl_get(env, {})", Self::binding(p, *id)?),
            },
            ExprKind::Binary { op, left, right } => {
                let a = self.expr(p, left, out)?; let b = self.expr(p, right, out)?;
                let f = match op { BinaryOp::Add=>"hl_add", BinaryOp::Sub=>"hl_sub", BinaryOp::Mul=>"hl_mul",
                    BinaryOp::Div=>"hl_div", BinaryOp::Less=>"hl_less", BinaryOp::Greater=>"hl_greater",
                    BinaryOp::Equal=>"hl_equal", BinaryOp::NotEqual=>"hl_not_equal" };
                format!("{f}(env, {a}, {b})")
            }
            ExprKind::Index { target, index } => {
                let target = self.expr(p, target, out)?;
                let index = self.expr(p, index, out)?;
                format!("hl_index(env, {target}, {index})")
            }
            ExprKind::Array(items) => {
                let values = items.iter().map(|item| self.expr(p, item, out)).collect::<Result<Vec<_>, _>>()?;
                if values.is_empty() {
                    "hl_array(env, NULL, 0)".to_string()
                } else {
                    format!("hl_array(env, (HlValue[]){{ {} }}, {})", values.join(", "), values.len())
                }
            }
            ExprKind::Length(value) => {
                let value = self.expr(p, value, out)?;
                format!("hl_length(env, {value})")
            },
            ExprKind::Closure(function_id) => {
                let function = p.functions.get(function_id.0 as usize)
                    .filter(|function| function.id == *function_id)
                    .ok_or_else(|| format!("FunctionId inválido para closure: {}", function_id.0))?;
                let names = function.captures.iter()
                    .map(|capture| Self::binding(p, capture.binding))
                    .collect::<Result<Vec<_>, _>>()?;
                let captures = if names.is_empty() {
                    "NULL".to_string()
                } else {
                    format!("(const char *[]){{ {} }}", names.join(", "))
                };
                format!("hl_closure_new(env, UINT32_C({}), {}, {})",
                    function_id.0, captures, names.len())
            },
            ExprKind::Call { callee, args } => {
                let callee = self.expr(p, callee, out)?;
                let args = args.iter().map(|arg| self.expr(p, arg, out))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = if args.is_empty() { "NULL".to_string() }
                    else { format!("(HlValue[]){{ {} }}", args.join(", ")) };
                format!("hl_call(env, {callee}, {values}, {})", args.len())
            },
            ExprKind::Map(items) => {
                let mut entries = Vec::with_capacity(items.len());
                for (key, value) in items {
                    let key = self.expr(p, key, out)?;
                    let value = self.expr(p, value, out)?;
                    entries.push(format!("{{ {key}, {value} }}"));
                }
                if entries.is_empty() {
                    "hl_map(env, NULL, 0)".to_string()
                } else {
                    format!("hl_map(env, (HlMapEntry[]){{ {} }}, {})", entries.join(", "), entries.len())
                }
            },
            ExprKind::Ternary { condition, when_true, when_false } => {
                let c = self.expr(p, condition, out)?;
                // All generated temporaries are declared at function scope so their
                // addresses remain valid while registered as GC roots.
                Self::line(out, format!("if (hl_truthy({c})) {{"));
                let a = self.expr(p, when_true, out)?; Self::line(out, format!("{v} = {a};"));
                Self::line(out, "} else {");
                let b = self.expr(p, when_false, out)?; Self::line(out, format!("{v} = {b};"));
                Self::line(out, "}");
                Self::line(out, format!("hl_root_value_push(env, &{v});"));
                self.temp_roots.push(v.clone());
                return Ok(v);
            }
        };
        Self::line(out, format!("{v} = {rhs};"));
        Self::line(out, format!("hl_root_value_push(env, &{v});"));
        self.temp_roots.push(v.clone());
        Ok(v)
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
        self.statements(p, statements, out)?;
        self.scopes.pop(); Self::line(out, "hl_env_free(env);"); Self::line(out, format!("env = {prev};")); Self::line(out, "}"); Ok(())
    }
    fn restore(&self, out: &mut String, depth: usize) {
        for prev in self.scopes[depth..].iter().rev() { Self::line(out, "hl_env_free(env);"); Self::line(out, format!("env = {prev};")); }
    }
    /// A statement boundary has no live evaluator temporary, so it is a safe
    /// point to sweep unreachable closure/cell cycles.
    fn collect_point(&mut self, out: &mut String) {
        self.release_temps(out);
        if self.collect { Self::line(out, "hl_collect(env);"); }
    }
    fn statements(&mut self, p: &Program, statements: &[Statement], out: &mut String) -> Result<(), String> {
        for statement in statements {
            Self::line(out, "hl_eval_step(env);");
            self.stmt(p, statement, out)?;
            self.collect_point(out);
        }
        Ok(())
    }
    fn stmt(&mut self, p: &Program, s: &Statement, out: &mut String) -> Result<(), String> {
        if let Some(span) = p.statement_spans.get(&s.id) {
            Self::line(out, format!(
                "hl_set_source_range(env, {}, {}, {}, {});",
                span.start, span.end, span.line, span.column
            ));
        } else {
            // A synthetic statement must not inherit a misleading parent range.
            Self::line(out, "hl_clear_source_range(env);");
        }
        match &s.kind {
            StmtKind::Let { binding, value } => {
                // A captured binding is predeclared with an uninitialized cell
                // so the initializer (which may capture it) shares its identity.
                if self.cell_bindings.contains(binding) {
                    let site = self.ownership_sites.get(binding).copied().unwrap_or(0);
                    Self::line(out, format!("hl_binding_predeclare(env, {}, UINT64_C({site}));", Self::binding(p, *binding)?));
                }
                let v = self.expr(p, value, out)?;
                if let Some(slot) = self.inline_slots.get(binding) {
                    Self::line(out, format!("hl_frame_write_or_fail(env, {}, &frame, {slot}, {v});", self.frame_owner.as_deref().unwrap_or("env")));
                } else {
                    let storage = if self.owned_bindings.contains(binding) { "HL_STORAGE_OWNED" } else { "HL_STORAGE_SCOPE" };
                    let site = self.ownership_sites.get(binding).copied().unwrap_or(0);
                    Self::line(out, format!("hl_define_planned_site(env, {}, {v}, {storage}, UINT64_C({site}));", Self::binding(p, *binding)?));
                }
            }
            StmtKind::Assign { binding, value } => {
                let v = self.expr(p, value, out)?;
                if let Some((destination, site)) = self.promotion_sites.get(&s.id) {
                    if destination != binding {
                        return Err(format!("destino de promoción incoherente para StmtId {}", s.id.0));
                    }
                    Self::line(out, format!("hl_assign_planned_site(env, {}, {v}, HL_STORAGE_OWNED, UINT64_C({site}));", Self::binding(p, *binding)?));
                } else if let Some(slot) = self.inline_slots.get(binding) {
                    Self::line(out, format!("hl_frame_write_or_fail(env, {}, &frame, {slot}, {v});", self.frame_owner.as_deref().unwrap_or("env")));
                } else {
                    let storage = if self.owned_bindings.contains(binding) { "HL_STORAGE_OWNED" } else { "HL_STORAGE_SCOPE" };
                    Self::line(out, format!("hl_assign_planned(env, {}, {v}, {storage});", Self::binding(p, *binding)?));
                }
            }
            StmtKind::IndexAssign { binding, indices, value } => {
                let indices = indices.iter()
                    .map(|index| self.expr(p, index, out))
                    .collect::<Result<Vec<_>, _>>()?;
                if indices.is_empty() {
                    return Err("la asignación por índice requiere al menos un índice".into());
                }
                let value = self.expr(p, value, out)?;
                if let Some(slot) = self.inline_slots.get(binding).copied() {
                    // Frame-held values (parameters and locals) are not visible
                    // by name; assign through the value and write it back.
                    let index_values = if indices.len() == 1 {
                        format!("&{}", indices[0])
                    } else {
                        format!("(HlValue[]){{ {} }}", indices.join(", "))
                    };
                    let updated = self.temp();
                    Self::line(out, format!(
                        "{updated} = hl_index_assign_value(env, hl_frame_read_or_fail(env, &frame, {slot}), {index_values}, {}, {value});",
                        indices.len()
                    ));
                    Self::line(out, format!("hl_frame_write_or_fail(env, {}, &frame, {slot}, {updated});", self.frame_owner.as_deref().unwrap_or("env")));
                } else if indices.len() == 1 {
                    Self::line(out, format!(
                        "hl_index_assign(env, {}, {}, {value});",
                        Self::binding(p, *binding)?,
                        indices[0]
                    ));
                } else {
                    Self::line(out, format!(
                        "hl_index_assign_path(env, {}, (HlValue[]){{ {} }}, {}, {value});",
                        Self::binding(p, *binding)?,
                        indices.join(", "),
                        indices.len()
                    ));
                }
            },
            StmtKind::Print(e) => { let v = self.expr(p, e, out)?; Self::line(out, format!("hl_print({v});")); }
            StmtKind::Block { scope, statements } => self.block(p, *scope, statements, out)?,
            StmtKind::If { condition, then_scope, then_body, else_scope, else_body } => {
                let c = self.expr(p, condition, out)?;
                self.release_temps(out);
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
                Self::line(out, "hl_eval_step(env);");
                Self::line(out, format!("HlEnv *{iter_prev} = env;"));
                self.enter_scope_env(*scope, out); self.scopes.push(iter_prev.clone());
                let c = self.expr(p, condition, out)?;
                self.release_temps(out);
                Self::line(out, format!("if (!hl_truthy({c})) {{ hl_env_free(env); env = {iter_prev}; goto {end}; }}"));
                self.statements(p, body, out)?;
                self.scopes.pop();
                Self::line(out, "hl_env_free(env);"); Self::line(out, format!("env = {iter_prev};"));
                self.collect_point(out);
                Self::line(out, format!("goto {cond};")); Self::line(out, format!("{end}: ;"));
                self.loops.pop(); self.scopes.pop(); Self::line(out, format!("hl_env_free({loop_env});"));
                Self::line(out, format!("env = {prev};"));
            }
            StmtKind::Break => {
                let (_, end, depth) = self.loops.last().cloned().ok_or("break fuera de bucle en HIR")?;
                self.release_temps(out); self.restore(out, depth); Self::line(out, format!("goto {end};"));
            }
            StmtKind::Continue => {
                let (cond, _, depth) = self.loops.last().cloned().ok_or("continue fuera de bucle en HIR")?;
                self.release_temps(out); self.restore(out, depth); Self::line(out, format!("goto {cond};"));
            },
            StmtKind::Return(value) => {
                let (label, result) = self.function_return.clone()
                    .ok_or("return fuera de una función durante codegen nativo")?;
                let value = self.expr(p, value, out)?;
                // Preserve values produced in a nested lexical scope before restore() frees it.
                Self::line(out, format!("{result} = hl_clone_value(fn_activation, {value});"));
                self.release_temps(out);
                self.restore(out, 0);
                Self::line(out, format!("goto {label};"));
            }
        }
        Ok(())
    }
}


fn validate_native_expr(expression: &Expression) -> Result<(), String> {
    match &expression.kind {
        ExprKind::Call { callee, args } => {
            validate_native_expr(callee)?;
            for arg in args { validate_native_expr(arg)?; }
        }
        ExprKind::Binary { left, right, .. } => {
            validate_native_expr(left)?;
            validate_native_expr(right)?;
        }
        ExprKind::Ternary { condition, when_true, when_false } => {
            validate_native_expr(condition)?;
            validate_native_expr(when_true)?;
            validate_native_expr(when_false)?;
        }
        ExprKind::Array(items) => for item in items { validate_native_expr(item)?; },
        ExprKind::Map(items) => for (key, value) in items {
            validate_native_expr(key)?;
            validate_native_expr(value)?;
        },
        ExprKind::Index { target, index } => {
            validate_native_expr(target)?;
            validate_native_expr(index)?;
        }
        ExprKind::Length(value) => validate_native_expr(value)?,
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_)
        | ExprKind::Read(_) | ExprKind::Closure(_) => {}
    }
    Ok(())
}

/// Module-level bindings whose value is statically a function value.
/// Aliases are followed and any reassignment disqualifies the binding, so a
/// call site is only accepted when the callee identity cannot change.
fn module_function_bindings(program: &Program) -> HashMap<crate::hir::BindingId, crate::hir::FunctionId> {
    fn collect_assigned(statements: &[Statement], assigned: &mut std::collections::HashSet<crate::hir::BindingId>) {
        for statement in statements {
            match &statement.kind {
                StmtKind::Assign { binding, .. } => { assigned.insert(*binding); }
                StmtKind::Block { statements, .. } => collect_assigned(statements, assigned),
                StmtKind::If { then_body, else_body, .. } => {
                    collect_assigned(then_body, assigned);
                    collect_assigned(else_body, assigned);
                }
                StmtKind::While { body, .. } => collect_assigned(body, assigned),
                _ => {}
            }
        }
    }
    let mut assigned = std::collections::HashSet::new();
    collect_assigned(&program.statements, &mut assigned);
    for function in &program.functions {
        collect_assigned(&function.body.statements, &mut assigned);
    }
    let mut functions = HashMap::new();
    for statement in &program.statements {
        let StmtKind::Let { binding, value } = &statement.kind else { continue };
        if assigned.contains(binding) { continue; }
        let target = match &value.kind {
            ExprKind::Closure(id) => Some(*id),
            ExprKind::Read(source) => functions.get(source).copied(),
            _ => None,
        };
        if let Some(id) = target { functions.insert(*binding, id); }
    }
    functions
}

struct CallCollector {
    bindings: HashMap<crate::hir::BindingId, crate::hir::FunctionId>,
    reachable: Vec<crate::hir::FunctionId>,
    seen: std::collections::HashSet<crate::hir::FunctionId>,
    /// Whether some call site resolves its callee only at runtime (higher-order
    /// parameter, closure stored in an aggregate...).
    dynamic_calls: bool,
    /// Sticky flag: the runtime dispatcher must exist even when no closure
    /// function was widened into the reachable set.
    saw_dynamic_calls: bool,
}

fn record_call(
    program: &Program,
    memory: &MemoryPlan,
    collector: &mut CallCollector,
    callee: &Expression,
    args: &[Expression],
) -> Result<(), String> {
    // A callee whose value is not statically a known function is resolved by
    // the runtime dispatcher. Its possible targets are every function that can
    // produce a closure value, so the call is recorded instead of rejected.
    let id = match &callee.kind {
        ExprKind::Closure(id) => *id,
        ExprKind::Read(binding) => match collector.bindings.get(binding).copied() {
            Some(id) => id,
            None => {
                collector.dynamic_calls = true;
                collector.saw_dynamic_calls = true;
                return Ok(());
            }
        },
        _ => {
            collector.dynamic_calls = true;
            collector.saw_dynamic_calls = true;
            return Ok(());
        }
    };
    let function = program.functions.get(id.0 as usize).filter(|function| function.id == id)
        .ok_or_else(|| format!("FunctionId inválido en llamada: {}", id.0))?;
    let activation = memory.function_activations.get(&id)
        .ok_or_else(|| format!("MemoryPlan incompleto: falta activación FunctionId {}", id.0))?;
    if activation.frame_slots.len() + activation.cell_slots.len()
        != activation.bindings.len() {
        return Err(format!("MemoryPlan incompleto: activación incompleta en FunctionId {}", id.0));
    }
    if args.len() != function.params.len() {
        return Err(format!(
            "la función {} espera {} argumentos y la llamada pasa {}",
            id.0, function.params.len(), args.len()
        ));
    }
    if collector.seen.insert(id) { collector.reachable.push(id); }
    Ok(())
}

fn walk_expression_calls(
    program: &Program, memory: &MemoryPlan, expression: &Expression, collector: &mut CallCollector,
) -> Result<(), String> {
    match &expression.kind {
        ExprKind::Call { callee, args } => {
            walk_expression_calls(program, memory, callee, collector)?;
            for arg in args { walk_expression_calls(program, memory, arg, collector)?; }
            record_call(program, memory, collector, callee, args)?;
        }
        ExprKind::Binary { left, right, .. } => {
            walk_expression_calls(program, memory, left, collector)?;
            walk_expression_calls(program, memory, right, collector)?;
        }
        ExprKind::Ternary { condition, when_true, when_false } => {
            walk_expression_calls(program, memory, condition, collector)?;
            walk_expression_calls(program, memory, when_true, collector)?;
            walk_expression_calls(program, memory, when_false, collector)?;
        }
        ExprKind::Array(items) => for item in items { walk_expression_calls(program, memory, item, collector)?; },
        ExprKind::Map(items) => for (key, value) in items {
            walk_expression_calls(program, memory, key, collector)?;
            walk_expression_calls(program, memory, value, collector)?;
        },
        ExprKind::Index { target, index } => {
            walk_expression_calls(program, memory, target, collector)?;
            walk_expression_calls(program, memory, index, collector)?;
        }
        ExprKind::Length(value) => walk_expression_calls(program, memory, value, collector)?,
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_)
        | ExprKind::Read(_) | ExprKind::Closure(_) => {}
    }
    Ok(())
}

fn walk_statement_calls(
    program: &Program, memory: &MemoryPlan, statements: &[Statement], collector: &mut CallCollector,
) -> Result<(), String> {
    for statement in statements {
        match &statement.kind {
            StmtKind::Let { value, .. } | StmtKind::Assign { value, .. }
            | StmtKind::Print(value) | StmtKind::Return(value) =>
                walk_expression_calls(program, memory, value, collector)?,
            StmtKind::IndexAssign { indices, value, .. } => {
                for index in indices { walk_expression_calls(program, memory, index, collector)?; }
                walk_expression_calls(program, memory, value, collector)?;
            }
            StmtKind::Block { statements, .. } =>
                walk_statement_calls(program, memory, statements, collector)?,
            StmtKind::If { condition, then_body, else_body, .. } => {
                walk_expression_calls(program, memory, condition, collector)?;
                walk_statement_calls(program, memory, then_body, collector)?;
                walk_statement_calls(program, memory, else_body, collector)?;
            }
            StmtKind::While { condition, body, .. } => {
                walk_expression_calls(program, memory, condition, collector)?;
                walk_statement_calls(program, memory, body, collector)?;
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
    Ok(())
}

/// Every function id that can become a runtime closure value. A dynamic callee
/// may carry any of them, so their bodies and dispatcher cases must exist.
fn closure_functions(program: &Program) -> Vec<crate::hir::FunctionId> {
    fn expression(value: &Expression, out: &mut Vec<crate::hir::FunctionId>) {
        match &value.kind {
            ExprKind::Closure(id) => out.push(*id),
            ExprKind::Binary { left, right, .. } => { expression(left, out); expression(right, out); }
            ExprKind::Ternary { condition, when_true, when_false } => {
                expression(condition, out);
                expression(when_true, out);
                expression(when_false, out);
            }
            ExprKind::Array(items) => for item in items { expression(item, out); },
            ExprKind::Map(items) => for (key, value) in items {
                expression(key, out);
                expression(value, out);
            }
            ExprKind::Index { target, index } => { expression(target, out); expression(index, out); }
            ExprKind::Length(value) => expression(value, out),
            ExprKind::Call { callee, args } => {
                expression(callee, out);
                for arg in args { expression(arg, out); }
            }
            ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::String(_) | ExprKind::Read(_) => {}
        }
    }
    fn statements(values: &[Statement], out: &mut Vec<crate::hir::FunctionId>) {
        for statement in values {
            match &statement.kind {
                StmtKind::Let { value, .. } | StmtKind::Assign { value, .. }
                | StmtKind::Print(value) | StmtKind::Return(value) => expression(value, out),
                StmtKind::IndexAssign { indices, value, .. } => {
                    for index in indices { expression(index, out); }
                    expression(value, out);
                }
                StmtKind::Block { statements: body, .. } => statements(body, out),
                StmtKind::If { condition, then_body, else_body, .. } => {
                    expression(condition, out);
                    statements(then_body, out);
                    statements(else_body, out);
                }
                StmtKind::While { condition, body, .. } => {
                    expression(condition, out);
                    statements(body, out);
                }
                StmtKind::Break | StmtKind::Continue => {}
            }
        }
    }
    let mut out = Vec::new();
    statements(&program.statements, &mut out);
    for function in &program.functions {
        if let Some(value) = &function.body.expression { expression(value, &mut out); }
        statements(&function.body.statements, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Call targets whose bodies must be emitted. Direct calls are resolved
/// statically; a dynamic call site can reach any function that produces a
/// closure value, so those functions become reachable conservatively. Recursive
/// call graphs are accepted: the runtime collects closure/cell cycles.
fn reachable_functions(
    program: &Program, memory: &MemoryPlan,
) -> Result<(Vec<crate::hir::FunctionId>, bool), String> {
    let mut collector = CallCollector {
        bindings: module_function_bindings(program),
        reachable: Vec::new(),
        seen: std::collections::HashSet::new(),
        dynamic_calls: false,
        saw_dynamic_calls: false,
    };
    walk_statement_calls(program, memory, &program.statements, &mut collector)?;
    let mut index = 0;
    loop {
        while index < collector.reachable.len() {
            let id = collector.reachable[index];
            index += 1;
            let function = &program.functions[id.0 as usize];
            if let Some(expression) = &function.body.expression {
                walk_expression_calls(program, memory, expression, &mut collector)?;
            }
            walk_statement_calls(program, memory, &function.body.statements, &mut collector)?;
        }
        if !collector.dynamic_calls { break; }
        // Widening step: any closure value may flow into a dynamic callee.
        collector.dynamic_calls = false;
        let candidates = closure_functions(program);
        for callee in &candidates {
            if collector.seen.insert(*callee) { collector.reachable.push(*callee); }
        }
        if candidates.is_empty() { break; }
    }
    Ok((collector.reachable, collector.saw_dynamic_calls))
}

fn function_definition(
    program: &Program, memory: &MemoryPlan, function: &crate::hir::Function, collect: bool,
) -> Result<String, String> {
    let id = function.id.0;
    let activation = memory.function_activations.get(&function.id)
        .ok_or_else(|| format!("MemoryPlan incompleto: falta activación FunctionId {id}"))?;
    let inline_slots = activation.frame_slots.clone();
    let owned_bindings = activation.bindings.iter()
        .filter_map(|(binding, class)| matches!(class, StorageClass::OwnedRegion).then_some(*binding))
        .collect();
    let ownership_sites = activation.ownership_tokens.iter()
        .map(|(binding, token)| (*binding, token.0))
        .collect();
    let scope_lanes = memory.scope_stacks.iter()
        .map(|(scope, plan)| (*scope, plan.stack.0))
        .collect();
    let promotion_sites = memory.operations.iter()
        .map(|operation| (operation.statement, (operation.destination, operation.ownership_token.0)))
        .collect();
    let end_label = format!("fn_end_{id}");
    let result_slot = format!("fn_result_{id}");
    let cell_bindings = activation.cell_slots.keys().copied().collect();
    let mut generator = Generator {
        inline_slots, owned_bindings, ownership_sites, scope_lanes, promotion_sites,
        function_return: Some((end_label.clone(), result_slot.clone())),
        cell_bindings, collect, frame_owner: Some("fn_activation".into()),
        ..Generator::default()
    };
    let mut out = String::new();
    Generator::line(&mut out, format!("static HlValue hl_fn_{id}(HlEnv *fn_parent, HlValue fn_closure, const HlValue *fn_args, size_t fn_arg_count) {{"));
    Generator::line(&mut out, format!(
        "    if (fn_arg_count != {}) {{ char message[128]; snprintf(message, sizeof(message), \"La función esperaba {} argumentos y recibió %zu\", fn_arg_count); hl_fail(fn_parent, message); }}",
        function.params.len(),
        function.params.len()
    ));
    Generator::line(&mut out, "    HlEnv *env = hl_env_new(fn_parent);");
    Generator::line(&mut out, "    HlEnv *fn_activation = env;");
    Generator::line(&mut out, "    hl_clear_source_range(env);");
    if function.captures.is_empty() {
        Generator::line(&mut out, "    (void)fn_closure;");
    } else {
        // Order matches the closure descriptor built at creation time.
        for (index, capture) in function.captures.iter().enumerate() {
            Generator::line(&mut out, format!(
                "    hl_binding_alias_cell(env, {}, hl_closure_capture_cell(fn_closure, {index}));",
                Generator::binding(program, capture.binding)?
            ));
        }
    }
    if activation.frame_slot_count > 0 {
        Generator::line(&mut out, format!("    HlFrameSlot frame_slots[{}];", activation.frame_slot_count));
        Generator::line(&mut out, "    HlFrame frame;");
        Generator::line(&mut out, format!("    hl_frame_init(&frame, frame_slots, {});", activation.frame_slot_count));
    } else {
        Generator::line(&mut out, "    HlFrame frame = { NULL, 0 };");
    }
    // The frame and the pending result are roots of the closure/cell graph.
    Generator::line(&mut out, "    hl_root_frame_push(env, &frame);");
    for (index, parameter) in function.params.iter().enumerate() {
        if let Some(slot) = activation.frame_slots.get(parameter) {
            Generator::line(&mut out, format!(
                "    hl_frame_write_or_fail(env, fn_activation, &frame, {slot}, fn_args[{index}]);"
            ));
            continue;
        }
        // Captured parameters need a stable, name-addressable binding so a
        // closure created inside this activation can share their cell.
        if !activation.cell_slots.contains_key(parameter) {
            return Err(format!(
                "MemoryPlan incompleto: el parámetro BindingId {} de FunctionId {id} no tiene slot de frame ni celda",
                parameter.0
            ));
        }
        let site = activation.ownership_tokens.get(parameter).map_or(0, |token| token.0);
        Generator::line(&mut out, format!(
            "    hl_define_planned_site(env, {}, hl_clone_value(env, fn_args[{index}]), HL_STORAGE_OWNED, UINT64_C({site}));",
            Generator::binding(program, *parameter)?
        ));
    }
    Generator::line(&mut out, format!("    HlValue {result_slot} = hl_number(0);"));
    Generator::line(&mut out, format!("    hl_root_value_push(env, &{result_slot});"));
    let body_start = out.len();
    if let Some(expression) = &function.body.expression {
        let value = generator.expr(program, expression, &mut out)?;
        Generator::line(&mut out, format!("    {result_slot} = {value};"));
        generator.collect_point(&mut out);
    }
    generator.statements(program, &function.body.statements, &mut out)?;
    let declarations = generator.temp_declarations.concat();
    out.insert_str(body_start, &declarations);
    Generator::line(&mut out, format!("{end_label}: ;"));
    Generator::line(&mut out, format!("    hl_root_value_pop(env, &{result_slot});"));
    Generator::line(&mut out, "    hl_root_frame_pop(env);");
    Generator::line(&mut out, format!("    HlValue fn_out_{id} = hl_clone_value(fn_parent, {result_slot});"));
    Generator::line(&mut out, "    hl_env_free(env);");
    Generator::line(&mut out, format!("    return fn_out_{id};"));
    Generator::line(&mut out, "}");
    Ok(out)
}

fn call_dispatcher(reachable: &[crate::hir::FunctionId]) -> String {
    let mut out = String::new();
    Generator::line(&mut out, "static HlValue hl_call(HlEnv *fn_caller, HlValue fn_callee, const HlValue *fn_args, size_t fn_arg_count) {");
    Generator::line(&mut out, "    if (fn_callee.tag != HL_CLOSURE || !fn_callee.as.closure)");
    Generator::line(&mut out, "        hl_fail(fn_caller, \"solo se pueden llamar valores de función: el valor no es una función\");");
    Generator::line(&mut out, "    switch (fn_callee.as.closure->function_id) {");
    for id in reachable {
        Generator::line(&mut out, format!("        case {}: return hl_fn_{}(fn_caller, fn_callee, fn_args, fn_arg_count);", id.0, id.0));
    }
    Generator::line(&mut out, "        default: break;");
    Generator::line(&mut out, "    }");
    Generator::line(&mut out, "    hl_fail(fn_caller, \"esta función aún no está soportada por el backend nativo\");");
    Generator::line(&mut out, "    return hl_number(0);");
    Generator::line(&mut out, "}");
    out
}

fn validate_native_statements(statements: &[Statement]) -> Result<(), String> {
    for statement in statements {
        match &statement.kind {
            StmtKind::Let { value, .. } | StmtKind::Assign { value, .. }
            | StmtKind::Print(value) | StmtKind::Return(value) =>
                validate_native_expr(value)?,
            StmtKind::IndexAssign { indices, value, .. } => {
                for index in indices { validate_native_expr(index)?; }
                validate_native_expr(value)?;
            }
            StmtKind::Block { statements, .. } =>
                validate_native_statements(statements)?,
            StmtKind::If { condition, then_body, else_body, .. } => {
                validate_native_expr(condition)?;
                validate_native_statements(then_body)?;
                validate_native_statements(else_body)?;
            }
            StmtKind::While { condition, body, .. } => {
                validate_native_expr(condition)?;
                validate_native_statements(body)?;
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
    Ok(())
}

pub fn generate_c(program: &Program, memory: &MemoryPlan) -> Result<String, String> {
    // Validate every function body before emitting C. The backend below emits
    // reachable function bodies, activation frames and the call dispatcher.
    validate_native_statements(&program.statements)?;
    for function in &program.functions {
        if let Some(expression) = &function.body.expression {
            validate_native_expr(expression)?;
        }
        validate_native_statements(&function.body.statements)?;
    }
    // Only functions reachable from a supported call site get C bodies. Their
    // activation frames and return path are materialized below.
    let (reachable, has_dynamic_calls) = reachable_functions(program, memory)?;

    // Reject stale/incomplete plans instead of silently compiling without the
    // frontend's storage decisions. ScopeStackPlan is materialized below as
    // physical HlArenaLane instances; BindingSlotPlan remains frame-layout
    // metadata and is materialized separately as activation-local frame slots.
    for binding in &program.bindings {
        if let Some(owner) = binding.function_owner {
            let activation = memory.function_activations.get(&owner)
                .ok_or_else(|| format!("MemoryPlan incompleto: falta activación FunctionId {}", owner.0))?;
            if !activation.bindings.contains_key(&binding.id) {
                return Err(format!("MemoryPlan incompleto: falta BindingId {} en FunctionId {}", binding.id.0, owner.0));
            }
        } else if !memory.bindings.contains_key(&binding.id) {
            return Err(format!("MemoryPlan incompleto: falta BindingId {}", binding.id.0));
        }
    }
    // Validate stack counts, stack ownership, and interference constraints before codegen.
    crate::memory::validate_function_activation_plan(program, memory)?;
    crate::memory::validate_slot_plan(program, memory)?;
    crate::memory::validate_memory_operations(program, memory)?;
    let layout = crate::memory::frame_layout(program, memory)?;
    let inline_stack = crate::memory::DataStackId(u32::MAX);
    let inline_count = layout.stacks.get(&inline_stack).map_or(0, |slots| slots.len());
    // Module bindings captured by a closure keep a name-addressable environment
    // binding instead of a module frame slot, so capture creation can attach a
    // shared cell to it.
    let cell_bindings: std::collections::HashSet<_> = memory.module_cells.iter().copied().collect();
    let inline_slots: HashMap<_, _> = memory.binding_slots.iter()
        .filter(|(_, assignment)| assignment.stack == inline_stack)
        .filter(|(binding, _)| !cell_bindings.contains(binding))
        .map(|(binding, assignment)| (*binding, assignment.slot))
        .collect();
    let owned_bindings = memory.bindings.iter()
        .filter_map(|(binding, class)| matches!(class, StorageClass::OwnedRegion).then_some(*binding))
        .collect();
    let ownership_sites: HashMap<_, _> = memory.ownership_tokens.iter()
        .map(|(binding, token)| (*binding, token.0))
        .collect();
    let scope_lanes: HashMap<_, _> = memory.scope_stacks.iter()
        .map(|(scope, plan)| (*scope, plan.stack.0))
        .collect();
    let lane_count = scope_lanes.values().copied().max().map_or(0, |max| max + 1);
    let promotion_sites = memory.operations.iter()
        .map(|operation| (operation.statement, (operation.destination, operation.ownership_token.0)))
        .collect();
    let collect = !closure_functions(program).is_empty();
    let mut body = String::new();
    let mut generator = Generator {
        inline_slots, owned_bindings, ownership_sites: ownership_sites.clone(),
        scope_lanes: scope_lanes.clone(), promotion_sites,
        cell_bindings: cell_bindings.clone(), collect, frame_owner: Some("module_activation".into()),
        ..Generator::default()
    };
    // Module capture cells are root-owned and must exist before any module
    // statement runs: mutually recursive lambdas capture bindings that are
    // still undefined at closure-creation time.
    for binding in &memory.module_cells {
        let site = ownership_sites.get(binding).copied().unwrap_or(0);
        body.push_str(&format!(
            "hl_binding_predeclare(env, {}, UINT64_C({site}));\n",
            Generator::binding(program, *binding)?
        ));
    }
    generator.statements(program, &program.statements, &mut body)?;
    let temp_declarations = generator.temp_declarations.concat();
    let mut source = include_str!("native_runtime.c").to_string();
    if !reachable.is_empty() || has_dynamic_calls {
        source.push_str("\n/* Generated function bodies and activation frames. */\n");
        for id in &reachable {
            source.push_str(&format!(
                "static HlValue hl_fn_{}(HlEnv *fn_parent, HlValue fn_closure, const HlValue *fn_args, size_t fn_arg_count);\n",
                id.0
            ));
        }
        source.push_str("static HlValue hl_call(HlEnv *fn_caller, HlValue fn_callee, const HlValue *fn_args, size_t fn_arg_count);\n");
        for id in &reachable {
            source.push_str(&function_definition(program, memory, &program.functions[id.0 as usize], collect)?);
        }
        source.push_str(&call_dispatcher(&reachable));
    }
    source.push_str("\n/* Compiler MemoryPlan: physical scope lanes. */\n");
    for binding in &program.bindings {
        let class = if let Some(owner) = binding.function_owner {
            memory.function_activations[&owner].bindings[&binding.id]
        } else {
            memory.bindings[&binding.id]
        };
        let class_name = match class {
            StorageClass::Inline => "inline",
            StorageClass::ScopeRegion(_) => "scope-region",
            StorageClass::OwnedRegion => "owned-region",
        };
        source.push_str(&format!("/* binding {} ({}) => {} */\n", binding.id.0, binding.name, class_name));
        if let Some(slot) = memory.binding_slots.get(&binding.id) {
            source.push_str(&format!("/* binding {} => data-stack {} slot {} */\n",
                binding.id.0, slot.stack.0, slot.slot));
        } else if let Some(owner) = binding.function_owner {
            if let Some(slot) = memory.function_activations[&owner].cell_slots.get(&binding.id) {
                source.push_str(&format!("/* binding {} => function {} capture-cell {} */\n",
                    binding.id.0, owner.0, slot));
            }
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
    source.push_str("    HlEnv *module_activation = env;\n");
    source.push_str("    hl_root_frame_push(env, &frame);\n");
    source.push_str(&temp_declarations);
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
    fn native_backend_charges_statements_and_loop_iterations() {
        let ast = AstProgram {
            statements: vec![AstStmt::Print(PrintStatement {
                value: crate::ast::Expression::NumberLiteral(1.0),
            })],
        };
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("static void hl_eval_step(HlEnv *environment)"));
        assert!(c.contains("HACKLANG_MAX_EVAL_STEPS"));
        assert!(c.contains("HACKLANG_MAX_EVAL_MILLIS"));
        assert!(c.contains("max_eval_millis"));
        assert!(c.contains("se agotó el presupuesto de tiempo de CPU de ejecución nativa"));
        assert!(c.contains("HACKLANG_MAX_ARENA_BYTES"));
        assert!(c.contains("HACKLANG_MAX_HEAP_BYTES"));
        assert!(c.contains("heap_bytes_live"));
        assert!(c.contains("se agotó el presupuesto de memoria auxiliar del runtime nativo"));
        assert!(c.contains("arena_bytes_reserved"));
        assert!(c.contains("se agotó el presupuesto de memoria de arenas nativas"));
        assert!(c.contains("hl_eval_step(env);"));
        assert!(c.contains("se agotó el presupuesto de instrucciones de ejecución nativa"));
    }

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
        assert!(c.contains("typedef struct HlArenaLane"));
        assert!(c.contains("lane->current = block"));
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
        assert!(generated_main.contains("hl_define_planned_site(env,"));
        assert!(generated_main.contains("HL_STORAGE_SCOPE"));
        assert!(!generated_main.contains("HL_STORAGE_OWNED"));
    }

    #[test]
    fn generated_c_routes_nested_array_ownership_sites_into_runtime() {
        use crate::ast::{Expression as A, IndexAssignStatement, LetStatement, Program as AstProgram, Statement as AstStmt};
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "matrix".into(),
                value: A::Array(vec![
                    A::Array(vec![A::NumberLiteral(1.0), A::NumberLiteral(2.0)]),
                    A::Array(vec![A::NumberLiteral(3.0), A::NumberLiteral(4.0)]),
                ]),
            }),
            AstStmt::IndexAssign(IndexAssignStatement {
                name: "matrix".into(),
                indices: vec![A::NumberLiteral(1.0), A::NumberLiteral(0.0)],
                value: A::NumberLiteral(9.0),
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let binding = crate::hir::BindingId(0);
        assert_eq!(memory.bindings[&binding], StorageClass::OwnedRegion);
        assert_eq!(memory.ownership_tokens.get(&binding), Some(&crate::memory::OwnershipTokenId(1)));
        let c = generate_c(&lowered, &memory).unwrap();
        let generated_main = c.split("int main(void) {").last().unwrap();
        assert!(generated_main.contains("hl_define_planned_site(env,"));
        assert!(generated_main.contains("hl_index_assign_path(env,"));
        assert!(generated_main.contains("UINT64_C(1)"), "aggregate ownership site must reach generated C");
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
        assert!(generated_main.contains("hl_define_planned_site(env,"));
        assert!(generated_main.contains("UINT64_C(1)"), "planner ownership site must reach generated C");
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
        assert!(generated_main.contains("hl_assign_planned_site(env,"));
        assert!(generated_main.contains("UINT64_C(1)"));
        assert!(generated_main.contains("HL_STORAGE_OWNED"));
        assert!(!c.contains("hl_assign_promoted_copy"));
        assert!(c.contains("site de propiedad incoherente en asignación planificada"));
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
        assert!(c.contains("hl_frame_write_or_fail(env, module_activation, &frame,"));
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
    fn generated_c_emits_runtime_copy_for_each_proven_promotion() {
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "outer".into(), value: crate::ast::Expression::StringLiteral("initial".into()),
            }),
            AstStmt::Block(vec![
                AstStmt::Let(LetStatement {
                    name: "inner".into(), value: crate::ast::Expression::StringLiteral("temporary".into()),
                }),
                AstStmt::Assign(crate::ast::AssignStatement {
                    name: "outer".into(), value: crate::ast::Expression::Identifier("inner".into()),
                }),
            ]),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        assert_eq!(memory.operations.len(), 1);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("hl_assign_planned_site(env, \"outer\","));
        assert!(c.contains("UINT64_C(1)"));
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
        assert!(c.contains("hl_frame_write_or_fail(env, module_activation, &frame,"));
        assert!(c.contains("hl_frame_read_or_fail(env, &frame,"));
    }

    #[test]
    fn generated_c_lowers_array_literals_index_reads_and_index_assignments() {
        use crate::ast::{Expression as A, IndexAssignStatement, LetStatement, PrintStatement, Program as AstProgram, Statement as AstStmt};
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "items".into(),
                value: A::Array(vec![A::NumberLiteral(1.0), A::NumberLiteral(2.0)]),
            }),
            AstStmt::IndexAssign(IndexAssignStatement {
                name: "items".into(),
                indices: vec![A::NumberLiteral(0.0)],
                value: A::NumberLiteral(9.0),
            }),
            AstStmt::Print(PrintStatement {
                value: A::Index {
                    target: Box::new(A::Identifier("items".into())),
                    index: Box::new(A::NumberLiteral(0.0)),
                },
            }),
        ]};
        let lowered = crate::hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        let main = c.split("int main(void) {").last().unwrap();
        assert!(main.contains("hl_array(env, (HlValue[]){"));
        assert!(main.contains(r#"hl_index_assign(env, "items""#));
        assert!(main.contains("hl_index(env,"));
        assert!(!main.contains("la asignación por índice está representada"));
    }

    #[test]
    fn native_backend_emits_activation_frames_for_direct_calls() {
        use crate::ast::{Expression as A, LetStatement, Program as AstProgram, Statement as AstStmt};
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "identity".into(),
                value: A::Lambda {
                    params: vec!["value".into()],
                    body: Box::new(A::Identifier("value".into())),
                },
            }),
            AstStmt::Let(LetStatement {
                name: "result".into(),
                value: A::FunctionCall {
                    func: Box::new(A::Identifier("identity".into())),
                    args: vec![A::NumberLiteral(7.0)],
                },
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("static HlValue hl_fn_0(HlEnv *fn_parent"));
        assert!(c.contains("hl_frame_write_or_fail(env, fn_activation, &frame, 0, fn_args[0]);"));
        assert!(c.contains("case 0: return hl_fn_0(fn_caller, fn_callee, fn_args, fn_arg_count);"));
        assert!(c.contains("hl_call(env,"));
        assert!(c.contains("(void)fn_closure;"));
    }

    #[test]
    fn native_backend_installs_capture_cells_in_generated_bodies() {
        use crate::ast::{Expression as A, LetStatement, Program as AstProgram, Statement as AstStmt};
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "base".into(), value: A::NumberLiteral(2.0),
            }),
            AstStmt::Let(LetStatement {
                name: "add".into(),
                value: A::Lambda {
                    params: vec!["x".into()],
                    body: Box::new(A::Infix {
                        operator: crate::lexer::Token::Plus,
                        left: Box::new(A::Identifier("x".into())),
                        right: Box::new(A::Identifier("base".into())),
                    }),
                },
            }),
            AstStmt::Print(PrintStatement {
                value: A::FunctionCall {
                    func: Box::new(A::Identifier("add".into())),
                    args: vec![A::NumberLiteral(3.0)],
                },
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("hl_binding_alias_cell(env, \"base\", hl_closure_capture_cell(fn_closure, 0));"));
        let main = c.split("int main(void) {").last().unwrap();
        assert!(main.contains("hl_define_planned_site(env, \"base\""));
        assert!(main.contains("hl_closure_new(env, UINT32_C(0), (const char *[]){ \"base\" }, 1)"));
    }

    #[test]
    fn native_backend_keeps_captured_parameters_in_environment_bindings() {
        use crate::ast::{Expression as A, LetStatement, Program as AstProgram, Statement as AstStmt};
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "outer".into(),
                value: A::Lambda {
                    params: vec!["x".into()],
                    body: Box::new(A::Block(vec![
                        AstStmt::Let(LetStatement {
                            name: "inner".into(),
                            value: A::Lambda {
                                params: vec!["y".into()],
                                body: Box::new(A::Infix {
                                    operator: crate::lexer::Token::Plus,
                                    left: Box::new(A::Identifier("x".into())),
                                    right: Box::new(A::Identifier("y".into())),
                                }),
                            },
                        }),
                        AstStmt::Return(A::Identifier("x".into())),
                    ])),
                },
            }),
            AstStmt::Print(PrintStatement {
                value: A::FunctionCall {
                    func: Box::new(A::Identifier("outer".into())),
                    args: vec![A::NumberLiteral(4.0)],
                },
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("hl_define_planned_site(env, \"x\", hl_clone_value(env, fn_args[0])"));
        assert!(c.contains("hl_closure_new(env, UINT32_C(1), (const char *[]){ \"x\" }, 1)"));
    }

    #[test]
    fn native_backend_accepts_recursion_with_root_cells_and_tracing() {
        use crate::ast::{Expression as A, LetStatement, Program as AstProgram, Statement as AstStmt};
        // A recursive lambda captures its own module binding, so the binding
        // must exist with a root cell before the closure is constructed.
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "factorial".into(),
                value: A::Lambda {
                    params: vec!["n".into()],
                    body: Box::new(A::FunctionCall {
                        func: Box::new(A::Identifier("factorial".into())),
                        args: vec![A::NumberLiteral(1.0)],
                    }),
                },
            }),
            AstStmt::Print(PrintStatement {
                value: A::FunctionCall {
                    func: Box::new(A::Identifier("factorial".into())),
                    args: vec![A::NumberLiteral(5.0)],
                },
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        let generated_main = c.split("int main(void) {").last().unwrap();
        let predeclare = generated_main
            .find("hl_binding_predeclare(env, \"factorial\",")
            .expect("root cell must be predeclared");
        let creation = generated_main
            .find("hl_closure_new(env, UINT32_C(0)")
            .expect("closure must be created");
        assert!(predeclare < creation, "root cell must exist before its capture cell is requested");
        assert!(generated_main.contains("hl_collect(env);"), "statement boundaries must trace closures");
        assert!(c.contains("hl_root_frame_push(env, &frame);"));
        assert!(c.contains("hl_root_value_push(env, &fn_result_0);"));
        assert!(c.contains("case 0: return hl_fn_0(fn_caller, fn_callee, fn_args, fn_arg_count);"));
    }

    #[test]
    fn native_backend_dispatches_higher_order_callees_dynamically() {
        use crate::ast::{Expression as A, LetStatement, Program as AstProgram, Statement as AstStmt};
        // `apply` calls its parameter, so the target is only known at runtime.
        let ast = AstProgram { statements: vec![
            AstStmt::Let(LetStatement {
                name: "apply".into(),
                value: A::Lambda {
                    params: vec!["fn".into(), "value".into()],
                    body: Box::new(A::FunctionCall {
                        func: Box::new(A::Identifier("fn".into())),
                        args: vec![A::Identifier("value".into())],
                    }),
                },
            }),
            AstStmt::Let(LetStatement {
                name: "inc".into(),
                value: A::Lambda {
                    params: vec!["x".into()],
                    body: Box::new(A::Identifier("x".into())),
                },
            }),
            AstStmt::Print(PrintStatement {
                value: A::FunctionCall {
                    func: Box::new(A::Identifier("apply".into())),
                    args: vec![A::Identifier("inc".into()), A::NumberLiteral(2.0)],
                },
            }),
        ]};
        let lowered = hir::lower(&ast).unwrap();
        let memory = crate::memory::plan(&lowered);
        let c = generate_c(&lowered, &memory).unwrap();
        assert!(c.contains("hl_call(env, "), "dynamic callee must go through the runtime dispatcher");
        // Both the higher-order function and the closure it receives need bodies.
        assert!(c.contains("static HlValue hl_fn_0("));
        assert!(c.contains("static HlValue hl_fn_1("));
    }


}
