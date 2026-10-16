// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exercise the real stdio transport, capability negotiation and edits. A
//! timeout and kill-on-drop ensure protocol regressions cannot hang the suite.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

struct Server {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
    next_id: u64,
    notifications: Vec<Value>,
}
impl Server {
    fn new(snippets: bool) -> Self {
        Self::with_capabilities(snippets, true)
    }
    fn with_capabilities(snippets: bool, nested: bool) -> Self {
        Self::with_position_encodings(snippets, nested, None)
    }
    fn with_position_encodings(snippets: bool, nested: bool, encodings: Option<&[&str]>) -> Self {
        Self::start(snippets, nested, encodings, None)
    }
    fn start(
        snippets: bool,
        nested: bool,
        encodings: Option<&[&str]>,
        folders: Option<Value>,
    ) -> Self {
        Self::start_with_init(snippets, nested, encodings, folders, json!({}))
    }
    fn start_with_init(
        snippets: bool,
        nested: bool,
        encodings: Option<&[&str]>,
        folders: Option<Value>,
        overrides: Value,
    ) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_djotls"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut size = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length:") {
                        size = Some(n.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; size.unwrap()];
                reader.read_exact(&mut body).unwrap();
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut server = Self {
            child,
            input,
            output,
            next_id: 0,
            notifications: vec![],
        };
        let mut capabilities = json!({"textDocument":{"completion":{"completionItem":{"snippetSupport":snippets}},"documentSymbol":{"hierarchicalDocumentSymbolSupport":nested}}});
        if let Some(encodings) = encodings {
            capabilities["general"] = json!({"positionEncodings":encodings});
        }
        let mut initialization = json!({"processId":null,"rootUri":null,"workspaceFolders":folders,"capabilities":capabilities});
        for (key, value) in overrides.as_object().unwrap() {
            initialization[key] = value.clone();
        }
        let result = server.request("initialize", initialization);
        assert_eq!(result["capabilities"]["workspaceSymbolProvider"], true);
        let triggers = result["capabilities"]["completionProvider"]["triggerCharacters"]
            .as_array()
            .unwrap();
        assert!(triggers.contains(&json!("(")));
        assert!(triggers.contains(&json!("/")));
        assert_eq!(
            result["capabilities"]["workspace"]["workspaceFolders"]["supported"],
            true
        );
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        assert_eq!(result["capabilities"]["textDocumentSync"], 2);
        assert_eq!(
            result["capabilities"]["documentLinkProvider"]["resolveProvider"],
            false
        );
        server.notify("initialized", json!({}));
        server
    }
    fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        write!(self.input, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.input.write_all(&body).unwrap();
        self.input.flush().unwrap();
    }
    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let msg = self.response(method, params);
        assert!(msg.get("error").is_none(), "{msg}");
        msg["result"].clone()
    }
    fn response(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let msg = self
                .output
                .recv_timeout(Duration::from_secs(10))
                .expect("LSP response timed out");
            if msg["id"] == id {
                return msg;
            }
            if msg["method"] == "client/registerCapability" {
                self.send(json!({"jsonrpc":"2.0","id":msg["id"],"result":null}));
            }
            self.notifications.push(msg);
        }
    }
    fn diagnostics(&mut self) -> Value {
        loop {
            if let Some(i) = self
                .notifications
                .iter()
                .position(|msg| msg["method"] == "textDocument/publishDiagnostics")
            {
                return self.notifications.remove(i)["params"].clone();
            }
            let msg = self
                .output
                .recv_timeout(Duration::from_secs(10))
                .expect("Diagnostics timed out");
            self.notifications.push(msg);
        }
    }
    fn document(&mut self, method: &str) -> Value {
        self.request(method, json!({"textDocument":{"uri":"file:///test.dj"}}))
    }
    fn query(&mut self, method: &str, source: &str, offset: usize) -> Value {
        let pos = source_position(source, offset);
        self.at(
            method,
            pos["line"].as_u64().unwrap() as u32,
            pos["character"].as_u64().unwrap() as u32,
        )
    }
    fn refs(&mut self, source: &str, offset: usize, declaration: bool) -> Value {
        self.request("textDocument/references", json!({"textDocument":{"uri":"file:///test.dj"},"position":source_position(source, offset),"context":{"includeDeclaration":declaration}}))
    }
    fn open(&mut self, text: &str) {
        self.notify("textDocument/didOpen", json!({"textDocument":{"uri":"file:///test.dj","languageId":"djot","version":1,"text":text}}));
    }
    fn at(&mut self, method: &str, line: u32, character: u32) -> Value {
        self.request(method, json!({"textDocument":{"uri":"file:///test.dj"},"position":{"line":line,"character":character}}))
    }
    fn shutdown(&mut self) {
        assert!(self.request("shutdown", Value::Null).is_null());
        self.notify("exit", Value::Null);
        for _ in 0..100 {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("LSP failed to exit");
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stdio_navigation_outline_and_formatting() {
    let mut server = Server::new(true);
    server.open("# Intro\n\n## Child\n\n中😀 [x][guide]\n\n[guide]: manual.dj\n");
    let target = server.at("textDocument/definition", 4, 10);
    assert_eq!(target["range"]["start"], json!({"line":6,"character":1}));
    let refs = server.request("textDocument/references", json!({"textDocument":{"uri":"file:///test.dj"},"position":{"line":6,"character":2},"context":{"includeDeclaration":true}}));
    assert_eq!(refs.as_array().unwrap().len(), 2);
    assert_eq!(refs[0]["range"]["start"], json!({"line":4,"character":8}));
    let symbols = server.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":"file:///test.dj"}}),
    );
    assert_eq!(symbols[0]["name"], "Intro");
    assert_eq!(symbols[0]["children"][0]["name"], "Child");
    let edits = server.request("textDocument/formatting", json!({"textDocument":{"uri":"file:///test.dj"},"options":{"tabSize":4,"insertSpaces":true}}));
    assert!(edits.is_array());
    server.shutdown();
}

#[test]
fn incremental_changes_completion_and_stale_versions() {
    let mut server = Server::new(true);
    server.open("[guide]: target\n\n[text][gu");
    let items = server.at("textDocument/completion", 2, 9);
    let guide = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "guide")
        .unwrap();
    assert_eq!(guide["textEdit"]["newText"], "guide]");
    assert_eq!(guide["textEdit"]["range"]["start"]["character"], 7);
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":2},"contentChanges":[{"range":{"start":{"line":2,"character":7},"end":{"line":2,"character":9}},"text":"guide]"}]}));
    let target = server.at("textDocument/definition", 2, 9);
    assert_eq!(target["range"]["start"]["line"], 0);
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":1},"contentChanges":[{"text":"wrong"}]}));
    assert_eq!(server.at("textDocument/definition", 2, 9), target);
    server.shutdown();
}

