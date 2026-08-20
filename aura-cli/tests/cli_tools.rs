//! End-to-end tests for the project/tooling subcommands, driving the real
//! `aura` binary: init → run → test → build → run-compiled, plus repl, fmt,
//! lint, and doc.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn aura() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aura"))
}

/// A unique scratch directory per test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aura-cli-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn init_run_test_build_roundtrip() {
    let dir = scratch("roundtrip");

    let out = aura().args(["init", "proj"]).current_dir(&dir).output().unwrap();
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let proj = dir.join("proj");
    assert!(proj.join("aura.toml").is_file());
    assert!(proj.join("src/main.aura").is_file());

    // aura run (project mode)
    let out = aura().arg("run").current_dir(&proj).output().unwrap();
    assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Hello from proj!"), "unexpected output: {stdout}");

    // aura test (scaffolded smoke test passes)
    let out = aura().arg("test").current_dir(&proj).output().unwrap();
    assert!(out.status.success(), "test failed: {}", String::from_utf8_lossy(&out.stdout));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 passed, 0 failed"), "unexpected: {stdout}");

    // A failing test is reported and fails the command.
    fs::write(
        proj.join("tests/failing_test.aura"),
        "class Program { static void Main() { throw new Exception(); } }\n",
    )
    .unwrap();
    let out = aura().arg("test").current_dir(&proj).output().unwrap();
    assert!(!out.status.success(), "test should fail");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("FAIL"), "no FAIL line: {stdout}");
    assert!(stdout.contains("1 passed, 1 failed"), "unexpected: {stdout}");

    // The filter selects only the passing test.
    let out = aura().args(["test", "smoke"]).current_dir(&proj).output().unwrap();
    assert!(out.status.success());

    // aura build → run the compiled module.
    let out = aura().arg("build").current_dir(&proj).output().unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let module = proj.join("build/proj.aurac");
    assert!(module.is_file());
    let out = aura().arg("run").arg(&module).current_dir(&proj).output().unwrap();
    assert!(out.status.success(), "compiled run failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Hello from proj!"));
}

#[test]
fn repl_session() {
    let mut child = aura()
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"int x = 40;\n\
              x + 2\n\
              class Doubler {\n    static int Twice(int n) {\n        return n * 2;\n    }\n}\n\
              Doubler.Twice(21)\n\
              nonsense +\n\
              :quit\n",
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("42"), "expression echo missing: {stdout}");
    // Both evaluations print 42; the class-method one must appear after the
    // first (i.e. twice in total).
    assert_eq!(stdout.matches("42").count(), 2, "expected two 42s: {stdout}");
    // The bad input reported an error without killing the session.
    assert!(stdout.contains("error"), "compile error not surfaced: {stdout}");
}

#[test]
fn fmt_normalizes_and_is_safe() {
    let dir = scratch("fmt");
    let file = dir.join("m.aura");
    fs::write(
        &file,
        "class Program {\nstatic void Main() {\nint x = 1;\nprint(x);\n}\n}\n",
    )
    .unwrap();

    // --check reports without rewriting.
    let out = aura().args(["fmt", "--check"]).arg(&file).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("would reformat"));
    let unchanged = fs::read_to_string(&file).unwrap();
    assert!(unchanged.contains("\nstatic void Main"));

    // Formatting indents; a second run is a no-op.
    let out = aura().arg("fmt").arg(&file).output().unwrap();
    assert!(out.status.success());
    let formatted = fs::read_to_string(&file).unwrap();
    assert!(formatted.contains("    static void Main() {"), "not indented: {formatted}");
    assert!(formatted.contains("        int x = 1;"));
    let out = aura().arg("fmt").arg(&file).output().unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("reformatted"));

    // Multi-line strings are preserved verbatim.
    let raw = dir.join("raw.aura");
    fs::write(
        &raw,
        "class Program {\n    static void Main() {\n        string s = \"\"\"alpha\n  keep me\nomega\"\"\";\n        print(s);\n    }\n}\n",
    )
    .unwrap();
    let before = fs::read_to_string(&raw).unwrap();
    let out = aura().arg("fmt").arg(&raw).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(fs::read_to_string(&raw).unwrap(), before, "string interior touched");
}

#[test]
fn lint_reports_and_doc_renders() {
    let dir = scratch("lintdoc");
    let file = dir.join("m.aura");
    fs::write(
        &file,
        "/// Frobnicates.\nclass Frob {\n    /// Runs the frob.\n    int Go() {\n        int unused = 1;\n        return 2;\n        print(\"dead\");\n    }\n}\n",
    )
    .unwrap();

    let out = aura().arg("lint").arg(&file).output().unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("unused local `unused`"), "{stdout}");
    assert!(stdout.contains("unreachable code"), "{stdout}");

    let out = aura().arg("doc").arg(&file).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## class `Frob`"), "{stdout}");
    assert!(stdout.contains("Frobnicates."), "{stdout}");
    assert!(stdout.contains("`int32 Go()`"), "{stdout}");
    assert!(stdout.contains("Runs the frob."), "{stdout}");

    // A blank line between the /// block and its declaration must not
    // detach the docs (the conventional spaced style).
    let spaced = dir.join("spaced.aura");
    fs::write(
        &spaced,
        "/// Widget docs survive a blank line.\n\nclass Widget {\n    int Id() {\n        return 1;\n    }\n}\n",
    )
    .unwrap();
    let out = aura().arg("doc").arg(&spaced).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Widget docs survive a blank line."),
        "doc block dropped across blank line: {stdout}"
    );

    // A clean file exits zero.
    let clean = dir.join("clean.aura");
    fs::write(&clean, "class C {\n    int V() {\n        return 1;\n    }\n}\n").unwrap();
    let out = aura().arg("lint").arg(&clean).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn manifest_run_defaults_apply() {
    let dir = scratch("manifest");
    let out = aura().args(["init", "p"]).current_dir(&dir).output().unwrap();
    assert!(out.status.success());
    let proj = dir.join("p");
    // A bad gc-mode in [run] must surface when running the project.
    fs::write(
        proj.join("aura.toml"),
        "[package]\nname = \"p\"\n\n[run]\ngc-mode = \"bogus\"\n",
    )
    .unwrap();
    let out = aura().arg("run").current_dir(&proj).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown gc mode"));
    // And an unknown key is rejected at parse time.
    fs::write(proj.join("aura.toml"), "[package]\nname = \"p\"\ntypo = 1\n").unwrap();
    let out = aura().arg("run").current_dir(&proj).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("aura.toml"));
}
