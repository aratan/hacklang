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