#[test]
fn completion_snippets_and_plain_text_fallback() {
    for snippets in [true, false] {
        let mut server = Server::new(snippets);
        server.open("[");
        let items = server.at("textDocument/completion", 0, 1);
        let item = &items[0];
        assert_eq!(item["label"], "Inline link");
        assert_eq!(item["insertTextFormat"], if snippets { 2 } else { 1 });
        assert_eq!(
            item["textEdit"]["newText"],
            if snippets {
                "[${1:text}](${2:destination})"
            } else {
                "[text](destination)"
            }
        );
        server.shutdown();
    }
}

#[test]
fn diagnostics_are_versioned_and_cleared_on_close() {
    let mut server = Server::new(true);
    server.open("[x][missing]");
    let params = server.diagnostics();
    assert_eq!(params["version"], 1);
    assert_eq!(
        params["diagnostics"][0]["range"],
        json!({"start":{"line":0,"character":4},"end":{"line":0,"character":11}})
    );
    assert_eq!(
        params["diagnostics"][0]["message"],
        "Unresolved reference: missing"
    );
    server.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":"file:///test.dj"}}),
    );
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    server.shutdown();
}

#[test]
fn footnote_anchor_and_link_conversion_completions() {
    let mut server = Server::new(true);
    for (version, body, line, character, label, new_text) in [
        (2, "[^note]: text\n\n[^no", 2, 4, "note", "note]"),
        (
            3,
            "{#custom}\n# Title\n\n[x](#cu",
            3,
            7,
            "custom",
            "custom)",
        ),
        (4, "[guide]: url\n\n[x][]()", 2, 4, "guide", "guide]"),
    ] {
        if version == 2 {
            server.open("");
        }
        server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":version},"contentChanges":[{"text":body}]}));
        let items = server.at("textDocument/completion", line, character);
        let item = items
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == label)
            .unwrap();
        assert_eq!(item["textEdit"]["newText"], new_text);
        if version == 4 {
            assert_eq!(item["textEdit"]["range"]["end"]["character"], 7);
        }
    }
    server.shutdown();
}

#[test]
fn document_links_resolve_files_and_reference_targets() {
    let mut server = Server::new(true);
    let uri = "file:///workspace/docs/main.dj";
    let source = "[direct](../manual.dj#Intro) [unicode](指南.dj) [encoded](a%20b.dj?q=1#part) [absolute](/shared/file.dj) [web](https://example.com/a) [ref][guide] ![image](img.png) [local](#Intro) [missing][unknown] ` [code](fake.dj) `\n\n[guide]: other.dj\n";
    server.notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri,"languageId":"djot","version":1,"text":source}}),
    );
    let result = server.request(
        "textDocument/documentLink",
        json!({"textDocument":{"uri":uri}}),
    );
    let links = result.as_array().unwrap();
    let targets: Vec<_> = links
        .iter()
        .map(|l| l["target"].as_str().unwrap())
        .collect();
    assert_eq!(
        targets,
        [
            "file:///workspace/manual.dj#Intro",
            "file:///workspace/docs/%E6%8C%87%E5%8D%97.dj",
            "file:///workspace/docs/a%20b.dj?q=1#part",
            "file:///shared/file.dj",
            "https://example.com/a",
            "file:///workspace/docs/other.dj",
            "file:///workspace/docs/img.png",
            "file:///workspace/docs/other.dj",
        ]
    );
    assert_eq!(
        links[0]["range"],
        json!({"start":{"line":0,"character":0},"end":{"line":0,"character":28}})
    );
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"[ref][guide]\n\n[guide]: updated.dj"}]}));
    let result = server.request(
        "textDocument/documentLink",
        json!({"textDocument":{"uri":uri}}),
    );
    assert_eq!(result[0]["target"], "file:///workspace/docs/updated.dj");
    assert_eq!(result.as_array().unwrap().len(), 2);
    server.shutdown();
}

