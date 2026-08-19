# Contributing

Thanks for your interest in Aura! This document covers how to build, test, and
submit changes.

## Getting started

```bash
# Build the whole workspace
cargo build

# Run the test suites
cargo test --workspace

# Build in release mode
cargo build --release
```

Aura is split into four crates:

| Crate           | Role                                                        |
|-----------------|-------------------------------------------------------------|
| `aura-bytecode` | Bytecode ISA, `Value` model, object/metadata types          |
| `aura-vm`       | Stack VM, managed heap, mark-and-sweep GC, x86-64 JIT       |
| `aura-compiler` | Lexer → parser → type-checker → bytecode emitter            |
| `aura-cli`      | `aura` CLI — compile and run `.aura` programs               |

## Running the language

```bash
# Run an example program
cargo run -p aura-cli -- run examples/hello.aura

# Dump the bytecode for a program
cargo run -p aura-cli -- compile examples/hello.aura
```

The CLI also has a project workflow (`aura init` / `build` / `test` with an
`aura.toml` manifest) and developer tools (`repl`, `fmt`, `lint`, `doc`,
`run --watch`, `run --stats`) — see the README. Their end-to-end tests live
in `aura-cli/tests/cli_tools.rs`; `aura fmt` in particular must stay
token-safe (it re-lexes its output and refuses any change to the token
stream), so formatter changes need a run of the fmt tests plus the
examples-sweep.

There are 70+ example programs under [`examples/`](examples/) that exercise the
language; a change to the compiler or VM should keep them all running.
`tools/examples-sweep.sh` runs every example under both tiers and fails if any
example errors or the interpreter and JIT outputs differ.

## Testing

- Run `cargo test` at the workspace root.
- The JIT's lowering/CFG tests live in `aura-vm/src/jit/ir.rs` and run on every
  architecture.
- The x86-64 JIT (codegen, register allocation, executable memory) is only
  compiled on x86-64 hosts. Verify it with:

  ```bash
  cargo check -p aura-vm --target x86_64-unknown-linux-gnu
  ```

  Runtime validation of JIT-compiled code requires an x86-64 host, as the
  generated machine code is architecture-specific.

### Running JIT tests on non-x86-64 hosts (e.g. an ARM Chromebook)

On an ARM (aarch64) machine you can still compile **and run** the x86-64 JIT
tests using a cross-toolchain plus QEMU user-mode emulation. QEMU executes the
JIT's generated x86-64 machine code, so the full 9-test suite runs:

```bash
# Debian/Ubuntu: install the cross toolchain and qemu
sudo apt install -y gcc-x86-64-linux-gnu libc6-dev-amd64-cross qemu-user
rustup target add x86_64-unknown-linux-gnu

# The repo's .cargo/config.toml already points this target at the cross
# linker and a qemu runner, so this just works:
cargo test -p aura-vm --target x86_64-unknown-linux-gnu

# Or run a real program with the JIT enabled
cargo run -p aura-cli --target x86_64-unknown-linux-gnu -- run --jit examples/fib.aura
```

Remember QEMU emulates the compiled code, so this verifies *correctness*, not
performance — there is no speedup compared with the interpreter.

Tests that spawn the built `aura` binary as a child process (the DAP test)
additionally need binfmt-level qemu and the x86-64 loader/libc visible at
their standard paths:

```bash
sudo apt install -y qemu-user-binfmt
sudo ln -s /usr/x86_64-linux-gnu/lib /lib64
for f in /usr/x86_64-linux-gnu/lib/*.so*; do sudo ln -sf "$f" /usr/lib/x86_64-linux-gnu/; done
```

## Building the Windows installer

`packaging/windows/build-msi.sh` cross-compiles `aura.exe` for
`x86_64-pc-windows-gnu` and packages `target/Aura-<version>-x64.msi`, entirely
from Linux:

```bash
sudo apt install -y gcc-mingw-w64-x86-64 wixl
rustup target add x86_64-pc-windows-gnu
packaging/windows/build-msi.sh
```

The WiX source is `packaging/windows/aura.wxs`. Its `UpgradeCode` GUID is
permanent — never change it, or upgrades will stack installs instead of
replacing them.

## Building the Debian package

`packaging/debian/build-deb.sh` builds `target/aura_<version>_<arch>.deb` for
the architecture it runs on (arm64 on an ARM machine, amd64 on x86-64) — only
a Rust toolchain and `dpkg-deb` are needed:

```bash
packaging/debian/build-deb.sh
sudo dpkg -i target/aura_*_$(dpkg --print-architecture).deb
```

Shared-library dependencies are computed from the built binary with
`dpkg-shlibdeps`, so the control file's `Depends` tracks the toolchain
instead of being hand-maintained. Bump `packaging/debian/changelog` when
cutting a release.

## Style

- Run `cargo fmt` before submitting changes.
- Keep functions small and documented; the crates build with `#![warn(missing_docs)]`.
- Prefer the existing patterns — the compiler and VM favor simple, explicit code
  over clever abstractions.
- Avoid adding comments that restate the code; document *why*, not *what*.

## Committing

- Write concise commit messages describing the change and its motivation.
- Keep related changes (e.g. bytecode + VM + compiler) in a single commit when
  they form one feature.

## Reporting issues

Include:

- What you ran (command line and any relevant source file).
- What you expected, and what happened instead.
- Your host architecture (the JIT is x86-64 only).

## License

Aura is released under the [MIT license](LICENSE).
