use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

fn case_directory() -> PathBuf {
    let id = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "hacklang-array-parity-{}-{id}",
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

fn assert_interpreter_native_failure(name: &str, source_text: &str, error_fragment: &str) {
    let directory = case_directory();
    let source = directory.join(format!("{name}.hl"));
    let executable = directory.join(name);
    fs::write(&source, source_text).expect("write HackLang failure fixture");

    let interpreter = Command::new(env!("CARGO_BIN_EXE_hl"))
        .arg("run")
        .arg(&source)
        .output()
        .expect("execute interpreter failure fixture");
    assert!(!interpreter.status.success(), "{name}: interpreter unexpectedly succeeded");
    let interpreter_error = String::from_utf8_lossy(&interpreter.stderr).to_lowercase();
    assert!(
        interpreter_error.contains(error_fragment),
        "{name}: interpreter error did not contain '{error_fragment}': {interpreter_error}"
    );

    let build = Command::new(env!("CARGO_BIN_EXE_hl"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("build native failure fixture");
    assert!(
        build.status.success(),
        "{name}: native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let native = Command::new(&executable)
        .output()
        .expect("execute native failure fixture");
    assert!(!native.status.success(), "{name}: native executable unexpectedly succeeded");
    let native_error = String::from_utf8_lossy(&native.stderr).to_lowercase();
    assert!(
        native_error.contains(error_fragment),
        "{name}: native error did not contain '{error_fragment}': {native_error}"
    );

    fs::remove_dir_all(directory).expect("remove isolated failure test directory");
}

#[test]
fn arrays_match_interpreter_for_literals_reads_and_direct_mutation() {
    assert_interpreter_native_parity(
        "array_literals_reads_mutation",
        r#"
let values = [10, 20, 30];
print values[0];
print values[1];
values[1] = 99;
print values[1];
print values[2];
"#,
    );
}

#[test]
fn nested_array_reads_and_value_copies_match_interpreter() {
    assert_interpreter_native_parity(
        "nested_array_value_copy",
        r#"
let matrix = [[1, 2], [3, 4]];
let copied = matrix;
copied[0] = [8, 9];
print matrix[0][1];
print copied[0][1];
print matrix[1][0];
"#,
    );
}

#[test]
fn arrays_promoted_from_child_scopes_match_interpreter() {
    assert_interpreter_native_parity(
        "array_scope_promotion",
        r#"
let result = [[0, 0]];
if (true) (
    let temporary = [[7, 8], [9, 10]];
    result = temporary;
)
print result[0][0];
print result[0][1];
print result[1][0];
print result[1][1];
"#,
    );
}

#[test]
fn array_reads_inside_loop_match_interpreter() {
    assert_interpreter_native_parity(
        "array_loop_reads",
        r#"
let values = [4, 5, 6];
let index = 0;
while (index < 3) (
    print values[index];
    index = index + 1;
)
"#,
    );
}

#[test]
fn nested_array_mutation_matches_interpreter_and_preserves_value_copies() {
    assert_interpreter_native_parity(
        "nested_array_mutation",
        r#"
let matrix = [[1, 2], [3, 4]];
let copied = matrix;
let row = 1;
let column = 0;
matrix[row][column] = 77;
copied[0][column] = 88;
print matrix[0][0];
print matrix[0][1];
print matrix[1][0];
print matrix[1][1];
print copied[0][0];
print copied[0][1];
print copied[1][0];
"#,
    );
}

#[test]
fn three_level_array_mutation_matches_interpreter() {
    assert_interpreter_native_parity(
        "three_level_array_mutation",
        r#"
let cube = [[[1, 2], [3, 4]], [[5, 6], [7, 8]]];
cube[1][0][1] = 66;
print cube[0][1][0];
print cube[1][0][0];
print cube[1][0][1];
print cube[1][1][1];
"#,
    );
}

#[test]
fn invalid_array_read_indices_match_interpreter_error_classification() {
    assert_interpreter_native_failure(
        "array_read_out_of_range",
        "let values = [1, 2];\nprint values[9];\n",
        "fuera de rango",
    );
    assert_interpreter_native_failure(
        "array_read_fractional_index",
        "let values = [1, 2];\nprint values[1.5];\n",
        "entero no negativo",
    );
    assert_interpreter_native_failure(
        "nested_array_write_out_of_range",
        "let matrix = [[1, 2]];\nmatrix[0][9] = 7;\n",
        "fuera de rango",
    );
}

#[test]
fn invalid_nested_array_intermediate_matches_interpreter_error_classification() {
    assert_interpreter_native_failure(
        "nested_array_write_non_array_intermediate",
        "let matrix = [1, 2];\nmatrix[0][1] = 7;\n",
        "no se puede",
    );
}

#[test]
fn nested_array_replacement_repeatedly_matches_interpreter() {
    assert_interpreter_native_parity(
        "nested_array_repeated_replacement",
        r#"
let matrix = [[0, 0]];
let index = 0;
while (index < 40) (
    matrix[0] = [index, index + 1];
    index = index + 1;
)
print matrix[0][0];
print matrix[0][1];
"#,
    );
}

#[test]
fn owned_arrays_survive_break_and_continue_scope_exits() {
    assert_interpreter_native_parity(
        "array_loop_control_scope_cleanup",
        r#"
let result = [[0]];
let index = 0;
while (index < 5) (
    let temporary = [[index, index + 1]];
    if (index < 2) (
        index = index + 1;
        continue;
    )
    result = temporary;
    if (index > 3) (
        break;
    )
    index = index + 1;
)
print result[0][0];
print result[0][1];
"#,
    );
}

#[test]
fn owned_array_binding_can_change_type_and_acquire_again() {
    assert_interpreter_native_parity(
        "array_binding_resource_reacquire",
        r#"
let value = [[1, 2]];
value = 7;
value = [[3, 4]];
print value[0][0];
print value[0][1];
"#,
    );
}

#[test]
fn map_literals_reads_updates_and_insertions_match_interpreter() {
    assert_interpreter_native_parity(
        "map_literals_reads_updates",
        r#"
let values = @("alpha": 10, "beta": 20);
print values["alpha"];
print values["beta"];
values["alpha"] = 99;
values["gamma"] = 30;
print values["alpha"];
print values["gamma"];
"#,
    );
}

#[test]
fn map_copies_are_independent_and_keys_use_value_equality() {
    assert_interpreter_native_parity(
        "map_copy_key_equality",
        r#"
let original = @("key": [1, 2], 7: "number", true: "boolean", [3, 4]: "array-key");
let copied = original;
copied["key"] = [8, 9];
print original["key"][0];
print copied["key"][0];
print original[7];
print original[true];
print original[[3, 4]];
"#,
    );
}