#[test]
fn document_links_handle_spaces_file_uris_and_non_file_documents() {
    let mut server = Server::new(false);
    server.open("[space](my file.dj) [file](file:///tmp/other.dj)\n");
    let result = server.request(
        "textDocument/documentLink",
        json!({"textDocument":{"uri":"file:///test.dj"}}),
    );
    assert_eq!(result[0]["target"], "file:///my%20file.dj");
    assert_eq!(result[1]["target"], "file:///tmp/other.dj");
    server.notify("textDocument/didOpen", json!({"textDocument":{"uri":"untitled:buffer","languageId":"djot","version":1,"text":"[relative](a.dj) [web](https://example.com/)"}}));
    let result = server.request(
        "textDocument/documentLink",
        json!({"textDocument":{"uri":"untitled:buffer"}}),
    );
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["target"], "https://example.com/");
    server.shutdown();
}

// These helpers deliberately use only source text and JSON, never the analysis
// model or its coordinate conversion. Expected positions are independent of it.
fn source_position(source: &str, offset: usize) -> Value {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count();
    let tail = prefix.rsplit('\n').next().unwrap();
    json!({"line":line,"character":tail.encode_utf16().count()})
}

fn source_offset(source: &str, pos: &Value) -> usize {
    let line = pos["line"].as_u64().expect("line") as usize;
    let column = pos["character"].as_u64().expect("character") as usize;
    let lines: Vec<_> = source.split('\n').collect();
    let text = lines
        .get(line)
        .expect("line outside source")
        .trim_end_matches('\r');
    let mut units = 0;
    let mut bytes = 0;
    for ch in text.chars() {
        if units == column {
            break;
        }
        units += ch.len_utf16();
        bytes += ch.len_utf8();
    }
    assert_eq!(units, column, "invalid UTF-16 position: {pos}");
    lines[..line].iter().map(|l| l.len() + 1).sum::<usize>() + bytes
}

fn range_text<'a>(source: &'a str, range: &Value) -> &'a str {
    let start = source_offset(source, &range["start"]);
    let end = source_offset(source, &range["end"]);
    assert!(start <= end, "reversed range: {range}");
    &source[start..end]
}

fn apply_edit(source: &str, edit: &Value) -> String {
    let mut result = source.to_owned();
    let start = source_offset(source, &edit["range"]["start"]);
    let end = source_offset(source, &edit["range"]["end"]);
    result.replace_range(start..end, edit["newText"].as_str().unwrap());
    result
}

#[test]
fn stdio_definition_and_reverse_references_cover_all_label_kinds() {
    let mut server = Server::new(true);
    let source =
        "# Intro\n\n[x][guide] [Intro][] [x](#Intro) [^a]\n\n[guide]: manual.dj\n[^a]: note";
    server.open(source);
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    for (needle, label, definition) in [
        ("[guide]", "guide", source.find("[guide]:").unwrap() + 1),
        ("[Intro][]", "Intro", 2),
        ("#Intro)", "Intro", 2),
        ("[^a]", "a", source.find("[^a]:").unwrap() + 2),
    ] {
        let at = source.find(needle).unwrap() + if needle.starts_with("[^") { 2 } else { 1 };
        let result = server.query("textDocument/definition", source, at);
        assert_eq!(result["uri"], "file:///test.dj");
        assert_eq!(
            result["range"]["start"],
            source_position(source, definition)
        );
        assert_eq!(range_text(source, &result["range"]), label);
    }
    let refs = server.refs(source, source.find("[guide]:").unwrap() + 1, true);
    assert_eq!(refs.as_array().unwrap().len(), 2);
    for loc in refs.as_array().unwrap() {
        assert_eq!(range_text(source, &loc["range"]), "guide");
    }
    let refs = server.refs(source, 2, false);
    assert_eq!(refs.as_array().unwrap().len(), 2);
    for loc in refs.as_array().unwrap() {
        assert_eq!(range_text(source, &loc["range"]), "Intro");
    }
    server.shutdown();
}

#[test]
fn stdio_duplicate_definitions_override_heading_and_report_precise_warning() {
    let mut server = Server::new(true);
    let source = "# Same\n\n[Same][]\n\n[Same]: first\n[Same]: last\n";
    server.open(source);
    let diagnostics = server.diagnostics();
    assert_eq!(diagnostics["diagnostics"].as_array().unwrap().len(), 1);
    let warning = &diagnostics["diagnostics"][0];
    assert_eq!(warning["severity"], 2);
    assert_eq!(warning["message"], "Duplicate definition: Same");
    assert_eq!(range_text(source, &warning["range"]), "Same");
    assert_eq!(
        warning["range"]["start"],
        source_position(source, source.rfind("[Same]").unwrap() + 1)
    );
    let target = server.query(
        "textDocument/definition",
        source,
        source.find("[Same][]").unwrap() + 1,
    );
    assert_eq!(target["range"], warning["range"]);
    let links = server.document("textDocument/documentLink");
    assert_eq!(links.as_array().unwrap().len(), 3);
    assert_eq!(links[0]["target"], "file:///last");
    assert_eq!(links[1]["target"], "file:///first");
    assert_eq!(links[2]["target"], "file:///last");
    server.shutdown();
}

