// src/ast.rs
// Este módulo define la estructura de datos que representa nuestro código.
// Es el "puente" entre el Parser y el Evaluador.
//
// Spans: cada nodo puede conservar un SourceSpan opcional. Cuando el parser
// se construye con tokens spanned, los rellena. Los constructores antiguos
// (tests, formatter, etc.) pueden dejar span = None sin romper la API.

use crate::lexer::{SourceSpan, Token};

/// Programa = lista de sentencias + span opcional del archivo completo.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub span: Option<SourceSpan>,
}

impl Program {
    pub fn new(statements: Vec<Statement>) -> Self {
        Self {
            statements,
            span: None,
        }
    }
}

/// Sentencias. Las variantes con payload llevan el span dentro de su struct.
/// Break / Continue llevan el span directamente.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Let(LetStatement),
    Assign(AssignStatement),
    IndexAssign(IndexAssignStatement),
    Print(PrintStatement),
    Return(ReturnStatement),
    Break(BreakStatement),
    Continue(ContinueStatement),
    Block(BlockStatement),
    While(WhileStatement),
    If(IfStatement),
}

impl Statement {
    /// Devuelve el span del nodo si está disponible.
    pub fn span(&self) -> Option<SourceSpan> {
        match self {
            Statement::Let(s) => s.span,
            Statement::Assign(s) => s.span,
            Statement::IndexAssign(s) => s.span,
            Statement::Print(s) => s.span,
            Statement::Return(s) => s.span,
            Statement::Break(s) => s.span,
            Statement::Continue(s) => s.span,
            Statement::Block(s) => s.span,
            Statement::While(s) => s.span,
            Statement::If(s) => s.span,
        }
    }
}

/// Expresiones. Cada variante con datos lleva su propio span.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Identifier {
        name: String,
        span: Option<SourceSpan>,
    },
    NumberLiteral {
        value: f64,
        span: Option<SourceSpan>,
    },
    BoolLiteral {
        value: bool,
        span: Option<SourceSpan>,
    },
    StringLiteral {
        value: String,
        span: Option<SourceSpan>,
    },
    Infix {
        left: Box<Expression>,
        operator: Token,
        right: Box<Expression>,
        span: Option<SourceSpan>,
    },
    Ternary {
        condition: Box<Expression>,
        true_branch: Box<Expression>,
        false_branch: Box<Expression>,
        span: Option<SourceSpan>,
    },
    Lambda {
        params: Vec<String>,
        body: Box<Expression>,
        span: Option<SourceSpan>,
    },
    /// Bloque de varias sentencias usado como cuerpo de una función.
    Block {
        statements: Vec<Statement>,
        span: Option<SourceSpan>,
    },
    FunctionCall {
        func: Box<Expression>,
        args: Vec<Expression>,
        span: Option<SourceSpan>,
    },
    Array {
        elements: Vec<Expression>,
        span: Option<SourceSpan>,
    },
    Map {
        entries: Vec<(Expression, Expression)>,
        span: Option<SourceSpan>,
    },
    Index {
        target: Box<Expression>,
        index: Box<Expression>,
        span: Option<SourceSpan>,
    },
}

impl Expression {
    pub fn span(&self) -> Option<SourceSpan> {
        match self {
            Expression::Identifier { span, .. }
            | Expression::NumberLiteral { span, .. }
            | Expression::BoolLiteral { span, .. }
            | Expression::StringLiteral { span, .. }
            | Expression::Infix { span, .. }
            | Expression::Ternary { span, .. }
            | Expression::Lambda { span, .. }
            | Expression::Block { span, .. }
            | Expression::FunctionCall { span, .. }
            | Expression::Array { span, .. }
            | Expression::Map { span, .. }
            | Expression::Index { span, .. } => *span,
        }
    }

    // --- Constructores de conveniencia (span = None) para código existente ---

    pub fn identifier(name: impl Into<String>) -> Self {
        Expression::Identifier {
            name: name.into(),
            span: None,
        }
    }

    pub fn number(value: f64) -> Self {
        Expression::NumberLiteral { value, span: None }
    }

    pub fn bool_lit(value: bool) -> Self {
        Expression::BoolLiteral { value, span: None }
    }

    pub fn string(value: impl Into<String>) -> Self {
        Expression::StringLiteral {
            value: value.into(),
            span: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Estructuras detalladas de sentencias
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct LetStatement {
    pub name: String,
    pub value: Expression,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssignStatement {
    pub name: String,
    pub value: Expression,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexAssignStatement {
    /// Base variable being indexed (leftmost identifier).
    pub name: String,
    /// Chain of index expressions, e.g. `a[i][j]` => indices = [i, j].
    /// Each index is evaluated once, left to right.
    pub indices: Vec<Expression>,
    pub value: Expression,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrintStatement {
    pub value: Expression,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnStatement {
    pub value: Expression,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BreakStatement {
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContinueStatement {
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockStatement {
    pub statements: Vec<Statement>,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileStatement {
    pub condition: Expression,
    pub body: Vec<Statement>,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStatement {
    pub condition: Expression,
    pub true_body: Vec<Statement>,
    pub false_body: Option<Vec<Statement>>,
    pub span: Option<SourceSpan>,
}
