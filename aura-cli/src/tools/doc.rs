//! `aura doc` — generate Markdown API documentation from source.
//!
//! Structure comes from the AST (public and protected members; private and
//! internal ones are omitted). Prose comes from `///` doc comments: the
//! contiguous block of `///` lines directly above a declaration is attached
//! to it, matched via the declaration's recorded source line.

use aura_compiler::ast::{
    ClassDecl, Decl, EnumDecl, Member, MethodDecl, Program, Visibility,
};
use std::collections::HashMap;
use std::fmt::Write;

/// Generate Markdown for one source file's declarations.
pub fn document(module_name: &str, source: &str, program: &Program) -> String {
    let docs = harvest_doc_comments(source);
    let mut out = String::new();
    let _ = writeln!(out, "# Module `{module_name}`\n");

    for decl in &program.decls {
        match decl {
            Decl::Class(class) => {
                // Skip the parser-injected Exception base class.
                if class.name == "Exception" && class.line == 0 {
                    continue;
                }
                document_class(class, &docs, &mut out);
            }
            Decl::Enum(e) => document_enum(e, &mut out),
            Decl::TypeAlias(a) => {
                let _ = writeln!(
                    out,
                    "## type `{}{}` = `{}`\n",
                    a.name,
                    generic_suffix_names(a.generic_params.iter().map(|g| g.name.as_str())),
                    a.target.name()
                );
            }
            Decl::Newtype(n) => {
                let _ = writeln!(out, "## newtype `{}` over `{}`\n", n.name, n.underlying.name());
            }
            Decl::LiteralUnion(u) => {
                let members: Vec<String> = u
                    .operands
                    .iter()
                    .map(|op| match op {
                        aura_compiler::ast::UnionOperand::Literal(s) => format!("\"{s}\""),
                        aura_compiler::ast::UnionOperand::Named(n) => n.clone(),
                    })
                    .collect();
                let _ = writeln!(out, "## type `{}` = {}\n", u.name, members.join(" | "));
            }
        }
    }
    out
}

/// `///` blocks by the 1-based line of the first following declaration line.
fn harvest_doc_comments(source: &str) -> HashMap<usize, String> {
    let mut docs = HashMap::new();
    let mut block: Vec<&str> = Vec::new();
    for (idx, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if let Some(text) = line.strip_prefix("///") {
            block.push(text.strip_prefix(' ').unwrap_or(text));
        } else if line.is_empty() {
            // Blank lines are carried over: a block still attaches to the
            // next declaration even with blank lines between them.
        } else if !block.is_empty() {
            // First substantive line after a block: the declaration it
            // documents.
            docs.insert(idx + 1, block.join("\n"));
            block.clear();
        }
    }
    docs
}