#[test]
fn stdio_heading_ids_hierarchy_and_flat_outline() {
    for nested in [true, false] {
        let mut server = Server::with_capabilities(true, nested);
        let source = "{#custom}\n# Title\n\n## Child\n\n# Title\n\n# Title\n\n[x](#custom) [x](#Child) [x](#Title) [x](#Title-1)\n";
        server.open(source);
        assert_eq!(server.diagnostics()["diagnostics"], json!([]));
        let symbols = server.document("textDocument/documentSymbol");
        if nested {
            assert_eq!(symbols.as_array().unwrap().len(), 3);
            assert_eq!(symbols[0]["children"][0]["name"], "Child");
            assert_eq!(symbols[1]["name"], "Title");
            assert!(symbols[1].get("children").is_none());
        } else {
            assert_eq!(symbols.as_array().unwrap().len(), 4);
            assert_eq!(symbols[1]["name"], "Child");
            assert_eq!(symbols[1]["containerName"], "Title");
            assert!(symbols[2].get("containerName").is_none());
        }
        for (id, line) in [("custom", 0), ("Child", 3), ("Title", 5), ("Title-1", 7)] {
            let at = source.find(&format!("(#{id})")).unwrap() + 2;
            let target = server.query("textDocument/definition", source, at);
            assert_eq!(target["range"]["start"]["line"], line, "{id}");
            assert_eq!(
                range_text(source, &target["range"]),
                if id == "custom" {
                    "custom"
                } else if id == "Child" {
                    "Child"
                } else {
                    "Title"
                }
            );
        }
        server.shutdown();
    }
}

#[test]
fn stdio_heading_labels_exclude_destinations_and_footnotes() {
    let mut server = Server::new(true);
    let source = "# A [link](https://example.com)[^n] :symbol:\n\n[A link][]\n\n[^n]: note\n";
    server.open(source);
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    let symbols = server.document("textDocument/documentSymbol");
    assert_eq!(symbols[0]["name"], "A link");
    let target = server.query(
        "textDocument/definition",
        source,
        source.find("[A link][]").unwrap() + 1,
    );
    assert_eq!(target["range"], symbols[0]["selectionRange"]);
    server.shutdown();
}

#[test]
fn stdio_attribute_ownership_controls_anchor_navigation() {
    let mut server = Server::new(true);
    for source in ["{#bad} text\n", "word {#bad}\n", "{#bad}\n\ntext\n"] {
        let source = format!("{source}\n[x](#bad)\n");
        server.open(&source);
        let diagnostics = server.diagnostics();
        assert_eq!(
            diagnostics["diagnostics"][0]["message"],
            "Unresolved reference: bad"
        );
        let result = server.query(
            "textDocument/definition",
            &source,
            source.rfind("bad").unwrap(),
        );
        assert!(result.is_null(), "{source}: {result}");
    }
    let source = "word{#good}\n\n[x](#good)\n";
    server.open(source);
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    let target = server.query(
        "textDocument/definition",
        source,
        source.rfind("good").unwrap(),
    );
    assert_eq!(range_text(source, &target["range"]), "good");
    assert_eq!(
        target["range"]["start"],
        source_position(source, source.find("good").unwrap())
    );
    server.shutdown();
}

#[test]
fn stdio_completion_context_and_applied_replacement_edits() {
    let mut server = Server::new(false);
    for (prefix, input, label, expected) in [
        ("[guide]: url\n\n", "[text][gu", "guide", "[text][guide]"),
        ("[^note]: text\n\n", "[^no", "note", "[^note]"),
        ("# Intro\n\n", "[text](#In", "Intro", "[text](#Intro)"),
        ("", "[", "Inline link", "[text](destination)"),
        ("[guide]: url\n\n", "[text][)", "guide", "[text][guide]"),
        ("[guide]: url\n\n", "[text][]()", "guide", "[text][guide]"),
        (
            "[guide]: url\n\n",
            "[text][guide]",
            "guide",
            "[text][guide]",
        ),
    ] {
        let source = format!("{prefix}{input}");
        server.open(&source);
        let at = if let Some(i) = input.find("][") {
            prefix.len()
                + i
                + 2
                + if input == "[text][guide]" || input.ends_with("gu") {
                    2
                } else {
                    0
                }
        } else {
            source.len()
        };
        let result = server.query("textDocument/completion", &source, at);
        let item = result
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == label)
            .unwrap();
        if input == "[text][guide]" {
            assert_eq!(range_text(&source, &item["textEdit"]["range"]), "guide]");
        }
        assert_eq!(
            apply_edit(&source, &item["textEdit"]),
            format!("{prefix}{expected}")
        );
    }
    for source in [
        "\\[",
        "`[text][gu`",
        "```\n[text][gu",
        "$`[text][gu`",
        "word{key=\"[text][gu\"}",
    ] {
        server.open(source);
        let at = source.find("gu").map_or(source.len(), |i| i + 2);
        assert!(
            server
                .query("textDocument/completion", source, at)
                .is_null(),
            "{source}"
        );
    }
    server.shutdown();
}

