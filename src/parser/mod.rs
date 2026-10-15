// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod attributes;
mod block;
mod find;
mod inline;

/// A parsing event, identical in structure to djot.js's Event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub startpos: usize,
    pub endpos: usize,
    pub annot: String,
}

/// Parse a Djot document into an event stream compatible with djot.js.
///
/// Events are produced lazily and consumed once. Collect the iterator when
/// repeated access is needed. Paragraph-level inline parsing still buffers a
/// paragraph before emitting its inline events.
///
/// Positions are inclusive UTF-8 byte offsets. A missing final newline is
/// appended internally, so an event may refer to that virtual newline.
pub fn parse_events(input: &str) -> impl std::iter::FusedIterator<Item = Event> + Send + use<> {
    let text = if input.ends_with('\n') {
        input.to_string()
    } else {
        format!("{}\n", input)
    };
    block::EventParser::new(text.into())
}
