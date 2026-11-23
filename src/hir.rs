use crate::ast::{Expression as AstExpr, Program as AstProgram, Statement as AstStmt};
use crate::lexer::Token;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StmtId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp { Add, Sub, Mul, Div, Less, Greater, Equal, NotEqual }

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub scopes: Vec<Scope>,
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind { Module, Block, Branch, Loop }

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub id: BindingId,
    pub scope: ScopeId,
    pub name: String,
    pub mutable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub id: StmtId,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Let { binding: BindingId, value: Expression },
    Assign { binding: BindingId, value: Expression },
    IndexAssign { binding: BindingId, indices: Vec<Expression>, value: Expression },
    Print(Expression),
    Block { scope: ScopeId, statements: Vec<Statement> },
    If {
        condition: Expression,
        then_scope: ScopeId,
        then_body: Vec<Statement>,
        else_scope: Option<ScopeId>,
        else_body: Vec<Statement>,
    },
    While { scope: ScopeId, condition: Expression, body: Vec<Statement> },
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub id: ExprId,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Number(f64),
    Bool(bool),
    String(String),
    Read(BindingId),
    Binary { op: BinaryOp, left: Box<Expression>, right: Box<Expression> },
    Ternary {
        condition: Box<Expression>,
        when_true: Box<Expression>,
        when_false: Box<Expression>,
    },
    Array(Vec<Expression>),
    Map(Vec<(Expression, Expression)>),
    Index { target: Box<Expression>, index: Box<Expression> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LowerError {
    pub message: String,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LowerError {}

struct Lowerer {
    scopes: Vec<Scope>,
    bindings: Vec<Binding>,
    symbols: Vec<HashMap<String, BindingId>>,
    scope_stack: Vec<ScopeId>,
    next_expr: u32,
    next_stmt: u32,
    loop_depth: usize,
}

impl Lowerer {
    fn new() -> Self {
        Self {
            scopes: vec![Scope { id: ScopeId(0), parent: None, kind: ScopeKind::Module }],
            bindings: Vec::new(),
            symbols: vec![HashMap::new()],
            scope_stack: vec![ScopeId(0)],
            next_expr: 0,
            next_stmt: 0,
            loop_depth: 0,
        }
    }

    fn enter_scope(&mut self, kind: ScopeKind) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        let parent = self.current_scope();
        self.scopes.push(Scope { id, parent: Some(parent), kind });
        self.symbols.push(HashMap::new());
        self.scope_stack.push(id);
        id
    }

    fn leave_scope(&mut self) {
        if self.symbols.len() > 1 {
            self.symbols.pop();
            self.scope_stack.pop();
        }
    }

    fn current_scope(&self) -> ScopeId {
        *self.scope_stack.last().expect("module scope exists")
    }

    fn declare(&mut self, name: &str) -> BindingId {
        let id = BindingId(self.bindings.len() as u32);
        let scope = self.current_scope();
        self.bindings.push(Binding { id, scope, name: name.to_string(), mutable: true });
        self.symbols.last_mut().expect("module scope exists").insert(name.to_string(), id);
        id
    }

    fn resolve(&self, name: &str) -> Result<BindingId, LowerError> {
        self.symbols.iter().rev().find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| LowerError { message: format!("Variable no resuelta durante AST→HIR: '{}'", name) })
    }

    fn expr(&mut self, ast: &AstExpr) -> Result<Expression, LowerError> {
        let kind = match ast {
            AstExpr::NumberLiteral(value) => ExprKind::Number(*value),
            AstExpr::BoolLiteral(value) => ExprKind::Bool(*value),
            AstExpr::StringLiteral(value) => ExprKind::String(value.clone()),
            AstExpr::Identifier(name) => ExprKind::Read(self.resolve(name)?),
            AstExpr::Infix { left, operator, right } => {
                let op = match operator {
                    Token::Plus => BinaryOp::Add,
                    Token::Minus => BinaryOp::Sub,
                    Token::Star => BinaryOp::Mul,
                    Token::Slash => BinaryOp::Div,
                    Token::Less => BinaryOp::Less,
                    Token::Greater => BinaryOp::Greater,
                    Token::Tilde => BinaryOp::Equal,
                    Token::Caret => BinaryOp::NotEqual,
                    other => return Err(LowerError { message: format!("Operador no admitido en HIR inicial: {:?}", other) }),
                };
                let left = Box::new(self.expr(left)?);
                let right = Box::new(self.expr(right)?);
                ExprKind::Binary { op, left, right }
            }
            AstExpr::Ternary { condition, true_branch, false_branch } => ExprKind::Ternary {
                condition: Box::new(self.expr(condition)?),
                when_true: Box::new(self.expr(true_branch)?),
                when_false: Box::new(self.expr(false_branch)?),
            },
            AstExpr::Lambda { .. } => return Err(LowerError { message: "Las funciones y closures todavía no están soportadas por HIR nativa".into() }),
            AstExpr::FunctionCall { .. } => return Err(LowerError { message: "Las llamadas a funciones y builtins todavía no están soportadas por HIR nativa".into() }),
            AstExpr::Array(items) => ExprKind::Array(items.iter().map(|item| self.expr(item)).collect::<Result<_, _>>()?),
            AstExpr::Map(items) => ExprKind::Map(items.iter().map(|(key, value)| Ok((self.expr(key)?, self.expr(value)?))).collect::<Result<_, LowerError>>()?),
            AstExpr::Index { target, index } => ExprKind::Index { target: Box::new(self.expr(target)?), index: Box::new(self.expr(index)?) },
            AstExpr::Block(_) => return Err(LowerError { message: "Los bloques como expresión todavía no están soportados por HIR nativa".into() }),
        };
        let id = ExprId(self.next_expr);
        self.next_expr += 1;
        Ok(Expression { id, kind })
    }

    fn stmt(&mut self, ast: &AstStmt) -> Result<Statement, LowerError> {
        let kind = match ast {
            AstStmt::Let(binding) => {
                let value = self.expr(&binding.value)?;
                let binding = self.declare(&binding.name);
                StmtKind::Let { binding, value }
            }
            AstStmt::Assign(assignment) => {
                let value = self.expr(&assignment.value)?;
                let binding = self.resolve(&assignment.name)?;
                StmtKind::Assign { binding, value }
            }
            AstStmt::IndexAssign(assignment) => {
                let binding = self.resolve(&assignment.name)?;
                let indices = assignment.indices.iter()
                    .map(|index| self.expr(index))
                    .collect::<Result<Vec<_>, _>>()?;
                StmtKind::IndexAssign { binding, indices, value: self.expr(&assignment.value)? }
            }
            AstStmt::Print(print) => StmtKind::Print(self.expr(&print.value)?),
            AstStmt::Block(statements) => {
                let scope = self.enter_scope(ScopeKind::Block);
                let result = self.statements(statements);
                self.leave_scope();
                StmtKind::Block { scope, statements: result? }
            }
            AstStmt::If(branch) => {
                let condition = self.expr(&branch.condition)?;
                let then_scope = self.enter_scope(ScopeKind::Branch);
                let then_result = self.statements(&branch.true_body);
                self.leave_scope();
                let then_body = then_result?;
                let (else_scope, else_body) = if let Some(body) = &branch.false_body {
                    let scope = self.enter_scope(ScopeKind::Branch);
                    let result = self.statements(body);
                    self.leave_scope();
                    (Some(scope), result?)
                } else { (None, Vec::new()) };
                StmtKind::If { condition, then_scope, then_body, else_scope, else_body }
            }
            AstStmt::While(loop_stmt) => {
                let scope = self.enter_scope(ScopeKind::Loop);
                self.loop_depth += 1;
                let condition = self.expr(&loop_stmt.condition)?;
                let body_result = self.statements(&loop_stmt.body);
                self.loop_depth -= 1;
                self.leave_scope();
                StmtKind::While { scope, condition, body: body_result? }
            }
            AstStmt::Break => {
                if self.loop_depth == 0 {
                    return Err(LowerError { message: "break fuera de un bucle durante AST→HIR".into() });
                }
                StmtKind::Break
            }
            AstStmt::Continue => {
                if self.loop_depth == 0 {
                    return Err(LowerError { message: "continue fuera de un bucle durante AST→HIR".into() });
                }
                StmtKind::Continue
            }
            other => return Err(LowerError { message: format!("Sentencia todavía no admitida en HIR inicial: {:?}", other) }),
        };
        let id = StmtId(self.next_stmt);
        self.next_stmt += 1;
        Ok(Statement { id, kind })
    }

    fn statements(&mut self, ast: &[AstStmt]) -> Result<Vec<Statement>, LowerError> {
        ast.iter().map(|statement| self.stmt(statement)).collect()
    }
}

