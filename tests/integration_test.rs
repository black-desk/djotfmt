use djotfmt::WriterConfig;
use libtest_mimic::{Arguments, Failed, Trial};
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

fn run_format_test(
    input_path: std::path::PathBuf,
    expected_path: std::path::PathBuf,
) -> Result<(), Failed> {
    let input = std::fs::read_to_string(&input_path).map_err(|e| e.to_string())?;
    let expected = std::fs::read_to_string(&expected_path).map_err(|e| e.to_string())?;

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

    assert_eq!(output, expected, "test case {:?}", input_path.file_stem().unwrap());
    Ok(())
}

fn run_idempotent_test(path: std::path::PathBuf) -> Result<(), Failed> {
    let input = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;

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

    assert_eq!(output, input, "idempotent test failed for {:?}", path.file_stem().unwrap());
    Ok(())
}

fn main() {
    let args = Arguments::from_args();
    let mut trials = Vec::new();

    let tests = discover_tests("./tests/", &["in", "out"]);
    assert!(!tests.is_empty(), "no test cases found");

    for paths in &tests {
        let input_path = paths
            .iter()
            .find(|p| p.extension().unwrap() == "in")
            .unwrap()
            .clone();
        let expected_path = paths
            .iter()
            .find(|p| p.extension().unwrap() == "out")
            .unwrap()
            .clone();
        let name = input_path
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        trials.push(Trial::test(name, move || {
            run_format_test(input_path, expected_path)
        }));
    }

    let idem_tests = discover_tests("./tests/", &["out"]);
    assert!(!idem_tests.is_empty(), "no idempotent test cases found");

    for paths in &idem_tests {
        let path = paths[0].clone();
        let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
        let name = format!("idempotent::{}", stem);

        trials.push(Trial::test(name, move || run_idempotent_test(path)));
    }

    libtest_mimic::run(&args, trials).exit();
}
