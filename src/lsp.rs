// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Protocol adapter. Analysis and text coordinates are independent of this crate.
use djotls::{
    analysis::{
        links::{Resolution, Resolver},
        text::LineIndex,
        CompletionKind, Document, Severity, SymbolKind as DjotSymbolKind,
    },
    fmt::{self, FmtConfig},
    workspace::Workspace,
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range as ByteRange,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};
use tower_lsp_server::{
    jsonrpc::{Error, Result},
    ls_types::*,
    Client, LanguageServer,
};

struct Snapshot {
    version: i32,
    doc: Arc<Document>,
}

pub struct Backend {
    client: Client,
    documents: RwLock<HashMap<Uri, Arc<Snapshot>>>,
    workspace: tokio::sync::Mutex<Workspace>,
    diagnostics: tokio::sync::Mutex<()>,
    watch_files: AtomicBool,
    snippets: AtomicBool,
    nested_symbols: AtomicBool,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            workspace: tokio::sync::Mutex::new(Workspace::default()),
            diagnostics: tokio::sync::Mutex::new(()),
            watch_files: AtomicBool::new(false),
            snippets: AtomicBool::new(false),
            nested_symbols: AtomicBool::new(false),
        }
    }

    fn snapshot(&self, uri: &Uri) -> Result<Arc<Snapshot>> {
        self.documents
            .read()
            .unwrap()
            .get(uri)
            .cloned()
            .ok_or_else(|| Error::invalid_params("Document is not open"))
    }

    async fn refresh_workspace(workspace: &mut Workspace) -> Result<()> {
        let folders = workspace.folders.clone();
        let previous = workspace.documents.clone();
        *workspace = tokio::task::spawn_blocking(move || Workspace::scan(folders, previous))
            .await
            .map_err(|_| Error::internal_error())?;
        Ok(())
    }

    async fn workspace_documents(
        &self,
    ) -> Result<(Vec<std::path::PathBuf>, HashMap<Uri, Arc<Document>>)> {
        let mut workspace = self.workspace.lock().await;
        // Refresh on demand for clients without watchers and changes to Git rules.
        Self::refresh_workspace(&mut workspace).await?;
        let mut documents: HashMap<Uri, Arc<Document>> = workspace
            .documents
            .iter()
            .filter_map(|(path, doc)| {
                let uri = url::Url::from_file_path(path).ok()?.as_str().parse().ok()?;
                Some((uri, doc.clone()))
            })
            .collect();
        let roots = workspace.roots.clone();
        drop(workspace);
        for (uri, snapshot) in self.documents.read().unwrap().iter() {
            documents.insert(uri.clone(), snapshot.doc.clone());
        }
        Ok((roots, documents))
    }

    async fn refresh_diagnostics(&self) -> Result<()> {
        // Serialize diagnostic batches; buffers can still update while disk I/O
        // runs. Only publish results whose immutable snapshot is still current.
        let _guard = self.diagnostics.lock().await;
        let (_, mut documents) = self.workspace_documents().await?;
        let mut open: Vec<_> = self
            .documents
            .read()
            .unwrap()
            .iter()
            .map(|(uri, snapshot)| (uri.clone(), snapshot.clone()))
            .collect();
        open.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        for (uri, snapshot) in &open {
            documents.insert(uri.clone(), snapshot.doc.clone());
        }
        let batches = tokio::task::spawn_blocking(move || {
            let mut resolver = Resolver::new(
                documents
                    .into_iter()
                    .map(|(uri, doc)| (uri.as_str().to_owned(), doc)),
            );
            open.into_iter()
                .map(|(uri, snapshot)| {
                    let diagnostics = link_diagnostics(&uri, &snapshot.doc, &mut resolver);
                    (uri, snapshot, diagnostics)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| Error::internal_error())?;
        for (uri, snapshot, diagnostics) in batches {
            let current = self
                .documents
                .read()
                .unwrap()
                .get(&uri)
                .is_some_and(|current| Arc::ptr_eq(current, &snapshot));
            if current {
                self.client
                    .publish_diagnostics(uri, diagnostics, Some(snapshot.version))
                    .await;
            }
        }
        Ok(())
    }
}

fn link_diagnostics(uri: &Uri, doc: &Document, resolver: &mut Resolver) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<_> = doc
        .diagnostics
        .iter()
        .map(|d| Diagnostic {
            range: range(doc, d.range.clone()),
            severity: Some(match d.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
            }),
            source: Some("djotls".into()),
            message: d.message.clone(),
            ..Default::default()
        })
        .collect();
    for link in &doc.links {
        // Direct local anchors already have precise unresolved-reference
        // diagnostics from the document analyzer. Reference-style destinations
        // still need validation against their resolved file/anchor.
        let reference = doc.references.iter().find(|r| r.full_range == link.range);
        if reference.is_some_and(|r| r.kind == DjotSymbolKind::Anchor) {
            continue;
        }
        let resolution = resolver.resolve(uri.as_str(), &link.destination);
        let mut bytes = link.destination_range.clone();
        let source = &doc.source[bytes.clone()];
        let fragment = reference.is_none().then(|| source.find('#')).flatten();
        let path_end = reference
            .is_none()
            .then(|| source.find(['#', '?']))
            .flatten();
        let (severity, message) = match resolution {
            Resolution::MissingFile(path) => {
                if let Some(end) = path_end {
                    bytes.end = bytes.start + end;
                }
                (
                    DiagnosticSeverity::ERROR,
                    format!("Link target does not exist: {}", path.display()),
                )
            }
            Resolution::MissingAnchor(anchor) => {
                if let Some(fragment) = fragment {
                    bytes.start += fragment + 1;
                }
                (
                    DiagnosticSeverity::ERROR,
                    format!("Anchor not found: {anchor}"),
                )
            }
            Resolution::Unavailable(message) => (DiagnosticSeverity::WARNING, message),
            Resolution::Target { .. } | Resolution::External => continue,
        };
        diagnostics.push(Diagnostic {
            range: range(doc, bytes),
            severity: Some(severity),
            source: Some("djotls".into()),
            message,
            ..Default::default()
        });
    }
    diagnostics
}

