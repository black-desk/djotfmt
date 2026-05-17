// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Faithful port of djot.js/src/inline.ts

use crate::parser::attributes::AttributeParser;
use crate::parser::find;
use crate::parser::Event;
use regex::bytes::Regex;

// All patterns compiled once. Unicode disabled for byte-level matching.
lazy_static::lazy_static! {
    static ref RE_SPECIAL: Regex = find::pattern(r#"[\r\n"'()*+.:<=\[\\\]^_`${}~-]"#);
    static ref PATT_LINE_END: Regex = find::pattern(r"[ \t]*\r?\n");
    static ref PATT_AUTO_LINK: Regex = find::pattern(r"<([^<>\s]+)>");
    static ref PATT_SYMBOL: Regex = find::pattern(r":[\w_+-]+:");
    static ref PATT_TWO_PERIODS: Regex = find::pattern(r"\.\.");
    static ref PATT_BACKTICKS0: Regex = find::pattern(r"`*");
    static ref PATT_BACKTICKS1: Regex = find::pattern(r"`+");
    static ref PATT_DOUBLE_DOLLARS: Regex = find::pattern(r"\$\$");
    static ref PATT_SINGLE_DOLLAR: Regex = find::pattern(r"\$");
    static ref PATT_RAW_ATTRIBUTE: Regex = find::pattern(r"\{=[^\s{}`]+\}");
    static ref PATT_NOTE_REFERENCE: Regex = find::pattern(r"\^([^\]]+)\]");
    static ref PATT_BACKSLASH: Regex = find::pattern(r"\\");
    static ref PATT_PUNCTUATION: Regex = find::pattern(r"[!-/:-@\[-`{-~]");
    static ref PATT_DELIM: Regex = find::pattern(r#"[_*~^+='"-]"#);
    static ref PATT_NONSPACE: Regex = find::pattern(r"[^ \t\r\n]");
}

const C_TAB: u32 = 9;
const C_LF: u32 = 10;
const C_CR: u32 = 13;
const C_SPACE: u32 = 32;
const C_BANG: u32 = 33;
const C_DOUBLE_QUOTE: u32 = 34;
const C_DOLLARS: u32 = 36;
const C_SINGLE_QUOTE: u32 = 39;
const C_LEFT_PAREN: u32 = 40;
const C_RIGHT_PAREN: u32 = 41;
const C_ASTERISK: u32 = 42;
const C_PLUS: u32 = 43;
const C_HYPHEN: u32 = 45;
const C_PERIOD: u32 = 46;
const C_COLON: u32 = 58;
const C_LESSTHAN: u32 = 60;
const C_EQUALS: u32 = 61;
const C_LEFT_BRACKET: u32 = 91;
const C_BACKSLASH: u32 = 92;
const C_RIGHT_BRACKET: u32 = 93;
const C_HAT: u32 = 94;
const C_UNDERSCORE: u32 = 95;
const C_BACKTICK: u32 = 96;
const C_LEFT_BRACE: u32 = 123;
const C_RIGHT_BRACE: u32 = 125;
const C_TILDE: u32 = 126;

struct Opener {
    match_index: usize,
    startpos: usize,
    endpos: usize,
    annot: Option<String>,
    sub_match_index: usize,
    substartpos: Option<usize>,
    subendpos: Option<usize>,
}

type OpenerMap = Vec<(String, Vec<Opener>)>;

/// Get the byte value at `pos` as a u32 code point. O(1).
/// Returns 0 for out-of-bounds positions; callers rely on 0 not
/// matching any ASCII constant (space, newline, etc.).
fn cp(subject: &str, pos: usize) -> u32 {
    subject.as_bytes().get(pos).copied().unwrap_or(0) as u32
}

fn find_special(subject: &str, startpos: usize, endpos: usize) -> Option<usize> {
    let subj = &subject.as_bytes()[startpos..];
    for mat in RE_SPECIAL.find_iter(subj) {
        let byte_pos = startpos + mat.start();
        if byte_pos > endpos {
            break;
        }
        return Some(byte_pos);
    }
    None
}

fn has_brace(subject: &str, pos: usize) -> bool {
    (pos > 0 && cp(subject, pos - 1) == C_LEFT_BRACE)
        || cp(subject, pos + 1) == C_RIGHT_BRACE
}

pub struct InlineParser<'a> {
    subject: &'a str,
    matches: Vec<Event>,
    openers: OpenerMap,
    pub verbatim: usize,
    verbatim_type: String,
    destination: bool,
    pub firstpos: isize,
    pub lastpos: usize,
    allow_attributes: bool,
    attribute_parser: Option<AttributeParser<'a>>,
    attribute_start: Option<usize>,
    pub attribute_slices: Option<Vec<(usize, usize)>>,
}

impl<'a> InlineParser<'a> {
    pub fn new(subject: &'a str) -> Self {
        InlineParser {
            subject,
            matches: Vec::new(),
            openers: Vec::new(),
            verbatim: 0,
            verbatim_type: String::new(),
            destination: false,
            firstpos: -1,
            lastpos: 0,
            allow_attributes: true,
            attribute_parser: None,
            attribute_start: None,
            attribute_slices: None,
        }
    }

    fn add_match(&mut self, startpos: usize, endpos: usize, annot: &str) {
        self.matches.push(Event {
            startpos,
            endpos,
            annot: annot.to_string(),
        });
    }

    fn add_match_at(&mut self, idx: usize, startpos: usize, endpos: usize, annot: &str) {
        if idx < self.matches.len() {
            self.matches[idx] = Event {
                startpos,
                endpos,
                annot: annot.to_string(),
            };
        }
    }

    fn single_char(&mut self, pos: usize) -> usize {
        self.add_match(pos, pos, "str");
        pos + 1
    }

    pub fn reparse_attributes(&mut self) {
        let slices = self.attribute_slices.take();
        if slices.is_none() {
            return;
        }
        let slices = slices.unwrap();
        self.allow_attributes = false;
        self.attribute_parser = None;
        self.attribute_start = None;
        for (sp, ep) in slices {
            self.feed(sp, ep);
        }
        self.allow_attributes = true;
    }

    pub fn get_matches(mut self) -> Vec<Event> {
        let subject = self.subject;
        if self.attribute_parser.is_some() {
            self.reparse_attributes();
        }

        // remove trailing softbreak and any spaces
        let len = self.matches.len();
        if len > 0 && self.matches[len - 1].annot == "soft_break" {
            self.matches.pop();
            if let Some(last) = self.matches.last() {
                if last.annot == "str" && cp(subject, last.endpos) == C_SPACE {
                    let sp = last.startpos;
                    let ep = self.matches.last_mut().unwrap();
                    while ep.endpos >= sp && cp(subject, ep.endpos) == C_SPACE {
                        ep.endpos -= 1;
                    }
                    if ep.endpos < sp {
                        self.matches.pop();
                    }
                }
            }
        }

        // add -verbatim if needed (unclosed verbatim)
        if !self.matches.is_empty() && self.verbatim > 0 {
            let last = self.matches.last().unwrap();
            self.matches.push(Event {
                startpos: last.endpos,
                endpos: last.endpos,
                annot: format!("-{}", self.verbatim_type),
            });
        }

        // Remove any placeholder markers from image fixup, then sort by position
        self.matches.retain(|m| m.annot != "__remove__");
        self.matches.sort_by_key(|m| m.startpos);

        self.matches
    }

    fn add_opener(
        &mut self,
        name: &str,
        startpos: usize,
        endpos: usize,
        default_annot: &str,
    ) {
        let match_index = self.matches.len();
        self.add_match(startpos, endpos, default_annot);

        if let Some(entry) = self.openers.iter_mut().find(|(k, _)| *k == name) {
            entry.1.push(Opener {
                match_index,
                startpos,
                endpos,
                annot: None,
                sub_match_index: self.matches.len(),
                substartpos: None,
                subendpos: None,
            });
        } else {
            self.openers.push((
                name.to_string(),
                vec![Opener {
                    match_index,
                    startpos,
                    endpos,
                    annot: None,
                    sub_match_index: self.matches.len(),
                    substartpos: None,
                    subendpos: None,
                }],
            ));
        }
    }

    fn clear_openers(&mut self, startpos: usize, endpos: usize) {
        for (_k, v) in self.openers.iter_mut() {
            let mut i = v.len();
            while i > 0 {
                i -= 1;
                let opener = &v[i];
                if opener.startpos >= startpos && opener.endpos <= endpos {
                    v.remove(i);
                } else if let (Some(subsp), Some(subep)) = (opener.substartpos, opener.subendpos) {
                    if subsp >= startpos && subep <= endpos {
                        v[i].substartpos = None;
                        v[i].subendpos = None;
                        v[i].annot = None;
                    }
                } else {
                    break;
                }
            }
        }
    }

    fn str_matches(&mut self, startpos: usize, endpos: usize) {
        let mut i = self.matches.len();
        if i == 0 {
            return;
        }
        i -= 1;
        while i > 0 && self.matches[i].startpos >= startpos {
            i -= 1;
        }
        if self.matches[i].startpos < startpos {
            i += 1;
        }
        while i < self.matches.len() && self.matches[i].endpos <= endpos {
            if self.matches[i].annot != "escape" && self.matches[i].annot != "str" {
                self.matches[i].annot = "str".to_string();
            }
            i += 1;
        }
    }

    /// betweenMatched: handles emphasis, strong, subscript, superscript, etc.
    fn between_matched(
        &mut self,
        c_char: &str,
        annotation: &str,
        defaultmatch: &str,
        opentest: fn(&str, usize) -> bool,
        pos: usize,
        endpos: usize,
    ) -> usize {
        let subject = self.subject;
        let co_find = find::find_pos(subject, &PATT_NONSPACE, pos + 1, None);
        // NOTE: JS calls find(subject, pattNonspace, pos - 1) unconditionally,
        // relying on the regex engine to return null for pos == -1. We guard
        // with pos > 0 instead, which is equivalent (can_close = false at pos 0).
        let cc_find = if pos > 0 {
            find::find_pos(subject, &PATT_NONSPACE, pos - 1, None)
        } else { None };
        let mut can_open = co_find.is_some()
            && opentest(subject, pos);
        let mut can_close = pos > 0 && cc_find.is_some();

        let lastmatch = self.matches.last();
        let has_open_marker = lastmatch.map(|m| m.annot == "open_marker").unwrap_or(false);
        let has_close_marker = pos + 1 <= endpos && cp(subject, pos + 1) == C_RIGHT_BRACE;

        let mut endcloser = pos;
        let mut startopener = pos;
        let mut defaultmatch = defaultmatch.to_string();

        if has_open_marker {
            startopener = if pos > 0 { pos - 1 } else { pos };
            can_open = true;
            can_close = false;
        }
        if !has_open_marker && has_close_marker {
            endcloser = if pos + 1 <= endpos { pos + 1 } else { pos };
            can_close = true;
            can_open = false;
        }

        if has_open_marker && defaultmatch.starts_with("right") {
            defaultmatch = defaultmatch.replacen("right", "left", 1);
        } else if has_close_marker && defaultmatch.starts_with("left") {
            defaultmatch = defaultmatch.replacen("left", "right", 1);
        }

        // NOTE: djot.js uses "{-" as the opener key for close-marker cases,
        // while we use "{-}". Both are internally consistent (keys always
        // match against keys we produce ourselves), so this deviation is
        // harmless but worth noting for anyone comparing against djot.js.
        let d = if has_close_marker {
            format!("{{{}}}", c_char)
        } else {
            c_char.to_string()
        };

        if can_close {
            // Pre-check link openers for destination check (before mutable borrow)
            let link_opener_startpos: Option<usize> = if self.destination {
                self.openers.iter()
                    .find(|(k, _)| *k == "[")
                    .and_then(|v| v.1.last())
                    .filter(|o| o.annot.as_deref() == Some("explicit_link"))
                    .map(|o| o.startpos)
            } else {
                None
            };

            let mut matched = false;
            let mut opener_match_index = 0;
            let mut opener_startpos = 0;
            let mut opener_endpos = 0;

            // check openers for a match
            if let Some(opener_vec) = self.openers.iter_mut().find(|(k, _)| *k == d) {
                if !opener_vec.1.is_empty() {
                    let last_idx = opener_vec.1.len() - 1;
                    let o_endpos = opener_vec.1[last_idx].endpos;
                    let o_startpos = opener_vec.1[last_idx].startpos;

                    if o_endpos != pos.saturating_sub(1) {
                        // exclude empty emph
                        let skip = if let Some(link_sp) = link_opener_startpos {
                            o_startpos < link_sp
                        } else {
                            false
                        };

                        if skip {
                            self.add_match(pos, endcloser, &defaultmatch);
                            return endcloser + 1;
                        }

                        opener_match_index = opener_vec.1[last_idx].match_index;
                        opener_startpos = o_startpos;
                        opener_endpos = o_endpos;
                        matched = true;
                        opener_vec.1.pop();
                    }
                }
            }

            if matched {
                self.clear_openers(opener_startpos, pos);
                self.add_match_at(opener_match_index, opener_startpos, opener_endpos, &format!("+{}", annotation));
                self.add_match(pos, endcloser, &format!("-{}", annotation));
                return endcloser + 1;
            }
        }

        // If we get here, we didn't match an opener
        if can_open {
            // NOTE: same key format deviation as the close-marker case above.
            let e = if has_open_marker {
                format!("{{{}}}", c_char)
            } else {
                c_char.to_string()
            };
            self.add_opener(&e, startopener, pos, &defaultmatch);
            pos + 1
        } else {
            self.add_match(pos, endcloser, &defaultmatch);
            endcloser + 1
        }
    }

    fn handle_right_bracket(&mut self, pos: usize, endpos: usize) -> Option<usize> {
        let subject = self.subject;

        // Find the [ openers
        let ob_idx = self.openers.iter().position(|(k, _)| *k == "[");
        if ob_idx.is_none() {
            return None;
        }
        let ob_idx = ob_idx.unwrap();

        if self.openers[ob_idx].1.is_empty() {
            return None;
        }

        let last_idx = self.openers[ob_idx].1.len() - 1;
        let annot = self.openers[ob_idx].1[last_idx].annot.clone();
        let opener_startpos = self.openers[ob_idx].1[last_idx].startpos;
        let opener_endpos = self.openers[ob_idx].1[last_idx].endpos;
        let opener_match_index = self.openers[ob_idx].1[last_idx].match_index;
        let opener_sub_match_index = self.openers[ob_idx].1[last_idx].sub_match_index;
        let opener_substartpos = self.openers[ob_idx].1[last_idx].substartpos;
        let opener_subendpos = self.openers[ob_idx].1[last_idx].subendpos;

        if annot.as_deref() == Some("reference_link") {
            // found a reference link
            let sub_sp = opener_substartpos.unwrap_or(opener_startpos);
            let sub_ep = opener_subendpos.unwrap_or(opener_endpos);
            // convert all matches inside reference to str
            self.str_matches(sub_ep + 1, pos - 1);

            let is_image = opener_startpos > 0
                && cp(subject, opener_startpos - 1) == C_BANG
                && (opener_startpos < 2 || cp(subject, opener_startpos - 2) != C_BACKSLASH);

            if is_image {
                // Trim the preceding str match to exclude '!', then append
                // image_marker instead of replacing it (which would lose text).
                let prev = &mut self.matches[opener_match_index - 1];
                if prev.annot == "str" && prev.endpos >= opener_startpos - 1 {
                    prev.endpos = opener_startpos.saturating_sub(2);
                    if prev.endpos < prev.startpos {
                        // Was only the '!' — remove entirely
                        self.matches[opener_match_index - 1].annot = "__remove__".to_string();
                    }
                }
                self.add_match(opener_startpos - 1, opener_startpos - 1, "image_marker");
                self.add_match_at(opener_match_index, opener_startpos, opener_endpos, "+imagetext");
                self.add_match_at(opener_sub_match_index, sub_sp, sub_sp, "-imagetext");
            } else {
                self.add_match_at(opener_match_index, opener_startpos, opener_endpos, "+linktext");
                self.add_match_at(opener_sub_match_index, sub_sp, sub_sp, "-linktext");
            }

            let sub_ep = opener_subendpos.unwrap_or(opener_endpos);
            self.add_match_at(opener_sub_match_index + 1, sub_ep, sub_ep, "+reference");
            self.add_match(pos, pos, "-reference");
            self.clear_openers(opener_startpos, pos);
            return Some(pos + 1);
        } else if pos + 1 <= endpos && cp(subject, pos + 1) == C_LEFT_BRACKET {
            self.openers[ob_idx].1[last_idx].annot = Some("reference_link".to_string());
            self.add_match(pos, pos, "str");
            self.openers[ob_idx].1[last_idx].sub_match_index = self.matches.len() - 1;
            self.add_match(pos + 1, pos + 1, "str");
            self.openers[ob_idx].1[last_idx].substartpos = Some(pos);
            self.openers[ob_idx].1[last_idx].subendpos = Some(pos + 1);
            let sp = self.openers[ob_idx].1[last_idx].startpos + 1;
            self.clear_openers(sp, pos - 1);
            return Some(pos + 2);
        } else if pos + 1 <= endpos && cp(subject, pos + 1) == C_LEFT_PAREN {
            // clear ( openers
            if let Some(parens_idx) = self.openers.iter_mut().position(|(k, _)| *k == "(") {
                self.openers[parens_idx].1.clear();
            }
            self.openers[ob_idx].1[last_idx].annot = Some("explicit_link".to_string());
            self.add_match(pos, pos, "str");
            self.openers[ob_idx].1[last_idx].sub_match_index = self.matches.len() - 1;
            self.add_match(pos + 1, pos + 1, "str");
            self.openers[ob_idx].1[last_idx].substartpos = Some(pos);
            self.openers[ob_idx].1[last_idx].subendpos = Some(pos + 1);
            self.destination = true;
            let sp = self.openers[ob_idx].1[last_idx].startpos + 1;
            self.clear_openers(sp, pos - 1);
            return Some(pos + 2);
        } else if pos + 1 <= endpos && cp(subject, pos + 1) == C_LEFT_BRACE {
            // bracketed span
            self.add_match_at(opener_match_index, opener_startpos, opener_endpos, "+span");
            self.add_match(pos, pos, "-span");
            self.clear_openers(opener_startpos, pos);
            return Some(pos + 1);
        }

        None
    }

    fn handle_right_paren(&mut self, pos: usize, _endpos: usize) -> Option<usize> {
        if !self.destination {
            return None;
        }

        // check for ( openers
        let parens_idx = self.openers.iter().position(|(k, _)| *k == "(");
        if let Some(pi) = parens_idx {
            if !self.openers[pi].1.is_empty() {
                self.openers[pi].1.pop();
                self.add_match(pos, pos, "str");
                return Some(pos + 1);
            }
        }

        // check for explicit link
        let ob_idx = self.openers.iter().position(|(k, _)| *k == "[");
        if let Some(oi) = ob_idx {
            if !self.openers[oi].1.is_empty() {
                let last = self.openers[oi].1.len() - 1;
                if self.openers[oi].1[last].annot.as_deref() == Some("explicit_link") {
                    let opener_startpos = self.openers[oi].1[last].startpos;
                    let opener_endpos = self.openers[oi].1[last].endpos;
                    let opener_match_index = self.openers[oi].1[last].match_index;
                    let opener_sub_match_index = self.openers[oi].1[last].sub_match_index;
                    let opener_substartpos = self.openers[oi].1[last].substartpos.unwrap_or(opener_startpos);
                    let opener_subendpos = self.openers[oi].1[last].subendpos.unwrap_or(opener_endpos);

                    // convert matches inside destination to str
                    self.str_matches(opener_subendpos + 1, pos - 1);

                    let subject = self.subject;
                    let is_image = opener_startpos > 0
                        && cp(subject, opener_startpos - 1) == C_BANG
                        && (opener_startpos < 2 || cp(subject, opener_startpos - 2) != C_BACKSLASH);

                    if is_image {
                        // Trim the preceding str match to exclude '!', then append
                        // image_marker instead of replacing it (which would lose text).
                        let prev = &mut self.matches[opener_match_index - 1];
                        if prev.annot == "str" && prev.endpos >= opener_startpos - 1 {
                            prev.endpos = opener_startpos.saturating_sub(2);
                            if prev.endpos < prev.startpos {
                                self.matches[opener_match_index - 1].annot = "__remove__".to_string();
                            }
                        }
                        self.add_match(opener_startpos - 1, opener_startpos - 1, "image_marker");
                        self.add_match_at(opener_match_index, opener_startpos, opener_endpos, "+imagetext");
                        self.add_match_at(opener_sub_match_index, opener_substartpos, opener_substartpos, "-imagetext");
                    } else {
                        self.add_match_at(opener_match_index, opener_startpos, opener_endpos, "+linktext");
                        self.add_match_at(opener_sub_match_index, opener_substartpos, opener_substartpos, "-linktext");
                    }
                    self.add_match_at(opener_sub_match_index + 1, opener_subendpos, opener_subendpos, "+destination");
                    self.add_match(pos, pos, "-destination");
                    self.destination = false;
                    self.clear_openers(opener_startpos, pos);
                    return Some(pos + 1);
                }
            }
        }
        None
    }

    fn handle_hyphen(&mut self, pos: usize, endpos: usize) -> Option<usize> {
        let subject = self.subject;
        // check for delete with braces
        // Note: JS checks codePointAt(pos+1) without endpos guard (lookahead)
        let has_open_brace = pos > 0 && cp(subject, pos - 1) == C_LEFT_BRACE;
        let has_close_brace = cp(subject, pos + 1) == C_RIGHT_BRACE;

        if has_open_brace || has_close_brace {
            let newpos = self.between_matched(
                "-", "delete", "str",
                |s: &str, p: usize| has_brace(s, p),
                pos, endpos,
            );
            return Some(newpos);
        }

        // smart hyphens
        let mut ep = pos;
        let mut hyphens = 0usize;
        while ep <= endpos && cp(subject, ep) == C_HYPHEN {
            ep += 1;
            hyphens += 1;
        }
        if cp(subject, ep) == C_RIGHT_BRACE {
            hyphens -= 1;
        }
        if hyphens == 0 {
            self.add_match(pos, pos + 1, "str");
            return Some(pos + 2);
        }

        let all_em = hyphens % 3 == 0;
        let all_en = hyphens % 2 == 0;
        let mut p = pos;
        while hyphens > 0 {
            if all_em {
                self.add_match(p, p + 2, "em_dash");
                p += 3;
                hyphens -= 3;
            } else if all_en {
                self.add_match(p, p + 1, "en_dash");
                p += 2;
                hyphens -= 2;
            } else if hyphens >= 3 && (hyphens % 2 != 0 || hyphens > 4) {
                self.add_match(p, p + 2, "em_dash");
                p += 3;
                hyphens -= 3;
            } else if hyphens >= 2 {
                self.add_match(p, p + 1, "en_dash");
                p += 2;
                hyphens -= 2;
            } else {
                self.add_match(p, p, "str");
                p += 1;
                hyphens -= 1;
            }
        }
        Some(p)
    }

    pub fn feed(&mut self, startpos: usize, endpos: usize) {
        let subject = self.subject;

        if self.firstpos == -1 || (startpos as isize) < self.firstpos {
            self.firstpos = startpos as isize;
        }
        if self.lastpos == 0 || endpos > self.lastpos {
            self.lastpos = endpos;
        }

        let mut pos = startpos;

        while pos <= endpos {
            if self.attribute_parser.is_some() {
                let sp = pos;
                let next_special = find_special(subject, pos, endpos);
                let ep2 = next_special.unwrap_or(endpos);
                let (status, ep) = self.attribute_parser.as_mut().unwrap().feed(sp, ep2);

                if status == "done" {
                    let attribute_start = self.attribute_start;
                    if let Some(as_) = attribute_start {
                        self.add_match(as_, as_, "+attributes");
                    }
                    let attr_matches = self.attribute_parser.take().unwrap().get_matches();
                    for m in attr_matches {
                        self.matches.push(m);
                    }
                    self.add_match(ep, ep, "-attributes");
                    self.attribute_parser = None;
                    self.attribute_start = None;
                    self.attribute_slices = None;
                    pos = ep + 1;
                } else if status == "fail" {
                    self.reparse_attributes();
                    pos = sp;
                } else {
                    // continue
                    if self.attribute_slices.is_none() {
                        self.attribute_slices = Some(Vec::new());
                    }
                    self.attribute_slices.as_mut().unwrap().push((sp, ep));
                    pos = ep + 1;
                }
                continue;
            }

            // find next interesting character
            let next_special = find_special(subject, pos, endpos);
            let newpos = match next_special {
                Some(ns) => ns,
                None => endpos + 1,
            };

            if newpos > pos {
                self.add_match(pos, newpos - 1, "str");
                pos = newpos;
                if pos > endpos {
                    break;
                }
            }

            let c = cp(subject, pos);

            if c == C_CR || c == C_LF {
                if c == C_CR && cp(subject, pos + 1) == C_LF && pos + 1 <= endpos {
                    self.add_match(pos, pos + 1, "soft_break");
                    pos += 2;
                } else {
                    self.add_match(pos, pos, "soft_break");
                    pos += 1;
                }
            } else if self.verbatim > 0 {
                if c == C_BACKTICK {
                    if let Some((_m_start, m_end, _caps)) =
                        find::find(subject, &PATT_BACKTICKS1, pos, Some(endpos))
                    {
                        let endchar = m_end;
                        if m_end - pos + 1 == self.verbatim {
                            // check for raw attribute
                            let has_raw = find::find(subject, &PATT_RAW_ATTRIBUTE, endchar + 1, Some(endpos));
                            if has_raw.is_some() && self.verbatim_type == "verbatim" {
                                let (m2_start, m2_end, _) = has_raw.unwrap();
                                self.add_match(pos, endchar, &format!("-{}", self.verbatim_type));
                                self.add_match(m2_start, m2_end, "raw_format");
                                pos = m2_end + 1;
                            } else {
                                self.add_match(pos, endchar, &format!("-{}", self.verbatim_type));
                                pos = endchar + 1;
                            }
                            self.verbatim = 0;
                            self.verbatim_type = "verbatim".to_string();
                        } else {
                            self.add_match(pos, endchar, "str");
                            pos = endchar + 1;
                        }
                    } else {
                        self.add_match(pos, endpos, "str");
                        pos = endpos + 1;
                    }
                } else {
                    self.add_match(pos, pos, "str");
                    pos += 1;
                }
            } else {
                // dispatch on character
                let handled: Option<usize> = match c {
                    C_BACKTICK => {
                        if let Some((_m_start, m_end)) =
                            find::find_pos(subject, &PATT_BACKTICKS0, pos, Some(endpos))
                        {
                            let endchar = m_end;
                            if pos >= 2
                                && find::find_pos(subject, &PATT_DOUBLE_DOLLARS, pos - 2, None).is_some()
                                && (pos < 3 || find::find_pos(subject, &PATT_BACKSLASH, pos - 3, None).is_none())
                            {
                                self.matches.pop(); // remove second $
                                self.matches.pop(); // remove first $
                                self.add_match(pos - 2, endchar, "+display_math");
                                self.verbatim_type = "display_math".to_string();
                            } else if pos >= 1
                                && find::find_pos(subject, &PATT_SINGLE_DOLLAR, pos - 1, None).is_some()
                            {
                                self.matches.pop(); // remove $
                                self.add_match(pos - 1, endchar, "+inline_math");
                                self.verbatim_type = "inline_math".to_string();
                            } else {
                                self.add_match(pos, endchar, "+verbatim");
                                self.verbatim_type = "verbatim".to_string();
                            }
                            self.verbatim = endchar - pos + 1;
                            Some(endchar + 1)
                        } else {
                            None
                        }
                    }
                    C_BACKSLASH => {
                        if let Some((_m_start, m_end)) =
                            find::find_pos(subject, &PATT_LINE_END, pos + 1, Some(endpos))
                        {
                            if !self.matches.is_empty() {
                                let last_idx = self.matches.len() - 1;
                                if self.matches[last_idx].annot == "str" {
                                    let sp = self.matches[last_idx].startpos;
                                    let mut ep = self.matches[last_idx].endpos;
                                    while ep >= sp && (cp(subject, ep) == C_SPACE || cp(subject, ep) == C_TAB) {
                                        ep -= 1;
                                    }
                                    if ep < sp {
                                        self.matches.pop();
                                    } else {
                                        self.matches[last_idx].endpos = ep;
                                    }
                                }
                            }
                            self.add_match(pos, pos, "escape");
                            self.add_match(pos + 1, m_end, "hard_break");
                            Some(m_end + 1)
                        } else if let Some((m_start, m_end)) =
                            find::find_pos(subject, &PATT_PUNCTUATION, pos + 1, Some(endpos))
                        {
                            self.add_match(pos, pos, "escape");
                            self.add_match(m_start, m_end, "str");
                            Some(m_end + 1)
                        } else if pos + 1 <= endpos && cp(subject, pos + 1) == C_SPACE {
                            self.add_match(pos, pos, "escape");
                            self.add_match(pos + 1, pos + 1, "non_breaking_space");
                            Some(pos + 2)
                        } else {
                            self.add_match(pos, pos, "str");
                            Some(pos + 1)
                        }
                    }
                    C_LESSTHAN => {
                        if let Some((m_start, m_end, caps)) =
                            find::find(subject, &PATT_AUTO_LINK, pos, Some(endpos))
                        {
                            let url = &caps[0];
                            // JS checks: url.match(/[^:]@/) for email
                            let is_email = {
                                let bytes = url.as_bytes();
                                let mut found = false;
                                for i in 1..bytes.len() {
                                    if bytes[i] == b'@' && bytes[i - 1] != b':' {
                                        found = true;
                                        break;
                                    }
                                }
                                found
                            };
                            // JS checks: url.match(/[a-zA-Z]:/) for url
                            let is_url = {
                                let bytes = url.as_bytes();
                                let mut found = false;
                                for i in 1..bytes.len() {
                                    if bytes[i] == b':' && bytes[i - 1].is_ascii_alphabetic() {
                                        found = true;
                                        break;
                                    }
                                }
                                found
                            };
                            if is_email {
                                self.add_match(m_start, m_start, "+email");
                                self.add_match(m_start + 1, m_end - 1, "str");
                                self.add_match(m_end, m_end, "-email");
                                Some(m_end + 1)
                            } else if is_url {
                                self.add_match(m_start, m_start, "+url");
                                self.add_match(m_start + 1, m_end - 1, "str");
                                self.add_match(m_end, m_end, "-url");
                                Some(m_end + 1)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    C_TILDE => {
                        Some(self.between_matched("~", "subscript", "str",
                            |_s, _p| true, pos, endpos))
                    }
                    C_HAT => {
                        Some(self.between_matched("^", "superscript", "str",
                            |_s, _p| true, pos, endpos))
                    }
                    C_UNDERSCORE => {
                        Some(self.between_matched("_", "emph", "str",
                            |_s, _p| true, pos, endpos))
                    }
                    C_ASTERISK => {
                        Some(self.between_matched("*", "strong", "str",
                            |_s, _p| true, pos, endpos))
                    }
                    C_PLUS => {
                        Some(self.between_matched("+", "insert", "str",
                            |s, p| has_brace(s, p), pos, endpos))
                    }
                    C_EQUALS => {
                        Some(self.between_matched("=", "mark", "str",
                            |s, p| has_brace(s, p), pos, endpos))
                    }
                    C_SINGLE_QUOTE => {
                        Some(self.between_matched("'", "single_quoted", "right_single_quote",
                            |s, p| {
                                if p == 0 { true } else {
                                    let prev = cp(s, p - 1);
                                    prev == C_SPACE || prev == C_TAB || prev == C_CR
                                        || prev == C_LF || prev == C_DOUBLE_QUOTE
                                        || prev == C_SINGLE_QUOTE || prev == C_HYPHEN
                                        || prev == C_LEFT_PAREN || prev == C_LEFT_BRACKET
                                }
                            }, pos, endpos))
                    }
                    C_DOUBLE_QUOTE => {
                        Some(self.between_matched("\"", "double_quoted", "left_double_quote",
                            |_s, _p| true, pos, endpos))
                    }
                    C_LEFT_BRACE => {
                        if find::find_pos(subject, &PATT_DELIM, pos + 1, Some(endpos)).is_some() {
                            self.add_match(pos, pos, "open_marker");
                            Some(pos + 1)
                        } else if self.allow_attributes {
                            self.attribute_parser = Some(AttributeParser::new(subject));
                            self.attribute_start = Some(pos);
                            self.attribute_slices = None;
                            Some(pos)
                        } else {
                            self.add_match(pos, pos, "str");
                            Some(pos + 1)
                        }
                    }
                    C_COLON => {
                        if let Some((m_start, m_end, _)) =
                            find::find(subject, &PATT_SYMBOL, pos, Some(endpos))
                        {
                            self.add_match(m_start, m_end, "symb");
                            Some(m_end + 1)
                        } else {
                            self.add_match(pos, pos, "str");
                            Some(pos + 1)
                        }
                    }
                    C_PERIOD => {
                        if find::find_pos(subject, &PATT_TWO_PERIODS, pos + 1, Some(endpos)).is_some() {
                            self.add_match(pos, pos + 2, "ellipses");
                            Some(pos + 3)
                        } else {
                            None
                        }
                    }
                    C_LEFT_BRACKET => {
                        if let Some((_m_start, m_end, _caps)) =
                            find::find(subject, &PATT_NOTE_REFERENCE, pos + 1, Some(endpos))
                        {
                            self.add_match(pos, m_end, "footnote_reference");
                            Some(m_end + 1)
                        } else {
                            self.add_opener("[", pos, pos, "str");
                            Some(pos + 1)
                        }
                    }
                    C_RIGHT_BRACKET => {
                        self.handle_right_bracket(pos, endpos)
                    }
                    C_LEFT_PAREN => {
                        if !self.destination {
                            None
                        } else {
                            self.add_opener("(", pos, pos, "str");
                            Some(pos + 1)
                        }
                    }
                    C_RIGHT_PAREN => {
                        self.handle_right_paren(pos, endpos)
                    }
                    C_HYPHEN => {
                        self.handle_hyphen(pos, endpos)
                    }
                    _ => None,
                };

                match handled {
                    Some(newpos) => pos = newpos,
                    None => pos = self.single_char(pos),
                }
            }
        }
    }
}
