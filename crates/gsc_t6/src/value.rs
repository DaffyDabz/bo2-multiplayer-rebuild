//! Script values.
//!
//! Strings are interned (`Str`): field names, notify names and string
//! constants compare by id. Arrays have value semantics: assigning or passing
//! one copies it, done here as a shared object copied on a script's first
//! write while shared (as the engine's refcounted arrays are); the engine's
//! own array functions (`arrayremovevalue`, `arrayinsert`, ...) change the
//! shared object in place, which scripts rely on. Structs,
//! entities and the `level`/`anim` objects are references into the VM's
//! object table (`ObjRef`, with a generation so a deleted entity reads as
//! undefined).

use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use indexmap::IndexMap;
use rustc_hash::{FxBuildHasher, FxHashMap};

/// An interned string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Str(pub u32);

/// The string table. Every id stays valid for the VM's life.
#[derive(Default, Debug)]
pub struct Strings {
    list: Vec<Arc<str>>,
    map: FxHashMap<Arc<str>, Str>,
}

impl Strings {
    pub fn intern(&mut self, s: &str) -> Str {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = Str(self.list.len() as u32);
        let rc: Arc<str> = Arc::from(s);
        self.list.push(rc.clone());
        self.map.insert(rc, id);
        id
    }

    pub fn find(&self, s: &str) -> Option<Str> {
        self.map.get(s).copied()
    }

    pub fn get(&self, id: Str) -> &str {
        self.list.get(id.0 as usize).map_or("", |s| s)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// A reference to an object (struct, entity, level, anim, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjRef {
    pub index: u32,
    pub generation: u32,
}

/// A function value: a script function, a builtin, or a name nothing defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FuncRef {
    Script(u32),
    Builtin(u32),
    Missing(Str),
}

/// An array key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Int(i32),
    Str(Str),
    /// Entities and structs can key an array in Black Ops II.
    Obj(ObjRef),
}

impl Key {
    pub fn value(self) -> Value {
        match self {
            Key::Int(i) => Value::Int(i),
            Key::Str(s) => Value::Str(s),
            Key::Obj(o) => Value::Object(o),
        }
    }
}

/// Array storage: insertion order kept; iteration (`getfirstarraykey`,
/// `getnextarraykey`, `getarraykeys`) runs newest first.
pub type ArrayMap = IndexMap<Key, Value, FxBuildHasher>;

#[derive(Clone, Debug, Default)]
pub struct Array {
    pub map: ArrayMap,
}

impl Array {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, k: &Key) -> Option<&Value> {
        self.map.get(k)
    }

    /// Store; undefined removes the key (order of the rest kept).
    pub fn set(&mut self, k: Key, v: Value) {
        if matches!(v, Value::Undefined) {
            self.map.shift_remove(&k);
        } else {
            self.map.insert(k, v);
        }
    }

    pub fn push(&mut self, v: Value) {
        let k = Key::Int(self.map.len() as i32);
        self.set(k, v);
    }

    /// The first key in iteration order (the newest).
    pub fn first_key(&self) -> Option<Key> {
        self.map.last().map(|(k, _)| *k)
    }

    /// The key after `k` in iteration order (the next older one).
    pub fn next_key(&self, k: &Key) -> Option<Key> {
        let i = self.map.get_index_of(k)?;
        if i == 0 {
            return None;
        }
        self.map.get_index(i - 1).map(|(k, _)| *k)
    }

    /// Keys in iteration order.
    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.map.keys().rev().copied()
    }

    /// Values in index order 0..n when the array is a plain list, else in
    /// insertion order.
    pub fn values_in_order(&self) -> impl Iterator<Item = &Value> + '_ {
        self.map.values()
    }
}

/// A script array: shared, copied on a script write while shared.
#[derive(Clone, Debug, Default)]
pub struct ArrayRef(Arc<RwLock<Array>>);

impl ArrayRef {
    pub fn new(a: Array) -> Self {
        Self(Arc::new(RwLock::new(a)))
    }

    /// Read it (never hold the guard across a call back into the VM).
    pub fn read(&self) -> RwLockReadGuard<'_, Array> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Change it in place, for everyone sharing it (the engine's array
    /// functions do this).
    pub fn write(&self) -> RwLockWriteGuard<'_, Array> {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A copy of the contents.
    pub fn snapshot(&self) -> Array {
        self.read().clone()
    }

    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    /// Before a script writes: a private copy when shared.
    pub fn make_unique(&mut self) {
        if Arc::strong_count(&self.0) > 1 {
            *self = Self::new(self.snapshot());
        }
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    pub fn get(&self, k: &Key) -> Option<Value> {
        self.read().get(k).cloned()
    }
}

/// A script value.
#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Undefined,
    Int(i32),
    Float(f32),
    Str(Str),
    /// A localized string reference (`&"..."`).
    IStr(Str),
    Hash(u32),
    Vec3([f32; 3]),
    Object(ObjRef),
    Array(ArrayRef),
    Func(FuncRef),
    /// An animation (`%anim`): index into the program's animation table.
    Anim(u32),
    /// An animation tree (`#animtree`): index into the program's tree table.
    AnimTree(u32),
    /// Internal: the argument boundary pushed before a call or a notify.
    Marker,
}

impl Value {
    pub fn array(a: Array) -> Value {
        Value::Array(ArrayRef::new(a))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    pub fn as_int(&self) -> Option<i32> {
        match *self {
            Value::Int(i) => Some(i),
            Value::Float(f) => Some(f as i32),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f32> {
        match *self {
            Value::Int(i) => Some(i as f32),
            Value::Float(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_vec3(&self) -> Option<[f32; 3]> {
        match *self {
            Value::Vec3(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<ObjRef> {
        match *self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_str_id(&self) -> Option<Str> {
        match *self {
            Value::Str(s) | Value::IStr(s) => Some(s),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::IStr(_) => "localized string",
            Value::Hash(_) => "hash",
            Value::Vec3(_) => "vector",
            Value::Object(_) => "object",
            Value::Array(_) => "array",
            Value::Func(_) => "function",
            Value::Anim(_) => "animation",
            Value::AnimTree(_) => "animtree",
            Value::Marker => "(marker)",
        }
    }

    pub fn bool(b: bool) -> Value {
        Value::Int(i32::from(b))
    }
}

/// What an object is. Hosts give entities and HUD elements their own kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjKind {
    Struct,
    Level,
    Anim,
    /// A host entity, by entity number.
    Entity(u32),
    /// A host HUD element, by its slot.
    HudElem(u32),
}

#[derive(Debug)]
pub struct Object {
    pub generation: u32,
    pub alive: bool,
    pub kind: ObjKind,
    pub fields: FxHashMap<Str, Value>,
}

/// Format a float the way script string concatenation does (`%g`-like).
pub fn format_float(f: f32) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1e9 {
        return format!("{}", f as i64);
    }
    let s = format!("{f:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_owned()
}
