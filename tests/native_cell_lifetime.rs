use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

#[test]
fn captured_cell_value_lane_is_reused_and_swept() {
    let id = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "hacklang-cell-lifetime-{}-{id}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create isolated test directory");

    let source = directory.join("cell_lifetime.c");
    let executable = directory.join("cell_lifetime");
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");

    fs::write(
        &source,
        r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "native_runtime.c"

static size_t lane_block_count(const HlArenaLane *lane) {
    size_t count = 0;
    for (const HlArenaBlock *block = lane->blocks; block; block = block->next) ++count;
    return count;
}

int main(void) {
    HlEnv *root = hl_env_new(NULL);
    if (!root) return 10;

    char *payload = (char *)malloc(16385);
    if (!payload) return 11;
    memset(payload, 'x', 16384);
    payload[16384] = '\0';

    HlValue initial;
    initial.tag = HL_STRING;
    initial.as.string = payload;
    HlCell *cell = hl_cell_new(root, initial, 1);
    if (!cell || !cell->value_lane.blocks) return 12;
    if (cell->value_lane.blocks == root->lane->blocks) return 13;

    for (size_t i = 0; i < 128; ++i) {
        payload[0] = (char)('a' + (i % 26));
        if (!hl_cell_write(cell, initial)) return 14;
        if (lane_block_count(&cell->value_lane) != 1) return 15;
        if (cell->value.as.string[0] != payload[0]) return 16;
    }

    /* The cell is not reachable from any binding, frame or temporary root.
     * Collection must unlink it and destroy its private value lane. */
    hl_collect(root);
    if (root->cells != NULL) return 17;

    hl_env_free(root);
    free(payload);
    return 0;
}
"#,
    )
    .expect("write C runtime fixture");

    let compile = Command::new("cc")
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-fsanitize=address,undefined")
        .arg("-fno-omit-frame-pointer")
        .arg("-I")
        .arg(&runtime)
        .arg(&source)
        .arg("-lm")
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("compile sanitizer-backed C runtime fixture");
    assert!(
        compile.status.success(),
        "C fixture compilation failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&executable)
        .env("ASAN_OPTIONS", "detect_leaks=1:halt_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .expect("run sanitizer-backed C runtime fixture");
    assert!(
        run.status.success(),
        "C fixture failed: stdout={} stderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    fs::remove_dir_all(directory).expect("remove isolated test directory");
}
