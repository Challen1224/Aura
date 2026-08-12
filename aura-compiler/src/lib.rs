//! Aura compiler front-end.
//!
//! Pipeline: source text → tokens → AST → typed AST → bytecode module.

#![warn(missing_docs)]

pub mod ast;
pub mod emitter;
pub mod lexer;
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
    let tokens = Lexer::new(source).lex().map_err(CompileError::Lex)?;
    let ast = Parser::new(&tokens).parse().map_err(CompileError::Parse)?;
    let typed = TypeChecker::new().check(&ast).map_err(|e| CompileError::Type(e.0))?;
    let module = Emitter::new(module_name)
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
                    body,
                    handlers: vec![],
                    max_stack: 8,
                    locals: 0,
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
