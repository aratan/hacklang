use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

#[test]
fn native_runtime_lifetime_and_unwinding_are_sanitizer_clean() {
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
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include "native_runtime.c"

static int cleanup_fd = -1;
static const char root_marker = 'R';
static const char child_marker = 'C';
static const char grandchild_marker = 'G';
static void record_cleanup(void *context) {
    const char marker = *(const char *)context;
    (void)write(cleanup_fd, &marker, 1);
}

static void noop_cleanup(void *context) {
    (void)context;
}

static size_t lane_block_count(const HlArenaLane *lane) {
    size_t count = 0;
    for (const HlArenaBlock *block = lane->blocks; block; block = block->next) ++count;
    return count;
}

int main(void) {
    HlEnv *root = hl_env_new(NULL);
    if (!root) return 10;

    /* The auxiliary heap budget tracks live bytes, not cumulative allocations. */
    const uint64_t heap_baseline = root->heap_bytes_live;
    for (size_t i = 0; i < 256; ++i) {
        void *scratch = hl_heap_alloc(root, 128);
        if (!scratch) return 27;
        memset(scratch, (int)i, 128);
        if (((uintptr_t)scratch % _Alignof(max_align_t)) != 0) return 28;
        hl_heap_free(scratch);
        if (root->heap_bytes_live != heap_baseline) return 29;
    }

    void *resized = hl_heap_alloc(root, 32);
    if (!resized) return 30;
    const uint64_t before_grow = root->heap_bytes_live;
    resized = hl_heap_realloc(root, resized, 96);
    if (!resized || root->heap_bytes_live != before_grow + 64) return 31;
    resized = hl_heap_realloc(root, resized, 8);
    if (!resized || root->heap_bytes_live != before_grow - 24) return 32;
    hl_heap_free(resized);
    if (root->heap_bytes_live != heap_baseline) return 33;

    /* A child scope shares the root budget and releases its registrations. */
    HlEnv *budget_child = hl_env_new(root);
    if (!budget_child) return 34;
    void *child_memory = hl_heap_alloc(budget_child, 64);
    if (!child_memory || root->heap_bytes_live <= heap_baseline) return 35;
    hl_heap_free(child_memory);
    if (root->heap_bytes_live != heap_baseline) return 36;
    if (!hl_env_defer(budget_child, noop_cleanup, NULL)) return 37;
    hl_env_free(budget_child);
    if (root->heap_bytes_live != heap_baseline) return 38;

    /* The limit is enforced across live allocations and can be recovered
     * after the rejected allocation is handled by the caller. */
    const uint64_t original_heap_limit = root->max_heap_bytes;
    root->max_heap_bytes = heap_baseline + sizeof(HlHeapHeader) + 128;
    void *limited = hl_heap_alloc(root, 128);
    if (!limited) return 39;
    if (hl_heap_alloc(root, 1) != NULL || !root->heap_limit_exceeded) return 40;
    root->heap_limit_exceeded = 0;
    hl_heap_free(limited);
    root->max_heap_bytes = original_heap_limit;
    if (root->heap_bytes_live != heap_baseline) return 41;

    char *payload = (char *)malloc(16385);
    if (!payload) return 11;
    memset(payload, 'x', 16384);
    payload[16384] = '\0';

    HlValue initial;
    initial.tag = HL_STRING;
    initial.as.string = payload;
    initial.string_length = 16384;
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
    if (root->heap_bytes_live != heap_baseline) return 42;

    /* Unreachable closure descriptors and their capture arrays are charged
     * and reclaimed by the same tracing collector. */
    HlValue unreachable_closure = hl_closure_new(root, 0, NULL, 0);
    if (unreachable_closure.tag != HL_CLOSURE || !root->closures) return 43;
    hl_collect(root);
    if (root->closures != NULL || root->heap_bytes_live != heap_baseline) return 44;

    /* A fatal runtime error must unwind every nested environment exactly once,
     * running child cleanups before parent cleanups. The child process lets the
     * parent inspect cleanup order despite hl_fail terminating with exit(1). */
    int descriptors[2];
    if (pipe(descriptors) != 0) return 18;
    pid_t child = fork();
    if (child < 0) return 19;
    if (child == 0) {
        close(descriptors[0]);
        cleanup_fd = descriptors[1];
        HlEnv *unwind_root = hl_env_new(NULL);
        HlEnv *unwind_child = hl_env_new(unwind_root);
        HlEnv *unwind_grandchild = hl_env_new(unwind_child);
        if (!hl_env_defer(unwind_root, record_cleanup, (void *)&root_marker)) _exit(20);
        if (!hl_env_defer(unwind_child, record_cleanup, (void *)&child_marker)) _exit(21);
        if (!hl_env_defer(unwind_grandchild, record_cleanup, (void *)&grandchild_marker)) _exit(22);
        hl_fail(unwind_grandchild, "fallo controlado durante unwinding");
        _exit(23);
    }

    close(descriptors[1]);
    char cleanup_order[4] = {0};
    size_t received = 0;
    while (received < 3) {
        ssize_t count = read(descriptors[0], cleanup_order + received, 3 - received);
        if (count <= 0) break;
        received += (size_t)count;
    }
    close(descriptors[0]);
    int child_status = 0;
    if (waitpid(child, &child_status, 0) != child) return 24;
    if (received != 3 || memcmp(cleanup_order, "GCR", 3) != 0) return 25;
    if (!WIFEXITED(child_status) || WEXITSTATUS(child_status) != 1) return 26;

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