fn document_class(class: &ClassDecl, docs: &HashMap<usize, String>, out: &mut String) {
    let kind = if class.extension_on.is_some() {
        "extension"
    } else if class.is_interface {
        "interface"
    } else if class.is_record {
        "record"
    } else if class.is_static {
        "static class"
    } else if class.is_abstract {
        "abstract class"
    } else if class.is_sealed {
        "sealed class"
    } else {
        "class"
    };
    let generics = generic_suffix_names(class.generic_params.iter().map(|g| g.name.as_str()));
    let bases = if class.bases.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = class
            .bases
            .iter()
            .map(|(name, args)| {
                if args.is_empty() {
                    name.clone()
                } else {
                    format!(
                        "{name}<{}>",
                        args.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ")
                    )
                }
            })
            .collect();
        format!(" : {}", list.join(", "))
    };
    let target = match &class.extension_on {
        Some(t) => format!(" on {}", t.name()),
        None => String::new(),
    };
    let _ = writeln!(out, "## {kind} `{}{generics}`{target}{bases}\n", class.name);
    if let Some(text) = docs.get(&class.line) {
        let _ = writeln!(out, "{text}\n");
    }

    if class.is_record && !class.record_params.is_empty() {
        let params: Vec<String> = class
            .record_params
            .iter()
            .map(|p| format!("{} {}", p.ty.name(), p.name))
            .collect();
        let _ = writeln!(out, "Positional properties: `({})`\n", params.join(", "));
    }

    let mut fields = Vec::new();
    let mut props = Vec::new();
    let mut methods = Vec::new();
    for member in &class.members {
        match member {
            Member::Field(f) if visible(f.visibility) => {
                fields.push(format!(
                    "- `{}{} {}`",
                    if f.is_static { "static " } else { "" },
                    f.ty.name(),
                    f.name
                ));
            }
            Member::Property(p) if visible(p.visibility) => {
                let mut acc = Vec::new();
                if p.getter.is_some() {
                    acc.push("get");
                }
                if p.setter.is_some() {
                    acc.push("set");
                }
                props.push(format!(
                    "- `{}{} {} {{ {}; }}`",
                    if p.is_static { "static " } else { "" },
                    p.ty.name(),
                    p.name,
                    acc.join("; ")
                ));
            }
            Member::Method(m) if visible(m.visibility) => {
                let mut entry = format!("- `{}`", method_signature(m));
                if let Some(text) = docs.get(&m.line) {
                    // Single-paragraph summary under the signature bullet.
                    let summary = text.lines().collect::<Vec<_>>().join(" ");
                    let _ = write!(entry, " — {summary}");
                }
                methods.push(entry);
            }
            _ => {}
        }
    }
    for (title, items) in [("Fields", fields), ("Properties", props), ("Methods", methods)] {
        if !items.is_empty() {
            let _ = writeln!(out, "**{title}**\n");
            for item in items {
                let _ = writeln!(out, "{item}");
            }
            let _ = writeln!(out);
        }
    }
    for nested in &class.nested {
        document_class(nested, docs, out);
    }
}

fn document_enum(e: &EnumDecl, out: &mut String) {
    let generics = generic_suffix_names(e.generic_params.iter().map(|g| g.name.as_str()));
    let _ = writeln!(out, "## enum `{}{generics}`\n", e.name);
    for variant in &e.variants {
        if variant.fields.is_empty() {
            let _ = writeln!(out, "- `{}`", variant.name);
        } else {
            let fields: Vec<String> = variant
                .fields
                .iter()
                .map(|f| format!("{} {}", f.ty.name(), f.name))
                .collect();
            let _ = writeln!(out, "- `{}({})`", variant.name, fields.join(", "));
        }
    }
    let _ = writeln!(out);
}

fn method_signature(m: &MethodDecl) -> String {
    let mut sig = String::new();
    if m.visibility == Visibility::Protected {
        sig.push_str("protected ");
    }
    if m.is_static {
        sig.push_str("static ");
    }
    if m.is_async {
        sig.push_str("async ");
    }
    if m.is_abstract {
        sig.push_str("abstract ");
    } else if m.is_virtual {
        sig.push_str("virtual ");
    }
    if m.is_override {
        sig.push_str("override ");
    }
    if m.is_final {
        sig.push_str("final ");
    }
    if !m.is_constructor {
        sig.push_str(&m.return_ty.name());
        sig.push(' ');
    }
    sig.push_str(&m.name);
    sig.push_str(&generic_suffix_names(m.generic_params.iter().map(|g| g.name.as_str())));
    let params: Vec<String> = m
        .params
        .iter()
        // Extension methods carry their receiver as a synthesized leading
        // `__self` parameter; readers see the receiver, not the desugar.
        .filter(|p| p.name != "__self")
        .map(|p| format!("{} {}", p.ty.name(), p.name))
        .collect();
    sig.push('(');
    sig.push_str(&params.join(", "));
    sig.push(')');
    if !m.throws.is_empty() {
        sig.push_str(" throws ");
        sig.push_str(&m.throws.join(", "));
    }
    sig
}

fn visible(v: Visibility) -> bool {
    matches!(v, Visibility::Public | Visibility::Protected)
}

fn generic_suffix_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let list: Vec<&str> = names.collect();
    if list.is_empty() {
        String::new()
    } else {
        format!("<{}>", list.join(", "))
    }
}
