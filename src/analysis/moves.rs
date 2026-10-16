// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plan file/folder moves against original URIs, without writing any files.
use super::{
    rename::{Edit, FileEdits},
    Document, SymbolKind,
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};
use url::Url;

struct Move {
    old: Url,
    old_path: PathBuf,
    new_path: PathBuf,
}

pub struct Moves(Vec<Move>);

fn same_target(a: &Url, b: &Url) -> bool {
    match (a.to_file_path(), b.to_file_path()) {
        (Ok(a_path), Ok(b_path)) => {
            a_path == b_path && a.query() == b.query() && a.fragment() == b.fragment()
        }
        _ => a == b,
    }
}

fn file_uri(text: &str) -> Result<(Url, PathBuf), String> {
    let uri = Url::parse(text).map_err(|_| "Invalid file URI")?;
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err("File-operation URIs must not contain queries or fragments".into());
    }
    let path = uri
        .to_file_path()
        .map_err(|_| "Cannot resolve local file URI")?;
    Ok((uri, path))
}

impl Moves {
    pub fn new(files: impl IntoIterator<Item = (String, String)>) -> Result<Self, String> {
        let mut moves: Vec<Move> = Vec::new();
        for (old, new) in files {
            let (old, old_path) = file_uri(&old)?;
            let (_, new_path) = file_uri(&new)?;
            if old_path == new_path {
                continue;
            }
            if new_path.starts_with(&old_path) {
                return Err("Cannot move a path into itself".into());
            }
            for m in &moves {
                if old_path.starts_with(&m.old_path) || m.old_path.starts_with(&old_path) {
                    return Err("Overlapping move sources are ambiguous".into());
                }
                if new_path == m.new_path {
                    return Err("Multiple moves have the same destination".into());
                }
            }
            moves.push(Move {
                old,
                old_path,
                new_path,
            });
        }
        Ok(Self(moves))
    }

    /// Apply the batch once, not sequentially: A -> B and B -> C keeps each
    /// link attached to its original target. Path components delimit folders.
    pub fn relocate(&self, uri: &Url) -> Url {
        let Ok(path) = uri.to_file_path() else {
            return uri.clone();
        };
        for m in &self.0 {
            if let Ok(suffix) = path.strip_prefix(&m.old_path) {
                let path = if suffix.as_os_str().is_empty() {
                    m.new_path.clone()
                } else {
                    m.new_path.join(suffix)
                };
                let mut result = Url::from_file_path(path).expect("an absolute local path");
                if uri.path().ends_with('/') && !result.path().ends_with('/') {
                    result.set_path(&format!("{}/", result.path()));
                }
                result.set_query(uri.query());
                result.set_fragment(uri.fragment());
                return result;
            }
        }
        uri.clone()
    }

