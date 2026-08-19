//! Project configuration: `aura.toml` discovery, parsing, and scaffolding.
//!
//! A project is a directory containing `aura.toml`. Commands that accept a
//! project (`build`, `test`, `run`/`debug` without file arguments) find it by
//! walking up from the current directory, the way cargo finds `Cargo.toml`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Parsed `aura.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// `[package]` — identity.
    pub package: PackageSection,
    /// `[build]` — what to compile and where to put it.
    #[serde(default)]
    pub build: BuildSection,
    /// `[run]` — default VM options for `aura run` / `aura test`.
    #[serde(default)]
    pub run: RunSection,
}

/// `[package]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSection {
    /// Package name; names the program and the `aura build` output file.
    pub name: String,
    /// Package version (informational).
    #[serde(default)]
    pub version: Option<String>,
}

/// `[build]` section.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildSection {
    /// The entry module — the file whose name becomes the program's first
    /// module. Must contain `class Program { static void Main() }`.
    pub entry: PathBuf,
    /// Additional source roots: directories (searched recursively for
    /// `.aura` files) or individual files. The entry file is always
    /// included and deduplicated.
    pub sources: Vec<PathBuf>,
    /// Directory `aura build` writes the compiled module into.
    pub output: PathBuf,
}

impl Default for BuildSection {
    fn default() -> Self {
        Self {
            entry: PathBuf::from("src/main.aura"),
            sources: vec![PathBuf::from("src")],
            output: PathBuf::from("build"),
        }
    }
}

/// `[run]` section: defaults applied by `aura run`/`aura test` in a project.
/// Command-line flags override these.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunSection {
    /// Enable the x86-64 JIT.
    pub jit: bool,
    /// Collector disposition (`throughput` | `balanced` | `latency`).
    #[serde(rename = "gc-mode")]
    pub gc_mode: Option<String>,
    /// Full-heap GC threshold in bytes.
    #[serde(rename = "gc-threshold")]
    pub gc_threshold: Option<usize>,
    /// Nursery size in bytes.
    #[serde(rename = "gc-nursery")]
    pub gc_nursery: Option<usize>,
    /// Hard heap limit in bytes.
    #[serde(rename = "gc-max-heap")]
    pub gc_max_heap: Option<usize>,
}

/// A discovered project: manifest plus its root directory.
pub struct Project {
    /// Directory containing `aura.toml`.
    pub root: PathBuf,
    /// The parsed manifest.
    pub manifest: Manifest,
}

impl Project {
    /// Find and parse `aura.toml`, walking up from the current directory.
    pub fn find() -> Result<Project> {
        let cwd = std::env::current_dir().context("reading current directory")?;
        let mut dir: &Path = &cwd;
        loop {
            let candidate = dir.join("aura.toml");
            if candidate.is_file() {
                return Self::load(dir.to_path_buf());
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => bail!(
                    "no aura.toml found in `{}` or any parent directory\n\
                     (run inside a project, pass source files explicitly, \
                     or create a project with `aura init`)",
                    cwd.display()
                ),
            }
        }
    }

    /// Parse the manifest in `root`.
    pub fn load(root: PathBuf) -> Result<Project> {
        let path = root.join("aura.toml");
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let manifest: Manifest =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if manifest.package.name.is_empty() {
            bail!("{}: package.name must not be empty", path.display());
        }
        Ok(Project { root, manifest })
    }

    /// The ordered source list for the program: the entry file first, then
    /// every `.aura` file under the source roots (sorted, deduplicated).
    pub fn sources(&self) -> Result<Vec<PathBuf>> {
        let entry = self.root.join(&self.manifest.build.entry);
        if !entry.is_file() {
            bail!(
                "entry `{}` does not exist (set build.entry in aura.toml)",
                entry.display()
            );
        }
        let mut rest = Vec::new();
        for src in &self.manifest.build.sources {
            let path = self.root.join(src);
            if path.is_dir() {
                collect_aura_files(&path, &mut rest)?;
            } else if path.is_file() {
                rest.push(path);
            } else {
                bail!("source `{}` does not exist", path.display());
            }
        }
        rest.sort();
        rest.dedup();
        let mut ordered = vec![entry.clone()];
        ordered.extend(rest.into_iter().filter(|p| *p != entry));
        Ok(ordered)
    }

    /// The project's test files: every `.aura` file under `tests/`, sorted.
    pub fn test_files(&self) -> Result<Vec<PathBuf>> {
        let dir = self.root.join("tests");
        let mut files = Vec::new();
        if dir.is_dir() {
            collect_aura_files(&dir, &mut files)?;
        }
        files.sort();
        Ok(files)
    }

    /// Where `aura build` writes the compiled module.
    pub fn output_path(&self) -> PathBuf {
        self.root
            .join(&self.manifest.build.output)
            .join(format!("{}.aurac", self.manifest.package.name))
    }
}

/// Recursively collect `.aura` files under `dir`.
fn collect_aura_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_aura_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "aura") {
            out.push(path);
        }
    }
    Ok(())
}

/// Scaffold a new project directory: `aura.toml`, `src/main.aura`, a sample
/// test, and a `.gitignore` for the build directory.
pub fn init(name: &str, dir: &Path) -> Result<()> {
    if dir.join("aura.toml").exists() {
        bail!("{} already contains aura.toml", dir.display());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || name.is_empty()
    {
        bail!("project name `{name}` must be alphanumeric (plus `-` and `_`)");
    }
    fs::create_dir_all(dir.join("src"))?;
    fs::create_dir_all(dir.join("tests"))?;
    fs::write(
        dir.join("aura.toml"),
        format!(
            "[package]\n\
             name = \"{name}\"\n\
             version = \"0.1.0\"\n\
             \n\
             # Defaults shown; delete what you don't customize.\n\
             [build]\n\
             entry = \"src/main.aura\"\n\
             sources = [\"src\"]\n\
             output = \"build\"\n\
             \n\
             [run]\n\
             jit = false\n"
        ),
    )?;
    fs::write(
        dir.join("src/main.aura"),
        "class Program {\n    static void Main() {\n        print(\"Hello from {name}!\");\n        println();\n    }\n}\n"
            .replace("{name}", name),
    )?;
    fs::write(
        dir.join("tests/smoke_test.aura"),
        "// Each file under tests/ is its own program: it is compiled with\n\
         // every source file except the entry, and passes when Main exits\n\
         // without a runtime error. Throw to fail.\n\
         class Program {\n    static void Main() {\n        int sum = 1 + 1;\n        if (sum != 2) {\n            throw new Exception();\n        }\n    }\n}\n",
    )?;
    fs::write(dir.join(".gitignore"), "/build\n")?;
    Ok(())
}
