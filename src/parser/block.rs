// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Faithful port of djot.js/src/block.ts

use crate::parser::attributes::AttributeParser;
use crate::parser::find;
use crate::parser::inline::InlineParser;
use crate::parser::Event;
use regex::bytes::Regex;

// All patterns compiled once, matching djot.js module-level constants.
// Using find::pattern() to disable Unicode mode for byte-level matching.
lazy_static::lazy_static! {
    static ref PATT_BLOCKQUOTE_PREFIX: Regex = find::pattern(r"[>][ \t\r\n]");
    static ref PATT_BANGS: Regex = find::pattern(r"#+");
    static ref PATT_WHITESPACE: Regex = find::pattern(r"[ \t\r\n]");
    static ref PATT_CAPTION_START: Regex = find::pattern(r"\^[ \t]+");
    static ref PATT_FOOTNOTE_START: Regex = find::pattern(r"\[\^([^\]]+)\]:[ \t\r\n]");
    static ref PATT_REFERENCE_DEF: Regex = find::pattern(r"\[([^\]\r\n]*)\]:([ \t]+[^ \t\r\n]*|)[\r\n]");
    static ref PATT_NON_WHITESPACE: Regex = find::pattern(r"[^ \t\r\n]+");
    static ref PATT_THEMATIC_BREAK: Regex = find::pattern(r"[-*][ \t]*[-*][ \t]*[-*][-* \t]*\r?\n");
    static ref PATT_LIST_MARKER: Regex = find::pattern(r"(:?[-*+:]|\([0-9]+\)|[0-9]+[.)]|[ivxlcdmIVXLCDM]+[.)]|\([ivxlcdmIVXLCDM]+\)|[a-zA-Z][.)]|\([a-zA-Z]\))[ \t\r\n]");
    static ref PATT_TASK_LIST_MARKER: Regex = find::pattern(r"[*+\-] \[[Xx ]\][ \t\r\n]");
    static ref PATT_TABLE_ROW: Regex = find::pattern(r"(\|[^\r\n]*\|)[ \t]*\r?\n");
    static ref PATT_ROW_SEP: Regex = find::pattern(r"(:?)--*(:?)([ \t]*\|[ \t]*)");
    static ref PATT_NEXT_BAR_OR_TICKS: Regex = find::pattern(r#"[^`|\r\n]*(?:[|]|`+)"#);
    static ref PATT_ENDLINE: Regex = find::pattern(r"[ \t]*\r?\n");
    static ref PATT_DIV_FENCE_START: Regex = find::pattern(r"(::::*)[ \t]*");
    static ref PATT_DIV_FENCE_END: Regex = find::pattern(r#"([\w_-]*)[ \t]*\r?\n"#);
    static ref PATT_DIV_FENCE: Regex = find::pattern(r"(::::*)[ \t]*\r?\n");
    static ref PATT_CODE_FENCE: Regex = find::pattern(r"(~~~~*|````*)([ \t]*)([^ \t\r\n`]*)[ \t]*\r?\n");
    static ref PATT_WORD: Regex = find::pattern(r"^\w+\s");
    // get_list_styles patterns
    static ref RE_TASK_LIST: Regex = find::pattern(r"^[+*\-] \[[Xx ]\]");
    static ref RE_DECIMAL: Regex = find::pattern(r"^[(]?[0-9]+[).]");
    static ref RE_DIGITS: Regex = find::pattern(r"[0-9]+");
    static ref RE_ROMAN_LO_SINGLE: Regex = find::pattern(r"^[(]?[ivxlcdm][).]");
    static ref RE_ROMAN_UP_SINGLE: Regex = find::pattern(r"^[(]?[IVXLCDM][).]");
    static ref RE_ROMAN_LO_MULTI: Regex = find::pattern(r"^[(]?[ivxlcdm]+[).]");
    static ref RE_ROMAN_UP_MULTI: Regex = find::pattern(r"^[(]?[IVXLCDM]+[).]");
    static ref RE_LO_LETTER: Regex = find::pattern(r"^[(]?[a-z][).]");
    static ref RE_UP_LETTER: Regex = find::pattern(r"^[(]?[A-Z][).]");
    static ref RE_LO_PLUS: Regex = find::pattern(r"[a-z]+");
    static ref RE_LO_SINGLE: Regex = find::pattern(r"[a-z]");
    static ref RE_UP_PLUS: Regex = find::pattern(r"[A-Z]+");
    static ref RE_UP_SINGLE: Regex = find::pattern(r"[A-Z]");
}

#[derive(Clone, Copy, PartialEq)]
enum ContentType {
    None,
    Inline,
    Block,
    Text,
    Cells,
    Attributes,
    ListItem,
}

struct ContainerExtra {
    level: usize,
    close_pattern: Option<Regex>,
    end_fence_startpos: usize,
    end_fence_endpos: usize,
    styles: Vec<String>,
    indent: usize,
    note_label: String,
    key: String,
    columns: usize,
    colons: usize,
    status: String,
    startpos: usize,
    slices: Vec<(usize, usize)>,
}

impl Default for ContainerExtra {
    fn default() -> Self {
        ContainerExtra {
            level: 0,
            close_pattern: None,
            end_fence_startpos: 0,
            end_fence_endpos: 0,
            styles: Vec::new(),
            indent: 0,
            note_label: String::new(),
            key: String::new(),
            columns: 0,
            colons: 0,
            status: String::new(),
            startpos: 0,
            slices: Vec::new(),
        }
    }
}

struct Container<'a> {
    name: String,
    ctype: ContentType,
    content: ContentType,
    extra: ContainerExtra,
    indent: usize,
    inline_parser: Option<InlineParser<'a>>,
    attribute_parser: Option<AttributeParser<'a>>,
}

/// Get the byte value at a byte offset. All comparisons are against ASCII
/// constants, so for multi-byte characters the continuation byte (128-191)
/// or leading byte (192-255) never matches any ASCII constant — which is
/// the correct result since multi-byte chars are never Djot syntax.
fn cp(subject: &str, pos: usize) -> u32 {
    subject.as_bytes().get(pos).copied().unwrap_or(0) as u32
}

/// Advance a byte offset past the current UTF-8 character.
fn next_char_pos(subject: &str, pos: usize) -> usize {
    let b = subject.as_bytes().get(pos).copied().unwrap_or(0);
    let width = if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    };
    pos + width
}

