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
    map('<leader>ln', vim.lsp.buf.rename, 'Djot: rename heading or ID')
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

`gd` on an inline link or a reference-definition destination opens the local
target file at its heading or explicit anchor. Without a fragment, it goes to
the start of the file. On a reference link’s label, `gd` first goes to the label
definition in the current file; use it again on that definition’s destination to
follow the link. The text part of a reference link can also navigate directly to
its destination.

External URLs remain available through `documentLink`. Neovim does not provide a
built-in `vim.lsp.buf.document_link()` action. For an optional `<leader>lo`
mapping, put the following function inside `on_attach`, after the other `map`
calls:

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
strings and heading fragments for local files. Use `gd` for same-document and
cross-file anchors. Ordinary bracket pairing can be configured with your
preferred Neovim pairing plugin.

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

- Workspace heading search through `workspace/symbol`, including unopened `.dj`
  files in the Git repositories containing the workspace folders. Names use
  `Title (path#anchor)`, with the path relative to the Git repository root and
  each heading’s explicit or generated anchor. Standalone local files use their
  filename; non-file documents use their URI. No `containerName` is returned.
  Duplicate titles keep separate file locations. Search matches name substrings
  without case sensitivity, including paths and anchors.

- Definition navigation and references within the open document for link labels,
  footnotes, explicit IDs and implicit heading references.

- Cross-file definition navigation for local inline links, reference-link text
  and reference-definition destinations. Relative paths resolve from the current
  document; query strings are ignored for file lookup and fragments locate
  explicit or generated anchors in `.dj` files. Target files need not be open or
  included in the workspace index. Links without a fragment (or with an empty
  fragment) navigate to the start of the file.

- Heading and explicit-ID rename through `textDocument/prepareRename` and
  `textDocument/rename`. Renaming a heading without an explicit ID updates its
  generated anchor and workspace links. If other generated anchors are
  renumbered, their links keep pointing to the same headings. Renaming a heading
  with an explicit ID preserves that ID and its fragment links; title-based
  references still follow the new title. Renaming an explicit ID updates its
  links without changing the heading text. A fragment or implicit title
  reference can also initiate the rename; generated fragments rename the owning
  heading, while explicit fragments rename the ID.

  Link paths, query strings and visible link text are preserved. For example,
  `[Old][]` becomes `[Old][New]` when its heading is renamed to `New`.
  Reference-link destinations are updated at the definition. New heading names
  are literal text; inline punctuation is escaped, replacing the old heading’s
  inline formatting. Renames that would remove an embedded ID, create a
  duplicate anchor or retarget an implicit heading reference are rejected.
  Ordinary reference labels and footnotes cannot be renamed yet.

- File and folder moves through `workspace/willRenameFiles` and
  `workspace/didRenameFiles`. Incoming links follow moved targets, and relative
  outgoing links in moved `.dj` files keep their original targets. Batch moves
  resolve each link against the original locations. Internal links within a
  moved folder keep their spelling when they still resolve correctly. Images and
  reference-definition destinations are included; external URLs, query strings,
  fragments and link text are preserved.

  You can also invoke rename on the path in a local link destination, including
  an image or reference definition. The new name is a path relative to the
  document containing that link, or an absolute file URI. The server returns
  text edits at the original URIs followed by a `RenameFile` operation. This
  requires client support for `documentChanges` and the `rename` resource
  operation. The source must exist, the new parent directory must exist, and
  existing destinations are not overwritten. Heading and ID rename remain
  available on fragments.

- Completion for link labels after `[text][`, footnotes after `[^`, and anchors
  after `[text](#`.

- Workspace heading completion after `[text](`. Current-document headings sort
  before headings in other files. Completion inserts `#anchor` for the current
  file and a URI relative to the current document’s directory for other files,
  including the explicit or generated heading anchor. Titles and destination
  prefixes can filter candidates. Existing closing parentheses are replaced, and
  destination paths and anchors are encoded as needed. Open target buffers
  contribute their unsaved headings. Percent-encoded same-document anchors
  resolve for definition navigation and reference diagnostics.

- Inline-link and reference-link templates after `[`. Accept a candidate to
  insert it; this does not automatically insert text without acceptance.
  Snippet-capable clients can move between the text and destination fields.
  Clients without snippet support receive plain text.

- Standard document links for direct links, reference links, images and
  reference-definition destinations. Relative paths resolve against the current
  document URI; Unicode and spaces are encoded as needed. Your client’s
  open-link action can open the target even when it is not already open in the
  editor. External URLs are supported too. Cross-file query strings and
  fragments are preserved in document-link targets. Use definition navigation to
  jump to a local target’s source position.

- Errors for unresolved reference labels, footnotes, local anchors, missing
  local target files and missing anchors in `.dj` targets. Duplicate definitions
  and IDs remain warnings. Read failures are reported separately as warnings,
  rather than as missing anchors. Other local file types are checked for
  existence; external URLs are not fetched or validated.

