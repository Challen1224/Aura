//! Aura CLI — compile and run .aura programs.

use anyhow::{Context, Result};

mod dap;
use aura_compiler::compile_files;
use aura_vm::Vm;
use clap::{Parser as ClapParser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(ClapParser)]
#[command(name = "aura", version, about = "Aura language CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile source files and print the resulting bytecode module.
    Compile {
        /// Source file paths. With several files, each file is a module:
        /// all declarations share one namespace, and `internal` members
        /// are only accessible from classes in the same file.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Compile and run source files.
    Run {
        /// Source file paths (see `compile` for multi-file semantics).
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Enable the x86-64 JIT (methods are compiled to native code once hot).
        #[arg(long)]
        jit: bool,
        /// Full-heap GC threshold in bytes (default 64KiB, grows adaptively).
        #[arg(long, value_name = "BYTES")]
        gc_threshold: Option<usize>,
        /// Nursery size in bytes (default: derived, capped at 64KiB).
        #[arg(long, value_name = "BYTES")]
        gc_nursery: Option<usize>,
        /// Hard heap limit in bytes; exceeding it after a full collection
        /// is a runtime error.
        #[arg(long, value_name = "BYTES")]
        gc_max_heap: Option<usize>,
        /// Collector disposition: throughput (fewer, bigger pauses),
        /// balanced, or latency (frequent, small minor pauses).
        #[arg(long, value_name = "MODE")]
        gc_mode: Option<String>,
        /// Best-effort soft target for minor pause times, in milliseconds.
        #[arg(long, value_name = "MS")]
        gc_pause_target_ms: Option<u64>,
        /// Concurrent GC: majors become background marking cycles with a
        /// brief snapshot pause and chunked sweeps (reclamation timing
        /// becomes marker-dependent; default is deterministic
        /// stop-the-world).
        #[arg(long)]
        gc_concurrent: bool,
        /// Print collector statistics to stderr after the program exits.
        #[arg(long)]
        gc_stats: bool,
    },
    /// Run source files under the interactive source-level debugger
    /// (interpreter tier; breakpoints, stepping, variable inspection).
    Debug {
        /// Source file paths (see `compile` for multi-file semantics).
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Breakpoint lines to set before the program starts.
        #[arg(long = "break", value_name = "LINE")]
        breakpoints: Vec<u32>,
    },
    /// Serve the Debug Adapter Protocol on stdio (for VS Code, nvim-dap,
    /// or any DAP client). The client supplies the program via `launch`.
    Dap,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile { paths } => {
            let (files, name) = load(&paths)?;
            let module = compile_files(&files, &name)?;
            println!("{:#?}", module);
            Ok(())
        }
        Command::Run {
            paths,
            jit,
            gc_threshold,
            gc_nursery,
            gc_max_heap,
            gc_mode,
            gc_pause_target_ms,
            gc_concurrent,
            gc_stats,
        } => {
            let (files, name) = load(&paths)?;
            let module = Arc::new(compile_files(&files, &name)?);
            let mut vm = Vm::new(module);
            if jit {
                vm.enable_jit();
            }
            if let Some(mode) = gc_mode.as_deref() {
                let mode = match mode {
                    "throughput" => aura_vm::heap::GcMode::Throughput,
                    "balanced" => aura_vm::heap::GcMode::Balanced,
                    "latency" => aura_vm::heap::GcMode::Latency,
                    other => anyhow::bail!(
                        "unknown --gc-mode `{other}` (throughput | balanced | latency)"
                    ),
                };
                vm.set_gc_mode(mode);
            }
            if let Some(bytes) = gc_threshold {
                vm.set_gc_threshold(bytes);
            }
            if let Some(bytes) = gc_nursery {
                vm.set_gc_nursery_size(Some(bytes));
            }
            if let Some(bytes) = gc_max_heap {
                vm.set_gc_max_heap(Some(bytes));
            }
            if gc_pause_target_ms.is_some() {
                vm.set_gc_pause_target_ms(gc_pause_target_ms);
            }
            if gc_concurrent {
                vm.set_gc_concurrent(true);
            }
            let result = vm.run();
            if gc_stats {
                let s = vm.gc_stats();
                eprintln!(
                    "gc: {} collections ({} minor, {} major), {} live objects / {} bytes, \
{} allocated total, {} bytes freed",
                    s.collections,
                    s.minor_collections,
                    s.major_collections,
                    s.live_objects,
                    s.live_bytes,
                    s.total_allocations,
                    s.bytes_freed
                );
                eprintln!(
                    "gc: pauses minor {:?} total / {:?} max, major {:?} total / {:?} max; \
threshold {} bytes, nursery {} bytes",
                    s.minor_pause_total,
                    s.max_minor_pause,
                    s.major_pause_total,
                    s.max_major_pause,
                    s.threshold,
                    s.nursery_size
                );
                if s.concurrent_cycles > 0 {
                    eprintln!(
                        "gc: {} concurrent cycles, {:?} background mark time (off-thread)",
                        s.concurrent_cycles, s.concurrent_mark_total
                    );
                }
            }
            if result.is_err() {
                for line in vm.stack_trace() {
                    eprintln!("  {line}");
                }
            }
            result.context("runtime error")?;
            Ok(())
        }
        Command::Dap => dap::serve(),
        Command::Debug { paths, breakpoints } => {
            let (files, name) = load(&paths)?;
            let module = Arc::new(compile_files(&files, &name)?);
            let mut vm = Vm::new(module);
            for line in breakpoints {
                vm.add_breakpoint(line);
            }
            vm.set_debugger(Box::new(CliDebugger::default()));
            match vm.run() {
                Ok(_) => {
                    println!("program finished");
                    Ok(())
                }
                Err(e) if format!("{e}").contains("debugger: quit") => Ok(()),
                Err(e) => {
                    for line in vm.stack_trace() {
                        eprintln!("  {line}");
                    }
                    Err(e).context("runtime error")
                }
            }
        }
    }
}

