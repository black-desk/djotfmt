use std::fmt::Write;

use unicode_width::UnicodeWidthStr;

pub struct Renderer<'a> {
    source: &'a str,
}

impl<'a> Renderer<'a> {
    pub fn new(s: &'a str) -> Self {
        Self { source: s }
    }

    pub fn push_offset<'s, I, W>(&self, events: I, mut out: W) -> std::fmt::Result
    where
        I: Iterator<Item = (jotdown::Event<'s>, std::ops::Range<usize>)>,
        W: std::fmt::Write,
    {
        let mut writer = Writer::new(self.source);
        writer.push(events, &mut out)?;
        Ok(())
    }
}

struct Writer<'a> {
    attrs: jotdown::Attributes<'a>,
    list_index: Vec<u64>,
    list_kind: Vec<jotdown::ListKind>,
    need_blankline: bool,
    prefix: Vec<String>,
    raw: bool,
    pending_line: String,
    pending_word: String,
    space_after_pending_word: bool,
    source: &'a str,
}

impl<'a> Writer<'a> {
    pub fn new(s: &'a str) -> Self {
        Self {
            attrs: jotdown::Attributes::new(),
            list_index: Vec::new(),
            list_kind: Vec::new(),
            need_blankline: false,
            prefix: Vec::new(),
            raw: false,
            source: s,
            pending_line: std::string::String::new(),
            pending_word: std::string::String::new(),
            space_after_pending_word: false,
        }
    }

    fn push_word(&mut self, word: &str) -> std::fmt::Result {
        self.pending_word.write_str(word)?;
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

        if length > 72 && !self.pending_line.is_empty() {
            self.wrap(out)?;
        } else if self.space_after_pending_word {
            self.pending_line.write_str(" ")?;
        }

        self.prefix()?;
        self.pending_line.write_str(&self.pending_word)?;
        log::trace!("Pending line: {:?}", self.pending_line);
        self.pending_word.clear();
        self.space_after_pending_word = space_after;
        Ok(())
    }

    fn push_raw(&mut self, text: &str) -> std::fmt::Result {
        assert!(
            self.pending_word.is_empty(),
            "Pending word: {:?}",
            self.pending_word
        );
        self.pending_line.write_str(text)?;
        log::trace!("Pending line: {:?}", self.pending_line);
        Ok(())
    }

    fn wrap<W>(&mut self, mut out: W) -> std::fmt::Result
    where
        W: std::fmt::Write,
    {
        out.write_str(self.pending_line.trim_end())?;
        out.write_str("\n")?;
        self.pending_line.clear();
        Ok(())
    }

    fn prefix(&mut self) -> std::fmt::Result {
        log::trace!("Prefix: {:?}", self.prefix);
        if !self.pending_line.is_empty() {
            return Ok(());
        }

        for prefix in self.prefix.iter() {
            self.pending_line.write_str(prefix)?;
        }
        log::trace!("Pending line: {:?}", self.pending_line);
        Ok(())
    }

    fn blankline<W>(&mut self, out: W) -> std::fmt::Result
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

