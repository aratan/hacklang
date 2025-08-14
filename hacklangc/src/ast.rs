// src/ast.rs
// Este módulo define la estructura de datos que representa nuestro código.
// Es el "puente" entre el Parser y el Evaluador.

// Importamos el Token de nuestro nuevo lexer, ya que la expresión Infix lo necesita.
use crate::lexer::Token;

// Un Programa es la raíz de nuestro AST, contiene una lista de sentencias.
#[derive(Debug)]
pub struct Program {
    pub statements: Vec<Statement>,
}

// Las Sentencias son acciones: let, print, etc.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Let(LetStatement),
    Print(PrintStatement),
}

// Las Expresiones son fragmentos de código que producen un valor: 5, x, a + b.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    // Un identificador, como el nombre de una variable. ej: `mi_variable`
    Identifier(String),
    
    // Un literal numérico. Lo guardamos como f64 directamente, como espera el evaluador.
    NumberLiteral(f64),
    
    // Una operación entre dos expresiones.
    Infix {
        left: Box<Expression>, // Usamos Box para evitar recursión infinita de tamaño
        operator: Token,       // Guardamos el token del operador (+, *, etc.)
        right: Box<Expression>,
    },
}

// Estructuras detalladas para cada tipo de sentencia.

#[derive(Debug, Clone, PartialEq)]
pub struct LetStatement {
    pub name: String,
    pub value: Expression,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrintStatement {
    pub value: Expression,
}