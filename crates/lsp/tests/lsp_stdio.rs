// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "tests/lsp_stdio.rs"
// ============================================================================
// crates/lsp/tests/lsp_stdio.rs
//! End-to-end test: spawn `ubel-lsp`, speak JSON-RPC over stdio, and check
//! the diagnostics that come back for a document with a type error.
//!
//! Plain `std` plus `serde_json`, no LSP client library, so the wire format
//! the editor sees is what is being asserted.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    out:   BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ubel-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ubel-lsp");
        let stdin = child.stdin.take().unwrap();
        let out   = BufReader::new(child.stdout.take().unwrap());
        Server { child, stdin: Some(stdin), out }
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        let stdin = self.stdin.as_mut().expect("stdin still open");
        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        stdin.flush().unwrap();
    }

    /// Close the server's stdin, as a client does when it is done. tower-lsp
    /// keeps serving after the `exit` notification until the next message or
    /// EOF, so the process ends when the pipe closes.
    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn read(&mut self) -> Value {
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            self.out.read_line(&mut line).expect("read header");
            let line = line.trim();
            if line.is_empty() { break; }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut buf = vec![0u8; len];
        self.out.read_exact(&mut buf).expect("read body");
        serde_json::from_slice(&buf).unwrap()
    }

    /// Read messages until one has `method`, skipping the rest.
    fn read_until_method(&mut self, method: &str) -> Value {
        loop {
            let m = self.read();
            if m["method"] == method { return m; }
        }
    }

    /// Read until the response to request `id`.
    fn read_response(&mut self, id: u64) -> Value {
        loop {
            let m = self.read();
            if m["id"] == id && m.get("method").is_none() { return m; }
        }
    }
}

/// Run `f` against a fresh server, killing it if the test hangs.
fn with_server(f: impl FnOnce(&mut Server)) {
    let mut server = Server::start();
    let pid = server.child.id();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watchdog = thread::spawn(move || {
        if done_rx.recv_timeout(Duration::from_secs(30)).is_err() {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
        }
    });

    server.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}));
    let init = server.read_response(1);
    assert_eq!(init["result"]["serverInfo"]["name"], "ubel-lsp");
    server.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));

    f(&mut server);

    server.send(json!({"jsonrpc":"2.0","id":99,"method":"shutdown"}));
    server.read_response(99);
    server.send(json!({"jsonrpc":"2.0","method":"exit"}));
    server.close_stdin();
    let status = server.child.wait().expect("wait for ubel-lsp");
    assert!(status.success(), "ubel-lsp should exit cleanly after shutdown, exit and EOF");
    let _ = done_tx.send(());
    let _ = watchdog.join();
}

fn open(server: &mut Server, uri: &str, version: i32, text: &str) {
    server.send(json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":uri,"languageId":"ubel","version":version,"text":text}}
    }));
}

const URI: &str = "file:///project/main.ubl";

#[test]
fn advertises_full_sync_only() {
    let mut server = Server::start();
    server.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}));
    let init = server.read_response(1);
    assert_eq!(init["result"]["capabilities"]["textDocumentSync"], 1);
    assert!(init["result"]["capabilities"]["hoverProvider"].is_null());
    let _ = server.child.kill();
}

#[test]
fn a_type_error_is_published_with_code_and_position() {
    with_server(|s| {
        open(s, URI, 1, "fn main() void {\n    let x: int = \"a\"\n}\n");
        let m = s.read_until_method("textDocument/publishDiagnostics");
        assert_eq!(m["params"]["uri"], URI);
        let d = &m["params"]["diagnostics"][0];
        assert_eq!(d["code"], "TYPE-101");
        assert_eq!(d["source"], "ubel");
        assert_eq!(d["range"]["start"]["line"], 1, "the bad initializer is on the second line");
    });
}

#[test]
fn a_clean_document_publishes_an_empty_list() {
    with_server(|s| {
        open(s, URI, 1, "fn main() void { println(\"hi\") }\n");
        let m = s.read_until_method("textDocument/publishDiagnostics");
        assert_eq!(m["params"]["diagnostics"].as_array().unwrap().len(), 0);
    });
}

#[test]
fn an_edit_that_fixes_the_error_clears_it() {
    with_server(|s| {
        open(s, URI, 1, "fn main() void { let x: int = \"a\" }\n");
        let first = s.read_until_method("textDocument/publishDiagnostics");
        assert_eq!(first["params"]["diagnostics"].as_array().unwrap().len(), 1);

        s.send(json!({
            "jsonrpc":"2.0","method":"textDocument/didChange",
            "params":{
                "textDocument":{"uri":URI,"version":2},
                "contentChanges":[{"text":"fn main() void { let x: int = 1 }\n"}]
            }
        }));
        let second = s.read_until_method("textDocument/publishDiagnostics");
        assert_eq!(second["params"]["version"], 2);
        assert_eq!(second["params"]["diagnostics"].as_array().unwrap().len(), 0);
    });
}

#[test]
fn closing_a_document_clears_its_diagnostics() {
    with_server(|s| {
        open(s, URI, 1, "fn main() void { let x: int = \"a\" }\n");
        s.read_until_method("textDocument/publishDiagnostics");

        s.send(json!({
            "jsonrpc":"2.0","method":"textDocument/didClose",
            "params":{"textDocument":{"uri":URI}}
        }));
        let m = s.read_until_method("textDocument/publishDiagnostics");
        assert_eq!(m["params"]["diagnostics"].as_array().unwrap().len(), 0);
    });
}
