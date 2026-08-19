//! `aura fmt` — whitespace normalizer.
//!
//! Deliberately scoped to what cannot change meaning: it rewrites *leading*
//! indentation (4 spaces per brace/paren/bracket depth, one extra level for
//! continuation lines), strips trailing whitespace, and normalizes the final
//! newline. Token layout within a line is never touched, so comments and
//! string contents survive untouched; lines inside block comments and
//! multi-line strings are preserved verbatim.
//!
//! Safety net: after formatting, both versions are lexed and their token
//! streams compared. On any difference the file is left unmodified and the
//! command fails — a formatter bug can never corrupt a program.

use anyhow::{bail, Result};
use aura_compiler::lexer::Lexer;

/// Per-line scanner state that survives line breaks. Shared with the REPL,
/// which uses the same scanner to detect unfinished multi-line input.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LineState {
    /// Ordinary code.
    Code,
    /// Inside a `/* ... */` block comment.
    BlockComment,
    /// Inside a `"""..."""` multi-line string.
    TripleString,
    /// Inside a multi-line `r"..."` raw string (no escapes; the next `"`
    /// ends it).
    RawString,
}

/// Format Aura source text. Returns the formatted text (which may equal the
/// input). Fails if the reformat would change the token stream.
pub fn format_source(source: &str) -> Result<String> {
    let mut out = String::with_capacity(source.len());
    let mut depth: i32 = 0;
    let mut state = LineState::Code;
    // Indent of the previous emitted code line, and whether it left an
    // expression open (continuation heuristic).
    let mut prev_open = false;
    let mut prev_indent: i32 = 0;

    for line in source.lines() {
        let entry_state = state;
        state = scan_line(line, entry_state, &mut depth);

        // Interior of a block comment or multi-line string: whitespace may be
        // meaningful (string contents) or hand-aligned (comment art) — keep
        // the line verbatim.
        if entry_state != LineState::Code {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }

        // Dedent for the closers the line starts with (`}`, `)`, `]`,
        // possibly several, as in `});`).
        let lead_close = trimmed
            .chars()
            .take_while(|c| matches!(c, '}' | ')' | ']' | ';'))
            .filter(|c| *c != ';')
            .count() as i32;
        // `depth` has already consumed this line's brackets; recover the
        // depth at line start from the net change.
        let depth_at_start = depth - net_depth_change(line, entry_state);
        let mut indent = (depth_at_start - lead_close).max(0);

        // Continuation: a line after an open-ended one, unless it starts by
        // closing a bracket (the bracket's indent already applies).
        if prev_open && lead_close == 0 {
            indent = indent.max(prev_indent + 1);
        }

        for _ in 0..indent {
            out.push_str("    ");
        }
        out.push_str(trimmed);
        out.push('\n');

        // A line "leaves an expression open" when it is code (not a pure
        // comment line) and ends mid-expression. Lines ending a statement,
        // opening/closing a block, a label, or a comma-separated list item
        // do not trigger continuation indent.
        let code_part = strip_line_comment(trimmed);
        let tail = code_part.trim_end();
        prev_open = !tail.is_empty()
            && state == LineState::Code
            && !tail.ends_with([';', '{', '}', ':', ','])
            && !tail.ends_with("*/");
        prev_indent = indent;
    }

    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }

    verify_tokens(source, &out)?;
    Ok(out)
}

/// The part of a code line before any `//` comment (string-aware).
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// Net bracket-depth change contributed by `line` starting in `state`.
fn net_depth_change(line: &str, state: LineState) -> i32 {
    let mut d = 0;
    scan_line(line, state, &mut d);
    d
}

/// Scan one line, updating bracket depth for code regions and returning the
/// state the next line starts in. Strings, char literals, and comments do
/// not contribute to depth.
pub(crate) fn scan_line(line: &str, mut state: LineState, depth: &mut i32) -> LineState {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match state {
            LineState::BlockComment => {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    state = LineState::Code;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            LineState::TripleString => {
                if bytes[i] == b'"'
                    && bytes.get(i + 1) == Some(&b'"')
                    && bytes.get(i + 2) == Some(&b'"')
                {
                    state = LineState::Code;
                    i += 3;
                } else {
                    i += 1;
                }
            }
            LineState::RawString => {
                if bytes[i] == b'"' {
                    state = LineState::Code;
                }
                i += 1;
            }
            LineState::Code => match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => return state,
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    state = LineState::BlockComment;
                    i += 2;
                }
                b'"' if bytes.get(i + 1) == Some(&b'"') && bytes.get(i + 2) == Some(&b'"') => {
                    state = LineState::TripleString;
                    i += 3;
                }
                b'r' if bytes.get(i + 1) == Some(&b'"') => {
                    i += 2;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        // Unclosed on this line: the raw string continues on
                        // the next (raw strings may span lines).
                        return LineState::RawString;
                    }
                    i += 1;
                }
                b'"' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                b'\'' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'\'' {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                b'{' | b'(' | b'[' => {
                    *depth += 1;
                    i += 1;
                }
                b'}' | b')' | b']' => {
                    *depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            },
        }
    }
    state
}

/// Lex both versions and require identical token streams.
fn verify_tokens(before: &str, after: &str) -> Result<()> {
    let lex = |s: &str| Lexer::new(s).lex().map(|(tokens, _)| tokens);
    match (lex(before), lex(after)) {
        (Ok(a), Ok(b)) if a == b => Ok(()),
        (Ok(_), Ok(_)) => bail!(
            "internal error: formatting would change the token stream; \
             file left unmodified (please report this)"
        ),
        // A file that does not lex fails identically on both sides; its
        // errors surface at compile time, not here.
        (Err(_), Err(_)) => Ok(()),
        (Err(e), _) | (_, Err(e)) => {
            bail!("internal error: formatting changed lexability ({e}); file left unmodified")
        }
    }
}
