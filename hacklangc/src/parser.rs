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
    Ternary, // ? :
    Sum,     // + , -
    Product, // * , /
    Comparison, // < , >
}

pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    precedences: HashMap<Token, Precedence>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        let mut precedences = HashMap::new();
        precedences.insert(Token::Plus, Precedence::Sum);
        precedences.insert(Token::Minus, Precedence::Sum);
        precedences.insert(Token::Star, Precedence::Product);
        precedences.insert(Token::Slash, Precedence::Product);
        precedences.insert(Token::Less, Precedence::Comparison);
        precedences.insert(Token::Greater, Precedence::Comparison);
        precedences.insert(Token::Tilde, Precedence::Comparison);
        precedences.insert(Token::Caret, Precedence::Comparison);
        precedences.insert(Token::Question, Precedence::Ternary);
        precedences.insert(Token::Colon, Precedence::Ternary);

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
            Token::ParenOpen => self.parse_block_statement(),
            Token::While => self.parse_while_statement(),
            Token::Identifier(name) => {
                let next = self.peek_ahead(1);
                if next == Some(&Token::Equals) {
                    self.parse_let_statement()
                } else {
                    // Puede ser llamada a función o index
                    let ident = name.clone();
                    self.advance();
                    if *self.current_token() == Token::ParenOpen {
                        self.advance();
                        let args = self.parse_comma_list(Token::ParenClose)?;
                        Ok(Statement::Print(PrintStatement {
                            value: Expression::FunctionCall {
                                func: Box::new(Expression::Identifier(ident)),
                                args,
                            },
                        }))
                    } else if *self.current_token() == Token::ParenOpen {
                        self.advance();
                        let args = self.parse_comma_list(Token::ParenClose)?;
                        Ok(Statement::Let(LetStatement {
                            name: ident,
                            value: Expression::FunctionCall {
                                func: Box::new(Expression::Identifier(ident)),
                                args,
                            },
                        }))
                    } else {
                        Ok(Statement::Let(LetStatement {
                            name: ident,
                            value: Expression::Identifier(ident),
                        }))
                    }
                }
            }
            Token::Pipe => self.parse_lambda_statement(),
            _ => Err(format!(
                "Se esperaba el inicio de una sentencia, pero se encontró: {:?}",
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

    fn parse_block_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume (
        let mut stmts = Vec::new();
        while !self.is_at_end() && *self.current_token() != Token::ParenClose {
            let stmt = self.parse_statement()?;
            stmts.push(stmt);
            if *self.current_token() == Token::Semicolon {
                self.advance();
            }
        }
        if *self.current_token() == Token::ParenClose {
            self.advance();
        }
        Ok(Statement::Block(stmts))
    }

    fn parse_while_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'while'
        let condition = self.parse_expression(Precedence::Lowest)?;
        if *self.current_token() != Token::ParenOpen {
            return Err("Se esperaba '(' después de la condición".to_string());
        }
        self.advance(); // consume (
        let body = self.parse_block()?;
        if *self.current_token() != Token::ParenClose {
            return Err("Se esperaba ')' al final del while".to_string());
        }
        self.advance(); // consume )
        Ok(Statement::While(WhileStatement { condition, body }))
    }

    fn parse_if_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'if'
        let condition = self.parse_expression(Precedence::Lowest)?;
        self.advance(); // consume (
        let true_body = self.parse_block()?;
        self.advance(); // consume )
        
        let false_body = if *self.current_token() == Token::ParenOpen {
            self.advance(); // consume (
            Some(self.parse_block()?)
        } else {
            None
        };

        Ok(Statement::If(IfStatement {
            condition,
            true_body,
            false_body,
        }))
    }

    // --- Sentencia Lambda: |x, y| { cuerpo } ---
    fn parse_lambda_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume primer |
        let params = self.parse_params()?;
        
        if !self.is_at_end() && *self.current_token() != Token::Pipe {
            return Err("Se esperaba '|' después de los parámetros".to_string());
        }
        self.advance(); // consume |

        let body = self.parse_expression(Precedence::Lowest)?;

        if *self.current_token() == Token::Semicolon {
            self.advance();
        }

        let name = format!("__lambda_{}", params.join("_"));
        Ok(Statement::Let(LetStatement {
            name,
            value: Expression::Lambda { params, body: Box::new(body) },
        }))
    }

    fn parse_params(&mut self) -> Result<Vec<String>, String> {
        let mut params = Vec::new();
        while !self.is_at_end() && *self.current_token() != Token::Pipe {
            if *self.current_token() == Token::Comma {
                self.advance();
                continue;
            }
            match self.current_token() {
                Token::Identifier(name) => {
                    params.push(name.clone());
                    self.advance();
                }
                _ => return Err("Se esperaba un nombre de parámetro".to_string()),
            }
        }
        Ok(params)
    }

    // --- El Parser de Expresiones (Pratt Parser) ---

    fn get_precedence(&self, token: &Token) -> Precedence {
        if let Token::String(_) = token {
            return Precedence::Sum;
        }
        self.precedences.get(token).cloned().unwrap_or(Precedence::Lowest)
    }

    fn parse_expression(&mut self, precedence: Precedence) -> Result<Expression, String> {
        // Parsea la parte "prefijo" (un número, una variable, un paréntesis)
        let mut left_expr = self.parse_prefix()?;

        // Mientras el siguiente token sea un operador con más "fuerza"...
        while *self.current_token() != Token::Semicolon && precedence < self.get_precedence(self.current_token()) {
            let operator = self.current_token().clone();
            if operator == Token::Eof { break; }
            // No procesar operadores que no están en precedences como binarios
            if self.precedences.contains_key(&operator) {
                self.advance(); // Consumimos el operador
                let right_expr = self.parse_expression(self.get_precedence(&operator))?;
                left_expr = Expression::Infix {
                    left: Box::new(left_expr),
                    operator,
                    right: Box::new(right_expr),
                };
            } else {
                break;
            }
        }

        // Ternario: ? true_expr : false_expr
        if *self.current_token() == Token::Question && precedence <= Precedence::Ternary {
            self.advance(); // consume ?
            let true_branch = self.parse_expression(Precedence::Lowest)?;
            if *self.current_token() == Token::Colon {
                self.advance(); // consume :
                let false_branch = self.parse_expression(Precedence::Lowest)?;
                left_expr = Expression::Ternary {
                    condition: Box::new(true_branch),
                    true_branch,
                    false_branch,
                };
            }
        }

        Ok(left_expr)
    }

    // Parsea los elementos básicos de una expresión
    fn parse_prefix(&mut self) -> Result<Expression, String> {
        let token = self.current_token().clone();
        match token {
            Token::Identifier(name) => {
                self.advance();
                // Verificar si es llamada a función
                if !self.is_at_end() && *self.current_token() == Token::ParenOpen {
                    self.advance();
                    let args = self.parse_comma_list(Token::ParenClose)?;
                    Ok(Expression::FunctionCall {
                        func: Box::new(Expression::Identifier(name)),
                        args,
                    })
                } else {
                    Ok(Expression::Identifier(name))
                }
            }
            // Extraemos el f64 de adentro del struct Number
            Token::Number(Number(value)) => {
                self.advance();
                Ok(Expression::NumberLiteral(value))
            }
            // Strings son valores que se concatenan con +
            Token::String(s) => {
                self.advance();
                Ok(Expression::StringLiteral(s))
            }
            Token::ParenOpen => {
                self.advance();
                let expr = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() == Token::ParenClose {
                    self.advance();
                }
                Ok(expr)
            }
            Token::Pipe => {
                // Lambda: |x, y| { body }
                self.advance();
                let params = self.parse_params()?;
                if !self.is_at_end() && *self.current_token() != Token::Pipe {
                    return Err("Se esperaba '|' después de los parámetros".to_string());
                }
                self.advance();
                let body = self.parse_expression(Precedence::Lowest)?;
                Ok(Expression::Lambda { params, body: Box::new(body) })
            }
            Token::At => {
                // Mapa: @{ "key": value }
                self.advance();
                if *self.current_token() != Token::ParenOpen {
                    return Err("Se esperaba '(' después de '@'".to_string());
                }
                self.advance();
                let pairs = self.parse_map_entries()?;
                Ok(Expression::Map(pairs))
            }
            _ => Err(format!(
                "Token inesperado: {:?}. Se esperaba un número, un identificador, un string, '(' o '|'.",
                token
            )),
        }
    }

    // parsea una lista separada por comas
    fn parse_comma_list(&mut self, end_token: Token) -> Result<Vec<Expression>, String> {
        let mut items = Vec::new();
        while !self.is_at_end() && *self.current_token() != end_token {
            if *self.current_token() == Token::Comma {
                self.advance();
                continue;
            }
            let expr = self.parse_expression(Precedence::Lowest)?;
            items.push(expr);
            // Consumir coma opcional
            if *self.current_token() == Token::Comma {
                self.advance();
            }
        }
        Ok(items)
    }

    // parsea entradas de mapa key:value separadas por comas
    fn parse_map_entries(&mut self) -> Result<Vec<(Expression, Expression)>, String> {
        let mut items = Vec::new();
        while !self.is_at_end() && *self.current_token() != Token::ParenClose {
            if *self.current_token() == Token::Comma {
                self.advance();
                continue;
            }
            let key = self.parse_expression(Precedence::Lowest)?;
            if *self.current_token() != Token::Colon {
                return Err(format!("Se esperaba ':' después de '{}'", key));
            }
            self.advance();
            let value = self.parse_expression(Precedence::Lowest)?;
            items.push((key, value));
            if *self.current_token() == Token::Comma {
                self.advance();
            }
        }
        Ok(items)
    }

    fn parse_block(&mut self) -> Result<Vec<Statement>, String> {
        let mut stmts = Vec::new();
        while !self.is_at_end() && *self.current_token() != Token::ParenClose {
            let stmt = self.parse_statement()?;
            stmts.push(stmt);
            if *self.current_token() == Token::Semicolon {
                self.advance();
            }
        }
        Ok(stmts)
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

    fn peek_ahead(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.position + offset)
    }
}
