#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/lexer.rs"]
mod lexer;

fuzz_target!(|data: &[u8]| {
    const MAX_INPUT_BYTES: usize = 4096;
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    let source = String::from_utf8_lossy(data).into_owned();
    let mut frontend = lexer::Lexer::new(source);
    let _ = frontend.tokenize_spanned();
});