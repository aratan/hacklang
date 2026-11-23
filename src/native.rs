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
    let _memory_plan = crate::memory::plan(&hir);
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

    let c_source = crate::hir_native::generate_c(&hir)?;
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
    HlEnv *outer = hl_env_new(NULL);
    HlEnv *inner = hl_env_new(outer);
    HlAllocation *first = hl_env_defer(inner, record_cleanup, &one);
    HlAllocation *moved = hl_env_defer(inner, record_cleanup, &two);
    HlAllocation *cancelled = hl_env_defer(inner, record_cleanup, &three);
    if (!first || !moved || !cancelled) return 10;
    if (!hl_env_transfer(inner, outer, moved)) return 11;
    if (!hl_env_cancel(inner, cancelled)) return 12;
    if (!hl_env_defer(inner, record_cleanup, &four)) return 13;
    hl_env_free(inner);
    if (order != 41) return 14;
    hl_env_free(outer);
    if (order != 412) return 15;
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
            "C cleanup test failed with code {:?}",
            run.status.code()
        );
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
}