fn is_space_or_tab(cp: u32) -> bool {
    cp == 32 || cp == 9
}

fn is_eol_char(cp: u32) -> bool {
    cp == 10 || cp == 13
}

fn get_list_styles(marker: &str) -> Vec<String> {
    let mb = marker.as_bytes();
    if marker == "+" || marker == "-" || marker == "*" || marker == ":" {
        return vec![marker.to_string()];
    }
    if RE_TASK_LIST.captures(mb).is_some() {
        return vec![format!("{}X", mb[0] as char)];
    }
    if RE_DECIMAL.is_match(mb) {
        return vec![std::str::from_utf8(&RE_DIGITS.replace(mb, b"1"))
            .unwrap()
            .to_string()];
    }
    if RE_ROMAN_LO_SINGLE.is_match(mb) {
        return vec![
            std::str::from_utf8(&RE_LO_PLUS.replace(mb, b"i"))
                .unwrap()
                .to_string(),
            std::str::from_utf8(&RE_LO_SINGLE.replace(mb, b"a"))
                .unwrap()
                .to_string(),
        ];
    }
    if RE_ROMAN_UP_SINGLE.is_match(mb) {
        return vec![
            std::str::from_utf8(&RE_UP_PLUS.replace(mb, b"I"))
                .unwrap()
                .to_string(),
            std::str::from_utf8(&RE_UP_SINGLE.replace(mb, b"A"))
                .unwrap()
                .to_string(),
        ];
    }
    if RE_ROMAN_LO_MULTI.is_match(mb) {
        return vec![std::str::from_utf8(&RE_LO_PLUS.replace(mb, b"i"))
            .unwrap()
            .to_string()];
    }
    if RE_ROMAN_UP_MULTI.is_match(mb) {
        return vec![std::str::from_utf8(&RE_UP_PLUS.replace(mb, b"I"))
            .unwrap()
            .to_string()];
    }
    if RE_LO_LETTER.is_match(mb) {
        return vec![std::str::from_utf8(&RE_LO_SINGLE.replace(mb, b"a"))
            .unwrap()
            .to_string()];
    }
    if RE_UP_LETTER.is_match(mb) {
        return vec![std::str::from_utf8(&RE_UP_SINGLE.replace(mb, b"A"))
            .unwrap()
            .to_string()];
    }
    Vec::new()
}

struct EventParser<'a> {
    subject: &'a str,
    maxoffset: usize,
    len: usize,
    pos: usize,
    indent: usize,
    startline: usize,
    starteol: usize,
    endeol: usize,
    matches: Vec<Event>,
    containers: Vec<Container<'a>>,
    last_matched_container: isize,
    finished_line: bool,
}

impl<'a> EventParser<'a> {
    fn new(subject: &'a str) -> Self {
        let len = subject.len();
        let maxoffset = len - 1;
        EventParser {
            subject,
            maxoffset,
            len,
            pos: 0,
            indent: 0,
            startline: 0,
            starteol: 0,
            endeol: 0,
            matches: Vec::new(),
            containers: Vec::new(),
            last_matched_container: -1,
            finished_line: false,
        }
    }

    fn add_match(&mut self, startpos: usize, endpos: usize, annot: &str) {
        self.matches.push(Event {
            startpos: startpos.min(self.maxoffset),
            endpos: endpos.min(self.maxoffset),
            annot: annot.to_string(),
        });
    }

    fn tip(&self) -> Option<&Container> {
        self.containers.last()
    }

    fn close_unmatched_containers(&mut self) {
        let last_matched = self.last_matched_container;
        while self.containers.len() as isize - 1 > last_matched {
            self.close_tip();
        }
    }

