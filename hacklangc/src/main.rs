// src/main.rs

use std::env;
use std::fs;

// Declaramos la existencia de todos nuestros módulos.
mod lexer;
mod ast;
mod parser;
mod evaluador; // El módulo que contiene la lógica de ejecución.

fn main() {
    // 1. Leer el código fuente del archivo pasado como argumento.
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        // Usamos eprintln! para imprimir errores al "standard error".
        eprintln!("Uso: hacklangc <nombre_de_archivo.hl>");
        return;
    }
    let filename = &args[1];
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
    let tokens = lexer.tokenize();

    // --- FASE 2: PARSER ---
    // Convierte la secuencia de Tokens en un Árbol de Sintaxis Abstracta (AST).
    let mut parser = parser::Parser::new(tokens);
    let ast = parser.parse_program();

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