    fn push<'s: 'a, I, W>(&mut self, events: I, mut out: W) -> std::fmt::Result
    where
        I: Iterator<Item = (jotdown::Event<'s>, std::ops::Range<usize>)>,
        W: std::fmt::Write,
    {
        log::trace!("Start render events");

        for e in events {
            let (e, range) = e;
            log::debug!("Event: {:?}", e);
            log::debug!("Source: {:?}", &self.source[range]);

            match e {
                jotdown::Event::Start(container, attributes) => {
                    self.attrs = attributes;
                    log::debug!("Attributes: {:?}", self.attrs);
                    if container.is_block() {
                        self.blankline(&mut out)?;
                        if !self.attrs.is_empty() {
                            self.prefix()?;
                        }
                        for (k, v) in self.attrs.clone() {
                            match k {
                                jotdown::AttributeKind::Class => {
                                    self.push_word("{ .")?;
                                }
                                jotdown::AttributeKind::Id => {
                                    self.push_word("{ #")?;
                                }
                                jotdown::AttributeKind::Pair { key } => {
                                    self.push_word("{ ")?;
                                    self.push_word(key)?;
                                    self.push_word("=")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    self.push_raw("{%")?;
                                    self.wrap(&mut out)?;
                                }
                            }
                            self.prefix.push(" ".to_string());
                            for part in v.parts() {
                                let mut space = true;
                                for char in part.chars() {
                                    if !char.is_whitespace() {
                                        space = false;
                                        self.push_word(char.to_string().as_str())?;
                                        continue;
                                    }

                                    if space {
                                        continue;
                                    }

                                    self.commit_word(true, &mut out)?;

                                    space = true;
                                }
                            }
                            match k {
                                jotdown::AttributeKind::Class => {
                                    if !self.pending_word.is_empty() {
                                        self.commit_word(true, &mut out)?;
                                    }
                                    if self.pending_line.is_empty() {
                                        self.space_after_pending_word = false;
                                    }
                                    self.push_word("}")?;
                                    self.commit_word(false, &mut out)?;
                                }
                                jotdown::AttributeKind::Id => {
                                    if !self.pending_word.is_empty() {
                                        self.commit_word(true, &mut out)?;
                                    }
                                    if self.pending_line.is_empty() {
                                        self.space_after_pending_word = false;
                                    }
                                    self.push_word("}")?;
                                    self.commit_word(false, &mut out)?;
                                }
                                jotdown::AttributeKind::Pair { key: _ } => {
                                    if !self.pending_word.is_empty() {
                                        self.commit_word(true, &mut out)?;
                                    }
                                    if self.pending_line.is_empty() {
                                        self.space_after_pending_word = false;
                                    }
                                    self.push_word("}")?;
                                    self.commit_word(false, &mut out)?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    if !self.pending_word.is_empty() {
                                        self.commit_word(true, &mut out)?;
                                    }
                                    self.wrap(&mut out)?;
                                    if self.pending_line.is_empty() {
                                        self.space_after_pending_word = false;
                                    }
                                    self.prefix()?;
                                    self.push_word("%}")?;
                                    self.commit_word(false, &mut out)?;
                                }
                            }
                            self.prefix.pop();
                            self.wrap(&mut out)?;
                        }
                    }

                    match container {
                        jotdown::Container::Blockquote => {
                            self.prefix.push("> ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::List { kind, tight: _ } => {
                            self.list_kind.push(kind);
                            self.list_index.push(0);
                        }
                        jotdown::Container::ListItem => {
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
                                jotdown::ListKind::Task(list_bullet_type) => todo!(),
                            }
                        }
                        jotdown::Container::TaskListItem { checked } => todo!(),
                        jotdown::Container::DescriptionList => (),
                        jotdown::Container::DescriptionDetails => {
                            self.prefix.push("  ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::Footnote { label } => {
                            self.prefix()?;
                            self.push_raw("[^")?;
                            self.push_raw(label)?;
                            self.push_raw("]:")?;
                            self.wrap(&mut out)?;
                            self.prefix.push("  ".to_string());
                        }
                        jotdown::Container::Table => (),
                        jotdown::Container::TableRow { head } => {
                            self.prefix()?;
                            self.push_raw("|")?;
                        }
                        jotdown::Container::Section { id } => (),
                        jotdown::Container::Div { class } => {
                            self.prefix()?;
                            self.push_raw("::: ")?;
                            self.push_raw(class)?;
                            self.wrap(&mut out)?;
                            self.need_blankline = true;
                        }
                        jotdown::Container::Paragraph => {}
                        jotdown::Container::Heading {
                            level,
                            has_section: _,
                            id: _,
                        } => {
                            self.prefix()?;
                            self.push_raw("#".repeat(level.into()).as_str())?;
                            self.push_raw(" ")?;
                            self.prefix.push(" ".repeat(level as usize + 1).to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::TableCell { alignment, head } => self.push_raw(" ")?,
                        jotdown::Container::Caption => todo!(),
                        jotdown::Container::DescriptionTerm => {
                            self.push_raw(": ")?;
                        }
                        jotdown::Container::LinkDefinition { label } => {
                            self.push_raw("[")?;
                            self.push_raw(label)?;
                            self.push_raw("]: ")?;
                        }
                        jotdown::Container::RawBlock { format } => {
                            self.prefix()?;
                            self.push_raw("``` =")?;
                            self.push_raw(format)?;
                            self.wrap(&mut out)?;
                            self.raw = true;
                        }
                        jotdown::Container::CodeBlock { language } => {
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
                            self.push_word("`")?;
                            self.raw = true;
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
                            self.need_blankline = true;
                            self.prefix.pop();
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::TaskListItem { checked } => todo!(),
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
                            self.wrap(&mut out)?;
                            self.need_blankline = true;
                        }
                        jotdown::Container::TableRow { head } => {
                            out.write_str("\n")?;
                            self.wrap(&mut out)?;
                            self.prefix()?;
                            if head {
                                self.wrap(&mut out)?;
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
                        jotdown::Container::TableCell { alignment, head } => {
                            self.push_raw(" |")?;
                        }
                        jotdown::Container::Caption => todo!(),
                        jotdown::Container::DescriptionTerm => self.wrap(&mut out)?,
                        jotdown::Container::LinkDefinition { label } => {
                            self.commit_word(false, &mut out)?;
                            self.wrap(&mut out)?;
                            self.prefix.pop();
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
                                        self.push_word(&cow)?;
                                        self.commit_word(false, &mut out)?;
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
                            self.raw = false;
                            self.push_word("`")?;
                        }
                        jotdown::Container::Math { display: _ } => self.push_word("`")?,
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
                        out.write_str("{")?;
                        for (k, v) in self.attrs.iter() {
                            match k {
                                jotdown::AttributeKind::Class => {
                                    out.write_str(" .")?;
                                }
                                jotdown::AttributeKind::Id => {
                                    out.write_str(" #")?;
                                }
                                jotdown::AttributeKind::Pair { key } => {
                                    out.write_str(key)?;
                                    out.write_str(" =")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    out.write_str(" %")?;
                                }
                            }
                            for part in v.parts() {
                                out.write_str(part)?;
                            }
                            match k {
                                jotdown::AttributeKind::Class => (),
                                jotdown::AttributeKind::Id => (),
                                jotdown::AttributeKind::Pair { key: _ } => (),
                                jotdown::AttributeKind::Comment => {
                                    out.write_str("%")?;
                                }
                            }
                        }
                        out.write_str("}")?;
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

                            self.commit_word(true, &mut out)?;

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
                jotdown::Event::NonBreakingSpace => todo!(),
                jotdown::Event::Softbreak => {
                    self.commit_word(false, &mut out)?;
                    self.wrap(&mut out)?;
                }
                jotdown::Event::Hardbreak => {
                    self.commit_word(false, &mut out)?;
                    self.wrap(&mut out)?;
                }
                jotdown::Event::Escape => out.write_str("\\")?,
                jotdown::Event::Blankline => (),
                jotdown::Event::ThematicBreak(attributes) => {
                    self.blankline(&mut out)?;
                    self.prefix()?;
                    self.push_raw("* * *")?;
                    let column = self.pending_line.width_cjk();
                    if column < 72 {
                        self.push_raw(" *".repeat((72 - column) / 2).as_str())?;
                    }
                    self.wrap(&mut out)?;
                    self.need_blankline = true;
                }
                jotdown::Event::Attributes(attributes) => {
                    self.blankline(&mut out)?;
                    self.prefix()?;
                    for (k, v) in attributes {
                        match k {
                            jotdown::AttributeKind::Class => {
                                self.push_word("{")?;
                                self.commit_word(true, &mut out)?;
                                self.push_word(".")?;
                                self.commit_word(false, &mut out)?;
                            }
                            jotdown::AttributeKind::Id => {
                                self.push_word("{")?;
                                self.commit_word(true, &mut out)?;
                                self.push_word("#")?;
                                self.commit_word(false, &mut out)?;
                            }
                            jotdown::AttributeKind::Pair { key } => {
                                self.push_word("{")?;
                                self.commit_word(true, &mut out)?;
                                self.push_word(key)?;
                                self.push_word("=")?;
                            }
                            jotdown::AttributeKind::Comment => {
                                self.push_raw("{%")?;
                                self.wrap(&mut out)?;
                            }
                        }
                        self.prefix.push(" ".to_string());
                        for part in v.parts() {
                            let mut space = true;
                            for char in part.chars() {
                                if !char.is_whitespace() {
                                    space = false;
                                    self.push_word(char.to_string().as_str())?;
                                    continue;
                                }

                                if space {
                                    continue;
                                }

                                self.commit_word(true, &mut out)?;

                                space = true;
                            }
                        }
                        match k {
                            jotdown::AttributeKind::Class => {
                                self.push_word("}")?;
                                self.commit_word(false, &mut out)?;
                            }
                            jotdown::AttributeKind::Id => {
                                self.push_word("}")?;
                                self.commit_word(false, &mut out)?;
                            }
                            jotdown::AttributeKind::Pair { key: _ } => {
                                self.push_raw("}")?;
                                self.commit_word(false, &mut out)?;
                            }
                            jotdown::AttributeKind::Comment => {
                                self.wrap(&mut out)?;
                                if self.pending_line.is_empty() {
                                    self.space_after_pending_word = false;
                                }
                                self.prefix()?;
                                self.push_word("%}")?;
                                self.commit_word(false, &mut out)?;
                            }
                        }
                        self.prefix.pop();
                    }
                    self.need_blankline = true;
                }
            }
        }
        log::trace!("Events rendered");
        Ok(())
    }
}
