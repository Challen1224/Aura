# GDB extension for the Aura VM — CPython's libpython.py model: teach GDB
# to read the interpreter's own data structures and render source-level
# views. Nothing here teaches GDB to *execute* Aura; it is inspection for
# VM crash/hang triage (the DAP adapter and `aura debug` cover live
# source-level debugging on a healthy VM).
#
# Usage:
#   cargo build --profile gdb -p aura-cli
#   gdb -x tools/gdb/aura_gdb.py --args target/gdb/aura run prog.aura
#   (gdb) aura-bt        # Aura-level backtrace of the current VM
#   (gdb) aura-locals    # named locals of the innermost Aura frame
#   (gdb) aura-line      # current source line of the innermost frame
#
# Requirements: the aura binary built with debuginfo for aura-vm and
# aura-bytecode (the opt-in `gdb` cargo profile). The VM registers
# itself in the no-mangle static `AURA_CURRENT_VM` for the whole `run`, so
# the commands work at any stop inside the run — including crashes.
#
# Layout notes: Vec/String are decoded through their std internals
# (buf.inner.ptr.pointer.pointer / len), which are stable in practice but
# not API; `find_vm` and the decoders keep every layout assumption in one
# place so a toolchain bump is a one-file fix.

import gdb


def vec_parts(vec):
    """(data_ptr_as_u8, length) of an alloc::vec::Vec value."""
    ptr = vec["buf"]["inner"]["ptr"]["pointer"]["pointer"]
    return ptr, int(vec["len"])


def vec_elems(vec, elem_type):
    ptr, length = vec_parts(vec)
    typed = ptr.cast(elem_type.pointer())
    return [(typed + i).dereference() for i in range(length)]


def vec_elem_type(vec):
    """Element type of a Vec value, via its type name."""
    name = str(vec.type.strip_typedefs())
    inner = name.split("<", 1)[1]
    depth, end = 0, 0
    for i, c in enumerate(inner):
        if c in "<([":
            depth += 1
        elif c in ">)]":
            depth -= 1
        elif c == "," and depth == 0:
            end = i
            break
    return gdb.lookup_type(inner[:end].strip())


def rust_string(s):
    """Contents of an alloc::string::String value."""
    ptr, length = vec_parts(s["vec"])
    if length == 0:
        return ""
    return (
        gdb.selected_inferior().read_memory(int(ptr), length).tobytes().decode(
            "utf-8", "replace"
        )
    )


def find_vm():
    """The registered running VM, or None.

    `AURA_CURRENT_VM` is a no-mangle static that DWARF never describes
    (Rust never reads it), so it exists only as an ELF minimal symbol;
    `info address` resolves it with relocation applied.
    """
    try:
        info = gdb.execute("info address AURA_CURRENT_VM", to_string=True)
    except gdb.error:
        return None
    import re

    m = re.search(r"0x[0-9a-fA-F]+", info)
    if not m:
        return None
    addr = int(m.group(), 16)
    try:
        raw = gdb.selected_inferior().read_memory(addr, 8).tobytes()
    except gdb.error:
        return None
    vm_addr = int.from_bytes(raw, "little")
    if vm_addr == 0:
        return None
    vm_ptr_type = gdb.lookup_type("aura_vm::Vm").pointer()
    return gdb.Value(vm_addr).cast(vm_ptr_type).dereference()


def method_index(vm):
    """method_id -> (label, line_starts, local_names) from vm.gdb_index."""
    index = {}
    entries = vec_elems(vm["gdb_index"], vec_elem_type(vm["gdb_index"]))
    for e in entries:
        pairs = []
        ls = e["line_starts"]
        ptr, length = vec_parts(ls)
        pair_type = vec_elem_type(ls)
        typed = ptr.cast(pair_type.pointer())
        for i in range(length):
            pair = (typed + i).dereference()
            pairs.append((int(pair["__0"]), int(pair["__1"])))
        # `label`'s own type is a *complete* String DIE — lookups by
        # name can land on declaration-only stubs.
        names = [rust_string(n) for n in vec_elems(e["local_names"], e["label"].type)]
        index[int(e["method_id"])] = (rust_string(e["label"]), pairs, names)
    return index


def line_for_pc(pairs, pc):
    line = 0
    for op, ln in pairs:
        if op > pc:
            break
        line = ln
    return line


def frames(vm):
    frame_type = gdb.lookup_type("aura_vm::Frame")
    return vec_elems(vm["call_stack"], frame_type)


def frame_line(index, frame, innermost):
    mid = int(frame["method_id"]["__0"])
    label, pairs, _ = index.get(mid, ("<method %d>" % mid, [], []))
    pc = int(frame["pc"])
    if not innermost:
        pc = max(pc - 1, 0)
    return label, line_for_pc(pairs, pc), pc


class AuraBt(gdb.Command):
    """Aura-level backtrace of the running VM (innermost last)."""

    def __init__(self):
        super().__init__("aura-bt", gdb.COMMAND_STACK)

    def invoke(self, arg, from_tty):
        vm = find_vm()
        if vm is None:
            print("no Aura VM is running (AURA_CURRENT_VM is null)")
            return
        index = method_index(vm)
        fs = frames(vm)
        if not fs:
            print("Aura call stack is empty")
            return
        for i, f in enumerate(fs):
            label, line, pc = frame_line(index, f, i == len(fs) - 1)
            jit = " [jit]" if bool(f["is_jit"]) else ""
            where = "line %d" % line if line else "pc %d" % pc
            print("#%-2d %s (%s)%s" % (i, label, where, jit))


class AuraLocals(gdb.Command):
    """Named locals of the innermost Aura frame."""

    def __init__(self):
        super().__init__("aura-locals", gdb.COMMAND_STACK)

    def invoke(self, arg, from_tty):
        vm = find_vm()
        if vm is None:
            print("no Aura VM is running (AURA_CURRENT_VM is null)")
            return
        fs = frames(vm)
        if not fs:
            print("Aura call stack is empty")
            return
        frame = fs[-1]
        index = method_index(vm)
        mid = int(frame["method_id"]["__0"])
        _, _, names = index.get(mid, ("", [], []))
        values = vec_elems(frame["locals"], vec_elem_type(frame["locals"]))
        shown = 0
        for slot, name in enumerate(names):
            if not name or name.startswith("__") or slot >= len(values):
                continue
            rendered = str(values[slot]).replace("aura_bytecode::Value::", "").replace("aura_bytecode::", "")
            print("%s = %s" % (name, rendered))
            shown += 1
        if not shown:
            print("no named locals")


class AuraLine(gdb.Command):
    """Current method and source line of the innermost Aura frame."""

    def __init__(self):
        super().__init__("aura-line", gdb.COMMAND_STACK)

    def invoke(self, arg, from_tty):
        vm = find_vm()
        if vm is None:
            print("no Aura VM is running (AURA_CURRENT_VM is null)")
            return
        fs = frames(vm)
        if not fs:
            print("Aura call stack is empty")
            return
        index = method_index(vm)
        label, line, pc = frame_line(index, fs[-1], True)
        print("%s, line %d (pc %d)" % (label, line, pc))


AuraBt()
AuraLocals()
AuraLine()
print("aura_gdb: commands loaded (aura-bt, aura-locals, aura-line)")
