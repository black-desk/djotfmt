// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Editor analysis directly over parser events; no dependency on an LSP runtime.
//! Snapshots own source and events. Context is computed on demand rather than
//! retaining a syntax tree. Ranges are half-open UTF-8 byte offsets.

pub mod text;
use crate::parser::{parse_events, Event};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};
use text::{source_range, LineIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Link,
    Footnote,
    Anchor,
    Heading,
}

#[derive(Debug, Clone)]
pub struct Definition {
    pub kind: SymbolKind,
    pub key: String,
    pub name_range: Range<usize>,
    pub full_range: Range<usize>,
    pub destination: String,
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub kind: SymbolKind,
    pub key: String,
    pub name_range: Range<usize>,
    pub full_range: Range<usize>,
    pub target: Option<usize>,
}

/// A source link with its decoded destination, before resolving a document URI.
#[derive(Debug, Clone)]
pub struct Link {
    pub range: Range<usize>,
    pub destination: String,
}

#[derive(Debug, Clone)]
pub struct Heading {
    pub id: String,
    pub level: usize,
    pub title: String,
    pub range: Range<usize>,
    pub selection: Range<usize>,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub kind: CompletionKind,
    pub replace: Range<usize>,
    pub prefix: String,
    pub suffix: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Destination,
    Link,
    Footnote,
    Anchor,
    Template,
}

pub struct Document {
    pub source: String,
    pub events: Vec<Event>,
    pub lines: LineIndex,
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    pub links: Vec<Link>,
    pub headings: Vec<Heading>,
    pub diagnostics: Vec<Diagnostic>,
    reverse: HashMap<usize, Vec<usize>>,
}

#[derive(Debug)]
struct Span {
    tag: String,
    open: usize,
    close: usize,
}

fn base(annot: &str) -> &str {
    annot.split('|').next().unwrap_or(annot)
}

fn spans(events: &[Event]) -> Vec<Span> {
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut result = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let annot = base(&e.annot);
        if let Some(tag) = annot.strip_prefix('+') {
            stack.push((tag.to_string(), i));
        } else if let Some(tag) = annot.strip_prefix('-') {
            if let Some(idx) = stack.iter().rposition(|(name, _)| name == tag) {
                let (tag, open) = stack.remove(idx);
                result.push(Span {
                    tag,
                    open,
                    close: i,
                });
            }
        }
    }
    result
}

