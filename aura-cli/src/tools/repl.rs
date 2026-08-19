//! `aura repl` — interactive session.
//!
//! Aura has no top-level statements, so the REPL synthesizes a program: the
//! declarations you enter accumulate at top level, the statements accumulate
//! inside a `Program.Main`, and every input recompiles and reruns the whole
//! session. Output is captured, and only the part the new input produced is
//! shown — for deterministic programs this is indistinguishable from
//! incremental evaluation (the VM is deterministic and single-threaded, so
//! programs without external input replay identically).
//!
//! * An input whose braces/parens stay open continues on the next line.
//! * Inputs starting with a declaration keyword (`class`, `record`,
//!   `interface`, `enum`, `type`, `newtype`, `extension`, ...) become
//!   top-level declarations.
//! * An input without a trailing `;` or `}` is an expression: it is wrapped
//!   in `print(...)` so its value echoes.
//! * Inputs that fail to compile — or fail at runtime — are reported and
//!   discarded; the session state stays as it was.
//!
//! Limitation, stated plainly: `Console.ReadLine()` shares stdin with the
//! REPL itself and each run replays the whole session, so interactive input
//! does not belong in a REPL session.

use anyhow::Result;
use aura_compiler::compile_files;
use aura_vm::Vm;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

/// Captured program output shared with the VM.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

const DECL_KEYWORDS: &[&str] = &[
    "class", "interface", "enum", "record", "type", "newtype", "extension", "abstract", "sealed",
    "static",
];

/// Session state: accumulated snippets and the output high-water mark.
struct Session {
    decls: Vec<String>,
    stmts: Vec<String>,
    /// Bytes of output the last successful run produced.
    seen: usize,
    jit: bool,
}

impl Session {
    fn program(&self, pending: Option<&str>, pending_decl: bool) -> String {
        let mut decls = self.decls.join("\n");
        let mut stmts = self.stmts.join("\n        ");
        match (pending, pending_decl) {
            (Some(text), true) => {
                decls.push('\n');
                decls.push_str(text);
            }
            (Some(text), false) => {
                stmts.push_str("\n        ");
                stmts.push_str(text);
            }
            (None, _) => {}
        }
        format!(
            "{decls}\nclass Program {{\n    static void Main() {{\n        {stmts}\n    }}\n}}\n"
        )
    }

    /// Compile and run the session plus `pending`; on success commit it.
    /// Returns the text to show the user.
    fn eval(&mut self, pending: &str, pending_decl: bool) -> String {
        let source = self.program(Some(pending), pending_decl);
        let files = vec![("repl".to_string(), source)];
        let module = match compile_files(&files, "repl") {
            Ok(m) => Arc::new(m),
            Err(e) => return format!("error: {e}"),
        };
        let buf = SharedBuf::default();
        let mut vm = Vm::new(module);
        vm.set_output(Box::new(buf.clone()));
        if self.jit {
            vm.enable_jit();
        }
        let result = vm.run();
        let output = buf.0.lock().unwrap();
        // Show everything this input produced beyond the last committed run,
        // whether it succeeded or not.
        let new_output = String::from_utf8_lossy(&output[self.seen.min(output.len())..]).into_owned();
        match result {
            Ok(_) => {
                self.seen = output.len();
                if pending_decl {
                    self.decls.push(pending.to_string());
                } else {
                    self.stmts.push(pending.to_string());
                }
                new_output
            }
            Err(e) => {
                // Discarded: the session replays without it next time, so the
                // high-water mark stays where the last good run left it.
                format!("{new_output}runtime error: {e}\n")
            }
        }
    }
}

/// True when the input opens a top-level declaration.
fn is_decl(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or("");
    DECL_KEYWORDS.contains(&first)
}

/// Net bracket depth of `text` (strings and comments ignored), used to
/// detect unfinished multi-line input.
fn net_depth(text: &str) -> i32 {
    let mut depth = 0i32;
    let mut state = super::fmt::LineState::Code;
    for line in text.lines() {
        state = super::fmt::scan_line(line, state, &mut depth);
    }
    depth
}

/// Run the interactive loop over stdin/stdout.
pub fn run(jit: bool) -> Result<()> {
    let mut session = Session { decls: Vec::new(), stmts: Vec::new(), seen: 0, jit };
    let interactive = is_terminal(&std::io::stdin());
    if interactive {
        println!("Aura {} — :help for commands, :quit to exit", env!("CARGO_PKG_VERSION"));
    }
    let stdin = std::io::stdin();
    let mut pending = String::new();
    loop {
        if interactive {
            print!("{}", if pending.is_empty() { "aura> " } else { "  ... " });
            std::io::stdout().flush()?;
        }
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // EOF
        }
        pending.push_str(&line);
        if net_depth(&pending) > 0 {
            continue; // unfinished block — keep reading
        }
        let input = std::mem::take(&mut pending);
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            ":quit" | ":q" | ":exit" => break,
            ":clear" => {
                session = Session { decls: Vec::new(), stmts: Vec::new(), seen: 0, jit };
                println!("session cleared");
                continue;
            }
            ":show" => {
                println!("{}", session.program(None, false));
                continue;
            }
            ":help" | ":h" => {
                println!(
                    "declarations (class, record, enum, ...) accumulate at top level;\n\
                     statements run inside Program.Main; a bare expression prints its value.\n\
                     :show   print the synthesized program\n\
                     :clear  reset the session\n\
                     :quit   exit"
                );
                continue;
            }
            _ => {}
        }
        let decl = is_decl(input);
        let stmt = if !decl && !input.ends_with(';') && !input.ends_with('}') {
            format!("print({input}); println();")
        } else {
            input.to_string()
        };
        print!("{}", session.eval(&stmt, decl));
    }
    Ok(())
}

/// Whether stdin is a terminal (prompts and banner are suppressed when
/// input is piped, keeping the REPL scriptable).
fn is_terminal(stdin: &std::io::Stdin) -> bool {
    use std::io::IsTerminal;
    stdin.is_terminal()
}
