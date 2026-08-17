//! The GDB extension (tools/gdb/aura_gdb.py) against the real binary:
//! break inside the VM mid-run, then assert `aura-bt` reconstructs the
//! Aura-level call stack with source lines, `aura-locals` decodes named
//! locals, and `aura-line` reports the innermost position. Skips (with a
//! visible note) when gdb is not installed.

use std::process::Command;

#[test]
fn gdb_commands_reconstruct_aura_state() {
    if Command::new("gdb").arg("--version").output().is_err() {
        eprintln!("skipping: gdb not installed");
        return;
    }
    // The extension needs a binary with aura-vm debuginfo — the opt-in
    // `gdb` profile (`cargo build --profile gdb -p aura-cli`). Honor an
    // explicit binary via AURA_GDB_BIN; otherwise use the test binary
    // only if it carries debuginfo (it does not under the lean default
    // release profile — skip with a note rather than fail).
    let binary = std::env::var("AURA_GDB_BIN").unwrap_or_else(|_| {
        env!("CARGO_BIN_EXE_aura").to_string()
    });
    let has_debuginfo = Command::new("readelf")
        .args(["-S", &binary])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            text.contains(".debug_info") || text.contains(".debug_gdb_scripts")
        })
        .unwrap_or(false);
    if !has_debuginfo {
        eprintln!(
            "skipping: {binary} has no debuginfo; build with \
`cargo build --profile gdb -p aura-cli` and set AURA_GDB_BIN"
        );
        return;
    }
    let dir = std::env::temp_dir().join("aura_gdb_test");
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let program = dir.join("gdb_probe.aura");
    std::fs::write(
        &program,
        r#"
class Worker {
    string tag;
    Worker(string tag) { this.tag = tag; }
    int process(int n) {
        var xs = new List<int>();
        for (var i in 1..=n) { xs.Add(i * 2); }
        return xs.Get(n - 1);
    }
}
class Program {
    static int Main() {
        Worker w = new Worker("alpha");
        return w.process(5);
    }
}
"#,
    )
    .expect("write program");

    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/gdb/aura_gdb.py");
    let output = Command::new("gdb")
        .args([
            "--batch",
            "-q",
            "-x",
            script,
            "-ex",
            "break aura_vm::native::exec_native",
            "-ex",
            "run",
            "-ex",
            "aura-bt",
            "-ex",
            "aura-locals",
            "-ex",
            "aura-line",
            "--args",
            &binary,
            "run",
            program.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run gdb");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // aura-bt: both frames with their lines (Main at the call on line 14,
    // Worker.process stopped inside line 6's List ctor).
    assert!(text.contains("Program.Main (line 14)"), "bt Main:\n{text}");
    assert!(
        text.contains("Worker.process (line 6)"),
        "bt process:\n{text}"
    );
    // aura-locals: parameters decoded, unassigned slots as Unit, no
    // compiler temporaries.
    assert!(text.contains("n = Int(5)"), "locals n:\n{text}");
    assert!(text.contains("this = Object"), "locals this:\n{text}");
    assert!(!text.contains("__"), "temps must be hidden:\n{text}");
    // aura-line agrees with the innermost bt frame.
    assert!(
        text.contains("Worker.process, line 6"),
        "aura-line:\n{text}"
    );
}
