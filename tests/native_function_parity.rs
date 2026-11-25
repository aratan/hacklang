use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

fn case_directory() -> PathBuf {
    let id = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "hacklang-function-parity-{}-{id}",
        std::process::id()
    ));
    fs::create_dir(&path).expect("create isolated test directory");
    path
}

fn interpreter_output(stdout: &[u8]) -> String {
    let output = String::from_utf8(stdout.to_vec()).expect("interpreter output must be UTF-8");
    let start_marker = "--- Ejecutando código Hacklang ---\n";
    let end_marker = "--------------------------------\nPrograma finalizado sin errores.\n";
    let start = output
        .find(start_marker)
        .expect("interpreter execution marker must be present")
        + start_marker.len();
    let end = output[start..]
        .find(end_marker)
        .map(|offset| start + offset)
        .expect("interpreter completion marker must be present");
    output[start..end].to_string()
}

fn assert_interpreter_native_parity(name: &str, source_text: &str) {
    let directory = case_directory();
    let source = directory.join(format!("{name}.hl"));
    let executable = directory.join(name);
    fs::write(&source, source_text).expect("write HackLang fixture");

    let interpreter = Command::new(env!("CARGO_BIN_EXE_hl"))
        .arg("run")
        .arg(&source)
        .output()
        .expect("execute interpreter");
    assert!(
        interpreter.status.success(),
        "{name}: interpreter failed: {}",
        String::from_utf8_lossy(&interpreter.stderr)
    );

    let build = Command::new(env!("CARGO_BIN_EXE_hl"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native executable");
    assert!(
        build.status.success(),
        "{name}: native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let native = Command::new(&executable)
        .output()
        .expect("execute native program");
    assert!(
        native.status.success(),
        "{name}: native executable failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    let expected = interpreter_output(&interpreter.stdout);
    let actual = String::from_utf8(native.stdout).expect("native output must be UTF-8");
    assert_eq!(actual, expected, "{name}: interpreter/native output differs");

    fs::remove_dir_all(directory).expect("remove isolated test directory");
}

fn assert_native_build_failure(name: &str, source_text: &str, error_fragment: &str) {
    let directory = case_directory();
    let source = directory.join(format!("{name}.hl"));
    let executable = directory.join(name);
    fs::write(&source, source_text).expect("write HackLang failure fixture");

    let build = Command::new(env!("CARGO_BIN_EXE_hl"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native failure fixture");
    assert!(!build.status.success(), "{name}: native build unexpectedly succeeded");
    let error = String::from_utf8_lossy(&build.stderr).to_lowercase();
    assert!(
        error.contains(error_fragment),
        "{name}: native error did not contain '{error_fragment}': {error}"
    );
    assert!(!executable.exists(), "{name}: failed build left an executable behind");

    fs::remove_dir_all(directory).expect("remove isolated failure test directory");
}

#[test]
fn direct_calls_with_expression_and_block_bodies_match_interpreter() {
    assert_interpreter_native_parity(
        "direct_calls_bodies",
        r#"
let add = |x, y| (x + y);
print add(30, 12);
let sum = |a, b| (
    let total = a + b;
    return total;
);
print sum(3, 4);
let pick = |n| (
    if (n > 0) (
        return "positive";
    )
    return "other";
);
print pick(3);
print pick(0);
"#,
    );
}

#[test]
fn returned_aggregates_and_activation_locals_match_interpreter() {
    assert_interpreter_native_parity(
        "returned_aggregates",
        r#"
let fill = |n| (
    let values = [0, 0];
    let i = 0;
    while (i < 2) (
        values[i] = n + i;
        i = i + 1;
    );
    return values;
);
let first = fill(10);
let second = fill(20);
print first[0];
print first[1];
print second[0];
print second[1];
let make = |x| ([x, x + 1]);
let made = make(5);
print made[1];
"#,
    );
}

#[test]
fn arguments_and_index_writes_keep_value_semantics() {
    assert_interpreter_native_parity(
        "argument_value_semantics",
        r#"
let overwrite = |values| (
    values[0] = 99;
    return values[0];
);
let source = [1, 2];
print overwrite(source);
print source[0];
let update = |entries| (
    entries["x"] = 5;
    return entries["x"];
);
let base = @("x": 1);
print update(base);
print base["x"];
let append = |entries| (
    entries["added"] = 7;
    return len(entries);
);
print append(base);
print base["x"];
"#,
    );
}

#[test]
fn module_and_activation_captures_match_interpreter() {
    assert_interpreter_native_parity(
        "captures",
        r#"
let base = 10;
let add = |x| (x + base);
print add(5);
print add(0);
let total = 0;
let bump = |n| (total = total + n);
print bump(2);
print bump(3);
print total;
let counter = 0;
let inc = |n| (counter = counter + n);
let dec = |n| (counter = counter - n);
inc(10);
dec(3);
print counter;
let items = [1, 2];
let set_first = |v| (items[0] = v);
set_first(9);
print items[0];
print items[1];
let outer = |x| (
    let show = || (print x);
    print x;
);
outer(4);
outer(7);
"#,
    );
}

#[test]
fn captured_parameters_and_nested_scopes_match_interpreter() {
    assert_interpreter_native_parity(
        "captured_parameters",
        r#"
let factor = 3;
let scale = |value| (
    let keep = || (print value);
    if (value > 0) (
        print value * factor;
    )
    return value;
);
print scale(2);
print scale(0);
"#,
    );
}

#[test]
fn higher_order_callees_match_interpreter() {
    assert_interpreter_native_parity(
        "higher_order",
        r#"
let apply = |func, a| (func(a));
let inc = |x| (x + 1);
print apply(inc, 1);
let twice = |func, a| (func(func(a)));
print twice(inc, 5);
let base = 10;
let add_base = |x| (x + base);
print apply(add_base, 4);
let compose = |outer, inner, a| (outer(inner(a)));
print compose(inc, add_base, 0);
let ops = [inc, |x| (x * 10)];
print ops[1](3);
print ops[0](3);
let table = @("double": |x| (x * 2));
let lookup = |entries, key, a| (entries[key](a));
print lookup(table, "double", 7);
let pick = |flag, a| ((flag ? inc : add_base)(a));
print pick(1, 2);
print pick(0, 2);
"#,
    );
}


#[test]
fn nested_calls_and_binary_operands_preserve_left_to_right_evaluation() {
    assert_interpreter_native_parity(
        "left_to_right_evaluation",
        r#"
let sequence = 0;
let mark = |n| (
    sequence = sequence * 10 + n;
    return n;
);
let pair = |a, b| (a * 10 + b);
print pair(mark(1), mark(2));
print sequence;
sequence = 0;
print mark(3) + mark(4);
print sequence;
"#,
    );
}

#[test]
fn temporary_closure_arguments_survive_collection_during_later_arguments() {
    assert_interpreter_native_parity(
        "temporary_closure_root",
        r#"
let make = |value| (|| (value));
let churn = |count| (
    let i = 0;
    while (i < count) (
        let inner = |k| ((k < 1) ? i : inner(k - 1));
        i = i + 1;
    );
    return count;
);
let consume = |func, extra| (func() + extra);
print consume(make(40), churn(8));
"#,
    );
}


#[test]
fn returns_from_nested_scopes_preserve_dynamic_values() {
    assert_interpreter_native_parity(
        "nested_scope_returns",
        r#"
let return_text = |flag| (
    if (flag) (
        let value = "nested";
        return value;
    )
    return "fallback";
);
print return_text(1);
print return_text(0);
let return_array = |flag| (
    if (flag) (
        let values = [7, 8];
        return values;
    )
    return [0];
);
let result = return_array(1);
print result[0];
print result[1];
"#,
    );
}

#[test]
fn higher_order_argument_errors_match_interpreter() {
    assert_native_build_failure(
        "static_arity_mismatch",
        "let add = |x, y| (x + y);\nprint add(1);\n",
        "espera 2 argumentos",
    );
}

#[test]
fn recursive_and_mutually_recursive_calls_match_interpreter() {
    assert_interpreter_native_parity(
        "recursion",
        r#"
let factorial = |n| ((n < 2) ? 1 : n * factorial(n - 1));
print factorial(5);
print factorial(0);
let fib = |n| ((n < 2) ? n : fib(n - 1) + fib(n - 2));
print fib(10);
let even = |n| ((n < 1) ? 1 : odd(n - 1));
let odd = |n| ((n < 1) ? 0 : even(n - 1));
print even(10);
print odd(7);
let unused = |n| unused(n);
print 1;
"#,
    );
}

#[test]
fn recursive_closures_created_per_activation_match_interpreter() {
    // Each activation builds its own self-recursive closure, so the closure and
    // its capture cell form a cycle that tracing collection must reclaim.
    assert_interpreter_native_parity(
        "recursive_closures",
        r#"
let make = |n| (
    let countdown = |k| ((k < 1) ? n : countdown(k - 1));
    return countdown;
);
let down = make(4);
print down(10);
let other = make(9);
print other(2);
print down(3);
let tally = 0;
let i = 0;
while (i < 5) (
    let inner = |k| ((k < 1) ? i : inner(k - 1));
    tally = tally + inner(3);
    i = i + 1;
);
print tally;
"#,
    );
}
