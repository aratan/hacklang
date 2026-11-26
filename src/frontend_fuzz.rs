//! Deterministic, bounded fuzz-smoke coverage for the shared lexer/parser frontend.
//! This is a regression guard, not a replacement for long-running coverage-guided fuzzing.

use crate::lexer::Lexer;
use crate::parser::Parser;
use std::panic::{AssertUnwindSafe, catch_unwind};

const MAX_INPUT_BYTES: usize = 4 * 1024;
const MAX_PARSE_DEPTH: usize = 64;
const GENERATED_CASES: usize = 256;

const CORPUS: &[&str] = &[
    "",
    "let x = 1; print x;",
    "let f = |x| x + 1; print f(41);",
    "n:=10\nprint n",
    "let broken = \"unterminated;",
    "((((((((((((((((((((((((((((((((((((((((((((((((1))))))))))))))))))))))))))))))))))))))))))))))))",
    r#"if (true) { let x = [1, 2, @("k": "v")]; print x[0]; }"#,
    "// comment with unicode: áéíóú 🙂\nlet value = 2;",
    "let x = @(\"a\": [1, 2], \"b\": true);",
    "let x = 1e999999999999999999999999999999999999;",
    "\0\u{7f}\u{200b}\n",
];

fn exercise_frontend(source: &str, compact: bool) {
    if source.len() > MAX_INPUT_BYTES {
        return;
    }

    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut lexer = Lexer::new(source.to_owned());
        let tokens = match lexer.tokenize_spanned() {
            Ok(tokens) => tokens,
            Err(_) => return,
        };
        let mut parser = if compact {
            Parser::new_compact_spanned_with_max_depth(tokens, MAX_PARSE_DEPTH)
        } else {
            Parser::new_spanned_with_max_depth(tokens, MAX_PARSE_DEPTH)
        };
        // Parse errors are expected for mutated input; panics are never acceptable.
        let _ = parser.parse_program_with_spans();
    }));

    assert!(result.is_ok(), "frontend panicked for compact={compact}, input={source:?}");
}

fn next_random(state: &mut u64) -> u64 {
    // Small deterministic xorshift generator: reproducible across CI runs.
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn lexer_and_parser_survive_regression_corpus_in_both_modes() {
    for source in CORPUS {
        exercise_frontend(source, false);
        exercise_frontend(source, true);
    }
}

#[test]
fn lexer_and_parser_survive_bounded_deterministic_mutations() {
    const ALPHABET: &[char] = &[
        'a', 'Z', '_', '0', '9', '+', '-', '*', '/', '=', ';', '(', ')', '{', '}',
        '[', ']', ':', '|', ',', '@', '?', '"', '\\', ' ', '\n', '\t', 'á', '界', '🙂', '\0',
    ];

    let mut state = 0x4841_434b_4c41_4e47_u64;
    for case_index in 0..GENERATED_CASES {
        let length = (next_random(&mut state) as usize % 192) + 1;
        let mut source = String::with_capacity(length * 4);
        for _ in 0..length {
            let index = next_random(&mut state) as usize % ALPHABET.len();
            source.push(ALPHABET[index]);
        }

        exercise_frontend(&source, false);
        exercise_frontend(&source, true);

        // Keep a useful assertion message if the deterministic generator is edited.
        assert!(source.len() <= MAX_INPUT_BYTES, "generated case {case_index} exceeded input cap");
    }
}
