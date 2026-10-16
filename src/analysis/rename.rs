// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plan semantic heading/ID renames against immutable workspace snapshots.
//! This module returns source edits; it never writes files.
use super::{
    links::{Resolution, Resolver},
    normalize_label, Document, SymbolKind,
};
use std::{collections::HashMap, ops::Range, sync::Arc};

#[derive(Clone, Copy)]
enum Kind {
    Heading(usize),
    Anchor(usize),
}

pub struct Target {
    pub uri: String,
    pub document: Arc<Document>,
    pub selection: Range<usize>,
    pub placeholder: String,
    kind: Kind,
}

pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

pub struct FileEdits {
    pub document: Arc<Document>,
    pub edits: Vec<Edit>,
}

pub struct Rename {
    documents: HashMap<String, Arc<Document>>,
    resolver: Resolver,
}

fn inline_text(text: &str) -> String {
    text.chars().fold(String::new(), |mut result, ch| {
        if ch.is_ascii_punctuation() {
            result.push('\\');
        }
        result.push(ch);
        result
    })
}

fn intersects(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

impl Rename {
    pub fn new(documents: HashMap<String, Arc<Document>>) -> Self {
        let resolver = Resolver::new(
            documents
                .iter()
                .map(|(uri, doc)| (uri.clone(), doc.clone())),
        );
        Self {
            documents,
            resolver,
        }
    }

    fn definition_target(uri: &str, doc: Arc<Document>, definition: usize) -> Option<Target> {
        let d = &doc.definitions[definition];
        let kind = match d.kind {
            SymbolKind::Heading => {
                Kind::Heading(doc.headings.iter().position(|h| h.range == d.full_range)?)
            }
            SymbolKind::Anchor => doc
                .headings
                .iter()
                .position(|h| h.selection == d.name_range)
                .map(Kind::Heading)
                .unwrap_or(Kind::Anchor(definition)),
            _ => return None,
        };
        let placeholder = match kind {
            Kind::Heading(i) => doc.headings[i].title.clone(),
            Kind::Anchor(_) => d.key.clone(),
        };
        Some(Target {
            uri: uri.into(),
            selection: d.name_range.clone(),
            document: doc,
            placeholder,
            kind,
        })
    }

    /// A link fragment can initiate the same rename as its anchor definition.
    /// Paths, ordinary link labels, code and unresolved targets are excluded.
    pub fn target(&mut self, uri: &str, at: usize) -> Option<Target> {
        let doc = self.documents.get(uri)?.clone();
        if let Some(reference) = doc.references.iter().find(|r| r.name_range.contains(&at)) {
            let mut target = Self::definition_target(uri, doc.clone(), reference.target?)?;
            target.selection = reference.name_range.clone();
            return Some(target);
        }
        if let Some(link) = doc.links.iter().find(|l| l.range.contains(&at)) {
            let raw = &doc.source[link.destination_range.clone()];
            let start = link.destination_range.start + raw.find('#')? + 1;
            let selection = start..link.destination_range.end;
            if !selection.contains(&at) {
                return None;
            }
            let Resolution::Target {
                uri,
                document: Some(document),
                selection: definition,
            } = self.resolver.resolve(uri, &link.destination)
            else {
                return None;
            };
            let index = document
                .definitions
                .iter()
                .position(|d| d.kind == SymbolKind::Anchor && d.name_range == definition)?;
            let mut target = Self::definition_target(uri.as_str(), document.clone(), index)?;
            target.selection = selection;
            self.documents.insert(uri.to_string(), document);
            return Some(target);
        }
        let definition = doc
            .definitions
            .iter()
            .position(|d| d.kind == SymbolKind::Anchor && d.name_range.contains(&at))
            .or_else(|| {
                doc.definitions
                    .iter()
                    .position(|d| d.kind == SymbolKind::Heading && d.name_range.contains(&at))
            })?;
        Self::definition_target(uri, doc, definition)
    }

    pub fn plan(
        &mut self,
        target: Target,
        name: &str,
    ) -> Result<HashMap<String, FileEdits>, String> {
        if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err("The new name must be nonempty, without surrounding whitespace or control characters".into());
        }
        if name == target.placeholder {
            return Ok(HashMap::new());
        }
        let old = &target.document;
        let (primary, replacement) = match target.kind {
            Kind::Heading(i) => (old.headings[i].selection.clone(), inline_text(name)),
            Kind::Anchor(i) => (old.definitions[i].name_range.clone(), name.to_string()),
        };
        let mut source = old.source.clone();
        source.replace_range(primary.clone(), &replacement);
        let renamed = Document::new(source);
        let anchors = |doc: &Document| {
            doc.definitions
                .iter()
                .enumerate()
                .filter(|(_, d)| d.kind == SymbolKind::Anchor)
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        };
        let before = anchors(old);
        let after = anchors(&renamed);
        if old.headings.len() != renamed.headings.len() || before.len() != after.len() {
            return Err(
                "The rename would change document structure or remove an explicit ID".into(),
            );
        }
        let valid = match target.kind {
            Kind::Heading(i) => renamed.headings[i].title == name,
            Kind::Anchor(i) => before
                .iter()
                .position(|&j| j == i)
                .is_some_and(|j| renamed.definitions[after[j]].key == name),
        };
        if !valid {
            return Err("The new name is not valid Djot heading text or an ID".into());
        }
        // Reparse the whole target: changing one heading/ID can renumber other
        // generated anchors. Match definitions by identity/order, never by name.
        let mut changed = HashMap::new();
        for (&a, &b) in before.iter().zip(&after) {
            let prev = &old.definitions[a];
            let next = &renamed.definitions[b];
            if prev.key != next.key {
                if after
                    .iter()
                    .any(|&i| i != b && renamed.definitions[i].key == next.key)
                {
                    return Err(format!(
                        "The rename would create a duplicate anchor: {}",
                        next.key
                    ));
                }
                changed.insert(
                    (prev.name_range.start, prev.name_range.end),
                    next.key.clone(),
                );
            }
        }
        let mut result = HashMap::new();
        for (uri, doc) in &self.documents {
            let mut edits = Vec::new();
            if uri == &target.uri {
                edits.push(Edit {
                    range: primary.clone(),
                    text: replacement.clone(),
                });
                for reference in &old.references {
                    if intersects(&primary, &reference.name_range) {
                        continue;
                    }
                    let Some(definition) = reference.target.map(|i| &old.definitions[i]) else {
                        continue;
                    };
                    if definition.kind != SymbolKind::Heading {
                        continue;
                    }
                    let index = old
                        .headings
                        .iter()
                        .position(|h| h.range == definition.full_range)
                        .unwrap();
                    let key = normalize_label(&renamed.headings[index].title);
                    // Explicit labels override titles; otherwise the first
                    // heading wins. Never silently retarget implicit references.
                    let winner = renamed
                        .definitions
                        .iter()
                        .rev()
                        .find(|d| d.kind == SymbolKind::Link && d.key == key)
                        .or_else(|| {
                            renamed
                                .definitions
                                .iter()
                                .find(|d| d.kind == SymbolKind::Heading && d.key == key)
                        });
                    if !winner.is_some_and(|d| {
                        d.kind == SymbolKind::Heading
                            && d.full_range == renamed.headings[index].range
                    }) {
                        return Err(format!(
                            "The rename would make heading references ambiguous: {key}"
                        ));
                    }
                    if key != definition.key {
                        // Expand [Old][] to [Old][New], preserving display
                        // text. In particular, a reference inside a different
                        // heading must not change that heading's own title/ID.
                        let range = if reference.name_range.end < reference.full_range.end - 1 {
                            reference.full_range.end - 1..reference.full_range.end - 1
                        } else {
                            reference.name_range.clone()
                        };
                        edits.push(Edit {
                            range,
                            text: inline_text(&key),
                        });
                    }
                }
            }
            for link in &doc.links {
                if uri == &target.uri && intersects(&primary, &link.range) {
                    continue;
                }
                // Reference uses inherit a destination; edit its definition,
                // not the label or visible text at each use.
                if doc
                    .references
                    .iter()
                    .any(|r| r.kind == SymbolKind::Link && r.full_range == link.range)
                {
                    continue;
                }
                let Resolution::Target {
                    uri: resolved,
                    selection,
                    ..
                } = self.resolver.resolve(uri, &link.destination)
                else {
                    continue;
                };
                if resolved.as_str() != target.uri {
                    continue;
                }
                let Some(key) = changed.get(&(selection.start, selection.end)) else {
                    continue;
                };
                let raw = &doc.source[link.destination_range.clone()];
                let Some(hash) = raw.find('#') else { continue };
                const FRAGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
                    .remove(b'-')
                    .remove(b'_')
                    .remove(b'.')
                    .remove(b'~');
                edits.push(Edit {
                    range: link.destination_range.start + hash + 1..link.destination_range.end,
                    text: percent_encoding::utf8_percent_encode(key, FRAGMENT).to_string(),
                });
            }
            edits.retain(|edit| doc.source[edit.range.clone()] != edit.text);
            edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
            edits.dedup_by(|a, b| a.range == b.range && a.text == b.text);
            if edits
                .windows(2)
                .any(|pair| intersects(&pair[0].range, &pair[1].range))
            {
                return Err("The rename would require overlapping source edits".into());
            }
            if !edits.is_empty() {
                result.insert(
                    uri.clone(),
                    FileEdits {
                        document: doc.clone(),
                        edits,
                    },
                );
            }
        }
        Ok(result)
    }
}
