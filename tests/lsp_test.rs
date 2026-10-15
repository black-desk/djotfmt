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
}
impl Server {
    fn new(snippets: bool) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_djot-lsp"))
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
        };
        let result = server.request("initialize", json!({"processId":null,"rootUri":null,"capabilities":{"textDocument":{"completion":{"completionItem":{"snippetSupport":snippets}},"documentSymbol":{"hierarchicalDocumentSymbolSupport":true}}}}));
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        assert_eq!(result["capabilities"]["textDocumentSync"], 2);
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
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let msg = self
                .output
                .recv_timeout(Duration::from_secs(10))
                .expect("LSP response timed out");
            if msg["id"] == id {
                assert!(msg.get("error").is_none(), "{msg}");
                return msg["result"].clone();
            }
        }
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
    loop {
        let msg = server.output.recv_timeout(Duration::from_secs(10)).unwrap();
        if msg["method"] == "textDocument/publishDiagnostics" {
            assert_eq!(msg["params"]["version"], 1);
            assert_eq!(
                msg["params"]["diagnostics"][0]["range"]["start"]["character"],
                4
            );
            assert!(msg["params"]["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing"));
            break;
        }
    }
    server.notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":"file:///test.dj"}}),
    );
    let msg = server.output.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(msg["params"]["diagnostics"], json!([]));
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
