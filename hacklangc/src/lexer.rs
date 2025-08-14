// src/lexer.rs

use std::hash::{Hash, Hasher};

// Tu struct `Number` y sus implementaciones de `Eq` y `Hash` son perfectas.
// Las mantenemos tal cual.
#[derive(Debug, Clone, PartialEq)]
pub struct Number(pub f64);

impl Eq for Number {}

impl Hash for Number {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.0.is_nan() {
            0.hash(state);
        } else {
            self.0.to_bits().hash(state);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    Plus,
    Minus,
    Star,
    Slash,
    Equals,
    Semicolon,
    Identifier(String),
    Number(Number),
    Let,
    Print,
    Eof,
}

pub struct Lexer {
    input: String,
    position: usize,
    chars: Vec<char>,
}

impl Lexer {
    pub fn new(input: String) -> Self {
        let chars = input.chars().collect();
        Lexer {
            input,
            position: 0,
            chars,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();

        while self.position < self.chars.len() {
            let c = self.chars[self.position];

            if c.is_whitespace() {
                self.position += 1;
                continue;
            }

            match c {
                '+' => {
                    tokens.push(Token::Plus);
                    self.position += 1;
                }
                '-' => {
                    tokens.push(Token::Minus);
                    self.position += 1;
                }
                '*' => {
                    tokens.push(Token::Star);
                    self.position += 1;
                }
                '/' => {
                    tokens.push(Token::Slash);
                    self.position += 1;
                }
                '=' => {
                    tokens.push(Token::Equals);
                    self.position += 1;
                }
                ';' => {
                    tokens.push(Token::Semicolon);
                    self.position += 1;
                }
                'a'..='z' | 'A'..='Z' | '_' => {
                    let ident = self.read_identifier();
                    let token = match ident.as_str() {
                        "let" => Token::Let,
                        "print" => Token::Print,
                        _ => Token::Identifier(ident),
                    };
                    tokens.push(token);
                }
                '0'..='9' => {
                    let number = self.read_number();
                    tokens.push(Token::Number(Number(number)));
                }
                _ => {
                    panic!("Carácter inesperado: {} en posición {}", c, self.position);
                }
            }
        }

        tokens.push(Token::Eof);
        tokens
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while self.position < self.chars.len() {
            let c = self.chars[self.position];
            if !c.is_alphanumeric() && c != '_' {
                break;
            }
            self.position += 1;
        }
        self.input[start..self.position].to_string()
    }

    // --- FUNCIÓN MEJORADA ---
    fn read_number(&mut self) -> f64 {
        let start = self.position;
        while self.position < self.chars.len() {
            let c = self.chars[self.position];
            if !c.is_digit(10) && c != '.' {
                break;
            }
            self.position += 1;
        }
        let number_str = &self.input[start..self.position];

        // En lugar de usar `.expect()`, usamos un `match` para manejar el error.
        // Esto nos permite dar un mensaje de error mucho más útil.
        match number_str.parse::<f64>() {
            Ok(num) => num,
            Err(_) => {
                // Si el parseo falla, el programa entrará en pánico aquí
                // y nos dirá exactamente qué trozo de texto no pudo convertir.
                panic!("Error léxico: '{}' no es un número válido.", number_str);
            }
        }
    }
}