// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Port of djot.js/src/attributes.ts

use crate::parser::Event;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Scanning = 0,
    ScanningId,
    ScanningClass,
    ScanningKey,
    ScanningValue,
    ScanningBareValue,
    ScanningQuotedValue,
    ScanningQuotedValueContinuation,
    ScanningEscaped,
    ScanningEscapedInContinuation,
    ScanningComment,
    Fail,
    Done,
    Start,
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '-'
}

pub struct AttributeParser<'a> {
    subject: &'a str,
    state: State,
    begin: Option<usize>,
    lastpos: Option<usize>,
    matches: Vec<Event>,
}

impl<'a> AttributeParser<'a> {
    pub fn new(subject: &'a str) -> Self {
        AttributeParser {
            subject,
            state: State::Start,
            begin: None,
            lastpos: None,
            matches: Vec::new(),
        }
    }

    fn add_event(&mut self, startpos: usize, endpos: usize, annot: &str) {
        self.matches.push(Event {
            startpos,
            endpos,
            annot: annot.to_string(),
        });
    }

    /// Get the byte at `pos` as a char. For multi-byte UTF-8 characters this
    /// returns a wrong code point (the individual byte cast to char, not the
    /// full character). This is safe because:
    /// - All attribute syntax delimiters are ASCII (0x00-0x7F).
    /// - Bytes 0x80-0xFF never match any ASCII comparison, so multi-byte
    ///   character bytes fall through to "other character" branches.
    /// - JS does the same: pos advances by 1 (UTF-16 code unit), not by
    ///   character, so it also sees partial character units.
    fn char_at(&self, pos: usize) -> Option<char> {
        self.subject.as_bytes().get(pos).copied().map(|b| b as char)
    }

