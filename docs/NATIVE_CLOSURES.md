# Native functions and closures: runtime contract

This document defines the compatibility contract for adding user functions and closures to the native backend. It is a design boundary, not a claim that native closures are implemented.

## Existing language semantics

The interpreter in `src/evaluador.rs` is authoritative:

- A function value contains its parameter list, body, and the identity of the environment in which the lambda was evaluated.
- Captures are by environment reference, not by copying the values visible at lambda creation.
- Assignments through one closure are visible through sibling closures that capture the same environment.
- A returned closure keeps its captured frame alive after the creating call and its lexical blocks have returned.
- Functions may be recursive or mutually recursive; function values may be nested inside arrays and maps.
- The evaluator uses mark-and-sweep over environment IDs. It traces from the root and current environments, temporary return values, parent links, and function values recursively nested in arrays/maps. This handles unreachable cycles that reference counting alone would leak.

Existing interpreter regressions include returned closures, mutable captures, sibling closures sharing a capture, recursive and mutually recursive closures, and closures nested in escaping collections (see the `mem_007`–`mem_012` tests and adjacent closure tests in `src/evaluador.rs`).

## Native runtime requirements

A native implementation must preserve all of the following:

1. **Stable environment identity.** A closure must not retain a pointer into a lexical lane that is rewound when its block exits.
2. **Shared mutable cells.** Captured bindings must remain addressable by every closure sharing the environment. Capturing a snapshot is not equivalent.
3. **Escape-safe lifetime.** Returning a closure, or storing one in an array/map that escapes, must preserve its captured environment and all required parent environments.
4. **Cycle collection.** Self-recursive and mutually recursive closures can form cycles through environment bindings. Plain reference counting is insufficient.
5. **Temporary roots.** A closure held only in an intermediate expression must remain live across evaluation of its arguments and any nested expression that can trigger collection.
6. **Structured unwinding.** Return, break, continue, and runtime errors must unwind active lexical scopes while retaining captured environments that remain reachable.
7. **Exactly-once resource cleanup.** Sweeping an environment must execute its existing cleanup registrations once, before its region/lane storage is reclaimed. It must not bypass the current ownership-token invalidation rules.

## Intended implementation order

1. **Completed in HIR:** explicit function bodies, parameters, call sites, return statements, deterministic function IDs, function scopes, and binding ownership by function activation.
2. **Completed as HIR metadata:** direct capture analysis over resolved `BindingId` values, including read/write access and same-scope self/mutual recursion. This is not yet a storage-lifetime proof.
3. **Logical activation planning completed:** `MemoryPlan` records per-function parameter/local storage, captured activation cells, deterministic cell slots, module-cell slots, and ownership-site identities. The C runtime now has stable heap-allocated `HlCell` objects registered on the root environment; reads/writes share cell identity, values (including nested arrays/maps and strings) are cloned into root-owned storage, and cells survive destruction of the creating activation. This is deliberately conservative retention until root teardown, not garbage collection.
4. Lower calls to generated C functions with explicit activation frames. Materialize parameter/local storage and captured-cell slots from `FunctionActivationPlan`; route every return through a single cleanup/unwind path that preserves returned closures and aggregates before destroying non-escaping locals.
5. Add a root-owned environment registry and tracing collection. Roots must include active environments, module bindings, call temporaries, return values, and aggregate values. Trace function captures through arrays and maps; sweep unreachable environment cycles.
6. Enable native function syntax incrementally only after differential tests cover each interpreter contract above. Run full CI and sanitizer-backed runtime tests after each ownership change.

## Explicit non-goals for the first implementation

- Do not implement captures by value as a shortcut.
- Do not use raw pointers to child scopes after their lexical cleanup.
- Do not use reference counting as the sole lifetime mechanism.
- Do not declare closures complete after testing only top-level lambdas or non-escaping calls.
- Do not reuse a captured binding's stack slot while any reachable closure can observe it.

## Current status

The interpreter already implements and tests the semantics above. The HIR now represents:

- Function IDs and lexical function-parent relationships.
- Function scopes and parameter bindings using the same `BindingId` space as ordinary variables.
- Expression-bodied and block-bodied lambdas.
- Closure construction, general call sites, and explicit `return` statements.
- Direct free-variable captures with read/write access flags, resolved from HIR `BindingId` values rather than a second name-resolution table.
- Same-scope self-recursive and mutually recursive lambda declarations through deterministic predeclaration of function bindings.
- A logical `CaptureStoragePlan` distinguishing module-root cells from activation-owned cells, with the functions that reference each binding.

The capture pass and logical cell plan are run as part of AST -> HIR lowering. `MemoryPlan` now also contains a conservative `FunctionActivationPlan` for each function: parameters default to owned storage, captured locals are forced into activation cells, captured bindings receive deterministic cell slots, and non-captured parameters/locals receive a separate dense activation-frame slot layout. The plan also records each parameter's destination in source argument order (`Frame` or `CaptureCell`), so the call ABI initializes captured parameters correctly without guessing from slot numbers. Each activation also receives deterministic ownership-site metadata. The frame layout is now materialized for the callable subset described below. Module bindings captured by closures are likewise excluded from ephemeral inline/lexical storage.

**Direct calls are enabled, including functions with captures.** `generate_c` emits one C function per reachable call target, with an activation `HlEnv`, `HlFrame` slots from `FunctionActivationPlan`, parameters cloned into the activation lane, and a single return path that clones the result into the caller environment before freeing the activation. A generated `hl_call` dispatcher checks the callee value, arity and `FunctionId`, and passes the callee closure value into the C function so it can install its capture cells. Call targets are resolved statically: a call is accepted only when the callee is a lambda literal or a module-level binding that is statically a function value and is never reassigned. Calls are rejected at compile time when the callee is dynamic (higher-order parameters, function values passed around), when the arity does not match, or when the call graph contains a cycle.

Capture access in generated bodies is enabled. At function entry the generated code aliases each `Function.captures` entry, in declaration order, to `hl_closure_capture_cell(fn_closure, index)`; the cells were created when the closure descriptor was built, so sibling closures and the creating activation share the same mutable identity. Parameters and locals listed in `FunctionActivationPlan.captured_cells` are kept as name-addressable environment bindings instead of frame slots, and module bindings captured by any closure are excluded from module frame slots (`MemoryPlan.module_cells`), so `hl_binding_capture` can attach a cell to the exact live binding. Reads and writes go through `hl_get`/`hl_assign_planned_site`, which already consult the cell, and index mutation synchronizes the cell after in-place array/map writes. Index assignment on frame-held parameters/locals still uses `hl_index_assign_value`.

Recursion remains disabled. A recursive lambda captures its own module binding, which does not exist yet when the closure descriptor is constructed, and root cells would form cycles that the runtime does not trace. `generate_c` therefore rejects every call cycle with an explicit compile-time error; mutual recursion is covered by the same check. Dynamic callees and higher-order function values remain rejected. Descriptors and cells stay conservatively retained until root teardown; tracing collection, temporary roots and structured unwinding are still prerequisites for recursion and higher-order calls.

Differential tests cover module captures, mutable shared captures, captured aggregates with index writes, captured parameters and nested closures created but not invoked. Sanitizer-backed tests cover the same success paths plus a runtime error raised inside a capturing function. GitHub Actions Rust CI passes with this subset included in the differential and sanitizer suites.
