use pretty_assertions::{assert_eq, assert_ne};

fn format(input: &std::path::Path, expected: &std::path::Path) {
    let input = std::fs::read_to_string(&input).unwrap();

    let mut output = &mut String::new();

    let input = input.as_str();

    djotfmt::Renderer::new(input)
        .push_offset(jotdown::Parser::new(input).into_offset_iter(), &mut output)
        .unwrap();

    let expected = std::fs::read_to_string(&expected).unwrap();

    assert_eq!(output.as_str(), expected);
}

macro_rules! djotfmt_test {
    ($name:ident, $dir:literal) => {
        #[test]
        fn $name() {
            format(
                &std::path::Path::new("tests")
                    .join($dir)
                    .join("input.dj"),
                &std::path::Path::new("tests")
                    .join($dir)
                    .join("expected.dj"),
            );
        }
    };
}

djotfmt_test!(syntax_reference, "references");
djotfmt_test!(readme, "readme");
djotfmt_test!(keep_comment, "keep-comments");
djotfmt_test!(heading, "heading");