    fn add_container(&mut self, container: Container<'a>) {
        self.add_container_inner(container, false);
    }

    fn add_container_skip_close_unmatched(&mut self, container: Container<'a>) {
        self.add_container_inner(container, true);
    }

    fn add_container_inner(&mut self, mut container: Container<'a>, skip_close_unmatched: bool) {
        if !skip_close_unmatched {
            self.close_unmatched_containers();
        }
        // Close containers whose content type doesn't match this container's type
        let container_type = container.ctype;
        while let Some(tip) = self.tip() {
            if tip.content == container_type {
                break;
            }
            self.close_tip();
        }
        if container.content == ContentType::Inline {
            container.inline_parser = Some(InlineParser::new(self.subject));
        }
        self.containers.push(container);
    }

    fn skip_space(&mut self) {
        let mut newpos = self.pos;
        while newpos < self.len && is_space_or_tab(cp(self.subject, newpos)) {
            newpos += 1; // space/tab are single-byte ASCII
        }
        self.indent = newpos - self.startline;
        self.pos = newpos;
    }

    fn get_eol(&mut self) {
        let mut i = self.pos;
        while i < self.len {
            let b = cp(self.subject, i);
            if is_eol_char(b) {
                break;
            }
            i = next_char_pos(self.subject, i);
        }
        self.starteol = i;
        if i + 1 < self.len && cp(self.subject, i) == 13 && cp(self.subject, i + 1) == 10 {
            self.endeol = i + 1;
        } else {
            self.endeol = i;
        }
    }

    fn find(&self, patt: &Regex) -> Option<(usize, usize, Vec<String>)> {
        find::find(self.subject, patt, self.pos, None)
    }

    // ---- Spec implementations ----

