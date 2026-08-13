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

There are 40+ example programs under [`examples/`](examples/) that exercise the
language; a change to the compiler or VM should keep them all running.

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
# Debian/Ubuntu: install the cross linker and qemu
sudo apt install -y gcc-x86-64-linux-gnu qemu-user-static

# Point cargo at them (this is a per-user, machine-specific setting)
mkdir -p ~/.cargo
cat >> ~/.cargo/config.toml <<'EOF'
[target.x86_64-unknown-linux-gnu]
linker = "x86_64-linux-gnu-gcc"
runner = "qemu-x86_64-static"
EOF

# Compile and run the whole JIT test suite under QEMU
cargo test -p aura-vm --target x86_64-unknown-linux-gnu

# Or run a real program with the JIT enabled
cargo run -p aura-cli --target x86_64-unknown-linux-gnu -- run --jit examples/fib.aura
```

Remember QEMU emulates the compiled code, so this verifies *correctness*, not
performance — there is no speedup compared with the interpreter.

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
