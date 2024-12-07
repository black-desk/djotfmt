use std::fmt::Write;

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
    source: &'a str,
    raw: bool,
    at_line_start: bool,
    prefix: Vec<String>,
    attrs: jotdown::Attributes<'a>,
    list_index: Vec<u64>,
    list_kind: Vec<(jotdown::ListKind, bool)>,
}

impl<'a> Writer<'a> {
    pub fn new(s: &'a str) -> Self {
        Self {
            at_line_start: true,
            source: s,
            raw: false,
            prefix: Vec::new(),
            attrs: jotdown::Attributes::new(),
            list_index: Vec::new(),
            list_kind: Vec::new(),
        }
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
                    match container {
                        jotdown::Container::Blockquote => {
                            self.prefix.push("> ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::List { kind, tight } => {
                            self.list_kind.push((kind, tight));
                            self.list_index.push(0);
                        }
                        jotdown::Container::ListItem => {
                            *self.list_index.last_mut().unwrap() += 1;
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                            let (kind, tight) = self.list_kind.last().unwrap();
                            match kind {
                                jotdown::ListKind::Unordered(list_bullet_type) => {
                                    match list_bullet_type {
                                        jotdown::ListBulletType::Dash => out.write_str("-")?,
                                        jotdown::ListBulletType::Star => out.write_str("*")?,
                                        jotdown::ListBulletType::Plus => out.write_str("+")?,
                                    }
                                    out.write_str(" ")?;
                                    self.prefix.push("  ".to_string());
                                    log::trace!("Prefix: {:?}", self.prefix);
                                }
                                jotdown::ListKind::Ordered {
                                    numbering,
                                    style,
                                    start,
                                } => {
                                    let mut width = 0;

                                    if *style == jotdown::OrderedListStyle::ParenParen {
                                        out.write_str("(")?;
                                        width += 1;
                                    }

                                    match numbering {
                                        jotdown::OrderedListNumbering::Decimal => {
                                            let n = start + self.list_index.last().unwrap() - 1;
                                            let n = n.to_string();
                                            width += n.len();
                                            out.write_str(&n)?;
                                        }
                                        jotdown::OrderedListNumbering::AlphaLower => {
                                            let n = start + self.list_index.last().unwrap() - 1;
                                            assert!(n <= 26);
                                            let n =
                                                ((n as u8 + ('a' as u8 - 1)) as char).to_string();
                                            width += n.len();
                                            out.write_str(&n)?
                                        }
                                        jotdown::OrderedListNumbering::AlphaUpper => {
                                            let n = start + self.list_index.last().unwrap() - 1;
                                            assert!(n <= 26);
                                            let n =
                                                ((n as u8 + ('A' as u8 - 1)) as char).to_string();
                                            let n = n.as_str();
                                            width += n.len();
                                            out.write_str(n)?
                                        }
                                        jotdown::OrderedListNumbering::RomanLower => {
                                            let n = self.list_index.last().unwrap().clone();
                                            let n = roman::to(n.try_into().unwrap())
                                                .unwrap()
                                                .to_lowercase();
                                            width += n.len();
                                            out.write_str(&n)?
                                        }
                                        jotdown::OrderedListNumbering::RomanUpper => {
                                            let n = self.list_index.last().unwrap().clone();
                                            let n = roman::to(n.try_into().unwrap())
                                                .unwrap()
                                                .to_uppercase();
                                            width += n.len();
                                            out.write_str(&n)?
                                        }
                                    };

                                    match style {
                                        jotdown::OrderedListStyle::Period => out.write_str(". ")?,
                                        _ => out.write_str(") ")?,
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
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }

                            out.write_str("[^")?;
                            out.write_str(label)?;
                            out.write_str("]:\n")?;

                            self.at_line_start = true;

                            self.prefix.push("  ".to_string());
                        }
                        jotdown::Container::Table => (),
                        jotdown::Container::TableRow { head } => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                            out.write_str("|")?;
                        }
                        jotdown::Container::Section { id } => (),
                        jotdown::Container::Div { class } => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                            out.write_str("::: ")?;
                            out.write_str(class)?;
                            out.write_str("\n")?;
                            self.at_line_start = true;
                        }
                        jotdown::Container::Paragraph => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                        }
                        jotdown::Container::Heading {
                            level,
                            has_section: _,
                            id: _,
                        } => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                            out.write_str("#".repeat(level.into()).as_str())?;
                            out.write_str(" ")?;
                            self.prefix.push("  ".to_string());
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::TableCell { alignment, head } => out.write_str(" ")?,
                        jotdown::Container::Caption => todo!(),
                        jotdown::Container::DescriptionTerm => out.write_str(": ")?,
                        jotdown::Container::LinkDefinition { label } => {
                            out.write_str("[")?;
                            out.write_str(label)?;
                            out.write_str("]: ")?;
                        }
                        jotdown::Container::RawBlock { format } => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                            }
                            out.write_str("``` =")?;
                            out.write_str(format)?;
                            out.write_str("\n")?;
                            self.at_line_start = true;
                            self.raw = true;
                        }
                        jotdown::Container::CodeBlock { language } => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                            out.write_str("``` ")?;
                            out.write_str(language)?;
                            out.write_str("\n")?;
                            self.at_line_start = true;
                            self.raw = true;
                        }
                        jotdown::Container::Span => out.write_str("[")?,
                        jotdown::Container::Link(cow, link_type) => match link_type {
                            jotdown::LinkType::Span(span_link_type) => match span_link_type {
                                jotdown::SpanLinkType::Inline => out.write_str("[")?,
                                jotdown::SpanLinkType::Reference => out.write_str("[")?,
                                jotdown::SpanLinkType::Unresolved => out.write_str("[")?,
                            },
                            jotdown::LinkType::AutoLink => out.write_str("<")?,
                            jotdown::LinkType::Email => out.write_str("<")?,
                        },
                        jotdown::Container::Image(cow, span_link_type) => out.write_str("![")?,
                        jotdown::Container::Verbatim => {
                            out.write_str("`")?;
                            self.raw = true;
                        }
                        jotdown::Container::Math { display } => match display {
                            true => out.write_str("$$`")?,
                            false => out.write_str("$`")?,
                        },
                        jotdown::Container::RawInline { format: _ } => out.write_str("`")?,
                        jotdown::Container::Subscript => out.write_str("{~")?,
                        jotdown::Container::Superscript => out.write_str("{^")?,
                        jotdown::Container::Insert => out.write_str("{+")?,
                        jotdown::Container::Delete => out.write_str("{-")?,
                        jotdown::Container::Strong => out.write_str("{*")?,
                        jotdown::Container::Emphasis => out.write_str("{_")?,
                        jotdown::Container::Mark => out.write_str("{=")?,
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
                            out.write_str("\n")?;
                        }
                        jotdown::Container::TableRow { head } => {
                            out.write_str("\n")?;
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }

                            if head {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                out.write_str("\n")?;
                                self.at_line_start = true;
                            }
                        }
                        jotdown::Container::Section { id: _ } => (),
                        jotdown::Container::Div { class: _ } => {
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                                self.at_line_start = false;
                            }
                            out.write_str(":::\n")?;
                            self.at_line_start = true;
                        }
                        jotdown::Container::Paragraph => {
                            out.write_str("\n")?;
                            self.at_line_start = true;
                        }
                        jotdown::Container::Heading {
                            level: _,
                            has_section: _,
                            id: _,
                        } => {
                            out.write_str("\n")?;
                            self.prefix.pop();
                            self.at_line_start = true;
                            log::trace!("Prefix: {:?}", self.prefix);
                        }
                        jotdown::Container::TableCell { alignment, head } => {
                            out.write_str(" |")?;
                        }
                        jotdown::Container::Caption => todo!(),
                        jotdown::Container::DescriptionTerm => out.write_str("\n")?,
                        jotdown::Container::LinkDefinition { label } => {
                            out.write_str("\n")?;
                            self.prefix.pop();
                            self.at_line_start = true;
                        }
                        jotdown::Container::RawBlock { format } => {
                            self.raw = false;
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                            }
                            out.write_str("```\n")?;
                            self.at_line_start = true;
                        }
                        jotdown::Container::CodeBlock { language: _ } => {
                            self.raw = false;
                            if self.at_line_start {
                                for prefix in self.prefix.iter() {
                                    out.write_str(prefix)?;
                                }
                            }
                            out.write_str("```\n")?;
                            self.at_line_start = true;
                        }
                        jotdown::Container::Span => {
                            out.write_str("]")?;
                        }
                        jotdown::Container::Link(cow, link_type) => match link_type {
                            jotdown::LinkType::Span(span_link_type) => {
                                out.write_str("]")?;
                                match span_link_type {
                                    jotdown::SpanLinkType::Inline => {
                                        out.write_str("(")?;
                                        out.write_str(&cow)?;
                                        out.write_str(")")?;
                                    }
                                    jotdown::SpanLinkType::Reference => {
                                        out.write_str("[")?;
                                        out.write_str(&cow)?;
                                        out.write_str("]")?;
                                    }
                                    jotdown::SpanLinkType::Unresolved => {
                                        out.write_str("[")?;
                                        out.write_str(&cow)?;
                                        out.write_str("]")?;
                                    }
                                }
                            }
                            jotdown::LinkType::AutoLink => out.write_str(">")?,
                            jotdown::LinkType::Email => out.write_str(">")?,
                        },
                        jotdown::Container::Image(cow, span_link_type) => {
                            out.write_str("](")?;
                            out.write_str(&cow)?;
                            out.write_str(")")?;
                        }
                        jotdown::Container::Verbatim => {
                            self.raw = false;
                            out.write_str("`")?;
                        }
                        jotdown::Container::Math { display: _ } => out.write_str("`")?,
                        jotdown::Container::RawInline { format } => {
                            out.write_str("`={")?;
                            out.write_str(format)?;
                            out.write_str("}")?;
                        }
                        jotdown::Container::Subscript => out.write_str("~}")?,
                        jotdown::Container::Superscript => out.write_str("^}")?,
                        jotdown::Container::Insert => out.write_str("+}")?,
                        jotdown::Container::Delete => out.write_str("-}")?,
                        jotdown::Container::Strong => out.write_str("*}")?,
                        jotdown::Container::Emphasis => out.write_str("_}")?,
                        jotdown::Container::Mark => out.write_str("=}")?,
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
                                    out.write_str(" ")?;
                                    out.write_str(key)?;
                                    out.write_str("=")?;
                                }
                                jotdown::AttributeKind::Comment => {
                                    out.write_str("%")?;
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
                        out.write_str(" }")?;
                    }
                }
                jotdown::Event::Str(cow) => match self.raw {
                    true => out.write_str(&cow)?,
                    false => {
                        let mut space = false;
                        for char in cow.chars() {
                            if !char.is_whitespace() {
                                space = false;
                                out.write_str(char.to_string().as_str())?;
                                continue;
                            }

                            if space {
                                continue;
                            }

                            out.write_str(" ")?;

                            space = true;
                        }
                    }
                },
                jotdown::Event::FootnoteReference(str) => {
                    out.write_str("[^")?;
                    out.write_str(str)?;
                    out.write_str("]")?;
                }
                jotdown::Event::Symbol(cow) => {
                    out.write_str(":")?;
                    out.write_str(&cow)?;
                    out.write_str(":")?;
                }
                jotdown::Event::LeftSingleQuote => out.write_str("{\'")?,
                jotdown::Event::RightSingleQuote => out.write_str("\'}")?,
                jotdown::Event::LeftDoubleQuote => out.write_str("{\"")?,
                jotdown::Event::RightDoubleQuote => out.write_str("\"}")?,
                jotdown::Event::Ellipsis => out.write_str("...")?,
                jotdown::Event::EnDash => out.write_str("--")?,
                jotdown::Event::EmDash => out.write_str("---")?,
                jotdown::Event::NonBreakingSpace => todo!(),
                jotdown::Event::Softbreak => {
                    out.write_str("\n")?;
                    for prefix in self.prefix.iter() {
                        out.write_str(prefix)?;
                    }
                    self.at_line_start = false;
                }
                jotdown::Event::Hardbreak => {
                    out.write_str("\\\n")?;
                    for prefix in self.prefix.iter() {
                        out.write_str(prefix)?;
                    }
                    self.at_line_start = false;
                }
                jotdown::Event::Escape => out.write_str("\\")?,
                jotdown::Event::Blankline => {
                    let mut pending = String::new();
                    if self.at_line_start {
                        for prefix in self.prefix.iter() {
                            pending.write_str(prefix)?;
                        }
                    }
                    let pending = pending.trim_end();
                    out.write_str(&pending)?;
                    out.write_str("\n")?;
                    self.at_line_start = true;
                }
                jotdown::Event::ThematicBreak(attributes) => {
                    out.write_str(
                        "* * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * * *\n",
                    )?;
                    for prefix in self.prefix.iter() {
                        out.write_str(prefix)?;
                    }
                }
                jotdown::Event::Attributes(attributes) => {
                    if self.at_line_start {
                        for prefix in self.prefix.iter() {
                            out.write_str(prefix)?;
                        }
                        self.at_line_start = false;
                    }
                    out.write_str("{")?;
                    for (k, v) in attributes {
                        match k {
                            jotdown::AttributeKind::Class => {
                                out.write_str(" .")?;
                            }
                            jotdown::AttributeKind::Id => {
                                out.write_str(" #")?;
                            }
                            jotdown::AttributeKind::Pair { key } => {
                                out.write_str(" ")?;
                                out.write_str(key)?;
                                out.write_str("=")?;
                            }
                            jotdown::AttributeKind::Comment => {
                                out.write_str("%")?;
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
                    out.write_str("}\n")?;
                    self.at_line_start = true;
                }
            }
        }
        log::trace!("Events rendered");
        Ok(())
    }
}
