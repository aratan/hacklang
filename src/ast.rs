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
