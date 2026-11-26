// src/parser.rs

// Importamos las definiciones del AST y los Tokens del Lexer.
// Necesitamos `Number` para poder extraer el f64 de adentro.
use crate::ast::*;
use crate::lexer::{Number, SourceSpan, SpannedToken, Token};
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

const MAX_PARSE_DEPTH: usize = 256;

pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    precedences: HashMap<Token, Precedence>,
    compact: bool,
    expression_depth: usize,
    statement_depth: usize,
    spans: Option<Vec<SourceSpan>>,
    statement_spans: Vec<Option<SourceSpan>>,
    expression_spans: Vec<Option<SourceSpan>>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self::with_mode(tokens, false)
    }

    pub fn new_compact(tokens: Vec<Token>) -> Self {
        Self::with_mode(tokens, true)
    }

    pub fn new_spanned(tokens: Vec<SpannedToken>) -> Self {
        Self::with_spanned_mode(tokens, false)
    }

    pub fn new_compact_spanned(tokens: Vec<SpannedToken>) -> Self {
        Self::with_spanned_mode(tokens, true)
    }

    fn with_spanned_mode(tokens: Vec<SpannedToken>, compact: bool) -> Self {
        let (plain, spans): (Vec<Token>, Vec<SourceSpan>) = tokens.into_iter()
            .map(|item| (item.token, item.span)).unzip();
        let mut parser = Self::with_mode(plain, compact);
        parser.spans = Some(spans);
        parser
    }

    fn with_mode(tokens: Vec<Token>, compact: bool) -> Self {
        let mut precedences = HashMap::new();
        precedences.insert(Token::Plus, Precedence::Sum);
        precedences.insert(Token::Minus, Precedence::Sum);
        precedences.insert(Token::Star, Precedence::Product);
        precedences.insert(Token::Slash, Precedence::Product);
        precedences.insert(Token::Less, Precedence::Comparison);
        precedences.insert(Token::Greater, Precedence::Comparison);
        precedences.insert(Token::Tilde, Precedence::Comparison);
        precedences.insert(Token::Caret, Precedence::Comparison);
        // Do NOT register Question or Colon here: ternary is handled by a separate
        // check after the infix loop, not as infix operators.

        Parser {
            tokens,
            position: 0,
            precedences,
            compact,
            expression_depth: 0,
            statement_depth: 0,
            spans: None,
            statement_spans: Vec::new(),
            expression_spans: Vec::new(),
        }
    }

    // Legacy API: callers that do not need diagnostics keep the original AST.
    pub fn parse_program(&mut self) -> Result<Program, String> {
        self.parse_program_with_spans().map(|(program, _)| program)
    }

    /// Parse the legacy AST and return source locations in a parallel sidecar.
    /// Statement/expression entries are recorded in pre-order, without changing
    /// public AST node constructors or their equality semantics.
    pub fn parse_program_with_spans(&mut self) -> Result<(Program, AstSpanMap), String> {
        match self.parse_program_inner() {
            Ok(program) => Ok((program, AstSpanMap {
                statements: std::mem::take(&mut self.statement_spans),
                expressions: std::mem::take(&mut self.expression_spans),
            })),
            Err(error) => {
                if let Some(span) = self.spans.as_ref().and_then(|items| items.get(self.position)) {
                    Err(format!("{} (línea {}, columna {}, rango {}..{})",
                        error, span.line, span.column, span.start, span.end))
                } else {
                    Err(error)
                }
            }
        }
    }

    fn span_for_tokens(&self, start: usize, end: usize) -> Option<SourceSpan> {
        let spans = self.spans.as_ref()?;
        let first = *spans.get(start)?;
        let last_index = if end > start { end - 1 } else { start };
        let last = *spans.get(last_index)?;
        Some(SourceSpan {
            start: first.start,
            end: last.end,
            line: first.line,
            column: first.column,
            end_line: last.end_line,
            end_column: last.end_column,
        })
    }

    fn parse_program_inner(&mut self) -> Result<Program, String> {
        let mut statements = Vec::new();
        while !self.is_at_end() {
            if *self.current_token() == Token::Semicolon {
                self.advance();
                continue;
            }
            statements.push(self.parse_statement()?);
        }
        Ok(Program { statements })
    }

    fn parse_statement(&mut self) -> Result<Statement, String> {
        if self.statement_depth >= MAX_PARSE_DEPTH {
            return Err(format!("Límite de anidamiento de sentencias excedido ({MAX_PARSE_DEPTH})"));
        }
        let span_slot = self.statement_spans.len();
        let start = self.position;
        self.statement_spans.push(None);
        self.statement_depth += 1;
        let result = self.parse_statement_inner();
        self.statement_depth -= 1;
        self.statement_spans[span_slot] = self.span_for_tokens(start, self.position);
        result
    }

    fn parse_statement_inner(&mut self) -> Result<Statement, String> {
        // First, try to handle Identifier(name) without a long-lived match borrow.
        // We check for the Identifier case here to avoid the borrow-checker conflict
        // between the match on self.current_token() and mutable self calls.
        if let Token::Identifier(name) = self.current_token() {
            let ident_name = name.clone();
            let next = self.peek_ahead(1).cloned();
            if next == Some(Token::DeclareAssign) {
                self.advance();
                self.advance();
                let value = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() == Token::Semicolon { self.advance(); }
                return Ok(Statement::Let(LetStatement { name: ident_name, value }));
            }
            if next == Some(Token::Equals) {
                self.advance(); // consume the identifier
                self.advance(); // consume =
                let value = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() == Token::Semicolon { self.advance(); }
                return Ok(Statement::Assign(AssignStatement { name: ident_name, value }));
            }
            if next == Some(Token::BracketOpen) {
                self.advance(); // consume identifier
                let mut indices = Vec::new();
                while *self.current_token() == Token::BracketOpen {
                    self.advance(); // consume [
                    let index = self.parse_expression(Precedence::Lowest)?;
                    if *self.current_token() != Token::BracketClose {
                        return Err("Se esperaba ']' en la asignación por índice".to_string());
                    }
                    self.advance();
                    indices.push(index);
                }
                if *self.current_token() != Token::Equals {
                    return Err("Se esperaba '=' después de la ruta de índices".to_string());
                }
                self.advance();
                let value = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() == Token::Semicolon { self.advance(); }
                return Ok(Statement::IndexAssign(IndexAssignStatement { name: ident_name, indices, value }));
            }
        }

        match self.current_token() {
            Token::Let => self.parse_let_statement(),
            Token::Print => self.parse_print_statement(),
            Token::Return => self.parse_return_statement(),
            Token::Break => self.parse_loop_control_statement(false),
            Token::Continue => self.parse_loop_control_statement(true),
            Token::ParenOpen if !self.compact => self.parse_block_statement(),
            Token::BraceOpen if self.compact => self.parse_block_statement(),
            Token::While => self.parse_while_statement(),
            Token::If => self.parse_if_statement(),
            Token::Identifier(name) => {
                // Called a function: func(args) or bare identifier: foo
                let ident = name.clone();
                self.advance();
                if *self.current_token() == Token::ParenOpen {
                    self.advance();
                    let args = self.parse_comma_list(Token::ParenClose)?;
                    if *self.current_token() != Token::ParenClose {
                        return Err("Se esperaba ')' al cerrar la llamada".to_string());
                    }
                    self.advance(); // consume )
                    if *self.current_token() == Token::Semicolon {
                        self.advance(); // consume ;
                    }
                    let call_expr = Expression::FunctionCall {
                        func: Box::new(Expression::Identifier(ident.clone())),
                        args,
                    };
                    Ok(Statement::Let(LetStatement {
                        name: format!("_temp_{ident}"),
                        value: call_expr,
                    }))
                } else {
                    let id_clone = ident.clone();
                    Ok(Statement::Let(LetStatement {
                        name: id_clone.clone(),
                        value: Expression::Identifier(id_clone),
                    }))
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

    fn parse_loop_control_statement(&mut self, is_continue: bool) -> Result<Statement, String> {
        self.advance();
        if *self.current_token() == Token::Semicolon {
            self.advance();
        }
        if is_continue {
            Ok(Statement::Continue)
        } else {
            Ok(Statement::Break)
        }
    }

    fn parse_return_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'return'
        let value = self.parse_expression(Precedence::Lowest)?;
        if *self.current_token() == Token::Semicolon {
            self.advance();
        }
        Ok(Statement::Return(value))
    }

    fn parse_print_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'print'
        let value = self.parse_expression(Precedence::Lowest)?;
        if *self.current_token() == Token::Semicolon {
            self.advance();
        }
        Ok(Statement::Print(PrintStatement { value }))
    }

    fn block_delimiters(&self) -> (Token, Token) {
        if self.compact { (Token::BraceOpen, Token::BraceClose) }
        else { (Token::ParenOpen, Token::ParenClose) }
    }

    // Legacy uses parentheses both for grouped expressions and statement blocks.
    // In lambda bodies, distinguish them before choosing the parser path.
    fn legacy_lambda_body_is_block(&self) -> bool {
        if self.compact || *self.current_token() != Token::ParenOpen {
            return false;
        }

        let start = self.position + 1;
        // Legacy lambda blocks are sometimes wrapped in an extra grouping pair:
        // |x| ((let value = x; return value;)). Skip only leading opening parens
        // when identifying the first statement token.
        let mut first_index = start;
        while matches!(self.tokens.get(first_index), Some(Token::ParenOpen)) {
            first_index += 1;
        }
        let first = match self.tokens.get(first_index) {
            Some(token) => token,
            None => return false,
        };

        if matches!(
            first,
            Token::Let | Token::Print | Token::Return | Token::Break
                | Token::Continue | Token::If | Token::While
        ) {
            return true;
        }

        if matches!(first, Token::Identifier(_)) {
            match self.tokens.get(first_index + 1) {
                Some(Token::Equals) => return true,
                Some(Token::BracketOpen) => {
                    let mut depth = 0usize;
                    for (offset, token) in self.tokens.iter().enumerate().skip(first_index + 1) {
                        match token {
                            Token::BracketOpen => depth += 1,
                            Token::BracketClose => {
                                depth = depth.saturating_sub(1);
                                if depth == 0 {
                                    return matches!(self.tokens.get(offset + 1), Some(Token::Equals));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // A semicolon anywhere before the matching outer ')' cannot belong to
        // a valid grouped expression, but can separate statements in a block.
        let mut paren_depth = 0usize;
        for token in self.tokens.iter().skip(start) {
            match token {
                Token::ParenOpen => paren_depth += 1,
                Token::ParenClose if paren_depth == 0 => return false,
                Token::ParenClose => paren_depth = paren_depth.saturating_sub(1),
                Token::Semicolon => return true,
                _ => {}
            }
        }
        false
    }

    fn parse_block_statement(&mut self) -> Result<Statement, String> {
        let (open, close) = self.block_delimiters();
        if *self.current_token() != open { return Err(format!("Se esperaba '{open:?}' al abrir el bloque")); }
        self.advance();
        let stmts = self.parse_block()?;
        if *self.current_token() != close { return Err(format!("Se esperaba '{close:?}' al cerrar el bloque")); }
        self.advance();
        Ok(Statement::Block(stmts))
    }

    fn parse_control_condition(&mut self, keyword: &str) -> Result<Expression, String> {
        if self.compact {
            let condition = self.parse_expression(Precedence::Lowest)?;
            if *self.current_token() != Token::BraceOpen {
                return Err(format!("Se esperaba '{{' después de la condición de {keyword}"));
            }
            Ok(condition)
        } else {
            if *self.current_token() != Token::ParenOpen { return Err(format!("Se esperaba '(' antes de la condición de {keyword}")); }
            self.advance();
            let condition = self.parse_expression(Precedence::Lowest)?;
            if *self.current_token() != Token::ParenClose { return Err(format!("Se esperaba ')' después de la condición de {keyword}")); }
            self.advance();
            Ok(condition)
        }
    }

    fn parse_control_body(&mut self, keyword: &str) -> Result<Vec<Statement>, String> {
        let (open, close) = self.block_delimiters();
        if *self.current_token() != open { return Err(format!("Se esperaba '{open:?}' antes del cuerpo de {keyword}")); }
        self.advance();
        let body = self.parse_block()?;
        if *self.current_token() != close { return Err(format!("Se esperaba '{close:?}' al final del cuerpo de {keyword}")); }
        self.advance();
        Ok(body)
    }

    fn parse_while_statement(&mut self) -> Result<Statement, String> {
        self.advance();
        let condition = self.parse_control_condition("while")?;
        let body = self.parse_control_body("while")?;
        Ok(Statement::While(WhileStatement { condition, body }))
    }

    fn parse_if_statement(&mut self) -> Result<Statement, String> {
        self.advance();
        let condition = self.parse_control_condition("if")?;
        let true_body = self.parse_control_body("if")?;
        let false_body = if *self.current_token() == Token::Identifier("else".to_string()) {
            self.advance();
            Some(self.parse_control_body("else")?)
        } else { None };
        Ok(Statement::If(IfStatement { condition, true_body, false_body }))
    }

    // --- Sentencia Lambda: |x, y| { cuerpo } ---
    fn parse_lambda_statement(&mut self) -> Result<Statement, String> {
        let lambda = self.parse_lambda_expression()?;
        if *self.current_token() == Token::Semicolon {
            self.advance();
        }
        let params = match &lambda {
            Expression::Lambda { params, .. } => params,
            _ => unreachable!("parse_lambda_expression always returns a Lambda"),
        };
        let name = format!("__lambda_{}", params.join("_"));
        Ok(Statement::Let(LetStatement { name, value: lambda }))
    }

    // Único punto de parsing para lambdas usadas como expresión o sentencia.
    fn parse_lambda_expression(&mut self) -> Result<Expression, String> {
        self.advance(); // consume primer |
        let params = self.parse_params()?;
        if !self.is_at_end() && *self.current_token() != Token::Pipe {
            return Err("Se esperaba '|' después de los parámetros".to_string());
        }
        self.advance(); // consume segundo |

        let (block_open, block_close) = self.block_delimiters();
        let is_block = if self.compact {
            *self.current_token() == block_open
        } else {
            self.legacy_lambda_body_is_block()
        };
        let body = if is_block {
            self.advance();
            let statements = self.parse_block()?;
            if *self.current_token() != block_close {
                return Err(format!("Se esperaba '{block_close:?}' al cerrar el cuerpo de la función"));
            }
            self.advance();
            Expression::Block(statements)
        } else {
            self.parse_expression(Precedence::Lowest)?
        };

        Ok(Expression::Lambda { params, body: Box::new(body) })
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
        if self.expression_depth >= MAX_PARSE_DEPTH {
            return Err(format!("Límite de profundidad de expresiones excedido ({MAX_PARSE_DEPTH})"));
        }
        let root_expression = self.expression_depth == 0;
        let span_slot = if root_expression {
            let slot = self.expression_spans.len();
            self.expression_spans.push(None);
            Some(slot)
        } else {
            None
        };
        let start = self.position;
        self.expression_depth += 1;
        let result = self.parse_expression_inner(precedence);
        self.expression_depth -= 1;
        if let Some(slot) = span_slot {
            self.expression_spans[slot] = self.span_for_tokens(start, self.position);
        }
        result
    }

    fn parse_expression_inner(&mut self, precedence: Precedence) -> Result<Expression, String> {
        // Parsea la parte "prefijo" (un número, una variable, un paréntesis)
        let mut left_expr = self.parse_prefix()?;

        // Operaciones postfix: indexación y llamadas sobre cualquier expresión.
        loop {
            if *self.current_token() == Token::BracketOpen {
                self.advance();
                let index = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() != Token::BracketClose {
                    return Err("Se esperaba ']' después del índice".to_string());
                }
                self.advance();
                left_expr = Expression::Index {
                    target: Box::new(left_expr),
                    index: Box::new(index),
                };
            } else if *self.current_token() == Token::ParenOpen {
                self.advance();
                let args = self.parse_comma_list(Token::ParenClose)?;
                if *self.current_token() != Token::ParenClose {
                    return Err("Se esperaba ')' al cerrar la llamada".to_string());
                }
                self.advance();
                left_expr = Expression::FunctionCall {
                    func: Box::new(left_expr),
                    args,
                };
            } else {
                break;
            }
        }

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
            if *self.current_token() != Token::Colon {
                return Err("Se esperaba ':' entre las ramas de la expresión ternaria".to_string());
            }
            self.advance(); // consume :
            let false_branch = self.parse_expression(Precedence::Lowest)?;
            left_expr = Expression::Ternary {
                condition: Box::new(left_expr),
                true_branch: Box::new(true_branch),
                false_branch: Box::new(false_branch),
            };
        }

        Ok(left_expr)
    }

    // Parsea los elementos básicos de una expresión
    fn parse_prefix(&mut self) -> Result<Expression, String> {
        let token = self.current_token().clone();
        match token {
            Token::True => {
                self.advance();
                Ok(Expression::BoolLiteral(true))
            }
            Token::False => {
                self.advance();
                Ok(Expression::BoolLiteral(false))
            }
            Token::Identifier(name) => {
                self.advance();
                // Verificar si es llamada a función
                if !self.is_at_end() && *self.current_token() == Token::ParenOpen {
                    self.advance();
                    let args = self.parse_comma_list(Token::ParenClose)?;
                    if *self.current_token() != Token::ParenClose {
                        return Err("Se esperaba ')' al cerrar la llamada".to_string());
                    }
                    self.advance();
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
                if *self.current_token() != Token::ParenClose {
                    return Err("Se esperaba ')' al cerrar la expresión agrupada".to_string());
                }
                self.advance();
                Ok(expr)
            }
            Token::Pipe => {
                self.parse_lambda_expression()
            }
            Token::BracketOpen => {
                self.advance();
                let items = self.parse_comma_list(Token::BracketClose)?;
                if *self.current_token() != Token::BracketClose {
                    return Err("Se esperaba ']' al cerrar el array".to_string());
                }
                self.advance();
                Ok(Expression::Array(items))
            }
            Token::At => {
                self.advance();
                let (open, close) = if self.compact { (Token::BraceOpen, Token::BraceClose) } else { (Token::ParenOpen, Token::ParenClose) };
                if *self.current_token() != open { return Err(format!("Se esperaba '{open:?}' después de '@'")); }
                self.advance();
                let pairs = self.parse_map_entries_with_close(close.clone())?;
                if *self.current_token() != close { return Err(format!("Se esperaba '{close:?}' al cerrar el mapa")); }
                self.advance();
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
        self.parse_map_entries_with_close(Token::ParenClose)
    }

    fn parse_map_entries_with_close(&mut self, close: Token) -> Result<Vec<(Expression, Expression)>, String> {
        let mut items = Vec::new();
        while !self.is_at_end() && *self.current_token() != close {
            if *self.current_token() == Token::Comma {
                self.advance();
                continue;
            }
            let key = self.parse_expression(Precedence::Lowest)?;
            if *self.current_token() != Token::Colon {
                return Err(format!("Se esperaba ':' después de '{:?}'", key));
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
        let (_, close) = self.block_delimiters();
        while !self.is_at_end() && *self.current_token() != close {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Program {
        let mut lexer = crate::lexer::Lexer::new(source.to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        parser.parse_program().unwrap()
    }

    #[test]
    fn test_parse_let_number() {
        let program = parse("let x = 42;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let(stmt) => {
                assert_eq!(stmt.name, "x");
                assert_eq!(stmt.value, Expression::NumberLiteral(42.0));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_let_string() {
        let program = parse(r#"let s = "hello";"#);
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let(stmt) => {
                assert_eq!(stmt.name, "s");
                assert_eq!(stmt.value, Expression::StringLiteral("hello".to_string()));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_let_from_identifier() {
        // Assignment is distinct from a declaration and updates an existing binding.
        let program = parse("x = 10;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Assign(stmt) => {
                assert_eq!(stmt.name, "x");
                assert_eq!(stmt.value, Expression::NumberLiteral(10.0));
            }
            other => panic!("expected Assign, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_print() {
        let program = parse("print 42;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Print(stmt) => {
                assert_eq!(stmt.value, Expression::NumberLiteral(42.0));
            }
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_while() {
        let program = parse("while (i < 10) ( print i; )");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::While(stmt) => {
                assert_eq!(
                    stmt.condition,
                    Expression::Infix {
                        left: Box::new(Expression::Identifier("i".to_string())),
                        operator: Token::Less,
                        right: Box::new(Expression::NumberLiteral(10.0)),
                    }
                );
                assert_eq!(stmt.body.len(), 1);
                assert!(matches!(stmt.body[0], Statement::Print(_)));
            }
            other => panic!("expected While, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_if() {
        let program = parse("if (a > b) ( print a; )");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::If(stmt) => {
                assert!(matches!(stmt.condition, Expression::Infix { .. }));
                assert_eq!(stmt.true_body.len(), 1);
                assert!(stmt.false_body.is_none());
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_if_else() {
        let program = parse("if (a > b) ( print a; ) else ( print b; )");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::If(stmt) => {
                assert_eq!(stmt.true_body.len(), 1);
                assert!(stmt.false_body.is_some());
                assert_eq!(stmt.false_body.as_ref().unwrap().len(), 1);
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_ternary_in_let() {
        let program = parse("let x = (a > b) ? a : b;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let(stmt) => {
                assert!(matches!(stmt.value, Expression::Ternary { .. }));
                if let Expression::Ternary { ref condition, ref true_branch, ref false_branch } = stmt.value {
                    assert!(matches!(condition.as_ref(), Expression::Infix { .. }));
                    assert!(matches!(true_branch.as_ref(), Expression::Identifier(name) if name == "a"));
                    assert!(matches!(false_branch.as_ref(), Expression::Identifier(name) if name == "b"));
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn legacy_lambda_parenthesized_expression_is_not_misparsed_as_block() {
        let program = parse("let add = |x, y| (x + y);");
        match &program.statements[0] {
            Statement::Let(stmt) => match &stmt.value {
                Expression::Lambda { params, body } => {
                    assert_eq!(params, &vec!["x".to_string(), "y".to_string()]);
                    assert!(matches!(body.as_ref(), Expression::Infix { operator: Token::Plus, .. }));
                }
                other => panic!("expected Lambda, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn legacy_lambda_nested_parenthesized_statement_block_still_parses() {
        let program = parse("let factory = |x| ((let inner = |y| x + y; return inner;));");
        match &program.statements[0] {
            Statement::Let(stmt) => match &stmt.value {
                Expression::Lambda { body, .. } => {
                    assert!(matches!(
                        body.as_ref(),
                        Expression::Block(statements)
                            if statements.len() == 1
                                && matches!(&statements[0], Statement::Block(inner) if inner.len() == 2)
                    ));
                }
                other => panic!("expected Lambda, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn legacy_lambda_parenthesized_statement_block_still_parses() {
        let program = parse("let get = |x| (let y = x; return y;);");
        match &program.statements[0] {
            Statement::Let(stmt) => match &stmt.value {
                Expression::Lambda { body, .. } => {
                    assert!(matches!(body.as_ref(), Expression::Block(statements) if statements.len() == 2));
                }
                other => panic!("expected Lambda, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_lambda() {
        let program = parse("let add = |x, y| x + y;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let(stmt) => {
                assert_eq!(stmt.name, "add");
                assert!(matches!(stmt.value, Expression::Lambda { .. }));
                if let Expression::Lambda { ref params, ref body } = stmt.value {
                    assert_eq!(params.len(), 2);
                    assert_eq!(params[0], "x");
                    assert_eq!(params[1], "y");
                    assert!(matches!(
                        body.as_ref(),
                        Expression::Infix { operator: Token::Plus, .. }
                    ));
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_call_on_parenthesized_expression() {
        let program = parse("let result = (factory)(2);");
        match &program.statements[0] {
            Statement::Let(stmt) => match &stmt.value {
                Expression::FunctionCall { func, args } => {
                    assert_eq!(func.as_ref(), &Expression::Identifier("factory".to_string()));
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected FunctionCall, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_function_call() {
        // Parser wraps f(...) in a Let with name _temp_f
        let program = parse("add(1, 2);");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let(stmt) => {
                assert!(stmt.name.starts_with("_temp_"));
                assert!(matches!(stmt.value, Expression::FunctionCall { .. }));
                if let Expression::FunctionCall { ref func, ref args } = stmt.value {
                    assert!(matches!(func.as_ref(), Expression::Identifier(name) if name == "add"));
                    assert_eq!(args.len(), 2);
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_grouped_expression_in_block() {
        // (2 + 3) * 4; → inside a block → parsed as block
        // Actually bare infix is handled differently — test via let
        let program = parse("let x = (2 + 3) * 4;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let(stmt) => {
                // (2 + 3) * 4 should be Infix { *, left: Infix { +, 2, 3 }, right: 4 }
                assert!(matches!(stmt.value, Expression::Infix { operator: Token::Star, .. }));
                if let Expression::Infix { ref left, operator: _, ref right } = stmt.value {
                    assert!(matches!(left.as_ref(), Expression::Infix { operator: Token::Plus, .. }));
                    assert_eq!(right.as_ref(), &Expression::NumberLiteral(4.0));
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_precedence_via_let() {
        // 2 + 3 * 4 → 2 + (3 * 4)
        let program = parse("let x = 2 + 3 * 4;");
        match &program.statements[0] {
            Statement::Let(stmt) => {
                assert!(matches!(stmt.value, Expression::Infix { operator: Token::Plus, .. }));
                if let Expression::Infix { ref left, operator: _, ref right } = stmt.value {
                    assert_eq!(left.as_ref(), &Expression::NumberLiteral(2.0));
                    assert!(matches!(right.as_ref(), Expression::Infix { operator: Token::Star, .. }));
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_block() {
        let program = parse("( let x = 1; let y = 2; )");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Block(stmts) => {
                assert_eq!(stmts.len(), 2);
            }
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_empty_input() {
        let program = parse("");
        assert_eq!(program.statements.len(), 0);
    }

    fn parse_compact(source: &str) -> Program {
        let mut lexer = crate::lexer::Lexer::new(source.to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new_compact(tokens);
        parser.parse_program().unwrap()
    }

    #[test]
    fn compact_mode_parses_declarations_and_brace_blocks() {
        let program = parse_compact("n:=3 while n>0{n=n-1}");
        assert!(matches!(program.statements[0], Statement::Let(_)));
        assert!(matches!(program.statements[1], Statement::While(_)));
    }

    #[test]
    fn compact_mode_parses_if_else_and_lambda_blocks() {
        let program = parse_compact("f:=|x|{if x>0{return x}else{return 0}}");
        match &program.statements[0] {
            Statement::Let(stmt) => assert!(matches!(stmt.value, Expression::Lambda { .. })),
            other => panic!("expected compact lambda declaration, got {:?}", other),
        }
    }

    #[test]
    fn compact_mode_parses_map_literal_with_braces() {
        let program = parse_compact("data:=@{\"x\":1,\"y\":2}");
        match &program.statements[0] {
            Statement::Let(stmt) => assert!(matches!(stmt.value, Expression::Map(_))),
            other => panic!("expected compact map declaration, got {:?}", other),
        }
    }

    #[test]
    fn legacy_mode_still_parses_parenthesis_blocks() {
        let program = parse("while (i < 2) ( print i; )");
        assert!(matches!(program.statements[0], Statement::While(_)));
    }

    #[test]
    fn deeply_nested_expression_returns_error() {
        let source = format!("let x = {}1{};", "(".repeat(300), ")".repeat(300));
        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("Límite de profundidad"));
    }

    #[test]
    fn deeply_nested_blocks_return_error() {
        let source = format!("{}print 1;{}", "(".repeat(300), ")".repeat(300));
        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("Límite de anidamiento"));
    }

    #[test]
    fn parser_returns_statement_and_expression_span_sidecar() {
        let mut lexer = crate::lexer::Lexer::new("\nlet result = true + 2;".to_string());
        let tokens = lexer.tokenize_spanned().unwrap();
        let mut parser = Parser::new_spanned(tokens);
        let (program, spans) = parser.parse_program_with_spans().unwrap();
        assert_eq!(program.statements.len(), 1);
        assert_eq!(spans.statements.len(), 1);
        assert_eq!(spans.expressions.len(), 1);
        let statement = spans.statements[0].unwrap();
        let expression = spans.expressions[0].unwrap();
        assert_eq!((statement.line, statement.column), (2, 1));
        assert_eq!((expression.line, expression.column), (2, 13));
        assert_eq!(expression.end, 21);
    }

    #[test]
    fn spanned_parser_reports_line_and_column() {
        let mut lexer = crate::lexer::Lexer::new("\nlet = 1;".to_string());
        let tokens = lexer.tokenize_spanned().unwrap();
        let mut parser = Parser::new_spanned(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("línea 2, columna 5"), "{}", error);
        assert!(error.contains("rango 5..6"), "{}", error);
    }

    #[test]
    fn spanned_parser_reports_missing_grouping_parenthesis_at_eof() {
        let mut lexer = crate::lexer::Lexer::new("\nlet value = (1 + 2".to_string());
        let tokens = lexer.tokenize_spanned().unwrap();
        let mut parser = Parser::new_spanned(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("Se esperaba ')' al cerrar la expresión agrupada"), "{error}");
        assert!(error.contains("línea 2, columna"), "{error}");
    }

    #[test]
    fn spanned_parser_reports_unclosed_statement_call_at_eof() {
        let mut lexer = crate::lexer::Lexer::new("\nprocess(1".to_string());
        let tokens = lexer.tokenize_spanned().unwrap();
        let mut parser = Parser::new_spanned(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("Se esperaba ')' al cerrar la llamada"), "{error}");
        assert!(error.contains("línea 2, columna"), "{error}");
    }

    #[test]
    fn spanned_parser_rejects_ternary_without_colon() {
        let mut lexer = crate::lexer::Lexer::new("\nlet value = true ? 1;".to_string());
        let tokens = lexer.tokenize_spanned().unwrap();
        let mut parser = Parser::new_spanned(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("Se esperaba ':' entre las ramas de la expresión ternaria"), "{error}");
        assert!(error.contains("línea 2, columna"), "{error}");
    }

    #[test]
    fn test_parse_unexpected_token_returns_error() {
        let mut lexer = crate::lexer::Lexer::new("let = 42;".to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("nombre de variable"));
    }
}
