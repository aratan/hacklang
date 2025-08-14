// src/parser.rs

// Importamos las definiciones del AST y los Tokens del Lexer.
// Necesitamos `Number` para poder extraer el f64 de adentro.
use crate::ast::*;
use crate::lexer::{Number, Token};
use std::collections::HashMap;

// Definimos la "fuerza" de cada operador para manejar la precedencia.
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug)]
enum Precedence {
    Lowest,
    Sum,     // + , -
    Product, // * , /
}

pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    // ¡Volvemos a usar Token como llave! Es mucho más limpio gracias a tu lexer.
    precedences: HashMap<Token, Precedence>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        let mut precedences = HashMap::new();
        // Insertamos los operadores directamente como llaves.
        precedences.insert(Token::Plus, Precedence::Sum);
        precedences.insert(Token::Minus, Precedence::Sum);
        precedences.insert(Token::Star, Precedence::Product);
        precedences.insert(Token::Slash, Precedence::Product);

        Parser {
            tokens,
            position: 0,
            precedences,
        }
    }

    // El método principal que construye el árbol completo.
    pub fn parse_program(&mut self) -> Program {
        let mut statements = Vec::new();
        while !self.is_at_end() {
            match self.parse_statement() {
                Ok(stmt) => statements.push(stmt),
                Err(e) => panic!("Error de Análisis Sintáctico: {}", e),
            }
        }
        Program { statements }
    }

    fn parse_statement(&mut self) -> Result<Statement, String> {
        match self.current_token() {
            Token::Let => self.parse_let_statement(),
            Token::Print => self.parse_print_statement(),
            _ => Err(format!(
                "Se esperaba el inicio de una sentencia ('let' o 'print'), pero se encontró: {:?}",
                self.current_token()
            )),
        }
    }

    fn parse_let_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'let'

        let name = match self.current_token() {
            Token::Identifier(name) => name.clone(),
            _ => return Err(format!("Se esperaba un nombre de variable, se encontró {:?}", self.current_token())),
        };
        self.advance(); // consume identifier

        if *self.current_token() != Token::Equals {
            return Err("Se esperaba '=' después del nombre de la variable".to_string());
        }
        self.advance(); // consume '='

        let value = self.parse_expression(Precedence::Lowest)?;

        if *self.current_token() == Token::Semicolon {
            self.advance();
        }

        Ok(Statement::Let(LetStatement { name, value }))
    }

    fn parse_print_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'print'
        let value = self.parse_expression(Precedence::Lowest)?;
        if *self.current_token() == Token::Semicolon {
            self.advance();
        }
        Ok(Statement::Print(PrintStatement { value }))
    }

    // --- El Parser de Expresiones (Pratt Parser) ---

    fn get_precedence(&self, token: &Token) -> Precedence {
        // La búsqueda ahora es directa y devuelve una copia.
        self.precedences.get(token).cloned().unwrap_or(Precedence::Lowest)
    }

    fn parse_expression(&mut self, precedence: Precedence) -> Result<Expression, String> {
        // Parsea la parte "prefijo" (un número, una variable)
        let mut left_expr = self.parse_prefix()?;

        // Mientras el siguiente token sea un operador con más "fuerza"...
        while *self.current_token() != Token::Semicolon && precedence < self.get_precedence(self.current_token()) {
            let operator = self.current_token().clone();
            if operator == Token::Eof { break; }

            self.advance(); // Consumimos el operador

            let right_expr = self.parse_expression(self.get_precedence(&operator))?;

            left_expr = Expression::Infix {
                left: Box::new(left_expr),
                operator,
                right: Box::new(right_expr),
            };
        }
        Ok(left_expr)
    }

    // Parsea los elementos básicos de una expresión
    fn parse_prefix(&mut self) -> Result<Expression, String> {
        let token = self.current_token().clone();
        match token {
            Token::Identifier(name) => {
                self.advance();
                Ok(Expression::Identifier(name))
            }
            // ¡Punto clave! Extraemos el f64 de adentro del struct Number
            Token::Number(Number(value)) => {
                self.advance();
                Ok(Expression::NumberLiteral(value))
            }
            _ => Err(format!(
                "Token inesperado: {:?}. Se esperaba un número o un identificador.",
                token
            )),
        }
    }

    // --- Funciones de Ayuda ---
    fn advance(&mut self) {
        if !self.is_at_end() {
            self.position += 1;
        }
    }

    fn current_token(&self) -> &Token {
        self.tokens.get(self.position).unwrap_or(&Token::Eof)
    }

    fn is_at_end(&self) -> bool {
        matches!(self.current_token(), Token::Eof)
    }
}