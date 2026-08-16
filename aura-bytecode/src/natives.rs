//! Registry of native (runtime-implemented) stdlib functions.
//!
//! The stdlib's intrinsic types (`List<T>`, `Map<K, V>`, `Set<T>`, string
//! methods, `Console`, `File`) have no bytecode bodies. Calls to them compile
//! to [`crate::Op::NativeCall`] carrying a [`NativeId`], and the VM executes
//! the matching Rust implementation. This registry is the single source of
//! truth shared by the compiler (which emits the ids), the interpreter and
//! the JIT (which need each native's argument count for stack effects).
//!
//! Stack convention for `NativeCall`: the emitter pushes the receiver first
//! (for instance natives), then the remaining arguments left to right. The op
//! pops [`NativeId::arity`] values and always pushes exactly one result
//! (`Unit` for `void` natives).

macro_rules! natives {
    ($($variant:ident = $id:literal, $name:literal, arity $arity:literal;)*) => {
        /// Identifies a native stdlib function.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum NativeId {
            $(
                #[doc = $name]
                $variant = $id,
            )*
        }

        impl NativeId {
            /// Resolve an id from its `u16` encoding.
            pub fn from_u16(v: u16) -> Option<NativeId> {
                match v {
                    $($id => Some(NativeId::$variant),)*
                    _ => None,
                }
            }

            /// Diagnostic name, e.g. `"List.Add"`.
            pub fn name(self) -> &'static str {
                match self {
                    $(NativeId::$variant => $name,)*
                }
            }

            /// Number of stack arguments (receiver included for instance
            /// natives). Every native additionally pushes exactly one result.
            pub fn arity(self) -> usize {
                match self {
                    $(NativeId::$variant => $arity,)*
                }
            }
        }
    };
}

natives! {
    // List<T>
    ListNew = 0, "List.new", arity 0;
    ListAdd = 1, "List.Add", arity 2;
    ListGet = 2, "List.Get", arity 2;
    ListSet = 3, "List.Set", arity 3;
    ListInsert = 4, "List.Insert", arity 3;
    ListRemoveAt = 5, "List.RemoveAt", arity 2;
    ListIndexOf = 6, "List.IndexOf", arity 2;
    ListContains = 7, "List.Contains", arity 2;
    ListClear = 8, "List.Clear", arity 1;
    ListCount = 9, "List.Count", arity 1;

    // Map<K, V>
    MapNew = 20, "Map.new", arity 0;
    MapPut = 21, "Map.Put", arity 3;
    MapGet = 22, "Map.Get", arity 2;
    MapContainsKey = 23, "Map.ContainsKey", arity 2;
    MapRemove = 24, "Map.Remove", arity 2;
    MapKeys = 25, "Map.Keys", arity 1;
    MapValues = 26, "Map.Values", arity 1;
    MapClear = 27, "Map.Clear", arity 1;
    MapCount = 28, "Map.Count", arity 1;

    // Set<T>
    SetNew = 40, "Set.new", arity 0;
    SetAdd = 41, "Set.Add", arity 2;
    SetContains = 42, "Set.Contains", arity 2;
    SetRemove = 43, "Set.Remove", arity 2;
    SetToList = 44, "Set.ToList", arity 1;
    SetClear = 45, "Set.Clear", arity 1;
    SetCount = 46, "Set.Count", arity 1;

    // string methods (receiver is a string value)
    StrLength = 60, "string.Length", arity 1;
    StrSubstring = 61, "string.Substring", arity 3;
    StrCharAt = 62, "string.CharAt", arity 2;
    StrContains = 63, "string.Contains", arity 2;
    StrStartsWith = 64, "string.StartsWith", arity 2;
    StrEndsWith = 65, "string.EndsWith", arity 2;
    StrIndexOf = 66, "string.IndexOf", arity 2;
    StrSplit = 67, "string.Split", arity 2;
    StrTrim = 68, "string.Trim", arity 1;
    StrToUpper = 69, "string.ToUpper", arity 1;
    StrToLower = 70, "string.ToLower", arity 1;
    StrReplace = 71, "string.Replace", arity 3;
    StrToInt = 72, "string.ToInt", arity 1;
    StrToFloat = 73, "string.ToFloat", arity 1;

    // Console
    ConsoleReadLine = 90, "Console.ReadLine", arity 0;

    // Core language support
    AssertNonNull = 110, "non-null assertion", arity 1;

    // File
    FileReadAllText = 100, "File.ReadAllText", arity 1;
    FileWriteAllText = 101, "File.WriteAllText", arity 2;
    FileAppendAllText = 102, "File.AppendAllText", arity 2;
    FileExists = 103, "File.Exists", arity 1;
    FileDelete = 104, "File.Delete", arity 1;
    FileReadAllLines = 105, "File.ReadAllLines", arity 1;

    // Tasks
    TaskPause = 120, "Tasks.pause", arity 0;
    TaskWaitStub = 121, "Task.wait", arity 1;
    TasksAll = 122, "Tasks.all", arity 1;
    TasksRace = 123, "Tasks.race", arity 1;

    // Weak references
    WeakNew = 130, "WeakRef.new", arity 1;
    WeakIsAlive = 131, "WeakRef.isAlive", arity 1;
    WeakGet = 132, "WeakRef.get", arity 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_ids() {
        for id in [
            NativeId::ListNew,
            NativeId::MapPut,
            NativeId::SetCount,
            NativeId::StrSplit,
            NativeId::ConsoleReadLine,
            NativeId::FileReadAllLines,
        ] {
            assert_eq!(NativeId::from_u16(id as u16), Some(id), "{}", id.name());
        }
        assert_eq!(NativeId::from_u16(9999), None);
    }
}
