// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt::Write;

use unicode_width::UnicodeWidthStr;

pub struct Renderer<'a> {
    source: &'a str,
}

pub struct WriterConfig {
    pub max_cols: usize,
}

struct TableCellData {
    content: String,
    alignment: jotdown::Alignment,
}

struct TableRowData {
    cells: Vec<TableCellData>,
    is_head: bool,
}

struct TableData {
    rows: Vec<TableRowData>,
    current_row_cells: Vec<TableCellData>,
    current_cell_content: String,
    current_cell_alignment: jotdown::Alignment,
    current_row_is_head: bool,
    in_caption: bool,
    caption: Option<String>,
}

impl TableData {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            current_row_cells: Vec::new(),
            current_cell_content: String::new(),
            current_cell_alignment: jotdown::Alignment::Unspecified,
            current_row_is_head: false,
            in_caption: false,
            caption: None,
        }
    }
}

impl<'a> Renderer<'a> {
    pub fn new(s: &'a str) -> Self {
        Self { source: s }
    }

    pub fn push_offset<'s, I, W>(
        &self,
        events: I,
        mut out: W,
        config: &WriterConfig,
    ) -> std::fmt::Result
    where
        I: Iterator<Item = (jotdown::Event<'s>, std::ops::Range<usize>)>,
        W: std::fmt::Write,
    {
        let mut writer = Writer::new(self.source, config);
        writer.push(events, &mut out)?;
        Ok(())
    }
}

struct Writer<'a> {
    attrs: jotdown::Attributes<'a>,
    stack: Vec<bool>,
    list_index: Vec<u64>,
    list_kind: Vec<jotdown::ListKind>,
    need_blankline: bool,
    prefix: Vec<String>,
    raw: bool,
    pending_line: String,
    pending_word: String,
    space_after_pending_word: bool,
    source: &'a str,
    max_cols: usize,
    no_wrap: bool,
    table_data: Option<TableData>,
    /// True after writing a list item marker, suppresses wrap until the first
    /// content word is placed on the line.  Wrapping right after the marker is
    /// pointless: the continuation prefix (e.g. "  ") has the same width as
    /// the marker ("- "), so the next line offers no extra room.
    list_item_start: bool,
}

impl<'a> Writer<'a> {
    pub fn new(s: &'a str, config: &WriterConfig) -> Self {
        Self {
            attrs: jotdown::Attributes::new(),
            stack: Vec::new(),
            list_index: Vec::new(),
            list_kind: Vec::new(),
            need_blankline: false,
            prefix: Vec::new(),
            raw: false,
            source: s,
            pending_line: std::string::String::new(),
            pending_word: std::string::String::new(),
            space_after_pending_word: false,
            max_cols: config.max_cols,
            no_wrap: false,
            table_data: None,
            list_item_start: false,
        }
    }

    fn push_word(&mut self, word: impl AsRef<str>) -> std::fmt::Result {
        self.pending_word.write_str(word.as_ref())?;
        log::trace!("Pending word: {:?}", self.pending_word);
        Ok(())
    }

    fn commit_word<W>(&mut self, space_after: bool, out: W) -> std::fmt::Result
    where
        W: std::fmt::Write,
    {
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
        {
            self.wrap(out)?;
        } else if self.space_after_pending_word {
            self.pending_line.write_str(" ")?;
            log::trace!("Pending line: {:?}", self.pending_line);
        }

        self.prefix()?;
        self.pending_line.write_str(&self.pending_word)?;
        log::trace!("Pending line: {:?}", self.pending_line);
        self.pending_word.clear();
        self.space_after_pending_word = space_after;
        self.list_item_start = false;
        Ok(())
    }

    fn push_raw(&mut self, text: impl AsRef<str>) -> std::fmt::Result {
        assert!(
            self.pending_word.is_empty(),
            "Pending word: {:?}",
            self.pending_word
        );
        self.pending_line.write_str(text.as_ref())?;
        log::trace!("Pending line: {:?}", self.pending_line);
        Ok(())
    }

    fn wrap<W>(&mut self, mut out: W) -> std::fmt::Result
    where
        W: std::fmt::Write,
    {
        log::trace!("Wrap");
        if self.table_data.is_some() {
            // In table mode, don't write to out.
            // Cell content accumulates in pending_line and is
            // extracted at End(TableCell).
            return Ok(());
        }
        out.write_str(self.pending_line.trim_end())?;
        out.write_str("\n")?;
        self.pending_line.clear();
        self.space_after_pending_word = false;
        log::trace!("Pending line: {:?}", self.pending_line);
        Ok(())
    }

