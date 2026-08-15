//! Aura CLI — compile and run .aura programs.

use anyhow::{Context, Result};
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
    },
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
        Command::Run { paths, jit } => {
            let (files, name) = load(&paths)?;
            let module = Arc::new(compile_files(&files, &name)?);
            let mut vm = Vm::new(module);
            if jit {
                vm.enable_jit();
            }
            vm.run().context("runtime error")?;
            Ok(())
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
