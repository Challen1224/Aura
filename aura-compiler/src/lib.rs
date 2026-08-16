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
