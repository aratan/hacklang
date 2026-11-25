# Native functions and closures: runtime contract

This document records the compatibility contract and current implementation of native functions and closures. The backend now supports direct and dynamic calls, captures, higher-order calls, recursion, and targeted tracing of closure/cell cycles; the remaining gaps are listed below.

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

**Direct calls are enabled, including functions with captures.** `generate_c` emits one C function per reachable call target, with an activation `HlEnv`, `HlFrame` slots from `FunctionActivationPlan`, parameters cloned into the activation lane, and a single return path that clones the result into the caller environment before freeing the activation. A generated `hl_call` dispatcher checks the callee value, arity and `FunctionId`, and passes the callee closure value into the C function so it can install its capture cells. Before a `return` unwinds nested lexical environments, the returned value is cloned into the function activation environment; the shared return path then clones it into the caller environment before freeing the activation. This prevents strings, arrays and maps allocated in an inner block from being read after that block's arena has been released. A call whose callee is a lambda literal or a module-level binding that is statically a function value and is never reassigned is resolved statically, including its arity check. Every other callee (a higher-order parameter, a value read from an array or map, a ternary of function values) is dispatched dynamically at runtime through `hl_call`, which rejects non-function values and arity mismatches with the same error class as the interpreter.

Capture access in generated bodies is enabled. At function entry the generated code aliases each `Function.captures` entry, in declaration order, to `hl_closure_capture_cell(fn_closure, index)`; the cells were created when the closure descriptor was built, so sibling closures and the creating activation share the same mutable identity. Parameters and locals listed in `FunctionActivationPlan.captured_cells` are kept as name-addressable environment bindings instead of frame slots, and module bindings captured by any closure are excluded from module frame slots (`MemoryPlan.module_cells`), so `hl_binding_capture` can attach a cell to the exact live binding. Reads and writes go through `hl_get`/`hl_assign_planned_site`, which already consult the cell, and index mutation synchronizes the cell after in-place array/map writes. Index assignment on frame-held parameters/locals still uses `hl_index_assign_value`.

Recursion and higher-order calls are enabled. `generate_c` records every call site whose callee identity is not statically known and widens the emitted set with every function that can become a closure value, so `hl_call` can dispatch it. Capture cells whose binding does not exist yet are handled by predeclaration: all module capture bindings are predeclared with an uninitialized root cell before the first module statement runs, and a function-local captured binding is predeclared before its initializer. A self-recursive lambda therefore captures the same cell identity that its own definition initializes later, and mutually recursive lambdas work because every module cell exists from the start of the program.

The runtime now owns the cycle problem instead of rejecting it. `HlEnv` for the root environment keeps a registry of active activation frames and protected expression temporaries (`hl_root_frame_push`, `hl_root_value_push`); `hl_collect` marks cells and closure descriptors from the live environment chain plus those roots, tracing closure captures and closures nested in arrays and maps, then sweeps the unmarked ones. The C generator emits each expression into a function-scope `HlValue` temporary and evaluates operands, callees and arguments in explicit source order, rather than relying on C's unspecified function-argument evaluation order. Each produced temporary is registered as a root immediately; roots are released after the containing statement, and before condition branches or non-local returns where appropriate. Statement-boundary collection therefore runs after evaluator temporaries have been unregistered, while values already stored in bindings, activation frames or return slots remain visible to tracing. This also prevents a closure produced by an early call argument from being reclaimed while a later argument performs nested calls and collection. A recursive closure created inside an activation is reclaimed once it becomes unreachable instead of being retained until root teardown. Cells still clone their values into root-owned storage; reclaiming that storage is the remaining lifetime task.

Differential tests cover module captures, mutable shared captures, captured aggregates with index writes, captured parameters, nested closures, higher-order callees (parameters, arrays, maps, ternaries), direct and mutual recursion, recursive closures created per activation, returns of strings, arrays and closure-containing arrays from nested lexical scopes, and left-to-right evaluation of side-effecting arguments and binary operands. Sanitizer-backed tests compile and execute generated C for direct calls, higher-order/recursive closures, shared captures, and temporary closure roots that must survive collection during evaluation of later arguments; runtime-error paths are also exercised. GitHub Actions Rust CI compares `examples/test_functions.hl` and the function-parity corpus between both backends and keeps a compile-time rejection guard for unsupported programs.

## Frame-slot lifetime and lexical promotion

An activation frame outlives every lexical block executed by that activation. Therefore, a value written to a frame slot must be cloned into the frame owner's environment, not the current `env`, which may be a shorter-lived block environment. Generated functions pass their stable `fn_activation` as the frame owner; module-level slots use the stable module activation. This applies to parameter initialization, local declarations, assignments, and index-assignment writeback.

The sanitizer regression creates a map containing nested arrays and a closure inside an `if` block, stores it through a captured module cell, exits the block, replaces the aggregate on a second call, then reads the nested strings and invokes the captured closure. ASan/UBSan verifies that the frame and cell graphs no longer retain pointers into the released lexical lane. Differential parity covers the same escape and replacement behavior.
