//! Aura CLI — compile and run .aura programs.

use anyhow::{Context, Result};
use aura_compiler::compile;
use aura_vm::Vm;
use clap::{Parser as ClapParser, Subcommand};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(ClapParser)]
#[command(name = "aura", version, about = "Aura language CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a source file and print the resulting bytecode module.
    Compile {
        /// Source file path.
        path: PathBuf,
    },
    /// Compile and run a source file.
    Run {
        /// Source file path.
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile { path } => {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let module = compile(&source, module_name(&path)?)?;
            println!("{:#?}", module);
            Ok(())
        }
        Command::Run { path } => {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let module = Arc::new(compile(&source, module_name(&path)?)?);
            let mut vm = Vm::new(module);
            vm.run().context("runtime error")?;
            Ok(())
        }
    }
}

fn module_name(path: &PathBuf) -> Result<&str> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .context("invalid source file name")
}
