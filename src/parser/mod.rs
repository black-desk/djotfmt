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
/// Internally uses byte offsets for O(1) character access.
pub fn parse_events(input: &str) -> Vec<Event> {
    let text = if input.ends_with('\n') {
        input.to_string()
    } else {
        format!("{}\n", input)
    };
    block::parse(&text)
}
