// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ops::Range;

/// LSP-independent line index. All internal offsets are UTF-8 bytes.
#[derive(Debug, Clone)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    /// Convert a UTF-16 LSP position; reject invalid lines, split surrogate
    /// pairs and positions past the line instead of silently changing edits.
    pub fn offset(&self, text: &str, line: u32, character: u32) -> Option<usize> {
        let start = *self.starts.get(line as usize)?;
        let end = self
            .starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let content = text[start..end].trim_end_matches(['\r', '\n']);
        let mut units = 0;
        for (i, c) in content.char_indices() {
            if units == character {
                return Some(start + i);
            }
            units += c.len_utf16() as u32;
            if units > character {
                return None;
            }
        }
        (units == character).then_some(start + content.len())
    }

    pub fn position(&self, text: &str, offset: usize) -> (u32, u32) {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let end = text[self.starts[line]..offset].trim_end_matches('\r');
        (line as u32, end.encode_utf16().count() as u32)
    }
}

/// Convert the parser's inclusive byte positions, including virtual newline
/// positions, into valid half-open ranges in the original source.
pub fn source_range(text: &str, start: usize, end: usize) -> Range<usize> {
    let mut start = start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = end.saturating_add(1).min(text.len()).max(start);
    while !text.is_char_boundary(end) {
        end += 1;
    }
    start..end
}
