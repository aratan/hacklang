#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/lexer.rs"]
mod lexer;
#[path = "../../src/ast.rs"]
mod ast;
#[path = "../../src/parser.rs"]
mod parser;

fuzz_target!(|data: &[u8]| {
    const MAX_INPUT_BYTES: usize = 4096;
    const MAX_PARSE_DEPTH: usize = 64;
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    let source = String::from_utf8_lossy(data).into_owned();
    let mut frontend = lexer::Lexer::new(source);
    let Ok(tokens) = frontend.tokenize_spanned() else {
        return;
    };

    for compact in [false, true] {
        let mut parser = if compact {
            parser::Parser::new_compact_spanned_with_max_depth(tokens.clone(), MAX_PARSE_DEPTH)
        } else {
            parser::Parser::new_spanned_with_max_depth(tokens.clone(), MAX_PARSE_DEPTH)
        };
        let _ = parser.parse_program_with_spans();
    }
});