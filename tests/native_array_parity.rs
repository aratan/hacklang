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
fn invalid_array_write_indices_match_interpreter_error_classification() {
    assert_interpreter_native_failure(
        "array_write_out_of_range",
        "let values = [1, 2];\nvalues[9] = 3;\n",
        "fuera de rango",
    );
    assert_interpreter_native_failure(
        "array_write_fractional_index",
        "let values = [1, 2];\nvalues[1.5] = 3;\n",
        "entero no negativo",
    );
    assert_interpreter_native_failure(
        "array_write_negative_index",
        // The parser has no unary minus; produce the negative index at runtime.
        "let values = [1, 2];\nlet i = 0 - 1;\nvalues[i] = 3;\n",
        "entero no negativo",
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
let original = @("key": [1, 2], 7: "number", true: "boolean", [3, 4]: "array-key", @("inner": 1): "map-key");
let copied = original;
copied["key"] = [8, 9];
print original["key"][0];
print copied["key"][0];
print original[7];
print original[true];
print original[[3, 4]];
print original[@("inner": 1)];
"#,
    );
}

#[test]
fn duplicate_and_unicode_map_keys_match_interpreter() {
    assert_interpreter_native_parity(
        "map_duplicate_unicode_keys",
        r#"
let repeated = @("alpha": 1, "alpha": 2);
print repeated["alpha"];
print len(repeated);
let accented = @("ñ": "tilde", "🙂": "emoji");
print accented["ñ"];
print accented["🙂"];
print len(accented);
"#,
    );
}

#[test]
fn map_inside_array_copies_are_deep_and_independent() {
    assert_interpreter_native_parity(
        "map_inside_array_deep_copy",
        r#"
let rows = [@("name": "alpha", "tags": ["x"])];
let copy = rows;
copy[0]["name"] = "beta";
copy[0]["tags"][0] = "y";
print rows[0]["name"];
print rows[0]["tags"][0];
print copy[0]["name"];
print copy[0]["tags"][0];
print len(copy[0]);
"#,
    );
}

#[test]
fn array_inside_map_copy_mutation_matches_interpreter() {
    assert_interpreter_native_parity(
        "array_inside_map_deep_copy",
        r#"
let values = @("rows": [[1, 2], [3, 4]]);
let copy = values;
copy["rows"][1][0] = 99;
print values["rows"][1][0];
print copy["rows"][1][0];
print len(copy["rows"]);
"#,
    );
}

#[test]
fn map_promoted_from_child_scope_keeps_nested_values() {
    assert_interpreter_native_parity(
        "map_promotion_nested_values",
        r#"
let saved = @("k": [0]);
if (true) (
    let inner = @("deep": [1, 2]);
    saved = inner;
)
print saved["deep"][1];
print len(saved);
"#,
    );
}

#[test]
fn nested_maps_and_map_inside_array_match_interpreter() {
    assert_interpreter_native_parity(
        "nested_map_and_array_map",
        r#"
let catalog = @("items": @("first": 1));
catalog["items"]["second"] = 2;
let rows = [@("name": "alpha")];
rows[0]["name"] = "beta";
print catalog["items"]["first"];
print catalog["items"]["second"];
print rows[0]["name"];
"#,
    );
}

#[test]
fn missing_map_keys_and_nested_paths_match_interpreter_errors() {
    assert_interpreter_native_failure(
        "map_missing_key_read",
        "let values = @(\"present\": 1);\nprint values[\"missing\"];\n",
        "clave no encontrada",
    );
    assert_interpreter_native_failure(
        "map_missing_intermediate_write",
        "let values = @();\nvalues[\"missing\"][\"child\"] = 2;\n",
        "clave no encontrada",
    );
}

#[test]
fn maps_promoted_from_child_scopes_and_reacquired_after_scalar_match_interpreter() {
    assert_interpreter_native_parity(
        "map_scope_promotion_and_reacquire",
        r#"
let result = @("initial": 0);
if (true) (
    let temporary = @("promoted": 7);
    result = temporary;
)
result = 3;
result = @("reacquired": 9);
print result["reacquired"];
"#,
    );
}

#[test]
fn len_builtin_matches_interpreter_for_strings_arrays_and_maps() {
    assert_interpreter_native_parity(
        "len_builtin_collections",
        r#"
let values = [1, 2, 3];
let entries = @("alpha": 1, 2: 3);
let text = "mañana";
print len(values);
print len(entries);
print len(text);
print len(@());
"#,
    );
}

#[test]
fn string_indexing_matches_interpreter_for_ascii_and_unicode() {
    assert_interpreter_native_parity(
        "string_indexing_unicode",
        r#"
let text = "mañana";
print text[0];
print text[2];
print text[5];
let emoji = "A🌍B";
print emoji[1];
print emoji[2];
print len(emoji);
print text[1][0];
"#,
    );
}

#[test]
fn invalid_string_indices_match_interpreter_error_classification() {
    assert_interpreter_native_failure(
        "string_index_out_of_range",
        "let text = \"hola\";\nprint text[9];\n",
        "índice de cadena fuera de rango",
    );
    assert_interpreter_native_failure(
        "string_index_fractional",
        "let text = \"hola\";\nprint text[1.5];\n",
        "índice de una cadena debe ser entero no negativo",
    );
    assert_interpreter_native_failure(
        "string_index_negative",
        // The parser has no unary minus, so the negative index is produced at
        // runtime; this exercises the index contract instead of the lexer.
        "let text = \"hola\";\nlet i = 0 - 1;\nprint text[i];\n",
        "índice de una cadena debe ser entero no negativo",
    );
}

