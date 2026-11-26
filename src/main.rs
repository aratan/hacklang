// src/main.rs

use std::env;
use std::fs;
use std::process::ExitCode;
use std::time::Duration;

// Un único front-end compartido para run/check/build.
mod lexer;
mod diagnostic;
mod ast;
mod hir;
mod capture;
mod memory;
mod region;
mod hir_native;
mod parser;
mod evaluador;
mod type_checker;
mod formatter;
mod native;

#[cfg(test)]
mod frontend_fuzz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Run,
    Check,
    Build,
}

#[derive(Debug, Default)]
struct Cli {
    command: Option<Command>,
    filename: Option<String>,
    output: Option<String>,
    compact: bool,
    format: bool,
    diagnostics_json: bool,
    max_parse_depth: Option<usize>,
    max_eval_depth: Option<usize>,
    max_eval_steps: Option<usize>,
    max_eval_millis: Option<u64>,
}

fn usage() -> &'static str {
    "Uso:
  hl [run] [--compact] [--format] [--diagnostics-json] [--max-parse-depth N] [--max-eval-depth N] [--max-eval-steps N] [--max-eval-millis N] <archivo.hl>
  hl check [--compact] [--diagnostics-json] [--max-parse-depth N] <archivo.hl>
  hl build [--compact] [--max-parse-depth N] [--diagnostics-json] <archivo.hl> [-o <ejecutable>]

La forma legacy 'hl <archivo.hl>' equivale a 'hl run <archivo.hl>'."
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut index = 0;

    if let Some(first) = args.first() {
        cli.command = match first.as_str() {
            "run" => Some(Command::Run),
            "check" => Some(Command::Check),
            "build" => Some(Command::Build),
            _ => None,
        };
        if cli.command.is_some() {
            index = 1;
        }
    }

    while index < args.len() {
        match args[index].as_str() {
            "--compact" => cli.compact = true,
            "--format" => cli.format = true,
            "--diagnostics-json" => cli.diagnostics_json = true,
            "--max-parse-depth" => {
                index += 1;
                let value = args.get(index).ok_or("Falta el valor de --max-parse-depth")?;
                let depth = value.parse::<usize>().map_err(|_| "--max-parse-depth requiere un entero positivo")?;
                if depth == 0 { return Err("--max-parse-depth debe ser mayor que cero".to_string()); }
                cli.max_parse_depth = Some(depth);
            }
            "--max-eval-depth" => {
                index += 1;
                let value = args.get(index).ok_or("Falta el valor de --max-eval-depth")?;
                let depth = value.parse::<usize>().map_err(|_| "--max-eval-depth requiere un entero positivo")?;
                if depth == 0 { return Err("--max-eval-depth debe ser mayor que cero".to_string()); }
                cli.max_eval_depth = Some(depth);
            }
            "--max-eval-steps" => {
                index += 1;
                let value = args.get(index).ok_or("Falta el valor de --max-eval-steps")?;
                let steps = value.parse::<usize>().map_err(|_| "--max-eval-steps requiere un entero positivo")?;
                if steps == 0 { return Err("--max-eval-steps debe ser mayor que cero".to_string()); }
                cli.max_eval_steps = Some(steps);
            }
            "--max-eval-millis" => {
                index += 1;
                let value = args.get(index).ok_or("Falta el valor de --max-eval-millis")?;
                let millis = value.parse::<u64>().map_err(|_| "--max-eval-millis requiere un entero positivo")?;
                if millis == 0 { return Err("--max-eval-millis debe ser mayor que cero".to_string()); }
                cli.max_eval_millis = Some(millis);
            }
            "-o" => {
                index += 1;
                let output = args.get(index).ok_or("Falta la ruta después de -o")?;
                if output.starts_with('-') {
                    return Err("La ruta de salida después de -o no es válida".to_string());
                }
                cli.output = Some(output.clone());
            }
            arg if arg.starts_with('-') => {
                return Err(format!("Opción desconocida: {arg}"));
            }
            filename => {
                if cli.filename.replace(filename.to_string()).is_some() {
                    return Err("Solo se permite un archivo fuente".to_string());
                }
            }
        }
        index += 1;
    }

    if cli.filename.is_none() {
        return Err(usage().to_string());
    }
    if cli.format && cli.command == Some(Command::Check) {
        return Err("--format no se puede combinar con 'check'".to_string());
    }
    if cli.diagnostics_json && cli.format {
        return Err("--diagnostics-json y --format son modos excluyentes".to_string());
    }
    if (cli.max_eval_depth.is_some() || cli.max_eval_steps.is_some() || cli.max_eval_millis.is_some()) && matches!(cli.command, Some(Command::Check | Command::Build)) {
        return Err("--max-eval-depth, --max-eval-steps y --max-eval-millis solo se admiten con 'run'".to_string());
    }
    if cli.output.is_some() && cli.command != Some(Command::Build) {
        return Err("-o solo se admite con 'build'".to_string());
    }
    if cli.command == Some(Command::Build) && cli.format {
        return Err("'build' no admite --format ni --diagnostics-json".to_string());
    }

    Ok(cli)
}

