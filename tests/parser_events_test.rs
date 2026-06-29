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

/// Path to the pinned djot.js submodule, relative to the crate root (integration
/// tests run with the crate root as their working directory).
const DJOT_JS: &str = "third_party/djot.js";

/// When set (to a non-empty, non-"0"/"false" value) the suite must actually run:
/// any condition that would otherwise cause a silent skip (missing JS toolchain,
/// missing submodule, or a failed build) instead fails the test binary. Set this
/// in CI so a runner-image change — e.g. npm disappearing — surfaces as a red
/// build instead of the conformance suite quietly vanishing.
fn require_conformance() -> bool {
    matches!(
        std::env::var("DJOTFMT_REQUIRE_CONFORMANCE"),
        Ok(v) if !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    )
}

/// Print an error and exit non-zero, failing the test run.
fn fatal(msg: &str) -> ! {
    eprintln!("error: {msg}.");
    std::process::exit(1)
}

/// Ensure `third_party/djot.js/lib/cli.js` is built from the pinned submodule
/// source before the suite runs. `lib/` is a gitignored build artifact
/// (`tsc && webpack`), so it is absent on a fresh checkout and would otherwise
/// make the whole suite skip silently. Building it here — its only consumer —
/// keeps the formatter itself free of any JS toolchain requirement.
fn ensure_djot_built() {
    use std::path::Path;

    let base = Path::new(DJOT_JS);

    // Submodule not checked out (e.g. a shallow clone) — nothing to build; the
    // suite skips via `check_djot_available`.
    if !base.join("src").is_dir() {
        if require_conformance() {
            fatal(
                "DJOTFMT_REQUIRE_CONFORMANCE is set but the djot.js submodule is \
                 not checked out",
            );
        }
        return;
    }

    let lib_cli = base.join("lib/cli.js");
    if lib_cli.exists() && is_fresh(&lib_cli, &base.join("src")) {
        return;
    }

    // No JS toolchain (or explicitly opted out) — leave the gap to
    // `check_djot_available`, which skips the suite with a clear message.
    if std::env::var_os("DJOTFMT_SKIP_JS_BUILD").is_some() || !have("node") || !have("npm") {
        if require_conformance() {
            let reason = if std::env::var_os("DJOTFMT_SKIP_JS_BUILD").is_some() {
                "DJOTFMT_SKIP_JS_BUILD is set".to_string()
            } else {
                "node/npm is not available on PATH".to_string()
            };
            fatal(&format!(
                "DJOTFMT_REQUIRE_CONFORMANCE is set but the djot.js reference build \
                 cannot be produced ({reason})"
            ));
        }
        eprintln!(
            "note: djot.js lib/cli.js is missing or stale and no JS toolchain is \
             available; parser conformance tests will be skipped. Build manually with \
             `npm --prefix {DJOT_JS} ci && npm --prefix {DJOT_JS} run build`, or set \
             DJOTFMT_SKIP_JS_BUILD=1 to silence this."
        );
        return;
    }

    eprintln!("note: building djot.js lib/ from the pinned submodule source...");

    // `npm ci` installs exactly from the lockfile, but on non-macOS hosts it
    // also rewrites yarn.lock to drop the macOS-only `fsevents` optional
    // dependency. Snapshot and restore the lockfiles so the submodule working
    // tree stays clean.
    if !base.join("node_modules").is_dir() {
        let snapshots = ["yarn.lock", "package-lock.json"]
            .map(|name| (name, std::fs::read(base.join(name)).ok()));
        npm(&["ci"], "install djot.js dependencies (locked)");
        for (name, snapshot) in &snapshots {
            if let Some(content) = snapshot {
                let _ = std::fs::write(base.join(name), content);
            }
        }
    }

    npm(&["run", "build"], "build djot.js lib/ from the pinned source");
}

/// `true` iff `lib_cli` is at least as new as the newest file under `src_dir`.
fn is_fresh(lib_cli: &std::path::Path, src_dir: &std::path::Path) -> bool {
    let Some(lib_mtime) = mtime(lib_cli) else {
        return false;
    };
    let mut fresh = true;
    walk(src_dir, &mut |path| {
        if let Some(t) = mtime(path) {
            if t > lib_mtime {
                fresh = false;
            }
        }
    });
    fresh
}

/// Probe whether a command is runnable by asking it for its version.
fn have(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Run an `npm` command inside the djot.js submodule, exiting the test binary
/// (and thus failing the suite) on error.
fn npm(args: &[&str], label: &str) {
    let status = std::process::Command::new("npm")
        .args(args)
        .current_dir(DJOT_JS)
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "error: failed to {label} (`npm {}` exited with {status}).",
                args.join(" ")
            );
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("error: failed to {label}: cannot run `npm`: {err}.");
            std::process::exit(1);
        }
    }
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn walk(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, f);
        } else {
            f(&path);
        }
    }
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

    // Build the djot.js reference CLI from the pinned submodule so the suite
    // compares against the right artifacts instead of silently skipping on a
    // fresh checkout. This is the only consumer of lib/cli.js, so the build
    // lives here rather than in build.rs (the formatter itself stays JS-free).
    ensure_djot_built();

    if !check_djot_available() {
        if require_conformance() {
            fatal(
                "DJOTFMT_REQUIRE_CONFORMANCE is set but the djot.js CLI is unavailable \
                 (lib/cli.js missing or node cannot run it)",
            );
        }
        eprintln!("SKIP: djot CLI not found. Install with: npm install -g @djot/djot");
        libtest_mimic::run(&args, vec![]).exit();
    }

    let test_dir = std::path::Path::new("third_party/djot.js/test");
    let mut trials = Vec::new();

    let entries: Vec<_> = glob::glob(test_dir.join("*.test").to_str().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();

    for entry in &entries {
        let file_name = entry.file_name().unwrap().to_str().unwrap().to_string();
        let content = std::fs::read_to_string(entry).unwrap();
        let cases = parse_test_file(&content);

        for (case_idx, input) in cases.into_iter().enumerate() {
            let name = format!("{}::case_{}", file_name, case_idx);
            let test_file = file_name.clone();
            trials.push(Trial::test(name, move || {
                run_parser_event_test(&test_file, case_idx, &input)
            }));
        }
    }

    if trials.is_empty() {
        eprintln!("No test cases found in {}", test_dir.display());
    }

    // Also test integration .in/.out files
    let int_entries: Vec<_> = glob::glob("tests/*.in")
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    for entry in &int_entries {
        let in_path = entry.as_path();
        let stem = in_path.file_stem().unwrap().to_str().unwrap().to_string();
        let in_content = std::fs::read_to_string(in_path).unwrap();

        let in_stem = stem.clone();
        let in_name = format!("integration::{}::in", stem);
        let in_content_clone = in_content.clone();
        trials.push(Trial::test(in_name, move || {
            run_parser_event_test(&format!("{}.in", in_stem), 0, &in_content_clone)
        }));

        let out_path = in_path.with_extension("out");
        if out_path.exists() {
            let out_content = std::fs::read_to_string(&out_path).unwrap();
            let out_name = format!("integration::{}::out", stem);
            trials.push(Trial::test(out_name, move || {
                run_parser_event_test(&format!("{}.out", stem), 0, &out_content)
            }));
        }
    }

    libtest_mimic::run(&args, trials).exit();
}
