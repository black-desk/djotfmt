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
        for operation in ["willRename", "didRename"] {
            let supported =
                overrides["capabilities"]["workspace"]["fileOperations"][operation] == true;
            let registration = &result["capabilities"]["workspace"]["fileOperations"][operation];
            assert_eq!(registration.is_object(), supported);
            if supported {
                assert_eq!(registration["filters"][0]["scheme"], "file");
                assert_eq!(registration["filters"][0]["pattern"]["glob"], "**/*");
            }
        }
        assert_eq!(
            result["capabilities"]["renameProvider"]["prepareProvider"],
            true
        );
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
        assert_eq!(result["capabilities"]["textDocumentSync"]["change"], 2);
        assert_eq!(result["capabilities"]["textDocumentSync"]["save"], true);
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
    fn diagnostics_for(&mut self, uri: &str) -> Value {
        loop {
            let params = self.diagnostics();
            if params["uri"] == uri {
                return params;
            }
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
    assert_eq!(params["diagnostics"][0]["severity"], 1);
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
        "# Intro\n\n[x][guide] [Intro][] [x](#Intro) [^a]\n\n[guide]: https://example.com/manual\n[^a]: note";
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
    let source = "# Same\n\n[Same][]\n\n[Same]: https://example.com/first\n[Same]: https://example.com/last\n";
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
    assert_eq!(links[0]["target"], "https://example.com/last");
    assert_eq!(links[1]["target"], "https://example.com/first");
    assert_eq!(links[2]["target"], "https://example.com/last");
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
        server.open("中😀 [x][标😀签]\r\n\r\n[标😀签]: https://example.com/guide");
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
        assert_eq!(links[0]["target"], "https://example.com/guide");
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

fn definition_uri(server: &mut Server, uri: &str, source: &str, at: usize) -> Value {
    server.request(
        "textDocument/definition",
        json!({"textDocument":{"uri":uri},"position":source_position(source, at)}),
    )
}

#[test]
fn stdio_cross_file_definitions_resolve_unopened_headings_and_explicit_ids() {
    let repo = Repository::new();
    let target = "# Intro\r\n\r\n# Intro\r\n\r\n{#custom}\r\n## 中😀\r\n\r\nword{#inline}\r\n";
    repo.write("guide.dj", target);
    let source = "中😀 [auto](../guide.dj#Intro-1) [explicit](../guide.dj#custom) [inline](../guide.dj#inline) [file](../guide.dj?view=source)\n";
    repo.write("docs/current.dj", source);
    let uri = repo.uri("docs/current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    for (needle, expected, line) in [
        ("Intro-1)", "Intro", 2),
        ("custom)", "custom", 4),
        ("inline)", "inline", 7),
    ] {
        let location = definition_uri(&mut server, &uri, source, source.find(needle).unwrap());
        assert_eq!(location["uri"], repo.uri("guide.dj"));
        assert_eq!(location["range"]["start"]["line"], line);
        assert_eq!(range_text(target, &location["range"]), expected);
    }
    let location = definition_uri(&mut server, &uri, source, source.find("?view").unwrap());
    assert_eq!(location["uri"], repo.uri("guide.dj"));
    assert_eq!(
        location["range"],
        json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}})
    );
    server.shutdown();
}

#[test]
fn stdio_cross_file_encoded_paths_and_fragments_use_actual_target_locations() {
    let repo = Repository::new();
    let target = "# 中 文\n";
    repo.write("docs/中 file.dj", target);
    let source = "😀 [x](docs/%E4%B8%AD%20file.dj?x=1#%E4%B8%AD-%E6%96%87)\n";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let location = definition_uri(&mut server, &uri, source, source.find("#%").unwrap() + 1);
    assert_eq!(location["uri"], repo.uri("docs/中 file.dj"));
    assert_eq!(range_text(target, &location["range"]), "中 文");
    server.shutdown();
}

#[test]
fn stdio_broken_links_are_errors_at_the_file_fragment_or_reference() {
    let repo = Repository::new();
    repo.write("guide.dj", "# Existing\n");
    repo.write("image.png", "image");
    let source = "中😀 [missing](absent.dj#intro) [anchor](guide.dj#absent) ![image](image.png#part) [external](https://example.invalid/missing#absent)\n\n[x][missing] [^note] [local](#missing)\n";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let diagnostics = server.diagnostics_for(&uri)["diagnostics"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(diagnostics.len(), 5);
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic["severity"], 1);
    }
    let file = diagnostics
        .iter()
        .find(|d| {
            d["message"]
                .as_str()
                .unwrap()
                .contains("Link target does not exist")
        })
        .unwrap();
    assert_eq!(range_text(source, &file["range"]), "absent.dj");
    let anchor = diagnostics
        .iter()
        .find(|d| d["message"] == "Anchor not found: absent")
        .unwrap();
    assert_eq!(range_text(source, &anchor["range"]), "absent");
    for needle in ["absent.dj", "guide.dj#absent", "https://"] {
        assert!(
            definition_uri(&mut server, &uri, source, source.find(needle).unwrap() + 1).is_null()
        );
    }
    server.shutdown();
}

#[test]
fn stdio_reference_label_navigation_is_preserved_and_destination_jumps_across_files() {
    let repo = Repository::new();
    let target = "{#setup}\n# Install\n";
    repo.write("guide.dj", target);
    let source = "[manual][guide]\n\n[guide]: guide.dj#setup\n";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let label = definition_uri(&mut server, &uri, source, source.find("guide]").unwrap());
    assert_eq!(label["uri"], uri);
    assert_eq!(label["range"]["start"]["line"], 2);
    for at in [
        source.find("manual").unwrap(),
        source.find("guide.dj").unwrap(),
    ] {
        let location = definition_uri(&mut server, &uri, source, at);
        assert_eq!(location["uri"], repo.uri("guide.dj"));
        assert_eq!(range_text(target, &location["range"]), "setup");
    }
    server.shutdown();
}