/// Interactive command-line debug controller: a small REPL served at
/// every stop. `help` lists commands.
#[derive(Default)]
struct CliDebugger;

impl aura_vm::Debugger for CliDebugger {
    fn on_stop(
        &mut self,
        view: &aura_vm::debug::DebugView<'_>,
        stop: &aura_vm::DebugStop,
    ) -> aura_vm::DebugCommand {
        use std::io::{BufRead, Write};
        println!("stopped at line {} in {} (pc {})", stop.line, stop.method, stop.pc);
        for (path, value) in &stop.watches {
            println!("watch {path} = {value}");
        }
        let mut cmd = aura_vm::DebugCommand::default();
        let stdin = std::io::stdin();
        loop {
            print!("(aura-db) ");
            let _ = std::io::stdout().flush();
            let mut input = String::new();
            match stdin.lock().read_line(&mut input) {
                Ok(0) | Err(_) => {
                    // EOF: quit cleanly.
                    cmd.resume = Some(aura_vm::DebugResume::Quit);
                    return cmd;
                }
                Ok(_) => {}
            }
            let mut parts = input.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some("c") | Some("continue"), _) => {
                    cmd.resume = Some(aura_vm::DebugResume::Continue);
                    return cmd;
                }
                (Some("s") | Some("step"), _) => {
                    cmd.resume = Some(aura_vm::DebugResume::Step);
                    return cmd;
                }
                (Some("n") | Some("next"), _) => {
                    cmd.resume = Some(aura_vm::DebugResume::Next);
                    return cmd;
                }
                (Some("o") | Some("out") | Some("finish"), _) => {
                    cmd.resume = Some(aura_vm::DebugResume::Out);
                    return cmd;
                }
                (Some("q") | Some("quit"), _) => {
                    cmd.resume = Some(aura_vm::DebugResume::Quit);
                    return cmd;
                }
                (Some("b") | Some("break"), Some(arg)) => match arg.parse::<u32>() {
                    Ok(line) => {
                        cmd.add_breakpoints.push(line);
                        println!("breakpoint at line {line}");
                    }
                    Err(_) => println!("usage: b <line>"),
                },
                (Some("d") | Some("delete"), Some(arg)) => match arg.parse::<u32>() {
                    Ok(line) => {
                        cmd.remove_breakpoints.push(line);
                        println!("breakpoint at line {line} removed");
                    }
                    Err(_) => println!("usage: d <line>"),
                },
                (Some("bb"), Some(label)) => {
                    match parts.next().and_then(|a| a.parse::<u32>().ok()) {
                        Some(op) if view.resolve_method_label(label) => {
                            cmd.add_bytecode_breakpoints.push((label.to_string(), op));
                            println!("bytecode breakpoint at {label} op {op}");
                        }
                        Some(_) => println!("no method `{label}`"),
                        None => println!("usage: bb <Class.Method> <op-index>"),
                    }
                }
                (Some("w") | Some("watch"), Some(path)) => {
                    match view.eval_path(stop.depth - 1, path) {
                        Ok(v) => {
                            println!("watch {path} = {v}");
                            cmd.add_watches.push(path.to_string());
                        }
                        Err(e) => println!("{e}"),
                    }
                }
                (Some("unw") | Some("unwatch"), Some(path)) => {
                    cmd.remove_watches.push(path.to_string());
                    println!("unwatched {path}");
                }
                (Some("dis"), _) => {
                    for (i, op, current) in view.disassemble(6) {
                        let marker = if current { "->" } else { "  " };
                        println!("{marker} {i:4}  {op}");
                    }
                }
                (Some("locals"), _) => {
                    if stop.locals.is_empty() {
                        println!("no named locals");
                    }
                    for (name, value) in &stop.locals {
                        println!("{name} = {value}");
                    }
                }
                (Some("p") | Some("print"), Some(path)) => {
                    match view.eval_path(stop.depth - 1, path) {
                        Ok(value) => println!("{path} = {value}"),
                        Err(e) => println!("{e}"),
                    }
                }
                (Some("bt") | Some("backtrace"), _) => {
                    for line in &stop.backtrace {
                        println!("  {line}");
                    }
                }
                (Some("h") | Some("help"), _) => {
                    println!(
                        "commands: c(ontinue), s(tep into), n(ext / step over), \
o(ut / finish), b <line>, d <line>, bb <Class.Method> <op>, \
w <path>, unw <path>, locals, p <path>, dis, bt, q(uit)"
                    );
                }
                (None, _) => {}
                (Some(other), _) => println!("unknown command `{other}` (h for help)"),
            }
        }
    }
}

/// Read every source file; the program is named after the first file.
fn load(paths: &[PathBuf]) -> Result<(Vec<(String, String)>, String)> {
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let source = fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        files.push((module_name(path)?.to_string(), source));
    }
    let name = module_name(&paths[0])?.to_string();
    Ok((files, name))
}

fn module_name(path: &Path) -> Result<&str> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .context("invalid source file name")
}
