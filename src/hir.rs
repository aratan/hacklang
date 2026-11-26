use crate::ast::{AstSpanMap, Expression as AstExpr, Program as AstProgram, Statement as AstStmt};
use crate::lexer::{SourceSpan, Token};
use std::collections::{HashMap, HashSet};