#[test]
fn stdio_link_diagnostics_follow_unsaved_targets_close_and_file_events() {
    let repo = Repository::new();
    repo.write("target.dj", "{#intro}\n# Disk\n");
    let source = "[x](target.dj#intro)";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let target_uri = repo.uri("target.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    open_uri(&mut server, &target_uri, "# Changed\n");
    let diagnostics = server.diagnostics_for(&uri);
    assert_eq!(diagnostics["version"], 1);
    assert_eq!(
        diagnostics["diagnostics"][0]["message"],
        "Anchor not found: intro"
    );
    assert!(definition_uri(&mut server, &uri, source, source.find("intro").unwrap()).is_null());
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":target_uri,"version":2},"contentChanges":[{"text":"{#intro}\n# Unsaved\n"}]}));
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let location = definition_uri(&mut server, &uri, source, source.find("intro").unwrap());
    assert_eq!(location["uri"], target_uri);
    assert_eq!(location["range"]["start"]["line"], 0);
    // An on-disk change cannot override an open target's unsaved text.
    repo.write("target.dj", "# Gone\n");
    server.notify("textDocument/didSave", json!({"textDocument":{"uri":uri}}));
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    server.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":target_uri}}),
    );
    assert_eq!(
        server.diagnostics_for(&uri)["diagnostics"][0]["message"],
        "Anchor not found: intro"
    );
    repo.write("target.dj", "{#intro}\n# Restored\n");
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":target_uri,"type":2}]}),
    );
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    std::fs::remove_file(repo.0.join("target.dj")).unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":target_uri,"type":3}]}),
    );
    assert!(server.diagnostics_for(&uri)["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Link target does not exist"));
    repo.write("target.dj", "{#intro}\n# Created\n");
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":target_uri,"type":1}]}),
    );
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    server.shutdown();
}

#[test]
fn stdio_link_targets_outside_the_index_and_new_unsaved_files_are_resolved() {
    let repo = Repository::new();
    let outside = Repository::new();
    repo.write(".gitignore", "ignored.dj\n");
    repo.write("ignored.dj", "# Hidden\n");
    outside.write("outside.dj", "# Outside\n");
    let source = format!(
        "[ignored](ignored.dj#Hidden) [outside]({}#Outside) [new](new.dj#Draft)",
        outside.uri("outside.dj")
    );
    repo.write("current.dj", &source);
    let uri = repo.uri("current.dj");
    let new_uri = repo.uri("new.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, &source);
    let diagnostics = server.diagnostics_for(&uri)["diagnostics"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(range_text(&source, &diagnostics[0]["range"]), "new.dj");
    for (needle, target_uri) in [
        ("ignored.dj#Hidden", repo.uri("ignored.dj")),
        ("#Outside", outside.uri("outside.dj")),
    ] {
        assert_eq!(
            definition_uri(&mut server, &uri, &source, source.find(needle).unwrap())["uri"],
            target_uri
        );
    }
    open_uri(&mut server, &new_uri, "# Draft\n");
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    assert_eq!(
        definition_uri(&mut server, &uri, &source, source.find("#Draft").unwrap())["uri"],
        new_uri
    );
    server.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":new_uri}}),
    );
    assert_eq!(
        server.diagnostics_for(&uri)["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    server.shutdown();
}

#[test]
fn stdio_non_djot_files_are_checked_for_existence_and_read_failures_are_distinct() {
    let repo = Repository::new();
    std::fs::write(repo.0.join("bad.dj"), [0xff, 0xfe]).unwrap();
    let source = "![x](image.png#%FF) [bad](bad.dj#anchor) [external](https://example.invalid/#%FF) [top](#)";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let diagnostics = server.diagnostics_for(&uri)["diagnostics"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["severity"], 1);
    assert_eq!(range_text(source, &diagnostics[0]["range"]), "image.png");
    assert_eq!(diagnostics[1]["severity"], 2);
    assert!(diagnostics[1]["message"]
        .as_str()
        .unwrap()
        .contains("Cannot read link target"));
    assert!(definition_uri(&mut server, &uri, source, source.find("bad.dj").unwrap()).is_null());
    let top = definition_uri(&mut server, &uri, source, source.find("[top]").unwrap() + 1);
    assert_eq!(top["uri"], uri);
    assert_eq!(top["range"]["start"], json!({"line":0,"character":0}));
    repo.write("image.png", "image");
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":repo.uri("image.png"),"type":1}]}),
    );
    let diagnostics = server.diagnostics_for(&uri)["diagnostics"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["severity"], 2);
    server.shutdown();
}

#[test]
#[cfg(unix)] // A remote file host cannot map to a local path on Unix.
fn stdio_invalid_local_link_uris_and_fragments_report_warnings() {
    let repo = Repository::new();
    repo.write("guide.dj", "# Guide\n");
    let source = "[remote](file://other-host/guide.dj#Guide) [encoding](guide.dj#%FF)";
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let diagnostics = server.diagnostics_for(&uri)["diagnostics"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(diagnostics.len(), 2);
    for (diagnostic, destination, message) in [
        (
            &diagnostics[0],
            "file://other-host/guide.dj#Guide",
            "Cannot resolve local file URI",
        ),
        (
            &diagnostics[1],
            "guide.dj#%FF",
            "Target fragment is not valid UTF-8",
        ),
    ] {
        assert_eq!(diagnostic["severity"], 2);
        assert_eq!(diagnostic["message"], message);
        assert_eq!(range_text(source, &diagnostic["range"]), destination);
        assert!(
            definition_uri(&mut server, &uri, source, source.find(destination).unwrap()).is_null()
        );
    }
    // A subsequent valid edit clears the warnings and still resolves normally.
    let source = "[valid](guide.dj#Guide)";
    server.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":source}]}),
    );
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    assert_eq!(
        definition_uri(&mut server, &uri, source, source.find("guide.dj").unwrap())["uri"],
        repo.uri("guide.dj")
    );
    server.shutdown();
}

#[test]
fn stdio_rapid_link_edits_publish_current_diagnostics_and_clear_on_close() {
    let repo = Repository::new();
    // Keep the target outside the workspace index so validation reads it on a
    // blocking worker while later edits can arrive over the real transport.
    repo.write(".gitignore", "target.dj\n");
    repo.write("target.dj", &"# Target\n\n".repeat(500));
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, "[link](target.dj#Target)");
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    for version in 2..=32 {
        let source = format!("[link](target.dj#missing-{version})");
        server.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":source}]}),
        );
    }
    let mut last_version = 1;
    loop {
        let params = server.diagnostics_for(&uri);
        let version = params["version"].as_u64().unwrap();
        assert!(
            version >= last_version,
            "diagnostics moved backwards: {params}"
        );
        last_version = version;
        assert_eq!(params["diagnostics"].as_array().unwrap().len(), 1);
        assert_eq!(params["diagnostics"][0]["severity"], 1);
        assert_eq!(
            params["diagnostics"][0]["message"],
            format!("Anchor not found: missing-{version}")
        );
        if version == 32 {
            break;
        }
    }
    server.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
    let params = server.diagnostics_for(&uri);
    assert!(params["version"].is_null());
    assert_eq!(params["diagnostics"], json!([]));
    // Drain outstanding protocol work and reject diagnostics after the clear.
    workspace_symbols(&mut server, "");
    assert!(!server.notifications.iter().any(|msg| {
        msg["method"] == "textDocument/publishDiagnostics" && msg["params"]["uri"] == uri
    }));
    server.shutdown();
}

