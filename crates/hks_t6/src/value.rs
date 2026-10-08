//! The VM's values and tables.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::Proto;

/// A native function: (VM, arguments) -> results.
pub type NativeFn =
    dyn Fn(&mut crate::vm::Vm, Vec<Value>) -> Result<Vec<Value>, crate::vm::LuaError>;

pub struct Native {
    pub name: &'static str,
    pub f: Box<NativeFn>,
}

/// An upvalue: open while its local is still on the stack (the stack
/// slot), closed afterwards (the value).
#[derive(Debug)]
pub enum UpVal {
    Open(usize),
    Closed(Value),
}

pub struct Closure {
    pub proto: Rc<Proto>,
    pub upvals: Vec<Rc<RefCell<UpVal>>>,
}

/// Engine-side data a script holds (an LUI element, a material).
pub struct UserData {
    pub kind: &'static str,
    pub data: RefCell<Box<dyn std::any::Any>>,
    pub meta: RefCell<Option<TableRef>>,
}

pub type TableRef = Rc<RefCell<Table>>;

#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Num(f32),
    Str(Rc<str>),
    Table(TableRef),
    Func(Rc<Closure>),
    Native(Rc<Native>),
    User(Rc<UserData>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(Rc::from(s))
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Func(_) | Value::Native(_) => "function",
            Value::User(_) => "userdata",
        }
    }

    pub fn as_num(&self) -> Option<f32> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Str(s) => parse_number(s),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn table(&self) -> Option<&TableRef> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    /// Raw equality (no metamethods): numbers by value, strings by
    /// content, the rest by identity.
    pub fn raw_eq(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(a, b),
            (Value::Func(a), Value::Func(b)) => Rc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => Rc::ptr_eq(a, b),
            (Value::User(a), Value::User(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Lua's number syntax (decimal or 0x hex, surrounding spaces allowed).
pub fn parse_number(s: &str) -> Option<f32> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u32::from_str_radix(h, 16).ok().map(|v| v as f32);
    }
    t.parse::<f32>().ok().filter(|_| !t.is_empty())
}

/// How a number prints (Lua's %.14g, at the float's own precision).
pub fn format_number(n: f32) -> String {
    if n.is_finite() && n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else if n.is_nan() {
        "nan".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        }
    } else {
        format!("{n}")
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => f.write_str("nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => f.write_str(&format_number(*n)),
            Value::Str(s) => f.write_str(s),
            Value::Table(t) => write!(f, "table: {:p}", Rc::as_ptr(t)),
            Value::Func(c) => write!(f, "function: {:p}", Rc::as_ptr(c)),
            Value::Native(n) => write!(f, "function: builtin {}", n.name),
            Value::User(u) => write!(f, "userdata {}: {:p}", u.kind, Rc::as_ptr(u)),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{s:?}"),
            other => write!(f, "{other}"),
        }
    }
}

/// A table key: values that can index (not nil, not NaN), hashed by the
/// raw equality rules.
#[derive(Clone)]
pub struct Key(pub Value);

impl PartialEq for Key {
    fn eq(&self, o: &Key) -> bool {
        self.0.raw_eq(&o.0)
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, h: &mut H) {
        match &self.0 {
            Value::Nil => 0u8.hash(h),
            Value::Bool(b) => b.hash(h),
            Value::Num(n) => {
                // -0 and 0 are one key.
                let n = if *n == 0.0 { 0.0f32 } else { *n };
                n.to_bits().hash(h)
            }
            Value::Str(s) => s.hash(h),
            Value::Table(t) => (Rc::as_ptr(t) as usize).hash(h),
            Value::Func(c) => (Rc::as_ptr(c) as usize).hash(h),
            Value::Native(n) => (Rc::as_ptr(n) as usize).hash(h),
            Value::User(u) => (Rc::as_ptr(u) as usize).hash(h),
        }
    }
}

/// A table: an array part for 1..n and a hash part for the rest, in
/// insertion order so `next` is stable while a loop runs.
#[derive(Default)]
pub struct Table {
    pub array: Vec<Value>,
    pub hash: HashMap<Key, usize>,
    pub entries: Vec<(Key, Value)>,
    pub meta: Option<TableRef>,
}

impl Table {
    pub fn new_ref() -> TableRef {
        Rc::new(RefCell::new(Table::default()))
    }

    fn array_index(k: &Value) -> Option<usize> {
        match k {
            Value::Num(n) if *n >= 1.0 && *n == n.trunc() && *n <= 16_777_216.0 => {
                Some(*n as usize - 1)
            }
            _ => None,
        }
    }

    pub fn get(&self, k: &Value) -> Value {
        if let Some(i) = Self::array_index(k)
            && i < self.array.len()
        {
            return self.array[i].clone();
        }
        match self.hash.get(&Key(k.clone())) {
            Some(&i) => self.entries[i].1.clone(),
            None => Value::Nil,
        }
    }

    pub fn get_str(&self, k: &str) -> Value {
        self.get(&Value::str(k))
    }

    pub fn set(&mut self, k: Value, v: Value) {
        if let Some(i) = Self::array_index(&k) {
            if i < self.array.len() {
                self.array[i] = v;
                if i + 1 == self.array.len() {
                    while matches!(self.array.last(), Some(Value::Nil)) {
                        self.array.pop();
                    }
                }
                return;
            }
            if i == self.array.len() && !matches!(v, Value::Nil) {
                self.array.push(v);
                // Pull following keys out of the hash part.
                loop {
                    let next = Value::Num((self.array.len() + 1) as f32);
                    let key = Key(next);
                    let Some(&at) = self.hash.get(&key) else {
                        break;
                    };
                    let moved = std::mem::take(&mut self.entries[at].1);
                    self.hash.remove(&key);
                    if matches!(moved, Value::Nil) {
                        break;
                    }
                    self.array.push(moved);
                }
                return;
            }
        }
        let key = Key(k);
        match self.hash.get(&key) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                if matches!(v, Value::Nil) {
                    return;
                }
                self.hash.insert(key.clone(), self.entries.len());
                self.entries.push((key, v));
            }
        }
    }

    pub fn set_str(&mut self, k: &str, v: Value) {
        self.set(Value::str(k), v);
    }

    /// The border `#t` gives.
    pub fn len(&self) -> usize {
        self.array.len()
    }

    pub fn is_empty(&self) -> bool {
        self.array.is_empty() && self.entries.iter().all(|(_, v)| matches!(v, Value::Nil))
    }

    /// The key after `k` (nil starts), skipping removed entries.
    pub fn next(&self, k: &Value) -> Option<(Value, Value)> {
        let mut start_hash = 0;
        match k {
            Value::Nil => {
                if let Some((i, v)) = self
                    .array
                    .iter()
                    .enumerate()
                    .find(|(_, v)| !matches!(v, Value::Nil))
                {
                    return Some((Value::Num((i + 1) as f32), v.clone()));
                }
            }
            _ => {
                if let Some(i) = Self::array_index(k)
                    && i < self.array.len()
                {
                    if let Some((j, v)) = self
                        .array
                        .iter()
                        .enumerate()
                        .skip(i + 1)
                        .find(|(_, v)| !matches!(v, Value::Nil))
                    {
                        return Some((Value::Num((j + 1) as f32), v.clone()));
                    }
                } else {
                    start_hash = self
                        .hash
                        .get(&Key(k.clone()))
                        .map_or(self.entries.len(), |&i| i + 1);
                }
            }
        }
        self.entries
            .iter()
            .skip(start_hash)
            .find(|(_, v)| !matches!(v, Value::Nil))
            .map(|(k, v)| (k.0.clone(), v.clone()))
    }
}
