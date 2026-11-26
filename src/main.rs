// src/main.rs

use std::env;
use std::fs;
use std::process::ExitCode;

// Un único front-end compartido para run/check/build.
mod lexer;
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
}

fn usage() -> &'static str {
    "Uso:
  hl [run] [--compact] [--format] [--diagnostics-json] <archivo.hl>
  hl check [--compact] [--diagnostics-json] <archivo.hl>
  hl build [--compact] <archivo.hl> [-o <ejecutable>]

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
    if cli.output.is_some() && cli.command != Some(Command::Build) {
        return Err("-o solo se admite con 'build'".to_string());
    }
    if cli.command == Some(Command::Build) && (cli.format || cli.diagnostics_json) {
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
    let tokens = match lexer.tokenize_spanned() {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("Error léxico: {error}");
            return ExitCode::from(1);
        }
    };

    let mut parser = if cli.compact {
        parser::Parser::new_compact_spanned(tokens)
    } else {
        parser::Parser::new_spanned(tokens)
    };
    let ast = match parser.parse_program() {
        Ok(program) => program,
        Err(error) => {
            eprintln!("Error de análisis sintáctico: {error}");
            return ExitCode::from(1);
        }
    };

    if command == Command::Build {
        let output = cli.output.as_deref().map(std::path::Path::new);
        match native::compile_to_executable(&ast, std::path::Path::new(filename), output) {
            Ok(executable) => {
                println!("Ejecutable nativo generado: {}", executable.display());
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("Error de compilación nativa: {error}");
                return ExitCode::from(1);
            }
        }
    }

    if cli.format {
        println!("{}", formatter::format_program(&ast));
        return ExitCode::SUCCESS;
    }

    let diagnostics = type_checker::TypeChecker::check(&ast);
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
    let mut evaluator = evaluador::Evaluator::new();
    match evaluator.evaluate(&ast) {
        Ok(()) => {
            println!("--------------------------------");
            println!("Programa finalizado sin errores.");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("\nError en tiempo de ejecución: {error}");
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
    fn rejects_duplicate_source_files_and_invalid_flag_combinations() {
        assert!(parse_args(&args(&["run", "a.hl", "b.hl"])).is_err());
        assert!(parse_args(&args(&["check", "--format", "a.hl"])).is_err());
        assert!(parse_args(&args(&["run", "-o", "app", "a.hl"])).is_err());
    }
}