    /// Call on a blocking worker: explicit moved .dj files outside the index
    /// also need outgoing-link edits. Folder traversal stays with the index.
    pub fn plan(
        &self,
        mut documents: HashMap<String, Arc<Document>>,
    ) -> Result<HashMap<String, FileEdits>, String> {
        if self.0.is_empty() {
            return Ok(HashMap::new());
        }
        for m in &self.0 {
            if documents.keys().any(|uri| {
                Url::parse(uri)
                    .ok()
                    .and_then(|u| u.to_file_path().ok())
                    .is_some_and(|p| p == m.old_path)
            }) || m.old_path.extension().is_none_or(|e| e != "dj")
            {
                continue;
            }
            match std::fs::metadata(&m.old_path) {
                Ok(metadata) if metadata.is_file() => {
                    let source = std::fs::read_to_string(&m.old_path)
                        .map_err(|e| format!("Cannot read moved Djot file: {e}"))?;
                    documents.insert(m.old.to_string(), Arc::new(Document::new(source)));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Cannot inspect moved Djot file: {e}")),
            }
        }
        let mut destinations = HashSet::new();
        for uri in documents.keys() {
            let Ok(uri) = Url::parse(uri) else { continue };
            if self
                .relocate(&uri)
                .to_file_path()
                .is_ok_and(|path| !destinations.insert(path))
            {
                return Err("The moves would overwrite another document".into());
            }
        }
        let mut result = HashMap::new();
        for (uri, doc) in documents {
            let Ok(base) = Url::parse(&uri) else { continue };
            let new_base = self.relocate(&base);
            let mut edits = Vec::new();
            for link in &doc.links {
                if doc
                    .references
                    .iter()
                    .any(|r| r.kind == SymbolKind::Link && r.full_range == link.range)
                {
                    continue;
                }
                let Ok(target) = base.join(&link.destination) else {
                    continue;
                };
                if target.scheme() != "file" {
                    continue;
                }
                let mut new_target = self.relocate(&target);
                // Preserve source spelling whenever it still resolves to the
                // correct target after both source and target have moved.
                if new_base
                    .join(&link.destination)
                    .is_ok_and(|u| same_target(&u, &new_target))
                {
                    continue;
                }
                new_target.set_fragment(None);
                new_target.set_query(None);
                let raw = &doc.source[link.destination_range.clone()];
                let end = raw.find(['?', '#']).unwrap_or(raw.len());
                let path = if Url::parse(&link.destination).is_ok() {
                    new_target.to_string()
                } else if link.destination.starts_with('/') {
                    new_target.path().to_string()
                } else {
                    new_base
                        .make_relative(&new_target)
                        .ok_or("Cannot make a relative link to the moved file")?
                };
                let path = if path.is_empty() && end == raw.len() {
                    new_target
                        .path_segments()
                        .and_then(|mut p| p.next_back())
                        .unwrap_or("")
                        .to_string()
                } else {
                    path
                };
                edits.push(Edit {
                    range: link.destination_range.start..link.destination_range.start + end,
                    text: path.replace('(', "%28").replace(')', "%29"),
                });
            }
            edits.retain(|edit| doc.source[edit.range.clone()] != edit.text);
            if !edits.is_empty() {
                result.insert(
                    uri,
                    FileEdits {
                        document: doc,
                        edits,
                    },
                );
            }
        }
        Ok(result)
    }
}

pub struct LinkPath {
    pub selection: Range<usize>,
    pub placeholder: String,
    pub uri: Url,
}

/// Only physical destination paths can initiate a file rename. Labels and
/// fragments have their own semantics; missing targets cannot be moved.
pub fn link_path(doc: &Document, base: &str, at: usize) -> Option<LinkPath> {
    let link = doc.links.iter().find(|l| {
        l.destination_range.contains(&at)
            && !doc
                .references
                .iter()
                .any(|r| r.kind == SymbolKind::Link && r.full_range == l.range)
    })?;
    let raw = &doc.source[link.destination_range.clone()];
    let end = raw.find(['?', '#']).unwrap_or(raw.len());
    let selection = link.destination_range.start..link.destination_range.start + end;
    if !selection.contains(&at) {
        return None;
    }
    let mut uri = Url::parse(base).ok()?.join(&link.destination).ok()?;
    uri.set_fragment(None);
    uri.set_query(None);
    let path = uri.to_file_path().ok()?;
    std::fs::symlink_metadata(path).ok()?;
    Some(LinkPath {
        selection,
        placeholder: raw[..end].to_string(),
        uri,
    })
}

pub fn destination(base: &str, name: &str) -> Result<Url, String> {
    if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
        return Err(
            "The new path must be nonempty without surrounding whitespace or control characters"
                .into(),
        );
    }
    let base = Url::parse(base).map_err(|_| "Invalid source URI")?;
    let uri = base.join(name).map_err(|_| "Invalid destination URI")?;
    let (uri, _) = file_uri(uri.as_str())?;
    Ok(uri)
}
