use crate::ast::{Expression as AstExpr, Program as AstProgram, Statement as AstStmt};
use crate::lexer::Token;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StmtId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp { Add, Sub, Mul, Div, Less, Greater, Equal, NotEqual }

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
    pub scopes: Vec<Scope>,
    pub bindings: Vec<Binding>,
    pub functions: Vec<Function>,
    pub capture_storage: crate::capture::CaptureStoragePlan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub id: FunctionId,
    pub parent: Option<FunctionId>,
    pub scope: ScopeId,
    pub params: Vec<BindingId>,
    pub body: FunctionBody,
    pub captures: Vec<Capture>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionBody {
    /// Expression-bodied lambdas return this value implicitly.
    pub expression: Option<Expression>,
    /// Block-bodied lambdas execute statements and return explicitly.
    pub statements: Vec<Statement>,
    /// The interpreter returns numeric zero when a block body falls through.
    pub implicit_zero_return: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capture {
    pub binding: BindingId,
    pub reads: bool,
    pub writes: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind { Module, Block, Branch, Loop, Function }

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub id: BindingId,
    pub scope: ScopeId,
    pub name: String,
    pub mutable: bool,
    pub function_owner: Option<FunctionId>,
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
    Return(Expression),
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
    Length(Box<Expression>),
    Index { target: Box<Expression>, index: Box<Expression> },
    Closure(FunctionId),
    Call { callee: Box<Expression>, args: Vec<Expression> },
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
    functions: Vec<Function>,
    function_stack: Vec<FunctionId>,
    predeclared_functions: HashSet<BindingId>,
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
            functions: Vec::new(),
            function_stack: Vec::new(),
            predeclared_functions: HashSet::new(),
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
        self.bindings.push(Binding {
            id, scope, name: name.to_string(), mutable: true,
            function_owner: self.function_stack.last().copied(),
        });
        self.symbols.last_mut().expect("module scope exists").insert(name.to_string(), id);
        id
    }

    /// Reserve names of lambda declarations in this lexical scope so
    /// self-recursive and mutually recursive closures resolve to stable IDs.
    /// The reservation is limited to direct declarations in this scope.
    fn predeclare_functions(&mut self, ast: &[AstStmt]) {
        let mut seen = HashSet::new();
        for statement in ast {
            if let AstStmt::Let(binding) = statement {
                if !seen.insert(binding.name.clone()) {
                    continue;
                }
                if matches!(&binding.value, AstExpr::Lambda { .. }) {
                    let id = self.declare(&binding.name);
                    self.predeclared_functions.insert(id);
                }
            }
        }
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
            AstExpr::Lambda { params, body } => {
                let function = self.lower_function(params, body)?;
                ExprKind::Closure(function)
            }
            AstExpr::FunctionCall { func, args }
                if matches!(func.as_ref(), AstExpr::Identifier(name) if name == "len") && args.len() == 1 =>
                    ExprKind::Length(Box::new(self.expr(&args[0])?)),
            AstExpr::FunctionCall { func, args } => ExprKind::Call {
                callee: Box::new(self.expr(func)?),
                args: args.iter().map(|arg| self.expr(arg)).collect::<Result<_, _>>()?,
            },
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
                let binding = if matches!(&value.kind, ExprKind::Closure(_)) {
                    self.symbols.last().and_then(|scope| scope.get(&binding.name)).copied()
                        .filter(|id| self.predeclared_functions.remove(id))
                        .unwrap_or_else(|| self.declare(&binding.name))
                } else {
                    self.declare(&binding.name)
                };
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
            AstStmt::Return(value) => {
                if self.function_stack.is_empty() {
                    return Err(LowerError { message: "return fuera de una función durante AST→HIR".into() });
                }
                StmtKind::Return(self.expr(value)?)
            },
            AstStmt::Block(statements) => {
                let scope = self.enter_scope(ScopeKind::Block);
                self.predeclare_functions(statements);
                let result = self.statements(statements);
                self.leave_scope();
                StmtKind::Block { scope, statements: result? }
            }
            AstStmt::If(branch) => {
                let condition = self.expr(&branch.condition)?;
                let then_scope = self.enter_scope(ScopeKind::Branch);
                self.predeclare_functions(&branch.true_body);
                let then_result = self.statements(&branch.true_body);
                self.leave_scope();
                let then_body = then_result?;
                let (else_scope, else_body) = if let Some(body) = &branch.false_body {
                    let scope = self.enter_scope(ScopeKind::Branch);
                    self.predeclare_functions(body);
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
                self.predeclare_functions(&loop_stmt.body);
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
        };
        let id = StmtId(self.next_stmt);
        self.next_stmt += 1;
        Ok(Statement { id, kind })
    }

    fn statements(&mut self, ast: &[AstStmt]) -> Result<Vec<Statement>, LowerError> {
        ast.iter().map(|statement| self.stmt(statement)).collect()
    }

    fn lower_function(&mut self, params: &[String], body: &AstExpr) -> Result<FunctionId, LowerError> {
        let id = FunctionId(self.functions.len() as u32);
        let parent = self.function_stack.last().copied();
        let scope = self.enter_scope(ScopeKind::Function);
        self.functions.push(Function {
            id, parent, scope, params: Vec::with_capacity(params.len()),
            body: FunctionBody::default(), captures: Vec::new(),
        });
        self.function_stack.push(id);
        let previous_loop_depth = self.loop_depth;
        self.loop_depth = 0;

        let result = (|| {
            for name in params {
                let binding = self.declare(name);
                self.functions[id.0 as usize].params.push(binding);
            }
            match body {
                AstExpr::Block(statements) => {
                    self.predeclare_functions(statements);
                    Ok(FunctionBody {
                        expression: None,
                        statements: self.statements(statements)?,
                        implicit_zero_return: true,
                    })
                },
                expression => Ok(FunctionBody {
                    expression: Some(self.expr(expression)?),
                    statements: Vec::new(),
                    implicit_zero_return: false,
                }),
            }
        })();

        self.loop_depth = previous_loop_depth;
        self.function_stack.pop();
        self.leave_scope();
        self.functions[id.0 as usize].body = result?;
        Ok(id)
    }
}

pub fn lower(program: &AstProgram) -> Result<Program, LowerError> {
    let mut lowerer = Lowerer::new();
    lowerer.predeclare_functions(&program.statements);
    let statements = lowerer.statements(&program.statements)?;
    let mut program = Program {
        statements,
        scopes: lowerer.scopes,
        bindings: lowerer.bindings,
        functions: lowerer.functions,
        capture_storage: crate::capture::CaptureStoragePlan::default(),
    };
    program.functions = crate::capture::analyze(&program)
        .map_err(|error| LowerError { message: error.message })?;
    program.capture_storage = crate::capture::plan_storage(&program)
        .map_err(|error| LowerError { message: error.message })?;
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssignStatement, LetStatement, PrintStatement};

    fn ast(statements: Vec<AstStmt>) -> AstProgram { AstProgram { statements } }




    #[test]
    fn block_lambda_preserves_interpreter_zero_fallthrough_value() {
        let mut lexer = crate::lexer::Lexer::new("let noop = |x| (print x;);".to_string());
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let ast = parser.parse_program().unwrap();
        let hir = lower(&ast).unwrap();
        assert!(hir.functions[0].body.implicit_zero_return);
        assert!(hir.functions[0].body.expression.is_none());
    }

    #[test]
    fn predeclares_mutually_recursive_lambdas_in_the_same_lexical_scope() {
        let mut lexer = crate::lexer::Lexer::new(
            "let even = |n| odd(n); let odd = |n| even(n);".to_string()
        );
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let ast = parser.parse_program().unwrap();
        let hir = lower(&ast).unwrap();

        assert_eq!(hir.functions.len(), 2);
        let even = hir.bindings.iter().find(|binding| binding.name == "even").unwrap().id;
        let odd = hir.bindings.iter().find(|binding| binding.name == "odd").unwrap().id;
        assert_eq!(hir.functions[0].captures, vec![Capture { binding: odd, reads: true, writes: false }]);
        assert_eq!(hir.functions[1].captures, vec![Capture { binding: even, reads: true, writes: false }]);
    }

    #[test]
    fn lowers_function_calls_to_hir_nodes_with_resolved_callee_and_arguments() {
        let mut lexer = crate::lexer::Lexer::new(
            "let add = |x| x + 1; let result = add(41);".to_string()
        );
        let tokens = lexer.tokenize().unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        let ast = parser.parse_program().unwrap();
        let hir = lower(&ast).unwrap();

        assert_eq!(hir.functions.len(), 1);
        let StmtKind::Let { value, .. } = &hir.statements[1].kind else { panic!("expected let"); };
        assert!(matches!(&value.kind, ExprKind::Call { callee, args }
            if matches!(callee.kind, ExprKind::Read(BindingId(0)))
                && args.len() == 1
                && matches!(args[0].kind, ExprKind::Number(41.0))));
    }

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
            indices,
            value: Expression { kind: ExprKind::Number(2.0), .. },
        } if indices.len() == 1 && matches!(&indices[0].kind, ExprKind::Number(0.0))));
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
