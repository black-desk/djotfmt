<div id="Djot-language-server" class="section">

# Djot language server

Build with `cargo build --release --bin djotls`, or install both tools with
`cargo install --path .`. Run `djotls` to serve the Language Server Protocol
over standard input and output. The server writes only protocol messages to
standard output. The existing `djotfmt` command remains the formatter;
`cargo run` still selects it by default.

<div id="Editor-setup" class="section">

## Editor setup

Configure your editor’s LSP client to run `djotls` for Djot buffers. Use
language ID `djot` and associate `.dj` and `.djot` files with it. \### Neovim
0.11 or newer

Install from the repository with `cargo install --path . --locked`. Make sure
`~/.cargo/bin` is on Neovim’s `PATH`: `:echo exepath('djotls')` should print the
executable’s path. If you only built the project, replace `'djotls'` below with
the absolute path to `target/release/djotls`.

Put this in `init.lua`. It uses Neovim’s built-in LSP client, completion and
snippets; `nvim-lspconfig`, Mason and a completion plugin are optional.

```lua
vim.filetype.add({ extension = { dj = 'djot', djot = 'djot' } })
vim.lsp.config('djot', {
  cmd = { 'djotls' },
  filetypes = { 'djot' },
  root_markers = { '.git' },
  workspace_required = false, -- Also attach to standalone files.
  on_attach = function(client, bufnr)
    vim.lsp.completion.enable(true, client.id, bufnr, { autotrigger = true })
    local function map(lhs, rhs, desc)
      vim.keymap.set('n', lhs, rhs, { buffer = bufnr, desc = desc })
    end
    map('gd', vim.lsp.buf.definition, 'Djot: go to definition')
    map('<leader>lr', vim.lsp.buf.references, 'Djot: references')
    map('<leader>ls', vim.lsp.buf.document_symbol, 'Djot: heading outline')
    map('<leader>lf', function()
      vim.lsp.buf.format({ bufnr = bufnr, name = 'djot', async = false })
    end, 'Djot: format')
    map('<leader>ld', vim.diagnostic.open_float, 'Djot: diagnostics')
  end,
})
vim.lsp.enable('djot')
```

Reopen a `.dj` or `.djot` file after loading the configuration. Check
`:set filetype?` (it should be `djot`) and `:checkhealth vim.lsp` if the server
does not attach. `:lua =vim.lsp.get_clients({ bufnr = 0 })` shows clients
attached to the current buffer.

In Insert mode, use `<C-x><C-o>` to request completion manually and `<C-y>` to
accept a selected candidate. Automatic triggering is enabled above for the
server’s trigger characters. Try `[`, `[text][`, `[^`, or `[text](#` with
matching definitions in the document. If you already use a completion plugin,
keep its setup and omit `vim.lsp.completion.enable` to avoid running two
completion UIs.

`gd` on a reference link goes to its label definition in the current file.
Opening the destination file is a separate `documentLink` operation; Neovim does
not provide a built-in `vim.lsp.buf.document_link()` action. For an optional
`<leader>lo` mapping, put the following function inside `on_attach`, after the
other `map` calls:

```lua
map('<leader>lo', function()
  local params = vim.lsp.util.make_position_params(0, client.offset_encoding)
  local pos = params.position
  client:request('textDocument/documentLink', {
    textDocument = params.textDocument,
  }, function(err, links)
    if err then
      vim.notify(err.message, vim.log.levels.ERROR)
      return
    end
    for _, link in ipairs(links or {}) do
      local r = link.range
      local after_start = pos.line > r.start.line
        or (pos.line == r.start.line and pos.character >= r.start.character)
      local before_end = pos.line < r['end'].line
        or (pos.line == r['end'].line and pos.character < r['end'].character)
      if after_start and before_end and link.target then
        if link.target:match('^file:') then
          -- Open the file; cross-file heading fragments are not resolved.
          local uri = link.target:gsub('[?#].*$', '')
          vim.cmd.edit(vim.fn.fnameescape(vim.uri_to_fname(uri)))
        else
          vim.ui.open(link.target)
        end
        return
      end
    end
    vim.notify('No Djot link under cursor')
  end, bufnr)
end, 'Djot: open link')
```

The mapping opens local files in Neovim and delegates external URLs to
`vim.ui.open`. It opens cross-file links at the file level and ignores query
strings and heading fragments for local files. Use `gd` for same-document
anchors. Ordinary bracket pairing can be configured with your preferred Neovim
pairing plugin.

Enable your client’s completion UI and snippet support to use link placeholders.
Automatic completion triggering is a client setting; manual completion also
works. Ordinary bracket pairing remains an editor setting. No custom protocol or
editor-specific plugin is required by this server.

</div>

<div id="First-version" class="section">

## First version

The server supports:

- Whole-document formatting, using the existing formatter.

- A hierarchical heading outline (a flat outline for clients that do not support
  hierarchical document symbols).

- Definition navigation and references within the open document for link labels,
  footnotes, explicit IDs and implicit heading references.

- Completion for link labels after `[text][`, footnotes after `[^`, and anchors
  after `[text](#`.

- Inline-link and reference-link templates after `[`. Accept a candidate to
  insert it; this does not automatically insert text without acceptance.
  Snippet-capable clients can move between the text and destination fields.
  Clients without snippet support receive plain text.

- Standard document links for direct links, reference links, images and
  reference-definition destinations. Relative paths resolve against the current
  document URI; Unicode and spaces are encoded as needed. Your client’s
  open-link action can open the target even when it is not already open in the
  editor. External URLs are supported too. Cross-file query strings and
  fragments are preserved; target headings are not analyzed. Same-document
  anchors continue to use definition navigation.

- Warnings for unresolved references and duplicate definitions or IDs.

Completion suppresses suggestions in code, math and parsed attributes. Reference
completions replace an existing closing bracket and can remove empty inline-link
punctuation left by a link template.

Formatting defaults to 72 columns. Clients can pass a positive integer
`maxColumns` in formatting options to change it. The LSP indentation option
`tabSize` is not used as a line-width setting.

</div>

<div id="Scope-and-implementation" class="section">

## Scope and implementation

This version analyzes open documents independently. It does not scan a
workspace, analyze symbols across files, offer rename, or validate filesystem
paths. An incomplete delimiter alone is not reported as a syntax error.

Each document version is analyzed in full. Its snapshot retains source, parser
events, headings, definitions, references and document links. Context is
calculated on demand from events and nearby source; there is no complete AST or
stored context tree. Analysis uses half-open UTF-8 byte ranges internally, while
the protocol adapter uses UTF-16 positions. Parser positions referring to the
synthetic final newline are clamped to the actual source.

The protocol adapter uses `tower-lsp-server` 0.23.0. The analysis library has no
dependency on that framework’s types or runtime. Document updates are applied
before notification handlers yield, and requests take immutable snapshots.
Diagnostics carry document versions. Parsing and analysis are currently
synchronous; large-document scheduling is a later optimization.

</div>

<div id="Validation" class="section">

## Validation

`cargo test --release` runs parser conformance tests, formatter tests, and stdio
protocol tests covering document analysis and position-encoding negotiation.
Parser conformance needs the pinned djot.js submodule and Node.js; its harness
builds the reference when needed. The Rust server has no JavaScript runtime
dependency.

</div>

</div>
