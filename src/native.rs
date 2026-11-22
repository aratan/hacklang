use crate::ast::{Expression, Program, Statement};
use crate::lexer::Token;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_BUILD_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Generator {
    next_temp: usize,
    next_scope: usize,
    next_loop: usize,
    scopes: Vec<String>,
    loops: Vec<LoopTarget>,
}

struct LoopTarget {
    continue_label: String,
    break_label: String,
    scope_depth: usize,
}

impl Generator {
    fn temp(&mut self) -> String {
        let name = format!("value_{}", self.next_temp);
        self.next_temp += 1;
        name
    }

    fn line(out: &mut String, text: impl AsRef<str>) {
        out.push_str(text.as_ref());
        out.push('\n');
    }

    fn c_string(value: &str) -> String {
        let mut output = String::from("\"");
        for byte in value.as_bytes() {
            match *byte {
                b'"' => output.push_str("\\\""),
                b'\\' => output.push_str("\\\\"),
                b'\n' => output.push_str("\\n"),
                b'\r' => output.push_str("\\r"),
                b'\t' => output.push_str("\\t"),
                0x20..=0x7e => output.push(*byte as char),
                byte => output.push_str(&format!("\\{:03o}", byte)),
            }
        }
        output.push('"');
        output
    }

    fn expression(&mut self, expression: &Expression, out: &mut String) -> Result<String, String> {
        match expression {
            Expression::NumberLiteral(number) => {
                if !number.is_finite() {
                    return Err("El backend nativo inicial no admite NaN ni infinito como literales".into());
                }
                let result = self.temp();
                Self::line(out, format!("HlValue {result} = hl_number({number:.17});"));
                Ok(result)
            }
            Expression::BoolLiteral(value) => {
                let result = self.temp();
                Self::line(out, format!("HlValue {result} = hl_bool({});", if *value { 1 } else { 0 }));
                Ok(result)
            }
            Expression::StringLiteral(value) => {
                let result = self.temp();
                Self::line(out, format!("HlValue {result} = hl_string({});", Self::c_string(value)));
                Ok(result)
            }
            Expression::Identifier(name) => {
                let result = self.temp();
                Self::line(out, format!("HlValue {result} = hl_get(env, {});", Self::c_string(name)));
                Ok(result)
            }
            Expression::Infix { left, operator, right } => {
                let left_value = self.expression(left, out)?;
                let right_value = self.expression(right, out)?;
                let function = match operator {
                    Token::Plus => "hl_add",
                    Token::Minus => "hl_sub",
                    Token::Star => "hl_mul",
                    Token::Slash => "hl_div",
                    Token::Less => "hl_less",
                    Token::Greater => "hl_greater",
                    Token::Tilde => "hl_equal",
                    Token::Caret => "hl_not_equal",
                    other => return Err(format!("Operador no soportado por el backend nativo: {other:?}")),
                };
                let result = self.temp();
                Self::line(out, format!("HlValue {result} = {function}({left_value}, {right_value});"));
                Ok(result)
            }
            Expression::Ternary { condition, true_branch, false_branch } => {
                let condition_value = self.expression(condition, out)?;
                let result = self.temp();
                Self::line(out, format!("HlValue {result};"));
                Self::line(out, format!("if (hl_truthy({condition_value})) {{"));
                let true_value = self.expression(true_branch, out)?;
                Self::line(out, format!("{result} = {true_value};"));
                Self::line(out, "} else {");
                let false_value = self.expression(false_branch, out)?;
                Self::line(out, format!("{result} = {false_value};"));
                Self::line(out, "}");
                Ok(result)
            }
            Expression::Lambda { .. } => Err("Las funciones y closures todavía no están soportadas por el backend nativo".into()),
            Expression::Block(_) => Err("Los bloques como expresión requieren funciones; todavía no están soportados por el backend nativo".into()),
            Expression::FunctionCall { .. } => Err("Las llamadas a funciones y builtins todavía no están soportadas por el backend nativo".into()),
            Expression::Array(_) => Err("Los arrays todavía no están soportados por el backend nativo".into()),
            Expression::Map(_) => Err("Los mapas todavía no están soportados por el backend nativo".into()),
            Expression::Index { .. } => Err("La indexación todavía no está soportada por el backend nativo".into()),
        }
    }

    fn scoped_statements(&mut self, statements: &[Statement], out: &mut String) -> Result<(), String> {
        let id = self.next_scope;
        self.next_scope += 1;
        let previous = format!("previous_scope_{id}");
        Self::line(out, "{");
        Self::line(out, format!("HlEnv *{previous} = env;"));
        Self::line(out, "env = hl_env_new(env);");
        self.scopes.push(previous.clone());
        for statement in statements {
            self.statement(statement, out)?;
        }
        self.scopes.pop();
        Self::line(out, format!("env = {previous};"));
        Self::line(out, "}");
        Ok(())
    }

    fn restore_scopes_for_jump(&self, out: &mut String, target_depth: usize) {
        for previous in self.scopes[target_depth..].iter().rev() {
            Self::line(out, format!("env = {previous};"));
        }
    }

