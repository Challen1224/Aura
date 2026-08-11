# Aura

A from-scratch, statically-typed, object-oriented programming language and runtime, architected similarly to C# / .NET CLR.

**Host language:** Rust

## Components

| Crate | Role |
|-------|------|
| `aura-bytecode` | Custom bytecode ISA, values, object model, method/class metadata |
| `aura-vm` | Stack-based VM, managed heap, mark-and-sweep GC |
| `aura-compiler` | Lexer → parser → type-checker → bytecode emitter |
| `aura-cli` | `aura` CLI — compile and run `.aura` programs |

## Quick start

```bash
# Build
cargo build --release

# Run a program
cargo run -p aura-cli -- run examples/hello.aura

# Compile to bytecode dump
cargo run -p aura-cli -- compile examples/hello.aura
```

## Language sketch

```aura
class Program {
    static void Main() {
        int x = 40;
        int y = 2;
        print(x + y);
    }
}
```

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
┌──────────────────────────────────────────────────────────┐
│  VM: call stack · eval stack · locals · managed heap · GC │
└──────────────────────────────────────────────────────────┘
```
