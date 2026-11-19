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
    BraceOpen,
    BraceClose,
    DeclareAssign, // := declaración compacta
    BracketOpen,
    BracketClose,
    Identifier(String),
    Number(Number),
    String(String),
    Let,
    Print,
    Return,
    Break,
    Continue,
    While,
    If,
    True,
    False,
    
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

/// Character-based source location. Offsets and columns count Unicode scalar values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// A token paired with its exact source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpannedToken {
    pub token: Token,
    pub span: SourceSpan,
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
        self.tokenize_spanned().map(|items| items.into_iter().map(|item| item.token).collect())
    }

    pub fn tokenize_spanned(&mut self) -> Result<Vec<SpannedToken>, String> {
        let mut tokens = Vec::new();

        while self.position < self.chars.len() {
            let token_start = self.position;
            let c = self.chars[self.position];

            if c.is_whitespace() {
                self.position += 1;
                continue;
            }

            match c {
                '+' => {
                    tokens.push(spanned(Token::Plus, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '-' => {
                    tokens.push(spanned(Token::Minus, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '*' => {
                    tokens.push(spanned(Token::Star, token_start, self.position, &self.chars));
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
                        tokens.push(spanned(Token::Slash, token_start, self.position, &self.chars));
                        self.position += 1;
                    }
                }
                ':' if self.position + 1 < self.chars.len() && self.chars[self.position + 1] == '=' => {
                    tokens.push(spanned(Token::DeclareAssign, token_start, self.position, &self.chars));
                    self.position += 2;
                }
                '=' => {
                    tokens.push(spanned(Token::Equals, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                ';' => {
                    tokens.push(spanned(Token::Semicolon, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                'a'..='z' | 'A'..='Z' | '_' => {
                    let ident = self.read_identifier();
                    let token = match ident.as_str() {
                        "let" => Token::Let,
                        "print" => Token::Print,
                        "return" => Token::Return,
                        "break" => Token::Break,
                        "continue" => Token::Continue,
                        "while" => Token::While,
                        "if" => Token::If,
                        "true" => Token::True,
                        "false" => Token::False,
                        _ => Token::Identifier(ident),
                    };
                    tokens.push(spanned(token, token_start, self.position, &self.chars));
                }
                '0'..='9' => {
                    let number = self.read_number()?;
                    tokens.push(spanned(Token::Number(Number(number)), token_start, self.position, &self.chars));
                }
                '"' => {
                    let string = self.read_string()?;
                    tokens.push(spanned(Token::String(string), token_start, self.position, &self.chars));
                }
                '(' => {
                    tokens.push(spanned(Token::ParenOpen, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                ')' => {
                    tokens.push(spanned(Token::ParenClose, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '{' => {
                    tokens.push(spanned(Token::BraceOpen, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '}' => {
                    tokens.push(spanned(Token::BraceClose, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '[' => {
                    tokens.push(spanned(Token::BracketOpen, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                ']' => {
                    tokens.push(spanned(Token::BracketClose, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '<' => {
                    tokens.push(spanned(Token::Less, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '>' => {
                    tokens.push(spanned(Token::Greater, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '~' => {
                    tokens.push(spanned(Token::Tilde, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '^' => {
                    tokens.push(spanned(Token::Caret, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '?' => {
                    tokens.push(spanned(Token::Question, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                ':' => {
                    tokens.push(spanned(Token::Colon, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '|' => {
                    tokens.push(spanned(Token::Pipe, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                ',' => {
                    tokens.push(spanned(Token::Comma, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                '@' => {
                    tokens.push(spanned(Token::At, token_start, self.position, &self.chars));
                    self.position += 1;
                }
                _ => {
                    return Err(format!("Carácter inesperado: '{}' en posición {}", c, self.position));
                }
            }
        }

        tokens.push(spanned(Token::Eof, token_start, self.position, &self.chars));
        Ok(tokens)
    }


fn spanned(token: Token, start: usize, end: usize, chars: &[char]) -> SpannedToken {
    let mut line = 1;
    let mut column = 1;
    for ch in &chars[..start] {
        if *ch == '\n' { line += 1; column = 1; } else { column += 1; }
    }
    let start_line = line;
    let start_column = column;
    for ch in &chars[start..end] {
        if *ch == '\n' { line += 1; column = 1; } else { column += 1; }
    }
    SpannedToken {
        token,
        span: SourceSpan { start, end, line: start_line, column: start_column, end_line: line, end_column: column },
    }
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
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![Token::Eof]);
    }

    #[test]
    fn test_keywords() {
        let mut lexer = Lexer::new("let print while if true false".to_string());
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![
            Token::Let,
            Token::Print,
            Token::While,
            Token::If,
            Token::True,
            Token::False,
            Token::Eof,
        ]);
    }

    #[test]
    fn test_identifiers() {
        let mut lexer = Lexer::new("foo bar _tmp x42".to_string());
        let tokens = lexer.tokenize().unwrap();
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
        let tokens = lexer.tokenize().unwrap();
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
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![
            Token::String("hello".to_string()),
            Token::String("world".to_string()),
            Token::Eof,
        ]);
    }

    #[test]
    fn test_operators() {
        let mut lexer = Lexer::new("+ - * / < > ~ ^".to_string());
        let tokens = lexer.tokenize().unwrap();
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
        let mut lexer = Lexer::new("= := ; ( ) { } ? : | , @".to_string());
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens, vec![
            Token::Equals,
            Token::DeclareAssign,
            Token::Semicolon,
            Token::ParenOpen,
            Token::ParenClose,
            Token::BraceOpen,
            Token::BraceClose,
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
        let tokens = lexer.tokenize().unwrap();
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
        let tokens = lexer.tokenize().unwrap();
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

    #[test]
    fn spanned_tokens_report_unicode_aware_line_and_column() {
        let mut lexer = Lexer::new("\"ñ\"\\nvalue := 2".to_string());
        let tokens = lexer.tokenize_spanned().unwrap();
        assert_eq!(tokens[0].span.start, 0);
        assert_eq!(tokens[0].span.end, 3);
        assert_eq!(tokens[1].span.line, 2);
        assert_eq!(tokens[1].span.column, 1);
        assert_eq!(tokens[1].span.start, 4);
        assert_eq!(tokens[2].span.column, 7);
        assert_eq!(tokens[2].span.end_column, 9);
    }

}