#[test]
fn stdio_unicode_crlf_positions_and_invalid_changes() {
    let mut server = Server::new(true);
    let source = "中😀x\r\n[中][标签]\r\n\r\n[标签]: 地址";
    server.open(source);
    let expected = json!({"uri":"file:///test.dj","range":{"start":{"line":3,"character":1},"end":{"line":3,"character":3}}});
    assert_eq!(server.at("textDocument/definition", 1, 5), expected);
    // UTF-16 character 2 bisects the emoji. Other coordinates exceed line/EOF.
    for (line, character) in [(0, 2), (0, 5), (4, 0)] {
        let response = server.response("textDocument/definition", json!({"textDocument":{"uri":"file:///test.dj"},"position":{"line":line,"character":character}}));
        assert_eq!(response["error"]["code"], -32602);
        server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":2},"contentChanges":[{"range":{"start":{"line":line,"character":character},"end":{"line":line,"character":character}},"text":"broken"}]}));
        assert_eq!(server.at("textDocument/definition", 1, 5), expected);
    }
    // A valid edit after all rejected updates still applies at the same version.
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":2},"contentChanges":[{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":4}},"text":"[x][标签]"}]}));
    assert_eq!(server.at("textDocument/definition", 0, 8), expected);
    let refs = server.refs("中😀[x][标签]\r\n[中][标签]\r\n\r\n[标签]: 地址", 11, false);
    assert_eq!(refs.as_array().unwrap().len(), 2);
    server.shutdown();
}

