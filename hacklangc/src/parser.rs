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
        // Do NOT register Question or Colon here: ternary is handled by a separate
        // check after the infix loop, not as infix operators.

        Parser {
            tokens,
            position: 0,
            precedences,
        }
    }

    // El método principal que construye el árbol completo.
    pub fn parse_program(&mut self) -> Result<Program, String> {
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
        // First, try to handle Identifier(name) without a long-lived match borrow.
        // We check for the Identifier case here to avoid the borrow-checker conflict
        // between the match on self.current_token() and mutable self calls.
        if let Token::Identifier(name) = self.current_token() {
            let ident_name = name.clone();
            let next = self.peek_ahead(1).cloned();
            if next == Some(Token::Equals) {
                self.advance(); // consume the identifier
                self.advance(); // consume =
                let value = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() == Token::Semicolon { self.advance(); }
                return Ok(Statement::Assign(AssignStatement { name: ident_name, value }));
            }
            if next == Some(Token::BracketOpen) {
                self.advance(); // consume identifier
                self.advance(); // consume [
                let index = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() != Token::BracketClose {
                    return Err("Se esperaba ']' en la asignación por índice".to_string());
                }
                self.advance();
                if *self.current_token() != Token::Equals {
                    return Err("Se esperaba '=' después del índice".to_string());
                }
                self.advance();
                let value = self.parse_expression(Precedence::Lowest)?;
                if *self.current_token() == Token::Semicolon { self.advance(); }
                return Ok(Statement::IndexAssign(IndexAssignStatement { name: ident_name, index, value }));
            }
        }

        match self.current_token() {
            Token::Let => self.parse_let_statement(),
            Token::Print => self.parse_print_statement(),
            Token::Return => self.parse_return_statement(),
            Token::Break => self.parse_loop_control_statement(false),
            Token::Continue => self.parse_loop_control_statement(true),
            Token::ParenOpen => self.parse_block_statement(),
            Token::While => self.parse_while_statement(),
            Token::If => self.parse_if_statement(),
            Token::Identifier(name) => {
                // Called a function: func(args) or bare identifier: foo
                let ident = name.clone();
                self.advance();
                if *self.current_token() == Token::ParenOpen {
                    self.advance();
                    let args = self.parse_comma_list(Token::ParenClose)?;
                    if *self.current_token() == Token::ParenClose {
                        self.advance(); // consume )
                    }
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
        if *self.current_token() != Token::ParenOpen {
            return Err("Se esperaba '(' antes de la condición del while".to_string());
        }
        self.advance();
        let condition = self.parse_expression(Precedence::Lowest)?;
        if *self.current_token() != Token::ParenClose {
            return Err("Se esperaba ')' después de la condición del while".to_string());
        }
        self.advance();

        if *self.current_token() != Token::ParenOpen {
            return Err("Se esperaba '(' antes del cuerpo del while".to_string());
        }
        self.advance();
        let body = self.parse_block()?;
        if *self.current_token() != Token::ParenClose {
            return Err("Se esperaba ')' al final del while".to_string());
        }
        self.advance();
        Ok(Statement::While(WhileStatement { condition, body }))
    }

    fn parse_if_statement(&mut self) -> Result<Statement, String> {
        self.advance(); // consume 'if'
        if *self.current_token() != Token::ParenOpen {
            return Err("Se esperaba '(' antes de la condición del if".to_string());
        }
        self.advance();
        let condition = self.parse_expression(Precedence::Lowest)?;
        if *self.current_token() != Token::ParenClose {
            return Err("Se esperaba ')' después de la condición del if".to_string());
        }
        self.advance();

        if *self.current_token() != Token::ParenOpen {
            return Err("Se esperaba '(' antes del cuerpo del if".to_string());
        }
        self.advance();
        let true_body = self.parse_block()?;
        if *self.current_token() != Token::ParenClose {
            return Err("Se esperaba ')' al final del bloque if".to_string());
        }
        self.advance();

        let false_body = if *self.current_token() == Token::Identifier("else".to_string()) {
            self.advance();
            if *self.current_token() != Token::ParenOpen {
                return Err("Se esperaba '(' después de 'else'".to_string());
            }
            self.advance();
            let body = self.parse_block()?;
            if *self.current_token() != Token::ParenClose {
                return Err("Se esperaba ')' al final del bloque else".to_string());
            }
            self.advance();
            Some(body)
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

        let body = if *self.current_token() == Token::ParenOpen {
            self.advance();
            let statements = self.parse_block()?;
            if *self.current_token() != Token::ParenClose {
                return Err("Se esperaba ')' al cerrar el cuerpo de la función".to_string());
            }
            self.advance();
            Expression::Block(statements)
        } else {
            self.parse_expression(Precedence::Lowest)?
        };

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
            if *self.current_token() == Token::Colon {
                self.advance(); // consume :
                let false_branch = self.parse_expression(Precedence::Lowest)?;
                left_expr = Expression::Ternary {
                    condition: Box::new(left_expr),
                    true_branch: Box::new(true_branch),
                    false_branch: Box::new(false_branch),
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
                let body = if *self.current_token() == Token::ParenOpen {
                    self.advance();
                    let statements = self.parse_block()?;
                    if *self.current_token() != Token::ParenClose {
                        return Err("Se esperaba ')' al cerrar el cuerpo de la función".to_string());
                    }
                    self.advance();
                    Expression::Block(statements)
                } else {
                    self.parse_expression(Precedence::Lowest)?
                };
                Ok(Expression::Lambda { params, body: Box::new(body) })
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
                // Mapa: @{ "key": value }
                self.advance();
                if *self.current_token() != Token::ParenOpen {
                    return Err("Se esperaba '(' después de '@'".to_string());
                }
                self.advance();
                let pairs = self.parse_map_entries()?;
                if *self.current_token() != Token::ParenClose {
                    return Err("Se esperaba ')' al cerrar el mapa".to_string());
                }
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
        let mut items = Vec::new();
        while !self.is_at_end() && *self.current_token() != Token::ParenClose {
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

    #[test]
    fn test_parse_unexpected_token_returns_error() {
        let mut lexer = crate::lexer::Lexer::new("let = 42;".to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let error = parser.parse_program().unwrap_err();
        assert!(error.contains("nombre de variable"));
    }
}
