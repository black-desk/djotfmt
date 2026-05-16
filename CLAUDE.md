# CLAUDE.md — djotfmt

## Project Overview

djotfmt is a **Djot markup formatter** written in Rust. It parses Djot documents
via the `jotdown` parser and re-emits them with consistent formatting and line
wrapping. Early stage, not production-ready.

- **License**: GPL-3.0-or-later
- **Repository**: https://github.com/black-desk/djotfmt

## Build & Run Commands

```bash
cargo build --release          # Build
cargo test --release           # Run all tests
cargo install --path .         # Install locally
cargo run -r -- --help         # Show CLI help
cargo run -r -- file.dj        # Format a file (stdout)
cargo run -r -- -i file.dj     # Format inplace
cargo run -r -- -vvv file.dj   # Max verbosity (trace-level logging)
```

## Project Structure

```
src/
  main.rs        — CLI entry point (arg parsing, file I/O, inplace editing)
  cli.rs         — Clap-based CLI argument definitions
  lib.rs         — Library re-exports (Renderer, WriterConfig)
  renderer.rs    — Core formatting engine (~1200 lines, the heart of the project)

tests/
  integration_test.rs  — Custom test harness (libtest-mimic)
  *.in / *.out         — Paired input/expected-output test cases (~44 tests)

third_party/
  jotdown/       — Forked/patched Djot parser (Rust),
                   used via Cargo patch (only this one is used at runtime)
  djot/          — Reference Djot implementation
                   (git submodule, reference only, not used at build or runtime)
  djot.js/       — JavaScript Djot implementation
                   (git submodule, reference only, not used at build or runtime)

docs/            — Long about text and after-help text for CLI
```

## Architecture

**Pipeline**: Djot source -> `jotdown::Parser` -> Event stream -> `Writer` ->
Formatted Djot

The `Writer` struct in `renderer.rs` is the core. It processes `jotdown::Event`
items one-by-one and manages:
- Word accumulation and line wrapping (Unicode-aware via `unicode-width`)
- Prefix stack for indentation (lists, blockquotes)
- Table rendering (multi-pass: accumulate -> calculate widths -> render)
- List tracking (ordered/unordered, nesting, tight/loose)
- Attribute handling
- In-place editing via swap files

`Renderer` is the public API wrapper; `Writer` does the actual work.

## Key Types

| Type | Location | Purpose |
|------|----------|---------|
| `Renderer` | `renderer.rs` | Public API; holds source string, creates `Writer` |
| `WriterConfig` | `renderer.rs` | Config: `max_cols` (default 72) |
| `Writer` | `renderer.rs` | Internal formatting engine with state machine |
| `TableData` | `renderer.rs` | Accumulates table content for multi-pass rendering |
| `Cli` | `cli.rs` | Clap-derived CLI args (verbose, input, inplace, columns) |

## Testing

No unit tests. All testing is end-to-end via integration tests.

Integration tests use a **paired-file approach**:
- `tests/<name>.in` — input Djot document
- `tests/<name>.out` — expected formatted output
- Each `.out` is also verified for **idempotency** (formatting twice yields the
  same result)
- Use `{ % @columns: N % }` in `.in` files to set custom line width (default 72)
- Custom harness built on `libtest-mimic`; not the standard `#[test]` attribute

To add a test: create `tests/<name>.in` and `tests/<name>.out`, then `cargo test
--release`.

## Code Style

- Standard Rust conventions: `snake_case` functions/vars, `CamelCase` types
- SPDX license headers on all source files
- Extensive `log::trace!` / `log::debug!` instrumentation in `renderer.rs`
- Error handling: `std::fmt::Result` throughout the renderer; `unwrap()` only
  where invariants are guaranteed
- No separate linting config — standard `cargo clippy` applies

## Dependencies

| Crate | Purpose |
|-------|---------|
| `jotdown` | Djot parser (patched to local fork in `third_party/jotdown`) |
| `clap` | CLI argument parsing (derive API) |
| `unicode-width` | Unicode-aware character width for line wrapping |
| `roman` | Roman numeral conversion for ordered lists |
| `log` + `colog` + `env_logger` | Logging at configurable verbosity |
| `libtest-mimic` (dev) | Custom test harness |
| `pretty_assertions` (dev) | Readable test diffs |
