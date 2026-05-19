<!--
SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>

SPDX-License-Identifier: MIT
-->

# CLAUDE.md — djotfmt

## Project Overview

djotfmt is a **Djot markup formatter** written in Rust. It parses Djot documents
via a custom parser (ported from djot.js) and re-emits them with consistent
formatting and line wrapping. Early stage, not production-ready.

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
  lib.rs         — Library re-exports (fmt, parser modules)
  fmt.rs         — Core formatting engine (~1350 lines, the heart of the project)
  parser/        — Djot parser ported from djot.js (produces event stream)
    mod.rs       — Public API: parse_events(), Event struct
    block.rs     — Block-level parsing (~1020 lines)
    inline.rs    — Inline parsing (~935 lines)
    attributes.rs — Attribute parsing (~327 lines)
    find.rs      — Regex helper utilities (find_pos / find)

tests/
  fmt_test.rs          — Formatter test harness (libtest-mimic, 92 tests)
  parser_events_test.rs — Parser event tests (364 cases, compared against djot.js CLI)
  *.in / *.out         — Paired input/expected-output test cases (46 pairs)

third_party/
  djot/          — Reference Djot implementation
                   (git submodule, reference only, not used at build or runtime)
  djot.js/       — JavaScript Djot implementation
                   (git submodule, reference only, not used at build or runtime)

docs/            — Long about text and after-help text for CLI
```

## Architecture

**Pipeline**: Djot source -> `parser::parse_events()` -> Event stream -> `FmtWriter` ->
Formatted Djot

The `FmtWriter` struct in `fmt.rs` is the core. It processes `parser::Event`
items one-by-one and manages:
- Word accumulation and line wrapping (Unicode-aware via `unicode-width`)
- Prefix stack for indentation (lists, blockquotes)
- Table rendering (multi-pass: accumulate -> calculate widths -> render)
- List tracking (ordered/unordered, nesting, tight/loose)
- Attribute handling
- In-place editing via swap files

`format()` is the public API entry point; `FmtWriter` does the actual work.

## Key Types

| Type | Location | Purpose |
|------|----------|---------|
| `FmtConfig` | `fmt.rs` | Config: `max_cols` (default 72) |
| `format()` | `fmt.rs` | Public API: takes input string + config, returns formatted output |
| `FmtWriter` | `fmt.rs` | Internal formatting engine with state machine |
| `TableData` | `fmt.rs` | Accumulates table content for multi-pass rendering |
| `Cli` | `cli.rs` | Clap-derived CLI args (verbose, input, inplace, columns) |
| `parser::Event` | `parser/mod.rs` | Parse event with startpos, endpos, annot (djot.js compatible) |
| `parser::parse_events()` | `parser/mod.rs` | Parse Djot input into event stream |

## Testing

No unit tests. All testing is end-to-end via integration tests.

**Formatter tests** use a **paired-file approach**:
- `tests/<name>.in` — input Djot document
- `tests/<name>.out` — expected formatted output
- Each `.out` is also verified for **idempotency** (formatting twice yields the
  same result)
- Use `{ % @columns: N % }` in `.in` files to set custom line width (default 72)
- Custom harness built on `libtest-mimic`; not the standard `#[test]` attribute

**Parser tests** (`tests/parser_events_test.rs`):
- 364 test cases from the djot.js test suite
- Compares Rust parser output against `djot.js` CLI (`djot --to events`)
- Requires `djot` (Node.js) to be available in PATH for generating expected output

To add a formatter test: create `tests/<name>.in` and `tests/<name>.out`, then
`cargo test --release`.

## Code Style

- Standard Rust conventions: `snake_case` functions/vars, `CamelCase` types
- SPDX license headers on all source files
- Extensive `log::trace!` / `log::debug!` instrumentation in `fmt.rs`
- Error handling: `std::fmt::Result` throughout the formatter; `unwrap()` only
  where invariants are guaranteed
- No separate linting config — standard `cargo clippy` applies

## Dependencies

| Crate | Purpose |
|-------|---------|
| `regex` | Regex for parser module (byte-level matching with Unicode disabled) |
| `lazy_static` | Lazy statics for parser regex patterns |
| `clap` | CLI argument parsing (derive API) |
| `unicode-width` | Unicode-aware character width for line wrapping |
| `roman` | Roman numeral conversion for ordered lists |
| `log` + `colog` + `env_logger` | Logging at configurable verbosity |
| `libtest-mimic` (dev) | Custom test harness |
| `pretty_assertions` (dev) | Readable test diffs |
