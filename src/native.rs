use crate::ast::Program;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_BUILD_ID: AtomicU64 = AtomicU64::new(0);

fn select_c_compiler() -> Result<String, String> {
    for candidate in ["cc", "gcc", "clang"] {
        if Command::new(candidate).arg("--version").output().is_ok_and(|result| result.status.success()) {
            return Ok(candidate.to_string());
        }
    }
    Err("No se encontró un compilador C compatible. Instala GCC o Clang y vuelve a intentarlo.".into())
}

fn temporary_directory() -> Result<PathBuf, String> {
    let root = std::env::temp_dir();
    for _ in 0..100 {
        let id = NEXT_BUILD_ID.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!("hacklang-build-{}-{id}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("No se pudo crear el directorio temporal: {error}")),
        }
    }
    Err("No se pudo reservar un directorio temporal único para la compilación".into())
}

pub fn compile_to_executable(
    program: &Program,
    source_path: &Path,
    requested_output: Option<&Path>,
) -> Result<PathBuf, String> {
    // HIR es la única entrada del backend nativo.
    let hir = crate::hir::lower(program)
        .map_err(|error| format!("No se pudo construir HIR: {error}"))?;
    // Plan conservador: ningún valor se acorta de vida si el análisis no lo demuestra.
    let memory_plan = crate::memory::plan(&hir);
    let source_absolute = source_path.canonicalize()
        .map_err(|error| format!("No se pudo resolver el archivo fuente: {error}"))?;
    let output = requested_output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_path.with_extension(""));
    let output_absolute = if output.is_absolute() {
        output.clone()
    } else {
        std::env::current_dir().map_err(|error| error.to_string())?.join(&output)
    };
    if output_absolute == source_absolute {
        return Err("El ejecutable de salida no puede sobrescribir el archivo fuente".into());
    }
    if output.exists() {
        return Err(format!("El destino '{}' ya existe; no se sobrescribirá", output.display()));
    }

    let c_source = crate::hir_native::generate_c(&hir, &memory_plan)?;
    let compiler = select_c_compiler()?;
    let temporary = temporary_directory()?;
    let c_file = temporary.join("program.c");
    if let Err(error) = fs::write(&c_file, c_source)
        .map_err(|error| format!("No se pudo escribir el C temporal: {error}")) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(error);
    }

    let result = Command::new(&compiler)
        .arg("-std=c11")
        .arg("-O2")
        .arg(&c_file)
        .arg("-o")
        .arg(&output)
        .arg("-lm")
        .output();

    let _ = fs::remove_dir_all(&temporary);
    match result {
        Err(error) => Err(format!("No se pudo ejecutar {compiler}: {error}")),
        Ok(output_result) if output_result.status.success() => Ok(output),
        Ok(output_result) => {
            let diagnostics = String::from_utf8_lossy(&output_result.stderr);
            Err(format!("Falló la compilación C con {compiler}:\n{diagnostics}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        AssignStatement, Expression, IfStatement, LetStatement, PrintStatement, Statement,
        WhileStatement,
    };

    #[test]
    fn c_runtime_cleanup_callbacks_run_lifo_and_transfer_without_duplication() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("cleanup_runtime.c");
        let executable = directory.join("cleanup_runtime");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
typedef struct { int *order; int digit; } CleanupProbe;
static void record_cleanup(void *context) {
    CleanupProbe *probe = (CleanupProbe *)context;
    *probe->order = *probe->order * 10 + probe->digit;
}
int main(void) {
    int order = 0;
    CleanupProbe one = { &order, 1 };
    CleanupProbe two = { &order, 2 };
    CleanupProbe three = { &order, 3 };
    CleanupProbe four = { &order, 4 };
    CleanupProbe five = { &order, 5 };
    HlEnv *outer = hl_env_new(NULL);
    HlEnv *inner = hl_env_new(outer);
    HlAllocation *first = hl_env_defer(inner, record_cleanup, &one);
    HlAllocation *moved = hl_env_defer(inner, record_cleanup, &two);
    HlAllocation *cancelled = hl_env_defer(inner, record_cleanup, &three);
    if (!first || !moved || !cancelled) return 10;
    if (!hl_env_transfer(inner, outer, moved)) return 11;
    /* The old owner no longer controls this registration. A second transfer
     * or cancellation through it must fail without duplicating/dropping it. */
    if (hl_env_transfer(inner, outer, moved)) return 18;
    if (hl_env_cancel(inner, moved)) return 19;
    HlAllocation *outer_owned = hl_env_defer(outer, record_cleanup, &five);
    if (!outer_owned) return 12;
    if (hl_env_transfer(outer, inner, outer_owned)) return 13;
    if (!hl_env_cancel(inner, cancelled)) return 14;
    if (hl_env_cancel(inner, cancelled)) return 20;
    if (!hl_env_defer(inner, record_cleanup, &four)) return 15;
    hl_env_free(inner);
    if (order != 41) return 16;
    hl_env_free(outer);
    if (order != 4152) return 17;
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C runtime test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .arg("-fsanitize=address,undefined")
            .arg("-fno-omit-frame-pointer")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C cleanup registry test with sanitizers");
        assert!(
            build.status.success(),
            "sanitized C cleanup test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable).output().expect("run sanitized C cleanup registry test");
        assert!(
            run.status.success(),
            "C cleanup test failed with code {:?}",
            run.status.code()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_applies_scope_and_owned_storage_policies() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("planned_storage.c");
        let executable = directory.join("planned_storage");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *environment = hl_env_new(NULL);
    hl_define_planned(environment, "scoped",
        hl_string(environment, "scope-value"), HL_STORAGE_SCOPE);
    HlBinding *scoped = hl_find(environment, "scoped");
    if (!scoped || scoped->string_storage != NULL) return 10;
    if (strcmp(scoped->value.as.string, "scope-value") != 0) return 11;

    hl_define_planned(environment, "owned",
        hl_string(environment, "owned-value"), HL_STORAGE_OWNED);
    HlBinding *owned = hl_find(environment, "owned");
    if (!owned || !owned->string_storage || owned->string_capacity == 0) return 12;
    char *storage = owned->string_storage;
    for (int i = 0; i < 1000; ++i) {
        char value[32];
        snprintf(value, sizeof(value), "owned-%d", i);
        hl_assign_planned(environment, "owned",
            hl_string(environment, value), HL_STORAGE_OWNED);
        if (owned->string_storage != storage) return 13;
    }
    if (strcmp(owned->value.as.string, "owned-999") != 0) return 14;
    hl_env_free(environment);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C runtime test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O2")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C runtime test");
        assert!(
            build.status.success(),
            "C runtime test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable).output().expect("run C runtime test");
        assert!(
            run.status.success(),
            "planned storage test failed with code {:?}",
            run.status.code()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_owned_binding_survives_assignment_from_child_scope() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("owned_child_scope.c");
        let executable = directory.join("owned_child_scope");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *outer = hl_env_new(NULL);
    hl_define_planned(outer, "result",
        hl_string(outer, "initial"), HL_STORAGE_OWNED);
    HlBinding *binding = hl_find(outer, "result");
    if (!binding || !binding->string_storage) return 10;

    HlEnv *inner = hl_env_new(outer);
    hl_assign_planned(inner, "result",
        hl_string(inner, "created-in-child"), HL_STORAGE_OWNED);
    if (strcmp(hl_get(inner, "result").as.string, "created-in-child") != 0)
        return 11;

    hl_env_free(inner);
    if (strcmp(hl_get(outer, "result").as.string, "created-in-child") != 0)
        return 12;
    if (binding->owner != outer || !binding->string_storage) return 13;

    hl_env_free(outer);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C runtime test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O2")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C runtime test");
        assert!(
            build.status.success(),
            "C runtime test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable).output().expect("run C runtime test");
        assert!(
            run.status.success(),
            "child-scope ownership test failed with code {:?}",
            run.status.code()
        );
        let _ = fs::remove_dir_all(directory);
    }


    #[test]
    fn c_runtime_ownership_tokens_track_transfer_drop_and_callback_invalidation() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("ownership_tokens.c");
        let executable = directory.join("ownership_tokens");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
typedef struct {
    HlEnv *environment;
    uint64_t token;
    int *calls;
    int saw_inactive;
} OwnershipProbe;
static void ownership_cleanup(void *context) {
    OwnershipProbe *probe = (OwnershipProbe *)context;
    *probe->calls += 1;
    probe->saw_inactive = !hl_ownership_is_active(probe->environment, probe->token);
}
int main(void) {
    int calls = 0;
    uint64_t token = 0, repeated_token = 0, cleanup_token = 0;
    HlEnv *root = hl_env_new(NULL);
    HlEnv *child = hl_env_new(root);
    OwnershipProbe probe = { child, 0, &calls, 0 };
    if (!hl_ownership_acquire(child, 41, &token, ownership_cleanup, &probe)) return 10;
    probe.token = token;
    if (!hl_ownership_is_active(child, token)) return 11;
    if (!hl_ownership_transfer(child, root, token)) return 12;
    if (hl_ownership_transfer(child, root, token)) return 13;
    if (hl_ownership_drop(child, token)) return 14;
    if (!hl_ownership_is_active(root, token)) return 15;
    if (!hl_ownership_drop(root, token)) return 16;
    if (hl_ownership_drop(root, token)) return 17;
    if (calls != 0 || hl_ownership_is_active(root, token)) return 18;

    /* The same static compiler site can execute repeatedly (loop iterations
       or recursion), while each dynamic resource receives a fresh handle. */
    OwnershipProbe repeated = { child, 0, &calls, 0 };
    if (!hl_ownership_acquire(child, 41, &repeated_token, ownership_cleanup, &repeated)) return 19;
    repeated.token = repeated_token;
    if (repeated_token == token || !hl_ownership_is_active(child, repeated_token)) return 20;
    if (hl_ownership_is_active(child, token) || !hl_ownership_is_active(child, repeated_token)) return 21;

    OwnershipProbe cleanup_probe = { child, 0, &calls, 0 };
    if (!hl_ownership_acquire(child, 42, &cleanup_token, ownership_cleanup, &cleanup_probe)) return 22;
    cleanup_probe.token = cleanup_token;
    hl_env_free(child);
    if (calls != 2 || !cleanup_probe.saw_inactive || !repeated.saw_inactive) return 23;
    if (hl_ownership_is_active(root, cleanup_token)) return 24;
    hl_env_free(root);
    if (calls != 2) return 25;
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C ownership token test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .arg("-fsanitize=address,undefined")
            .arg("-fno-omit-frame-pointer")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C ownership token test with sanitizers");
        assert!(build.status.success(), "sanitized ownership token compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run C ownership token test");
        assert!(run.status.success(), "ownership token test failed with code {:?}: {}",
            run.status.code(), String::from_utf8_lossy(&run.stderr));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_cleanup_callbacks_cannot_resurrect_closing_environments() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("cleanup_reentrancy.c");
        let executable = directory.join("cleanup_reentrancy");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
typedef struct {
    HlEnv *environment;
    int called;
    int rejected_registration;
} ReentrantCleanupProbe;
static void noop_cleanup(void *context) { (void)context; }
static void reentrant_cleanup(void *context) {
    ReentrantCleanupProbe *probe = (ReentrantCleanupProbe *)context;
    probe->called += 1;
    if (!hl_env_defer(probe->environment, noop_cleanup, NULL))
        probe->rejected_registration += 1;
    /* Closing an environment from its own cleanup callback must be a no-op. */
    hl_env_free(probe->environment);
}
int main(void) {
    HlEnv *environment = hl_env_new(NULL);
    ReentrantCleanupProbe probe = { environment, 0, 0 };
    if (!hl_env_defer(environment, reentrant_cleanup, &probe)) return 10;
    hl_env_free(environment);
    if (probe.called != 1) return 11;
    if (probe.rejected_registration != 1) return 12;
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C runtime test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .arg("-fsanitize=address,undefined")
            .arg("-fno-omit-frame-pointer")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile reentrant cleanup test with sanitizers");
        assert!(
            build.status.success(),
            "sanitized reentrant cleanup test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable).output().expect("run sanitized reentrant cleanup test");
        assert!(
            run.status.success(),
            "cleanup reentrancy test failed with code {:?}",
            run.status.code()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_frame_slots_are_activation_local_and_checked() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("frame_runtime.c");
        let executable = directory.join("frame_runtime");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlFrameSlot outer_slots[2];
    HlFrameSlot inner_slots[2];
    HlFrame outer, inner;
    HlValue value;
    if (!hl_frame_init(&outer, outer_slots, 2)) return 10;
    if (!hl_frame_init(&inner, inner_slots, 2)) return 11;
    if (hl_frame_read(&outer, 0, &value)) return 12;
    if (!hl_frame_write(&outer, 0, hl_number(42.0))) return 13;
    if (!hl_frame_read(&outer, 0, &value) || value.as.number != 42.0) return 14;
    if (!hl_frame_write(&inner, 0, hl_number(7.0))) return 15;
    if (!hl_frame_read(&inner, 0, &value) || value.as.number != 7.0) return 16;
    if (!hl_frame_read(&outer, 0, &value) || value.as.number != 42.0) return 17;
    if (hl_frame_write(&outer, 2, hl_number(0.0))) return 18;
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C frame test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler).arg("-std=c11").arg("-O2")
            .arg(&c_file).arg("-o").arg(&executable).arg("-lm")
            .output().expect("compile C frame test");
        assert!(build.status.success(), "frame test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run C frame test");
        assert!(run.status.success(), "frame test failed: {:?}", run.status.code());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_reuses_sibling_lane_storage_and_isolates_nested_lanes() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("arena_lanes.c");
        let executable = directory.join("arena_lanes");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *root = hl_env_new(NULL);
    HlArenaLane sibling_lane = {0};
    HlArenaLane nested_lane = {0};
    HlEnv *first = hl_env_new_in_lane(root, &sibling_lane);
    void *first_address = hl_copy_string(first, "first");
    hl_env_free(first);
    HlEnv *second = hl_env_new_in_lane(root, &sibling_lane);
    char *second_value = hl_copy_string(second, "second");
    if ((void *)second_value != first_address) return 10;
    if (strcmp(second_value, "second") != 0) return 11;
    HlEnv *nested = hl_env_new_in_lane(second, &nested_lane);
    char *nested_value = hl_copy_string(nested, "nested");
    if ((void *)nested_value == (void *)second_value) return 12;
    if (strcmp(second_value, "second") != 0) return 13;
    hl_env_free(nested);
    if (strcmp(second_value, "second") != 0) return 14;
    hl_env_free(second);
    hl_lane_destroy(&sibling_lane);
    hl_lane_destroy(&nested_lane);
    hl_env_free(root);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C lane test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler).arg("-std=c11").arg("-O2")
            .arg(&c_file).arg("-o").arg(&executable).arg("-lm")
            .output().expect("compile C lane test");
        assert!(build.status.success(), "lane test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run C lane test");
        assert!(run.status.success(), "lane test failed: {:?}, stderr: {}",
            run.status.code(), String::from_utf8_lossy(&run.stderr));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_bump_arena_grows_aligns_and_releases_by_scope() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("arena_runtime.c");
        let executable = directory.join("arena_runtime");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *outer = hl_env_new(NULL);
    HlEnv *inner = hl_env_new(outer);
    unsigned char *outer_value = (unsigned char *)hl_alloc(outer, 32);
    outer_value[0] = 0x5a;

    for (int i = 0; i < 24; ++i) {
        void *item = hl_alloc(inner, 512);
        if (((uintptr_t)item % _Alignof(max_align_t)) != 0) return 10;
        memset(item, i, 512);
    }
    if (!inner->lane || !inner->lane->blocks || !inner->lane->blocks->next) return 11;
    if (outer_value[0] != 0x5a) return 12;

    hl_env_free(inner);
    if (outer_value[0] != 0x5a) return 13;
    hl_env_free(outer);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C arena test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O2")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C arena test");
        assert!(
            build.status.success(),
            "C arena test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable).output().expect("run C arena test");
        assert!(
            run.status.success(),
            "C arena growth/alignment/scope test failed with code {:?}",
            run.status.code()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_reuses_binding_string_storage_across_reassignment_and_self_assignment() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("binding_string_storage.c");
        let executable = directory.join("binding_string_storage");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *outer = hl_env_new(NULL);
    hl_define(outer, "value", hl_string(outer, "initial"));
    HlBinding *binding = hl_find(outer, "value");
    if (!binding || !binding->string_storage) return 10;

    char *storage = NULL;
    size_t capacity = 0;
    for (int i = 0; i < 10000; ++i) {
        char text[64];
        snprintf(text, sizeof(text), "value-%08d", i);
        HlEnv *iteration = hl_env_new(outer);
        HlValue next = hl_string(iteration, text);
        hl_assign(iteration, "value", next);
        hl_env_free(iteration);

        if (strcmp(hl_get(outer, "value").as.string, text) != 0) return 11;
        if (i == 0) {
            storage = binding->string_storage;
            capacity = binding->string_capacity;
        } else if (binding->string_storage != storage || binding->string_capacity != capacity) {
            return 12;
        }
    }

    HlValue current = hl_get(outer, "value");
    hl_assign(outer, "value", current);
    if (strcmp(hl_get(outer, "value").as.string, "value-00009999") != 0) return 13;
    if (binding->string_storage != storage || binding->string_capacity != capacity) return 14;

    hl_assign(outer, "value", hl_string(outer, "short"));
    if (strcmp(hl_get(outer, "value").as.string, "short") != 0) return 15;
    if (binding->string_storage != storage || binding->string_capacity != capacity) return 16;

    hl_env_free(outer);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write binding string storage test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .arg("-fsanitize=address,undefined")
            .arg("-fno-omit-frame-pointer")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile binding string storage test with sanitizers");
        assert!(build.status.success(), "sanitized string storage test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run sanitized binding string storage test");
        assert!(run.status.success(), "sanitized string storage test failed: {:?}", run.status.code());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_failure_rewinds_active_physical_lanes() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("lane_failure.c");
        let executable = directory.join("lane_failure");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
static HlArenaLane failed_outer_lane = {0};
static HlArenaLane failed_inner_lane = {0};
static void verify_failed_lane_rewind(void) {
    int outer_ok = failed_outer_lane.blocks
        && failed_outer_lane.current == failed_outer_lane.blocks
        && failed_outer_lane.blocks->used == 0;
    int inner_ok = failed_inner_lane.blocks
        && failed_inner_lane.current == failed_inner_lane.blocks
        && failed_inner_lane.blocks->used == 0;
    _Exit(outer_ok && inner_ok ? 0 : 90);
}
int main(void) {
    atexit(verify_failed_lane_rewind);
    HlEnv *root = hl_env_new(NULL);
    HlEnv *outer = hl_env_new_in_lane(root, &failed_outer_lane);
    (void)hl_copy_string(outer, "outer");
    HlEnv *inner = hl_env_new_in_lane(outer, &failed_inner_lane);
    (void)hl_copy_string(inner, "inner");
    hl_fail(inner, "forced physical lane failure");
    return 99;
}
"#
        );
        fs::write(&c_file, source).expect("write lane failure test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler).arg("-std=c11").arg("-O2")
            .arg(&c_file).arg("-o").arg(&executable).arg("-lm")
            .output().expect("compile lane failure test");
        assert!(build.status.success(), "lane failure test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run lane failure test");
        assert!(run.status.success(), "runtime error did not rewind active lanes: {:?}, stderr: {}",
            run.status.code(), String::from_utf8_lossy(&run.stderr));
        assert!(String::from_utf8_lossy(&run.stderr).contains("forced physical lane failure"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn c_runtime_failure_cleans_the_entire_active_environment_chain() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("cleanup_failure.c");
        let executable = directory.join("cleanup_failure");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
static int cleanup_order = 0;
static int outer_digit = 1;
static int inner_digit = 2;
static void record_failure_cleanup(void *context) {
    cleanup_order = cleanup_order * 10 + *(int *)context;
}
static void verify_failure_cleanup(void) {
    _Exit(cleanup_order == 21 ? 0 : 90);
}
int main(void) {
    atexit(verify_failure_cleanup);
    HlEnv *outer = hl_env_new(NULL);
    HlEnv *inner = hl_env_new(outer);
    if (!hl_env_defer(outer, record_failure_cleanup, &outer_digit)) return 10;
    if (!hl_env_defer(inner, record_failure_cleanup, &inner_digit)) return 11;
    hl_fail(inner, "forced test failure");
    return 99;
}
"#
        );
        fs::write(&c_file, source).expect("write C runtime failure test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O2")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C runtime failure test");
        assert!(
            build.status.success(),
            "C runtime failure test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable).output().expect("run C runtime failure test");
        assert!(
            run.status.success(),
            "runtime error skipped cleanup or ran it in the wrong order; stderr: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(
            String::from_utf8_lossy(&run.stderr).contains("forced test failure"),
            "runtime failure diagnostic was not emitted"
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn compiles_and_runs_a_native_program_with_variables_conditions_and_loops() {
        let program = Program {
            statements: vec![
                Statement::Let(LetStatement {
                    name: "counter".into(),
                    value: Expression::NumberLiteral(0.0),
                }),
                Statement::While(WhileStatement {
                    condition: Expression::Infix {
                        left: Box::new(Expression::Identifier("counter".into())),
                        operator: crate::lexer::Token::Less,
                        right: Box::new(Expression::NumberLiteral(3.0)),
                    },
                    body: vec![Statement::Assign(AssignStatement {
                        name: "counter".into(),
                        value: Expression::Infix {
                            left: Box::new(Expression::Identifier("counter".into())),
                            operator: crate::lexer::Token::Plus,
                            right: Box::new(Expression::NumberLiteral(1.0)),
                        },
                    })],
                }),
                Statement::If(IfStatement {
                    condition: Expression::Infix {
                        left: Box::new(Expression::Identifier("counter".into())),
                        operator: crate::lexer::Token::Greater,
                        right: Box::new(Expression::NumberLiteral(2.0)),
                    },
                    true_body: vec![Statement::Print(PrintStatement {
                        value: Expression::Identifier("counter".into()),
                    })],
                    false_body: Some(vec![Statement::Print(PrintStatement {
                        value: Expression::NumberLiteral(0.0),
                    })]),
                }),
            ],
        };

        let directory = temporary_directory().expect("temporary directory");
        let source = directory.join("program.hl");
        let executable = directory.join("program");
        fs::write(&source, "native backend test").unwrap();
        let build = compile_to_executable(&program, &source, Some(&executable));
        assert!(build.is_ok(), "native compilation failed: {build:?}");
        let run = Command::new(&executable).output().expect("run native executable");
        assert!(run.status.success());
        assert_eq!(String::from_utf8_lossy(&run.stdout), "3\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn escaped_string_assignment_survives_inner_region_release() {
        let program = Program {
            statements: vec![
                Statement::Let(LetStatement { name: "outer".into(), value: Expression::StringLiteral("initial".into()) }),
                Statement::Block(vec![
                    Statement::Let(LetStatement { name: "inner".into(), value: Expression::StringLiteral("promoted".into()) }),
                    Statement::Assign(AssignStatement { name: "outer".into(), value: Expression::Identifier("inner".into()) }),
                ]),
                Statement::Print(PrintStatement { value: Expression::Identifier("outer".into()) }),
            ],
        };
        let directory = temporary_directory().expect("temporary directory");
        let source = directory.join("escape.hl");
        let executable = directory.join("escape");
        fs::write(&source, "native region escape test").unwrap();
        compile_to_executable(&program, &source, Some(&executable)).expect("native compilation");
        let run = Command::new(&executable).output().expect("run native executable");
        assert!(run.status.success());
        assert_eq!(String::from_utf8_lossy(&run.stdout), "promoted\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn loop_iteration_regions_preserve_values_until_print_and_release_each_iteration() {
        let program = Program {
            statements: vec![
                Statement::Let(LetStatement { name: "i".into(), value: Expression::NumberLiteral(0.0) }),
                Statement::While(WhileStatement {
                    condition: Expression::Infix {
                        left: Box::new(Expression::Identifier("i".into())),
                        operator: crate::lexer::Token::Less,
                        right: Box::new(Expression::NumberLiteral(3.0)),
                    },
                    body: vec![
                        Statement::Let(LetStatement { name: "local".into(), value: Expression::StringLiteral("iteration".into()) }),
                        Statement::Print(PrintStatement { value: Expression::Identifier("local".into()) }),
                        Statement::Assign(AssignStatement {
                            name: "i".into(),
                            value: Expression::Infix {
                                left: Box::new(Expression::Identifier("i".into())),
                                operator: crate::lexer::Token::Plus,
                                right: Box::new(Expression::NumberLiteral(1.0)),
                            },
                        }),
                    ],
                }),
            ],
        };
        let directory = temporary_directory().expect("temporary directory");
        let source = directory.join("loop_regions.hl");
        let executable = directory.join("loop_regions");
        fs::write(&source, "native loop region test").unwrap();
        compile_to_executable(&program, &source, Some(&executable)).expect("native compilation");
        let run = Command::new(&executable).output().expect("run native executable");
        assert!(run.status.success());
        assert_eq!(String::from_utf8_lossy(&run.stdout), "iteration\niteration\niteration\n");
        let _ = fs::remove_dir_all(directory);
    }


    #[test]
    fn nested_continue_and_break_release_regions_and_preserve_outer_string() {
        let program = Program {
            statements: vec![
                Statement::Let(LetStatement { name: "i".into(), value: Expression::NumberLiteral(0.0) }),
                Statement::Let(LetStatement { name: "result".into(), value: Expression::StringLiteral("initial".into()) }),
                Statement::While(WhileStatement {
                    condition: Expression::Infix {
                        left: Box::new(Expression::Identifier("i".into())),
                        operator: crate::lexer::Token::Less,
                        right: Box::new(Expression::NumberLiteral(5.0)),
                    },
                    body: vec![
                        Statement::Assign(AssignStatement {
                            name: "i".into(),
                            value: Expression::Infix {
                                left: Box::new(Expression::Identifier("i".into())),
                                operator: crate::lexer::Token::Plus,
                                right: Box::new(Expression::NumberLiteral(1.0)),
                            },
                        }),
                        Statement::Block(vec![
                            Statement::Let(LetStatement {
                                name: "temporary".into(),
                                value: Expression::StringLiteral("iteration".into()),
                            }),
                            Statement::If(IfStatement {
                                condition: Expression::Infix {
                                    left: Box::new(Expression::Identifier("i".into())),
                                    operator: crate::lexer::Token::Tilde,
                                    right: Box::new(Expression::NumberLiteral(2.0)),
                                },
                                true_body: vec![Statement::Continue],
                                false_body: None,
                            }),
                            Statement::If(IfStatement {
                                condition: Expression::Infix {
                                    left: Box::new(Expression::Identifier("i".into())),
                                    operator: crate::lexer::Token::Tilde,
                                    right: Box::new(Expression::NumberLiteral(4.0)),
                                },
                                true_body: vec![Statement::Break],
                                false_body: None,
                            }),
                            Statement::Assign(AssignStatement {
                                name: "result".into(),
                                value: Expression::Identifier("temporary".into()),
                            }),
                        ]),
                    ],
                }),
                Statement::Print(PrintStatement { value: Expression::Identifier("result".into()) }),
                Statement::Print(PrintStatement { value: Expression::Identifier("i".into()) }),
            ],
        };
        let directory = temporary_directory().expect("temporary directory");
        let source = directory.join("nested_control.hl");
        let executable = directory.join("nested_control");
        fs::write(&source, "nested region control-flow test").unwrap();
        compile_to_executable(&program, &source, Some(&executable)).expect("native compilation");
        let run = Command::new(&executable).output().expect("run native executable");
        assert!(run.status.success(), "native program failed: {}", String::from_utf8_lossy(&run.stderr));
        assert_eq!(String::from_utf8_lossy(&run.stdout), "iteration\n4\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn rejects_unsupported_closures_during_ast_to_hir_lowering() {
        let program = Program {
            statements: vec![Statement::Let(LetStatement {
                name: "identity".into(),
                value: Expression::Lambda {
                    params: vec!["x".into()],
                    body: Box::new(Expression::Identifier("x".into())),
                },
            })],
        };
        let error = crate::hir::lower(&program).unwrap_err();
        assert!(error.message.contains("closures"));
    }

    #[test]
    fn promoted_string_copy_survives_source_region_release() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("promoted_string_copy.c");
        let executable = directory.join("promoted_string_copy");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *outer = hl_env_new(NULL);
    hl_define_planned_site(outer, "destination", hl_string(outer, "initial"), HL_STORAGE_OWNED, UINT64_C(1));
    HlEnv *inner = hl_env_new(outer);
    hl_define_planned(inner, "source", hl_string(inner, "survives-region"), HL_STORAGE_SCOPE);

    hl_assign_planned_site(outer, "destination", hl_get(inner, "source"), HL_STORAGE_OWNED, UINT64_C(1));
    if (strcmp(hl_get(outer, "destination").as.string, "survives-region") != 0) return 1;
    hl_env_free(inner);

    if (strcmp(hl_get(outer, "destination").as.string, "survives-region") != 0) return 2;
    hl_assign_planned(outer, "destination", hl_string(outer, "replacement"), HL_STORAGE_OWNED);
    if (strcmp(hl_get(outer, "destination").as.string, "replacement") != 0) return 3;
    hl_env_free(outer);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write promoted string copy test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .arg("-fsanitize=address,undefined")
            .arg("-fno-omit-frame-pointer")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile promoted string copy test with sanitizers");
        assert!(build.status.success(), "sanitized promotion test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run sanitized promotion test");
        assert!(run.status.success(), "promotion test failed: {}",
            String::from_utf8_lossy(&run.stderr));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn owned_binding_strings_register_one_environment_cleanup() {
        let directory = temporary_directory().expect("temporary directory");
        let c_file = directory.join("owned_binding_cleanup.c");
        let executable = directory.join("owned_binding_cleanup");
        let source = format!(
            "{}\n{}",
            include_str!("native_runtime.c"),
            r#"
int main(void) {
    HlEnv *env = hl_env_new(NULL);
    hl_define_planned_site(env, "value", hl_string(env, "first"), HL_STORAGE_OWNED, UINT64_C(77));
    HlBinding *binding = hl_find(env, "value");
    if (!binding || !binding->string_storage || !binding->string_cleanup) return 1;
    if (binding->ownership_site != 77 || !binding->ownership_token
        || !hl_ownership_is_active(env, binding->ownership_token)) return 7;
    HlOwnershipRecord *record = hl_ownership_find(env, binding->ownership_token);
    if (!record || record->site_id != 77 || record->allocation != binding->string_cleanup) return 8;
    HlAllocation *registration = binding->string_cleanup;

    for (int i = 0; i < 1000; ++i) {
        char value[32];
        snprintf(value, sizeof(value), "value-%d", i);
        hl_assign_planned(env, "value", hl_string(env, value), HL_STORAGE_OWNED);
        if (binding->string_cleanup != registration) return 2;
    }

    size_t registrations = 0;
    for (HlAllocation *entry = env->allocations; entry; entry = entry->next)
        ++registrations;
    if (registrations != 1) return 3;

    hl_assign_planned(env, "value", hl_number(7.0), HL_STORAGE_OWNED);
    if (binding->string_storage || binding->string_capacity != 0) return 4;
    hl_assign_planned(env, "value", hl_string(env, "scoped"), HL_STORAGE_SCOPE);
    if (binding->string_storage || binding->string_cleanup != registration) return 5;
    hl_assign_planned(env, "value", hl_string(env, "again"), HL_STORAGE_OWNED);
    if (!binding->string_storage || binding->string_cleanup != registration) return 6;

    hl_env_free(env);
    return 0;
}
"#
        );
        fs::write(&c_file, source).expect("write C runtime test");
        let compiler = select_c_compiler().expect("C compiler");
        let build = Command::new(&compiler)
            .arg("-std=c11")
            .arg("-O1")
            .arg("-g")
            .arg("-fsanitize=address,undefined")
            .arg("-fno-omit-frame-pointer")
            .arg(&c_file)
            .arg("-o")
            .arg(&executable)
            .arg("-lm")
            .output()
            .expect("compile C runtime test with sanitizers");
        assert!(build.status.success(), "sanitized C runtime test compilation failed: {}",
            String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&executable).output().expect("run sanitized C runtime test");
        assert!(run.status.success(), "C runtime test failed: {}",
            String::from_utf8_lossy(&run.stderr));
        let _ = fs::remove_dir_all(directory);
    }

}
