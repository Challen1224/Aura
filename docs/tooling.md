# Tooling Reference

Everything the `aura` binary does: commands, the project manifest, and
runtime flags. For a guided introduction, start with
[Getting Started](getting-started.md).

## Commands at a glance

| Command | Purpose |
|---|---|
| `aura init [name]` | scaffold a project (in `./name`, or in place without a name) |
| `aura run [paths]` | compile and run — sources, a `.aurac` module, or the project |
| `aura build` | compile the project to `build/<name>.aurac` |
| `aura test [filter]` | run the project's `tests/*.aura` programs |
| `aura repl` | interactive session |
| `aura fmt [paths] [--check]` | normalize whitespace (token-safe) |
| `aura lint [paths]` | unused locals, unreachable code, empty catches |
| `aura doc [paths] [-o FILE]` | Markdown API docs from declarations and `///` |
| `aura compile <paths>` | compile and dump the bytecode module (debugging aid) |
| `aura debug <paths> [--break N]` | interactive source-level debugger |
| `aura dap` | Debug Adapter Protocol server on stdio |

Commands that take optional paths (`run`, `debug`, `fmt`, `lint`, `doc`)
use the current project when given none — found by walking up from the
working directory to the nearest `aura.toml`, the way Cargo finds its
manifest.

## Projects: `aura.toml`

```toml
[package]
name = "myapp"          # names the program and the build output
version = "0.1.0"       # informational

[build]
entry = "src/main.aura" # must contain class Program { static void Main() }
sources = ["src"]       # extra roots: directories (recursive) or files
output = "build"        # where `aura build` writes myapp.aurac

[run]                   # defaults for `aura run` / `aura test`;
jit = false             # command-line flags override these
gc-mode = "balanced"    # throughput | balanced | latency
gc-threshold = 65536    # bytes
gc-nursery = 32768      # bytes
gc-max-heap = 16777216  # bytes; exceeding it is a runtime error
```

Every key above is optional except `package.name`; the values shown for
`[build]` are the defaults. Unknown keys are **errors**, not ignored — a
typo cannot silently do nothing.

Multi-file semantics: all `.aura` files under the source roots compile
together into one namespace. Each file is a *module*; `internal` members
are visible only within their file.

## `aura run`

```
aura run                       # the current project
aura run main.aura util.aura   # explicit sources (first file = program name)
aura run build/myapp.aurac     # a compiled module (must be the only path)
```

| Flag | Effect |
|---|---|
| `--jit` | enable the x86-64 JIT (hot methods compile to native code) |
| `--watch` | re-compile and re-run on every source change (1s polling) |
| `--stats` | compile time, run time, JIT method count on stderr |
| `--gc-*` | collector tuning — see [GC flags](#gc-flags) |

In watch mode, compile and runtime failures are reported and watching
continues — fix the file and save.

## `aura test`

Each `.aura` file under `tests/` is an independent program: it declares its
own `class Program { static void Main() }` and is compiled together with
every project source **except the entry file** (whose `Program` would
collide). A test passes when `Main` returns; it fails by throwing or by any
other runtime error. `aura test parse` runs only test files whose name
contains `parse`. The `[run]` manifest section applies (so a project can
run its tests under the JIT by default), and the command exits non-zero on
any failure.

## `aura fmt`

Reindents by bracket depth (4 spaces, one extra level for continuation
lines), strips trailing whitespace, and normalizes the final newline —
nothing else. Interiors of block comments, `"""` strings, and multi-line
raw strings are preserved byte-for-byte.

Safety property worth trusting: after formatting, the output is re-lexed
and its token stream compared with the original's; on any difference the
file is left untouched and the command fails. The formatter is incapable of
changing what your program means.

`--check` reports files that would change (non-zero exit) without writing —
made for CI.

## `aura lint`

| Check | Fires on |
|---|---|
| unused local | a declared local never read (writes don't count) |
| unreachable code | statements after `return` / `throw` / `break` / `continue` |
| empty catch | a catch clause with an empty body |

Prefix a name with `_` to mark it intentionally unused. Parameters,
pattern bindings, and `using` bindings are exempt by design. Exits non-zero
when anything is reported.

## `aura doc`

Generates Markdown: one section per declaration, public and protected
members only, signatures rendered with modifiers, generics, and `throws`.
A `///` comment block directly above a declaration becomes its prose:

```aura
/// A 2D point with integer coordinates.
class Point {
    /// Distance from the origin, squared.
    int NormSquared() { return 0; }
}
```

`aura doc -o api.md` writes a file; without `-o` the Markdown goes to
stdout for piping.

## `aura repl`

Declarations (classes, enums, records, ...) accumulate at top level;
statements accumulate in a synthesized `Main`; a bare expression prints its
value. Unbalanced braces continue the input on the next line. Session
commands: `:show` (print the synthesized program), `:clear` (reset),
`:help`, `:quit`.

Under the hood each input recompiles and reruns the whole session, showing
only the new output — exact because the VM is deterministic. Consequence:
keep `Console.ReadLine()` out of REPL sessions (it shares stdin with the
REPL, and replays would re-read).

## Debugging

`aura debug program.aura --break 12` starts the interactive debugger:
breakpoints (`b`/`d`), bytecode breakpoints (`bb Class.Method <op>`),
step into/over/out (`s`/`n`/`o`), `locals`, `p <path>`, watches
(`w`/`unw`), disassembly (`dis`), backtrace (`bt`). `aura dap` serves the
same engine over the Debug Adapter Protocol for VS Code / nvim-dap. Both
run on the interpreter tier. Details: [debugging.md](debugging.md).

## GC flags

The collector is generational (nursery + tenured) with an optional
concurrent mode. All knobs apply to `aura run` and, via `[run]`, to
projects:

| Flag | Meaning |
|---|---|
| `--gc-threshold BYTES` | full-heap collection trigger (adaptive growth) |
| `--gc-nursery BYTES` | nursery size (default derived, ≤64KiB) |
| `--gc-max-heap BYTES` | hard limit; still over it after a full collection → runtime error |
| `--gc-mode MODE` | `throughput` (fewer, bigger pauses) / `balanced` / `latency` |
| `--gc-pause-target-ms MS` | best-effort soft target for *minor* pauses |
| `--gc-concurrent` | majors become background marking with brief pauses |
| `--gc-stats` | collection counts, pause times, heap numbers on stderr |

Two honest notes: major collections in the default mode are
stop-the-world and unbounded (the pause target governs minors), and under
`--gc-concurrent` reclamation timing depends on marker speed — which is why
deterministic stop-the-world remains the default.

## Environment

The `aura` binary is fully self-contained — no runtime dependencies beyond
the platform C library. The JIT activates on x86-64 Linux and Windows;
other platforms run the (identical-semantics) interpreter.
