// src/ast.rs
// Este módulo define la estructura de datos que representa nuestro código.
// Es el "puente" entre el Parser y el Evaluador.

// Importamos el Token de nuestro nuevo lexer, ya que la expresión Infix lo necesita.
use crate::lexer::{Token, Number};

// Un Programa es la raíz de nuestro AST, contiene una lista de sentencias.
#[derive(Debug)]
pub struct Program {
    pub statements: Vec<Statement>,
}

// Las Sentencias son acciones: let, print, while, if, etc.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Let(LetStatement),
    Print(PrintStatement),
    Block(Vec<Statement>), // Bloque { stmt; stmt; ... }
    While(WhileStatement),
    If(IfStatement),
}

// Las Expresiones son fragmentos de código que producen un valor: 5, x, a + b.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    // Un identificador, como el nombre de una variable. ej: `mi_variable`
    Identifier(String),
    
    // Un literal numérico. Lo guardamos como f64 directamente, como espera el evaluador.
    NumberLiteral(f64),
    
    // Un literal de cadena de texto. ej: `"Hola Mundo"`
    StringLiteral(String),
    
    // Una operación entre dos expresiones.
    Infix {
        left: Box<Expression>, // Usamos Box para evitar recursión infinita de tamaño
        operator: Token,       // Guardamos el token del operador (+, *, etc.)
        right: Box<Expression>,
    },
    
    // Ternario: cond ? expr1 : expr2
    Ternary {
        condition: Box<Expression>,
        true_branch: Box<Expression>,
        false_branch: Box<Expression>,
    },
    
    // Lambda: |x, y| { cuerpo }
    Lambda {
        params: Vec<String>,
        body: Box<Expression>,
    },
    
    // Llamada a función: func(a, b)
    FunctionCall {
        func: Box<Expression>,
        args: Vec<Expression>,
    },
    
    // Array: [1, "hola", 2 * 2]
    Array(Vec<Expression>),
    
    // Mapa: @{ "key": value }
    Map(Vec<(Expression, Expression)>),
    
    // Indexación: expr[index]
    Index {
        target: Box<Expression>,
        index: Box<Expression>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct WhileStatement {
    pub condition: Expression,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStatement {
    pub condition: Expression,
    pub true_body: Vec<Statement>,
    pub false_body: Option<Vec<Statement>>,
}