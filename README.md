<div id="djotls" class="section">

# djotls

[![codecov](https://codecov.io/gh/black-desk/djotls/graph/badge.svg?token=GF5SAU9WIC)](https://codecov.io/gh/black-desk/djotls)

<div class="warning">

**This project is under very early stage and not ready for use.**

</div>

djotls is a [`Djot`](https://djot.net) toolkit written in Rust, with a parser,
formatter (`djotfmt`) and language server (`djotls`).

<div id="Build" class="section">

## Build

```bash
cargo build --release
```

</div>

<div id="Test" class="section">

## Test

```bash
cargo test --release
```

</div>

<div id="Install" class="section">

## Install

```bash
cargo install --path .
```

</div>

<div id="Usage" class="section">

## Usage

You can check the usage by running:

```bash
cargo run -r -- --help
```

Or after installing:

```bash
djotfmt --help
djotls --help
```

</div>

<div id="Language-server" class="section">

## Language server

The `djotls` executable provides formatting, navigation, references, heading
outlines, completion and diagnostics over stdio. See the
[language server guide](docs/lsp.md) for setup and scope.

</div>

<div id="Contributing" class="section">

## Contributing

- [Djot syntax reference](https://htmlpreview.github.io/?https://github.com/jgm/djot/blob/master/doc/syntax.html)

</div>

<div id="Bugs" class="section">

## Bugs

Report bugs to <https://github.com/black-desk/djotls/issues/new>.

</div>

<div id="Contact" class="section">

## Contact

<a href="mailto:me@black-desk.cn"
class="uri">mailto:me@black-desk.cn</a>

</div>

<div id="Reference" class="section">

## Reference

- [djot-rs](https://github.com/matklad/djot-rs)

- [djot.js](https://github.com/jgm/djot.js/)

- [djot.lua](https://github.com/jgm/djot.lua/)

- [djot](https://github.com/jgm/djot/)

- [jotdown](https://github.com/hellux/jotdown/)

- [pandoc](https://github.com/jgm/pandoc)

</div>

</div>
