# Frontend coverage-guided fuzzing

This package contains isolated cargo-fuzz targets for HackLang's lexer and parser. It imports the production source files directly so the fuzz harness exercises the same implementation without changing the binary crate layout.

## Requirements

- Rust nightly (libFuzzer sanitizer support)
- cargo-fuzz (cargo install cargo-fuzz --locked)

## Run locally

From the repository root:

```bash
cargo +nightly fuzz run lexer -- -max_total_time=60 -max_len=4096 -timeout=2
cargo +nightly fuzz run parser -- -max_total_time=60 -max_len=4096 -timeout=2
```

The parser target sends every valid token stream through both legacy and compact parser modes. Both targets reject inputs larger than 4 KiB; the parser also caps nesting at 64 to keep the campaign bounded.

## Corpus and findings

Seed inputs live under fuzz/corpus/lexer/ and fuzz/corpus/parser/. libFuzzer adds minimized findings to the corresponding corpus during a run. Preserve any reproducer that exposes a bug as a regression test in src/frontend_fuzz.rs or the relevant unit-test module before changing the implementation.

## CI policy

Pull requests that change fuzz/** run short five-second smoke campaigns for each target. The scheduled/manual workflow runs longer bounded campaigns. These jobs are supplementary: the deterministic frontend smoke tests in cargo test remain part of the main required Rust CI.

A clean short campaign is evidence only that no failure was found within that run's time and corpus; it is not proof of parser or lexer safety.