# Debugging Aura

Two front-ends drive the same VM debugger (interpreter tier; installing a
debugger suppresses JIT tier-up for the run):

## Interactive CLI

```
aura debug program.aura [--break 12 --break 30]
```

Commands at the `(aura-db)` prompt: `c` continue, `s` step into, `n` step
over, `o` step out, `b <line>` / `d <line>` line breakpoints,
`bb <Class.Method> <op>` bytecode breakpoint at an exact op index,
`locals`, `p <path>` (paths: `a`, `p.x`, `xs[0]`, `a.b[2].c`),
`w <path>` / `unw <path>` watches reported at every stop, `dis`
disassembly around the current pc, `bt` backtrace, `q` quit.

## DAP (VS Code, nvim-dap, any DAP client)

`aura dap` serves the Debug Adapter Protocol on stdio. VS Code needs a
launch configuration that starts the adapter; with a generic DAP bridge
(or a thin extension declaring type `aura`), the shape is:

```jsonc
// .vscode/launch.json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "aura",              // your DAP client's type name
      "request": "launch",
      "name": "Debug Aura program",
      "program": "${workspaceFolder}/main.aura",
      "stopOnEntry": false,
      // point the client at the adapter binary:
      "debugServer": null,          // stdio adapter: run `aura dap`
    }
  ]
}
```

For nvim-dap:

```lua
dap.adapters.aura = { type = "executable", command = "aura", args = { "dap" } }
dap.configurations.aura = {
  { type = "aura", request = "launch", name = "Debug",
    program = "${file}", stopOnEntry = false },
}
```

Supported: launch, line breakpoints (set before start or while stopped),
continue / next / stepIn / stepOut, threads (one), stackTrace with real
source lines, scopes/variables (named locals, temporaries hidden),
evaluate with inspection paths (`p.x`, `xs[0]` — watch panel and hovers),
program output as `output` events, exited/terminated.

Known limits (deliberate, stated): single thread; breakpoints are
line-only and shared across the files of a multi-file program; `evaluate`
is inspection, not an expression evaluator (no method calls, no
arithmetic); `pause` while running is unsupported (the VM has no async
interrupt); breakpoint edits made while running take effect at the next
stop.

## GDB: VM crash/hang triage (`tools/gdb/aura_gdb.py`)

The third front-end covers the case DAP cannot: **when the VM itself is
the bug** — a JIT segfault or a hang under some program leaves you in
gdb staring at Rust frames with no idea where the *Aura* program was.
This is CPython's `libpython.py` model: the script teaches GDB to read
the interpreter's own data structures, not to execute Aura.

```
cargo build --profile gdb -p aura-cli     # release + VM debuginfo
gdb -x tools/gdb/aura_gdb.py --args target/gdb/aura run prog.aura
(gdb) aura-bt        # Aura-level backtrace with source lines
(gdb) aura-locals    # named locals of the innermost Aura frame
(gdb) aura-line      # current method + line + pc
```

Works at any stop inside a run — breakpoints on VM functions, crashes,
hangs (Ctrl-C) — including against `qemu-x86_64 -g <port>` with
`gdb-multiarch` for the JIT-enabled target. VM cooperation is two small
pieces: the no-mangle static `AURA_CURRENT_VM` (set for the duration of
every run, so the script finds the VM even from a crash), and
`Vm.gdb_index`, a flat method index (plain `Vec`s of labels, line
tables, local names) so the script never walks a Rust `HashMap`'s
unstable hashbrown internals. Debuginfo lives in the opt-in `gdb`
profile (release optimization + DWARF for `aura-vm`/`aura-bytecode`,
split into `.dwo` files) — kept out of the default release profile so
the full test-binary set still fits constrained disks. The
`aura-cli/tests/gdb_ext.rs` test exercises the extension end-to-end;
it skips on binaries without debuginfo (set `AURA_GDB_BIN` to the
gdb-profile binary to run it).

Limits, stated: inspection only (no stepping Aura from gdb — that is
DAP's job); heap objects render as `Object(GcRef(n))` without chasing
into the heap map; the Vec/String decoding leans on std layouts that are
stable in practice but not API (all assumptions live in one file).

## Why not LLDB or DWARF for Aura itself

LLDB has a different Python API; the GDB script does not port for free,
and it is parked until someone actually debugs the VM under LLDB.
Emitting DWARF for JIT-compiled Aura code (the GDB JIT interface) would
let gdb itself show Aura frames — but only JIT frames, since interpreter
frames are heap data no unwinder can walk; that remains parked as a
large project with partial coverage. Aura-source debugging goes through
the DAP adapter; the debug information is a custom format in the module
(line tables and local-name tables), which is the JVM/CPython approach,
not DWARF.