    fn try_block_quote(&mut self) -> bool {
        if let Some((sp, _ep, _)) = self.find(&PATT_BLOCKQUOTE_PREFIX) {
            self.add_container(Container {
                name: "block_quote".to_string(),
                ctype: ContentType::Block,
                content: ContentType::Block,
                extra: ContainerExtra::default(),
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            self.add_match(sp, sp, "+block_quote");
            self.pos = sp + 1;
            return true;
        }
        false
    }

    fn continue_block_quote(&mut self, _idx: usize) -> bool {
        if let Some((sp, _ep, _)) = self.find(&PATT_BLOCKQUOTE_PREFIX) {
            self.pos = sp + 1;
            return true;
        }
        false
    }

    fn try_heading(&mut self) -> bool {
        if let Some((sp, ep, _)) = self.find(&PATT_BANGS) {
            if find::find_pos(self.subject, &PATT_WHITESPACE, ep + 1, None).is_some() {
                let level = ep - sp + 1;

                self.add_container(Container {
                    name: "heading".to_string(),
                    ctype: ContentType::Block,
                    content: ContentType::Inline,
                    extra: ContainerExtra {
                        level,
                        ..Default::default()
                    },
                    indent: self.indent,
                    inline_parser: None,
                    attribute_parser: None,
                });
                self.add_match(sp, ep, "+heading");
                self.pos = ep + 1;
                return true;
            }
        }
        false
    }

    fn continue_heading(&mut self, idx: usize) -> bool {
        let level = self.containers[idx].extra.level;
        if let Some((sp, ep, _)) = self.find(&PATT_BANGS) {
            if ep - sp + 1 == level
                && find::find_pos(self.subject, &PATT_WHITESPACE, ep + 1, None).is_some()
            {
                self.pos = ep + 1;
                return true;
            }
        }
        false
    }

    fn try_caption(&mut self) -> bool {
        if let Some((_sp, ep, _)) = self.find(&PATT_CAPTION_START) {
            self.pos = ep + 1;
            self.add_container(Container {
                name: "caption".to_string(),
                ctype: ContentType::Block,
                content: ContentType::Inline,
                extra: ContainerExtra::default(),
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            self.add_match(self.pos, self.pos, "+caption");
            return true;
        }
        false
    }

    fn try_footnote(&mut self) -> bool {
        if let Some((sp, ep, caps)) = self.find(&PATT_FOOTNOTE_START) {
            let label = caps.get(0).map(|s| s.as_str()).unwrap_or("");
            self.add_container(Container {
                name: "footnote".to_string(),
                ctype: ContentType::Block,
                content: ContentType::Block,
                extra: ContainerExtra {
                    note_label: label.to_string(),
                    indent: self.indent,
                    ..Default::default()
                },
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            self.add_match(sp, sp, "+footnote");
            self.add_match(sp + 2, ep - 3, "note_label");
            self.pos = ep;
            return true;
        }
        false
    }

    fn continue_footnote(&mut self, idx: usize) -> bool {
        let tip_indent = self.containers[idx].extra.indent;
        self.indent > tip_indent || self.pos == self.starteol
    }

    fn try_reference_definition(&mut self) -> bool {
        if let Some((sp, _ep, caps)) = self.find(&PATT_REFERENCE_DEF) {
            let label = caps.get(0).map(|s| s.as_str()).unwrap_or("");
            let value = caps.get(1).map(|s| s.trim_start()).unwrap_or("");
            self.add_container(Container {
                name: "reference_definition".to_string(),
                ctype: ContentType::Block,
                content: ContentType::None,
                extra: ContainerExtra {
                    key: label.to_string(),
                    indent: self.indent,
                    ..Default::default()
                },
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            self.add_match(sp, sp, "+reference_definition");
            self.add_match(sp, sp + label.len() + 1, "reference_key");
            if !value.is_empty() {
                self.add_match(
                    self.starteol - value.len(),
                    self.starteol - 1,
                    "reference_value",
                );
            }
            self.pos = self.starteol - 1;
            return true;
        }
        false
    }

    fn continue_reference_definition(&mut self, idx: usize) -> bool {
        let tip_indent = self.containers[idx].extra.indent;
        if tip_indent >= self.indent {
            return false;
        }
        if self.pos < self.starteol {
            if let Some((_sp, ep, _)) = self.find(&PATT_NON_WHITESPACE) {
                if ep == self.starteol - 1 {
                    self.add_match(self.pos, self.starteol - 1, "reference_value");
                    self.pos = self.starteol;
                    return true;
                }
            }
        }
        false
    }

    fn try_thematic_break(&mut self) -> bool {
        if let Some((sp, ep, _)) = self.find(&PATT_THEMATIC_BREAK) {
            self.add_container(Container {
                name: "thematic_break".to_string(),
                ctype: ContentType::Block,
                content: ContentType::None,
                extra: ContainerExtra::default(),
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            self.add_match(sp, ep, "thematic_break");
            self.pos = ep;
            self.finished_line = true;
            return true;
        }
        false
    }

    fn try_list(&mut self) -> bool {
        if let Some((sp, ep, _caps)) = self.find(&PATT_LIST_MARKER) {
            let mut marker = self.subject[sp..ep].to_string();
            if let Some((tsp, _tep, _)) = self.find(&PATT_TASK_LIST_MARKER) {
                marker = self.subject[tsp..tsp + 5].to_string();
            }
            let styles = get_list_styles(&marker);
            if styles.is_empty() {
                return false;
            }
            self.add_container(Container {
                name: "list".to_string(),
                ctype: ContentType::Block,
                content: ContentType::ListItem,
                extra: ContainerExtra {
                    styles: styles.clone(),
                    indent: self.indent,
                    ..Default::default()
                },
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            let mut annot = "+list".to_string();
            for style in &styles {
                annot.push_str("|");
                annot.push_str(style);
            }
            self.add_match(sp, ep - 1, &annot);
            return true;
        }
        false
    }

    fn continue_list(&mut self, idx: usize) -> bool {
        let tip_indent = self.containers[idx].extra.indent;
        if self.indent > tip_indent || self.pos == self.starteol {
            return true;
        }
        if let Some((sp, ep, _caps)) = self.find(&PATT_LIST_MARKER) {
            let mut marker = self.subject[sp..ep].to_string();
            if let Some((tsp, _tep, _)) = self.find(&PATT_TASK_LIST_MARKER) {
                marker = self.subject[tsp..tsp + 5].to_string();
            }
            let styles = get_list_styles(&marker);
            let container_styles = &self.containers[idx].extra.styles;
            let newstyles: Vec<String> = container_styles
                .iter()
                .filter(|s| styles.contains(s))
                .cloned()
                .collect();
            if !newstyles.is_empty() {
                self.containers[idx].extra.styles = newstyles;
                return true;
            }
        }
        false
    }

    fn try_list_item(&mut self) -> bool {
        if let Some((sp, ep, _caps)) = self.find(&PATT_LIST_MARKER) {
            let mut marker = self.subject[sp..ep].to_string();
            let mut checkbox: Option<char> = None;
            if let Some((tsp, _tep, _)) = self.find(&PATT_TASK_LIST_MARKER) {
                marker = self.subject[tsp..tsp + 5].to_string();
                checkbox = Some(self.subject.as_bytes()[tsp + 3] as char);
            }
            let styles = get_list_styles(&marker);
            if styles.is_empty() {
                return false;
            }
            self.add_container(Container {
                name: "list_item".to_string(),
                ctype: ContentType::ListItem,
                content: ContentType::Block,
                extra: ContainerExtra {
                    styles: styles.clone(),
                    indent: self.indent,
                    ..Default::default()
                },
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            let mut annot = "+list_item".to_string();
            for style in &styles {
                annot.push_str("|");
                annot.push_str(style);
            }
            self.add_match(sp, ep - 1, &annot);
            self.pos = ep;
            if let Some(cb) = checkbox {
                if cb == ' ' {
                    self.add_match(sp + 2, sp + 4, "checkbox_unchecked");
                } else {
                    self.add_match(sp + 2, sp + 4, "checkbox_checked");
                }
                self.pos = sp + 5;
            }
            return true;
        }
        false
    }

    fn continue_list_item(&mut self, idx: usize) -> bool {
        let tip_indent = self.containers[idx].extra.indent;
        self.indent > tip_indent || self.pos == self.starteol
    }

    fn try_table(&mut self) -> bool {
        if let Some((sp, ep, caps)) = self.find(&PATT_TABLE_ROW) {
            let rawrow = &caps[0];
            self.add_container(Container {
                name: "table".to_string(),
                ctype: ContentType::Block,
                content: ContentType::Cells,
                extra: ContainerExtra {
                    columns: 0,
                    ..Default::default()
                },
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            });
            self.add_match(sp, sp, "+table");
            if self.parse_table_row(sp, sp + rawrow.len() - 1) {
            } else {
                self.matches.pop();
                self.containers.pop();
                return false;
            }
        }
        false
    }

    fn continue_table(&mut self, _idx: usize) -> bool {
        if let Some((sp, _ep, caps)) = self.find(&PATT_TABLE_ROW) {
            let rawrow = &caps[0];
            return self.parse_table_row(sp, sp + rawrow.len() - 1);
        }
        false
    }

    fn try_attributes(&mut self) -> bool {
        if cp(self.subject, self.pos) != 123 {
            return false;
        }
        let mut attribute_parser = AttributeParser::new(self.subject);
        let res = attribute_parser.feed(self.pos, self.starteol);
        if res.0 == "fail" {
            return false;
        }
        if res.0 == "done" && find::find_pos(self.subject, &PATT_ENDLINE, res.1 + 1, None).is_none()
        {
            return false;
        }
        let container = Container {
            name: "attributes".to_string(),
            ctype: ContentType::Block,
            content: ContentType::Attributes,
            extra: ContainerExtra {
                status: res.0.to_string(),
                indent: self.indent,
                startpos: self.pos,
                slices: vec![(self.pos, self.starteol)],
                ..Default::default()
            },
            indent: self.indent,
            inline_parser: None,
            attribute_parser: Some(attribute_parser),
        };
        self.add_container(container);
        self.pos = self.starteol;
        true
    }

    fn continue_attributes(&mut self, idx: usize) -> bool {
        let status = self.containers[idx].extra.status.clone();
        let tip_indent = self.containers[idx].extra.indent;
        if status == "done" {
            return false;
        }
        if self.indent > tip_indent {
            self.containers[idx]
                .extra
                .slices
                .push((self.pos, self.starteol));
            if let Some(ref mut ap) = self.containers[idx].attribute_parser {
                let res = ap.feed(self.pos, self.endeol);
                self.containers[idx].extra.status = res.0.to_string();
                if res.0 != "fail"
                    || find::find_pos(self.subject, &PATT_ENDLINE, res.1 + 1, None).is_none()
                {
                    self.pos = self.starteol;
                    return true;
                }
            }
        }
        let attr_start = self.containers[idx].extra.startpos;
        self.add_match(attr_start, attr_start, "+para");
        let attr_container = self.containers.pop();
        let para = Container {
            name: "para".to_string(),
            ctype: ContentType::Block,
            content: ContentType::Inline,
            extra: ContainerExtra::default(),
            indent: self.indent,
            inline_parser: None,
            attribute_parser: None,
        };
        self.add_container(para);
        if let (Some(ac), Some(ref mut ip)) = (attr_container, self.containers.last_mut()) {
            if let Some(ref mut ip) = ip.inline_parser {
                ip.attribute_slices = Some(ac.extra.slices);
                ip.reparse_attributes();
                self.pos = ip.lastpos + 1;
            }
        }
        true
    }

    fn try_fenced_div(&mut self) -> bool {
        if let Some((sp, ep, caps)) = self.find(&PATT_DIV_FENCE_START) {
            let colons = &caps[0];
            if let Some((clsp, clep, caps2)) =
                find::find(self.subject, &PATT_DIV_FENCE_END, ep + 1, None)
            {
                let lang = &caps2[0];
                self.add_container(Container {
                    name: "fenced_div".to_string(),
                    ctype: ContentType::Block,
                    content: ContentType::Block,
                    extra: ContainerExtra {
                        colons: colons.len(),
                        ..Default::default()
                    },
                    indent: self.indent,
                    inline_parser: None,
                    attribute_parser: None,
                });
                self.add_match(sp, ep, "+div");
                if !lang.is_empty() {
                    self.add_match(clsp, clsp + lang.len() - 1, "class");
                }
                self.pos = clep + 1;
                self.finished_line = true;
                return true;
            }
        }
        false
    }

    fn continue_fenced_div(&mut self, idx: usize) -> bool {
        if let Some(tip) = self.tip() {
            if tip.name == "code_block" {
                return true;
            }
        }
        let colons = self.containers[idx].extra.colons;
        if let Some((sp, ep, caps)) = self.find(&PATT_DIV_FENCE) {
            let fence_colons = &caps[0];
            if fence_colons.len() >= colons {
                self.containers[idx].extra.end_fence_startpos = sp;
                self.containers[idx].extra.end_fence_endpos = sp + fence_colons.len() - 1;
                self.pos = ep;
                return false;
            }
        }
        true
    }

    fn try_code_block(&mut self) -> bool {
        if let Some((sp, ep, caps)) = self.find(&PATT_CODE_FENCE) {
            let border = &caps[0];
            let ws = &caps[1];
            let lang = &caps[2];
            let is_raw = lang.starts_with('=');
            let first_char = border.as_bytes()[0] as char;
            let close_patt = Regex::new(&format!(
                r"({}){}{}[ \t]*[\r\n]",
                regex::escape(border),
                regex::escape(&first_char.to_string()),
                "*"
            ))
            .unwrap();
            let container = Container {
                name: "code_block".to_string(),
                ctype: ContentType::Block,
                content: ContentType::Text,
                extra: ContainerExtra {
                    close_pattern: Some(close_patt),
                    indent: self.indent,
                    ..Default::default()
                },
                indent: self.indent,
                inline_parser: None,
                attribute_parser: None,
            };
            self.add_container(container);
            self.add_match(sp, sp + border.len() - 1, "+code_block");
            if !lang.is_empty() {
                let langstart = sp + border.len() + ws.len();
                if is_raw {
                    self.add_match(langstart, langstart + lang.len() - 1, "raw_format");
                } else {
                    self.add_match(langstart, langstart + lang.len() - 1, "code_language");
                }
            }
            self.pos = ep;
            self.finished_line = true;
            return true;
        }
        false
    }

    fn continue_code_block(&mut self, idx: usize) -> bool {
        if let Some(ref close_patt) = self.containers[idx].extra.close_pattern {
            if let Some((sp, ep, caps)) = self.find(close_patt) {
                let fence_str = &caps[0];
                self.containers[idx].extra.end_fence_startpos = sp;
                self.containers[idx].extra.end_fence_endpos = sp + fence_str.len() - 1;
                self.pos = ep;
                self.finished_line = true;
                return false;
            }
        }
        true
    }

    fn open_paragraph(&mut self) {
        self.add_container(Container {
            name: "para".to_string(),
            ctype: ContentType::Block,
            content: ContentType::Inline,
            extra: ContainerExtra::default(),
            indent: self.indent,
            inline_parser: None,
            attribute_parser: None,
        });
        self.add_match(self.pos, self.pos, "+para");
        if let Some(ref mut ip) = self.containers.last_mut().unwrap().inline_parser {
            ip.feed(self.pos, self.endeol);
        }
    }

    // ---- Table parsing ----

    fn parse_table_cell(&mut self, ep: usize) -> Option<(usize, usize, Vec<Event>)> {
        let mut inline_parser = InlineParser::new(self.subject);
        let sp = self.pos - 1;
        let mut cell_ep = sp;
        self.skip_space(); // skip space after |
        let mut cell_complete = false;
        while !cell_complete {
            if let Some((_msp, mep)) =
                find::find_pos(self.subject, &PATT_NEXT_BAR_OR_TICKS, self.pos, Some(ep))
            {
                let nextbar = mep;
                if self.subject.as_bytes()[nextbar] == b'`' || inline_parser.verbatim > 0 {
                    inline_parser.feed(self.pos, nextbar);
                } else if nextbar > 0 && self.subject.as_bytes()[nextbar - 1] == b'\\' {
                    inline_parser.feed(self.pos, nextbar);
                } else {
                    inline_parser.feed(self.pos, nextbar - 1);
                    cell_ep = nextbar;
                    cell_complete = true;
                }
                self.pos = nextbar + 1;
            } else {
                break;
            }
        }
        if cell_complete {
            Some((sp, cell_ep, inline_parser.get_matches()))
        } else {
            None
        }
    }

    fn parse_table_row(&mut self, sp: usize, ep: usize) -> bool {
        let orig_matches = self.matches.len();
        let startpos = self.pos;
        self.add_match(sp, sp, "+row");
        self.pos += 1;
        let mut seps = Vec::new();
        let mut p = self.pos;
        let mut sepfound = false;
        while !sepfound {
            if let Some((ssp, sep, caps)) =
                find::find(self.subject, &PATT_ROW_SEP, p, Some(self.starteol))
            {
                let left = &caps[0];
                let right = &caps[1];
                let trailing = &caps[2];
                let st = if !left.is_empty() && !right.is_empty() {
                    "separator_center"
                } else if !right.is_empty() {
                    "separator_right"
                } else if !left.is_empty() {
                    "separator_left"
                } else {
                    "separator_default"
                };
                seps.push((ssp, sep - trailing.len(), st.to_string()));
                p = sep + 1;
                if p == self.starteol {
                    sepfound = true;
                    break;
                }
            } else {
                break;
            }
        }
        if sepfound {
            for (s, e, annot) in &seps {
                self.add_match(*s, *e, annot);
            }
            self.add_match(self.starteol - 1, self.starteol - 1, "-row");
            self.pos = self.starteol;
            self.finished_line = true;
            return true;
        }
        while self.pos <= ep {
            let cell = self.parse_table_cell(ep);
            if let Some((csp, cep, cell_matches)) = cell {
                self.add_match(csp, csp, "+cell");
                for (i, m) in cell_matches.iter().enumerate() {
                    let mut e = m.endpos;
                    if i == cell_matches.len() - 1 && m.annot == "str" {
                        while cp(self.subject, e) == C_SPACE && e >= m.startpos {
                            e -= 1;
                        }
                    }
                    self.add_match(m.startpos, e, &m.annot);
                }
                self.add_match(cep, cep, "-cell");
            } else {
                self.pos = startpos;
                while self.matches.len() > orig_matches {
                    self.matches.pop();
                }
                return false;
            }
        }
        self.add_match(self.pos, self.pos, "-row");
        self.pos = self.starteol;
        self.finished_line = true;
        true
    }

    // ---- Main loop ----

    fn run(&mut self) -> Vec<Event> {
        while self.pos < self.len {
            self.indent = 0;
            self.startline = self.pos;
            self.finished_line = false;
            self.get_eol();

            self.last_matched_container = -1;
            let mut idx = 0;
            while idx < self.containers.len() {
                self.skip_space();
                let cont = self.containers[idx].name.clone();
                let matches = match cont.as_str() {
                    "block_quote" => self.continue_block_quote(idx),
                    "heading" => self.continue_heading(idx),
                    "footnote" => self.continue_footnote(idx),
                    "reference_definition" => self.continue_reference_definition(idx),
                    "list" => self.continue_list(idx),
                    "list_item" => self.continue_list_item(idx),
                    "table" => self.continue_table(idx),
                    "attributes" => self.continue_attributes(idx),
                    "fenced_div" => self.continue_fenced_div(idx),
                    "code_block" => self.continue_code_block(idx),
                    "para" | "caption" => self.find(&PATT_WHITESPACE).is_none(),
                    _ => self.pos < self.starteol || self.starteol > self.startline,
                };
                if matches {
                    self.last_matched_container = idx as isize;
                } else {
                    break;
                }
                idx += 1;
            }

            if self.finished_line {
                while self.containers.len() as isize > self.last_matched_container + 1 {
                    self.close_tip();
                }
            }

            if !self.finished_line {
                self.skip_space();
                let is_blank = self.pos == self.starteol;
                let last_match = if self.last_matched_container >= 0 {
                    self.containers.get(self.last_matched_container as usize)
                } else {
                    None
                };
                let mut last_match_content = last_match.map(|c| c.content);
                let check_starts = !is_blank
                    && (last_match_content.is_none()
                        || last_match_content == Some(ContentType::Block)
                        || last_match_content == Some(ContentType::ListItem))
                    && self.find(&PATT_WORD).is_none();

                let mut new_starts = false;
                let mut check = check_starts;
                while check {
                    check = false;
                    let spec_type = last_match_content.unwrap_or(ContentType::Block);
                    if self.try_spec(spec_type).is_some() {
                        self.last_matched_container = self.containers.len() as isize - 1;
                        new_starts = true;
                        if !self.finished_line {
                            self.skip_space();
                            let tip = self.tip();
                            let tip_content = tip.map(|t| t.content);
                            last_match_content = tip_content;
                            check = tip_content == Some(ContentType::Block)
                                || tip_content == Some(ContentType::ListItem);
                        }
                    }
                }

                if !self.finished_line {
                    self.skip_space();
                    let is_blank = self.pos == self.starteol;
                    let tip = self.tip();
                    let tip_content = tip.map(|c| c.content);
                    let is_lazy = !is_blank
                        && !new_starts
                        && self.last_matched_container < self.containers.len() as isize - 1
                        && tip_content == Some(ContentType::Inline);

                    if !is_lazy {
                        self.close_unmatched_containers();
                    }

                    let tip = self.tip();
                    let tip_content = tip.map(|c| c.content);

                    if tip_content.is_none() || tip_content == Some(ContentType::Block) {
                        if is_blank {
                            if !new_starts {
                                self.add_match(self.pos, self.endeol, "blankline");
                            }
                        } else {
                            self.open_paragraph();
                        }
                    }
                    if tip_content == Some(ContentType::Text) {
                        let tip_indent = self.tip().map(|t| t.indent).unwrap_or(0);
                        let mut startpos = self.pos;
                        if self.indent > tip_indent {
                            startpos -= self.indent - tip_indent;
                        }
                        self.add_match(startpos, self.endeol, "str");
                    } else if tip_content == Some(ContentType::Inline) && !is_blank {
                        if let Some(ref mut ip) = self.containers.last_mut().unwrap().inline_parser
                        {
                            ip.feed(self.pos, self.endeol);
                        }
                    }
                }
            }
            self.pos = self.endeol + 1;
        }

        self.last_matched_container = -1;
        self.close_unmatched_containers();
        self.matches.clone()
    }

    fn try_spec(&mut self, spec_type: ContentType) -> Option<()> {
        // Only try specs whose type matches spec_type
        // type: Block specs
        if spec_type == ContentType::Block {
            if self.try_block_quote() {
                return Some(());
            }
            if self.try_heading() {
                return Some(());
            }
            if self.try_caption() {
                return Some(());
            }
            if self.try_footnote() {
                return Some(());
            }
            if self.try_reference_definition() {
                return Some(());
            }
            if self.try_thematic_break() {
                return Some(());
            }
            if self.try_list() {
                return Some(());
            }
            if self.try_table() {
                return Some(());
            }
            if self.try_attributes() {
                return Some(());
            }
            if self.try_fenced_div() {
                return Some(());
            }
            if self.try_code_block() {
                return Some(());
            }
        }
        // type: ListItem specs
        if spec_type == ContentType::ListItem {
            if self.try_list_item() {
                return Some(());
            }
        }
        None
    }

    fn close_tip(&mut self) {
        if let Some(container) = self.containers.pop() {
            match container.name.as_str() {
                "para" | "heading" | "caption" => {
                    if let Some(ip) = container.inline_parser {
                        let inline_matches = ip.get_matches();
                        let mut last: Option<&Event> = None;
                        for m in inline_matches {
                            if let Some(l) = last {
                                if l.annot == "str"
                                    && m.annot == "str"
                                    && m.startpos == l.endpos + 1
                                {
                                    self.matches.last_mut().unwrap().endpos = m.endpos;
                                    last = Some(&self.matches[self.matches.len() - 1]);
                                    continue;
                                }
                            }
                            self.matches.push(m);
                            last = Some(&self.matches[self.matches.len() - 1]);
                        }
                    }
                    // NOTE: djot.js hardcodes this.pos - 1 for the close event position.
                    // We derive it from the last match's endpos + 1 instead, which is
                    // equivalent because endpos is always < pos after inline parsing.
                    let last_ep = self
                        .matches
                        .last()
                        .map(|e| e.endpos + 1)
                        .unwrap_or(self.pos);
                    self.add_match(
                        last_ep.min(self.maxoffset),
                        last_ep.min(self.maxoffset),
                        &format!("-{}", container.name),
                    );
                }
                "block_quote" => {
                    self.add_match(self.pos, self.pos, "-block_quote");
                }
                "footnote" => {
                    self.add_match(self.pos, self.pos, "-footnote");
                }
                "reference_definition" => {
                    self.add_match(self.pos, self.pos, "-reference_definition");
                }
                "thematic_break" => {}
                "list" => {
                    self.add_match(self.pos, self.pos, "-list");
                }
                "list_item" => {
                    self.add_match(self.pos - 1, self.pos - 1, "-list_item");
                }
                "table" => {
                    self.add_match(self.pos, self.pos, "-table");
                }
                "attributes" => {
                    if container.extra.status == "continue" {
                        self.add_match(container.extra.startpos, container.extra.startpos, "+para");
                        let para = Container {
                            name: "para".to_string(),
                            ctype: ContentType::Block,
                            content: ContentType::Inline,
                            extra: ContainerExtra::default(),
                            indent: self.indent,
                            inline_parser: None,
                            attribute_parser: None,
                        };
                        // JS uses addContainer(..., true) here — skip closeUnmatched
                        // but still check content type compatibility.
                        self.add_container_skip_close_unmatched(para);
                        if let Some(ref mut ip) = self.containers.last_mut().unwrap().inline_parser
                        {
                            ip.attribute_slices = Some(container.extra.slices);
                            ip.reparse_attributes();
                        }
                        self.close_tip();
                    } else {
                        self.add_match(
                            container.extra.startpos,
                            container.extra.startpos,
                            "+block_attributes",
                        );
                        if let Some(ap) = container.attribute_parser {
                            for m in ap.get_matches() {
                                self.matches.push(m);
                            }
                        }
                        self.add_match(self.pos, self.pos, "-block_attributes");
                    }
                }
                "fenced_div" => {
                    let sp = if container.extra.end_fence_startpos > 0 {
                        container.extra.end_fence_startpos
                    } else {
                        self.pos
                    };
                    let ep = if container.extra.end_fence_endpos > 0 {
                        container.extra.end_fence_endpos
                    } else {
                        self.pos
                    };
                    self.add_match(sp, ep, "-div");
                }
                "code_block" => {
                    let sp = if container.extra.end_fence_startpos > 0 {
                        container.extra.end_fence_startpos
                    } else {
                        self.pos
                    };
                    let ep = if container.extra.end_fence_endpos > 0 {
                        container.extra.end_fence_endpos
                    } else {
                        self.pos
                    };
                    self.add_match(sp, ep, "-code_block");
                }
                _ => {
                    let last_ep = self
                        .matches
                        .last()
                        .map(|e| e.endpos + 1)
                        .unwrap_or(self.pos);
                    self.add_match(
                        last_ep.min(self.maxoffset),
                        last_ep.min(self.maxoffset),
                        &format!("-{}", container.name),
                    );
                }
            }
        }
    }
}

const C_SPACE: u32 = 32;

pub fn parse(input: &str) -> Vec<Event> {
    let mut parser = EventParser::new(input);
    parser.run()
}
