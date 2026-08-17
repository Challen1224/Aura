//! Debug Adapter Protocol server (`aura dap`): source-level debugging of
//! Aura programs from VS Code, nvim-dap, or any DAP client, over stdio.
//!
//! Architecture: the main thread runs the protocol loop (stdout carries
//! the protocol, so program `print` output is redirected into DAP
//! `output` events). The VM runs on a worker thread with a channel-backed
//! [`aura_vm::Debugger`]: at every stop it emits a `stopped` event, then
//! serves stack/variables/evaluate queries from the paused VM until the
//! client resumes. Debugging drives the interpreter tier (the VM
//! suppresses JIT tier-up while a debugger is installed).
//!
//! Honest limits: single thread (id 1); breakpoints are line-only and
//! shared across files of a multi-file program; `evaluate` accepts
//! local-rooted inspection paths (`p.x`, `xs[0]`), not expressions;
//! `pause` while running is unsupported (no async interrupt); breakpoint
//! edits made while running apply at the next stop.

use anyhow::{Context, Result};
use aura_vm::debug::DebugView;
use aura_vm::{DebugCommand, DebugResume, DebugStop, Debugger, Vm};
use serde_json::{json, Value as Json};
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Messages from the protocol loop to the paused VM's debugger.
enum ToVm {
    SetBreakpoints(Vec<u32>),
    StackTrace,
    Variables(usize),
    Evaluate(usize, String),
    Resume(DebugResume),
}

/// Query replies from the paused VM (events go straight to the writer).
enum FromVm {
    Frames(Vec<(String, u32)>),
    Variables(Vec<(String, String)>),
    Evaluated(Result<String, String>),
}

/// Shared, sequenced protocol writer (framing + seq numbering).
struct DapWriter {
    out: std::io::Stdout,
    seq: u64,
}

impl DapWriter {
    fn send(&mut self, mut msg: Json) {
        self.seq += 1;
        msg["seq"] = json!(self.seq);
        let body = msg.to_string();
        let _ = write!(self.out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
        let _ = self.out.flush();
    }

    fn event(&mut self, name: &str, body: Json) {
        self.send(json!({"type": "event", "event": name, "body": body}));
    }

    fn respond(&mut self, req: &Json, success: bool, body: Json) {
        self.send(json!({
            "type": "response",
            "request_seq": req["seq"],
            "command": req["command"],
            "success": success,
            "body": body,
        }));
    }
}

type SharedWriter = Arc<Mutex<DapWriter>>;

/// Program-output shim: turns `print` bytes into DAP `output` events.
struct OutputEvents {
    writer: SharedWriter,
}

impl Write for OutputEvents {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf).to_string();
        if let Ok(mut w) = self.writer.lock() {
            w.event("output", json!({"category": "stdout", "output": text}));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The channel-backed debug controller running on the VM thread.
struct DapDebugger {
    writer: SharedWriter,
    rx: Receiver<ToVm>,
    tx: Sender<FromVm>,
    /// Source-line breakpoints currently installed (to diff against
    /// client updates).
    breaks: HashSet<u32>,
    first_stop: bool,
    stop_on_entry: bool,
    last_resume: DebugResume,
}

impl DapDebugger {
    fn stop_reason(&self) -> &'static str {
        if self.first_stop {
            "entry"
        } else if self.last_resume == DebugResume::Continue {
            "breakpoint"
        } else {
            "step"
        }
    }
}

impl Debugger for DapDebugger {
    fn on_stop(&mut self, view: &DebugView<'_>, _stop: &DebugStop) -> DebugCommand {
        let mut cmd = DebugCommand::default();
        // Breakpoint edits sent while running apply here, before the stop
        // is reported.
        while let Ok(ToVm::SetBreakpoints(lines)) = self.rx.try_recv() {
            apply_breakpoints(&mut self.breaks, lines, &mut cmd);
        }
        if self.first_stop && !self.stop_on_entry {
            self.first_stop = false;
            cmd.resume = Some(DebugResume::Continue);
            return cmd;
        }
        if let Ok(mut w) = self.writer.lock() {
            w.event(
                "stopped",
                json!({"reason": self.stop_reason(), "threadId": 1, "allThreadsStopped": true}),
            );
        }
        self.first_stop = false;
        loop {
            match self.rx.recv() {
                Ok(ToVm::SetBreakpoints(lines)) => {
                    apply_breakpoints(&mut self.breaks, lines, &mut cmd);
                }
                Ok(ToVm::StackTrace) => {
                    let _ = self.tx.send(FromVm::Frames(view.frames()));
                }
                Ok(ToVm::Variables(index)) => {
                    let _ = self.tx.send(FromVm::Variables(view.frame_locals(index)));
                }
                Ok(ToVm::Evaluate(index, expr)) => {
                    let _ = self.tx.send(FromVm::Evaluated(view.eval_path(index, &expr)));
                }
                Ok(ToVm::Resume(r)) => {
                    self.last_resume = r;
                    cmd.resume = Some(r);
                    return cmd;
                }
                Err(_) => {
                    // Protocol loop gone: shut the program down.
                    cmd.resume = Some(DebugResume::Quit);
                    return cmd;
                }
            }
        }
    }
}

fn apply_breakpoints(current: &mut HashSet<u32>, lines: Vec<u32>, cmd: &mut DebugCommand) {
    let new: HashSet<u32> = lines.into_iter().collect();
    for gone in current.difference(&new) {
        cmd.remove_breakpoints.push(*gone);
    }
    for added in new.difference(current) {
        cmd.add_breakpoints.push(*added);
    }
    *current = new;
}

/// Read one Content-Length-framed DAP message from stdin.
fn read_message(stdin: &mut impl BufRead) -> Result<Option<Json>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            return Ok(None); // EOF
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            length = Some(rest.trim().parse().context("bad Content-Length")?);
        }
    }
    let length = length.context("missing Content-Length header")?;
    let mut buf = vec![0u8; length];
    stdin.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

