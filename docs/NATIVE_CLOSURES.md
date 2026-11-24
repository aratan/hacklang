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

The capture pass and logical cell plan are run as part of AST -> HIR lowering. `MemoryPlan` now also contains a conservative `FunctionActivationPlan` for each function: parameters default to owned storage, captured locals are forced into activation cells, and each activation receives deterministic ownership-site metadata. Module bindings captured by closures are likewise excluded from ephemeral inline/lexical storage.

Native closure invocation remains disabled: generated C materializes closure expressions as runtime descriptors, but does not yet emit function bodies or calls. The generator validates module bindings against the module plan and parameters/locals against their owning `FunctionActivationPlan`; it no longer incorrectly requires function-local bindings to exist in the module binding map. The C runtime now represents closure values with a deterministic function ID and an ordered array of shared `HlCell` references. Capture construction resolves binding names to their live bindings, promotes values into root-owned cells, and connects subsequent binding assignments to those cells. Closure capture reads/writes share that identity after the creating activation is destroyed. The HIR-to-C generator now lowers closure expressions using their deterministic `FunctionId` and ordered captured `BindingId` names. Runtime values also handle closure truthiness, identity equality, cloning (shared descriptor), and debug/print output. Sanitizer-backed runtime tests cover closure capture mutation and scope exit; a native compile/run test covers closure construction with a module capture. Descriptors/cells are conservatively retained until root teardown; tracing collection, generated capture-slot access, temporary roots, and structured unwinding remain prerequisites for native invocation. GitHub Actions Rust CI run 898 passed: type-check, build, 276 unit tests, 18 differential tests, supported `.hl` examples, and interpreter/native smoke comparison.
