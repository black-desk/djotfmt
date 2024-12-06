use super::*;

fn format(input: &str) {
    let content = std::fs::read_to_string(input).unwrap();
    let mut output = &mut String::new();
    Renderer::new()
        .push(jotdown::Parser::new(content.as_str()), &mut output)
        .unwrap();
    assert_eq!(output.as_str(), content);
}

#[test]
fn test_format() {
    format("README.dj");
}