    /// Feed parser a slice of text from startpos to endpos inclusive.
    /// Returns (status, position) where status is "done", "fail", or "continue".
    pub fn feed(&mut self, startpos: usize, endpos: usize) -> (&'static str, usize) {
        let mut pos = startpos;
        while pos <= endpos {
            let c = self.char_at(pos);
            let c = match c {
                Some(c) => c,
                None => {
                    self.lastpos = Some(pos);
                    return ("continue", endpos);
                }
            };

            self.state = match self.state {
                State::Start => {
                    if c == '{' {
                        State::Scanning
                    } else {
                        State::Fail
                    }
                }
                State::Fail => State::Fail,
                State::Done => State::Done,
                State::Scanning => {
                    if c == '\n' || c == '\r' {
                        State::Scanning
                    } else if c == ' ' || c == '\t' {
                        self.add_event(pos, pos, "attr_space");
                        State::Scanning
                    } else if c == '}' {
                        State::Done
                    } else if c == '#' {
                        self.begin = Some(pos);
                        self.add_event(pos, pos, "attr_id_marker");
                        State::ScanningId
                    } else if c == '%' {
                        self.begin = Some(pos);
                        State::ScanningComment
                    } else if c == '.' {
                        self.begin = Some(pos);
                        self.add_event(pos, pos, "attr_class_marker");
                        State::ScanningClass
                    } else if is_key_char(c) {
                        self.begin = Some(pos);
                        State::ScanningKey
                    } else {
                        State::Fail
                    }
                }
                State::ScanningComment => {
                    if c == '%' {
                        if let Some(begin) = self.begin {
                            if pos > begin {
                                self.add_event(begin, pos, "comment");
                            }
                        }
                        State::Scanning
                    } else if c == '}' {
                        State::Done
                    } else {
                        State::ScanningComment
                    }
                }
                State::ScanningId => {
                    // ID chars: anything that's not a punctuation (except : _ -) or whitespace
                    let is_id_char = !c.is_whitespace()
                        && !matches!(
                            c,
                            ']'
                                | '['
                                | '~'
                                | '!'
                                | '@'
                                | '#'
                                | '%'
                                | '^'
                                | '&'
                                | '*'
                                | '('
                                | ')'
                                | '`'
                                | ','
                                | '.'
                                | '<'
                                | '>'
                                | '\\'
                                | '|'
                                | '='
                                | '+'
                                | '/'
                                | '?'
                                | '{'
                                | '}'
                        );
                    if is_id_char {
                        State::ScanningId
                    } else if c == '}' {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            if lastpos > begin {
                                self.add_event(begin + 1, lastpos, "id");
                            }
                        }
                        self.begin = None;
                        State::Done
                    } else if c.is_whitespace() {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            if lastpos > begin {
                                self.add_event(begin + 1, lastpos, "id");
                            }
                        }
                        if c != '\r' && c != '\n' {
                            self.add_event(pos, pos, "attr_space");
                        }
                        self.begin = None;
                        State::Scanning
                    } else {
                        State::Fail
                    }
                }
                State::ScanningClass => {
                    if c.is_alphanumeric() || c == '_' || c == '-' || c == ':' {
                        State::ScanningClass
                    } else if c == '}' {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            if lastpos > begin {
                                self.add_event(begin + 1, lastpos, "class");
                            }
                        }
                        self.begin = None;
                        State::Done
                    } else if c.is_whitespace() {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            if lastpos > begin {
                                self.add_event(begin + 1, lastpos, "class");
                            }
                        }
                        if c != '\r' && c != '\n' {
                            self.add_event(pos, pos, "attr_space");
                        }
                        self.begin = None;
                        State::Scanning
                    } else {
                        State::Fail
                    }
                }
                State::ScanningKey => {
                    if c == '=' {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(begin, lastpos, "key");
                            self.add_event(pos, pos, "attr_equal_marker");
                        }
                        self.begin = None;
                        State::ScanningValue
                    } else if is_key_char(c) {
                        State::ScanningKey
                    } else {
                        State::Fail
                    }
                }
                State::ScanningValue => {
                    if c == '"' {
                        self.begin = Some(pos);
                        self.add_event(pos, pos, "attr_quote_marker");
                        State::ScanningQuotedValue
                    } else if is_key_char(c) {
                        self.begin = Some(pos);
                        State::ScanningBareValue
                    } else {
                        State::Fail
                    }
                }
                State::ScanningBareValue => {
                    if is_key_char(c) {
                        State::ScanningBareValue
                    } else if c == '}' {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(begin, lastpos, "value");
                        }
                        self.begin = None;
                        State::Done
                    } else if c.is_whitespace() {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(begin, lastpos, "value");
                        }
                        if c != '\r' && c != '\n' {
                            self.add_event(pos, pos, "attr_space");
                        }
                        self.begin = None;
                        State::Scanning
                    } else {
                        State::Fail
                    }
                }
                State::ScanningEscaped => State::ScanningQuotedValue,
                State::ScanningEscapedInContinuation => State::ScanningQuotedValueContinuation,
                State::ScanningQuotedValue => {
                    if c == '"' {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(begin + 1, lastpos, "value");
                            self.add_event(pos, pos, "attr_quote_marker");
                        }
                        self.begin = None;
                        State::Scanning
                    } else if c == '\n' {
                        if let (Some(begin), Some(_lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(begin + 1, pos, "value");
                        }
                        self.begin = None;
                        State::ScanningQuotedValueContinuation
                    } else if c == '\\' {
                        State::ScanningEscaped
                    } else {
                        State::ScanningQuotedValue
                    }
                }
                State::ScanningQuotedValueContinuation => {
                    if self.begin.is_none() {
                        self.begin = Some(pos);
                    }
                    if c == '"' {
                        if let (Some(begin), Some(lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(pos, pos, "attr_quote_marker");
                            self.add_event(begin, lastpos, "value");
                        }
                        self.begin = None;
                        State::Scanning
                    } else if c == '\n' {
                        if let (Some(begin), Some(_lastpos)) = (self.begin, self.lastpos) {
                            self.add_event(begin, pos, "value");
                        }
                        self.begin = None;
                        State::ScanningQuotedValueContinuation
                    } else if c == '\\' {
                        State::ScanningEscapedInContinuation
                    } else {
                        State::ScanningQuotedValueContinuation
                    }
                }
            };

            if self.state == State::Done {
                return ("done", pos);
            } else if self.state == State::Fail {
                self.lastpos = Some(pos);
                return ("fail", pos);
            } else {
                self.lastpos = Some(pos);
                pos += 1;
            }
        }
        ("continue", endpos)
    }

    pub fn get_matches(self) -> Vec<Event> {
        self.matches
    }
}
