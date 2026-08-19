# Aura

A from-scratch, statically-typed, object-oriented programming language and
runtime, architected like C# / .NET CLR and written entirely in Rust.

This program aims at not replacing C#/.NET CLR but showing what 1 solo developer and an AI companion can accomplish. This is just... a project I thought would challenge my programming abilities in compilers. Even with an AI companion who did most of the debugging to save time I still put effort into making it as smooth and fast!

```aura
class Program {
    static void Main() {
        int x = 40;
        int y = 2;
        print(x + y);
    }
}
```

## Highlights

- **Modern type system** — numeric hierarchy (`int8`–`int64`, `uint8`–`uint64`,
  `float32`/`float64`), `char`, `bool`, `string`, `null`
- **Object-oriented core** — classes, interfaces, inheritance, virtual dispatch,
  abstract/sealed/final, properties, records with value semantics
- **Algebraic data types** — enums with pattern matching, generic sum types,
  tuples with destructuring
- **Generics** — reified generic classes and methods with constraints
- **Exceptions** — try/catch/finally, custom exception classes, stack traces,
  `using` for resource management
- **Rich expressions** — ranges, pattern guards, null-coalescing (`??`) and
  null-conditional (`?.`), expression blocks, labeled loops
- **Tiered JIT** (x86-64) — hot methods compile to native code; the rest run on
  a bytecode interpreter

## Install

**Windows (x64):** download `Aura-<version>-x64.msi` from the releases page and
run it. The installer puts `aura.exe` on your `PATH` (open a new terminal after
installing), along with the examples and docs under
`C:\Program Files\Aura`. Then:

```
aura run "C:\Program Files\Aura\examples\hello.aura"
```

The installer is built from Linux — no Windows machine or WiX toolset needed —
by [`packaging/windows/build-msi.sh`](packaging/windows/build-msi.sh).

**Linux / anywhere with Rust:** build from source (below).

## Quick start

```bash
# Build the whole toolchain
cargo build --release

# Compile and run a program
cargo run -p aura-cli -- run examples/hello.aura

# Run with the x86-64 JIT enabled (native code once methods are hot)
cargo run -p aura-cli -- run --jit examples/hello.aura

# Compile and dump the resulting bytecode module
cargo run -p aura-cli -- compile examples/hello.aura
```

Try one of the 70+ programs under [`examples/`](examples/):

```bash
cargo run -p aura-cli -- run examples/fib.aura
cargo run -p aura-cli -- run examples/records.aura
cargo run -p aura-cli -- run examples/generics.aura
```

## Components

| Crate           | Role                                                        |
|-----------------|-------------------------------------------------------------|
| `aura-bytecode` | Bytecode ISA, 16-byte `Value` model, object/metadata types  |
| `aura-vm`       | Stack VM, managed heap, mark-and-sweep GC, x86-64 JIT       |
| `aura-compiler` | Lexer → parser → type-checker → bytecode emitter            |
| `aura-cli`      | `aura` CLI — compile and run `.aura` programs               |

### The JIT

`aura-vm` ships a baseline x86-64 JIT. Methods run interpreted until they cross
an invocation threshold, then are compiled to native code via an SSA-like IR,
optimization passes, linear-scan register allocation, and a machine-code
emitter. Complex operations (allocation, calls, field access, exceptions,
`div`/`rem`) delegate to VM helper stubs that reuse the interpreter's exact
semantics.

The JIT is enabled through the VM API (`vm.enable_jit()`) or `aura run --jit`,
and requires an x86-64 host — on Linux it maps executable pages with raw
syscalls, on Windows through the `kernel32` virtual-memory API (both
dependency-free). Generated code always speaks the System V ABI; the Rust
boundary functions are declared `extern "sysv64"`, so no per-OS codegen is
needed. Other architectures fall back to the interpreter.

## Architecture

```
.source.aura
    │
    ▼
┌────────┐   ┌────────┐   ┌──────────────┐   ┌─────────┐
│ Lexer  │ → │ Parser │ → │ Type-checker │ → │ Emitter │
└────────┘   └────────┘   └──────────────┘   └─────────┘
                                                    │
                                                    ▼
                                             Bytecode module
                                                    │
                                                    ▼
┌────────────────────────────────────────────────────────┐
│ VM: call stack · eval stack · locals · heap · GC       │
│ x86-64 JIT (tiered: interpret → compile hot)           │
└────────────────────────────────────────────────────────┘
```

Every `Value` is a fixed 16-byte `[tag, payload]` pair, which lets the JIT and
interpreter share one compact representation and lets compiled code treat a
value as two registers.

## Roadmap

See [TODO.md](TODO.md) for the feature roadmap and current status.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to build, test, and contribute.

## License

Aura is released under the [MIT license](LICENSE).
