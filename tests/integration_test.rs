use jotdown::Render;

fn format(input: &str, expected: &str) {
    let input = std::fs::read_to_string(input).unwrap();

    let mut output = &mut String::new();

    djotfmt::renderer::Renderer::new()
        .push(jotdown::Parser::new(input.as_str()), &mut output)
        .unwrap();

    let expected = std::fs::read_to_string(expected).unwrap();

    assert_eq!(output.as_str(), expected);
}

#[test]
fn test_format() {
    format("README.dj", "README.dj");

    let paths = std::fs::read_dir("tests/data").unwrap();

    for path in paths {
        let Ok(path) = path else { continue };
        let Ok(file_type) = path.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = path.path();
        let input = path.join("input.dj");
        let expected = path.join("expected.dj");

        format(input.to_str().unwrap(), expected.to_str().unwrap());
    }
}
