//! bo2zm M3: Black Ops II compiled scripts (GSC) for IW4L.
//!
//! [`object`] reads and decodes a script object from a zone, [`program`]
//! links objects into one program, [`vm`] runs it: threads, waits, notifies,
//! endons and the interpreter. [`natives`] holds the builtins that need no
//! engine; the engine binds the rest ([`vm::Vm::bind`]).
//! Not part of upstream IW4L.

pub mod compile;
pub mod math;
pub mod natives;
pub mod object;
pub mod program;
pub mod value;
pub mod vm;

pub use compile::compile;
pub use object::{Import, Insn, ScriptObject, op_name};
pub use program::{CallKind, Program};
pub use value::{Array, ArrayRef, FuncRef, Key, ObjKind, ObjRef, Str, Strings, Value};
pub use vm::{Hooks, Native, ThreadId, Vm, hash_name, truthy};
