//! End-to-end smoke test: spawns the compiled server binary and drives it
//! with raw JSON-RPC frames over stdio (initialize -> didOpen -> diagnostics).

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_webspeed-language-server"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_string(value).unwrap();
        let stdin = self.stdin.as_mut().expect("stdin already closed");
        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        stdin.flush().unwrap();
    }

    /// Closes the client side of the pipe; the transport loop ends and the
    /// server process exits.
    fn close(&mut self) {
        self.stdin.take();
    }

    /// Reads one JSON-RPC frame; `None` on EOF.
    fn read_frame(&mut self) -> Option<Value> {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).ok()? == 0 {
                return None;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(len) = line
                .strip_prefix("Content-Length:")
                .and_then(|s| s.trim().parse::<usize>().ok())
            {
                content_length = Some(len);
            }
        }
        let len = content_length?;
        let mut body = vec![0u8; len];
        self.stdout.read_exact(&mut body).ok()?;
        serde_json::from_slice(&body).ok()
    }

    /// Reads frames until one matches `predicate`, or panics on timeout.
    fn wait_for<F>(&mut self, predicate: F) -> Value
    where
        F: Fn(&Value) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(frame) = self.read_frame() {
                if predicate(&frame) {
                    return frame;
                }
            } else {
                panic!("server exited prematurely");
            }
        }
        panic!("timed out waiting for frame");
    }
}

fn open_doc(uri: &str, text: &str, version: i32) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": { "uri": uri, "languageId": "htm", "version": version, "text": text }
        }
    })
}

#[test]
fn smoke_lifecycle_and_diagnostics() {
    let mut srv = Server::start();

    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "rootUri": null, "capabilities": {} }
    }));
    let init = srv.wait_for(|f| f["id"] == 1);
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["textDocumentSync"], json!(1), "full sync expected");
    assert!(caps["completionProvider"].is_object());
    assert!(caps["hoverProvider"].is_object() || caps["hoverProvider"] == json!(true));
    assert!(caps["definitionProvider"].is_object() || caps["definitionProvider"] == json!(true));
    assert!(
        caps["documentFormattingProvider"].is_object()
            || caps["documentFormattingProvider"] == json!(true)
    );
    assert!(caps["semanticTokensProvider"].is_object());

    srv.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));

    // ABL syntax error (missing terminator dot) + unknown function.
    let text = "<html><body><% define variable ok as integer. %>\n\
                <script>let a = ;</script>\n\
                <% message fn(1). %>\n</body></html>";
    let uri = "file:///tmp/webspeed-smoke.htm";
    srv.send(&open_doc(uri, text, 1));

    let diags = srv
        .wait_for(|f| f["method"] == "textDocument/publishDiagnostics" && f["params"]["uri"] == uri)
        .take();
    let items = diags["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(!items.is_empty(), "expected diagnostics, got {items:?}");

    // All reported ranges must stay within the document.
    for d in items {
        let start_line = d["range"]["start"]["line"].as_u64().unwrap();
        let end_line = d["range"]["end"]["line"].as_u64().unwrap();
        assert!(
            start_line <= 4 && end_line <= 4,
            "range out of bounds: {d:?}"
        );
    }

    srv.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": {} }));
    let shutdown = srv.wait_for(|f| f["id"] == 2);
    assert!(shutdown["result"].is_null());
    srv.send(&json!({ "jsonrpc": "2.0", "method": "exit", "params": null }));
    srv.close();
    let status = srv.child.wait().expect("server should exit cleanly");
    assert!(status.success());
}

#[test]
fn smoke_ignores_non_webspeed_files() {
    let mut srv = Server::start();
    srv.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "rootUri": null, "capabilities": {} }
    }));
    let _ = srv.wait_for(|f| f["id"] == 1);
    srv.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
    srv.send(&open_doc("file:///tmp/not_webspeed.p", "message oops", 1));
    srv.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": {} }));
    let _ = srv.wait_for(|f| f["id"] == 2);
    srv.send(&json!({ "jsonrpc": "2.0", "method": "exit", "params": null }));
    srv.close();
    let status = srv.child.wait().expect("server should exit cleanly");
    assert!(status.success());
}
