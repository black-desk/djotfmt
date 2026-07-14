// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Formatter that consumes [`parser::Event`] and emits formatted Djot.
//!
//! This is an alternative renderer to the one in [`renderer`] — it uses the
//! new djot.js-based parser instead of `jotdown`.

use unicode_width::UnicodeWidthStr;

use crate::parser::{self, Event};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Configuration for the formatter.
pub struct FmtConfig {
    pub max_cols: usize,
}

/// Format a Djot document and return the formatted string.
pub fn format(input: &str, config: &FmtConfig) -> String {
    let events = parser::parse_events(input);
    let mut writer = FmtWriter::new(input, config);
    let mut out = String::new();
    writer.run(&events, &mut out).unwrap();
    out
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Alignment {
    Unspecified,
    Left,
    Right,
    Center,
}

#[derive(Clone, Debug, PartialEq)]
enum ListStyle {
    Dash,
    Star,
    Plus,
    Decimal,
    AlphaLower,
    AlphaUpper,
    RomanLower,
    RomanUpper,
    ParenParen,
    Task,
    Description,
}

struct TableCellData {
    content: String,
}

enum TableRow {
    Data(Vec<TableCellData>),
    Separator(Vec<Alignment>),
}

struct TableData {
    /// All rows in order (data and separator interleaved).
    rows: Vec<TableRow>,
    current_row_cells: Vec<TableCellData>,
    current_cell_content: String,
    /// True when the current row is a separator (contains only separator_* events).
    current_row_is_separator: bool,
    /// Alignments accumulated for the current separator row.
    current_row_alignments: Vec<Alignment>,
}

impl TableData {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            current_row_cells: Vec::new(),
            current_cell_content: String::new(),
            current_row_is_separator: false,
            current_row_alignments: Vec::new(),
        }
    }
}