#[test]
fn stdio_links_inside_headings_take_precedence_over_the_surrounding_heading() {
    let repo = Repository::new();
    let target = "# Target\n";
    repo.write("target.dj", target);
    let source = "# [good](target.dj#Target) [bad](target.dj#missing) [local](#missing)\n";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let location = definition_uri(
        &mut server,
        &uri,
        source,
        source.find("target.dj#Target").unwrap(),
    );
    assert_eq!(location["uri"], repo.uri("target.dj"));
    assert_eq!(range_text(target, &location["range"]), "Target");
    for needle in ["target.dj#missing", "#missing)"] {
        assert!(
            definition_uri(&mut server, &uri, source, source.find(needle).unwrap() + 1).is_null()
        );
    }
    server.shutdown();
}

#[test]
fn stdio_reference_destinations_are_validated_without_treating_labels_as_uris() {
    let repo = Repository::new();
    repo.write("guide.dj", "# Existing\n");
    let source = "[x][file#label] [y][anchor#label]\n\n[file#label]: absent.dj#part\n[anchor#label]: guide.dj#absent\n";
    repo.write("current.dj", source);
    let uri = repo.uri("current.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    let diagnostics = server.diagnostics_for(&uri)["diagnostics"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(diagnostics.len(), 4);
    let mut ranges: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d["severity"], 1);
            range_text(source, &d["range"])
        })
        .collect();
    ranges.sort();
    assert_eq!(
        ranges,
        ["absent", "absent.dj", "anchor#label", "file#label"]
    );
    server.shutdown();
}

fn rename_request(server: &mut Server, uri: &str, source: &str, at: usize, name: &str) -> Value {
    server.response(
        "textDocument/rename",
        json!({"textDocument":{"uri":uri},"position":source_position(source, at),"newName":name}),
    )
}

fn prepare_rename(server: &mut Server, uri: &str, source: &str, at: usize) -> Value {
    server.request(
        "textDocument/prepareRename",
        json!({"textDocument":{"uri":uri},"position":source_position(source, at)}),
    )
}

// Apply the actual protocol edits with independent UTF-16 conversion. No
// analysis APIs participate in the tests, including duplicate-anchor checks.
fn apply_workspace_edit(
    edit: &Value,
    sources: &[(&str, &str)],
) -> std::collections::HashMap<String, String> {
    let files: Vec<(String, Vec<Value>)> = if let Some(changes) = edit["changes"].as_object() {
        changes
            .iter()
            .map(|(uri, edits)| (uri.clone(), edits.as_array().unwrap().clone()))
            .collect()
    } else {
        edit["documentChanges"]
            .as_array()
            .expect("workspace edit")
            .iter()
            .filter(|file| file.get("textDocument").is_some())
            .map(|file| {
                (
                    file["textDocument"]["uri"].as_str().unwrap().to_string(),
                    file["edits"].as_array().unwrap().clone(),
                )
            })
            .collect()
    };
    let mut result = std::collections::HashMap::new();
    for (uri, mut edits) in files {
        let source = sources
            .iter()
            .find(|(key, _)| *key == uri)
            .expect("unexpected file edited")
            .1;
        edits.sort_by_key(|edit| source_offset(source, &edit["range"]["start"]));
        for pair in edits.windows(2) {
            assert!(
                source_offset(source, &pair[0]["range"]["end"])
                    <= source_offset(source, &pair[1]["range"]["start"]),
                "overlapping edits"
            );
        }
        let mut updated = source.to_string();
        for edit in edits.iter().rev() {
            updated = apply_edit(&updated, edit);
        }
        assert!(result.insert(uri, updated).is_none(), "duplicate file edit");
    }
    result
}

