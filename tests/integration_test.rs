use djotfmt::WriterConfig;
use pretty_assertions::assert_eq;

fn discover_tests(base: &str, extensions: &[&str]) -> Vec<Vec<std::path::PathBuf>> {
    let base = std::path::Path::new(base);
    let mut groups: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
        std::collections::BTreeMap::new();

    for ext in extensions {
        let pattern = base.join(format!("*.{ext}"));
        for entry in glob::glob(pattern.to_str().unwrap()).unwrap() {
            let path = entry.unwrap();
            let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
            groups.entry(stem).or_default().push(path);
        }
    }

    groups
        .into_values()
        .filter(|paths| paths.len() == extensions.len())
        .collect()
}

#[test]
fn test_all() {
    let tests = discover_tests("./tests/", &["in", "out"]);
    assert!(!tests.is_empty(), "no test cases found");

    for paths in &tests {
        let input_path = paths.iter().find(|p| p.extension().unwrap() == "in").unwrap();
        let expected_path = paths.iter().find(|p| p.extension().unwrap() == "out").unwrap();

        let input = std::fs::read_to_string(input_path).unwrap();
        let expected = std::fs::read_to_string(expected_path).unwrap();

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

        assert_eq!(
            output, expected,
            "test case {:?} failed",
            input_path.file_stem().unwrap()
        );
    }
}

#[test]
fn test_idempotent() {
    let tests = discover_tests("./tests/", &["out"]);
    assert!(!tests.is_empty(), "no test cases found");

    for paths in &tests {
        let path = &paths[0];
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

        assert_eq!(
            output, input,
            "idempotent test failed for {:?}",
            path.file_stem().unwrap()
        );
    }
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
