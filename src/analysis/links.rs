// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Resolve local links against immutable open-buffer snapshots and disk files.
//! Call on a blocking worker: filesystem failures are distinct from missing
//! targets, and no network requests are made.
use super::{Document, SymbolKind};
use std::{collections::HashMap, ops::Range, path::PathBuf, sync::Arc};
use url::Url;

pub enum Resolution {
    Target {
        uri: Url,
        document: Option<Arc<Document>>,
        selection: Range<usize>,
    },
    MissingFile(PathBuf),
    MissingAnchor(String),
    Unavailable(String),
    External,
}

#[derive(Clone)]
enum File {
    Djot(Arc<Document>),
    Other,
    Missing,
    Unavailable(String),
}

pub struct Resolver {
    files: HashMap<PathBuf, File>,
    documents: HashMap<Url, Arc<Document>>,
}

impl Resolver {
    pub fn new(documents: impl IntoIterator<Item = (String, Arc<Document>)>) -> Self {
        let mut files = HashMap::new();
        let mut snapshots = HashMap::new();
        for (uri, doc) in documents {
            let Ok(mut uri) = Url::parse(&uri) else {
                continue;
            };
            uri.set_fragment(None);
            uri.set_query(None);
            if let Ok(path) = uri.to_file_path() {
                files.insert(path, File::Djot(doc.clone()));
            }
            snapshots.insert(uri, doc);
        }
        Self {
            files,
            documents: snapshots,
        }
    }

    pub fn resolve(&mut self, base: &str, destination: &str) -> Resolution {
        let Ok(base) = Url::parse(base) else {
            return Resolution::External;
        };
        let target = Url::parse(destination).or_else(|_| base.join(destination));
        let Ok(mut target) = target else {
            return Resolution::External;
        };
        let mut document_uri = target.clone();
        document_uri.set_fragment(None);
        document_uri.set_query(None);
        if target.scheme() != "file" && !self.documents.contains_key(&document_uri) {
            return Resolution::External;
        }
        let fragment = target
            .fragment()
            .filter(|f| !f.is_empty())
            .map(str::to_owned);
        target.set_fragment(None);
        target.set_query(None);
        // Non-file buffers can still resolve their own local fragments.
        let file = if let Some(doc) = self.documents.get(&target) {
            File::Djot(doc.clone())
        } else if target.scheme() == "file" {
            let Ok(path) = target.to_file_path() else {
                return Resolution::Unavailable("Cannot resolve local file URI".into());
            };
            self.files
                .entry(path.clone())
                .or_insert_with(|| read_file(&path))
                .clone()
        } else {
            return Resolution::External;
        };
        match file {
            File::Missing => Resolution::MissingFile(target.to_file_path().unwrap()),
            File::Unavailable(message) => Resolution::Unavailable(message),
            File::Other => Resolution::Target {
                uri: target,
                document: None,
                selection: 0..0,
            },
            File::Djot(doc) => {
                let selection = if let Some(fragment) = fragment {
                    let fragment =
                        match percent_encoding::percent_decode_str(&fragment).decode_utf8() {
                            Ok(fragment) => fragment.into_owned(),
                            Err(_) => {
                                return Resolution::Unavailable(
                                    "Target fragment is not valid UTF-8".into(),
                                )
                            }
                        };
                    // Explicit IDs and generated heading anchors share the same
                    // last-definition-wins rule as document-local navigation.
                    let Some(definition) = doc
                        .definitions
                        .iter()
                        .rev()
                        .find(|d| d.kind == SymbolKind::Anchor && d.key == fragment)
                    else {
                        return Resolution::MissingAnchor(fragment);
                    };
                    definition.name_range.clone()
                } else {
                    0..0
                };
                Resolution::Target {
                    uri: target,
                    document: Some(doc),
                    selection,
                }
            }
        }
    }
}

fn read_file(path: &PathBuf) -> File {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return file_error(error),
    };
    if !metadata.is_file() || path.extension().is_none_or(|extension| extension != "dj") {
        return File::Other;
    }
    match std::fs::read_to_string(path) {
        Ok(source) => File::Djot(Arc::new(Document::new(source))),
        Err(error) => file_error(error),
    }
}

fn file_error(error: std::io::Error) -> File {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    ) {
        File::Missing
    } else {
        File::Unavailable(format!("Cannot read link target: {error}"))
    }
}