#[test]
fn printing_nested_arrays_and_maps_matches_interpreter_debug_format() {
    assert_interpreter_native_parity(
        "aggregate_debug_print",
        r#"
let values = [1, "two", [3, 4]];
let entries = @("name": "alpha", 2: [true, "line\nfeed"]);
print values;
print entries;
"#,
    );
}

#[test]
fn deeply_nested_maps_keep_value_semantics_match_interpreter() {
    assert_interpreter_native_parity(
        "deep_nested_maps",
        r#"
let tree = @("a": @("b": @("c": 1)));
print tree["a"]["b"]["c"];
tree["a"]["b"]["c"] = 7;
print tree["a"]["b"]["c"];
tree["a"]["b"] = @("d": 9);
print tree["a"]["b"]["d"];
print len(tree["a"]);
let copy = tree;
copy["a"]["b"]["d"] = 11;
print tree["a"]["b"]["d"];
print copy["a"]["b"]["d"];
let rows = [@("x": @("y": 1))];
rows[0]["x"]["y"] = 5;
print rows[0]["x"]["y"];
print len(rows[0]["x"]);
let nested = @("lista": [@("n": 2)]);
nested["lista"][0]["n"] = 4;
print nested["lista"][0]["n"];
"#,
    );
}

#[test]
fn nested_map_errors_match_interpreter_error_classification() {
    assert_interpreter_native_failure(
        "nested_map_missing_inner_key",
        "let tree = @(\"a\": @(\"b\": 1));\nprint tree[\"a\"][\"missing\"];\n",
        "clave no encontrada",
    );
    assert_interpreter_native_failure(
        "nested_map_deep_missing_key_read",
        "let tree = @(\"a\": @(\"b\": @(\"c\": 1)));\nprint tree[\"a\"][\"x\"][\"c\"];\n",
        "clave no encontrada",
    );
    assert_interpreter_native_failure(
        "nested_map_write_through_missing_key",
        "let tree = @(\"a\": @(\"b\": 1));\ntree[\"a\"][\"c\"][\"d\"] = 2;\n",
        "clave no encontrada",
    );
    assert_interpreter_native_failure(
        "nested_map_write_into_scalar",
        "let tree = @(\"a\": 1);\ntree[\"a\"][\"b\"] = 2;\n",
        "no se puede indexar por asignación",
    );
}

#[test]
fn type_errors_match_interpreter_error_classification() {
    assert_interpreter_native_failure(
        "index_scalar_value",
        "let x = 5;\nprint x[0];\n",
        "no se puede indexar",
    );
    assert_interpreter_native_failure(
        "map_plus_number",
        "print @(\"a\": 1) + 1;\n",
        "type mismatch",
    );
    assert_interpreter_native_failure(
        "array_plus_number",
        "let values = [1];\nprint values + 2;\n",
        "type mismatch",
    );
    assert_interpreter_native_failure(
        "len_rejects_scalar",
        "print len(5);\n",
        "len()",
    );
    assert_interpreter_native_failure(
        "call_non_function_value",
        "let y = 3;\nprint y(1);\n",
        "no es una función",
    );
    assert_interpreter_native_failure(
        "index_assign_read_only_value",
        "let n = 1;\nn[0] = 2;\n",
        "no se puede indexar por asignación",
    );
}


#[test]
fn invalid_array_read_indices_cover_negative_and_wrong_type() {
    assert_interpreter_native_failure(
        "array_read_negative_index",
        "let values = [1, 2];\nlet index = 0 - 1;\nprint values[index];\n",
        "entero no negativo",
    );
    assert_interpreter_native_failure(
        "array_read_boolean_index",
        "let values = [1, 2];\nprint values[true];\n",
        "entero no negativo",
    );
    assert_interpreter_native_failure(
        "array_read_string_index",
        "let values = [1, 2];\nprint values[\"first\"];\n",
        "entero no negativo",
    );
}

#[test]
fn string_index_errors_match_interpreter_classification() {
    assert_interpreter_native_failure(
        "string_read_out_of_range",
        "let value = \"a\";\nprint value[2];\n",
        "fuera de rango",
    );
    assert_interpreter_native_failure(
        "string_read_negative_index",
        "let value = \"a\";\nlet index = 0 - 1;\nprint value[index];\n",
        "entero no negativo",
    );
    assert_interpreter_native_failure(
        "string_read_fractional_index",
        "let value = \"abc\";\nprint value[1.5];\n",
        "entero no negativo",
    );
}

#[test]
fn missing_map_keys_and_non_indexable_values_match_interpreter_classification() {
    assert_interpreter_native_failure(
        "map_read_missing_key",
        "let values = @(\"present\": 1);\nprint values[\"missing\"];\n",
        "clave no encontrada",
    );
    assert_interpreter_native_failure(
        "nested_map_write_missing_intermediate",
        "let values = @(\"present\": 1);\nvalues[\"missing\"][\"child\"] = 2;\n",
        "clave no encontrada",
    );
    assert_interpreter_native_failure(
        "read_non_indexable_scalar",
        "let value = 7;\nprint value[0];\n",
        "no se puede indexar",
    );
    assert_interpreter_native_failure(
        "write_non_indexable_scalar",
        "let value = 7;\nvalue[0] = 1;\n",
        "no se puede indexar",
    );
}
