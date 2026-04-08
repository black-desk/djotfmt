use djotfmt::WriterConfig;
use pretty_assertions::assert_eq;
use test_each_file::test_each_path;

test_each_path! { for ["in", "out"] in "./tests/" => test }

fn test([input_path, expected]: [&std::path::Path; 2]) {
    let input = std::fs::read_to_string(input_path).unwrap();
    let expected = std::fs::read_to_string(&expected).unwrap();

    let max_cols = parse_max_cols(&input);
    let config = WriterConfig { max_cols };

    let mut output = String::new();
    djotfmt::Renderer::new(&input)
        .push_offset(
            jotdown::Parser::new(&input).into_offset_iter(),
            &mut output,
            &config,
        )
        .unwrap();

    assert_eq!(output, expected);
}

test_each_path! { for ["out"] in "./tests/" as idempotent => test_idempotent }

fn test_idempotent([path]: [&std::path::Path; 1]) {
    let input = std::fs::read_to_string(path).unwrap();

    let max_cols = parse_max_cols(&input);
    let config = WriterConfig { max_cols };

    let mut output = String::new();
    djotfmt::Renderer::new(&input)
        .push_offset(
            jotdown::Parser::new(&input).into_offset_iter(),
            &mut output,
            &config,
        )
        .unwrap();

    assert_eq!(output, input);
}

fn parse_max_cols(content: &str) -> usize {
    let mut max_cols = 72;
    for line in content.lines() {
        if let Some(idx) = line.find("@columns:") {
            let rest = &line[idx + "@columns:".len()..];
            let num = rest
                .trim_start()
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or("");
            if let Ok(cols) = num.parse::<usize>() {
                max_cols = cols;
            }
        }
    }
    max_cols
}