    fn prefix(&mut self) -> std::fmt::Result {
        if !self.pending_line.is_empty() {
            return Ok(());
        }
        if self.table_data.is_some() {
            return Ok(());
        }

        for prefix in self.prefix.iter() {
            self.pending_line.write_str(prefix)?;
        }
        Ok(())
    }

    fn blankline<W>(&mut self, mut out: W) -> std::fmt::Result
    where
        W: std::fmt::Write,
    {
        if !self.need_blankline {
            return Ok(());
        }

        self.prefix()?;
        self.wrap(out)?;

        self.need_blankline = false;

        Ok(())
    }

    fn pad_content(content: &str, width: usize, alignment: jotdown::Alignment) -> String {
        let content_width = content.width();
        if content_width >= width {
            return content.to_string();
        }
        let padding = width - content_width;
        match alignment {
            jotdown::Alignment::Unspecified | jotdown::Alignment::Left => {
                format!("{}{}", content, " ".repeat(padding))
            }
            jotdown::Alignment::Right => {
                format!("{}{}", " ".repeat(padding), content)
            }
            jotdown::Alignment::Center => {
                let left = padding / 2;
                let right = padding - left;
                format!(
                    "{}{}{}",
                    " ".repeat(left),
                    content,
                    " ".repeat(right)
                )
            }
        }
    }

    fn format_separator_cell(width: usize, alignment: jotdown::Alignment) -> String {
        let total = width + 2; // +2 for the space margin on each side
        match alignment {
            jotdown::Alignment::Unspecified => "-".repeat(total),
            jotdown::Alignment::Left => format!(":{}", "-".repeat(total - 1)),
            jotdown::Alignment::Right => format!("{}:", "-".repeat(total - 1)),
            jotdown::Alignment::Center => format!(":{}:", "-".repeat(total - 2)),
        }
    }

    fn render_table<W: std::fmt::Write>(
        &mut self,
        td: TableData,
        out: &mut W,
    ) -> std::fmt::Result {
        let num_cols = td.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return Ok(());
        }

        // Compute max width per column
        let mut col_widths = vec![0usize; num_cols];
        for row in &td.rows {
            for (i, cell) in row.cells.iter().enumerate() {
                let w = cell.content.width();
                if w > col_widths[i] {
                    col_widths[i] = w;
                }
            }
        }