/// djot.js label normalization preserves case and collapses ASCII whitespace.
pub fn normalize_label(s: &str) -> String {
    s.trim()
        .split([' ', '\t', '\r', '\n'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn plain_text(source: &str, events: &[Event]) -> String {
    let mut text = String::new();
    let mut hidden = 0usize;
    for e in events {
        let annot = base(&e.annot);
        if matches!(
            annot,
            "+destination" | "+reference" | "+attributes" | "+block_attributes"
        ) {
            hidden += 1;
        }
        if hidden > 0 {
            if matches!(
                annot,
                "-destination" | "-reference" | "-attributes" | "-block_attributes"
            ) {
                hidden -= 1;
            }
            continue;
        }
        match annot {
            "str" => text.push_str(&source[source_range(source, e.startpos, e.endpos)]),
            "soft_break" | "hard_break" => text.push('\n'),
            "left_single_quote" | "right_single_quote" => text.push('\''),
            "left_double_quote" | "right_double_quote" => text.push('"'),
            "ellipses" => text.push_str("..."),
            "en_dash" => text.push_str("--"),
            "em_dash" => text.push_str("---"),
            _ => {}
        }
    }
    text
}

fn identifier(title: &str, used: &HashSet<String>) -> String {
    let base = title
        .split(|c: char| c.is_whitespace() || "][~!@#$%^&*(){}`,.<>\\|=+/?".contains(c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let mut id = base.clone();
    let mut n = 0;
    while id.is_empty() || used.contains(&id) {
        n += 1;
        id = format!("{}-{n}", if base.is_empty() { "s" } else { &base });
    }
    id
}

impl Document {
    pub fn new(source: String) -> Self {
        let events = parse_events(&source).collect();
        let lines = LineIndex::new(&source);
        let mut doc = Self {
            source,
            events,
            lines,
            definitions: vec![],
            references: vec![],
            links: vec![],
            headings: vec![],
            diagnostics: vec![],
            reverse: HashMap::new(),
        };
        doc.analyze();
        doc
    }

    fn event_range(&self, i: usize) -> Range<usize> {
        let e = &self.events[i];
        source_range(&self.source, e.startpos, e.endpos)
    }

    fn span_range(&self, span: &Span) -> Range<usize> {
        let start = self.event_range(span.open).start;
        // Most block close events are boundary markers, not source characters.
        let end = if matches!(
            span.tag.as_str(),
            "para"
                | "heading"
                | "footnote"
                | "reference_definition"
                | "list"
                | "list_item"
                | "block_quote"
                | "table"
        ) {
            self.events[span.close].startpos.min(self.source.len())
        } else {
            self.event_range(span.close).end
        };
        start..end.max(start)
    }

    fn analyze(&mut self) {
        let spans = spans(&self.events);
        let mut explicit_ids: HashMap<usize, String> = HashMap::new();
        // Resolve attribute ownership before deriving heading IDs. Ordinary
        // block attributes attach only across adjacent attribute groups.
        for (i, _) in self
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.annot == "id")
        {
            let name_range = self.event_range(i);
            let key = self.source[name_range.clone()].to_string();
            let attr = spans.iter().find(|s| {
                s.open < i
                    && i < s.close
                    && matches!(s.tag.as_str(), "attributes" | "block_attributes")
            });
            let owner = attr.and_then(|attr| {
                if attr.tag == "block_attributes" {
                    self.events
                        .iter()
                        .enumerate()
                        .skip(attr.close + 1)
                        .take_while(|(_, e)| e.annot != "blankline")
                        .find(|(_, e)| {
                            matches!(
                                base(&e.annot),
                                "+heading"
                                    | "+para"
                                    | "+div"
                                    | "+code_block"
                                    | "+block_quote"
                                    | "+list"
                                    | "+table"
                                    | "+footnote"
                                    | "+reference_definition"
                                    | "thematic_break"
                            )
                        })
                        .map(|(i, _)| i)
                } else {
                    spans
                        .iter()
                        .find(|s| s.close + 1 == attr.open && s.tag != "attributes")
                        .map(|s| s.open)
                }
            });
            if attr.is_some_and(|s| s.tag == "block_attributes") && owner.is_none() {
                continue;
            }
            if let Some(owner) = owner {
                explicit_ids.insert(owner, key.clone());
            }
            let bare_word = attr
                .filter(|a| a.tag == "attributes")
                .and_then(|a| a.open.checked_sub(1))
                .filter(|&i| self.events[i].annot == "str")
                .and_then(|i| {
                    let range = self.event_range(i);
                    let text = &self.source[range.clone()];
                    if text.chars().last().is_some_and(char::is_whitespace) {
                        return None;
                    }
                    let start = text.rfind(char::is_whitespace).map_or(range.start, |i| {
                        range.start + i + text[i..].chars().next().unwrap().len_utf8()
                    });
                    Some(start..range.end)
                });
            if owner.is_none() && bare_word.is_none() {
                continue;
            }
            let full_range = owner
                .and_then(|o| spans.iter().find(|s| s.open == o))
                .map(|s| self.span_range(s))
                .or(bare_word)
                .unwrap_or_else(|| name_range.clone());
            self.definitions.push(Definition {
                kind: SymbolKind::Anchor,
                key: key.clone(),
                name_range,
                full_range,
                destination: format!("#{key}"),
            });
        }
        let links: HashMap<_, _> = spans
            .iter()
            .filter(|s| matches!(s.tag.as_str(), "linktext" | "imagetext"))
            .map(|s| (s.close + 1, s))
            .collect();
        let mut ordered: Vec<_> = spans.iter().collect();
        ordered.sort_by_key(|s| s.close);
        let mut used = HashSet::new();
        let mut processed_id = 0;
        for span in ordered {
            for e in &self.events[processed_id..=span.close] {
                if e.annot == "id" {
                    used.insert(
                        self.source[source_range(&self.source, e.startpos, e.endpos)].to_string(),
                    );
                }
            }
            processed_id = span.close + 1;
            let range = self.span_range(span);
            let inner = &self.events[span.open + 1..span.close];
            match span.tag.as_str() {
                "reference_definition" => {
                    if let Some(key_event) = inner.iter().find(|e| e.annot == "reference_key") {
                        let key_range = source_range(
                            &self.source,
                            key_event.startpos + 1,
                            key_event.endpos.saturating_sub(1),
                        );
                        let key = normalize_label(&self.source[key_range.clone()]);
                        let destination = inner
                            .iter()
                            .filter(|e| e.annot == "reference_value")
                            .map(|e| &self.source[source_range(&self.source, e.startpos, e.endpos)])
                            .collect::<String>();
                        if !key.is_empty() {
                            let values: Vec<_> = inner
                                .iter()
                                .filter(|e| e.annot == "reference_value")
                                .collect();
                            if let (Some(first), Some(last)) = (values.first(), values.last()) {
                                self.links.push(Link {
                                    range: source_range(&self.source, first.startpos, last.endpos),
                                    destination: destination.clone(),
                                });
                            }
                            self.definitions.push(Definition {
                                kind: SymbolKind::Link,
                                key,
                                name_range: key_range,
                                full_range: range,
                                destination,
                            });
                        }
                    }
                }
                "footnote" => {
                    if let Some(e) = inner.iter().find(|e| e.annot == "note_label") {
                        let name_range = source_range(&self.source, e.startpos, e.endpos);
                        self.definitions.push(Definition {
                            kind: SymbolKind::Footnote,
                            key: normalize_label(&self.source[name_range.clone()]),
                            name_range,
                            full_range: range,
                            destination: String::new(),
                        });
                    }
                }
                "heading" => {
                    let title = plain_text(&self.source, inner).trim().to_string();
                    let id = explicit_ids
                        .get(&span.open)
                        .cloned()
                        .unwrap_or_else(|| identifier(&title, &used));
                    used.insert(id.clone());
                    let selection = inner
                        .first()
                        .map(|e| {
                            let start = source_range(&self.source, e.startpos, e.endpos).start;
                            let end = range.end.max(start);
                            let trimmed = self.source[start..end].trim_end();
                            start..start + trimmed.len()
                        })
                        .unwrap_or_else(|| range.clone());
                    if !explicit_ids.contains_key(&span.open) {
                        self.definitions.push(Definition {
                            kind: SymbolKind::Anchor,
                            key: id.clone(),
                            name_range: selection.clone(),
                            full_range: range.clone(),
                            destination: format!("#{id}"),
                        });
                    }
                    self.definitions.push(Definition {
                        kind: SymbolKind::Heading,
                        key: normalize_label(&title),
                        name_range: selection.clone(),
                        full_range: range.clone(),
                        destination: format!("#{id}"),
                    });
                    let open = &self.events[span.open];
                    self.headings.push(Heading {
                        id,
                        level: open.endpos + 1 - open.startpos,
                        title,
                        range,
                        selection,
                        parent: None,
                    });
                }
                "reference" | "destination" => {
                    let text = plain_text(&self.source, inner);
                    let is_reference = span.tag == "reference";
                    let link = links.get(&span.open);
                    let Some(link) = link else {
                        continue;
                    };
                    let full_range = self.event_range(link.open).start..range.end;
                    if !is_reference {
                        let destination = text.replace(['\r', '\n'], "");
                        self.links.push(Link {
                            range: full_range.clone(),
                            destination: destination.clone(),
                        });
                        if !destination.starts_with('#') {
                            continue;
                        }
                    }
                    let name_start = self.event_range(span.open).end;
                    let name_end = self.event_range(span.close).start;
                    let mut name_range = name_start.min(name_end)..name_end;
                    let key = if is_reference && text.is_empty() {
                        name_range =
                            self.event_range(link.open).end..self.event_range(link.close).start;
                        normalize_label(&plain_text(
                            &self.source,
                            &self.events[link.open + 1..link.close],
                        ))
                    } else if is_reference {
                        normalize_label(&text)
                    } else {
                        name_range.start = (name_range.start + 1).min(name_range.end);
                        percent_encoding::percent_decode_str(&text[1..])
                            .decode_utf8()
                            .map(|key| key.into_owned())
                            .unwrap_or_else(|_| text[1..].to_string())
                    };
                    self.references.push(Reference {
                        kind: if is_reference {
                            SymbolKind::Link
                        } else {
                            SymbolKind::Anchor
                        },
                        key,
                        name_range,
                        full_range,
                        target: None,
                    });
                }
                _ => {}
            }
        }
        for e in self
            .events
            .iter()
            .filter(|e| e.annot == "footnote_reference")
        {
            let full_range = source_range(&self.source, e.startpos, e.endpos);
            let name_range = (full_range.start + 2).min(full_range.end)
                ..full_range.end.saturating_sub(1).max(full_range.start + 2);
            self.references.push(Reference {
                kind: SymbolKind::Footnote,
                key: normalize_label(&self.source[name_range.clone()]),
                name_range,
                full_range,
                target: None,
            });
        }
        self.references.sort_by_key(|r| r.name_range.start);
        self.headings.sort_by_key(|h| h.range.start);
        let mut parents: Vec<usize> = vec![];
        for i in 0..self.headings.len() {
            while parents
                .last()
                .is_some_and(|&p| self.headings[p].level >= self.headings[i].level)
            {
                parents.pop();
            }
            self.headings[i].parent = parents.last().copied();
            parents.push(i);
        }
        let mut lookup = HashMap::new();
        // Implicit heading labels use the first heading; explicit definitions
        // override implicit labels and later explicit definitions win.
        for (i, d) in self
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == SymbolKind::Heading)
        {
            lookup.entry((SymbolKind::Link, d.key.clone())).or_insert(i);
        }
        let mut seen = HashSet::new();
        for (i, d) in self
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind != SymbolKind::Heading)
        {
            let key = (d.kind, d.key.clone());
            if !seen.insert(key.clone()) {
                self.diagnostics.push(Diagnostic {
                    range: d.name_range.clone(),
                    message: format!("Duplicate definition: {}", d.key),
                });
            }
            lookup.insert(key, i);
        }
        for (i, r) in self.references.iter_mut().enumerate() {
            r.target = lookup.get(&(r.kind, r.key.clone())).copied();
            if let Some(target) = r.target {
                self.reverse.entry(target).or_default().push(i);
            } else {
                self.diagnostics.push(Diagnostic {
                    range: r.name_range.clone(),
                    message: format!("Unresolved reference: {}", r.key),
                });
            }
        }
        // Resolve reference-style links using exactly the same precedence as
        // definition navigation; unresolved references are not clickable.
        for reference in &self.references {
            if reference.kind == SymbolKind::Link {
                if let Some(target) = reference.target {
                    self.links.push(Link {
                        range: reference.full_range.clone(),
                        destination: self.definitions[target].destination.clone(),
                    });
                }
            }
        }
        self.links.sort_by_key(|link| link.range.start);
    }

    pub fn target_at(&self, offset: usize) -> Option<usize> {
        self.references
            .iter()
            .find(|r| r.name_range.contains(&offset))
            .and_then(|r| r.target)
            .or_else(|| {
                self.definitions
                    .iter()
                    .position(|d| d.name_range.contains(&offset))
            })
    }

    pub fn references_to(&self, definition: usize) -> impl Iterator<Item = &Reference> {
        let selected = &self.definitions[definition];
        self.definitions
            .iter()
            .enumerate()
            .filter(move |(i, d)| {
                *i == definition
                    || (matches!(selected.kind, SymbolKind::Heading | SymbolKind::Anchor)
                        && matches!(d.kind, SymbolKind::Heading | SymbolKind::Anchor)
                        && d.full_range == selected.full_range)
            })
            .flat_map(|(i, _)| self.reverse.get(&i).into_iter().flatten())
            .map(|&i| &self.references[i])
    }

    /// Query only when requested. Pairing uses event order, not binary search:
    /// block close markers are not necessarily sorted by source offset.
    pub fn completion_context(&self, offset: usize) -> Option<CompletionContext> {
        if offset > self.source.len() || !self.source.is_char_boundary(offset) {
            return None;
        }
        let spans = spans(&self.events);
        if spans.iter().any(|s| {
            matches!(
                s.tag.as_str(),
                "code_block"
                    | "verbatim"
                    | "inline_math"
                    | "display_math"
                    | "url"
                    | "email"
                    | "attributes"
                    | "block_attributes"
            ) && {
                let range = self.span_range(s);
                range.start < offset
                    && (offset < range.end
                        || (offset == range.end
                            && (s.tag == "code_block"
                                && self.events[s.close].startpos >= self.source.len())))
            }
        }) {
            return None;
        }
        let start = spans
            .iter()
            .filter(|s| {
                matches!(s.tag.as_str(), "para" | "heading" | "caption" | "cell") && {
                    let range = self.span_range(s);
                    range.start <= offset && offset <= range.end
                }
            })
            .map(|s| self.event_range(s.open).start)
            .max()
            .unwrap_or_else(|| self.source[..offset].rfind('\n').map_or(0, |i| i + 1));
        let before = &self.source[start..offset];
        let bracket = before.rfind('[')?;
        let absolute = start + bracket;
        if spans.iter().any(|s| {
            matches!(
                s.tag.as_str(),
                "code_block"
                    | "verbatim"
                    | "inline_math"
                    | "display_math"
                    | "url"
                    | "email"
                    | "attributes"
                    | "block_attributes"
            ) && self.span_range(s).contains(&absolute)
        }) {
            return None;
        }
        if escaped(&self.source, absolute) {
            return None;
        }
        let tail = &before[bracket + 1..];
        let after = &self.source[offset..];
        let anchor = before.rfind("](#").filter(|&i| i >= bracket);
        let (kind, prefix, name_start, close) = if let Some(anchor) = anchor {
            (
                CompletionKind::Anchor,
                &before[anchor + 3..],
                start + anchor + 3,
                ')',
            )
        } else if let Some(destination) = before.rfind("](").filter(|&i| i >= bracket) {
            (
                CompletionKind::Destination,
                &before[destination + 2..],
                start + destination + 2,
                ')',
            )
        } else if let Some(prefix) = tail.strip_prefix('^') {
            (CompletionKind::Footnote, prefix, absolute + 2, ']')
        } else if bracket > 0 && before.as_bytes()[bracket - 1] == b']' {
            (CompletionKind::Link, tail, absolute + 1, ']')
        } else if tail.is_empty() {
            (CompletionKind::Template, tail, absolute, ']')
        } else {
            return None;
        };
        if prefix.contains(['[', ']', '(', ')', '\r', '\n']) {
            return None;
        }
        let existing_end = after.find([close, '\n', '\r']);
        // Replace the remainder only when a closing delimiter is present on
        // this line; otherwise do not swallow unrelated text after the cursor.
        let replace_end = existing_end
            .filter(|&i| after.as_bytes()[i] == close as u8)
            .map(|i| offset + i + 1)
            .unwrap_or(offset);
        let replace_end = if matches!(kind, CompletionKind::Template | CompletionKind::Link)
            && after.starts_with("]()")
        {
            offset + 3
        } else if kind == CompletionKind::Link && after.starts_with(')') {
            offset + 1
        } else if kind == CompletionKind::Template {
            if after.starts_with(']') {
                offset + 1
            } else {
                offset
            }
        } else {
            replace_end
        };
        Some(CompletionContext {
            kind,
            replace: name_start..replace_end,
            prefix: prefix.to_string(),
            suffix: if matches!(kind, CompletionKind::Anchor | CompletionKind::Destination) {
                ")"
            } else {
                "]"
            },
        })
    }
}

fn escaped(source: &str, offset: usize) -> bool {
    source[..offset]
        .bytes()
        .rev()
        .take_while(|&b| b == b'\\')
        .count()
        % 2
        == 1
}