#[test]
fn stdio_document_link_ranges_and_duplicate_reference_precedence() {
    let mut server = Server::new(true);
    let source = "[x](a.dj) [r][guide] ![img](p.png) ` [code](bad.dj) `\n\n[guide]: first.dj\n[guide]: last.dj\n";
    server.open(source);
    let links = server.document("textDocument/documentLink");
    let actual: Vec<_> = links
        .as_array()
        .unwrap()
        .iter()
        .map(|link| {
            (
                range_text(source, &link["range"]),
                link["target"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        actual,
        [
            ("[x](a.dj)", "file:///a.dj"),
            ("[r][guide]", "file:///last.dj"),
            ("[img](p.png)", "file:///p.png"),
            ("first.dj", "file:///first.dj"),
            ("last.dj", "file:///last.dj")
        ]
    );
    server.shutdown();
}

// Recursively validate every protocol range, including nested symbols, rather
// than relying on analysis internals to enumerate the objects being tested.
fn validate_ranges(source: &str, value: &Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("start") && object.contains_key("end") {
                range_text(source, value);
            } else {
                for child in object.values() {
                    validate_ranges(source, child);
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_ranges(source, child);
            }
        }
        _ => {}
    }
}

#[test]
fn stdio_corpus_and_missing_final_newline_have_valid_source_ranges() {
    let mut server = Server::new(true);
    let mut sources: Vec<(String, String)> = [
        "",
        "中😀",
        "# 中😀",
        "[^中]: 脚注\n\n[^中]",
        "[中][标签]\n\n[标签]: 地址",
    ]
    .into_iter()
    .map(|s| (format!("{s:?}"), s.into()))
    .collect();
    for path in glob::glob("tests/*.in")
        .unwrap()
        .chain(glob::glob("tests/*.out").unwrap())
    {
        let path = path.unwrap();
        sources.push((
            path.display().to_string(),
            std::fs::read_to_string(path).unwrap(),
        ));
    }
    for (name, source) in sources {
        server.open(&source);
        let diagnostics = server.diagnostics();
        validate_ranges(&source, &diagnostics);
        for method in ["textDocument/documentSymbol", "textDocument/documentLink"] {
            let result = server.document(method);
            validate_ranges(&source, &result);
        }
        // Probe all source character boundaries. This reaches definition names,
        // uses, attached IDs and heading labels without a second parser oracle.
        for offset in source
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(source.len()))
        {
            if source[..offset].ends_with('\r') {
                continue;
            }
            let result = server.query("textDocument/definition", &source, offset);
            validate_ranges(&source, &result);
            if !result.is_null() {
                validate_ranges(&source, &server.refs(&source, offset, true));
            }
        }
        assert_eq!(diagnostics["version"], 1, "{name}");
    }
    server.shutdown();
}

#[test]
fn stdio_formatting_applies_expected_text_and_reports_noop() {
    let mut server = Server::new(true);
    let source = include_str!("heading-same-line.in");
    let expected = include_str!("heading-same-line.out");
    server.open(source);
    let options = json!({"textDocument":{"uri":"file:///test.dj"},"options":{"tabSize":4,"insertSpaces":true}});
    let edits = server.request("textDocument/formatting", options.clone());
    assert_eq!(edits.as_array().unwrap().len(), 1);
    assert_eq!(range_text(source, &edits[0]["range"]), source);
    assert_eq!(apply_edit(source, &edits[0]), expected);
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":2},"contentChanges":[{"text":expected}]}));
    assert_eq!(
        server.request("textDocument/formatting", options),
        json!([])
    );
    server.shutdown();
}

#[test]
fn stdio_position_encoding_negotiation_and_unicode_round_trip() {
    // LSP requires UTF-16 support even if it is absent from the client's list.
    // This server always selects UTF-16, including for legacy clients that omit
    // general.positionEncodings. Verify actual requests/edits, not just the label.
    for encodings in [
        None,
        Some(&[][..]),
        Some(&["utf-16"][..]),
        Some(&["utf-8", "utf-16"][..]),
        Some(&["utf-32", "utf-8"][..]),
        Some(&["utf-8"][..]),
    ] {
        let mut server = Server::with_position_encodings(true, true, encodings);
        server.open("中😀 [x][标😀签]\r\n\r\n[标😀签]: guide.dj");
        assert_eq!(server.diagnostics()["diagnostics"], json!([]));
        let expected = json!({"uri":"file:///test.dj","range":{"start":{"line":2,"character":1},"end":{"line":2,"character":5}}});
        assert_eq!(
            server.at("textDocument/definition", 0, 9),
            expected,
            "{encodings:?}"
        );
        let refs = server.request("textDocument/references", json!({"textDocument":{"uri":"file:///test.dj"},"position":{"line":2,"character":1},"context":{"includeDeclaration":true}}));
        assert_eq!(
            refs,
            json!([
                {"uri":"file:///test.dj","range":{"start":{"line":0,"character":8},"end":{"line":0,"character":12}}},
                expected
            ])
        );
        server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":2},"contentChanges":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}},"text":"😀"}]}));
        assert_eq!(server.at("textDocument/definition", 0, 7), expected);
        let links = server.document("textDocument/documentLink");
        assert_eq!(
            links[0]["range"],
            json!({"start":{"line":0,"character":2},"end":{"line":0,"character":11}})
        );
        assert_eq!(links[0]["target"], "file:///guide.dj");
        server.shutdown();
    }
}

// Fixtures are real Git repositories and all assertions go through the binary's
// stdio interface. Nothing inspects the workspace or analysis implementation.
struct Repository(std::path::PathBuf);
impl Repository {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "djotls-workspace-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let repo = Self(path);
        repo.git(&["init", "--quiet"]);
        repo
    }
    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn write(&self, name: &str, source: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    fn uri(&self, name: &str) -> String {
        url::Url::from_file_path(self.0.join(name))
            .unwrap()
            .to_string()
    }
    fn folder(&self, name: &str) -> Value {
        json!({"uri": self.uri(name), "name":"fixture"})
    }
}
impl Drop for Repository {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn workspace_symbols(server: &mut Server, query: &str) -> Vec<Value> {
    server
        .request("workspace/symbol", json!({"query":query}))
        .as_array()
        .unwrap()
        .clone()
}
fn symbol_names(server: &mut Server) -> Vec<String> {
    let mut names: Vec<_> = workspace_symbols(server, "")
        .iter()
        .map(|v| {
            assert!(v.get("containerName").is_none());
            let name = v["name"].as_str().unwrap();
            let (title, target) = name.rsplit_once(" (").expect("title (path#anchor)");
            assert!(target.contains('#') && target.ends_with(')'));
            title.to_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn stdio_workspace_git_discovery_and_unopened_headings() {
    let repo = Repository::new();
    repo.write(".gitignore", "ignored/\ntracked.dj\n");
    repo.write("tracked.dj", "# Tracked\n");
    repo.git(&["add", "-f", "tracked.dj"]);
    // Index the working tree rather than the staged content.
    repo.write("tracked.dj", "# Working tree\n");
    repo.write("docs/中 文.dj", "# 中😀\r\n\r\n## Child\r\n");
    repo.write("ignored/no.dj", "# Ignored\n");
    repo.write("wrong.djot", "# Wrong extension\n");
    repo.write("upper.DJ", "# Upper extension\n");
    repo.write("vendor/inner.dj", "# Submodule\n");
    repo.git(&["-C", "vendor", "init", "--quiet"]);
    repo.git(&["-C", "vendor", "add", "inner.dj"]);
    repo.git(&[
        "-C",
        "vendor",
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ]);
    repo.git(&["add", "vendor"]);
    repo.write("gone.dj", "# Deleted\n");
    repo.git(&["add", "gone.dj"]);
    std::fs::remove_file(repo.0.join("gone.dj")).unwrap();
    // Starting below the root still indexes the entire repository. Repeating a
    // repository through overlapping workspace folders must not duplicate it.
    let mut server = Server::start(
        true,
        true,
        None,
        Some(json!([repo.folder("docs"), repo.folder("")])),
    );
    assert_eq!(symbol_names(&mut server), ["Child", "Working tree", "中😀"]);
    let symbols = workspace_symbols(&mut server, "CHILD");
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0]["location"]["uri"], repo.uri("docs/中 文.dj"));
    assert!(symbols[0].get("containerName").is_none());
    assert_eq!(symbols[0]["name"], "Child (docs/中 文.dj#Child)");
    let source = "# 中😀\r\n\r\n## Child\r\n";
    assert_eq!(
        range_text(source, &symbols[0]["location"]["range"]),
        "Child"
    );
    server.shutdown();
}

#[test]
fn stdio_workspace_open_buffers_override_disk_until_close() {
    let repo = Repository::new();
    repo.write("a.dj", "# Disk\n");
    let uri = repo.uri("a.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    assert_eq!(symbol_names(&mut server), ["Disk"]);
    server.notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri,"languageId":"djot","version":1,"text":"# Unsaved\n"}}),
    );
    assert_eq!(symbol_names(&mut server), ["Unsaved"]);
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":9}},"text":"Edited"}]}));
    assert_eq!(symbol_names(&mut server), ["Edited"]);
    repo.write("a.dj", "# External change\n");
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":uri,"type":2}]}),
    );
    assert_eq!(symbol_names(&mut server), ["Edited"]);
    server.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
    assert_eq!(symbol_names(&mut server), ["External change"]);
    std::fs::remove_file(repo.0.join("a.dj")).unwrap();
    assert!(symbol_names(&mut server).is_empty());
    repo.write("new.dj", "# New\n");
    assert_eq!(symbol_names(&mut server), ["New"]);
    repo.write(".gitignore", "new.dj\n");
    assert!(symbol_names(&mut server).is_empty());
    server.shutdown();
}