Completion suppresses suggestions in code, math and parsed attributes. Reference
completions replace an existing closing bracket and can remove empty inline-link
punctuation left by a link template. Destination completion requests are
triggered by `(`, `/` and `#`; clients may also request completion manually.
Destination completion lists are marked incomplete so clients can requery while
typing title or path prefixes.

Formatting defaults to 72 columns. Clients can pass a positive integer
`maxColumns` in formatting options to change it. The LSP indentation option
`tabSize` is not used as a line-width setting.

</div>

<div id="Scope-and-implementation" class="section">

## Scope and implementation

Workspace folders are taken from the client, with `rootUri` (or legacy
`rootPath`) as a fallback. Each folder discovers its enclosing Git repository,
including linked worktrees. Git must be installed for repository discovery. The
index reads working-tree content for tracked and non-ignored untracked files
with the exact `.dj` extension. Submodules and symbolic-link files are not
traversed. Standalone open documents remain supported without Git.

Open buffers take precedence over disk snapshots, including unsaved changes.
Closing a buffer restores disk content for searches, completion, navigation and
diagnostics. Clients with file-watching support receive watcher registrations
for `.dj` changes and file creation/deletion. Workspace-folder and file-change
notifications refresh the index and open-document diagnostics. Opening, editing,
saving or closing a buffer also rechecks links in all open documents. Without
file watching, saving a buffer refreshes link diagnostics. Searches, destination
completion and cross-file navigation refresh disk state and Git ignore rules on
demand. Discovery and disk analysis run on a blocking worker; unchanged source
reuses its parsed snapshot. Each refresh still enumerates and reads repository
files.

Link resolution reads local `.dj` targets outside the index on demand, including
ignored files and paths outside the repository. Open-buffer content takes
precedence, including new files that have not yet been saved. The same resolver
supplies navigation and link validation, with a per-operation cache for target
reads. Diagnostics are published for open documents and retain source versions;
results for superseded snapshots are discarded. Revalidation currently checks
all open documents rather than maintaining an incremental reverse dependency
index. File reads and workspace link validation run on blocking workers.

Rename searches indexed workspace documents and all open buffers, including
unsaved files. A linked target outside the index can be renamed on demand;
incoming links in other unindexed, unopened files are not discovered. The server
returns a workspace edit for the client to apply and never writes files itself.
Clients supporting `documentChanges` receive versioned edits for open buffers;
other clients receive `changes`. If open buffers change during rename planning,
the request fails with `ContentModified` so the client can retry.

File-move planning uses the same index and open-buffer snapshots. An explicitly
moved `.dj` file outside the index is read on demand to update outgoing links;
unindexed, unopened descendants of moved folders are not traversed. Overlapping
source moves and document destination collisions are rejected. A
`didRenameFiles` notification migrates open buffers to the new URIs, clears old
diagnostics, relocates moved workspace folders and refreshes diagnostics. A
buffer already opened at the destination takes precedence over a migrated
snapshot. Index overlays treat equivalent encoded file URIs as the same file.

The client’s file manager must send `willRenameFiles`, apply the returned edits,
perform the move and send `didRenameFiles` (when advertised). A terminal `mv` or
file-watch notification alone cannot repair links automatically. The server uses
the standard file-operation callbacks in `tower-lsp-server`.

Find references remains document-local. An incomplete delimiter alone is not
reported as a syntax error.

Each document version is analyzed in full. Its snapshot retains source, parser
events, headings, definitions, references and document links. Context is
calculated on demand from events and nearby source; there is no complete AST or
stored context tree. Analysis uses half-open UTF-8 byte ranges internally, while
the protocol adapter uses UTF-16 positions. Parser positions referring to the
synthetic final newline are clamped to the actual source.

The protocol adapter uses `tower-lsp-server` 0.23.0. The analysis library has no
dependency on that framework’s types or runtime. Document updates are applied
before notification handlers yield, and requests take immutable snapshots.
Diagnostics carry document versions. Open-buffer parsing and analysis are
currently synchronous; large-document scheduling is a later optimization.

</div>

<div id="Validation" class="section">

## Validation

`cargo test --release` runs parser conformance tests, formatter tests, and stdio
protocol tests covering document analysis and position-encoding negotiation.
Workspace stdio tests use temporary Git repositories to cover unopened files,
ignore rules, duplicate titles, linked worktrees, open-buffer precedence, disk
changes, workspace-folder changes, destination completion ordering, relative
links and URI encoding. Further stdio tests cover cross-file navigation, exact
error ranges, reference-label precedence, links inside headings, unsaved and
out-of-index targets, disk changes and read failures. Rename stdio tests apply
the returned workspace edits and cover generated anchor renumbering, explicit
IDs, implicit title references, unchanged link text inside other headings,
unsaved buffers, versioned edits, URI encoding, duplicate definitions,
conflicting names and rejected positions. File-move stdio tests cover incoming
and outgoing links, folder and batch moves, assets, resource-operation ordering,
unsaved buffer migration, workspace folder relocation, encoded URI equivalence,
ignored targets and invalid moves. Parser conformance needs the pinned djot.js
submodule and Node.js; its harness builds the reference when needed. The Rust
server has no JavaScript runtime dependency.

</div>

</div>