fn position(doc: &Document, offset: usize) -> Position {
    let (line, character) = doc.lines.position(&doc.source, offset);
    Position::new(line, character)
}

fn range(doc: &Document, bytes: ByteRange<usize>) -> Range {
    Range::new(position(doc, bytes.start), position(doc, bytes.end))
}

fn offset(doc: &Document, pos: Position) -> Result<usize> {
    doc.lines
        .offset(&doc.source, pos.line, pos.character)
        .ok_or_else(|| Error::invalid_params("Invalid UTF-16 position"))
}

/// URI resolution deliberately does not consult the filesystem. It works for
/// unopened files and preserves query/fragment components for the client.
fn link_target(base: &Uri, destination: &str) -> Option<Uri> {
    if destination.is_empty() || destination.starts_with('#') {
        return None;
    }
    let target = url::Url::parse(destination)
        .or_else(|_| url::Url::parse(base.as_str())?.join(destination))
        .ok()?;
    target.as_str().parse().ok()
}

fn uri_path(uri: &Uri) -> Option<std::path::PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

/// Link destinations use the source document's directory as their base.
fn heading_destination(base: &Uri, target: &Uri, anchor: &str) -> Option<String> {
    const ANCHOR: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    let anchor = percent_encoding::utf8_percent_encode(anchor, ANCHOR);
    if base == target {
        return Some(format!("#{anchor}"));
    }
    let base = url::Url::parse(base.as_str()).ok()?;
    let mut target = url::Url::parse(target.as_str()).ok()?;
    if base.scheme() != "file" || target.scheme() != "file" {
        return None;
    }
    target.set_fragment(None);
    target.set_query(None);
    let relative = base.make_relative(&target)?;
    Some(format!(
        "{}#{anchor}",
        relative.replace('(', "%28").replace(')', "%29")
    ))
}

