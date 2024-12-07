use pretty_assertions::assert_eq;
use test_each_file::test_each_path;

test_each_path! { for ["in", "out"] in "./tests/" => test }

fn test([input, expected]: [&std::path::Path; 2]) {
    let mut output = &mut String::new();

    let input = std::fs::read_to_string(input).unwrap();

    let input = input.as_str();

    djotfmt::Renderer::new(input)
        .push_offset(jotdown::Parser::new(input).into_offset_iter(), &mut output)
        .unwrap();

    let expected = std::fs::read_to_string(&expected).unwrap();

    assert_eq!(output.as_str(), expected);
}
