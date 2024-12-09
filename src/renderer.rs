pub struct Renderer {}

impl Renderer {
    pub fn new() -> Self {
        Self {}
    }

    pub fn push_offset<'s, I, W>(&self, events: I, mut out: W) -> std::fmt::Result
    where
        I: Iterator<Item = (jotdown::Event<'s>, std::ops::Range<usize>)>,
        W: std::fmt::Write,
    {
        let mut writer = Writer::new();
        writer.push(events, &mut out)?;
        Ok(())
    }
}

struct Writer {
    list_bullet_type: Option<jotdown::ListBulletType>,
    raw: bool,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            list_bullet_type: None,
            raw: false,
        }
    }

    fn push<'s, I, W>(&mut self, events: I, mut out: W) -> std::fmt::Result
    where
        I: Iterator<Item = (jotdown::Event<'s>, std::ops::Range<usize>)>,
        W: std::fmt::Write,
    {
        log::trace!("Start render events");

        for e in events {
            log::debug!("{:?}", e);

            let (e, range) = e;

            match e {
                jotdown::Event::Start(container, attributes) => match container {
                    jotdown::Container::Blockquote => todo!(),
                    jotdown::Container::List { kind, tight } => match kind {
                        jotdown::ListKind::Unordered(list_bullet_type) => {
                            self.list_bullet_type = Some(list_bullet_type);
                        }
                        jotdown::ListKind::Ordered {
                            numbering,
                            style,
                            start,
                        } => todo!(),
                        jotdown::ListKind::Task(list_bullet_type) => todo!(),
                    },
                    jotdown::Container::ListItem => {
                        match self.list_bullet_type {
                            None => todo!(),
                            Some(list_bullet_type) => match list_bullet_type {
                                jotdown::ListBulletType::Dash => out.write_str("-")?,
                                jotdown::ListBulletType::Star => out.write_str("*")?,
                                jotdown::ListBulletType::Plus => out.write_str("+")?,
                            },
                        };
                        out.write_str(" ")?;
                    }
                    jotdown::Container::TaskListItem { checked } => todo!(),
                    jotdown::Container::DescriptionList => todo!(),
                    jotdown::Container::DescriptionDetails => todo!(),
                    jotdown::Container::Footnote { label } => todo!(),
                    jotdown::Container::Table => todo!(),
                    jotdown::Container::TableRow { head } => todo!(),
                    jotdown::Container::Section { id } => (),
                    jotdown::Container::Div { class } => {
                        out.write_str("::: ")?;
                        out.write_str(class)?;
                        out.write_str("\n")?;
                    }
                    jotdown::Container::Paragraph => (),
                    jotdown::Container::Heading {
                        level,
                        has_section: _,
                        id: _,
                    } => {
                        out.write_str("#".repeat(level.into()).as_str())?;
                        out.write_str(" ")?;
                    }
                    jotdown::Container::TableCell { alignment, head } => todo!(),
                    jotdown::Container::Caption => todo!(),
                    jotdown::Container::DescriptionTerm => todo!(),
                    jotdown::Container::LinkDefinition { label } => {
                        out.write_str("[")?;
                        out.write_str(label)?;
                        out.write_str("]: ")?;
                    }
                    jotdown::Container::RawBlock { format } => todo!(),
                    jotdown::Container::CodeBlock { language } => {
                        out.write_str("``` ")?;
                        out.write_str(language)?;
                        out.write_str("\n")?;
                        self.raw = true;
                    }
                    jotdown::Container::Span => todo!(),
                    jotdown::Container::Link(cow, link_type) => match link_type {
                        jotdown::LinkType::Span(span_link_type) => match span_link_type {
                            jotdown::SpanLinkType::Inline => out.write_str("[")?,
                            jotdown::SpanLinkType::Reference => out.write_str("[")?,
                            jotdown::SpanLinkType::Unresolved => todo!(),
                        },
                        jotdown::LinkType::AutoLink => out.write_str("<")?,
                        jotdown::LinkType::Email => out.write_str("<")?,
                    },
                    jotdown::Container::Image(cow, span_link_type) => out.write_str("![")?,
                    jotdown::Container::Verbatim => {
                        out.write_str("`")?;
                        self.raw = true;
                    }
                    jotdown::Container::Math { display } => todo!(),
                    jotdown::Container::RawInline { format } => todo!(),
                    jotdown::Container::Subscript => todo!(),
                    jotdown::Container::Superscript => todo!(),
                    jotdown::Container::Insert => todo!(),
                    jotdown::Container::Delete => todo!(),
                    jotdown::Container::Strong => todo!(),
                    jotdown::Container::Emphasis => todo!(),
                    jotdown::Container::Mark => todo!(),
                },
                jotdown::Event::End(container) => match container {
                    jotdown::Container::Blockquote => todo!(),
                    jotdown::Container::List { kind: _, tight: _ } => (),
                    jotdown::Container::ListItem => (),
                    jotdown::Container::TaskListItem { checked } => todo!(),
                    jotdown::Container::DescriptionList => todo!(),
                    jotdown::Container::DescriptionDetails => todo!(),
                    jotdown::Container::Footnote { label } => todo!(),
                    jotdown::Container::Table => todo!(),
                    jotdown::Container::TableRow { head } => todo!(),
                    jotdown::Container::Section { id: _ } => (),
                    jotdown::Container::Div { class: _ } => out.write_str(":::\n")?,
                    jotdown::Container::Paragraph => out.write_str("\n")?,
                    jotdown::Container::Heading {
                        level: _,
                        has_section: _,
                        id: _,
                    } => out.write_str("\n")?,
                    jotdown::Container::TableCell { alignment, head } => todo!(),
                    jotdown::Container::Caption => todo!(),
                    jotdown::Container::DescriptionTerm => todo!(),
                    jotdown::Container::LinkDefinition { label } => out.write_str("\n")?,
                    jotdown::Container::RawBlock { format } => todo!(),
                    jotdown::Container::CodeBlock { language: _ } => {
                        self.raw = false;
                        out.write_str("```\n")?;
                    }
                    jotdown::Container::Span => todo!(),
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
                                    out.write_str("]")?;
                                }
                                jotdown::SpanLinkType::Unresolved => todo!(),
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
                    jotdown::Container::Math { display } => todo!(),
                    jotdown::Container::RawInline { format } => todo!(),
                    jotdown::Container::Subscript => todo!(),
                    jotdown::Container::Superscript => todo!(),
                    jotdown::Container::Insert => todo!(),
                    jotdown::Container::Delete => todo!(),
                    jotdown::Container::Strong => todo!(),
                    jotdown::Container::Emphasis => todo!(),
                    jotdown::Container::Mark => todo!(),
                },
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
                jotdown::Event::FootnoteReference(_) => todo!(),
                jotdown::Event::Symbol(cow) => todo!(),
                jotdown::Event::LeftSingleQuote => todo!(),
                jotdown::Event::RightSingleQuote => todo!(),
                jotdown::Event::LeftDoubleQuote => todo!(),
                jotdown::Event::RightDoubleQuote => todo!(),
                jotdown::Event::Ellipsis => todo!(),
                jotdown::Event::EnDash => todo!(),
                jotdown::Event::EmDash => todo!(),
                jotdown::Event::NonBreakingSpace => todo!(),
                jotdown::Event::Softbreak => todo!(),
                jotdown::Event::Hardbreak => todo!(),
                jotdown::Event::Escape => todo!(),
                jotdown::Event::Blankline => out.write_str("\n")?,
                jotdown::Event::ThematicBreak(attributes) => {
                    out.write_str("---\n")?;
                }
                jotdown::Event::Attributes(attributes) => {
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
                }
            }
        }
        log::trace!("Events rendered");
        Ok(())
    }
}