fn snippet_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.watch_files.store(
            params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|w| w.did_change_watched_files.as_ref())
                .and_then(|w| w.dynamic_registration)
                .unwrap_or(false),
            Ordering::Relaxed,
        );
        let mut folders: Vec<_> = params
            .workspace_folders
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| uri_path(&f.uri))
            .collect();
        #[allow(deprecated)]
        if folders.is_empty() {
            if let Some(path) = params.root_uri.as_ref().and_then(uri_path) {
                folders.push(path);
            } else if let Some(path) = params.root_path {
                folders.push(path.into());
            }
        }
        {
            let mut workspace = self.workspace.lock().await;
            workspace.folders = folders;
            Self::refresh_workspace(&mut workspace).await?;
        }
        let text = params.capabilities.text_document.as_ref();
        self.snippets.store(
            text.and_then(|t| t.completion.as_ref())
                .and_then(|c| c.completion_item.as_ref())
                .and_then(|i| i.snippet_support)
                .unwrap_or(false),
            Ordering::Relaxed,
        );
        self.nested_symbols.store(
            text.and_then(|t| t.document_symbol.as_ref())
                .and_then(|s| s.hierarchical_document_symbol_support)
                .unwrap_or(false),
            Ordering::Relaxed,
        );
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    ..Default::default()
                }),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: Default::default(),
                }),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "[".into(),
                        "^".into(),
                        "#".into(),
                        "(".into(),
                        "/".into(),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            offset_encoding: None,
            server_info: Some(ServerInfo {
                name: "djotls".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.watch_files.load(Ordering::Relaxed) {
            let registration = Registration {
                id: "djotls-workspace-files".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: Some(
                    serde_json::json!({"watchers":[{"globPattern":"**/*.dj"},{"globPattern":"**/*","kind":5}]}),
                ),
            };
            if let Err(error) = self.client.register_capability(vec![registration]).await {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("File watching unavailable: {error}"),
                    )
                    .await;
            }
        }
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let mut workspace = self.workspace.lock().await;
        for folder in params.event.removed {
            if let Some(path) = uri_path(&folder.uri) {
                workspace.folders.retain(|p| p != &path);
            }
        }
        for folder in params.event.added {
            if let Some(path) = uri_path(&folder.uri) {
                if !workspace.folders.contains(&path) {
                    workspace.folders.push(path);
                }
            }
        }
        drop(workspace);
        let _ = self.refresh_diagnostics().await;
    }

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {
        let _ = self.refresh_diagnostics().await;
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        let _ = self.refresh_diagnostics().await;
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let (roots, documents) = self.workspace_documents().await?;
        let query = params.query.to_lowercase();
        let mut symbols = Vec::new();
        let mut add = |uri: &Uri, doc: &Document| {
            let path = uri_path(uri);
            let display_path = path
                .as_ref()
                .and_then(|path| {
                    roots
                        .iter()
                        .filter_map(|root| path.strip_prefix(root).ok())
                        .min_by_key(|relative| relative.components().count())
                        .map(|relative| {
                            relative
                                .components()
                                .map(|component| component.as_os_str().to_string_lossy())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                        .or_else(|| {
                            path.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                        })
                })
                .unwrap_or_else(|| uri.as_str().to_owned());
            for heading in &doc.headings {
                let title = if heading.title.is_empty() {
                    "(untitled)"
                } else {
                    &heading.title
                };
                let name = format!("{title} ({display_path}#{})", heading.id);
                if !name.to_lowercase().contains(&query) {
                    continue;
                }
                #[allow(deprecated)]
                symbols.push(SymbolInformation {
                    name,
                    kind: SymbolKind::STRING,
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri.clone(), range(doc, heading.selection.clone())),
                    container_name: None,
                });
            }
        };
        for (uri, doc) in &documents {
            add(uri, doc);
        }
        symbols.sort_by(|a, b| {
            a.location
                .uri
                .as_str()
                .cmp(b.location.uri.as_str())
                .then(
                    a.location
                        .range
                        .start
                        .line
                        .cmp(&b.location.range.start.line),
                )
                .then(
                    a.location
                        .range
                        .start
                        .character
                        .cmp(&b.location.range.start.character),
                )
        });
        Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let item = params.text_document;
        let snapshot = Arc::new(Snapshot {
            version: item.version,
            doc: Arc::new(Document::new(item.text)),
        });
        self.documents
            .write()
            .unwrap()
            .insert(item.uri.clone(), snapshot.clone());
        let _ = self.refresh_diagnostics().await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Update synchronously before the first await. Requests snapshot the
        // resulting immutable document; no lock is held across protocol I/O.
        {
            let mut docs = self.documents.write().unwrap();
            let Some(old) = docs.get(&params.text_document.uri) else {
                return;
            };
            if params.text_document.version <= old.version {
                return;
            }
            let mut source = old.doc.source.clone();
            for change in params.content_changes {
                if let Some(range) = change.range {
                    let index = LineIndex::new(&source);
                    let Some(start) =
                        index.offset(&source, range.start.line, range.start.character)
                    else {
                        return;
                    };
                    let Some(end) = index.offset(&source, range.end.line, range.end.character)
                    else {
                        return;
                    };
                    if start > end {
                        return;
                    }
                    source.replace_range(start..end, &change.text);
                } else {
                    source = change.text;
                }
            }
            let snapshot = Arc::new(Snapshot {
                version: params.text_document.version,
                doc: Arc::new(Document::new(source)),
            });
            docs.insert(params.text_document.uri.clone(), snapshot);
        }
        let _ = self.refresh_diagnostics().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents
            .write()
            .unwrap()
            .remove(&params.text_document.uri);
        {
            let _guard = self.diagnostics.lock().await;
            self.client
                .publish_diagnostics(params.text_document.uri, vec![], None)
                .await;
        }
        let _ = self.refresh_diagnostics().await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let p = params.text_document_position_params;
        let snapshot = self.snapshot(&p.text_document.uri)?;
        let doc = &snapshot.doc;
        let at = offset(doc, p.position)?;
        // Labels, footnotes and same-document anchor references keep their
        // precise local targets. Do not let a surrounding heading's broad name
        // range hide a link inside that heading.
        if let Some(reference) = doc.references.iter().find(|r| r.name_range.contains(&at)) {
            return Ok(reference.target.map(|target| {
                GotoDefinitionResponse::Scalar(Location::new(
                    p.text_document.uri.clone(),
                    range(doc, doc.definitions[target].name_range.clone()),
                ))
            }));
        }
        let Some(link) = doc.links.iter().find(|link| link.range.contains(&at)) else {
            return Ok(doc.target_at(at).map(|target| {
                GotoDefinitionResponse::Scalar(Location::new(
                    p.text_document.uri.clone(),
                    range(doc, doc.definitions[target].name_range.clone()),
                ))
            }));
        };
        let destination = link.destination.clone();
        let (_, mut documents) = self.workspace_documents().await?;
        documents.insert(p.text_document.uri.clone(), snapshot.doc.clone());
        let base = p.text_document.uri.as_str().to_owned();
        let resolution = tokio::task::spawn_blocking(move || {
            Resolver::new(
                documents
                    .into_iter()
                    .map(|(uri, doc)| (uri.as_str().to_owned(), doc)),
            )
            .resolve(&base, &destination)
        })
        .await
        .map_err(|_| Error::internal_error())?;
        let Resolution::Target {
            uri,
            document,
            selection,
        } = resolution
        else {
            return Ok(None);
        };
        let uri = uri.as_str().parse().map_err(|_| Error::internal_error())?;
        let target_range = document.map_or_else(Range::default, |doc| range(&doc, selection));
        Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
            uri,
            target_range,
        ))))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        let snapshot = self.snapshot(&uri)?;
        let links = snapshot
            .doc
            .links
            .iter()
            .filter_map(|link| {
                let target = link_target(&uri, &link.destination)?;
                Some(DocumentLink {
                    range: range(&snapshot.doc, link.range.clone()),
                    target: Some(target),
                    tooltip: None,
                    data: None,
                })
            })
            .collect();
        Ok(Some(links))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let p = params.text_document_position;
        let snapshot = self.snapshot(&p.text_document.uri)?;
        let doc = &snapshot.doc;
        let Some(target) = doc.target_at(offset(doc, p.position)?) else {
            return Ok(None);
        };
        let mut locations: Vec<_> = doc
            .references_to(target)
            .map(|r| {
                Location::new(
                    p.text_document.uri.clone(),
                    range(doc, r.name_range.clone()),
                )
            })
            .collect();
        if params.context.include_declaration {
            locations.push(Location::new(
                p.text_document.uri,
                range(doc, doc.definitions[target].name_range.clone()),
            ));
        }
        Ok(Some(locations))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let snapshot = self.snapshot(&params.text_document.uri)?;
        let doc = &snapshot.doc;
        // Djot line width is independent of LSP's indentation tab_size.
        let width = params
            .options
            .properties
            .get("maxColumns")
            .and_then(|v| match v {
                FormattingProperty::Number(n) if *n > 0 => Some(*n as u64),
                _ => None,
            })
            .filter(|&v| v > 0)
            .unwrap_or(72) as usize;
        let output = fmt::format(&doc.source, &FmtConfig { max_cols: width });
        if output == doc.source {
            return Ok(Some(vec![]));
        }
        Ok(Some(vec![TextEdit::new(
            range(doc, 0..doc.source.len()),
            output,
        )]))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let snapshot = self.snapshot(&params.text_document.uri)?;
        let doc = &snapshot.doc;
        if !self.nested_symbols.load(Ordering::Relaxed) {
            #[allow(deprecated)]
            let flat = doc
                .headings
                .iter()
                .map(|h| SymbolInformation {
                    name: if h.title.is_empty() {
                        "(untitled)".into()
                    } else {
                        h.title.clone()
                    },
                    kind: SymbolKind::STRING,
                    tags: None,
                    deprecated: None,
                    location: Location::new(
                        params.text_document.uri.clone(),
                        range(doc, h.range.clone()),
                    ),
                    container_name: h.parent.map(|p| doc.headings[p].title.clone()),
                })
                .collect();
            return Ok(Some(DocumentSymbolResponse::Flat(flat)));
        }
        let mut children: HashMap<Option<usize>, Vec<DocumentSymbol>> = HashMap::new();
        for (i, h) in doc.headings.iter().enumerate().rev() {
            let end = doc
                .headings
                .iter()
                .skip(i + 1)
                .find(|next| next.level <= h.level)
                .map_or(doc.source.len(), |next| next.range.start);
            let mut nested = children.remove(&Some(i)).unwrap_or_default();
            nested.reverse();
            #[allow(deprecated)]
            let symbol = DocumentSymbol {
                name: if h.title.is_empty() {
                    "(untitled)".into()
                } else {
                    h.title.clone()
                },
                detail: None,
                kind: SymbolKind::STRING,
                tags: None,
                deprecated: None,
                range: range(doc, h.range.start..end),
                selection_range: range(doc, h.selection.clone()),
                children: (!nested.is_empty()).then_some(nested),
            };
            children.entry(h.parent).or_default().push(symbol);
        }
        let mut roots = children.remove(&None).unwrap_or_default();
        roots.reverse();
        Ok(Some(DocumentSymbolResponse::Nested(roots)))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let p = params.text_document_position;
        let snapshot = self.snapshot(&p.text_document.uri)?;
        let doc = &snapshot.doc;
        let Some(context) = doc.completion_context(offset(doc, p.position)?) else {
            return Ok(None);
        };
        if context.kind == CompletionKind::Destination {
            let (_, mut documents) = self.workspace_documents().await?;
            documents.insert(p.text_document.uri.clone(), snapshot.doc.clone());
            let mut items = Vec::new();
            let prefix = context.prefix.to_lowercase();
            for (uri, target) in documents {
                let local = uri == p.text_document.uri;
                for heading in &target.headings {
                    let Some(destination) =
                        heading_destination(&p.text_document.uri, &uri, &heading.id)
                    else {
                        continue;
                    };
                    if !destination.to_lowercase().starts_with(&prefix)
                        && !heading.title.to_lowercase().contains(&prefix)
                    {
                        continue;
                    }
                    let title = if heading.title.is_empty() {
                        "(untitled)"
                    } else {
                        &heading.title
                    };
                    items.push(CompletionItem {
                        label: format!("{title} ({destination})"),
                        kind: Some(CompletionItemKind::REFERENCE),
                        detail: Some(destination.clone()),
                        sort_text: Some(format!(
                            "{}:{title}:{destination}",
                            if local { 0 } else { 1 }
                        )),
                        filter_text: Some(
                            if !prefix.is_empty() && destination.to_lowercase().starts_with(&prefix)
                            {
                                destination.clone()
                            } else {
                                title.to_string()
                            },
                        ),
                        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                        text_edit: Some(
                            TextEdit::new(
                                range(doc, context.replace.clone()),
                                format!("{destination})"),
                            )
                            .into(),
                        ),
                        ..Default::default()
                    });
                }
            }
            items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
            // Requery as the user types: the same candidate can be matched by
            // its title or destination, with an appropriate client filter text.
            return Ok(Some(CompletionResponse::List(CompletionList {
                is_incomplete: true,
                items,
            })));
        }
        let snippets = self.snippets.load(Ordering::Relaxed);
        let mut items = Vec::new();
        if context.kind == CompletionKind::Template {
            for (label, snippet, plain) in [
                (
                    "Inline link",
                    "[${1:text}](${2:destination})",
                    "[text](destination)",
                ),
                ("Reference link", "[${1:text}][${2:label}]", "[text][label]"),
            ] {
                items.push(CompletionItem {
                    label: label.into(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    insert_text_format: Some(if snippets {
                        InsertTextFormat::SNIPPET
                    } else {
                        InsertTextFormat::PLAIN_TEXT
                    }),
                    text_edit: Some(
                        TextEdit::new(
                            range(doc, context.replace.clone()),
                            if snippets { snippet } else { plain }.into(),
                        )
                        .into(),
                    ),
                    ..Default::default()
                });
            }
        }
        let mut keys = HashSet::new();
        for d in &doc.definitions {
            let applicable = match context.kind {
                CompletionKind::Link | CompletionKind::Template => {
                    matches!(d.kind, DjotSymbolKind::Link | DjotSymbolKind::Heading)
                }
                CompletionKind::Footnote => d.kind == DjotSymbolKind::Footnote,
                CompletionKind::Anchor => d.kind == DjotSymbolKind::Anchor,
                CompletionKind::Destination => false,
            };
            if !applicable || !d.key.starts_with(&context.prefix) || !keys.insert(d.key.clone()) {
                continue;
            }
            let new_text = if context.kind == CompletionKind::Template {
                if snippets {
                    format!(
                        "[${{1:{}}}][{}]",
                        snippet_escape(&d.key),
                        snippet_escape(&d.key)
                    )
                } else {
                    format!("[{}][{}]", d.key, d.key)
                }
            } else {
                format!("{}{}", d.key, context.suffix)
            };
            items.push(CompletionItem {
                label: d.key.clone(),
                detail: (!d.destination.is_empty()).then(|| d.destination.clone()),
                kind: Some(CompletionItemKind::REFERENCE),
                insert_text_format: Some(if snippets && context.kind == CompletionKind::Template {
                    InsertTextFormat::SNIPPET
                } else {
                    InsertTextFormat::PLAIN_TEXT
                }),
                text_edit: Some(
                    TextEdit::new(range(doc, context.replace.clone()), new_text).into(),
                ),
                ..Default::default()
            });
        }
        Ok(Some(CompletionResponse::Array(items)))
    }
}