fn run() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    let command = cli.command.unwrap_or(Command::Run);

    let filename = cli.filename.as_deref().expect("parse_args valida filename");
    let source_code = match fs::read_to_string(filename) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: No se pudo leer el archivo '{filename}': {error}");
            return ExitCode::from(1);
        }
    };

    // Front-end único: los tres comandos reutilizan lexer y parser.
    let mut lexer = lexer::Lexer::new(source_code);
    let tokens = match lexer.tokenize_spanned_diagnostic() {
        Ok(tokens) => tokens,
        Err(diagnostic) => {
            if cli.diagnostics_json {
                println!("[{}]", diagnostic.to_json());
            } else {
                eprintln!("Error léxico: {diagnostic}");
            }
            return ExitCode::from(1);
        }
    };

    let max_parse_depth = cli.max_parse_depth.unwrap_or(256);
    let mut parser = if cli.compact {
        parser::Parser::new_compact_spanned_with_max_depth(tokens, max_parse_depth)
    } else {
        parser::Parser::new_spanned_with_max_depth(tokens, max_parse_depth)
    };
    let (ast, source_spans) = match parser.parse_program_with_spans_diagnostic() {
        Ok(parsed) => parsed,
        Err(diagnostic) => {
            if cli.diagnostics_json {
                println!("[{}]", diagnostic.to_json());
            } else {
                eprintln!("Error de análisis sintáctico: {diagnostic}");
            }
            return ExitCode::from(1);
        }
    };

    if command == Command::Build {
        let output = cli.output.as_deref().map(std::path::Path::new);
        match native::compile_to_executable_with_spans_diagnostic(&ast, &source_spans, std::path::Path::new(filename), output) {
            Ok(executable) => {
                println!("Ejecutable nativo generado: {}", executable.display());
                return ExitCode::SUCCESS;
            }
            Err(diagnostic) => {
                if cli.diagnostics_json {
                    println!("[{}]", diagnostic.to_json());
                } else {
                    eprintln!("Error de compilación nativa: {diagnostic}");
                }
                return ExitCode::from(1);
            }
        }
    }

    if cli.format {
        println!("{}", formatter::format_program(&ast));
        return ExitCode::SUCCESS;
    }

    let diagnostics = type_checker::TypeChecker::check_with_spans(&ast, &source_spans);
    if cli.diagnostics_json {
        let items = diagnostics.iter()
            .map(|diagnostic| diagnostic.to_json())
            .collect::<Vec<_>>();
        println!("[{}]", items.join(","));
        return ExitCode::SUCCESS;
    }

    if command == Command::Check {
        if diagnostics.is_empty() {
            println!("Análisis estático: sin incompatibilidades detectadas.");
        } else {
            for diagnostic in &diagnostics {
                println!("{diagnostic}");
            }
            eprintln!("Análisis completado: {} aviso(s).", diagnostics.len());
        }
        return ExitCode::SUCCESS;
    }

    if diagnostics.is_empty() {
        println!("Análisis estático: sin incompatibilidades detectadas.");
    } else {
        eprintln!("Análisis estático: {} aviso(s):", diagnostics.len());
        for diagnostic in &diagnostics {
            eprintln!("  - {diagnostic}");
        }
    }

    println!("--- Ejecutando código Hacklang ---");
    let configured_eval_millis = env::var("HACKLANG_MAX_EVAL_MILLIS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(evaluador::DEFAULT_MAX_EVAL_MILLIS);
    let mut evaluator = evaluador::Evaluator::new_with_resource_limits(
        cli.max_eval_depth.unwrap_or(256),
        cli.max_eval_steps.unwrap_or(evaluador::DEFAULT_MAX_EVAL_STEPS),
        Duration::from_millis(cli.max_eval_millis.unwrap_or(configured_eval_millis)),
    );
    match evaluator.evaluate_with_spans_diagnostic(&ast, &source_spans) {
        Ok(()) => {
            println!("--------------------------------");
            println!("Programa finalizado sin errores.");
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            if cli.diagnostics_json {
                println!("[{}]", diagnostic.to_json());
            } else {
                eprintln!("\nError en tiempo de ejecución: {diagnostic}");
            }
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    run()
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn legacy_invocation_defaults_to_run() {
        let cli = parse_args(&args(&["program.hl"])).unwrap();
        assert_eq!(cli.command, None);
        assert_eq!(cli.filename.as_deref(), Some("program.hl"));
    }

    #[test]
    fn explicit_run_and_check_are_parsed() {
        assert_eq!(parse_args(&args(&["run", "program.hl"])).unwrap().command, Some(Command::Run));
        assert_eq!(parse_args(&args(&["check", "program.hl"])).unwrap().command, Some(Command::Check));
    }

    #[test]
    fn build_output_path_is_parsed() {
        let cli = parse_args(&args(&["build", "program.hl", "-o", "app"])).unwrap();
        assert_eq!(cli.command, Some(Command::Build));
        assert_eq!(cli.output.as_deref(), Some("app"));
    }

    #[test]
    fn configurable_depth_limits_are_parsed_and_validated() {
        let cli = parse_args(&args(&["run", "--max-parse-depth", "512", "--max-eval-depth", "128", "program.hl"])).unwrap();
        assert_eq!(cli.max_parse_depth, Some(512));
        assert_eq!(cli.max_eval_depth, Some(128));
        assert!(parse_args(&args(&["run", "--max-parse-depth", "0", "program.hl"])).is_err());
        assert!(parse_args(&args(&["run", "--max-eval-depth", "abc", "program.hl"])).is_err());
        assert!(parse_args(&args(&["check", "--max-eval-depth", "32", "program.hl"])).is_err());
    }

    #[test]
    fn instruction_budget_flag_is_parsed_and_validated() {
        let cli = parse_args(&args(&["run", "--max-eval-steps", "500", "program.hl"])).unwrap();
        assert_eq!(cli.max_eval_steps, Some(500));
        assert!(parse_args(&args(&["run", "--max-eval-steps", "0", "program.hl"])).is_err());
        assert!(parse_args(&args(&["run", "--max-eval-steps", "abc", "program.hl"])).is_err());
        assert!(parse_args(&args(&["check", "--max-eval-steps", "500", "program.hl"])).is_err());
        assert!(parse_args(&args(&["build", "--max-eval-steps", "500", "program.hl"])).is_err());
    }

    #[test]
    fn evaluation_time_budget_flag_is_parsed_and_validated() {
        let cli = parse_args(&args(&["run", "--max-eval-millis", "250", "program.hl"])).unwrap();
        assert_eq!(cli.max_eval_millis, Some(250));
        assert!(parse_args(&args(&["run", "--max-eval-millis", "0", "program.hl"])).is_err());
        assert!(parse_args(&args(&["run", "--max-eval-millis", "abc", "program.hl"])).is_err());
        assert!(parse_args(&args(&["check", "--max-eval-millis", "250", "program.hl"])).is_err());
        assert!(parse_args(&args(&["build", "--max-eval-millis", "250", "program.hl"])).is_err());
    }

    #[test]
    fn build_allows_json_diagnostics_but_not_formatting() {
        let cli = parse_args(&args(&["build", "--diagnostics-json", "program.hl"])).unwrap();
        assert_eq!(cli.command, Some(Command::Build));
        assert!(cli.diagnostics_json);
        assert!(parse_args(&args(&["build", "--format", "program.hl"])).is_err());
    }

    #[test]
    fn rejects_duplicate_source_files_and_invalid_flag_combinations() {
        assert!(parse_args(&args(&["run", "a.hl", "b.hl"])).is_err());
        assert!(parse_args(&args(&["check", "--format", "a.hl"])).is_err());
        assert!(parse_args(&args(&["run", "-o", "app", "a.hl"])).is_err());
    }
}
