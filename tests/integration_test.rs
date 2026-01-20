use djotfmt::WriterConfig;
use pretty_assertions::assert_eq;
use test_each_file::test_each_path;

test_each_path! { for ["in", "out"] in "./tests/" => test }

fn test([input_path, expected]: [&std::path::Path; 2]) {
    let mut output = String::new();

    let input = std::fs::read_to_string(input_path).unwrap();

    let max_cols = parse_max_cols_from_filename(input_path);
    let config = WriterConfig { max_cols };

    djotfmt::Renderer::new(&input)
        .push_offset(
            jotdown::Parser::new(&input).into_offset_iter(),
            &mut output,
            &config,
        )
        .unwrap();

    let expected = std::fs::read_to_string(&expected).unwrap();

    assert_eq!(output.as_str(), expected);
}

fn parse_max_cols_from_filename(path: &std::path::Path) -> usize {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.rsplit_once("_at_"))
        .and_then(|(_, cols)| cols.parse().ok())
        .unwrap_or(72)
}
