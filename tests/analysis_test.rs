// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

use djotfmt::analysis::{text::LineIndex, CompletionKind, Document, SymbolKind};

#[test]
fn positions_handle_unicode_crlf_and_invalid_edits() {
    let text = "中😀x\r\nlast";
    let index = LineIndex::new(text);
    assert_eq!(index.offset(text, 0, 1), Some(3));
    assert_eq!(index.offset(text, 0, 2), None);
    assert_eq!(index.offset(text, 0, 3), Some(7));
    assert_eq!(index.offset(text, 0, 5), None);
    assert_eq!(index.position(text, 7), (0, 3));
    assert_eq!(index.offset(text, 1, 4), Some(text.len()));
    assert_eq!(index.offset(text, 2, 0), None);
}

#[test]
fn links_footnotes_and_heading_references_resolve_with_precise_names() {
    let source =
        "# Intro\n\n[x][guide] [Intro][] [x](#Intro) [^a]\n\n[guide]: manual.dj\n[^a]: note";
    let doc = Document::new(source.into());
    assert_eq!(doc.references.len(), 4, "{:#?}", doc.references);
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    for r in &doc.references {
        assert!(r.target.is_some(), "{r:?}");
        assert_eq!(&source[r.name_range.clone()], r.key);
    }
    let link = doc.references.iter().find(|r| r.key == "guide").unwrap();
    let target = &doc.definitions[link.target.unwrap()];
    assert_eq!(&source[target.name_range.clone()], "guide");
    assert_eq!(target.destination, "manual.dj");
    assert_eq!(doc.references_to(link.target.unwrap()).count(), 1);
}

#[test]
fn duplicate_definitions_override_implicit_labels_and_preserve_diagnostics() {
    let doc = Document::new("# Same\n\n[Same][]\n\n[Same]: first\n[Same]: last\n".into());
    assert_eq!(
        doc.definitions[doc.references[0].target.unwrap()].destination,
        "last"
    );
    assert!(doc
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Duplicate")));
    assert_eq!(
        doc.definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Link)
            .count(),
        2
    );
}

#[test]
fn headings_generate_unique_ids_and_respect_explicit_ids() {
    let doc = Document::new("{#custom}\n# Title\n\n## Child\n\n# Title\n\n# Title\n".into());
    let ids: Vec<_> = doc
        .definitions
        .iter()
        .filter(|d| d.kind == SymbolKind::Anchor)
        .map(|d| d.key.as_str())
        .collect();
    assert_eq!(ids, ["custom", "Child", "Title", "Title-1"]);
    assert_eq!(doc.headings[1].parent, Some(0));
    assert_eq!(doc.headings[2].parent, None);
}

#[test]
fn context_handles_incomplete_input_and_code() {
    for (source, kind) in [
        ("[text][gu", CompletionKind::Link),
        ("[^no", CompletionKind::Footnote),
        ("[text](#In", CompletionKind::Anchor),
        ("[", CompletionKind::Template),
    ] {
        let doc = Document::new(source.into());
        assert_eq!(
            doc.completion_context(source.len()).map(|c| c.kind),
            Some(kind),
            "{source}"
        );
    }
    for source in ["\\[", "`[text][gu`", "```\n[text][gu", "$`[text][gu`"] {
        let doc = Document::new(source.into());
        assert!(doc.completion_context(source.len()).is_none(), "{source}");
    }
}

#[test]
fn completion_replaces_the_existing_label_and_closing_bracket() {
    let source = "[text][guide]";
    let doc = Document::new(source.into());
    let c = doc
        .completion_context(source.find("guide").unwrap() + 2)
        .unwrap();
    assert_eq!(c.prefix, "gu");
    assert_eq!(&source[c.replace], "guide]");
}

#[test]
fn source_ranges_are_valid_without_a_final_newline() {
    for source in [
        "",
        "中😀",
        "# 中😀",
        "[^中]: 脚注\n\n[^中]",
        "[中][标签]\n\n[标签]: 地址",
    ] {
        let doc = Document::new(source.into());
        for range in doc
            .definitions
            .iter()
            .flat_map(|d| [&d.name_range, &d.full_range])
            .chain(
                doc.references
                    .iter()
                    .flat_map(|r| [&r.name_range, &r.full_range]),
            )
        {
            assert!(source.get(range.clone()).is_some(), "{source:?}: {range:?}");
        }
    }
}

#[test]
fn heading_labels_exclude_link_targets_and_footnotes() {
    let doc = Document::new(
        "# A [link](https://example.com)[^n] :symbol:\n\n[A link][]\n\n[^n]: note\n".into(),
    );
    assert_eq!(doc.headings[0].title, "A link");
    assert!(doc
        .references
        .iter()
        .find(|r| r.key == "A link")
        .unwrap()
        .target
        .is_some());
}

#[test]
fn references_from_a_heading_include_implicit_label_and_anchor_uses() {
    let source = "# Title\n\n[Title][] [x](#Title)\n";
    let doc = Document::new(source.into());
    let target = doc.target_at(source.find("Title").unwrap()).unwrap();
    assert_eq!(doc.references_to(target).count(), 2);
}

#[test]
fn unattached_attributes_do_not_define_anchors() {
    for source in ["{#bad} text\n", "word {#bad}\n", "{#bad}\n\ntext\n"] {
        let doc = Document::new(source.into());
        assert!(
            doc.definitions.is_empty(),
            "{source}: {:?}",
            doc.definitions
        );
    }
    let source = "word{#good}\n\n[x](#good)\n";
    let doc = Document::new(source.into());
    assert_eq!(doc.definitions[0].key, "good");
    assert_eq!(&source[doc.definitions[0].full_range.clone()], "word");
    assert!(doc.references[0].target.is_some());
}

#[test]
fn reference_completion_can_replace_obsolete_inline_link_punctuation() {
    for source in ["[text][)", "[text][]()"] {
        let doc = Document::new(source.into());
        let offset = source.find("][").unwrap() + 2;
        let context = doc.completion_context(offset).unwrap();
        let mut edited = source.to_string();
        edited.replace_range(context.replace, "guide]");
        assert_eq!(edited, "[text][guide]");
    }
}

#[test]
fn analysis_accepts_existing_formatter_corpus() {
    for path in glob::glob("tests/*.in")
        .unwrap()
        .chain(glob::glob("tests/*.out").unwrap())
    {
        let path = path.unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        let doc = Document::new(source.clone());
        for range in doc
            .definitions
            .iter()
            .flat_map(|d| [&d.name_range, &d.full_range])
            .chain(
                doc.references
                    .iter()
                    .flat_map(|r| [&r.name_range, &r.full_range]),
            )
            .chain(doc.headings.iter().flat_map(|h| [&h.range, &h.selection]))
        {
            assert!(
                source.get(range.clone()).is_some(),
                "{}: {range:?}",
                path.display()
            );
        }
    }
}
