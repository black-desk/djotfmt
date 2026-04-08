<div id="Contributing-to-djotfmt" class="section">

# Contributing to djotfmt

Thank you for your interest in contributing to djotfmt!

<div id="Build" class="section">

## Build

``` bash
cargo build
```

</div>

<div id="Test" class="section">

## Test

``` bash
cargo test
```

</div>

<div id="Debug" class="section">

## Debug

djotfmt uses the `log` crate for logging. You can increase the verbosity
level with the `-v` flag to see more detailed information about the
formatting process.

- *(default)* Warn: Only warnings and errors.

- `-v` Info: General informational messages.

- `-vv` Debug: Show jotdown parse events and the corresponding source
  text.

- `-vvv` Trace: Show internal rendering state: pending words, pending
  lines, prefix stack, etc.

For example, to debug the formatting of a file with full trace output:

``` bash
cargo run -- -vvv input.dj
```

At `Debug` level you will see output like:

```
DEBUG Event: Start(Paragraph, ...)
DEBUG Source: "hello world"
DEBUG Attributes: ...
```

At `Trace` level you will see additional internal state:

```
TRACE Pending word: "hello"
TRACE Commit word: "hello"
TRACE Pending line: "hello "
TRACE Prefix: ["> "]
```

</div>

</div>
