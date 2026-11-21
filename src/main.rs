// src/main.rs

use std::env;
use std::fs;

// Declaramos la existencia de todos nuestros módulos.
mod lexer;
mod ast;
mod parser;
mod evaluador; // El módulo que contiene la lógica de ejecución.
mod type_checker; // Análisis estático no bloqueante.
mod formatter; // Formateador canónico de sintaxis compacta.

fn main() {
    // 1. Leer el código fuente del archivo pasado como argumento.
    let args: Vec<String> = env::args().skip(1).collect();
    let compact_mode = args.iter().any(|arg| arg == "--compact");
    let format_mode = args.iter().any(|arg| arg == "--format");
    let diagnostics_json = args.iter().any(|arg| arg == "--diagnostics-json");
    let filename = match args.iter().find(|arg| arg.as_str() != "--compact" && arg.as_str() != "--format" && arg.as_str() != "--diagnostics-json") {
        Some(filename) => filename,
        None => {
            eprintln!("Uso: hl [--compact] [--format] [--diagnostics-json] <nombre_de_archivo.hl>");
            return;
        }
    };
    let source_code = match fs::read_to_string(filename) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: No se pudo leer el archivo '{}': {}", filename, e);
            return;
        }
    };

    // --- FASE 1: LEXER ---
    // Convierte el texto en una secuencia de Tokens.
    let mut lexer = lexer::Lexer::new(source_code);
    let tokens = match lexer.tokenize_spanned() {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("Error léxico: {}", error);
            return;
        }
    };

    // --- FASE 2: PARSER ---
    // Convierte la secuencia de Tokens en un Árbol de Sintaxis Abstracta (AST).
    let mut parser = if compact_mode {
        parser::Parser::new_compact_spanned(tokens)
    } else {
        parser::Parser::new_spanned(tokens)
    };
    let ast = match parser.parse_program() {
        Ok(program) => program,
        Err(error) => {
            eprintln!("Error de análisis sintáctico: {}", error);
            return;
        }
    };

    if format_mode {
        println!("{}", formatter::format_program(&ast));
        return;
    }

    // --- FASE 3: ANÁLISIS ESTÁTICO (NO BLOQUEANTE) ---
    // Informa de incompatibilidades demostrables, pero no impide ejecutar
    // programas existentes ni sustituye las comprobaciones del runtime.
    let diagnostics = type_checker::TypeChecker::check(&ast);
    if diagnostics_json {
        let items = diagnostics.iter().map(|diagnostic| diagnostic.to_json()).collect::<Vec<_>>();
        println!("[{}]", items.join(","));
        return;
    }

    if diagnostics.is_empty() {
        println!("Análisis estático: sin incompatibilidades detectadas.");
    } else {
        eprintln!("Análisis estático: {} aviso(s):", diagnostics.len());
        for diagnostic in diagnostics {
            eprintln!("  - {}", diagnostic);
        }
    }

    // --- FASE 3: EVALUADOR (¡La Ejecución!) ---
    // Ahora, en lugar de solo imprimir el AST, lo vamos a ejecutar.
    println!("--- Ejecutando código Hacklang ---");
    
    // Creamos una nueva instancia de nuestro evaluador.
    let mut evaluator = evaluador::Evaluator::new();
    
    // Le pasamos el AST para que lo evalúe.
    // El evaluador se encargará de hacer los `println!` de nuestro lenguaje.
    match evaluator.evaluate(&ast) {
        Ok(_) => {
            // La ejecución fue exitosa.
            println!("--------------------------------");
            println!("Programa finalizado sin errores.");
        }
        Err(e) => {
            // Si el evaluador encuentra un error en tiempo de ejecución (ej: variable no definida),
            // lo capturamos aquí y lo mostramos.
            eprintln!("\nError en tiempo de ejecución: {}", e);
        }
    }
}