# Getting Started

This page takes you from nothing to a working Aura project with tests, in
about five minutes.

## Installing

**Debian / Ubuntu (including ARM devices):**

```bash
sudo dpkg -i aura_<version>_<arch>.deb
aura --version
```

**Windows (x64):** run `Aura-<version>-x64.msi`. It installs to
`C:\Program Files\Aura` and puts `aura.exe` on your `PATH` — open a new
terminal afterwards so the updated `PATH` is picked up.

**From source (any platform with Rust):**

```bash
git clone https://github.com/Challen1224/Aura.git
cd Aura && cargo build --release
# the binary is target/release/aura
```

Both installers also ship the example programs and this documentation —
under `/usr/share/aura` on Linux and `C:\Program Files\Aura` on Windows.

## Hello, Aura

Every Aura program starts at `Main` in a class named `Program`. Save this as
`hello.aura`:

```aura
class Program {
    static void Main() {
        print("Hello, Aura!");
        println();
    }
}
```

Run it:

```bash
aura run hello.aura
```

`print` writes a value without a newline; `println()` ends the line. String
literals interpolate expressions in braces:

```aura
class Program {
    static void Main() {
        string who = "world";
        int year = 2026;
        print("Hello {who}, it is {year}!");
        println();
    }
}
```

## A real project

For anything beyond a single file, let `aura init` set up a project:

```bash
aura init myapp
cd myapp
```

This creates:

```
myapp/
├── aura.toml          # the project manifest
├── src/
│   └── main.aura      # the entry file (contains Program.Main)
└── tests/
    └── smoke_test.aura
```

From inside the project directory, the commands need no arguments:

```bash
aura run           # compile and run src/
aura test          # run every program under tests/
aura build         # emit build/myapp.aurac (compiled bytecode)
aura run build/myapp.aurac
```

All `.aura` files under `src/` compile together: classes in one file can use
classes from any other, and each file is a *module* — members marked
`internal` are visible only within their own file.

While developing, two flags earn their keep:

```bash
aura run --watch   # re-compile and re-run on every save
aura run --stats   # compile/run timing and JIT counters on stderr
```

## Your first test

A test in Aura is a program: each `.aura` file under `tests/` is compiled
together with your sources (minus the entry file) and passes when its `Main`
returns without a runtime error. Failing is throwing:

```aura
class Program {
    static void Main() {
        int sum = 2 + 2;
        if (sum != 4) {
            throw new Exception();
        }
    }
}
```

```bash
aura test            # runs every test, prints PASS/FAIL per file
aura test smoke      # only files whose name contains "smoke"
```

## The tools you'll use daily

```bash
aura repl            # interactive session — try the language
aura fmt             # normalize formatting (token-safe; never changes meaning)
aura lint            # unused locals, unreachable code, empty catch blocks
aura doc -o api.md   # Markdown API docs from your declarations and /// comments
aura debug src/main.aura --break 5   # step through your program
```

A quick REPL taste — declarations, statements, and bare expressions all
work, and a bare expression prints its value:

```
$ aura repl
aura> int x = 40;
aura> x + 2
42
aura> class Doubler {
  ...     static int Twice(int n) { return n * 2; }
  ... }
aura> Doubler.Twice(21)
42
```

## Where next

Work through the [Language Guide](index.md#the-language-guide) in order —
it assumes nothing after this page — or jump straight to the chapter you
need. The `examples/` directory that ships with Aura holds 70+ small
programs, one per feature, that all run under both the interpreter and the
JIT.
