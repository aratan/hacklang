# HackLang

Lenguaje de programación interpretado, Turing completo, implementado en Rust.

## Arquitectura

```mermaid
flowchart LR
    A["Source (.hl)"] --> B["Lexer<br/><i>tokenize()</i>"]
    B --> C["Tokens<br/><i>Vec&lt;Token&gt;</i>"]
    C --> D["Parser<br/><i>parse_program()</i>"]
    D --> E["AST<br/><i>Program</i>"]
    E --> F["Evaluador<br/><i>evaluate()</i>"]
    F --> G["Output"]
```

### Componentes

```mermaid
classDiagram
    class Lexer {
        +tokenize() Vec~Token~
        -read_identifier() String
        -read_number() f64
        -read_string() String
    }
    class Token {
        <<enum>>
        Plus
        Minus
        Star
        Slash
        Let
        Print
        While
        If
        Identifier(String)
        Number(Number)
        String(String)
    }
    class Parser {
        +parse_program() Program
        -parse_statement() Statement
        -parse_expression(prec) Expression
        -parse_block() Vec~Statement~
    }
    class Program {
        Vec~Statement~ statements
    }
    class Statement {
        <<enum>>
        Let(LetStatement)
        Print(PrintStatement)
        Block(Vec~Statement~)
        While(WhileStatement)
        If(IfStatement)
    }
    class Expression {
        <<enum>>
        NumberLiteral(f64)
        StringLiteral(String)
        Identifier(String)
        Infix
        Ternary
        Lambda
        FunctionCall
        Array(Vec~Expression~)
        Map(Vec~(Expression, Expression)~)
        Index
    }
    class Evaluator {
        +evaluate(Program) Result
        -run_stmt(Statement) Result
        -eval_expression(Expression) Result~Value~
        -apply_binop(Value, Value, Token) Value
    }
    class Value {
        <<enum>>
        Number(f64)
        String(String)
    }

    Lexer ..> Token : produce
    Parser ..> Token : consume
    Parser --> Program : produce
    Program --> Statement : contains
    Statement --> Expression : contains
    Evaluator --> Program : consume
    Evaluator --> Value : produce
```

## Sintaxis

```hacklang
let x = 42;
print x;

let y = x + 1;

while (i < 10) {
  print i;
  i = i + 1;
}

let result = (x > 10) ? { x } : { 0 };

let add = |a, b| { a + b };
print add(3, 4);
```

## Cómo usar

```bash
cd hacklangc
cargo run ../test.hl
```

## Estado

| Componente | Estado |
|---|---|
| **Lexer** | ✅ Completo |
| **Parser** | ✅ Completo (infix, ternario, lambdas, arrays, mapas, indexación) |
| **Evaluador** | ⚠️ Parcial — lambdas, arrays, mapas, index y llamadas son stubs |
| **Tests** | ❌ Pendientes |