/// Serve DAP on stdio until the client disconnects.
pub fn serve() -> Result<()> {
    let writer: SharedWriter = Arc::new(Mutex::new(DapWriter {
        out: std::io::stdout(),
        seq: 0,
    }));
    let mut stdin = std::io::stdin().lock();

    // Session state.
    let mut program: Option<std::path::PathBuf> = None;
    let mut stop_on_entry = false;
    let mut pending_breaks: Vec<u32> = Vec::new();
    let mut to_vm: Option<Sender<ToVm>> = None;
    let mut from_vm: Option<Receiver<FromVm>> = None;
    let mut vm_thread: Option<std::thread::JoinHandle<()>> = None;

    while let Some(req) = read_message(&mut stdin)? {
        if req["type"] != "request" {
            continue;
        }
        let command = req["command"].as_str().unwrap_or("");
        match command {
            "initialize" => {
                writer.lock().unwrap().respond(
                    &req,
                    true,
                    json!({
                        "supportsConfigurationDoneRequest": true,
                        "supportsEvaluateForHovers": true,
                    }),
                );
                writer.lock().unwrap().event("initialized", json!({}));
            }
            "launch" => {
                let path = req["arguments"]["program"].as_str().unwrap_or("");
                stop_on_entry = req["arguments"]["stopOnEntry"].as_bool().unwrap_or(false);
                program = Some(std::path::PathBuf::from(path));
                writer.lock().unwrap().respond(&req, true, json!({}));
            }
            "setBreakpoints" => {
                let lines: Vec<u32> = req["arguments"]["breakpoints"]
                    .as_array()
                    .map(|bs| {
                        bs.iter()
                            .filter_map(|b| b["line"].as_u64().map(|l| l as u32))
                            .collect()
                    })
                    .unwrap_or_default();
                let verified: Vec<Json> = lines
                    .iter()
                    .map(|l| json!({"verified": true, "line": l}))
                    .collect();
                match &to_vm {
                    Some(tx) => {
                        let _ = tx.send(ToVm::SetBreakpoints(lines));
                    }
                    None => pending_breaks = lines,
                }
                writer
                    .lock()
                    .unwrap()
                    .respond(&req, true, json!({"breakpoints": verified}));
            }
            "configurationDone" => {
                let Some(path) = program.clone() else {
                    writer.lock().unwrap().respond(&req, false, json!({}));
                    continue;
                };
                let (tx_cmd, rx_cmd) = std::sync::mpsc::channel();
                let (tx_reply, rx_reply) = std::sync::mpsc::channel();
                let w = Arc::clone(&writer);
                let breaks = std::mem::take(&mut pending_breaks);
                let entry = stop_on_entry;
                vm_thread = Some(std::thread::spawn(move || {
                    run_vm(path, w, rx_cmd, tx_reply, breaks, entry);
                }));
                to_vm = Some(tx_cmd);
                from_vm = Some(rx_reply);
                writer.lock().unwrap().respond(&req, true, json!({}));
            }
            "threads" => {
                writer.lock().unwrap().respond(
                    &req,
                    true,
                    json!({"threads": [{"id": 1, "name": "main"}]}),
                );
            }
            "stackTrace" => {
                let frames = query(&to_vm, &from_vm, ToVm::StackTrace);
                let body = match frames {
                    Some(FromVm::Frames(fs)) => {
                        // DAP wants innermost first; the VM reports
                        // outermost first. Frame id encodes the VM index.
                        let source = program.as_ref().map(|p| p.display().to_string());
                        let list: Vec<Json> = fs
                            .iter()
                            .enumerate()
                            .rev()
                            .map(|(i, (name, line))| {
                                json!({
                                    "id": 1000 + i,
                                    "name": name,
                                    "line": line,
                                    "column": 1,
                                    "source": {"path": source},
                                })
                            })
                            .collect();
                        json!({"stackFrames": list, "totalFrames": list.len()})
                    }
                    _ => json!({"stackFrames": []}),
                };
                writer.lock().unwrap().respond(&req, true, body);
            }
            "scopes" => {
                let frame_id = req["arguments"]["frameId"].as_u64().unwrap_or(1000);
                writer.lock().unwrap().respond(
                    &req,
                    true,
                    json!({"scopes": [
                        {"name": "Locals", "variablesReference": frame_id, "expensive": false}
                    ]}),
                );
            }
            "variables" => {
                let vref = req["arguments"]["variablesReference"].as_u64().unwrap_or(1000);
                let index = (vref.saturating_sub(1000)) as usize;
                let vars = query(&to_vm, &from_vm, ToVm::Variables(index));
                let body = match vars {
                    Some(FromVm::Variables(vs)) => {
                        let list: Vec<Json> = vs
                            .iter()
                            .map(|(n, v)| {
                                json!({"name": n, "value": v, "variablesReference": 0})
                            })
                            .collect();
                        json!({"variables": list})
                    }
                    _ => json!({"variables": []}),
                };
                writer.lock().unwrap().respond(&req, true, body);
            }
            "evaluate" => {
                let expr = req["arguments"]["expression"].as_str().unwrap_or("");
                let frame_id = req["arguments"]["frameId"].as_u64().unwrap_or(u64::MAX);
                let index = if frame_id == u64::MAX {
                    usize::MAX // innermost, resolved VM-side via frames len
                } else {
                    (frame_id.saturating_sub(1000)) as usize
                };
                // usize::MAX means "innermost": translate via a frames query.
                let index = if index == usize::MAX {
                    match query(&to_vm, &from_vm, ToVm::StackTrace) {
                        Some(FromVm::Frames(fs)) if !fs.is_empty() => fs.len() - 1,
                        _ => 0,
                    }
                } else {
                    index
                };
                let reply = query(&to_vm, &from_vm, ToVm::Evaluate(index, expr.to_string()));
                match reply {
                    Some(FromVm::Evaluated(Ok(v))) => writer
                        .lock()
                        .unwrap()
                        .respond(&req, true, json!({"result": v, "variablesReference": 0})),
                    Some(FromVm::Evaluated(Err(e))) => {
                        writer.lock().unwrap().respond(&req, false, json!({"result": e}))
                    }
                    _ => writer.lock().unwrap().respond(&req, false, json!({})),
                }
            }
            "continue" => {
                resume(&to_vm, DebugResume::Continue);
                writer
                    .lock()
                    .unwrap()
                    .respond(&req, true, json!({"allThreadsContinued": true}));
            }
            "next" => {
                resume(&to_vm, DebugResume::Next);
                writer.lock().unwrap().respond(&req, true, json!({}));
            }
            "stepIn" => {
                resume(&to_vm, DebugResume::Step);
                writer.lock().unwrap().respond(&req, true, json!({}));
            }
            "stepOut" => {
                resume(&to_vm, DebugResume::Out);
                writer.lock().unwrap().respond(&req, true, json!({}));
            }
            "disconnect" | "terminate" => {
                resume(&to_vm, DebugResume::Quit);
                writer.lock().unwrap().respond(&req, true, json!({}));
                break;
            }
            other => {
                // Unknown/unsupported (e.g. pause): fail politely.
                writer
                    .lock()
                    .unwrap()
                    .respond(&req, false, json!({"error": format!("unsupported: {other}")}));
            }
        }
    }
    if let Some(t) = vm_thread {
        let _ = t.join();
    }
    Ok(())
}

