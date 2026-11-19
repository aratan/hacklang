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
    ParenOpen,
    ParenClose,
    BracketOpen,
    BracketClose,
    Identifier(String),
    Number(Number),
    String(String),
    Let,
    Print,
    While,
    If,
    
    // Operadores de comparación
    Less,
    Greater,
    Tilde,      // ~= para ~= (similar a ~=)
    Caret,      // ^ para !=
    
    // Operadores adicionales
    Question,   // ?
    Colon,      // :
    Pipe,       // |
    Comma,      // ,
    At,         // @
    
    Eof,
}

pub struct Lexer {
    position: usize,
    chars: Vec<char>,
}

impl Lexer {
    pub fn new(input: String) -> Self {
        let chars = input.chars().collect();
        Lexer {
            position: 0,
            chars,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
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
                    // Soporte para comentarios de línea //
                    if self.position + 1 < self.chars.len() && self.chars[self.position + 1] == '/' {
                        self.position += 2; // skip //
                        while self.position < self.chars.len() && self.chars[self.position] != '\n' {
                            self.position += 1;
                        }
                    } else {
                        tokens.push(Token::Slash);
                        self.position += 1;
                    }
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
                        "while" => Token::While,
                        "if" => Token::If,
                        _ => Token::Identifier(ident),
                    };
                    tokens.push(token);
                }
                '0'..='9' => {
                    let number = self.read_number()?;
                    tokens.push(Token::Number(Number(number)));
                }
                '"' => {
                    let string = self.read_string()?;
                    tokens.push(Token::String(string));
                }
                '(' => {
                    tokens.push(Token::ParenOpen);
                    self.position += 1;
                }
                ')' => {
                    tokens.push(Token::ParenClose);
                    self.position += 1;
                }
                '[' => {
                    tokens.push(Token::BracketOpen);
                    self.position += 1;
                }
                ']' => {
                    tokens.push(Token::BracketClose);
                    self.position += 1;
                }
                '<' => {
                    tokens.push(Token::Less);
                    self.position += 1;
                }
                '>' => {
                    tokens.push(Token::Greater);
                    self.position += 1;
                }
                '~' => {
                    tokens.push(Token::Tilde);
                    self.position += 1;
                }
                '^' => {
                    tokens.push(Token::Caret);
                    self.position += 1;
                }
                '?' => {
                    tokens.push(Token::Question);
                    self.position += 1;
                }
                ':' => {
                    tokens.push(Token::Colon);
                    self.position += 1;
                }
                '|' => {
                    tokens.push(Token::Pipe);
                    self.position += 1;
                }
                ',' => {
                    tokens.push(Token::Comma);
                    self.position += 1;
                }
                '@' => {
                    tokens.push(Token::At);
                    self.position += 1;
                }
                _ => {
                    return Err(format!("Carácter inesperado: '{}' en posición {}", c, self.position));
                }
            }
        }

        tokens.push(Token::Eof);
        Ok(tokens)
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
        self.chars[start..self.position].iter().collect()
    }

    fn read_string(&mut self) -> Result<String, String> {
        self.position += 1; // skip opening quote
        let start = self.position;
        while self.position < self.chars.len() {
            let c = self.chars[self.position];
            if c == '"' {
                let value = self.chars[start..self.position].iter().collect();
                self.position += 1; // skip closing quote
                return Ok(value);
            }
            self.position += 1;
        }
        Err(format!("Cadena sin cerrar desde la posición {}", start - 1))
    }

    // --- FUNCIÓN MEJORADA ---
    fn read_number(&mut self) -> Result<f64, String> {
        let start = self.position;
        while self.position < self.chars.len() {
            let c = self.chars[self.position];
            if !c.is_digit(10) && c != '.' {
                break;
            }
            self.position += 1;
        }
        let number_str: String = self.chars[start..self.position].iter().collect();
        number_str.parse::<f64>()
            .map_err(|_| format!("Error léxico: '{}' no es un número válido.", number_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let mut lexer = Lexer::new(String::new());
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![Token::Eof]);
    }

    #[test]
    fn test_whitespace_only() {
        let mut lexer = Lexer::new("   \n\t  ".to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![Token::Eof]);
    }

    #[test]
    fn test_keywords() {
        let mut lexer = Lexer::new("let print while if".to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::Let,
            Token::Print,
            Token::While,
            Token::If,
            Token::Eof,
        ]);
    }

    #[test]
    fn test_identifiers() {
        let mut lexer = Lexer::new("foo bar _tmp x42".to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::Identifier("foo".to_string()),
            Token::Identifier("bar".to_string()),
            Token::Identifier("_tmp".to_string()),
            Token::Identifier("x42".to_string()),
            Token::Eof,
        ]);
    }

    #[test]
    fn test_numbers() {
        let mut lexer = Lexer::new("42 3.14 0".to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::Number(Number(42.0)),
            Token::Number(Number(3.14)),
            Token::Number(Number(0.0)),
            Token::Eof,
        ]);
    }

    #[test]
    fn test_strings() {
        let mut lexer = Lexer::new(r#""hello" "world""#.to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::String("hello".to_string()),
            Token::String("world".to_string()),
            Token::Eof,
        ]);
    }

    #[test]
    fn test_operators() {
        let mut lexer = Lexer::new("+ - * / < > ~ ^".to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::Plus,
            Token::Minus,
            Token::Star,
            Token::Slash,
            Token::Less,
            Token::Greater,
            Token::Tilde,
            Token::Caret,
            Token::Eof,
        ]);
    }

    #[test]
    fn test_special_chars() {
        let mut lexer = Lexer::new("= ; ( ) ? : | , @".to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::Equals,
            Token::Semicolon,
            Token::ParenOpen,
            Token::ParenClose,
            Token::Question,
            Token::Colon,
            Token::Pipe,
            Token::Comma,
            Token::At,
            Token::Eof,
        ]);
    }

    #[test]
    fn test_program_tokens() {
        let src = "let x = 42; print x;";
        let mut lexer = Lexer::new(src.to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::Let,
            Token::Identifier("x".to_string()),
            Token::Equals,
            Token::Number(Number(42.0)),
            Token::Semicolon,
            Token::Print,
            Token::Identifier("x".to_string()),
            Token::Semicolon,
            Token::Eof,
        ]);
    }

    #[test]
    fn test_while_if_tokens() {
        let src = "while (i < 10) ( print i; )";
        let mut lexer = Lexer::new(src.to_string());
        let tokens = lexer.tokenize();
        assert_eq!(tokens, vec![
            Token::While,
            Token::ParenOpen,
            Token::Identifier("i".to_string()),
            Token::Less,
            Token::Number(Number(10.0)),
            Token::ParenClose,
            Token::ParenOpen,
            Token::Print,
            Token::Identifier("i".to_string()),
            Token::Semicolon,
            Token::ParenClose,
            Token::Eof,
        ]);
    }
    #[test]
    fn test_unexpected_character_returns_error() {
        let mut lexer = Lexer::new("let x = 1; $".to_string());
        assert!(lexer.tokenize().unwrap_err().contains("Carácter inesperado"));
    }

    #[test]
    fn test_unterminated_string_returns_error() {
        let mut lexer = Lexer::new("\"unterminated".to_string());
        assert!(lexer.tokenize().unwrap_err().contains("Cadena sin cerrar"));
    }

    #[test]
    fn test_unicode_before_identifier_does_not_panic() {
        let mut lexer = Lexer::new("\"ñ\" name".to_string());
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], Token::String("ñ".to_string()));
        assert_eq!(tokens[1], Token::Identifier("name".to_string()));
    }

}
