// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use libtest_mimic::{Arguments, Failed, Trial};
use pretty_assertions::assert_eq;

/// Format events as a string matching djot CLI output.
fn format_events(events: &[djotfmt::parser::Event]) -> String {
    if events.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::new();
    let last = events.len() - 1;
    for (i, ev) in events.iter().enumerate() {
        let comma = if i < last { "," } else { "" };
        if i == 0 {
            out.push_str(&format!(
                "[{{ startpos: {}, endpos: {}, annot: {:?} }}{}",
                ev.startpos, ev.endpos, ev.annot, comma
            ));
        } else {
            out.push_str(&format!(
                "\n {{ startpos: {}, endpos: {}, annot: {:?} }}{}",
                ev.startpos, ev.endpos, ev.annot, comma
            ));
        }
    }
    out.push(']');
    out
}

/// Precompute a mapping from UTF-8 byte offset to UTF-16 code unit index.
/// For a byte offset that lands in the middle of a multi-byte character,
/// returns the UTF-16 index of the character containing that byte.
fn build_byte_to_utf16_map(input: &str) -> Vec<usize> {
    let len = input.len();
    let mut map = vec![0usize; len + 1];
    let mut byte_pos = 0;
    let mut utf16_pos = 0;
    while byte_pos < len {
        let c = input[byte_pos..].chars().next().unwrap();
        let utf8_len = c.len_utf8();
        let utf16_len = c.len_utf16();
        let last_utf16 = utf16_pos + utf16_len - 1;
        for b in byte_pos..byte_pos + utf8_len {
            map[b] = last_utf16;
        }
        byte_pos += utf8_len;
        utf16_pos += utf16_len;
    }
    map[len] = utf16_pos;
    map
}

/// Parse a .test file and extract input for each test case.
/// Format: test cases are delimited by ``` lines, input/output separated by `.`.
fn parse_test_file(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut cases = Vec::new();
    let mut idx = 0;

    while idx < lines.len() {
        // Find opening ```
        while idx < lines.len() && !lines[idx].starts_with("```") {
            idx += 1;
        }
        if idx >= lines.len() {
            break;
        }
        idx += 1;

        // Read input until `.` or `!` on its own line
        let mut input = String::new();
        while idx < lines.len() {
            let line = lines[idx];
            if line == "." || line == "!" {
                break;
            }
            input.push_str(line);
            input.push('\n');
            idx += 1;
        }
        if idx < lines.len() {
            idx += 1; // skip the `.` or `!` line
        }

        // Skip output until closing ```
        while idx < lines.len() && !lines[idx].starts_with("```") {
            idx += 1;
        }
        if idx < lines.len() {
            idx += 1; // skip closing ```
        }

        cases.push(input);
    }

    cases
}

/// Run `djot --to events` on the given input and return raw stdout.
fn get_djot_js_output(input: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Use the djot.js submodule's CLI for consistent expected output
    let djot_cli = std::path::Path::new("third_party/djot.js/lib/cli.js");

    let mut child = Command::new("node")
        .arg(djot_cli)
        .arg("--to")
        .arg("events")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn djot CLI: {}. Is it installed?", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| format!("Failed to write to djot stdin: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for djot: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("djot CLI failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn check_djot_available() -> bool {
    let djot_cli = std::path::Path::new("third_party/djot.js/lib/cli.js");
    if !djot_cli.exists() {
        return false;
    }
    std::process::Command::new("node")
        .arg(djot_cli)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_parser_event_test(
    test_file: &str,
    case_index: usize,
    input: &str,
) -> Result<(), Failed> {
    let expected = get_djot_js_output(input).map_err(Failed::from)?;

    // Normalize input same way as parser (add trailing \n)
    let normalized = if input.ends_with('\n') {
        input.to_string()
    } else {
        format!("{}\n", input)
    };
    let map = build_byte_to_utf16_map(&normalized);

    // Get Rust parser output (byte offsets) and convert to UTF-16 positions
    let rust_events = djotfmt::parser::parse_events(input);
    let utf16_events: Vec<djotfmt::parser::Event> = rust_events.into_iter().map(|ev| {
        djotfmt::parser::Event {
            startpos: map.get(ev.startpos).copied().unwrap_or(0),
            endpos: map.get(ev.endpos).copied().unwrap_or(0),
            annot: ev.annot,
        }
    }).collect();
    let actual = format_events(&utf16_events);

    assert_eq!(
        actual, expected,
        "{}: case #{} (input: {:?})",
        test_file,
        case_index,
        &input[..input.len().min(80)]
    );
    Ok(())
}

fn main() {
    let args = Arguments::from_args();

    if !check_djot_available() {
        eprintln!("SKIP: djot CLI not found. Install with: npm install -g @djot/djot");
        libtest_mimic::run(&args, vec![]).exit();
    }

    let test_dir = std::path::Path::new("third_party/djot.js/test");
    let mut trials = Vec::new();

    let entries: Vec<_> = glob::glob(test_dir.join("*.test").to_str().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();

    // Cases to skip: our parser fixes djot.js bugs, so these differ from
    // djot.js CLI output. Text before image markers is correctly emitted
    // as a str event, but djot.js omits it.
    let skip_cases: &[(&str, usize)] = &[
        ("links_and_images.test", 1),
        ("links_and_images.test", 16),
        ("links_and_images.test", 18),
        ("links_and_images.test", 26),
    ];

    for entry in &entries {
        let file_name = entry.file_name().unwrap().to_str().unwrap().to_string();
        let content = std::fs::read_to_string(entry).unwrap();
        let cases = parse_test_file(&content);

        for (case_idx, input) in cases.into_iter().enumerate() {
            let name = format!("{}::case_{}", file_name, case_idx);
            if skip_cases.contains(&(file_name.as_str(), case_idx)) {
                trials.push(Trial::test(name, || Ok(())));
                continue;
            }
            let test_file = file_name.clone();
            trials.push(Trial::test(name, move || {
                run_parser_event_test(&test_file, case_idx, &input)
            }));
        }
    }

    if trials.is_empty() {
        eprintln!("No test cases found in {}", test_dir.display());
    }

    // Integration test files that hit the image-marker bug fixed in our parser
    let skip_integration: &[&str] = &[
        "links-and-images",
        "wrap-inline-boundary",
    ];

    // Also test integration .in/.out files
    let int_entries: Vec<_> = glob::glob("tests/*.in")
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    for entry in &int_entries {
        let in_path = entry.as_path();
        let stem = in_path.file_stem().unwrap().to_str().unwrap().to_string();
        let in_content = std::fs::read_to_string(in_path).unwrap();

        let skip = skip_integration.contains(&stem.as_str());

        let in_stem = stem.clone();
        let in_name = format!("integration::{}::in", stem);
        let in_content_clone = in_content.clone();
        if skip {
            trials.push(Trial::test(in_name, || Ok(())));
        } else {
            trials.push(Trial::test(in_name, move || {
                run_parser_event_test(&format!("{}.in", in_stem), 0, &in_content_clone)
            }));
        }

        let out_path = in_path.with_extension("out");
        if out_path.exists() {
            let out_content = std::fs::read_to_string(&out_path).unwrap();
            let out_name = format!("integration::{}::out", stem);
            if skip {
                trials.push(Trial::test(out_name, || Ok(())));
            } else {
                trials.push(Trial::test(out_name, move || {
                    run_parser_event_test(&format!("{}.out", stem), 0, &out_content)
                }));
            }
        }
    }

    libtest_mimic::run(&args, trials).exit();
}
