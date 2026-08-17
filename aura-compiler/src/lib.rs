//! Aura compiler front-end.
//!
//! Pipeline: source text → tokens → AST → typed AST → bytecode module.

#![warn(missing_docs)]

pub mod ast;
pub mod emitter;
pub mod intrinsics;
pub mod lexer;
pub mod nested;
pub mod parser;
pub mod typer;

use aura_bytecode::{ClassDef, ClassId, MethodDef, MethodId, Module, TypeDesc};
use emitter::Emitter;
use lexer::Lexer;
use parser::Parser;
use std::collections::HashMap;
use typer::TypeChecker;

/// Errors produced by the compiler.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// Lexical error.
    #[error("lex error: {0}")]
    Lex(String),
    /// Parse error.
    #[error("parse error: {0}")]
    Parse(String),
    /// Type error.
    #[error("type error: {0}")]
    Type(String),
    /// Emitter error.
    #[error("emit error: {0}")]
    Emit(String),
}

impl CompileError {
    /// Render this error (possibly several newline-separated diagnostics)
    /// with source context: each diagnostic that names a line gets that
    /// source line printed beneath it, rustc-style. `files` are the
    /// `(name, source)` pairs given to `compile_files`; a line is only
    /// shown when it can be attributed unambiguously (parse/lex errors
    /// carry their file name; type errors are attributed only for
    /// single-file programs).
    pub fn render(&self, files: &[(String, String)]) -> String {
        let (kind, body) = match self {
            CompileError::Lex(m) => ("lex error", m),
            CompileError::Parse(m) => ("parse error", m),
            CompileError::Type(m) => ("type error", m),
            CompileError::Emit(m) => ("emit error", m),
        };
        let mut out = String::new();
        let count = body.lines().count();
        for (i, diag) in body.lines().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("{kind}: {diag}"));
            if let Some(snippet) = source_snippet(self, diag, files) {
                out.push('\n');
                out.push_str(&snippet);
            }
        }
        if count > 1 {
            out.push_str(&format!("\n{count} errors reported"));
        }
        out
    }
}

/// The `  N | <source line>` context block for one diagnostic, when its
/// line can be found and attributed.
fn source_snippet(err: &CompileError, diag: &str, files: &[(String, String)]) -> Option<String> {
    // Diagnostics carry "line N" (typer: "line N, in `..`"; parser/lexer:
    // "name: line N:" / "at line N, col M").
    let idx = diag.find("line ")?;
    let rest = &diag[idx + 5..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let line_no: usize = digits.parse().ok()?;
    let source = match err {
        // Parse/lex diagnostics are prefixed "<file>: ".
        CompileError::Lex(_) | CompileError::Parse(_) => {
            let name = diag.split(':').next()?.trim();
            files.iter().find(|(n, _)| n == name).map(|(_, s)| s)
        }
        // Type/emit diagnostics carry no file; only unambiguous for
        // single-file programs.
        _ => (files.len() == 1).then(|| &files[0].1),
    }?;
    let text = source.lines().nth(line_no.saturating_sub(1))?;
    let trimmed = text.trim_end();
    if trimmed.trim().is_empty() {
        return None;
    }
    let gutter = line_no.to_string().len().max(3);
    let indent = trimmed.len() - trimmed.trim_start().len();
    Some(format!(
        "{:>gutter$} |\n{:>gutter$} | {}\n{:>gutter$} | {}{}",
        "",
        line_no,
        trimmed,
        "",
        " ".repeat(indent),
        "^".repeat(trimmed.trim().chars().count().max(1)),
        gutter = gutter
    ))
}

/// Compile source text into an Aura [`Module`].
pub fn compile(source: &str, module_name: &str) -> Result<Module, CompileError> {
    compile_files(&[(module_name.to_string(), source.to_string())], module_name)
}

/// Compile several source files into one program. Every declaration is
/// visible from every file (flat namespace, no imports); each file is a
/// module, and `internal` members are only accessible from classes declared
/// in the same file. Single-file programs leave every class in the same
/// module, so `internal` behaves as before there.
pub fn compile_files(
    files: &[(String, String)],
    program_name: &str,
) -> Result<Module, CompileError> {
    let mut merged = ast::Program { decls: Vec::new() };
    let tag = files.len() > 1;
    for (idx, (name, source)) in files.iter().enumerate() {
        let (tokens, lines) = Lexer::new(source)
            .lex()
            .map_err(|e| CompileError::Lex(format!("{}: {}", name, e)))?;
        let ast = Parser::new(&tokens, &lines)
            .parse()
            .map_err(|e| CompileError::Parse(format!("{}: {}", name, e)))?;
        let mut decls = ast.decls;
        // Every parse injects the builtin `Exception` base class at index 0;
        // keep only the first file's copy. (A user-declared Exception still
        // collides with the builtin, exactly as in single-file programs.)
        if idx > 0 {
            if matches!(decls.first(), Some(ast::Decl::Class(c)) if c.name == "Exception" && c.module.is_empty())
            {
                decls.remove(0);
            }
        }
        if tag {
            for decl in &mut decls {
                if let ast::Decl::Class(c) = decl {
                    // The builtin stays module-less: it belongs to no file.
                    if c.name != "Exception" {
                        c.module = name.clone();
                    }
                }
            }
        }
        merged.decls.extend(decls);
    }
    let merged = parser::expand_type_aliases(&merged).map_err(CompileError::Parse)?;
    let merged = nested::flatten_nested_classes(&merged).map_err(CompileError::Parse)?;
    let typed = TypeChecker::new().check(&merged).map_err(|e| CompileError::Type(e.0))?;
    let module = Emitter::new(program_name)
        .emit(&typed)
        .map_err(CompileError::Emit)?;
    Ok(module)
}

/// Compile source text and resolve class/method ids into a runtime module.
pub fn compile_module(source: &str, name: &str) -> Result<Module, CompileError> {
    compile(source, name)
}

/// Helper to build a minimal module directly from a method body.
/// Useful for VM tests that bypass the compiler.
pub fn synthetic_module(body: Vec<aura_bytecode::Op>) -> Module {
    let program_class = ClassDef {
        name: "Program".to_string(),
        generic_params: vec![],
        super_class: None,
        interfaces: vec![],
        is_interface: false,
        is_abstract: false,
        is_record: false,
        fields: vec![],
        static_fields: vec![],
        methods: HashMap::new(),
        static_methods: {
            let mut map = HashMap::new();
            map.insert(
                MethodId(0),
                MethodDef {
                    name: "Main".to_string(),
                    return_ty: TypeDesc::Unit,
                    params: vec![],
                    generic_params: vec![],
                    is_instance: false,
                    is_async: false,
                    body,
                    handlers: vec![],
                    max_stack: 8,
                    locals: 0,
                    line_starts: Vec::new(),
                    local_names: Vec::new(),
                },
            );
            map
        },
    };
    Module {
        name: "test".to_string(),
        classes: {
            let mut map = HashMap::new();
            map.insert(ClassId(0), program_class);
            map
        },
        enums: HashMap::new(),
        entrypoint: Some(MethodId(0)),
        constant_pool: vec![],
    }
}