        // Render rows
        let mut prev_was_head = false;
        for (row_idx, row) in td.rows.iter().enumerate() {
            // Insert separator at head→body transition
            if prev_was_head && !row.is_head {
                // Find the head rows immediately before this transition
                let mut head_start = row_idx;
                while head_start > 0 && td.rows[head_start - 1].is_head {
                    head_start -= 1;
                }

                self.prefix()?;
                out.write_str(&self.pending_line)?;
                self.pending_line.clear();
                for (i, width) in col_widths.iter().enumerate() {
                    // Get first non-Unspecified alignment from these head rows
                    let alignment = td.rows[head_start..row_idx]
                        .iter()
                        .find_map(|r| {
                            r.cells.get(i).and_then(|c| {
                                if c.alignment != jotdown::Alignment::Unspecified {
                                    Some(c.alignment)
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or(jotdown::Alignment::Unspecified);
                    out.write_str("|")?;
                    out.write_str(&Self::format_separator_cell(*width, alignment))?;
                }
                out.write_str("|\n")?;
            }

            self.prefix()?;
            out.write_str(&self.pending_line)?;
            self.pending_line.clear();
            for (i, width) in col_widths.iter().enumerate() {
                let cell = row.cells.get(i);
                let content = cell.map(|c| c.content.as_str()).unwrap_or("");
                let alignment = cell.map(|c| c.alignment).unwrap_or(jotdown::Alignment::Unspecified);
                let padded = Self::pad_content(content, *width, alignment);
                out.write_str("| ")?;
                out.write_str(&padded)?;
                out.write_str(" ")?;
            }
            out.write_str("|\n")?;
            prev_was_head = row.is_head;
        }

        // Render caption after the table
        if let Some(caption) = td.caption {
            let first_prefix = "^ ";
            let cont_prefix = "  ";
            let prefix_width = 2;
            let mut line_len = prefix_width;
            self.prefix()?;
            out.write_str(&self.pending_line)?;
            self.pending_line.clear();
            out.write_str(first_prefix)?;
            for word in caption.split_whitespace() {
                let word_width = word.width();
                if line_len + 1 + word_width > self.max_cols && line_len > prefix_width {
                    out.write_str("\n")?;
                    self.prefix()?;
                    out.write_str(&self.pending_line)?;
                    self.pending_line.clear();
                    out.write_str(cont_prefix)?;
                    line_len = prefix_width;
                } else if line_len > prefix_width {
                    out.write_str(" ")?;
                    line_len += 1;
                }
                out.write_str(word)?;
                line_len += word_width;
            }
            out.write_str("\n")?;
        }

        Ok(())
    }

    fn push<'s: 'a, I, W>(&mut self, events: I, mut out: W) -> std::fmt::Result
    where
        I: Iterator<Item = (jotdown::Event<'s>, std::ops::Range<usize>)>,
        W: std::fmt::Write,
    {
        log::trace!("Start render events");

        for e in events {
            let (e, range) = e;
            log::debug!("Event: {:?}", e);
            log::debug!("Source: {:?}", &self.source[range.clone()]);

            match e {
                jotdown::Event::Start(container, attributes) => {
                    self.attrs = attributes;
                    log::debug!("Attributes: {:?}", self.attrs);
                    match container {
                        jotdown::Container::Paragraph => {
                            self.stack.push(true);
                        }
                        _ => {}
                    }
                    log::debug!("stack: {:?}", self.stack);
                    if !self.attrs.is_empty() && container.is_block() {
                        self.push_word("{")?;
                        self.commit_word(true, &mut out)?;
                        for (k, v) in self.attrs.clone().iter() {
                            match k {
                                jotdown::AttributeKind::Class => {
                                    self.push_word(".")?;
                                }
                                jotdown::AttributeKind::Id => {
                                    self.push_word("#")?;
                                }
                                jotdown::AttributeKind::Pair { key } => {
                                    self.push_word(key.as_ref())?;
                                    self.push_word("=")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    self.push_word("%")?;
                                }
                            }
                            log::trace!("v: {:?}", v);
                            for part in v.parts() {
                                log::trace!("parts: {:?}", part);
                                match k {
                                    jotdown::AttributeKind::Class => (),
                                    jotdown::AttributeKind::Id => (),
                                    jotdown::AttributeKind::Pair { key: _ } => {
                                        self.push_word("\"")?;
                                    }
                                    jotdown::AttributeKind::Comment => {
                                        self.commit_word(true, &mut out)?;
                                    }
                                }

                                let mut space = false;
                                for char in part.chars() {
                                    if !char.is_whitespace() {
                                        space = false;
                                        self.push_word(char.to_string().as_str())?;
                                        continue;
                                    }

                                    if space {
                                        continue;
                                    }

                                    if !self.pending_word.is_empty() {
                                        self.commit_word(true, &mut out)?;
                                    }

                                    space = true;
                                }

                                match k {
                                    jotdown::AttributeKind::Class => (),
                                    jotdown::AttributeKind::Id => (),
                                    jotdown::AttributeKind::Pair { key: _ } => {
                                        self.push_word("\"")?;
                                    }
                                    jotdown::AttributeKind::Comment => (),
                                }
                            }
                            match k {
                                jotdown::AttributeKind::Class => {
                                    self.commit_word(true, &mut out)?;
                                }
                                jotdown::AttributeKind::Id => {
                                    self.commit_word(true, &mut out)?;
                                }
                                jotdown::AttributeKind::Pair { key: _ } => {
                                    self.commit_word(true, &mut out)?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    self.push_word("%")?;
                                    self.commit_word(true, &mut out)?;
                                }
                            }
                        }
                        self.push_word("}")?;
                        self.commit_word(true, &mut out)?;

                        self.attrs = jotdown::Attributes::new();
                        self.wrap(&mut out)?;
                    }

                    match container {
                        jotdown::Container::Blockquote => {
                            self.blankline(&mut out)?;
                            self.prefix.push("> ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::List { kind, tight: _ } => {
                            self.blankline(&mut out)?;
                            self.list_kind.push(kind);
                            self.list_index.push(0);
                        }
                        jotdown::Container::ListItem => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            *self.list_index.last_mut().unwrap() += 1;
                            let kind = self.list_kind.last().unwrap().clone();
                            match kind {
                                jotdown::ListKind::Unordered(list_bullet_type) => {
                                    match list_bullet_type {
                                        jotdown::ListBulletType::Dash => self.push_raw("-")?,
                                        jotdown::ListBulletType::Star => self.push_raw("*")?,
                                        jotdown::ListBulletType::Plus => self.push_raw("+")?,
                                    }
                                    self.push_raw(" ")?;
                                    self.prefix.push("  ".to_string());
                                    log::trace!("Prefix: {:?}", self.prefix);
                                }
                                jotdown::ListKind::Ordered {
                                    numbering,
                                    style,
                                    start,
                                } => {
                                    let mut width = 0;

                                    if style == jotdown::OrderedListStyle::ParenParen {
                                        self.push_raw("(")?;
                                        width += 1;
                                    }

                                    match numbering {
                                        jotdown::OrderedListNumbering::Decimal => {
                                            let n = start + self.list_index.last().unwrap() - 1;
                                            let n = n.to_string();
                                            width += n.len();
                                            self.push_raw(&n)?;
                                        }
                                        jotdown::OrderedListNumbering::AlphaLower => {
                                            let n = start + self.list_index.last().unwrap() - 1;
                                            assert!(n <= 26);
                                            let n =
                                                ((n as u8 + ('a' as u8 - 1)) as char).to_string();
                                            width += n.len();
                                            self.push_raw(&n)?
                                        }
                                        jotdown::OrderedListNumbering::AlphaUpper => {
                                            let n = start + self.list_index.last().unwrap() - 1;
                                            assert!(n <= 26);
                                            let n =
                                                ((n as u8 + ('A' as u8 - 1)) as char).to_string();
                                            let n = n.as_str();
                                            width += n.len();
                                            self.push_raw(n)?
                                        }
                                        jotdown::OrderedListNumbering::RomanLower => {
                                            let n = self.list_index.last().unwrap().clone();
                                            let n = roman::to(n.try_into().unwrap())
                                                .unwrap()
                                                .to_lowercase();
                                            width += n.len();
                                            self.push_raw(&n)?
                                        }
                                        jotdown::OrderedListNumbering::RomanUpper => {
                                            let n = self.list_index.last().unwrap().clone();
                                            let n = roman::to(n.try_into().unwrap())
                                                .unwrap()
                                                .to_uppercase();
                                            width += n.len();
                                            self.push_raw(&n)?
                                        }
                                    };

                                    match style {
                                        jotdown::OrderedListStyle::Period => self.push_raw(". ")?,
                                        _ => self.push_raw(") ")?,
                                    }

                                    width += 2;
                                    self.prefix.push(" ".repeat(width).to_string());
                                }
                                jotdown::ListKind::Task(_) => unreachable!(
                                    "task list items use TaskListItem container, not ListItem"
                                ),
                            }
                            self.list_item_start = true;
                        }
                        jotdown::Container::TaskListItem { checked } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw("- [")?;
                            if checked {
                                self.push_raw("x")?;
                            } else {
                                self.push_raw(" ")?;
                            }
                            self.push_raw("] ")?;
                            self.prefix.push("      ".to_string());
                            self.list_item_start = true;
                        }
                        jotdown::Container::DescriptionList => (),
                        jotdown::Container::DescriptionDetails => {
                            self.prefix.push("  ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::Footnote { label } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw("[^")?;
                            self.push_raw(label)?;
                            self.push_raw("]:")?;
                            self.wrap(&mut out)?;
                            self.prefix.push("  ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::Table => {
                            self.table_data = Some(TableData::new());
                        }
                        jotdown::Container::TableRow { head } => {
                            if let Some(ref mut td) = self.table_data {
                                td.current_row_is_head = head;
                                td.current_row_cells.clear();
                            }
                            self.no_wrap = true;
                        }
                        jotdown::Container::Section { id } => (),
                        jotdown::Container::Div { class } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw("::: ")?;
                            self.push_raw(class)?;
                            self.wrap(&mut out)?;
                            self.need_blankline = true;
                        }
                        jotdown::Container::Paragraph => {
                            self.blankline(&mut out)?;
                        }
                        jotdown::Container::Heading {
                            level,
                            has_section: _,
                            id: _,
                        } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw("#".repeat(level.into()).as_str())?;
                            self.push_raw(" ")?;
                            self.prefix.push(" ".repeat(level as usize + 1).to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::TableCell { alignment, head: _ } => {
                            if let Some(ref mut td) = self.table_data {
                                td.current_cell_alignment = alignment;
                                td.current_cell_content.clear();
                                self.pending_line.clear();
                                self.pending_word.clear();
                                self.space_after_pending_word = false;
                            }
                        }
                        jotdown::Container::Caption => {
                            if self.table_data.is_some() {
                                self.table_data.as_mut().unwrap().in_caption = true;
                                self.pending_line.clear();
                                self.pending_word.clear();
                                self.space_after_pending_word = false;
                            }
                        }
                        jotdown::Container::DescriptionTerm => {
                            self.blankline(&mut out)?;
                            self.prefix.push("  ".to_string());
                            self.push_raw(": ")?;
                        }
                        jotdown::Container::LinkDefinition { label } => {
                            self.push_raw("[")?;
                            self.push_raw(label)?;
                            self.push_raw("]: ")?;
                            self.prefix.push(" ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::RawBlock { format } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw("``` =")?;
                            self.push_raw(format)?;
                            self.wrap(&mut out)?;
                            self.raw = true;
                        }
                        jotdown::Container::CodeBlock { language } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw("```")?;
                            if !language.is_empty() {
                                self.push_raw(" ")?;
                                self.push_raw(language)?;
                            }
                            self.wrap(&mut out)?;
                            self.raw = true;
                        }
                        jotdown::Container::Span => self.push_word("[")?,
                        jotdown::Container::Link(cow, link_type) => match link_type {
                            jotdown::LinkType::Span(span_link_type) => match span_link_type {
                                jotdown::SpanLinkType::Inline => self.push_word("[")?,
                                jotdown::SpanLinkType::Reference => self.push_word("[")?,
                                jotdown::SpanLinkType::Unresolved => self.push_word("[")?,
                            },
                            jotdown::LinkType::AutoLink => {
                                self.push_word("<")?;
                            }
                            jotdown::LinkType::Email => {
                                self.push_word("<")?;
                            }
                        },
                        jotdown::Container::Image(cow, span_link_type) => self.push_word("![")?,
                        jotdown::Container::Verbatim => {
                            self.push_word(&self.source[range.clone()])?;
                        }
                        jotdown::Container::Math { display } => match display {
                            true => {
                                self.push_word("$$`")?;
                                self.raw = true;
                            }
                            false => {
                                self.push_word("$`")?;
                                self.raw = true;
                            }
                        },
                        jotdown::Container::RawInline { format: _ } => self.push_word("`")?,
                        jotdown::Container::Subscript => self.push_word("{~")?,
                        jotdown::Container::Superscript => self.push_word("{^")?,
                        jotdown::Container::Insert => self.push_word("{+")?,
                        jotdown::Container::Delete => self.push_word("{-")?,
                        jotdown::Container::Strong => self.push_word("{*")?,
                        jotdown::Container::Emphasis => self.push_word("{_")?,
                        jotdown::Container::Mark => self.push_word("{=")?,
                    }
                }
                jotdown::Event::End(container) => {
                    self.stack.pop();
                    log::debug!("stack: {:?}", self.stack);
                    match container {
                        jotdown::Container::Blockquote => {
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::List { kind: _, tight: _ } => {
                            self.list_index.pop();
                            self.list_kind.pop();
                        }
                        jotdown::Container::ListItem => {
                            if !self.pending_line.is_empty() {
                                self.wrap(&mut out)?;
                                self.need_blankline = true;
                            }
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::TaskListItem { checked } => {
                            self.prefix.pop();
                        }
                        jotdown::Container::DescriptionList => (),
                        jotdown::Container::DescriptionDetails => {
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::Footnote { label: _ } => {
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::Table => {
                            if self.table_data.is_some() {
                                let td = self.table_data.take().unwrap();
                                self.render_table(td, &mut out)?;
                            }
                            self.need_blankline = true;
                        }
                        jotdown::Container::TableRow { head: _ } => {
                            if self.table_data.is_some() {
                                let td = self.table_data.as_mut().unwrap();
                                let cells = std::mem::take(&mut td.current_row_cells);
                                let is_head = td.current_row_is_head;
                                td.rows.push(TableRowData { cells, is_head });
                            } else {
                                self.no_wrap = false;
                                self.wrap(&mut out)?;
                                self.prefix()?;
                            }
                        }
                        jotdown::Container::Section { id: _ } => (),
                        jotdown::Container::Div { class: _ } => {
                            self.blankline(&mut out)?;
                            self.prefix()?;
                            self.push_raw(":::")?;
                            self.wrap(&mut out)?;
                            self.need_blankline = true;
                        }
                        jotdown::Container::Paragraph => {
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, &mut out)?;
                            }
                            self.wrap(&mut out)?;
                            self.need_blankline = true;
                        }
                        jotdown::Container::Heading {
                            level: _,
                            has_section: _,
                            id: _,
                        } => {
                            self.commit_word(false, &mut out)?;
                            self.wrap(&mut out)?;
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                            self.need_blankline = true;
                        }
                        jotdown::Container::TableCell { alignment: _, head: _ } => {
                            if self.table_data.is_some() {
                                if !self.pending_word.is_empty() {
                                    self.commit_word(false, &mut out)?;
                                }
                                let content = std::mem::take(&mut self.pending_line);
                                let alignment = self
                                    .table_data
                                    .as_ref()
                                    .unwrap()
                                    .current_cell_alignment;
                                self.table_data
                                    .as_mut()
                                    .unwrap()
                                    .current_row_cells
                                    .push(TableCellData {
                                        content: content.trim_end().to_string(),
                                        alignment,
                                    });
                                self.pending_line.clear();
                                self.space_after_pending_word = false;
                            } else {
                                if !self.pending_word.is_empty() {
                                    self.commit_word(false, &mut out)?;
                                }
                                self.push_raw(" |")?;
                            }
                        }
                        jotdown::Container::Caption => {
                            if self.table_data.is_some() {
                                if !self.pending_word.is_empty() {
                                    self.commit_word(false, &mut out)?;
                                }
                                let content = std::mem::take(&mut self.pending_line);
                                let td = self.table_data.as_mut().unwrap();
                                td.caption = Some(content.trim_end().to_string());
                                td.in_caption = false;
                                self.space_after_pending_word = false;
                            }
                        }
                        jotdown::Container::DescriptionTerm => {
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, &mut out)?;
                            }
                            self.wrap(&mut out)?;
                            self.prefix.pop();
                            self.need_blankline = true;
                        }
                        jotdown::Container::LinkDefinition { label } => {
                            self.commit_word(false, &mut out)?;
                            self.wrap(&mut out)?;
                            self.prefix.pop();
                            self.need_blankline = true;
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::RawBlock { format } => {
                            self.commit_word(false, &mut out)?;
                            self.wrap(&mut out)?;
                            self.prefix()?;
                            self.push_raw("```")?;
                            self.wrap(&mut out)?;
                            self.raw = false;
                        }
                        jotdown::Container::CodeBlock { language: _ } => {
                            self.prefix()?;
                            self.push_raw("```")?;
                            self.wrap(&mut out)?;
                            self.need_blankline = true;
                            self.raw = false;
                        }
                        jotdown::Container::Span => {
                            self.push_word("]")?;
                        }
                        jotdown::Container::Link(cow, link_type) => match link_type {
                            jotdown::LinkType::Span(span_link_type) => {
                                self.push_word("]")?;
                                match span_link_type {
                                    jotdown::SpanLinkType::Inline => {
                                        self.push_word("(")?;
                                        self.commit_word(false, &mut out)?;
                                        self.push_word(&cow)?;
                                        self.commit_word(false, &mut out)?;
                                        self.push_word(")")?;
                                    }
                                    jotdown::SpanLinkType::Reference => {
                                        self.push_word("[")?;
                                        self.commit_word(false, &mut out)?;
                                        let src = &self.source[range.clone()];
                                        // src is like "][ref]", extract "ref"
                                        let label = &src[2..src.len() - 1];
                                        if !label.is_empty() {
                                            self.push_word(label)?;
                                            self.commit_word(false, &mut out)?;
                                        }
                                        self.push_word("]")?;
                                    }
                                    jotdown::SpanLinkType::Unresolved => {
                                        self.push_word("[")?;
                                        self.commit_word(false, &mut out)?;
                                        self.push_word(&cow)?;
                                        self.commit_word(false, &mut out)?;
                                        self.push_word("]")?;
                                    }
                                }
                            }
                            jotdown::LinkType::AutoLink => self.push_word(">")?,
                            jotdown::LinkType::Email => self.push_word(">")?,
                        },
                        jotdown::Container::Image(cow, span_link_type) => {
                            self.push_word("](")?;
                            self.commit_word(false, &mut out)?;
                            self.push_word(&cow)?;
                            self.commit_word(false, &mut out)?;
                            self.push_word(")")?;
                        }
                        jotdown::Container::Verbatim => {
                            for c in self.source[range.clone()].chars() {
                                if c != '`' {
                                    break;
                                }

                                self.push_word(c.to_string())?;
                            }
                        }
                        jotdown::Container::Math { display: _ } => {
                            self.raw = false;
                            self.push_word("`")?;
                        }
                        jotdown::Container::RawInline { format } => {
                            self.push_word("`{=")?;
                            self.push_word(format)?;
                            self.push_word("}")?;
                        }
                        jotdown::Container::Subscript => self.push_word("~}")?,
                        jotdown::Container::Superscript => self.push_word("^}")?,
                        jotdown::Container::Insert => self.push_word("+}")?,
                        jotdown::Container::Delete => self.push_word("-}")?,
                        jotdown::Container::Strong => self.push_word("*}")?,
                        jotdown::Container::Emphasis => self.push_word("_}")?,
                        jotdown::Container::Mark => self.push_word("=}")?,
                    }

                    if !self.attrs.is_empty() {
                        self.push_word("{")?;
                        self.commit_word(true, &mut out)?;
                        for (k, v) in self.attrs.clone().iter() {
                            match k {
                                jotdown::AttributeKind::Class => {
                                    self.push_word(".")?;
                                }
                                jotdown::AttributeKind::Id => {
                                    self.push_word("#")?;
                                }
                                jotdown::AttributeKind::Pair { key } => {
                                    self.push_word(key.as_ref())?;
                                    self.push_word("=")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    self.push_word("%")?;
                                }
                            }
                            log::trace!("v: {:?}", v);
                            for part in v.parts() {
                                log::trace!("parts: {:?}", part);
                                match k {
                                    jotdown::AttributeKind::Class => (),
                                    jotdown::AttributeKind::Id => (),
                                    jotdown::AttributeKind::Pair { key: _ } => {
                                        self.push_word("\"")?;
                                    }
                                    jotdown::AttributeKind::Comment => {
                                        self.commit_word(true, &mut out)?;
                                    }
                                }

                                let mut space = false;
                                for char in part.chars() {
                                    if !char.is_whitespace() {
                                        space = false;
                                        self.push_word(char.to_string().as_str())?;
                                        continue;
                                    }

                                    if space {
                                        continue;
                                    }

                                    if !self.pending_word.is_empty() {
                                        self.commit_word(true, &mut out)?;
                                    }

                                    space = true;
                                }

                                match k {
                                    jotdown::AttributeKind::Class => (),
                                    jotdown::AttributeKind::Id => (),
                                    jotdown::AttributeKind::Pair { key: _ } => {
                                        self.push_word("\"")?;
                                    }
                                    jotdown::AttributeKind::Comment => (),
                                }
                            }
                            match k {
                                jotdown::AttributeKind::Class => {
                                    self.commit_word(true, &mut out)?;
                                }
                                jotdown::AttributeKind::Id => {
                                    self.commit_word(true, &mut out)?;
                                }
                                jotdown::AttributeKind::Pair { key: _ } => {
                                    self.commit_word(true, &mut out)?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    self.push_word("%")?;
                                    self.commit_word(true, &mut out)?;
                                }
                            }
                        }
                        self.push_word("}")?;
                        self.commit_word(false, &mut out)?;
                    }
                    self.attrs = jotdown::Attributes::new();
                }
                jotdown::Event::Str(cow) => match self.raw {
                    true => {
                        for char in cow.chars() {
                            if char != '\n' {
                                self.push_word(char.to_string().as_str())?;
                                continue;
                            }
                            if !self.pending_word.is_empty() {
                                self.commit_word(false, &mut out)?;
                            }
                            self.wrap(&mut out)?;
                        }
                    }
                    false => {
                        let mut space = false;
                        for char in cow.chars() {
                            if !char.is_whitespace() {
                                space = false;
                                self.push_word(char.to_string().as_str())?;
                                continue;
                            }

                            if space {
                                continue;
                            }

                            if !self.pending_word.is_empty() {
                                self.commit_word(true, &mut out)?;
                            } else {
                                self.space_after_pending_word = true;
                            }

                            space = true;
                        }
                    }
                },
                jotdown::Event::FootnoteReference(str) => {
                    self.push_word("[^")?;
                    self.push_word(str)?;
                    self.push_word("]")?;
                }
                jotdown::Event::Symbol(cow) => {
                    self.push_word(":")?;
                    self.push_word(&cow)?;
                    self.push_word(":")?;
                }
                jotdown::Event::LeftSingleQuote => self.push_word("{\'")?,
                jotdown::Event::RightSingleQuote => self.push_word("\'}")?,
                jotdown::Event::LeftDoubleQuote => self.push_word("{\"")?,
                jotdown::Event::RightDoubleQuote => self.push_word("\"}")?,
                jotdown::Event::Ellipsis => self.push_word("...")?,
                jotdown::Event::EnDash => self.push_word("--")?,
                jotdown::Event::EmDash => self.push_word("---")?,
                jotdown::Event::NonBreakingSpace => {
                    self.push_word(" ")?;
                }
                jotdown::Event::Softbreak => {
                    self.commit_word(true, &mut out)?;
                    self.wrap(&mut out)?;
                }
                jotdown::Event::Hardbreak => {
                    self.commit_word(false, &mut out)?;
                    self.wrap(&mut out)?;
                }
                jotdown::Event::Escape => self.push_word("\\")?,
                jotdown::Event::Blankline => {
                    self.blankline(&mut out)?;
                }
                jotdown::Event::ThematicBreak(attributes) => {
                    self.blankline(&mut out)?;
                    self.prefix()?;
                    self.push_raw("* * *")?;
                    let column = self.pending_line.width_cjk();
                    if column < self.max_cols {
                        self.push_raw(" *".repeat((self.max_cols - column) / 2).as_str())?;
                    }
                    self.wrap(&mut out)?;
                    self.need_blankline = true;
                }
                jotdown::Event::Attributes(attributes) => {
                    self.blankline(&mut out)?;
                    self.prefix()?;
                    self.push_word("{")?;
                    self.commit_word(true, &mut out)?;

                    match self.stack.last() {
                        Some(is_para) => {
                            log::trace!("is_para: {:?}", is_para);
                            if *is_para {
                                self.prefix.push(" ".to_string());
                                log::trace!("Prefix: {:?}", self.prefix);
                                self.blankline(&mut out)?;
                            }
                        }
                        None => {
                            self.prefix.push(" ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                            self.blankline(&mut out)?;
                        }
                    }
                    for (k, v) in attributes.iter() {
                        match k {
                            jotdown::AttributeKind::Class => {
                                self.push_word(".")?;
                            }
                            jotdown::AttributeKind::Id => {
                                self.push_word("#")?;
                            }
                            jotdown::AttributeKind::Pair { key } => {
                                self.push_word(key.as_ref())?;
                                self.push_word("=")?;
                            }
                            jotdown::AttributeKind::Comment => {
                                self.push_word("%")?;
                            }
                        }
                        log::trace!("v: {:?}", v);
                        for part in v.parts() {
                            log::trace!("parts: {:?}", part);
                            match k {
                                jotdown::AttributeKind::Class => (),
                                jotdown::AttributeKind::Id => (),
                                jotdown::AttributeKind::Pair { key: _ } => {
                                    self.push_word("\"")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    self.commit_word(true, &mut out)?;
                                }
                            }

                            let mut space = false;
                            for char in part.chars() {
                                if !char.is_whitespace() {
                                    space = false;
                                    self.push_word(char.to_string().as_str())?;
                                    continue;
                                }

                                if space {
                                    continue;
                                }

                                if !self.pending_word.is_empty() {
                                    self.commit_word(true, &mut out)?;
                                }

                                space = true;
                            }

                            match k {
                                jotdown::AttributeKind::Class => (),
                                jotdown::AttributeKind::Id => (),
                                jotdown::AttributeKind::Pair { key: _ } => {
                                    self.push_word("\"")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    // self.commit_word(true, &mut out)?;
                                }
                            }
                        }
                        match k {
                            jotdown::AttributeKind::Class => {
                                self.commit_word(true, &mut out)?;
                            }
                            jotdown::AttributeKind::Id => {
                                self.commit_word(true, &mut out)?;
                            }
                            jotdown::AttributeKind::Pair { key: _ } => {
                                self.commit_word(true, &mut out)?;
                            }
                            jotdown::AttributeKind::Comment => {
                                self.push_word("%")?;
                                self.commit_word(true, &mut out)?;
                            }
                        }
                    }
                    match self.stack.last() {
                        Some(is_para) => {
                            log::trace!("is_para: {:?}", is_para);
                            if *is_para {
                                // Don't commit '}' yet for inline
                                // attributes in a paragraph — let the
                                // next word carry it so wrapping can
                                // keep '}' together with what follows.
                                self.push_word("}")?;
                                self.prefix.pop();
                                log::trace!("Prefix: {:?}", self.prefix);
                            } else {
                                self.push_word("}")?;
                            }
                        }
                        None => {
                            self.push_word("}")?;
                            self.commit_word(true, &mut out)?;
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                            self.need_blankline = true;
                            self.blankline(&mut out)?;
                            self.need_blankline = true;
                        }
                    }
                }
            }
        }
        log::trace!("Events rendered");
        Ok(())
    }
}
