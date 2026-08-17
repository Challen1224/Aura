//! End-to-end Debug Adapter Protocol session against the real `aura dap`
//! binary over stdio: initialize/launch/setBreakpoints/configurationDone,
//! a breakpoint stop with stack/scopes/variables, path evaluation,
//! stepping, program output as `output` events, and clean termination.

use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};

struct DapClient {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    seq: u64,
}

impl DapClient {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aura"))
            .arg("dap")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn aura dap");
        let reader = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            reader,
            seq: 0,
        }
    }

    fn send(&mut self, command: &str, args: Json) {
        self.seq += 1;
        let msg = json!({
            "type": "request", "seq": self.seq, "command": command, "arguments": args
        })
        .to_string();
        let stdin = self.child.stdin.as_mut().expect("stdin");
        write!(stdin, "Content-Length: {}\r\n\r\n{}", msg.len(), msg).expect("write");
        stdin.flush().expect("flush");
    }

    fn read_message(&mut self) -> Json {
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("Content-Length:") {
                length = rest.trim().parse().expect("length");
            }
        }
        let mut buf = vec![0u8; length];
        self.reader.read_exact(&mut buf).expect("body");
        serde_json::from_slice(&buf).expect("json")
    }

    /// Read until a response to `command` (asserting success) or an event
    /// named `command` arrives; collect passed-over output events.
    fn wait_for(&mut self, kind: &str, name: &str, outputs: &mut String) -> Json {
        loop {
            let m = self.read_message();
            let label = if m["type"] == "event" {
                m["event"].as_str().unwrap_or("").to_string()
            } else {
                m["command"].as_str().unwrap_or("").to_string()
            };
            if m["type"] == "event" && label == "output" {
                outputs.push_str(m["body"]["output"].as_str().unwrap_or(""));
            }
            if m["type"] == kind && label == name {
                if kind == "response" {
                    assert_eq!(m["success"], json!(true), "request failed: {m}");
                }
                return m;
            }
        }
    }
}

#[test]
fn full_dap_session() {
    let dir = std::env::temp_dir().join("aura_dap_test");
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let program = dir.join("dap_session.aura");
    std::fs::write(
        &program,
        r#"
class Point {
    int x;
    int y;
    Point(int x, int y) { this.x = x; this.y = y; }
}
class Program {
    static int Main() {
        int a = 41;
        Point p = new Point(3, 4);
        a = a + 1;
        print("done "); print(a); println();
        return a;
    }
}
"#,
    )
    .expect("write program");

    let mut c = DapClient::start();
    let mut out = String::new();

    c.send("initialize", json!({}));
    let init = c.wait_for("response", "initialize", &mut out);
    assert_eq!(init["body"]["supportsConfigurationDoneRequest"], json!(true));
    c.wait_for("event", "initialized", &mut out);

    c.send(
        "launch",
        json!({"program": program.display().to_string(), "stopOnEntry": false}),
    );
    c.wait_for("response", "launch", &mut out);

    c.send(
        "setBreakpoints",
        json!({"source": {"path": program.display().to_string()},
               "breakpoints": [{"line": 11}]}),
    );
    let bps = c.wait_for("response", "setBreakpoints", &mut out);
    assert_eq!(bps["body"]["breakpoints"][0]["verified"], json!(true));

    c.send("configurationDone", json!({}));
    c.wait_for("response", "configurationDone", &mut out);

    let stopped = c.wait_for("event", "stopped", &mut out);
    assert_eq!(stopped["body"]["reason"], json!("breakpoint"));

    c.send("stackTrace", json!({"threadId": 1}));
    let st = c.wait_for("response", "stackTrace", &mut out);
    let frame = &st["body"]["stackFrames"][0];
    assert_eq!(frame["name"], json!("Program.Main"));
    assert_eq!(frame["line"], json!(11));
    let frame_id = frame["id"].clone();

    c.send("scopes", json!({"frameId": frame_id}));
    let scopes = c.wait_for("response", "scopes", &mut out);
    let vref = scopes["body"]["scopes"][0]["variablesReference"].clone();

    c.send("variables", json!({"variablesReference": vref}));
    let vars = c.wait_for("response", "variables", &mut out);
    let list = vars["body"]["variables"].as_array().expect("vars");
    let get = |n: &str| {
        list.iter()
            .find(|v| v["name"] == json!(n))
            .unwrap_or_else(|| panic!("variable {n} missing"))["value"]
            .clone()
    };
    assert_eq!(get("a"), json!("41"));
    assert_eq!(get("p"), json!("Point"));

    c.send("evaluate", json!({"expression": "p.y", "frameId": frame_id}));
    let eval = c.wait_for("response", "evaluate", &mut out);
    assert_eq!(eval["body"]["result"], json!("4"));

    c.send("next", json!({}));
    c.wait_for("response", "next", &mut out);
    let step_stop = c.wait_for("event", "stopped", &mut out);
    assert_eq!(step_stop["body"]["reason"], json!("step"));

    c.send("continue", json!({}));
    c.wait_for("response", "continue", &mut out);
    let exited = c.wait_for("event", "exited", &mut out);
    assert_eq!(exited["body"]["exitCode"], json!(0));
    c.wait_for("event", "terminated", &mut out);
    assert_eq!(out, "done 42\n", "program output must arrive as output events");

    c.send("disconnect", json!({}));
    c.wait_for("response", "disconnect", &mut out);
    let status = c.child.wait().expect("wait");
    assert!(status.success());
}
