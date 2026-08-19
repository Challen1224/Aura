//! Aura CLI — compile, run, and develop .aura programs.

use anyhow::{bail, Context, Result};

mod dap;
mod project;
mod tools;

use aura_compiler::compile_files;
use aura_vm::Vm;
use clap::{Parser as ClapParser, Subcommand};
use project::Project;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(ClapParser)]
#[command(name = "aura", version, about = "Aura language CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// VM options shared by `run` and `test`. Project `[run]` settings apply
/// where the flag is absent.
#[derive(clap::Args, Default)]
struct VmOpts {
    /// Enable the x86-64 JIT (methods are compiled to native code once hot).
    #[arg(long)]
    jit: bool,
    /// Full-heap GC threshold in bytes (default 64KiB, grows adaptively).
    #[arg(long, value_name = "BYTES")]
    gc_threshold: Option<usize>,
    /// Nursery size in bytes (default: derived, capped at 64KiB).
    #[arg(long, value_name = "BYTES")]
    gc_nursery: Option<usize>,
    /// Hard heap limit in bytes; exceeding it after a full collection is a
    /// runtime error.
    #[arg(long, value_name = "BYTES")]
    gc_max_heap: Option<usize>,
    /// Collector disposition: throughput (fewer, bigger pauses), balanced,
    /// or latency (frequent, small minor pauses).
    #[arg(long, value_name = "MODE")]
    gc_mode: Option<String>,
    /// Best-effort soft target for minor pause times, in milliseconds.
    #[arg(long, value_name = "MS")]
    gc_pause_target_ms: Option<u64>,
    /// Concurrent GC: majors become background marking cycles with a brief
    /// snapshot pause and chunked sweeps (reclamation timing becomes
    /// marker-dependent; default is deterministic stop-the-world).
    #[arg(long)]
    gc_concurrent: bool,
    /// Print collector statistics to stderr after the program exits.
    #[arg(long)]
    gc_stats: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new project: aura.toml, src/main.aura, tests/.
    Init {
        /// Project name; with a name a new directory is created, without
        /// one the current directory is initialized in place.
        name: Option<String>,
    },
    /// Compile source files and print the resulting bytecode module.
    Compile {
        /// Source file paths. With several files, each file is a module:
        /// all declarations share one namespace, and `internal` members
        /// are only accessible from classes in the same file.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Compile the project to a binary bytecode module (build/<name>.aurac).
    Build,
    /// Compile and run: source files, a compiled .aurac module, or — with
    /// no paths — the aura.toml project in the current directory.
    Run {
        /// Source file paths (see `compile` for multi-file semantics), or a
        /// single .aurac file, or nothing to run the current project.
        paths: Vec<PathBuf>,
        #[command(flatten)]
        vm_opts: VmOpts,
        /// Re-compile and re-run whenever a source file changes.
        #[arg(long)]
        watch: bool,
        /// Print compile/run timing and JIT counters to stderr.
        #[arg(long)]
        stats: bool,
    },
    /// Run the project's tests: each tests/*.aura file is its own program,
    /// compiled with every source file except the entry; it passes when its
    /// Main exits without a runtime error.
    Test {
        /// Run only tests whose file name contains this substring.
        filter: Option<String>,
        #[command(flatten)]
        vm_opts: VmOpts,
    },
    /// Start an interactive session (declarations accumulate, statements
    /// run in a synthesized Main, expressions print their value).
    Repl {
        /// Enable the x86-64 JIT for session runs.
        #[arg(long)]
        jit: bool,
    },
    /// Normalize indentation and trailing whitespace (token-safe: refuses
    /// any edit that would change the token stream).
    Fmt {
        /// Files to format; with none, the project's sources and tests.
        paths: Vec<PathBuf>,
        /// Report files that would change without rewriting them; exits
        /// non-zero if any would.
        #[arg(long)]
        check: bool,
    },
    /// Static checks: unused locals, unreachable code, empty catch blocks.
    /// Exits non-zero when anything is reported.
    Lint {
        /// Files to lint; with none, the project's sources and tests.
        paths: Vec<PathBuf>,
    },
    /// Generate Markdown API documentation from declarations and `///`
    /// comments.
    Doc {
        /// Files to document; with none, the project's sources.
        paths: Vec<PathBuf>,
        /// Write to a file instead of stdout.
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Run source files under the interactive source-level debugger
    /// (interpreter tier; breakpoints, stepping, variable inspection).
    Debug {
        /// Source file paths; with none, the current project.
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
        Command::Init { name } => cmd_init(name),
        Command::Compile { paths } => {
            let (files, name) = load(&paths)?;
            let module = compile_rendered(&files, &name)?;
            println!("{:#?}", module);
            Ok(())
        }
        Command::Build => cmd_build(),
        Command::Run { paths, vm_opts, watch, stats } => cmd_run(paths, vm_opts, watch, stats),
        Command::Test { filter, vm_opts } => cmd_test(filter, vm_opts),
        Command::Repl { jit } => tools::repl::run(jit),
        Command::Fmt { paths, check } => cmd_fmt(paths, check),
        Command::Lint { paths } => cmd_lint(paths),
        Command::Doc { paths, output } => cmd_doc(paths, output),
        Command::Dap => dap::serve(),
        Command::Debug { paths, breakpoints } => cmd_debug(paths, breakpoints),
    }
}

// ---------------------------------------------------------------------------
// Project commands
// ---------------------------------------------------------------------------

fn cmd_init(name: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (dir, name) = match name {
        Some(name) => (cwd.join(&name), name),
        None => {
            let name = cwd
                .file_name()
                .and_then(|s| s.to_str())
                .context("cannot derive a project name from the current directory")?
                .to_string();
            (cwd, name)
        }
    };
    fs::create_dir_all(&dir)?;
    project::init(&name, &dir)?;
    println!("created project `{name}` in {}", dir.display());
    println!("  aura run    # from the project directory");
    println!("  aura test");
    Ok(())
}

fn cmd_build() -> Result<()> {
    let project = Project::find()?;
    let sources = project.sources()?;
    let (files, name) = load(&sources)?;
    let module = compile_rendered(&files, &name)?;
    let bytes = aura_bytecode::encode::encode(&module).context("encoding module")?;
    let out = project.output_path();
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
    println!("built {} ({} bytes)", out.display(), bytes.len());
    Ok(())
}

fn cmd_run(paths: Vec<PathBuf>, vm_opts: VmOpts, watch: bool, stats: bool) -> Result<()> {
    // Resolve what to run: explicit paths, a compiled module, or the project.
    let project = if paths.is_empty() { Some(Project::find()?) } else { None };
    let run_defaults = project.as_ref().map(|p| &p.manifest.run);

    // A single .aurac argument runs the pre-compiled module directly.
    if paths.len() == 1 && paths[0].extension().is_some_and(|e| e == "aurac") {
        if watch {
            bail!("--watch requires source files (a compiled module has no sources to watch)");
        }
        let bytes = fs::read(&paths[0]).with_context(|| format!("reading {}", paths[0].display()))?;
        let module =
            aura_bytecode::encode::decode(&bytes[..]).context("decoding compiled module")?;
        let start = Instant::now();
        let result = run_module(Arc::new(module), &vm_opts, run_defaults, stats, None);
        if stats {
            eprintln!("stats: total {:?}", start.elapsed());
        }
        return result;
    }

    if paths.iter().any(|p| p.extension().is_some_and(|e| e == "aurac")) {
        bail!("a compiled .aurac module must be the only argument");
    }
    let source_paths = match &project {
        Some(p) => p.sources()?,
        None => paths.clone(),
    };

    loop {
        let iteration = || -> Result<()> {
            let compile_start = Instant::now();
            let (files, name) = load(&source_paths)?;
            let module = Arc::new(compile_rendered(&files, &name)?);
            let compile_time = compile_start.elapsed();
            run_module(module, &vm_opts, run_defaults, stats, stats.then_some(compile_time))
        };
        let result = iteration();
        if !watch {
            return result;
        }
        if let Err(e) = result {
            // In watch mode failures are reported, not fatal — fix and save.
            eprintln!("{e:#}");
        }
        eprintln!("[watch] waiting for changes... (Ctrl-C to stop)");
        wait_for_change(&source_paths)?;
        eprintln!("[watch] re-running");
    }
}

fn cmd_test(filter: Option<String>, vm_opts: VmOpts) -> Result<()> {
    let project = Project::find()?;
    let tests = project.test_files()?;
    let tests: Vec<_> = tests
        .into_iter()
        .filter(|t| match &filter {
            Some(f) => t.file_name().is_some_and(|n| n.to_string_lossy().contains(f)),
            None => true,
        })
        .collect();
    if tests.is_empty() {
        println!("no tests found under {}", project.root.join("tests").display());
        return Ok(());
    }

    // Test programs share the sources minus the entry (each test provides
    // its own Program.Main).
    let entry = project.root.join(&project.manifest.build.entry);
    let shared: Vec<PathBuf> =
        project.sources()?.into_iter().filter(|p| *p != entry).collect();

    let mut failures = 0;
    for test in &tests {
        let display = test
            .strip_prefix(&project.root)
            .unwrap_or(test)
            .display()
            .to_string();
        let mut paths = vec![test.clone()];
        paths.extend(shared.iter().cloned());
        let outcome = (|| -> Result<()> {
            let (files, name) = load(&paths)?;
            let module = Arc::new(compile_rendered(&files, &name)?);
            run_module(module, &vm_opts, Some(&project.manifest.run), false, None)
        })();
        match outcome {
            Ok(()) => println!("PASS {display}"),
            Err(e) => {
                failures += 1;
                println!("FAIL {display}");
                for line in format!("{e:#}").lines() {
                    println!("     {line}");
                }
            }
        }
    }
    println!("test result: {} passed, {} failed", tests.len() - failures, failures);
    if failures > 0 {
        bail!("{failures} test(s) failed");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tooling commands
// ---------------------------------------------------------------------------

/// The file set for fmt/lint/doc: explicit paths, or the project's sources
/// (and, when `include_tests`, its tests).
fn tool_paths(paths: Vec<PathBuf>, include_tests: bool) -> Result<Vec<PathBuf>> {
    if !paths.is_empty() {
        return Ok(paths);
    }
    let project = Project::find()?;
    let mut all = project.sources()?;
    if include_tests {
        all.extend(project.test_files()?);
    }
    Ok(all)
}

fn cmd_fmt(paths: Vec<PathBuf>, check: bool) -> Result<()> {
    let mut would_change = 0;
    for path in tool_paths(paths, true)? {
        let source =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let formatted = tools::fmt::format_source(&source)
            .with_context(|| format!("formatting {}", path.display()))?;
        if formatted != source {
            would_change += 1;
            if check {
                println!("would reformat {}", path.display());
            } else {
                fs::write(&path, formatted)
                    .with_context(|| format!("writing {}", path.display()))?;
                println!("reformatted {}", path.display());
            }
        }
    }
    if check && would_change > 0 {
        bail!("{would_change} file(s) would be reformatted");
    }
    Ok(())
}

fn cmd_lint(paths: Vec<PathBuf>) -> Result<()> {
    let mut total = 0;
    for path in tool_paths(paths, true)? {
        let source =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let program = parse_source(&source)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        for diag in tools::lint::lint_program(&program) {
            total += 1;
            println!("{}:{}: warning: {}", path.display(), diag.line, diag.message);
        }
    }
    if total > 0 {
        bail!("{total} warning(s)");
    }
    println!("no warnings");
    Ok(())
}

fn cmd_doc(paths: Vec<PathBuf>, output: Option<PathBuf>) -> Result<()> {
    let mut out = String::new();
    for path in tool_paths(paths, false)? {
        let source =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let program = parse_source(&source)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        out.push_str(&tools::doc::document(module_name(&path)?, &source, &program));
    }
    match output {
        Some(file) => {
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file, &out).with_context(|| format!("writing {}", file.display()))?;
            println!("wrote {}", file.display());
        }
        None => print!("{out}"),
    }
    Ok(())
}

fn cmd_debug(paths: Vec<PathBuf>, breakpoints: Vec<u32>) -> Result<()> {
    let source_paths = if paths.is_empty() { Project::find()?.sources()? } else { paths };
    let (files, name) = load(&source_paths)?;
    let module = Arc::new(compile_rendered(&files, &name)?);
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

// ---------------------------------------------------------------------------
// Shared VM plumbing
// ---------------------------------------------------------------------------

/// Configure and run a VM over `module`. `defaults` supplies project
/// `[run]` settings for options the flags leave unset.
fn run_module(
    module: Arc<aura_bytecode::Module>,
    opts: &VmOpts,
    defaults: Option<&project::RunSection>,
    gc_stats_flag: bool,
    compile_time: Option<Duration>,
) -> Result<()> {
    let mut vm = Vm::new(module);
    let jit = opts.jit || defaults.is_some_and(|d| d.jit);
    if jit {
        vm.enable_jit();
    }
    let mode = opts
        .gc_mode
        .clone()
        .or_else(|| defaults.and_then(|d| d.gc_mode.clone()));
    if let Some(mode) = mode.as_deref() {
        let mode = match mode {
            "throughput" => aura_vm::heap::GcMode::Throughput,
            "balanced" => aura_vm::heap::GcMode::Balanced,
            "latency" => aura_vm::heap::GcMode::Latency,
            other => bail!("unknown gc mode `{other}` (throughput | balanced | latency)"),
        };
        vm.set_gc_mode(mode);
    }
    if let Some(bytes) = opts.gc_threshold.or(defaults.and_then(|d| d.gc_threshold)) {
        vm.set_gc_threshold(bytes);
    }
    if let Some(bytes) = opts.gc_nursery.or(defaults.and_then(|d| d.gc_nursery)) {
        vm.set_gc_nursery_size(Some(bytes));
    }
    if let Some(bytes) = opts.gc_max_heap.or(defaults.and_then(|d| d.gc_max_heap)) {
        vm.set_gc_max_heap(Some(bytes));
    }
    if opts.gc_pause_target_ms.is_some() {
        vm.set_gc_pause_target_ms(opts.gc_pause_target_ms);
    }
    if opts.gc_concurrent {
        vm.set_gc_concurrent(true);
    }

    let run_start = Instant::now();
    let result = vm.run();
    let run_time = run_start.elapsed();

    if let Some(compile_time) = compile_time {
        eprintln!(
            "stats: compile {compile_time:?}, run {run_time:?}, jit methods compiled: {}",
            vm.jit_compiled_count()
        );
    }
    if gc_stats_flag || opts.gc_stats {
        print_gc_stats(&vm);
    }
    if result.is_err() {
        for line in vm.stack_trace() {
            eprintln!("  {line}");
        }
    }
    result.context("runtime error")?;
    Ok(())
}

fn print_gc_stats(vm: &Vm) {
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

/// Block until any watched file's mtime changes (1s polling — simple,
/// portable, dependency-free).
fn wait_for_change(paths: &[PathBuf]) -> Result<()> {
    let snapshot = |paths: &[PathBuf]| -> Vec<Option<SystemTime>> {
        paths
            .iter()
            .map(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
            .collect()
    };
    let before = snapshot(paths);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if snapshot(paths) != before {
            return Ok(());
        }
    }
}

/// Lex and parse one source file (no type checking) for lint/doc.
fn parse_source(source: &str) -> Result<aura_compiler::ast::Program, String> {
    let (tokens, lines) = aura_compiler::lexer::Lexer::new(source).lex()?;
    aura_compiler::parser::Parser::new(&tokens, &lines).parse()
}

/// Compile, rendering failures with source context (line snippets,
/// did-you-mean suggestions, one block per diagnostic).
fn compile_rendered(
    files: &[(String, String)],
    name: &str,
) -> Result<aura_bytecode::Module, anyhow::Error> {
    compile_files(files, name).map_err(|e| anyhow::anyhow!("{}", e.render(files)))
}

/// Read every source file; the program is named after the first file.
fn load(paths: &[PathBuf]) -> Result<(Vec<(String, String)>, String)> {
    if paths.is_empty() {
        bail!("no source files");
    }
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
