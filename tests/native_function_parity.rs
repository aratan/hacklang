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
fn capture_recursion_and_dynamic_callees_stay_rejected() {
    assert_native_build_failure(
        "capturing_function",
        "let factorial = |n| ((n < 2) ? 1 : n * factorial(n - 1));\nprint factorial(5);\n",
        "captura variables de su entorno",
    );
    assert_native_build_failure(
        "higher_order_callee",
        "let apply = |func, a| (func(a));\nlet inc = |x| (x + 1);\nprint apply(inc, 1);\n",
        "valor de función dinámico",
    );
    assert_native_build_failure(
        "arity_mismatch",
        "let add = |x, y| (x + y);\nprint add(1);\n",
        "espera 2 argumentos",
    );
}