#[test]
fn stdio_workspace_folder_changes_and_standalone_buffers() {
    let first = Repository::new();
    let second = Repository::new();
    first.write("a.dj", "# First\n");
    second.write("b.dj", "# Second\n");
    let mut server = Server::start(true, true, None, Some(json!([first.folder("")])));
    assert_eq!(symbol_names(&mut server), ["First"]);
    server.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event":{"removed":[first.folder("")],"added":[second.folder("")]}}),
    );
    assert_eq!(symbol_names(&mut server), ["Second"]);
    server.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event":{"removed":[second.folder("")],"added":[]}}),
    );
    assert!(symbol_names(&mut server).is_empty());
    server.open("# Standalone\n");
    assert_eq!(symbol_names(&mut server), ["Standalone"]);
    server.shutdown();
}

#[test]
fn stdio_workspace_root_uri_watcher_registration_and_linked_worktree() {
    let repo = Repository::new();
    repo.write("root.dj", "# Root\n");
    repo.git(&["add", "root.dj"]);
    repo.git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ]);
    let worktree = Repository::new();
    std::fs::remove_dir_all(worktree.0.join(".git")).unwrap();
    repo.git(&["worktree", "add", "--detach", worktree.0.to_str().unwrap()]);
    worktree.write("root.dj", "# Worktree\n");
    let mut server = Server::start_with_init(
        true,
        true,
        None,
        None,
        json!({
            "rootUri":worktree.uri(""),
            "capabilities":{"workspace":{"didChangeWatchedFiles":{"dynamicRegistration":true}}}
        }),
    );
    assert_eq!(symbol_names(&mut server), ["Worktree"]);
    let registration = server
        .notifications
        .iter()
        .find(|v| v["method"] == "client/registerCapability")
        .expect("watcher registration");
    assert_eq!(
        registration["params"]["registrations"][0]["registerOptions"]["watchers"][0]["globPattern"],
        "**/*.dj"
    );
    server.shutdown();
}

#[test]
fn stdio_workspace_duplicate_titles_keep_distinct_file_locations() {
    let repo = Repository::new();
    repo.write("a.dj", "# Intro\n\n# Intro\n");
    repo.write("b.dj", "# Intro\n");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    let symbols = workspace_symbols(&mut server, "Intro");
    assert_eq!(symbols.len(), 3);
    for (symbol, file, line, anchor) in [
        (&symbols[0], "a.dj", 0, "Intro"),
        (&symbols[1], "a.dj", 2, "Intro-1"),
        (&symbols[2], "b.dj", 0, "Intro"),
    ] {
        assert_eq!(symbol["name"], format!("Intro ({file}#{anchor})"));
        assert!(symbol.get("containerName").is_none());
        assert_eq!(symbol["location"]["uri"], repo.uri(file));
        assert_eq!(
            symbol["location"]["range"]["start"],
            json!({"line":line,"character":2})
        );
    }
    server.shutdown();
}

#[test]
fn stdio_workspace_symbol_names_include_explicit_and_generated_anchors() {
    let repo = Repository::new();
    let source = "{#custom}\n# Title\n\n## 中 文\n";
    repo.write("names.dj", source);
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    let symbols = workspace_symbols(&mut server, "");
    assert_eq!(symbols.len(), 2);
    for (symbol, title, anchor) in [
        (&symbols[0], "Title", "custom"),
        (&symbols[1], "中 文", "中-文"),
    ] {
        assert_eq!(symbol["name"], format!("{title} (names.dj#{anchor})"));
        assert!(symbol.get("containerName").is_none());
        assert_eq!(symbol["location"]["uri"], repo.uri("names.dj"));
        assert_eq!(range_text(source, &symbol["location"]["range"]), title);
    }
    assert_eq!(
        workspace_symbols(&mut server, "#custom"),
        vec![symbols[0].clone()]
    );
    assert_eq!(workspace_symbols(&mut server, "names.dj").len(), 2);
    server.shutdown();
}

fn open_uri(server: &mut Server, uri: &str, source: &str) {
    server.notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri,"languageId":"djot","version":1,"text":source}}),
    );
}
fn complete_uri(server: &mut Server, uri: &str, source: &str, at: usize) -> Vec<Value> {
    let result = server.request(
        "textDocument/completion",
        json!({"textDocument":{"uri":uri},"position":source_position(source, at)}),
    );
    assert_eq!(result["isIncomplete"], true);
    result["items"].as_array().unwrap().clone()
}

