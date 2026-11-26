use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

#[test]
fn runtime_failure_unwinds_child_then_parent_cleanups_exactly_once() {
    let id = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "hacklang-error-unwind-{}-{id}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create isolated test directory");
    let source = directory.join("unwind.c");
    let executable = directory.join("unwind");
    let marker = directory.join("cleanup-order.txt");

    let c_source = format!(
        "{}\n{}",
        include_str!("../src/native_runtime.c"),
        r#"
static const char *marker_path;

static void record_cleanup(void *context) {
    FILE *file = fopen(marker_path, "ab");
    if (!file) return;
    fputc((int)(intptr_t)context, file);
    fclose(file);
}

int main(int argc, char **argv) {
    if (argc != 2) return 10;
    marker_path = argv[1];
    HlEnv *parent = hl_env_new(NULL);
    HlEnv *child = hl_env_new(parent);
    if (!parent || !child) return 11;
    if (!hl_env_defer(parent, record_cleanup, (void *)(intptr_t)'P')) return 12;
    if (!hl_env_defer(child, record_cleanup, (void *)(intptr_t)'C')) return 13;
    hl_fail(child, "forced unwinding");
    return 99;
}
"#
    );
    fs::write(&source, c_source).expect("write C runtime fixture");

    let compiler = ["cc", "gcc", "clang"]
        .into_iter()
        .find(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
        .expect("a C compiler is required by the native backend");
    let build = Command::new(compiler)
        .arg("-std=c11")
        .arg("-O1")
        .arg("-g")
        .arg("-fsanitize=address,undefined")
        .arg("-fno-omit-frame-pointer")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .arg("-lm")
        .output()
        .expect("compile runtime unwinding fixture with sanitizers");
    assert!(
        build.status.success(),
        "sanitized C fixture compilation failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&executable)
        .arg(&marker)
        .env("ASAN_OPTIONS", "detect_leaks=1:abort_on_error=1")
        .output()
        .expect("run runtime unwinding fixture");
    assert_eq!(run.status.code(), Some(1), "runtime error must exit with status 1: {}", String::from_utf8_lossy(&run.stderr));
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("forced unwinding"),
        "runtime error message was not preserved: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("cleanup marker must be written"),
        "CP",
        "each cleanup must run exactly once, child before parent"
    );

    fs::remove_dir_all(directory).expect("remove isolated test directory");
}