fn resume(to_vm: &Option<Sender<ToVm>>, r: DebugResume) {
    if let Some(tx) = to_vm {
        let _ = tx.send(ToVm::Resume(r));
    }
}

fn query(to_vm: &Option<Sender<ToVm>>, from_vm: &Option<Receiver<FromVm>>, q: ToVm) -> Option<FromVm> {
    let tx = to_vm.as_ref()?;
    let rx = from_vm.as_ref()?;
    tx.send(q).ok()?;
    rx.recv().ok()
}

/// VM thread body: compile, install the DAP debugger, run, report end.
fn run_vm(
    path: std::path::PathBuf,
    writer: SharedWriter,
    rx: Receiver<ToVm>,
    tx: Sender<FromVm>,
    breaks: Vec<u32>,
    stop_on_entry: bool,
) {
    let finish = |code: i64, message: Option<String>| {
        if let Ok(mut w) = writer.lock() {
            if let Some(m) = message {
                w.event("output", json!({"category": "stderr", "output": m}));
            }
            w.event("exited", json!({"exitCode": code}));
            w.event("terminated", json!({}));
        }
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => return finish(1, Some(format!("reading {}: {e}\n", path.display()))),
    };
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("program")
        .to_string();
    let module = match aura_compiler::compile(&source, &name) {
        Ok(m) => std::sync::Arc::new(m),
        Err(e) => return finish(1, Some(format!("{e}\n"))),
    };
    let mut vm = Vm::new(module);
    // The debugger diffs later client updates against what it believes is
    // installed, so its seed must match the initial install exactly.
    let breaks_set: HashSet<u32> = breaks.iter().copied().collect();
    for line in breaks {
        vm.add_breakpoint(line);
    }
    vm.set_output(Box::new(OutputEvents {
        writer: Arc::clone(&writer),
    }));
    vm.set_debugger(Box::new(DapDebugger {
        writer: Arc::clone(&writer),
        rx,
        tx,
        breaks: breaks_set,
        first_stop: true,
        stop_on_entry,
        last_resume: DebugResume::Continue,
    }));
    match vm.run() {
        Ok(_) => finish(0, None),
        Err(e) if format!("{e}").contains("debugger: quit") => finish(0, None),
        Err(e) => {
            let mut msg = format!("runtime error: {e}\n");
            for line in vm.stack_trace() {
                msg.push_str("  ");
                msg.push_str(&line);
                msg.push('\n');
            }
            finish(1, Some(msg));
        }
    }
}