// State for accumulating inline / block attributes.
#[derive(Clone)]
struct AttrState {
    /// What kind of attribute we're currently building.
    pending: Option<AttrKind>,
    /// Fragments accumulated so far.
    parts: Vec<(AttrKind, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum AttrKind {
    Class,
    Id,
    Key,
    Value,
    Comment,
}

impl AttrState {
    fn new() -> Self {
        Self {
            pending: None,
            parts: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.pending = None;
        self.parts.clear();
    }

    fn set_kind(&mut self, kind: AttrKind) {
        self.pending = Some(kind);
    }
}

// ---------------------------------------------------------------------------
// FmtWriter
// ---------------------------------------------------------------------------

struct FmtWriter<'a> {
    source: &'a str,
    max_cols: usize,

    // Word-buffer (same pattern as existing Writer)
    pending_line: String,
    pending_word: String,
    space_after_pending_word: bool,

    // Block state
    prefix: Vec<String>,
    need_blankline: bool,
    raw: bool,
    no_wrap: bool,
    list_item_start: bool,

    // List tracking
    list_style_stack: Vec<ListStyle>,

    // Table
    table_data: Option<TableData>,

    // Attribute accumulator
    attr: AttrState,

    // Track whether we're inside a block_attributes context
    in_block_attrs: bool,

    // Track whether we need to emit the `{` for an inline attribute
    in_inline_attrs: bool,

    // Verbatim opening backtick sequence (for matching close)
    verbatim_ticks: String,

    // For code_block: track if we need to emit language
    code_block_need_lang: bool,

    // Heading level tracking (parsed from source)
    heading_level: usize,

    /// True when content has been written to output since the last blank line.
    /// Used to decide whether a blankline event from the parser should produce
    /// output (preserving explicit blank lines in the source) or be collapsed.
    have_content: bool,

    // Whether we're inside a reference definition
    in_ref_def: bool,

    // Whether we just saw +linktext (need to close with ] on next dest/ref)
    pending_link_close: bool,

    /// True between +div and the class event — class name should go to raw output.
    div_needs_class: bool,

    /// True while inside +destination … -destination (link URL).
    in_destination: bool,

    /// Buffer for accumulating reference definition URL parts.
    ref_def_url: String,
}

impl<'a> FmtWriter<'a> {
    fn new(source: &'a str, config: &FmtConfig) -> Self {
        Self {
            source,
            max_cols: config.max_cols,
            pending_line: String::new(),
            pending_word: String::new(),
            space_after_pending_word: false,
            need_blankline: false,
            raw: false,
            no_wrap: false,
            list_item_start: false,
            prefix: Vec::new(),
            list_style_stack: Vec::new(),
            table_data: None,
            attr: AttrState::new(),
            in_block_attrs: false,
            in_inline_attrs: false,
            verbatim_ticks: String::new(),
            code_block_need_lang: false,
            heading_level: 0,
            in_ref_def: false,
            have_content: false,
            pending_link_close: false,
            div_needs_class: false,
            in_destination: false,
            ref_def_url: String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Low-level helpers (same pattern as existing Writer)
    // -----------------------------------------------------------------------

    fn push_word(&mut self, word: &str) -> std::fmt::Result {
        self.pending_word.push_str(word);
        log::trace!("Pending word: {:?}", self.pending_word);
        Ok(())
    }

    fn commit_word<W: std::fmt::Write>(
        &mut self,
        space_after: bool,
        out: &mut W,
    ) -> std::fmt::Result {
        log::trace!("Commit word: {:?}", self.pending_word);
        assert!(!self.pending_word.is_empty());

        let mut length = self.pending_line.width();
        if self.space_after_pending_word {
            length += 1;
        }
        length += self.pending_word.width();
        let length = length;

        if !self.no_wrap
            && !self.list_item_start
            && length > self.max_cols
            && !self.pending_line.is_empty()
            && self.table_data.is_none()
        {
            self.wrap(out)?;
        } else if self.space_after_pending_word {
            self.pending_line.push(' ');
            log::trace!("Pending line: {:?}", self.pending_line);
        }

        self.apply_prefix();
        self.pending_line.push_str(&self.pending_word);
        log::trace!("Pending line: {:?}", self.pending_line);
        self.pending_word.clear();
        self.space_after_pending_word = space_after;
        self.list_item_start = false;
        Ok(())
    }

    fn push_raw(&mut self, text: &str) -> std::fmt::Result {
        assert!(
            self.pending_word.is_empty(),
            "Pending word: {:?}",
            self.pending_word
        );
        self.pending_line.push_str(text);
        log::trace!("Pending line: {:?}", self.pending_line);
        Ok(())
    }

    fn wrap<W: std::fmt::Write>(&mut self, out: &mut W) -> std::fmt::Result {
        log::trace!("Wrap");
        if self.table_data.is_some() {
            return Ok(());
        }
        out.write_str(self.pending_line.trim_end())?;
        out.write_str("\n")?;
        self.pending_line.clear();
        self.space_after_pending_word = false;
        self.have_content = true;
        Ok(())
    }

    fn apply_prefix(&mut self) {
        if !self.pending_line.is_empty() {
            return;
        }
        if self.table_data.is_some() {
            return;
        }
        for prefix in &self.prefix {
            self.pending_line.push_str(prefix);
        }
    }

    fn blankline<W: std::fmt::Write>(&mut self, out: &mut W) -> std::fmt::Result {
        if !self.need_blankline {
            return Ok(());
        }
        self.apply_prefix();
        self.wrap(out)?;
        self.need_blankline = false;
        self.have_content = false;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Source extraction helper
    // -----------------------------------------------------------------------

    fn src(&self, event: &Event) -> String {
        if event.endpos < self.source.len() && event.startpos <= event.endpos {
            self.source[event.startpos..event.endpos + 1].to_string()
        } else if event.endpos == self.source.len() && event.startpos <= event.endpos {
            self.source[event.startpos..].to_string()
        } else {
            String::new()
        }
    }

    // -----------------------------------------------------------------------
    // List style parsing
    // -----------------------------------------------------------------------

    fn parse_list_style(annot: &str) -> ListStyle {
        // annot is like "+list|-" or "+list|1." or "+list|-X"
        let rest = annot.split_once('|').map(|(_, r)| r).unwrap_or("");
        if rest.starts_with("-X") || rest.starts_with("*X") || rest.starts_with("+X") {
            ListStyle::Task
        } else if rest == ":" || rest == ":|" {
            ListStyle::Description
        } else if rest == "-" || rest == "*" || rest == "+" {
            match rest {
                "-" => ListStyle::Dash,
                "*" => ListStyle::Star,
                "+" => ListStyle::Plus,
                _ => ListStyle::Dash,
            }
        } else if rest.starts_with('(') {
            ListStyle::ParenParen
        } else if let Some(ch) = rest.chars().next() {
            match ch {
                '1' => ListStyle::Decimal,
                'a' => ListStyle::AlphaLower,
                'A' => ListStyle::AlphaUpper,
                'i' => ListStyle::RomanLower,
                'I' => ListStyle::RomanUpper,
                _ => ListStyle::Dash,
            }
        } else {
            ListStyle::Dash
        }
    }

    // -----------------------------------------------------------------------
    // Attribute rendering helpers
    // -----------------------------------------------------------------------

    fn render_attr<W: std::fmt::Write>(
        &mut self,
        attr: &AttrState,
        out: &mut W,
    ) -> std::fmt::Result {
        if attr.parts.is_empty() {
            return Ok(());
        }
        let mut i = 0;
        while i < attr.parts.len() {
            let (kind, val) = &attr.parts[i];
            match kind {
                AttrKind::Class => {
                    self.push_word(&format!(".{}", val))?;
                    self.commit_word(true, out)?;
                }
                AttrKind::Id => {
                    self.push_word(&format!("#{}", val))?;
                    self.commit_word(true, out)?;
                }
                AttrKind::Key => {
                    // Combine Key with the following Value (if present)
                    // so key=value stays as one token.
                    if i + 1 < attr.parts.len() && attr.parts[i + 1].0 == AttrKind::Value {
                        let v = &attr.parts[i + 1].1;
                        // Quote the value if it contains whitespace or
                        // characters that would break bare-value parsing.
                        let needs_quotes = v.contains(|c: char| c.is_whitespace())
                            || v.contains('"')
                            || v.contains('=')
                            || v.contains('}')
                            || v.contains('{');
                        if needs_quotes {
                            self.push_word(&format!("{}=\"{}\"", val, v))?;
                        } else {
                            self.push_word(&format!("{}={}", val, v))?;
                        }
                        self.commit_word(true, out)?;
                        i += 1; // skip the Value part we just consumed
                    } else {
                        self.push_word(&format!("{}=", val))?;
                        self.commit_word(true, out)?;
                    }
                }
                AttrKind::Value => {
                    // Orphan value (no preceding Key)
                    self.push_word(val)?;
                    self.commit_word(true, out)?;
                }
                AttrKind::Comment => {
                    // Split comment content into individual words
                    // so long comments can wrap naturally.
                    let trimmed = val.trim();
                    self.push_word("%")?;
                    self.commit_word(true, out)?;
                    for word in trimmed.split_whitespace() {
                        self.push_word(word)?;
                        self.commit_word(true, out)?;
                    }
                    self.push_word("%")?;
                    self.commit_word(true, out)?;
                }
            }
            i += 1;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Table rendering
    // -----------------------------------------------------------------------

    fn pad_content(content: &str, width: usize, alignment: Alignment) -> String {
        let content_width = content.width();
        if content_width >= width {
            return content.to_string();
        }
        let padding = width - content_width;
        match alignment {
            Alignment::Unspecified | Alignment::Left => {
                format!("{}{}", content, " ".repeat(padding))
            }
            Alignment::Right => {
                format!("{}{}", " ".repeat(padding), content)
            }
            Alignment::Center => {
                let left = padding / 2;
                let right = padding - left;
                format!("{}{}{}", " ".repeat(left), content, " ".repeat(right))
            }
        }
    }

    fn format_separator_cell(width: usize, alignment: Alignment) -> String {
        let total = width + 2;
        match alignment {
            Alignment::Unspecified => "-".repeat(total),
            Alignment::Left => format!(":{}", "-".repeat(total - 1)),
            Alignment::Right => format!("{}:", "-".repeat(total - 1)),
            Alignment::Center => format!(":{}:", "-".repeat(total - 2)),
        }
    }

    fn render_table<W: std::fmt::Write>(&mut self, td: TableData, out: &mut W) -> std::fmt::Result {
        // Compute max width per column from data rows
        let mut num_cols = 0usize;
        for row in &td.rows {
            match row {
                TableRow::Data(cells) => {
                    if cells.len() > num_cols {
                        num_cols = cells.len();
                    }
                }
                TableRow::Separator(_) => {}
            }
        }
        if num_cols == 0 {
            return Ok(());
        }

        let mut col_widths = vec![0usize; num_cols];
        for row in &td.rows {
            if let TableRow::Data(cells) = row {
                for (i, cell) in cells.iter().enumerate() {
                    let w = cell.content.width();
                    if w > col_widths[i] {
                        col_widths[i] = w;
                    }
                }
            }
        }

        // First pass: determine alignment for each row.
        // Per djot spec: a separator's alignment applies to the previous row
        // (the header) and all subsequent rows until the next separator.
        let mut row_alignments: Vec<Vec<Alignment>> =
            vec![vec![Alignment::Unspecified; num_cols]; td.rows.len()];
        let mut current = vec![Alignment::Unspecified; num_cols];
        for (i, row) in td.rows.iter().enumerate() {
            if let TableRow::Separator(alignments) = row {
                current = alignments.clone();
                while current.len() < num_cols {
                    current.push(Alignment::Unspecified);
                }
                row_alignments[i] = current.clone();
                // Retroactively apply to the previous data row (header)
                if i > 0 {
                    if let TableRow::Data(_) = &td.rows[i - 1] {
                        row_alignments[i - 1] = current.clone();
                    }
                }
            } else {
                row_alignments[i] = current.clone();
            }
        }

        // Second pass: render rows with correct alignment.
        for (i, row) in td.rows.iter().enumerate() {
            let alignments = &row_alignments[i];
            match row {
                TableRow::Separator(_) => {
                    self.apply_prefix();
                    out.write_str(&self.pending_line)?;
                    self.pending_line.clear();
                    for (ci, width) in col_widths.iter().enumerate() {
                        let alignment = alignments
                            .get(ci)
                            .copied()
                            .unwrap_or(Alignment::Unspecified);
                        out.write_str("|")?;
                        out.write_str(&Self::format_separator_cell(*width, alignment))?;
                    }
                    out.write_str("|\n")?;
                }
                TableRow::Data(cells) => {
                    self.apply_prefix();
                    out.write_str(&self.pending_line)?;
                    self.pending_line.clear();
                    for (ci, width) in col_widths.iter().enumerate() {
                        let content = cells.get(ci).map(|c| c.content.as_str()).unwrap_or("");
                        let alignment = alignments
                            .get(ci)
                            .copied()
                            .unwrap_or(Alignment::Unspecified);
                        let padded = Self::pad_content(content, *width, alignment);
                        out.write_str("| ")?;
                        out.write_str(&padded)?;
                        out.write_str(" ")?;
                    }
                    out.write_str("|\n")?;
                }
            }
        }

        self.have_content = true;
        Ok(())
    }
    // -----------------------------------------------------------------------

    fn run<W: std::fmt::Write>(&mut self, events: &[Event], out: &mut W) -> std::fmt::Result {
        log::trace!("Start fmt render events");

        // List item counter per list nesting level
        let mut list_counter: Vec<u64> = Vec::new();

        let mut i = 0;
        while i < events.len() {
            let event = &events[i];
            let annot = &event.annot;
            log::debug!("Event: {} {:?}", annot, self.src(event));

            if annot.starts_with('+') || annot.starts_with('-') {
                let is_open = annot.starts_with('+');
                let tag = &annot[1..];

                // Strip the |... suffix for matching
                let base_tag = tag.split('|').next().unwrap_or(tag);

                match base_tag {
                    // ---- Block containers ----
                    "para" => {
                        if is_open {
                            self.blankline(out)?;
                        } else {
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                            }
                            self.wrap(out)?;
                            self.need_blankline = true;
                        }
                    }
                    "heading" => {
                        if is_open {
                            self.blankline(out)?;
                            self.apply_prefix();
                            // Extract heading level from source (e.g. "##" => level 2)
                            let src = self.src(event);
                            let level = src.chars().take_while(|c| *c == '#').count();
                            self.heading_level = level;
                            self.push_raw(&"#".repeat(level))?;
                            self.push_raw(" ")?;
                            self.prefix.push(" ".repeat(level + 1));
                        } else {
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                            }
                            self.wrap(out)?;
                            self.prefix.pop();
                            self.need_blankline = true;
                        }
                    }
                    "block_quote" => {
                        if is_open {
                            self.blankline(out)?;
                            self.prefix.push("> ".to_string());
                        } else {
                            self.prefix.pop();
                        }
                    }
                    "list" => {
                        if is_open {
                            self.blankline(out)?;
                            let style = Self::parse_list_style(annot);
                            self.list_style_stack.push(style);
                            list_counter.push(0);
                        } else {
                            self.list_style_stack.pop();
                            list_counter.pop();
                        }
                    }
                    "list_item" => {
                        if is_open {
                            self.blankline(out)?;
                            self.apply_prefix();
                            let style = self
                                .list_style_stack
                                .last()
                                .cloned()
                                .unwrap_or(ListStyle::Dash);
                            *list_counter.last_mut().unwrap() += 1;
                            let counter = *list_counter.last().unwrap();

                            match style {
                                ListStyle::Dash => {
                                    self.push_raw("- ")?;
                                    self.prefix.push("  ".to_string());
                                }
                                ListStyle::Star => {
                                    self.push_raw("* ")?;
                                    self.prefix.push("  ".to_string());
                                }
                                ListStyle::Plus => {
                                    self.push_raw("+ ")?;
                                    self.prefix.push("  ".to_string());
                                }
                                ListStyle::Task => {
                                    // marker and prefix emitted by checkbox_* event
                                }
                                ListStyle::Description => {
                                    self.push_raw(": ")?;
                                    self.prefix.push("  ".to_string());
                                }
                                ListStyle::Decimal => {
                                    let n = counter.to_string();
                                    self.push_raw(&n)?;
                                    self.push_raw(". ")?;
                                    let w = n.len() + 2;
                                    self.prefix.push(" ".repeat(w));
                                }
                                ListStyle::AlphaLower => {
                                    let ch = ((counter as u8 - 1) + b'a') as char;
                                    let n = ch.to_string();
                                    self.push_raw(&n)?;
                                    self.push_raw(". ")?;
                                    self.prefix.push("   ".to_string());
                                }
                                ListStyle::AlphaUpper => {
                                    let ch = ((counter as u8 - 1) + b'A') as char;
                                    let n = ch.to_string();
                                    self.push_raw(&n)?;
                                    self.push_raw(". ")?;
                                    self.prefix.push("   ".to_string());
                                }
                                ListStyle::RomanLower => {
                                    let n = roman::to(counter.try_into().unwrap_or(1))
                                        .unwrap_or_default()
                                        .to_lowercase();
                                    self.push_raw(&n)?;
                                    self.push_raw(". ")?;
                                    let w = n.len() + 2;
                                    self.prefix.push(" ".repeat(w));
                                }
                                ListStyle::RomanUpper => {
                                    let n = roman::to(counter.try_into().unwrap_or(1))
                                        .unwrap_or_default()
                                        .to_uppercase();
                                    self.push_raw(&n)?;
                                    self.push_raw(". ")?;
                                    let w = n.len() + 2;
                                    self.prefix.push(" ".repeat(w));
                                }
                                ListStyle::ParenParen => {
                                    let n = counter.to_string();
                                    self.push_raw("(")?;
                                    self.push_raw(&n)?;
                                    self.push_raw(") ")?;
                                    let w = n.len() + 3;
                                    self.prefix.push(" ".repeat(w));
                                }
                            }
                            if style != ListStyle::Task {
                                self.list_item_start = true;
                            }
                        } else {
                            if !self.pending_line.is_empty() {
                                self.wrap(out)?;
                                self.need_blankline = true;
                            }
                            self.prefix.pop();
                        }
                    }
                    "table" => {
                        if is_open {
                            self.table_data = Some(TableData::new());
                            self.no_wrap = true;
                        } else {
                            if self.table_data.is_some() {
                                let td = self.table_data.take().unwrap();
                                self.render_table(td, out)?;
                            }
                            self.no_wrap = false;
                            self.need_blankline = true;
                        }
                    }
                    "row" => {
                        if is_open {
                            if let Some(ref mut td) = self.table_data {
                                td.current_row_cells.clear();
                                td.current_row_is_separator = false;
                                td.current_row_alignments.clear();
                            }
                        } else if let Some(ref mut td) = self.table_data {
                            if td.current_row_is_separator {
                                let alignments = std::mem::take(&mut td.current_row_alignments);
                                td.rows.push(TableRow::Separator(alignments));
                            } else {
                                let cells = std::mem::take(&mut td.current_row_cells);
                                td.rows.push(TableRow::Data(cells));
                            }
                        }
                    }
                    "cell" => {
                        if is_open {
                            if let Some(ref mut td) = self.table_data {
                                td.current_cell_content.clear();
                                self.pending_line.clear();
                                self.pending_word.clear();
                                self.space_after_pending_word = false;
                            }
                        } else if self.table_data.is_some() {
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                            }
                            let content = std::mem::take(&mut self.pending_line);
                            self.table_data.as_mut().unwrap().current_row_cells.push(
                                TableCellData {
                                    content: content.trim_end().to_string(),
                                },
                            );
                            self.pending_line.clear();
                            self.space_after_pending_word = false;
                        }
                    }
                    "caption" => {
                        if is_open {
                            // Caption comes after -table. Render ^ prefix like heading.
                            self.pending_line.clear();
                            self.pending_word.clear();
                            self.space_after_pending_word = false;
                            self.apply_prefix();
                            self.push_raw("^ ")?;
                            self.prefix.push("  ".to_string());
                        } else {
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                            }
                            self.wrap(out)?;
                            self.prefix.pop();
                            self.need_blankline = true;
                        }
                    }
                    "code_block" => {
                        if is_open {
                            self.blankline(out)?;
                            self.apply_prefix();
                            self.push_raw("```")?;
                            self.raw = true;
                            self.code_block_need_lang = true;
                            // Don't wrap yet — language may follow
                        } else {
                            // Flush any pending content line first
                            if !self.pending_line.is_empty() || !self.pending_word.is_empty() {
                                if !self.pending_word.is_empty() {
                                    self.commit_word(false, out)?;
                                }
                                self.wrap(out)?;
                            }
                            self.apply_prefix();
                            self.push_raw("```")?;
                            self.wrap(out)?;
                            self.need_blankline = true;
                            self.raw = false;
                            self.code_block_need_lang = false;
                        }
                    }
                    "footnote" => {
                        if is_open {
                            // note_label event follows; we'll emit marker there
                        } else {
                            self.prefix.pop();
                        }
                    }
                    "div" => {
                        if is_open {
                            self.blankline(out)?;
                            self.apply_prefix();
                            self.push_raw("::: ")?;
                            self.div_needs_class = true;
                            // class event follows; wrap happens after class
                        } else {
                            // Write closing :::
                            self.apply_prefix();
                            self.push_raw(":::")?;
                            self.wrap(out)?;
                            // The closing ::: is a block boundary, not content.
                            // Reset have_content so that a trailing blankline
                            // event does not produce an extra blank line.
                            self.have_content = false;
                            self.need_blankline = true;
                        }
                    }
                    "block_attributes" => {
                        if is_open {
                            self.in_block_attrs = true;
                            self.attr.reset();
                        } else {
                            // Render accumulated block attributes
                            self.apply_prefix();
                            self.push_word("{")?;
                            self.commit_word(true, out)?;
                            let attr_snapshot = self.attr.clone();
                            self.render_attr(&attr_snapshot, out)?;
                            self.push_word("}")?;
                            self.commit_word(true, out)?;
                            self.wrap(out)?;
                            // Don't set need_blankline — block attributes attach
                            // to the next element without a blank line.
                            self.attr.reset();
                            self.in_block_attrs = false;
                        }
                    }
                    "reference_definition" => {
                        if is_open {
                            self.in_ref_def = true;
                        } else {
                            // Render the buffered URL
                            if !self.ref_def_url.is_empty() {
                                let key_width = self.pending_line.width();
                                let url_width = self.ref_def_url.len();
                                if key_width + 1 + url_width > self.max_cols {
                                    // URL won't fit on the same line as the key
                                    self.wrap(out)?;
                                    self.apply_prefix();
                                    self.pending_line.push_str(&self.ref_def_url);
                                } else {
                                    self.pending_line.push(' ');
                                    self.pending_line.push_str(&self.ref_def_url);
                                }
                                self.ref_def_url.clear();
                            }
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                            }
                            self.wrap(out)?;
                            self.prefix.pop();
                            self.need_blankline = true;
                            self.in_ref_def = false;
                        }
                    }

                    // ---- Inline containers ----
                    "strong" => {
                        if is_open {
                            self.push_word("{*")?;
                        } else {
                            self.push_word("*}")?;
                        }
                    }
                    "emph" => {
                        if is_open {
                            self.push_word("{_")?;
                        } else {
                            self.push_word("_}")?;
                        }
                    }
                    "subscript" => {
                        if is_open {
                            self.push_word("{~")?;
                        } else {
                            self.push_word("~}")?;
                        }
                    }
                    "superscript" => {
                        if is_open {
                            self.push_word("{^")?;
                        } else {
                            self.push_word("^}")?;
                        }
                    }
                    "insert" => {
                        if is_open {
                            self.push_word("{+")?;
                        } else {
                            self.push_word("+}")?;
                        }
                    }
                    "delete" => {
                        if is_open {
                            self.push_word("{-")?;
                        } else {
                            self.push_word("-}")?;
                        }
                    }
                    "mark" => {
                        if is_open {
                            self.push_word("{=")?;
                        } else {
                            self.push_word("=}")?;
                        }
                    }
                    "span" => {
                        if is_open {
                            self.push_word("[")?;
                        } else {
                            self.push_word("]")?;
                        }
                    }
                    "linktext" => {
                        if is_open {
                            self.push_word("[")?;
                            self.pending_link_close = true;
                        }
                        // close handled by +destination or +reference
                    }
                    "imagetext" => {
                        if is_open {
                            // image_marker event already emitted "!"
                            self.push_word("[")?;
                        }
                    }
                    "destination" => {
                        if is_open {
                            self.pending_link_close = false;
                            if self.pending_word == ")" {
                                // ) from a previous -destination (image-in-link
                                // pattern) — combine into )]( and wrap first.
                                self.pending_word.clear();
                                if !self.pending_line.is_empty() {
                                    self.wrap(out)?;
                                }
                                self.push_word(")](")?;
                            } else {
                                self.push_word("](")?;
                            }
                            self.commit_word(false, out)?;
                            self.in_destination = true;
                        } else {
                            self.in_destination = false;
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                            }
                            self.push_word(")")?;
                        }
                    }
                    "reference" => {
                        if is_open {
                            self.pending_link_close = false;
                            self.push_word("][")?;
                            self.commit_word(false, out)?;
                        } else {
                            self.push_word("]")?;
                        }
                    }
                    "verbatim" => {
                        if is_open {
                            // Extract opening backtick sequence from source
                            let src = self.src(event);
                            let ticks: String = src.chars().take_while(|c| *c == '`').collect();
                            self.verbatim_ticks = ticks.clone();
                            self.push_word(&ticks)?;
                            // Don't set raw — inline verbatim content should wrap like normal text
                        } else {
                            let ticks = self.verbatim_ticks.clone();
                            self.push_word(&ticks)?;
                        }
                    }
                    "inline_math" => {
                        if is_open {
                            self.push_word("$`")?;
                            self.raw = true;
                        } else {
                            self.raw = false;
                            self.push_word("`")?;
                        }
                    }
                    "display_math" => {
                        if is_open {
                            self.push_word("$$`")?;
                            self.raw = true;
                        } else {
                            self.raw = false;
                            self.push_word("`")?;
                        }
                    }
                    "url" | "email" => {
                        if is_open {
                            self.push_word("<")?;
                        } else {
                            self.push_word(">")?;
                        }
                    }
                    "attributes" => {
                        if is_open {
                            self.in_inline_attrs = true;
                            self.attr.reset();
                        } else {
                            // Render accumulated inline attributes.
                            // If there's a pending word, the attribute attaches to it:
                            // commit the word without trailing space, then no space
                            // before {.
                            // If pending_word is empty, the preceding text ended with
                            // a space (e.g. standalone comment): keep the space before {.
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, out)?;
                                self.space_after_pending_word = false;
                            }
                            self.push_word("{")?;
                            self.commit_word(true, out)?;
                            let attr_snapshot = self.attr.clone();
                            self.render_attr(&attr_snapshot, out)?;
                            self.push_word("}")?;
                            self.attr.reset();
                            self.in_inline_attrs = false;
                        }
                    }
                    "single_quoted" => {
                        if is_open {
                            self.push_word("{'")?;
                        } else {
                            self.push_word("'}")?;
                        }
                    }
                    "double_quoted" => {
                        if is_open {
                            self.push_word("{\"")?;
                        } else {
                            self.push_word("\"}")?;
                        }
                    }
                    _ => {
                        log::warn!("Unknown container event: {}", annot);
                    }
                }
            } else {
                // Leaf events
                match annot.as_str() {
                    "str" => {
                        let text = self.src(event);
                        if self.raw {
                            // First str in code_block without language: close the ``` line
                            if self.code_block_need_lang {
                                self.wrap(out)?;
                                self.code_block_need_lang = false;
                            }
                            for char in text.chars() {
                                if char != '\n' {
                                    self.push_word(char.to_string().as_str())?;
                                    continue;
                                }
                                if !self.pending_word.is_empty() {
                                    self.commit_word(false, out)?;
                                }
                                self.wrap(out)?;
                            }
                        } else if self.code_block_need_lang {
                            // This str after +code_block is the language
                            // Actually language comes via code_language event, not str
                            // Handle as normal str
                            self.emit_str_words(&text, out)?;
                        } else {
                            self.emit_str_words(&text, out)?;
                        }
                    }
                    "soft_break" => {
                        if !self.pending_word.is_empty() {
                            self.commit_word(true, out)?;
                        }
                        self.wrap(out)?;
                    }
                    "hard_break" => {
                        if !self.pending_word.is_empty() {
                            self.commit_word(false, out)?;
                        }
                        self.wrap(out)?;
                    }
                    "blankline" => {
                        // Source blank line: output a blank line if we've written
                        // content since the last blank line. This preserves explicit
                        // blank lines while collapsing consecutive ones.
                        if self.have_content {
                            self.need_blankline = true;
                            self.blankline(out)?;
                        }
                    }
                    "thematic_break" => {
                        self.blankline(out)?;
                        self.apply_prefix();
                        self.push_raw("* * *")?;
                        let column = self.pending_line.width_cjk();
                        if column < self.max_cols {
                            self.push_raw(" *".repeat((self.max_cols - column) / 2).as_str())?;
                        }
                        self.wrap(out)?;
                        self.need_blankline = true;
                    }
                    "escape" => {
                        self.push_word("\\")?;
                    }
                    "non_breaking_space" => {
                        self.push_word(" ")?;
                    }
                    "footnote_reference" => {
                        // src() now returns the complete [^label] text
                        let text = self.src(event);
                        self.push_word(&text)?;
                    }
                    "code_language" => {
                        let lang = self.src(event);
                        if !lang.is_empty() {
                            self.push_raw(" ")?;
                            self.push_raw(&lang)?;
                        }
                        self.code_block_need_lang = false;
                        // End the ``` language line
                        self.wrap(out)?;
                    }
                    "note_label" => {
                        // Footnote definition label
                        let label = self.src(event);
                        self.blankline(out)?;
                        self.apply_prefix();
                        self.push_raw("[^")?;
                        self.push_raw(&label)?;
                        self.push_raw("]:")?;
                        self.wrap(out)?;
                        self.prefix.push("  ".to_string());
                        self.need_blankline = false;
                    }
                    "checkbox_checked" => {
                        self.apply_prefix();
                        self.push_raw("- [x] ")?;
                        self.prefix.push("      ".to_string());
                        self.list_item_start = true;
                    }
                    "checkbox_unchecked" => {
                        self.apply_prefix();
                        self.push_raw("- [ ] ")?;
                        self.prefix.push("      ".to_string());
                        self.list_item_start = true;
                    }
                    "image_marker" => {
                        if !self.pending_word.is_empty() {
                            // Commit the preceding word. If it's a punctuation
                            // marker like "[" from linktext, don't add trailing
                            // space — "![" should follow immediately. Otherwise
                            // add space (normal word separation).
                            let space =
                                !matches!(self.pending_word.as_str(), "[" | "(" | "![" | "\"");
                            self.commit_word(space, out)?;
                        }
                        self.push_word("!")?;
                    }
                    "open_marker" => {
                        // "{" before emphasis etc — already handled by +strong etc.
                    }
                    "symb" => {
                        // src() returns ":name:" (already has both colons)
                        let text = self.src(event);
                        self.push_word(&text)?;
                    }

                    // Smart punctuation
                    "left_single_quote" => {
                        self.push_word("{'")?;
                    }
                    "right_single_quote" => {
                        self.push_word("'}")?;
                    }
                    "left_double_quote" => {
                        self.push_word("{\"")?;
                    }
                    "right_double_quote" => {
                        self.push_word("\"}")?;
                    }
                    "en_dash" => {
                        self.push_word("--")?;
                    }
                    "em_dash" => {
                        self.push_word("---")?;
                    }
                    "ellipses" => {
                        self.push_word("...")?;
                    }

                    // Table separators
                    "separator_default" | "separator_left" | "separator_right"
                    | "separator_center" => {
                        if let Some(ref mut td) = self.table_data {
                            let alignment = match annot.as_str() {
                                "separator_left" => Alignment::Left,
                                "separator_right" => Alignment::Right,
                                "separator_center" => Alignment::Center,
                                _ => Alignment::Unspecified,
                            };
                            td.current_row_is_separator = true;
                            td.current_row_alignments.push(alignment);
                        }
                    }

                    // Attribute events
                    "attr_class_marker" => {
                        self.attr.set_kind(AttrKind::Class);
                    }
                    "attr_id_marker" => {
                        self.attr.set_kind(AttrKind::Id);
                    }
                    "attr_equal_marker" => {
                        // The current pending value was actually a key
                        self.attr.set_kind(AttrKind::Key);
                    }
                    "attr_quote_marker" => {
                        // quote around value — no action needed, quoting is handled in render_attr
                    }
                    "attr_space" => {}
                    "class" => {
                        let val = self.src(event);
                        if self.div_needs_class {
                            self.push_raw(&val)?;
                            self.wrap(out)?;
                            self.need_blankline = true;
                            self.div_needs_class = false;
                        } else {
                            self.attr.pending = Some(AttrKind::Class);
                            self.attr.parts.push((AttrKind::Class, val.to_string()));
                            self.attr.pending = None;
                        }
                    }
                    "id" => {
                        let val = self.src(event);
                        self.attr.parts.push((AttrKind::Id, val.to_string()));
                    }
                    "key" => {
                        let val = self.src(event);
                        self.attr.parts.push((AttrKind::Key, val.to_string()));
                    }
                    "value" => {
                        let val = self.src(event);
                        self.attr.parts.push((AttrKind::Value, val.to_string()));
                    }
                    "comment" => {
                        let val = self.src(event);
                        // Strip leading/trailing % markers and normalize whitespace
                        let val = val
                            .trim_start_matches('%')
                            .trim_end_matches('%')
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        self.attr.parts.push((AttrKind::Comment, val));
                    }
                    "raw_format" => {
                        let val = self.src(event);
                        // Strip surrounding {=...} or =... markers
                        let format = val
                            .trim_start_matches('{')
                            .trim_start_matches('=')
                            .trim_end_matches('}');
                        if self.raw {
                            // Inside code_block, emit =format (no braces)
                            self.push_raw(" =")?;
                            self.push_raw(format)?;
                            self.wrap(out)?;
                            self.code_block_need_lang = false;
                        } else {
                            // Inline raw format after verbatim
                            self.push_word("{=")?;
                            self.push_word(format)?;
                            self.push_word("}")?;
                        }
                    }

                    "reference_key" => {
                        let src = self.src(event);
                        let key = src.trim_start_matches('[').trim_end_matches(']');
                        self.blankline(out)?;
                        self.apply_prefix();
                        self.push_raw("[")?;
                        self.push_raw(key)?;
                        self.push_raw("]:")?;
                        self.ref_def_url.clear();
                        self.prefix.push(" ".to_string());
                    }
                    "reference_value" => {
                        let val = self.src(event);
                        self.ref_def_url.push_str(&val);
                    }

                    _ => {
                        log::warn!("Unknown leaf event: {}", annot);
                    }
                }
            }

            i += 1;
        }

        // Final flush: ensure all pending content is written
        if !self.pending_word.is_empty() {
            self.commit_word(false, out)?;
        }
        if !self.pending_line.is_empty() {
            self.wrap(out)?;
        }

        log::trace!("Fmt events rendered");
        Ok(())
    }

    fn emit_str_words<W: std::fmt::Write>(&mut self, text: &str, out: &mut W) -> std::fmt::Result {
        // Inside a link destination, treat the whole content as one word
        // (multi-line URLs should not be split at whitespace).
        if self.in_destination {
            let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if !cleaned.is_empty() {
                self.push_word(&cleaned)?;
            }
            return Ok(());
        }

        let mut space = false;
        for char in text.chars() {
            if !char.is_whitespace() {
                space = false;
                self.push_word(char.to_string().as_str())?;
                continue;
            }

            if space {
                continue;
            }

            if !self.pending_word.is_empty() {
                self.commit_word(true, out)?;
            } else {
                self.space_after_pending_word = true;
            }

            space = true;
        }
        Ok(())
    }
}
