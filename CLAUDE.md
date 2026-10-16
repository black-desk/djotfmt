<!--
SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>

SPDX-License-Identifier: MIT
-->

# CLAUDE.md — djotls

## Project Overview

djotls is a **Djot toolkit** written in Rust. It includes a custom parser
(ported from djot.js), the `djotfmt` formatter and the `djotls` language server.
The server provides formatting, navigation, references, heading outlines,
completion and diagnostics. Early stage, not production-ready.

- **License**: GPL-3.0-or-later
- **Repository**: https://github.com/black-desk/djotls

## Build & Run Commands

```bash
cargo build --release          # Build
cargo test --release           # Run all tests
cargo install --path .         # Install djotfmt and djotls locally
cargo run -r -- --help         # Show formatter CLI help
cargo run -r --bin djotls      # Start the language server over stdio
cargo run -r -- file.dj        # Format a file (stdout)
cargo run -r -- -i file.dj     # Format inplace
cargo run -r -- -vvv file.dj   # Max verbosity (trace-level logging)
```

## Project Structure

```
src/
  bin/
    djotfmt.rs   — Formatter CLI entry point (file I/O, inplace editing)
    djotls.rs    — Language server entry point (stdio)
  lsp.rs         — LSP protocol adapter
  analysis/      — Document analysis and text coordinates, independent of LSP
  cli.rs         — Clap-based CLI argument definitions
  lib.rs         — Public library modules (analysis, fmt, parser)
  fmt.rs         — Core formatting engine
  parser/        — Djot parser ported from djot.js (produces event stream)
    mod.rs       — Public API: parse_events(), Event struct
    block.rs     — Block-level parsing
    inline.rs    — Inline parsing
    attributes.rs — Attribute parsing
    find.rs      — Regex helper utilities (find_pos / find)

tests/
  fmt_test.rs          — Formatter test harness (libtest-mimic)
  parser_events_test.rs — Parser conformance tests against djot.js
  lsp_test.rs          — Document analysis and stdio protocol tests
  *.in / *.out         — Paired input/expected-output formatter test cases

third_party/
  djot/          — Reference Djot implementation
                   (git submodule, reference only, not used at build or runtime)
  djot.js/       — JavaScript Djot implementation
                   (git submodule, reference only, not used at build or runtime)

docs/            — CLI help text and language server setup/scope guide
```

## Architecture

**Pipeline**: Djot source -> `parser::parse_events()` -> Event stream ->
`FmtWriter` -> Formatted Djot

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

| Type                     | Location        | Purpose                                                           |
| ------------------------ | --------------- | ----------------------------------------------------------------- |
| `FmtConfig`              | `fmt.rs`        | Config: `max_cols` (default 72)                                   |
| `format()`               | `fmt.rs`        | Public API: takes input string + config, returns formatted output |
| `FmtWriter`              | `fmt.rs`        | Internal formatting engine with state machine                     |
| `TableData`              | `fmt.rs`        | Accumulates table content for multi-pass rendering                |
| `Cli`                    | `cli.rs`        | Clap-derived CLI args (verbose, input, inplace, columns)          |
| `parser::Event`          | `parser/mod.rs` | Parse event with startpos, endpos, annot (djot.js compatible)     |
| `parser::parse_events()` | `parser/mod.rs` | Parse Djot input into event stream                                |

## Testing

Tests cover parser conformance, formatter output and idempotency, document
analysis, text coordinates and the LSP stdio protocol.

**Formatter tests** use a **paired-file approach**:

- `tests/<name>.in` — input Djot document
- `tests/<name>.out` — expected formatted output
- Each `.out` is also verified for **idempotency** (formatting twice yields the
  same result)
- Use `{ % @columns: N % }` in `.in` files to set custom line width (default 72)
- Custom harness built on `libtest-mimic`; not the standard `#[test]` attribute

**Parser tests** (`tests/parser_events_test.rs`):

- Test cases from the pinned djot.js test suite
- Compares Rust parser output against `djot.js` CLI (`djot --to events`)
- Requires Node.js and the pinned djot.js submodule; the harness builds the
  reference CLI when needed
- Set `DJOTLS_REQUIRE_CONFORMANCE=1` to fail if the reference cannot run;
  `DJOTLS_SKIP_JS_BUILD=1` skips the automatic reference build

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

| Crate                          | Purpose                                                             |
| ------------------------------ | ------------------------------------------------------------------- |
| `regex`                        | Regex for parser module (byte-level matching with Unicode disabled) |
| `lazy_static`                  | Lazy statics for parser regex patterns                              |
| `tower-lsp-server` + `tokio`   | LSP protocol adapter and async stdio runtime                        |
| `url`                          | Document-link URI resolution                                        |
| `clap`                         | CLI argument parsing (derive API)                                   |
| `unicode-width`                | Unicode-aware character width for line wrapping                     |
| `roman`                        | Roman numeral conversion for ordered lists                          |
| `log` + `colog` + `env_logger` | Logging at configurable verbosity                                   |
| `libtest-mimic` (dev)          | Custom test harness                                                 |
| `pretty_assertions` (dev)      | Readable test diffs                                                 |
