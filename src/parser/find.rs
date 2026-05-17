// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use regex::bytes::{Regex, RegexBuilder};

/// Compile a pattern into a Regex (equivalent to djot.js's `pattern()`).
///
/// Unicode mode is disabled so that byte-level matching works correctly
/// when startpos lands inside a multi-byte UTF-8 character (e.g. NBSP's
/// continuation byte 0xA0).  All Djot syntax delimiters are ASCII, so
/// this is safe.
pub fn pattern(patt: &str) -> Regex {
    RegexBuilder::new(patt)
        .unicode(false)
        .build()
        .unwrap_or_else(|e| panic!("Invalid regex pattern {}: {}", patt, e))
}

/// Like `find`, but returns only the position without capturing groups.
/// Use this when you only need to know *where* the match is (or that it
/// exists at all) to avoid the overhead of allocating a `Vec<String>`.
pub fn find_pos(
    subject: &str,
    patt: &Regex,
    startpos: usize,
    endpos: Option<usize>,
) -> Option<(usize, usize)> {
    let byte_end = match endpos {
        Some(ep) => (ep + 1).min(subject.len()),
        None => subject.len(),
    };

    if startpos >= byte_end {
        return None;
    }

    let bytes = subject.as_bytes();
    let slice = &bytes[startpos..byte_end];

    if let Some(caps) = patt.captures(slice) {
        if let Some(m) = caps.get(0) {
            if m.start() == 0 {
                return Some((startpos, startpos + m.end() - 1));
            }
        }
    }
    None
}

/// Find a pattern match in subject starting at startpos, bounded by endpos.
/// All positions are **byte offsets**.
///
/// CRITICAL: djot.js uses the `y` (sticky) flag, meaning the pattern must match
/// at exactly startpos, not later in the string. We emulate this by slicing the
/// subject from startpos and checking for a match at position 0.
///
/// Uses regex::bytes so startpos can land inside a multi-byte UTF-8 character
/// without panicking. Since all Djot syntax delimiters are ASCII, multi-byte
/// bytes (0x80-0xFF) never match any delimiter pattern.
pub fn find(
    subject: &str,
    patt: &Regex,
    startpos: usize,
    endpos: Option<usize>,
) -> Option<(usize, usize, Vec<String>)> {
    let byte_end = match endpos {
        Some(ep) => (ep + 1).min(subject.len()),
        None => subject.len(),
    };

    if startpos >= byte_end {
        return None;
    }

    let bytes = subject.as_bytes();
    let slice = &bytes[startpos..byte_end];

    if let Some(caps) = patt.captures(slice) {
        if let Some(m) = caps.get(0) {
            if m.start() == 0 {
                let sp = startpos;
                let ep = startpos + m.end() - 1;
                let mut captures = Vec::new();
                for i in 1..caps.len() {
                    if let Some(c) = caps.get(i) {
                        // Captured content from patterns anchored to ASCII
                        // delimiters is always valid UTF-8
                        let s = std::str::from_utf8(c.as_bytes()).unwrap_or("");
                        captures.push(s.to_string());
                    } else {
                        captures.push(String::new());
                    }
                }
                return Some((sp, ep, captures));
            }
        }
    }
    None
}