#[test]
fn stdio_rename_heading_updates_workspace_links_and_renumbered_anchors() {
    let repo = Repository::new();
    let target = "# Same\n\n# Same\n\n# Next\n\n[Same][] [shown][Same] [local](#Same-1)\n";
    let source = "[a](../guide%20x.dj?mode=1#Same) [b](../guide%20x.dj#Same-1) [c](../guide%20x.dj#Next) ![ref][r] [other](other.dj#Same) [web](https://example.com/guide.dj#Same)\n\n[r]: ../guide%20x.dj#Same-1\n";
    repo.write("guide x.dj", target);
    repo.write("docs/links.dj", source);
    repo.write("docs/other.dj", "# Same\n");
    let target_uri = repo.uri("guide x.dj");
    let source_uri = repo.uri("docs/links.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &target_uri, target);
    assert_eq!(
        server.diagnostics_for(&target_uri)["diagnostics"],
        json!([])
    );
    let prepare = prepare_rename(&mut server, &target_uri, target, 2);
    assert_eq!(prepare["placeholder"], "Same");
    assert_eq!(range_text(target, &prepare["range"]), "Same");
    let response = rename_request(&mut server, &target_uri, target, 2, "Next");
    assert!(response.get("error").is_none(), "{response}");
    let renamed = apply_workspace_edit(
        &response["result"],
        &[(&target_uri, target), (&source_uri, source)],
    );
    assert_eq!(renamed.len(), 2);
    let expected_target =
        "# Next\n\n# Same\n\n# Next\n\n[Same][Next] [shown][Next] [local](#Same)\n";
    let expected_source = "[a](../guide%20x.dj?mode=1#Next) [b](../guide%20x.dj#Same) [c](../guide%20x.dj#Next-1) ![ref][r] [other](other.dj#Same) [web](https://example.com/guide.dj#Same)\n\n[r]: ../guide%20x.dj#Same\n";
    assert_eq!(renamed[&target_uri], expected_target);
    assert_eq!(renamed[&source_uri], expected_source);
    // The server only proposes edits; the client applies them to disk/buffers.
    assert_eq!(
        std::fs::read_to_string(repo.0.join("guide x.dj")).unwrap(),
        target
    );
    repo.write("guide x.dj", expected_target);
    repo.write("docs/links.dj", expected_source);
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":target_uri,"version":2},"contentChanges":[{"text":expected_target}]}));
    assert_eq!(
        server.diagnostics_for(&target_uri)["diagnostics"],
        json!([])
    );
    open_uri(&mut server, &source_uri, expected_source);
    assert_eq!(
        server.diagnostics_for(&source_uri)["diagnostics"],
        json!([])
    );
    for (needle, expected_line) in [("#Next)", 0), ("#Same)", 2), ("#Next-1)", 4)] {
        let location = definition_uri(
            &mut server,
            &source_uri,
            expected_source,
            expected_source.find(needle).unwrap() + 1,
        );
        assert_eq!(location["uri"], target_uri);
        assert_eq!(location["range"]["start"]["line"], expected_line);
    }
    server.shutdown();
}

#[test]
fn stdio_rename_heading_with_id_only_updates_title_references() {
    let repo = Repository::new();
    let target = "{#stable}\n# Old\n\n[Old][] [text][Old] [anchor](#stable)\n";
    let source = "[incoming](target.dj#stable)\n";
    repo.write("target.dj", target);
    repo.write("links.dj", source);
    let uri = repo.uri("target.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, target);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    // Initiate from a title reference, with the same semantics as the heading.
    let at = target.find("[Old][]").unwrap() + 1;
    assert_eq!(
        prepare_rename(&mut server, &uri, target, at)["placeholder"],
        "Old"
    );
    let response = rename_request(&mut server, &uri, target, at, "New");
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(&response["result"], &[(&uri, target)]);
    assert_eq!(updated.len(), 1);
    assert_eq!(
        updated[&uri],
        "{#stable}\n# New\n\n[Old][New] [text][New] [anchor](#stable)\n"
    );
    server.shutdown();
}

#[test]
fn stdio_rename_id_recomputes_other_generated_anchors() {
    let repo = Repository::new();
    let target = "{#old}\n# First\n\n# new\n\n[x](#old) [generated](#new) [First][]\n";
    let source = "[x](target.dj#old) [y](target.dj#new)\n";
    repo.write("target.dj", target);
    repo.write("links.dj", source);
    let uri = repo.uri("target.dj");
    let incoming = repo.uri("links.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, target);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    assert_eq!(
        prepare_rename(&mut server, &uri, target, 2)["placeholder"],
        "old"
    );
    let response = rename_request(&mut server, &uri, target, 2, "new");
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(&response["result"], &[(&uri, target), (&incoming, source)]);
    assert_eq!(
        updated[&uri],
        "{#new}\n# First\n\n# new\n\n[x](#new) [generated](#new-1) [First][]\n"
    );
    assert_eq!(
        updated[&incoming],
        "[x](target.dj#new) [y](target.dj#new-1)\n"
    );
    server.shutdown();
}

#[test]
fn stdio_rename_from_encoded_fragment_uses_unsaved_buffers_and_versions() {
    let repo = Repository::new();
    repo.write("target.dj", "# Disk\n");
    let target = "{#旧}\r\n# 标题\r\n";
    let source = "😀 [x](target.dj?q=1#%E6%97%A7)\r\n";
    let uri = repo.uri("target.dj");
    let incoming = repo.uri("new.dj");
    let mut server = Server::start_with_init(
        true,
        true,
        None,
        Some(json!([repo.folder("")])),
        json!({"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true}}}}),
    );
    open_uri(&mut server, &uri, target);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    server.notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":incoming,"languageId":"djot","version":7,"text":source}}),
    );
    assert_eq!(server.diagnostics_for(&incoming)["diagnostics"], json!([]));
    let at = source.find("%E6").unwrap();
    let prepare = prepare_rename(&mut server, &incoming, source, at);
    assert_eq!(prepare["placeholder"], "旧");
    assert_eq!(range_text(source, &prepare["range"]), "%E6%97%A7");
    let response = rename_request(&mut server, &incoming, source, at, "新");
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"]["changes"].is_null());
    for file in response["result"]["documentChanges"].as_array().unwrap() {
        assert_eq!(
            file["textDocument"]["version"],
            if file["textDocument"]["uri"] == incoming {
                7
            } else {
                1
            }
        );
    }
    let updated = apply_workspace_edit(&response["result"], &[(&uri, target), (&incoming, source)]);
    assert_eq!(updated[&uri], "{#新}\r\n# 标题\r\n");
    assert_eq!(updated[&incoming], "😀 [x](target.dj?q=1#%E6%96%B0)\r\n");
    assert!(!repo.0.join("new.dj").exists());
    server.shutdown();
}

#[test]
fn stdio_rename_link_to_ignored_target_and_inline_id() {
    let repo = Repository::new();
    repo.write(".gitignore", "ignored.dj\n");
    let target = "word{#old}\n";
    let source = "[link](ignored.dj#old)";
    repo.write("ignored.dj", target);
    let uri = repo.uri("current.dj");
    let target_uri = repo.uri("ignored.dj");
    let mut server = Server::start_with_init(
        true,
        true,
        None,
        Some(json!([repo.folder("")])),
        json!({"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true}}}}),
    );
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let at = source.find("#old").unwrap() + 1;
    assert_eq!(
        prepare_rename(&mut server, &uri, source, at)["placeholder"],
        "old"
    );
    let response = rename_request(&mut server, &uri, source, at, "new");
    assert!(response.get("error").is_none(), "{response}");
    let target_edit = response["result"]["documentChanges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["textDocument"]["uri"] == target_uri)
        .unwrap();
    assert!(target_edit["textDocument"]["version"].is_null());
    let updated = apply_workspace_edit(
        &response["result"],
        &[(&uri, source), (&target_uri, target)],
    );
    assert_eq!(updated[&uri], "[link](ignored.dj#new)");
    assert_eq!(updated[&target_uri], "word{#new}\n");
    server.shutdown();
}

#[test]
fn stdio_rename_heading_escapes_literal_text_and_preserves_shadowing_labels() {
    let mut server = Server::new(true);
    let source = "# Old\n\n[Old][]\n\n[Old]: https://example.com\n";
    server.open(source);
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    let name = "New [title] *literal* 😀";
    let response = rename_request(&mut server, "file:///test.dj", source, 2, name);
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(&response["result"], &[("file:///test.dj", source)]);
    let expected = "# New \\[title\\] \\*literal\\* 😀\n\n[Old][]\n\n[Old]: https://example.com\n";
    assert_eq!(updated["file:///test.dj"], expected);
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":"file:///test.dj","version":2},"contentChanges":[{"text":expected}]}));
    assert_eq!(server.diagnostics()["diagnostics"], json!([]));
    assert_eq!(
        server.document("textDocument/documentSymbol")[0]["name"],
        name
    );
    server.shutdown();
}

#[test]
fn stdio_rename_duplicate_explicit_ids_updates_only_links_to_selected_definition() {
    for winning in [false, true] {
        let repo = Repository::new();
        let target = "{#same}\n# First\n\n{#same}\n# Second\n";
        let source = "[link](target.dj#same)";
        repo.write("target.dj", target);
        repo.write("links.dj", source);
        let uri = repo.uri("target.dj");
        let incoming = repo.uri("links.dj");
        let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
        open_uri(&mut server, &uri, target);
        let at = if winning {
            target.rfind("same").unwrap()
        } else {
            target.find("same").unwrap()
        };
        let response = rename_request(&mut server, &uri, target, at, "unique");
        assert!(response.get("error").is_none(), "{response}");
        let updated =
            apply_workspace_edit(&response["result"], &[(&uri, target), (&incoming, source)]);
        assert_eq!(updated.len(), if winning { 2 } else { 1 });
        if winning {
            assert_eq!(updated[&incoming], "[link](target.dj#unique)");
        }
        server.shutdown();
    }
}

#[test]
fn stdio_rename_rejects_invalid_names_conflicts_and_non_symbols() {
    let mut server = Server::new(true);
    for (source, at, name) in [
        ("# Old\n", 2, ""),
        ("# Old\n", 2, " new"),
        ("# Old\n", 2, "New\nHeading"),
        ("{#old}\n# Title\n", 2, "bad id"),
        ("{#old}\n# Title\n", 2, "new .class"),
        ("{#old}\n# Title\n\n{#taken}\n# Other\n", 2, "taken"),
        ("# Old\n\n{#taken}\n# Other\n", 2, "taken"),
        ("# First\n\n# Second\n\n[Second][]\n", 11, "First"),
        ("# word{#inline}\n", 2, "New"),
        ("# Old\n\n[Old][]\n\n[New]: https://example.com\n", 2, "New"),
    ] {
        server.open(source);
        let response = rename_request(&mut server, "file:///test.dj", source, at, name);
        assert_eq!(
            response["error"]["code"], -32602,
            "{source:?} -> {name:?}: {response}"
        );
    }
    let source = "plain `code` [file](other.dj) [missing](#absent) [x][label] [^n] [bad](absent.dj#anchor) [web](https://example.com/#external)\n\n[label]: https://example.com\n[^n]: note\n";
    server.open(source);
    for needle in [
        "plain",
        "code",
        "other.dj",
        "absent",
        "label]",
        "n]",
        "absent.dj",
        "anchor)",
        "external)",
    ] {
        let at = source.find(needle).unwrap();
        assert!(
            prepare_rename(&mut server, "file:///test.dj", source, at).is_null(),
            "{needle}"
        );
        assert_eq!(
            rename_request(&mut server, "file:///test.dj", source, at, "New")["error"]["code"],
            -32602
        );
    }
    server.open("# Same\n");
    let response = rename_request(&mut server, "file:///test.dj", "# Same\n", 2, "Same");
    assert_eq!(response["result"]["changes"], json!({}));
    server.shutdown();
}

#[test]
fn stdio_rename_generated_fragment_preserves_other_heading_link_text() {
    let repo = Repository::new();
    let target = "# Old\n\n# [Old][]\n\n[Old][] [shown][Old] [one](#Old) [two](#Old-1)\n";
    let source = "[incoming](target.dj#Old)";
    repo.write("target.dj", target);
    let uri = repo.uri("current.dj");
    let target_uri = repo.uri("target.dj");
    let mut server = Server::start(true, true, None, Some(json!([repo.folder("")])));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let at = source.find("#Old").unwrap() + 1;
    assert_eq!(
        prepare_rename(&mut server, &uri, source, at)["placeholder"],
        "Old"
    );
    let response = rename_request(&mut server, &uri, source, at, "New [title] 😀");
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(
        &response["result"],
        &[(&uri, source), (&target_uri, target)],
    );
    let expected = "# New \\[title\\] 😀\n\n# [Old][New \\[title\\] 😀]\n\n[Old][New \\[title\\] 😀] [shown][New \\[title\\] 😀] [one](#New-title-%F0%9F%98%80) [two](#Old)\n";
    assert_eq!(updated[&target_uri], expected);
    assert_eq!(
        updated[&uri],
        "[incoming](target.dj#New-title-%F0%9F%98%80)"
    );
    repo.write("target.dj", expected);
    open_uri(&mut server, &target_uri, expected);
    assert_eq!(
        server.diagnostics_for(&target_uri)["diagnostics"],
        json!([])
    );
    let symbols = server.request(
        "textDocument/documentSymbol",
        json!({"textDocument":{"uri":target_uri}}),
    );
    assert_eq!(symbols[0]["name"], "New [title] 😀");
    assert_eq!(symbols[1]["name"], "Old");
    let location = definition_uri(
        &mut server,
        &target_uri,
        expected,
        expected.find("#Old)").unwrap() + 1,
    );
    assert_eq!(location["range"]["start"]["line"], 2);
    server.shutdown();
}

#[test]
fn stdio_rename_heading_replaces_nested_links_and_keeps_unresolved_references() {
    let mut server = Server::new(true);
    let source = "# Prefix [shown][Other]\n\n# Other\n\n[x][missing]\n\n[in](#Prefix-shown)\n";
    server.open(source);
    let response = rename_request(&mut server, "file:///test.dj", source, 2, "Renamed");
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(&response["result"], &[("file:///test.dj", source)]);
    assert_eq!(
        updated["file:///test.dj"],
        "# Renamed\n\n# Other\n\n[x][missing]\n\n[in](#Renamed)\n"
    );
    server.shutdown();
}

fn file_operation_server(repo: &Repository, versioned: bool, resource_rename: bool) -> Server {
    file_operation_server_at(repo, "", versioned, resource_rename)
}

fn file_operation_server_at(
    repo: &Repository,
    folder: &str,
    versioned: bool,
    resource_rename: bool,
) -> Server {
    Server::start_with_init(
        true,
        true,
        None,
        Some(json!([repo.folder(folder)])),
        json!({
            "capabilities":{"workspace":{
                "fileOperations":{"willRename":true,"didRename":true},
                "workspaceEdit":{"documentChanges":versioned,"resourceOperations":if resource_rename { vec!["rename"] } else { vec![] }}
            }}
        }),
    )
}

fn file_move_params(files: &[(&str, &str)]) -> Value {
    json!({"files":files.iter().map(|(old,new)| json!({"oldUri":old,"newUri":new})).collect::<Vec<_>>()})
}

fn write_edited_sources(files: &std::collections::HashMap<String, String>) {
    for (uri, source) in files {
        let path = url::Url::parse(uri).unwrap().to_file_path().unwrap();
        std::fs::write(path, source).unwrap();
    }
}

#[test]
fn stdio_file_move_updates_incoming_outgoing_and_reference_destinations() {
    let repo = Repository::new();
    let target = "# Intro\n\n[related](related.dj#More) ![image](../assets/logo.png?size=1#frame) [self](guide.dj#Intro) [local](#Intro) [web](https://example.com/)\n";
    let source = "[guide](docs/guide.dj?q=%2F#Intro) [ref][g] [broken](docs/guide.dj#missing)\n\n[g]: docs/guide.dj#Intro\n";
    repo.write("docs/guide.dj", target);
    repo.write("docs/related.dj", "# More\n");
    repo.write("assets/logo.png", "image");
    repo.write("current.dj", source);
    std::fs::create_dir(repo.0.join("manual")).unwrap();
    let uri = repo.uri("current.dj");
    let old = repo.uri("docs/guide.dj");
    let new = repo.uri("manual/guide(中).dj");
    let mut server = file_operation_server(&repo, false, false);
    open_uri(&mut server, &uri, source);
    let params = file_move_params(&[(&old, &new)]);
    let edit = server.request("workspace/willRenameFiles", params.clone());
    assert!(edit["documentChanges"].is_null());
    let updated = apply_workspace_edit(&edit, &[(&uri, source), (&old, target)]);
    let path = "manual/guide%28%E4%B8%AD%29.dj";
    let expected_source = format!(
        "[guide]({path}?q=%2F#Intro) [ref][g] [broken]({path}#missing)\n\n[g]: {path}#Intro\n"
    );
    assert_eq!(updated[&uri], expected_source);
    let expected_target = "# Intro\n\n[related](../docs/related.dj#More) ![image](../assets/logo.png?size=1#frame) [self](#Intro) [local](#Intro) [web](https://example.com/)\n";
    assert_eq!(updated[&old], expected_target);
    assert!(repo.0.join("docs/guide.dj").exists());
    assert!(!repo.0.join("manual/guide(中).dj").exists());
    write_edited_sources(&updated);
    std::fs::rename(
        repo.0.join("docs/guide.dj"),
        repo.0.join("manual/guide(中).dj"),
    )
    .unwrap();
    server.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":expected_source}]}),
    );
    server.notify("workspace/didRenameFiles", params);
    open_uri(&mut server, &new, expected_target);
    assert_eq!(server.diagnostics_for(&new)["diagnostics"], json!([]));
    let location = definition_uri(
        &mut server,
        &uri,
        &expected_source,
        expected_source.find("manual/").unwrap(),
    );
    assert_eq!(
        url::Url::parse(location["uri"].as_str().unwrap())
            .unwrap()
            .to_file_path()
            .unwrap(),
        repo.0.join("manual/guide(中).dj")
    );
    assert_eq!(location["range"]["start"]["line"], 0);
    let related = definition_uri(
        &mut server,
        &new,
        expected_target,
        expected_target.find("../docs").unwrap(),
    );
    assert_eq!(related["uri"], repo.uri("docs/related.dj"));
    server.shutdown();
}

#[test]
fn stdio_folder_move_preserves_internal_links_and_unsaved_open_buffers() {
    let repo = Repository::new();
    repo.write("old/chapter.dj", "# Disk\n\n[out](../outside.dj#Out)\n");
    repo.write("old/peer.dj", "# Peer\n");
    repo.write("old/image.png", "image");
    repo.write("older/chapter.dj", "# Unchanged\n");
    repo.write("outside.dj", "# Out\n");
    let target = "# Draft\n\n[peer](peer.dj#Peer) ![img](image.png) [out](../outside.dj#Out)\n";
    let source =
        "[chapter](old/chapter.dj#Draft) [folder](old/) [sibling](older/chapter.dj#Unchanged)";
    let uri = repo.uri("current.dj");
    let old_doc = repo.uri("old/chapter.dj");
    let new_doc = repo.uri("nested/new/chapter.dj");
    let old_folder = repo.uri("old");
    let new_folder = repo.uri("nested/new");
    let mut server = file_operation_server_at(&repo, "old", true, false);
    open_uri(&mut server, &old_doc, target);
    assert_eq!(server.diagnostics_for(&old_doc)["diagnostics"], json!([]));
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let params = file_move_params(&[(&old_folder, &new_folder)]);
    let edit = server.request("workspace/willRenameFiles", params.clone());
    for file in edit["documentChanges"].as_array().unwrap() {
        assert_eq!(file["textDocument"]["version"], 1);
    }
    let updated = apply_workspace_edit(&edit, &[(&uri, source), (&old_doc, target)]);
    let expected_target =
        "# Draft\n\n[peer](peer.dj#Peer) ![img](image.png) [out](../../outside.dj#Out)\n";
    let expected_source = "[chapter](nested/new/chapter.dj#Draft) [folder](nested/new/) [sibling](older/chapter.dj#Unchanged)";
    assert_eq!(updated[&old_doc], expected_target);
    assert_eq!(updated[&uri], expected_source);
    // Apply buffer edits without saving: didRenameFiles must retain the draft,
    // not replace it with the different on-disk heading at the new location.
    server.notify("textDocument/didChange", json!({"textDocument":{"uri":old_doc,"version":2},"contentChanges":[{"text":expected_target}]}));
    server.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":expected_source}]}),
    );
    std::fs::create_dir(repo.0.join("nested")).unwrap();
    std::fs::rename(repo.0.join("old"), repo.0.join("nested/new")).unwrap();
    server.notify("workspace/didRenameFiles", params);
    let cleared = loop {
        let diagnostics = server.diagnostics_for(&old_doc);
        if diagnostics["version"].is_null() {
            break diagnostics;
        }
    };
    assert_eq!(cleared["diagnostics"], json!([]));
    let diagnostics = server.diagnostics_for(&new_doc);
    assert_eq!(diagnostics["version"], 2);
    assert_eq!(diagnostics["diagnostics"], json!([]));
    let location = definition_uri(
        &mut server,
        &uri,
        expected_source,
        expected_source.find("nested/").unwrap(),
    );
    assert_eq!(location["uri"], new_doc);
    assert_eq!(range_text(expected_target, &location["range"]), "Draft");
    assert!(symbol_names(&mut server).contains(&"Draft".to_string()));
    assert!(symbol_names(&mut server).contains(&"Peer".to_string()));
    assert!(!symbol_names(&mut server).contains(&"Disk".to_string()));
    server.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":old_doc}}),
    );
    assert_eq!(
        definition_uri(
            &mut server,
            &uri,
            expected_source,
            expected_source.find("nested/").unwrap()
        )["uri"],
        new_doc
    );
    server.shutdown();
}

#[test]
fn stdio_batch_file_moves_use_original_targets_without_chaining() {
    let repo = Repository::new();
    let a = "# A\n\n[b](b.dj#B) [self](#A) [file](a.dj)\n";
    let b = "# B\n\n[a](a.dj#A)\n";
    let source = "[a](a.dj#A) [b](b.dj#B)";
    repo.write("a.dj", a);
    repo.write("b.dj", b);
    repo.write("current.dj", source);
    let a_uri = repo.uri("a.dj");
    let b_uri = repo.uri("b.dj");
    let c_uri = repo.uri("c.dj");
    let uri = repo.uri("current.dj");
    let mut server = file_operation_server(&repo, true, false);
    let params = file_move_params(&[(&a_uri, &b_uri), (&b_uri, &c_uri)]);
    let edit = server.request("workspace/willRenameFiles", params);
    let updated = apply_workspace_edit(&edit, &[(&a_uri, a), (&b_uri, b), (&uri, source)]);
    assert_eq!(updated[&uri], "[a](b.dj#A) [b](c.dj#B)");
    assert_eq!(
        updated[&a_uri],
        "# A\n\n[b](c.dj#B) [self](#A) [file](b.dj)\n"
    );
    assert_eq!(updated[&b_uri], "# B\n\n[a](b.dj#A)\n");
    for file in edit["documentChanges"].as_array().unwrap() {
        assert!(file["textDocument"]["version"].is_null());
    }
    server.shutdown();
}

#[test]
fn stdio_rename_link_path_proposes_text_edits_before_resource_move() {
    let repo = Repository::new();
    let target = "# Intro\n\n[out](../other.dj#Other)\n";
    let source = "😀 [here](docs/old.dj?q=1#Intro)\r\n";
    repo.write("docs/old.dj", target);
    repo.write("other.dj", "# Other\n");
    std::fs::create_dir_all(repo.0.join("deep/nest")).unwrap();
    let uri = repo.uri("current.dj");
    let old = repo.uri("docs/old.dj");
    let new = repo.uri("deep/nest/new.dj");
    let mut server = file_operation_server(&repo, true, true);
    open_uri(&mut server, &uri, source);
    assert_eq!(server.diagnostics_for(&uri)["diagnostics"], json!([]));
    let at = source.find("docs/").unwrap();
    let prepare = prepare_rename(&mut server, &uri, source, at);
    assert_eq!(prepare["placeholder"], "docs/old.dj");
    assert_eq!(range_text(source, &prepare["range"]), "docs/old.dj");
    let response = rename_request(&mut server, &uri, source, at, "deep/nest/new.dj");
    assert!(response.get("error").is_none(), "{response}");
    let operations = response["result"]["documentChanges"].as_array().unwrap();
    let resource = operations.last().unwrap();
    assert_eq!(resource["kind"], "rename");
    assert_eq!(resource["oldUri"], old);
    assert_eq!(resource["newUri"], new);
    assert!(operations[..operations.len() - 1]
        .iter()
        .all(|op| op.get("textDocument").is_some()));
    let updated = apply_workspace_edit(&response["result"], &[(&uri, source), (&old, target)]);
    assert_eq!(updated[&uri], "😀 [here](deep/nest/new.dj?q=1#Intro)\r\n");
    assert_eq!(updated[&old], "# Intro\n\n[out](../../other.dj#Other)\n");
    assert!(repo.0.join("docs/old.dj").exists());
    write_edited_sources(&updated);
    std::fs::rename(repo.0.join("docs/old.dj"), repo.0.join("deep/nest/new.dj")).unwrap();
    server.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":updated[&uri]}]}),
    );
    server.notify(
        "workspace/didRenameFiles",
        file_move_params(&[(&old, &new)]),
    );
    assert_eq!(
        definition_uri(&mut server, &uri, &updated[&uri], at)["uri"],
        new
    );
    server.shutdown();
}

#[test]
fn stdio_rename_asset_path_at_reference_definition_preserves_suffix() {
    let repo = Repository::new();
    repo.write("img/a.png", "image");
    std::fs::create_dir(repo.0.join("icons")).unwrap();
    let source = "![pic][logo]\n\n[logo]: img/a.png?size=1#frame\n";
    let uri = repo.uri("current.dj");
    let mut server = file_operation_server(&repo, true, true);
    open_uri(&mut server, &uri, source);
    let at = source.find("img/").unwrap();
    assert_eq!(
        prepare_rename(&mut server, &uri, source, at)["placeholder"],
        "img/a.png"
    );
    let response = rename_request(&mut server, &uri, source, at, "icons/新 (1).png");
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(&response["result"], &[(&uri, source)]);
    assert_eq!(
        updated[&uri],
        "![pic][logo]\n\n[logo]: icons/%E6%96%B0%20%281%29.png?size=1#frame\n"
    );
    let operations = response["result"]["documentChanges"].as_array().unwrap();
    assert_eq!(operations.last().unwrap()["oldUri"], repo.uri("img/a.png"));
    assert_eq!(
        operations.last().unwrap()["newUri"],
        repo.uri("icons/新 (1).png")
    );
    server.shutdown();
}

#[test]
fn stdio_file_move_reads_explicit_ignored_target_and_preserves_absolute_links() {
    let repo = Repository::new();
    repo.write(".gitignore", "ignored.dj\n");
    let target = "# Hidden\n\n[out](other.dj#Other)\n";
    repo.write("ignored.dj", target);
    repo.write("other.dj", "# Other\n");
    let old = repo.uri("ignored.dj");
    let new = repo.uri("nested/new.dj");
    let absolute_path = repo.0.join("ignored.dj").to_string_lossy().to_string();
    let source = format!("[uri]({old}?q=1#Hidden) [root]({absolute_path}#Hidden)");
    repo.write("current.dj", &source);
    let uri = repo.uri("current.dj");
    let mut server = file_operation_server(&repo, false, false);
    let edit = server.request(
        "workspace/willRenameFiles",
        file_move_params(&[(&old, &new)]),
    );
    let updated = apply_workspace_edit(&edit, &[(&uri, &source), (&old, target)]);
    let new_path = repo.0.join("nested/new.dj").to_string_lossy().to_string();
    assert_eq!(
        updated[&uri],
        format!("[uri]({new}?q=1#Hidden) [root]({new_path}#Hidden)")
    );
    assert_eq!(updated[&old], "# Hidden\n\n[out](../other.dj#Other)\n");
    server.shutdown();
}

#[test]
fn stdio_file_moves_reject_conflicts_invalid_uris_and_unsupported_link_rename() {
    let repo = Repository::new();
    repo.write("a.dj", "# A\n");
    repo.write("b.dj", "# B\n");
    let uri = repo.uri("current.dj");
    let a = repo.uri("a.dj");
    let b = repo.uri("b.dj");
    let c = repo.uri("c.dj");
    let mut server = file_operation_server(&repo, false, false);
    for files in [
        vec![(a.as_str(), b.as_str())],
        vec![(a.as_str(), c.as_str()), (b.as_str(), c.as_str())],
        vec![("invalid", c.as_str())],
        vec![(a.as_str(), "https://example.com/new.dj")],
        vec![(a.as_str(), "file:///tmp/new.dj#anchor")],
        vec![(a.as_str(), "file:///tmp/new.dj?q=1")],
    ] {
        let response = server.response("workspace/willRenameFiles", file_move_params(&files));
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
    let folder = repo.uri("dir");
    let child = repo.uri("dir/a.dj");
    let descendant = repo.uri("dir/nested");
    for files in [
        vec![(folder.as_str(), descendant.as_str())],
        vec![(folder.as_str(), c.as_str()), (child.as_str(), b.as_str())],
    ] {
        assert_eq!(
            server.response("workspace/willRenameFiles", file_move_params(&files))["error"]["code"],
            -32602
        );
    }
    let source = "[a](a.dj#A)";
    open_uri(&mut server, &uri, source);
    assert!(prepare_rename(&mut server, &uri, source, source.find("a.dj").unwrap()).is_null());
    let response = rename_request(
        &mut server,
        &uri,
        source,
        source.find("a.dj").unwrap(),
        "c.dj",
    );
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(
        prepare_rename(&mut server, &uri, source, source.find("#A").unwrap() + 1)["placeholder"],
        "A"
    );
    let empty = server.request("workspace/willRenameFiles", file_move_params(&[(&a, &a)]));
    assert_eq!(empty["changes"], json!({}));
    server.shutdown();
    let mut server = file_operation_server(&repo, true, true);
    open_uri(&mut server, &uri, source);
    for name in [
        "",
        " new.dj",
        "new.dj#anchor",
        "new.dj?q=1",
        "https://example.com/new.dj",
        "missing/child.dj",
        "b.dj",
    ] {
        assert_eq!(
            rename_request(
                &mut server,
                &uri,
                source,
                source.find("a.dj").unwrap(),
                name
            )["error"]["code"],
            -32602,
            "{name}"
        );
    }
    let unchanged = rename_request(
        &mut server,
        &uri,
        source,
        source.find("a.dj").unwrap(),
        "a.dj",
    );
    assert_eq!(unchanged["result"]["documentChanges"], json!([]));
    server.shutdown();
}

#[test]
fn stdio_folder_link_rename_keeps_equivalent_encoded_internal_paths() {
    let repo = Repository::new();
    let inner = "# Src\n\n[peer](peer%28x%29.dj#Peer)\n";
    repo.write("old.dj/src.dj", inner);
    repo.write("old.dj/peer(x).dj", "# Peer\n");
    std::fs::create_dir(repo.0.join("nested")).unwrap();
    let source = "[folder](old.dj/) [peer](old.dj/peer%28x%29.dj#Peer)";
    let uri = repo.uri("current.dj");
    let mut server = file_operation_server(&repo, true, true);
    open_uri(&mut server, &uri, source);
    let at = source.find("old.dj/").unwrap();
    let response = rename_request(&mut server, &uri, source, at, "nested/new");
    assert!(response.get("error").is_none(), "{response}");
    let updated = apply_workspace_edit(&response["result"], &[(&uri, source)]);
    assert_eq!(updated.len(), 1);
    assert_eq!(
        updated[&uri],
        "[folder](nested/new/) [peer](nested/new/peer%28x%29.dj#Peer)"
    );
    let resource = response["result"]["documentChanges"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(resource["kind"], "rename");
    assert_eq!(resource["oldUri"], format!("{}/", repo.uri("old.dj")));
    assert_eq!(resource["newUri"], repo.uri("nested/new"));
    assert_eq!(
        std::fs::read_to_string(repo.0.join("old.dj/src.dj")).unwrap(),
        inner
    );
    server.shutdown();
}

#[test]
fn stdio_file_move_keeps_newly_opened_destination_buffer_authoritative() {
    let repo = Repository::new();
    repo.write("old.dj", "# Disk\n");
    let old = repo.uri("old.dj");
    let disk_uri = repo.uri("new(x).dj");
    let new = disk_uri.replace('(', "%28").replace(')', "%29");
    let mut server = file_operation_server(&repo, true, false);
    open_uri(&mut server, &old, "# Older draft\n");
    assert_eq!(server.diagnostics_for(&old)["diagnostics"], json!([]));
    std::fs::rename(repo.0.join("old.dj"), repo.0.join("new(x).dj")).unwrap();
    server.notify("textDocument/didOpen", json!({"textDocument":{"uri":new,"languageId":"djot","version":7,"text":"# Newer draft\n"}}));
    let params = file_move_params(&[(&old, &disk_uri)]);
    server.notify("workspace/didRenameFiles", params);
    let cleared = loop {
        let diagnostics = server.diagnostics_for(&old);
        if diagnostics["version"].is_null() {
            break diagnostics;
        }
    };
    assert_eq!(cleared["diagnostics"], json!([]));
    assert_eq!(
        server.request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":new}})
        )[0]["name"],
        "Newer draft"
    );
    assert_eq!(
        server.response(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":old}})
        )["error"]["code"],
        -32602
    );
    assert!(!symbol_names(&mut server).contains(&"Older draft".to_string()));
    assert!(!symbol_names(&mut server).contains(&"Disk".to_string()));
    // Invalid notifications leave the active buffers intact.
    server.notify(
        "workspace/didRenameFiles",
        file_move_params(&[("invalid", &new)]),
    );
    assert_eq!(
        server.request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":new}})
        )[0]["name"],
        "Newer draft"
    );
    server.shutdown();
}

#[test]
fn stdio_file_move_rejects_unreadable_targets_but_uses_unsaved_override() {
    let repo = Repository::new();
    repo.write(".gitignore", "bad.dj\n");
    std::fs::write(repo.0.join("bad.dj"), [0xff, 0xfe]).unwrap();
    repo.write("other.dj", "# Other\n");
    let old = repo.uri("bad.dj");
    let new = repo.uri("nested/new.dj");
    let mut server = file_operation_server(&repo, true, false);
    let params = file_move_params(&[(&old, &new)]);
    let response = server.response("workspace/willRenameFiles", params.clone());
    assert_eq!(response["error"]["code"], -32602);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Cannot read moved Djot file"));
    let target = "# Draft\n\n[out](other.dj#Other)\n";
    open_uri(&mut server, &old, target);
    assert_eq!(server.diagnostics_for(&old)["diagnostics"], json!([]));
    let edit = server.request("workspace/willRenameFiles", params);
    let updated = apply_workspace_edit(&edit, &[(&old, target)]);
    assert_eq!(updated[&old], "# Draft\n\n[out](../other.dj#Other)\n");
    assert_eq!(edit["documentChanges"][0]["textDocument"]["version"], 1);
    server.shutdown();
}

#[test]
fn stdio_file_moves_update_absolute_links_in_non_file_buffers() {
    let repo = Repository::new();
    let old = repo.uri("ghost.dj");
    let new = repo.uri("moved.dj");
    let source = "[missing](ghost.dj#anchor) [remote](file://other-host/share/a.png) [invalid](https://[invalid])";
    let uri = repo.uri("current.dj");
    repo.write("current.dj", source);
    let draft = format!("# Draft\n\n[file]({old}#anchor) [local](#Draft)\n");
    let mut server = file_operation_server(&repo, false, false);
    open_uri(&mut server, "untitled:draft", &draft);
    let edit = server.request(
        "workspace/willRenameFiles",
        file_move_params(&[(&old, &new)]),
    );
    let updated = apply_workspace_edit(&edit, &[(&uri, source), ("untitled:draft", &draft)]);
    assert_eq!(updated[&uri], "[missing](moved.dj#anchor) [remote](file://other-host/share/a.png) [invalid](https://[invalid])");
    assert_eq!(
        updated["untitled:draft"],
        format!("# Draft\n\n[file]({new}#anchor) [local](#Draft)\n")
    );
    assert!(symbol_names(&mut server).contains(&"Draft".to_string()));
    server.shutdown();
}