#[test]
fn stdio_workspace_heading_completion_prefers_local_and_inserts_relative_links() {
    let repo = Repository::new();
    repo.write("guide.dj", "{#custom}\n# Guide\n");
    repo.write("docs/sibling.dj", "# ARemote\n\n# ARemote\n\n# ZLocal\n");
    let source = "# ZLocal\n\n[x](";
    repo.write("docs/current.dj", source);
    let uri = repo.uri("docs/current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let items = complete_uri(&mut server, &uri, source, source.len());
    assert_eq!(items.len(), 5);
    assert_eq!(items[0]["label"], "ZLocal (#ZLocal)");
    assert_eq!(items[0]["detail"], "#ZLocal");
    assert_eq!(items[0]["filterText"], "ZLocal");
    for item in &items[1..] {
        assert!(items[0]["sortText"].as_str().unwrap() < item["sortText"].as_str().unwrap());
    }
    for destination in [
        "#ZLocal",
        "sibling.dj#ARemote",
        "sibling.dj#ARemote-1",
        "sibling.dj#ZLocal",
        "../guide.dj#custom",
    ] {
        let item = items
            .iter()
            .find(|v| v["detail"] == destination)
            .expect(destination);
        assert_eq!(
            apply_edit(source, &item["textEdit"]),
            format!("{source}{destination})")
        );
        assert_eq!(item["insertTextFormat"], 1);
    }
    // Complete a partly typed target and replace its existing closing paren.
    let source = "# ZLocal\n\n[x](../guide.dj#cu) after";
    open_uri(&mut server, &uri, source);
    let items = complete_uri(&mut server, &uri, source, source.find("cu)").unwrap() + 2);
    assert_eq!(items.len(), 1);
    assert_eq!(
        apply_edit(source, &items[0]["textEdit"]),
        "# ZLocal\n\n[x](../guide.dj#custom) after"
    );
    // The same basename and destination prefix are relative to the current
    // document, rather than the repository or LSP workspace folder.
    assert_eq!(items[0]["detail"], "../guide.dj#custom");
    assert_eq!(items[0]["filterText"], "../guide.dj#custom");
    let source = "# ZLocal\n\n[x](ARem";
    open_uri(&mut server, &uri, source);
    let items = complete_uri(&mut server, &uri, source, source.len());
    assert_eq!(items.len(), 2);
    for item in items {
        assert_eq!(item["filterText"], "ARemote");
        assert!(item["textEdit"]["newText"]
            .as_str()
            .unwrap()
            .starts_with("sibling.dj#ARemote"));
    }
    server.shutdown();
}

#[test]
fn stdio_workspace_heading_completion_tracks_unsaved_targets_and_disk_changes() {
    let repo = Repository::new();
    repo.write("other.dj", "# Disk\n");
    let source = "[x](";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let target = repo.uri("other.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(
        complete_uri(&mut server, &uri, source, source.len())[0]["detail"],
        "other.dj#Disk"
    );
    open_uri(&mut server, &target, "{#edited}\n# Unsaved\n");
    let items = complete_uri(&mut server, &uri, source, source.len());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "Unsaved (other.dj#edited)");
    server.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":target}}),
    );
    assert_eq!(
        complete_uri(&mut server, &uri, source, source.len())[0]["detail"],
        "other.dj#Disk"
    );
    std::fs::remove_file(repo.0.join("other.dj")).unwrap();
    assert!(complete_uri(&mut server, &uri, source, source.len()).is_empty());
    repo.write("new.dj", "# New\n");
    assert_eq!(
        complete_uri(&mut server, &uri, source, source.len())[0]["detail"],
        "new.dj#New"
    );
    server.shutdown();
}

#[test]
fn stdio_workspace_heading_completion_encodes_link_destinations_and_skips_code() {
    let repo = Repository::new();
    repo.write("docs/my file(1).dj", "# 中 文\n");
    let source = "中😀 [x]()";
    repo.write("docs/current.dj", source);
    let uri = repo.uri("docs/current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let items = complete_uri(&mut server, &uri, source, source.len() - 1);
    assert_eq!(items.len(), 1);
    let destination = "my%20file%281%29.dj#%E4%B8%AD-%E6%96%87";
    assert_eq!(items[0]["detail"], destination);
    let completed = apply_edit(source, &items[0]["textEdit"]);
    assert_eq!(completed, format!("中😀 [x]({destination})"));
    open_uri(&mut server, &uri, &completed);
    let links = server.request(
        "textDocument/documentLink",
        json!({"textDocument":{"uri":uri}}),
    );
    assert_eq!(links.as_array().unwrap().len(), 1);
    let target = url::Url::parse(links[0]["target"].as_str().unwrap()).unwrap();
    assert_eq!(
        target.to_file_path().unwrap(),
        repo.0.join("docs/my file(1).dj")
    );
    assert_eq!(target.fragment().unwrap(), "%E4%B8%AD-%E6%96%87");
    for (source, at) in [
        ("`[x](`", 5),
        ("```\n[x](", 8),
        ("\\[x](", 5),
        ("word{key=\"[x](\"}", 14),
        ("[x](done) trailing", 18),
    ] {
        open_uri(&mut server, &uri, source);
        let response = server.request(
            "textDocument/completion",
            json!({"textDocument":{"uri":uri},"position":source_position(source, at)}),
        );
        assert!(response.is_null(), "{source}: {response}");
    }
    server.shutdown();
}

#[test]
fn stdio_completed_local_encoded_anchors_resolve_without_diagnostics() {
    let repo = Repository::new();
    let source = "# 中 文\n\n[x](";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    let items = complete_uri(&mut server, &uri, source, source.len());
    assert_eq!(items.len(), 1);
    let destination = "#%E4%B8%AD-%E6%96%87";
    assert_eq!(items[0]["detail"], destination);
    let completed = apply_edit(source, &items[0]["textEdit"]);
    open_uri(&mut server, &uri, &completed);
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    let at = completed.rfind(destination).unwrap() + 1;
    let target = server.request(
        "textDocument/definition",
        json!({"textDocument":{"uri":uri},"position":source_position(&completed, at)}),
    );
    assert_eq!(target["uri"], uri);
    assert_eq!(range_text(&completed, &target["range"]), "中 文");
    server.shutdown();
}