    fn statement(&mut self, statement: &Statement, out: &mut String) -> Result<(), String> {
        match statement {
            Statement::Let(binding) => {
                let value = self.expression(&binding.value, out)?;
                Self::line(out, format!("hl_define(env, {}, {value});", Self::c_string(&binding.name)));
            }
            Statement::Assign(binding) => {
                let value = self.expression(&binding.value, out)?;
                Self::line(out, format!("hl_assign(env, {}, {value});", Self::c_string(&binding.name)));
            }
            Statement::Print(print) => {
                let value = self.expression(&print.value, out)?;
                Self::line(out, format!("hl_print({value});"));
            }
            Statement::Block(statements) => self.scoped_statements(statements, out)?,
            Statement::If(branch) => {
                let condition = self.expression(&branch.condition, out)?;
                Self::line(out, format!("if (hl_truthy({condition}))"));
                self.scoped_statements(&branch.true_body, out)?;
                if let Some(otherwise) = &branch.false_body {
                    Self::line(out, "else");
                    self.scoped_statements(otherwise, out)?;
                }
            }
            Statement::While(loop_statement) => {
                let id = self.next_loop;
                self.next_loop += 1;
                let previous = format!("previous_loop_scope_{id}");
                let continue_label = format!("while_condition_{id}");
                let break_label = format!("while_end_{id}");
                Self::line(out, format!("HlEnv *{previous} = env;"));
                Self::line(out, "env = hl_env_new(env);");
                self.scopes.push(previous.clone());
                let target_depth = self.scopes.len();
                self.loops.push(LoopTarget {
                    continue_label: continue_label.clone(),
                    break_label: break_label.clone(),
                    scope_depth: target_depth,
                });
                Self::line(out, format!("{continue_label}: ;"));
                let condition = self.expression(&loop_statement.condition, out)?;
                Self::line(out, format!("if (!hl_truthy({condition})) goto {break_label};"));
                for child in &loop_statement.body {
                    self.statement(child, out)?;
                }
                Self::line(out, format!("goto {continue_label};"));
                Self::line(out, format!("{break_label}: ;"));
                self.loops.pop();
                self.scopes.pop();
                Self::line(out, format!("env = {previous};"));
            }
            Statement::Break => {
                let target = self.loops.last().ok_or("break fuera de un bucle")?;
                self.restore_scopes_for_jump(out, target.scope_depth);
                Self::line(out, format!("goto {};", target.break_label));
            }
            Statement::Continue => {
                let target = self.loops.last().ok_or("continue fuera de un bucle")?;
                self.restore_scopes_for_jump(out, target.scope_depth);
                Self::line(out, format!("goto {};", target.continue_label));
            }
            Statement::Return(_) => return Err("return todavía no está soportado fuera de funciones; las funciones se incorporarán en una fase posterior".into()),
            Statement::IndexAssign(_) => return Err("La asignación por índice todavía no está soportada por el backend nativo".into()),
        }
        Ok(())
    }
}

fn generate_c(program: &Program) -> Result<String, String> {
    let mut body = String::new();
    let mut generator = Generator::default();
    for statement in &program.statements {
        generator.statement(statement, &mut body)?;
    }

    let mut source = include_str!("native_runtime.c").to_string();
    source.push_str("\nint main(void) {\n");
    source.push_str("    HlEnv *env = hl_env_new(NULL);\n");
    source.push_str(&body);
    source.push_str("    return 0;\n}\n");
    Ok(source)
}

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
    // HIR valida y normaliza el subconjunto nativo antes de generar C.
    // Durante esta migración, el generador C sigue usando el AST como referencia.
    let _hir = crate::hir::lower(program).map_err(|error| format!("No se pudo construir HIR: {error}"))?;
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

    let c_source = crate::hir_native::generate_c(&_hir)?;
    let compiler = select_c_compiler()?;
    let temporary = temporary_directory()?;
    let c_file = temporary.join("program.c");
    let write_result = fs::write(&c_file, c_source)
        .map_err(|error| format!("No se pudo escribir el C temporal: {error}"));
    if let Err(error) = write_result {
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
    use crate::ast::{LetStatement, PrintStatement};

    #[test]
    fn generates_native_c_for_literals_variables_and_arithmetic() {
        let program = Program {
            statements: vec![
                Statement::Let(LetStatement {
                    name: "answer".into(),
                    value: Expression::Infix {
                        left: Box::new(Expression::NumberLiteral(40.0)),
                        operator: Token::Plus,
                        right: Box::new(Expression::NumberLiteral(2.0)),
                    },
                }),
                Statement::Print(PrintStatement { value: Expression::Identifier("answer".into()) }),
            ],
        };
        let generated = generate_c(&program).unwrap();
        assert!(generated.contains("hl_add("));
        assert!(generated.contains("hl_define(env, \"answer\""));
        assert!(generated.contains("hl_print("));
    }

    #[test]
    fn compiles_and_runs_a_native_program_with_variables_conditions_and_loops() {
        use crate::ast::{AssignStatement, IfStatement, LetStatement, PrintStatement, WhileStatement};

        let program = Program {
            statements: vec![
                Statement::Let(LetStatement {
                    name: "counter".into(),
                    value: Expression::NumberLiteral(0.0),
                }),
                Statement::While(WhileStatement {
                    condition: Expression::Infix {
                        left: Box::new(Expression::Identifier("counter".into())),
                        operator: Token::Less,
                        right: Box::new(Expression::NumberLiteral(3.0)),
                    },
                    body: vec![Statement::Assign(AssignStatement {
                        name: "counter".into(),
                        value: Expression::Infix {
                            left: Box::new(Expression::Identifier("counter".into())),
                            operator: Token::Plus,
                            right: Box::new(Expression::NumberLiteral(1.0)),
                        },
                    })],
                }),
                Statement::If(IfStatement {
                    condition: Expression::Infix {
                        left: Box::new(Expression::Identifier("counter".into())),
                        operator: Token::Greater,
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
    fn reports_unsupported_closures_instead_of_generating_wrong_code() {
        let program = Program {
            statements: vec![Statement::Let(LetStatement {
                name: "identity".into(),
                value: Expression::Lambda {
                    params: vec!["x".into()],
                    body: Box::new(Expression::Identifier("x".into())),
                },
            })],
        };
        let error = generate_c(&program).unwrap_err();
        assert!(error.contains("closures"));
    }
}