pub fn lower(program: &AstProgram) -> Result<Program, LowerError> {
    let mut lowerer = Lowerer::new();
    let statements = lowerer.statements(&program.statements)?;
    Ok(Program { statements, scopes: lowerer.scopes, bindings: lowerer.bindings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssignStatement, LetStatement, PrintStatement};

    fn ast(statements: Vec<AstStmt>) -> AstProgram { AstProgram { statements } }

    #[test]
    fn nested_scope_parent_is_correct_after_closing_a_sibling() {
        let program = ast(vec![
            AstStmt::Block(vec![]),
            AstStmt::Block(vec![
                AstStmt::Block(vec![]),
            ]),
        ]);
        let hir = lower(&program).unwrap();
        assert_eq!(hir.scopes[1].parent, Some(ScopeId(0)));
        assert_eq!(hir.scopes[2].parent, Some(ScopeId(0)));
        assert_eq!(hir.scopes[3].parent, Some(ScopeId(2)));
    }

    #[test]
    fn lowers_literals_and_variable_reads_to_binding_ids() {
        let program = ast(vec![
            AstStmt::Let(LetStatement { name: "answer".into(), value: AstExpr::NumberLiteral(42.0) }),
            AstStmt::Print(PrintStatement { value: AstExpr::Identifier("answer".into()) }),
        ]);
        let hir = lower(&program).unwrap();
        assert_eq!(hir.bindings.len(), 1);
        assert_eq!(hir.bindings[0].name, "answer");
        assert_eq!(hir.statements.len(), 2);
        assert!(matches!(&hir.statements[1].kind, StmtKind::Print(Expression { kind: ExprKind::Read(BindingId(0)), .. })));
    }

    #[test]
    fn assignment_resolves_the_nearest_lexical_binding() {
        let program = ast(vec![
            AstStmt::Let(LetStatement { name: "x".into(), value: AstExpr::NumberLiteral(1.0) }),
            AstStmt::Block(vec![
                AstStmt::Let(LetStatement { name: "x".into(), value: AstExpr::NumberLiteral(2.0) }),
                AstStmt::Assign(AssignStatement { name: "x".into(), value: AstExpr::NumberLiteral(3.0) }),
            ]),
            AstStmt::Assign(AssignStatement { name: "x".into(), value: AstExpr::NumberLiteral(4.0) }),
        ]);
        let hir = lower(&program).unwrap();
        let outer = hir.bindings[0].id;
        let inner = hir.bindings[1].id;
        assert_ne!(outer, inner);
        let StmtKind::Block { statements, .. } = &hir.statements[1].kind else { panic!("expected block"); };
        assert!(matches!(&statements[1].kind, StmtKind::Assign { binding, .. } if *binding == inner));
        assert!(matches!(&hir.statements[2].kind, StmtKind::Assign { binding, .. } if *binding == outer));
    }

    #[test]
    fn rejects_unresolved_variables_and_control_flow_outside_loops() {
        let missing = ast(vec![AstStmt::Print(PrintStatement { value: AstExpr::Identifier("missing".into()) })]);
        assert!(lower(&missing).unwrap_err().message.contains("no resuelta"));
        assert!(lower(&ast(vec![AstStmt::Break])).unwrap_err().message.contains("break fuera"));
    }

    #[test]
    fn while_condition_and_body_share_loop_scope() {
        let program = ast(vec![AstStmt::While(crate::ast::WhileStatement {
            condition: AstExpr::BoolLiteral(false),
            body: vec![AstStmt::Let(LetStatement { name: "i".into(), value: AstExpr::NumberLiteral(0.0) })],
        })]);
        let hir = lower(&program).unwrap();
        let StmtKind::While { scope, body, .. } = &hir.statements[0].kind else { panic!("expected while"); };
        let StmtKind::Let { binding, .. } = &body[0].kind else { panic!("expected let"); };
        assert_eq!(hir.bindings[binding.0 as usize].scope, *scope);
    }

    #[test]
    fn lowers_nested_aggregates_and_index_assignment() {
        let program = ast(vec![
            AstStmt::Let(LetStatement {
                name: "items".into(),
                value: AstExpr::Array(vec![
                    AstExpr::NumberLiteral(1.0),
                    AstExpr::Map(vec![(
                        AstExpr::StringLiteral("name".into()),
                        AstExpr::StringLiteral("value".into()),
                    )]),
                ]),
            }),
            AstStmt::IndexAssign(crate::ast::IndexAssignStatement {
                name: "items".into(),
                indices: vec![AstExpr::NumberLiteral(0.0)],
                value: AstExpr::NumberLiteral(2.0),
            }),
        ]);
        let hir = lower(&program).unwrap();
        let StmtKind::Let { value, .. } = &hir.statements[0].kind else { panic!("expected let"); };
        assert!(matches!(&value.kind, ExprKind::Array(items)
            if matches!(&items[1].kind, ExprKind::Map(entries) if entries.len() == 1)));
        assert!(matches!(&hir.statements[1].kind, StmtKind::IndexAssign {
            binding: BindingId(0),
            indices: vec![Expression { kind: ExprKind::Number(0.0), .. }],
            value: Expression { kind: ExprKind::Number(2.0), .. },
        }));
    }

    #[test]
    fn lowers_nested_index_reads() {
        let program = ast(vec![
            AstStmt::Let(LetStatement {
                name: "items".into(),
                value: AstExpr::Array(vec![AstExpr::Array(vec![AstExpr::NumberLiteral(7.0)])]),
            }),
            AstStmt::Print(crate::ast::PrintStatement {
                value: AstExpr::Index {
                    target: Box::new(AstExpr::Index {
                        target: Box::new(AstExpr::Identifier("items".into())),
                        index: Box::new(AstExpr::NumberLiteral(0.0)),
                    }),
                    index: Box::new(AstExpr::NumberLiteral(0.0)),
                },
            }),
        ]);
        let hir = lower(&program).unwrap();
        let StmtKind::Print(value) = &hir.statements[1].kind else { panic!("expected print"); };
        assert!(matches!(&value.kind, ExprKind::Index { target, index }
            if matches!(&target.kind, ExprKind::Index { .. }) && matches!(&index.kind, ExprKind::Number(0.0))));
    }

}
