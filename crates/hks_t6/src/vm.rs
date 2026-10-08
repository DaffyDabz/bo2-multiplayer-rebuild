//! The interpreter: Lua 5.1 semantics on HavokScript's instruction set.
//!
//! Operands (see the crate notes): A is a register; B is a register (8
//! bits) or, in the `_BK` forms, a constant; C is RK (a register, or
//! 256 + a constant). GETFIELD, GETFIELD_R1 and GETTABLE_S carry two cache
//! words (DATA) after them; CLOSURE carries one DATA word per upvalue
//! (A = 1: the enclosing function's register C; A = 0: its upvalue C).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{Closure, Native, Table, TableRef, UpVal, Value, format_number};
use crate::{Const, Proto, decode};

#[derive(Clone, Debug)]
pub struct LuaError {
    pub msg: String,
    /// The value `error` was called with (a string for the VM's own).
    pub value: Value,
    /// Where it happened: "<function hash>:<pc>" innermost first.
    pub trace: Vec<String>,
}

impl LuaError {
    pub fn new(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        LuaError {
            value: Value::str(&msg),
            msg,
            trace: Vec::new(),
        }
    }
}

impl std::fmt::Display for LuaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)?;
        if !self.trace.is_empty() {
            write!(f, " [at {}]", self.trace.join(" < "))?;
        }
        Ok(())
    }
}

type Res<T> = Result<T, LuaError>;

/// Fields per SETLIST flush (Lua 5.1's LFIELDS_PER_FLUSH).
const FIELDS_PER_FLUSH: usize = 50;
const MAX_DEPTH: usize = 220;

pub struct Vm {
    pub globals: TableRef,
    pub stack: Vec<Value>,
    open: Vec<(usize, Rc<RefCell<UpVal>>)>,
    /// Constants as values, per function (the function kept alive with
    /// them: its address is the key).
    consts: HashMap<*const Proto, (Rc<Proto>, Rc<[Value]>)>,
    pub string_meta: Option<TableRef>,
    depth: usize,
    /// Instructions run (a guard against runaway loops in tests).
    pub steps: u64,
    pub step_limit: u64,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        Vm {
            globals: Table::new_ref(),
            stack: Vec::with_capacity(1024),
            open: Vec::new(),
            consts: HashMap::new(),
            string_meta: None,
            depth: 0,
            steps: 0,
            step_limit: u64::MAX,
        }
    }

    pub fn native(
        &self,
        name: &'static str,
        f: impl Fn(&mut Vm, Vec<Value>) -> Res<Vec<Value>> + 'static,
    ) -> Value {
        Value::Native(Rc::new(Native {
            name,
            f: Box::new(f),
        }))
    }

    pub fn set_global(&mut self, name: &str, v: Value) {
        self.globals.borrow_mut().set_str(name, v);
    }

    pub fn global(&self, name: &str) -> Value {
        self.globals.borrow().get_str(name)
    }

    /// A function for a script's main chunk.
    pub fn load(&mut self, main: Proto) -> Value {
        Value::Func(Rc::new(Closure {
            proto: Rc::new(main),
            upvals: Vec::new(),
        }))
    }

    fn konst(&mut self, p: &Rc<Proto>) -> Rc<[Value]> {
        let key = Rc::as_ptr(p);
        if let Some((_, k)) = self.consts.get(&key) {
            return k.clone();
        }
        let k: Rc<[Value]> = p
            .consts
            .iter()
            .map(|c| match c {
                Const::Nil => Value::Nil,
                Const::Bool(b) => Value::Bool(*b),
                Const::Number(n) => Value::Num(*n),
                Const::String(s) => Value::Str(Rc::from(String::from_utf8_lossy(s).as_ref())),
            })
            .collect();
        self.consts.insert(key, (p.clone(), k.clone()));
        k
    }

    // ---- metatables -----------------------------------------------------

    pub fn metatable(&self, v: &Value) -> Option<TableRef> {
        match v {
            Value::Table(t) => t.borrow().meta.clone(),
            Value::Str(_) => self.string_meta.clone(),
            Value::User(u) => u.meta.borrow().clone(),
            _ => None,
        }
    }

    fn metamethod(&self, v: &Value, event: &str) -> Value {
        self.metatable(v)
            .map_or(Value::Nil, |m| m.borrow().get_str(event))
    }

    /// `t[k]` with `__index`.
    pub fn index(&mut self, t: &Value, k: &Value) -> Res<Value> {
        let mut t = t.clone();
        for _ in 0..100 {
            let handler = match &t {
                Value::Table(tab) => {
                    let v = tab.borrow().get(k);
                    if !matches!(v, Value::Nil) {
                        return Ok(v);
                    }
                    let h = self.metamethod(&t, "__index");
                    if matches!(h, Value::Nil) {
                        return Ok(Value::Nil);
                    }
                    h
                }
                _ => {
                    let h = self.metamethod(&t, "__index");
                    if matches!(h, Value::Nil) {
                        return Err(LuaError::new(format!(
                            "attempt to index a {} value (key {k:?})",
                            t.type_name()
                        )));
                    }
                    h
                }
            };
            match handler {
                Value::Func(_) | Value::Native(_) => {
                    let r = self.call(handler, vec![t.clone(), k.clone()])?;
                    return Ok(r.into_iter().next().unwrap_or(Value::Nil));
                }
                other => t = other,
            }
        }
        Err(LuaError::new("'__index' chain too long"))
    }

    /// `t[k] = v` with `__newindex`.
    pub fn set_index(&mut self, t: &Value, k: Value, v: Value) -> Res<()> {
        let mut t = t.clone();
        for _ in 0..100 {
            let handler = match &t {
                Value::Table(tab) => {
                    let present = !matches!(tab.borrow().get(&k), Value::Nil);
                    let h = if present {
                        Value::Nil
                    } else {
                        self.metamethod(&t, "__newindex")
                    };
                    if matches!(h, Value::Nil) {
                        if matches!(k, Value::Nil) {
                            return Err(LuaError::new("table index is nil"));
                        }
                        tab.borrow_mut().set(k, v);
                        return Ok(());
                    }
                    h
                }
                _ => {
                    let h = self.metamethod(&t, "__newindex");
                    if matches!(h, Value::Nil) {
                        return Err(LuaError::new(format!(
                            "attempt to index a {} value (setting {k:?})",
                            t.type_name()
                        )));
                    }
                    h
                }
            };
            match handler {
                Value::Func(_) | Value::Native(_) => {
                    self.call(handler, vec![t.clone(), k, v])?;
                    return Ok(());
                }
                other => t = other,
            }
        }
        Err(LuaError::new("'__newindex' chain too long"))
    }

    fn arith(&mut self, op: u8, a: &Value, b: &Value) -> Res<Value> {
        if let (Some(x), Some(y)) = (a.as_num(), b.as_num()) {
            return Ok(Value::Num(match op {
                b'+' => x + y,
                b'-' => x - y,
                b'*' => x * y,
                b'/' => x / y,
                b'%' => x - (x / y).floor() * y,
                b'^' => x.powf(y),
                _ => -x,
            }));
        }
        let event = match op {
            b'+' => "__add",
            b'-' => "__sub",
            b'*' => "__mul",
            b'/' => "__div",
            b'%' => "__mod",
            b'^' => "__pow",
            _ => "__unm",
        };
        let mut h = self.metamethod(a, event);
        if matches!(h, Value::Nil) {
            h = self.metamethod(b, event);
        }
        if matches!(h, Value::Nil) {
            let bad = if a.as_num().is_none() { a } else { b };
            return Err(LuaError::new(format!(
                "attempt to perform arithmetic on a {} value",
                bad.type_name()
            )));
        }
        Ok(self
            .call(h, vec![a.clone(), b.clone()])?
            .into_iter()
            .next()
            .unwrap_or(Value::Nil))
    }

    pub fn equals(&mut self, a: &Value, b: &Value) -> Res<bool> {
        if a.raw_eq(b) {
            return Ok(true);
        }
        let same_kind = matches!(
            (a, b),
            (Value::Table(_), Value::Table(_)) | (Value::User(_), Value::User(_))
        );
        if !same_kind {
            return Ok(false);
        }
        let h = self.metamethod(a, "__eq");
        if matches!(h, Value::Nil) || !h.raw_eq(&self.metamethod(b, "__eq")) {
            return Ok(false);
        }
        Ok(self
            .call(h, vec![a.clone(), b.clone()])?
            .first()
            .is_some_and(Value::truthy))
    }

    fn less(&mut self, a: &Value, b: &Value, or_equal: bool) -> Res<bool> {
        match (a, b) {
            (Value::Num(x), Value::Num(y)) => Ok(if or_equal { x <= y } else { x < y }),
            (Value::Str(x), Value::Str(y)) => Ok(if or_equal { x <= y } else { x < y }),
            _ => {
                let event = if or_equal { "__le" } else { "__lt" };
                let h = self.metamethod(a, event);
                if matches!(h, Value::Nil) {
                    return Err(LuaError::new(format!(
                        "attempt to compare {} with {}",
                        a.type_name(),
                        b.type_name()
                    )));
                }
                Ok(self
                    .call(h, vec![a.clone(), b.clone()])?
                    .first()
                    .is_some_and(Value::truthy))
            }
        }
    }

    pub fn tostring(&mut self, v: &Value) -> Res<String> {
        let h = self.metamethod(v, "__tostring");
        if !matches!(h, Value::Nil) {
            let r = self.call(h, vec![v.clone()])?;
            return Ok(r.first().map(|v| v.to_string()).unwrap_or_else(String::new));
        }
        Ok(v.to_string())
    }

    fn concat(&mut self, a: &Value, b: &Value) -> Res<Value> {
        let piece = |v: &Value| match v {
            Value::Str(s) => Some(s.to_string()),
            Value::Num(n) => Some(format_number(*n)),
            _ => None,
        };
        if let (Some(x), Some(y)) = (piece(a), piece(b)) {
            return Ok(Value::str(&(x + &y)));
        }
        let mut h = self.metamethod(a, "__concat");
        if matches!(h, Value::Nil) {
            h = self.metamethod(b, "__concat");
        }
        if matches!(h, Value::Nil) {
            let bad = if piece(a).is_none() { a } else { b };
            return Err(LuaError::new(format!(
                "attempt to concatenate a {} value",
                bad.type_name()
            )));
        }
        Ok(self
            .call(h, vec![a.clone(), b.clone()])?
            .into_iter()
            .next()
            .unwrap_or(Value::Nil))
    }

    fn length(&mut self, v: &Value) -> Res<Value> {
        match v {
            Value::Str(s) => Ok(Value::Num(s.len() as f32)),
            Value::Table(t) => Ok(Value::Num(t.borrow().len() as f32)),
            _ => {
                let h = self.metamethod(v, "__len");
                if matches!(h, Value::Nil) {
                    return Err(LuaError::new(format!(
                        "attempt to get length of a {} value",
                        v.type_name()
                    )));
                }
                Ok(self
                    .call(h, vec![v.clone()])?
                    .into_iter()
                    .next()
                    .unwrap_or(Value::Nil))
            }
        }
    }

    // ---- calls -------------------------------------------------------------

    /// Call any callable with arguments; all results.
    pub fn call(&mut self, f: Value, args: Vec<Value>) -> Res<Vec<Value>> {
        match f {
            Value::Native(n) => (n.f)(self, args),
            Value::Func(c) => self.run(c, args),
            other => {
                let h = self.metamethod(&other, "__call");
                if matches!(h, Value::Nil) {
                    return Err(LuaError::new(format!(
                        "attempt to call a {} value",
                        other.type_name()
                    )));
                }
                let mut a = Vec::with_capacity(args.len() + 1);
                a.push(other);
                a.extend(args);
                self.call(h, a)
            }
        }
    }

    fn close_upvals(&mut self, from: usize) {
        while let Some((at, uv)) = self.open.last() {
            if *at < from {
                break;
            }
            let v = self.stack[*at].clone();
            *uv.borrow_mut() = UpVal::Closed(v);
            self.open.pop();
        }
    }

    fn find_upval(&mut self, at: usize) -> Rc<RefCell<UpVal>> {
        if let Some((_, uv)) = self.open.iter().rev().find(|(i, _)| *i == at) {
            return uv.clone();
        }
        let uv = Rc::new(RefCell::new(UpVal::Open(at)));
        // Keep sorted by stack slot.
        let pos = self.open.partition_point(|(i, _)| *i < at);
        self.open.insert(pos, (at, uv.clone()));
        uv
    }

    fn run(&mut self, closure: Rc<Closure>, args: Vec<Value>) -> Res<Vec<Value>> {
        if self.depth >= MAX_DEPTH {
            return Err(LuaError::new("stack overflow"));
        }
        self.depth += 1;
        let base = self.stack.len();
        let r = self.execute(closure, args, base);
        self.close_upvals(base);
        self.stack.truncate(base);
        self.depth -= 1;
        r
    }

    #[allow(clippy::too_many_lines)]
    fn execute(
        &mut self,
        mut cl: Rc<Closure>,
        mut args: Vec<Value>,
        base: usize,
    ) -> Res<Vec<Value>> {
        'call: loop {
            let p = cl.proto.clone();
            // T6LUA_CALLS=<hash>/<index>,...: say when those functions run
            // (debugging aid).
            if let Some(want) = traced_functions()
                && (want.contains(&(p.name_hash, p.index)) || want.contains(&(0, 0)))
            {
                let a: Vec<String> = args
                    .iter()
                    .take(3)
                    .map(|v| match crate::lui::element(v) {
                        Some(u) => format!("#{}", crate::lui::element_id(&u)),
                        None => format!("{v:?}"),
                    })
                    .collect();
                println!("  enter {:08x}/{} ({})", p.name_hash, p.index, a.join(", "));
            }
            let k = self.konst(&p);
            let nparams = p.params as usize;
            let varargs: Vec<Value> = if args.len() > nparams {
                args.split_off(nparams)
            } else {
                Vec::new()
            };
            self.stack.truncate(base);
            self.stack
                .resize(base + (p.stack as usize).max(nparams) + 1, Value::Nil);
            for (i, a) in args.drain(..).enumerate() {
                self.stack[base + i] = a;
            }
            let mut pc = 0usize;
            let mut top = base;
            macro_rules! r {
                ($i:expr) => {
                    self.stack[base + $i as usize]
                };
            }
            macro_rules! rk {
                ($i:expr) => {{
                    let i = $i as usize;
                    if i >= 256 {
                        kk!(i - 256)
                    } else {
                        self.stack[base + i].clone()
                    }
                }};
            }
            macro_rules! fail {
                ($e:expr) => {{
                    let mut e: LuaError = $e;
                    if e.trace.is_empty()
                        && e.msg.starts_with("attempt to call")
                        && let Some(name) = callee_name(&p, &k, pc.saturating_sub(1))
                    {
                        e.msg = format!("{} ({name})", e.msg);
                    }
                    e.trace
                        .push(format!("{:08x}/{}:{}", p.name_hash, p.index, pc.saturating_sub(1)));
                    return Err(e);
                }};
            }
            macro_rules! kk {
                ($i:expr) => {{
                    let i = $i as usize;
                    match k.get(i) {
                        Some(v) => v.clone(),
                        None => fail!(LuaError::new(format!(
                            "constant {i} of {} at pc {}",
                            k.len(),
                            pc - 1
                        ))),
                    }
                }};
            }
            macro_rules! tryv {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(e) => fail!(e),
                    }
                };
            }
            loop {
                let Some(&word) = p.code.get(pc) else {
                    return Ok(Vec::new());
                };
                pc += 1;
                self.steps += 1;
                if self.steps > self.step_limit {
                    fail!(LuaError::new("step limit"));
                }
                let d = decode(word);
                let (a, b, c) = (d.a as usize, d.b as usize, d.c as usize);
                match d.op {
                    // GETFIELD, GETFIELD_R1: R(A) := R(B)[K(C)], C a plain constant index (+ 2 cache words)
                    0 | 73 => {
                        let t = r!(b).clone();
                        let key = kk!(c);
                        let v = tryv!(self.index(&t, &key));
                        r!(a) = v;
                        pc += 2;
                    }
                    // TEST, TEST_R1: if not (R(A) <=> C) then skip
                    1 | 71 => {
                        if r!(a).truthy() != (c != 0) {
                            pc += 1;
                        }
                    }
                    // CALL_I, CALL_C, CALL_M, CALL, CALL_I_R1
                    2 | 3 | 29 | 30 | 69 => {
                        let f = r!(a).clone();
                        let nargs = if b == 0 { top - (base + a + 1) } else { b - 1 };
                        let argv: Vec<Value> =
                            self.stack[base + a + 1..base + a + 1 + nargs].to_vec();
                        let res = tryv!(self.call(f, argv));
                        if c == 0 {
                            let end = base + a + res.len();
                            if self.stack.len() < end + 1 {
                                self.stack.resize(end + 1, Value::Nil);
                            }
                            for (i, v) in res.into_iter().enumerate() {
                                self.stack[base + a + i] = v;
                            }
                            top = end;
                        } else {
                            let want = c - 1;
                            let mut it = res.into_iter();
                            for i in 0..want {
                                r!(a + i) = it.next().unwrap_or(Value::Nil);
                            }
                        }
                    }
                    // EQ, EQ_BK: if ((B == RK(C)) ~= A) then skip
                    4 | 5 => {
                        let x = if d.op == 5 { kk!(b) } else { r!(b).clone() };
                        let y = rk!(c);
                        if tryv!(self.equals(&x, &y)) != (a != 0) {
                            pc += 1;
                        }
                    }
                    // GETGLOBAL
                    6 => {
                        let key = kk!(d.bx as usize);
                        let g = Value::Table(self.globals.clone());
                        let v = tryv!(self.index(&g, &key));
                        r!(a) = v;
                    }
                    // MOVE
                    7 => {
                        let v = r!(b).clone();
                        r!(a) = v;
                    }
                    // SELF: R(A+1) := R(B); R(A) := R(B)[RK(C)]
                    8 => {
                        let obj = r!(b).clone();
                        let key = rk!(c);
                        let m = tryv!(self.index(&obj, &key));
                        r!(a + 1) = obj;
                        r!(a) = m;
                    }
                    // RETURN
                    9 => {
                        let n = if b == 0 { top - (base + a) } else { b - 1 };
                        return Ok(self.stack[base + a..base + a + n].to_vec());
                    }
                    // GETTABLE_S, GETTABLE_N, GETTABLE: R(A) := R(B)[RK(C)]
                    10..=12 => {
                        let t = r!(b).clone();
                        let key = rk!(c);
                        let v = tryv!(self.index(&t, &key));
                        r!(a) = v;
                        if d.op == 10 {
                            pc += 2;
                        }
                    }
                    // LOADBOOL
                    13 => {
                        r!(a) = Value::Bool(b != 0);
                        if c != 0 {
                            pc += 1;
                        }
                    }
                    // TFORLOOP: R(A+3..A+2+C) := R(A)(R(A+1), R(A+2))
                    14 => {
                        let f = r!(a).clone();
                        let argv = vec![r!(a + 1).clone(), r!(a + 2).clone()];
                        let res = tryv!(self.call(f, argv));
                        let mut it = res.into_iter();
                        for i in 0..c {
                            r!(a + 3 + i) = it.next().unwrap_or(Value::Nil);
                        }
                        if matches!(r!(a + 3), Value::Nil) {
                            pc += 1;
                        } else {
                            let v = r!(a + 3).clone();
                            r!(a + 2) = v;
                        }
                    }
                    // SETFIELD, SETFIELD_R1: R(A)[K(B)] := RK(C)
                    15 | 74 => {
                        let t = r!(a).clone();
                        let key = kk!(b);
                        let v = rk!(c);
                        tryv!(self.set_index(&t, key, v));
                    }
                    // SETTABLE_S, _S_BK, _N, _N_BK, SETTABLE, _BK: R(A)[B] := RK(C)
                    16..=21 => {
                        let t = r!(a).clone();
                        let key = if matches!(d.op, 17 | 19 | 21) {
                            kk!(b)
                        } else {
                            r!(b).clone()
                        };
                        let v = rk!(c);
                        tryv!(self.set_index(&t, key, v));
                    }
                    // TAILCALL_I, _C, _M, TAILCALL, TAILCALL_I_R1
                    22..=24 | 37 | 68 => {
                        let f = r!(a).clone();
                        let nargs = if b == 0 { top - (base + a + 1) } else { b - 1 };
                        let argv: Vec<Value> =
                            self.stack[base + a + 1..base + a + 1 + nargs].to_vec();
                        self.close_upvals(base);
                        match f {
                            Value::Func(next) => {
                                cl = next;
                                args = argv;
                                continue 'call;
                            }
                            other => {
                                let res = tryv!(self.call(other, argv));
                                return Ok(res);
                            }
                        }
                    }
                    // LOADK
                    25 => {
                        r!(a) = kk!(d.bx as usize);
                    }
                    // LOADNIL: R(A..B) := nil
                    26 => {
                        for i in a..=b {
                            r!(i) = Value::Nil;
                        }
                    }
                    // SETGLOBAL
                    27 => {
                        let key = kk!(d.bx as usize);
                        let v = r!(a).clone();
                        let g = Value::Table(self.globals.clone());
                        tryv!(self.set_index(&g, key, v));
                    }
                    // JMP
                    28 => {
                        pc = (pc as i64 + i64::from(d.sbx)) as usize;
                    }
                    // GETUPVAL
                    38 => {
                        let v = match &*cl.upvals[b].borrow() {
                            UpVal::Open(i) => self.stack[*i].clone(),
                            UpVal::Closed(v) => v.clone(),
                        };
                        r!(a) = v;
                    }
                    // SETUPVAL, SETUPVAL_R1
                    39 | 70 => {
                        let v = r!(a).clone();
                        let uv = cl.upvals[b].clone();
                        let mut uv = uv.borrow_mut();
                        match &mut *uv {
                            UpVal::Open(i) => self.stack[*i] = v,
                            UpVal::Closed(slot) => *slot = v,
                        }
                    }
                    // ADD .. POW_BK: R(A) := B op RK(C)
                    40..=51 => {
                        let op = [b'+', b'-', b'*', b'/', b'%', b'^'][((d.op - 40) / 2) as usize];
                        let x = if d.op & 1 == 1 { kk!(b) } else { r!(b).clone() };
                        let y = rk!(c);
                        let v = tryv!(self.arith(op, &x, &y));
                        r!(a) = v;
                    }
                    // NEWTABLE
                    52 => {
                        r!(a) = Value::Table(Table::new_ref());
                    }
                    // UNM
                    53 => {
                        let x = r!(b).clone();
                        let v = tryv!(self.arith(b'u', &x, &x));
                        r!(a) = v;
                    }
                    // NOT, NOT_R1
                    54 | 72 => {
                        let v = !r!(b).truthy();
                        r!(a) = Value::Bool(v);
                    }
                    // LEN
                    55 => {
                        let x = r!(b).clone();
                        let v = tryv!(self.length(&x));
                        r!(a) = v;
                    }
                    // LT, LT_BK, LE, LE_BK: if ((B < RK(C)) ~= A) then skip
                    56..=59 => {
                        let x = if d.op & 1 == 1 { kk!(b) } else { r!(b).clone() };
                        let y = rk!(c);
                        let r = tryv!(self.less(&x, &y, d.op >= 58));
                        if r != (a != 0) {
                            pc += 1;
                        }
                    }
                    // CONCAT: R(A) := R(B) .. ... .. R(C)
                    60 => {
                        let mut acc = r!(c).clone();
                        for i in (b..c).rev() {
                            let left = r!(i).clone();
                            acc = tryv!(self.concat(&left, &acc));
                        }
                        r!(a) = acc;
                    }
                    // TESTSET: if (R(B) <=> C) then R(A) := R(B) else skip
                    61 => {
                        if r!(b).truthy() == (c != 0) {
                            let v = r!(b).clone();
                            r!(a) = v;
                        } else {
                            pc += 1;
                        }
                    }
                    // FORPREP
                    62 => {
                        let (Some(init), Some(step)) = (r!(a).as_num(), r!(a + 2).as_num()) else {
                            fail!(LuaError::new("'for' values must be numbers"));
                        };
                        if r!(a + 1).as_num().is_none() {
                            fail!(LuaError::new("'for' limit must be a number"));
                        }
                        r!(a) = Value::Num(init - step);
                        pc = (pc as i64 + i64::from(d.sbx)) as usize;
                    }
                    // FORLOOP
                    63 => {
                        let step = r!(a + 2).as_num().unwrap_or(1.0);
                        let idx = r!(a).as_num().unwrap_or(0.0) + step;
                        let limit = r!(a + 1).as_num().unwrap_or(0.0);
                        let go = if step > 0.0 {
                            idx <= limit
                        } else {
                            idx >= limit
                        };
                        if go {
                            r!(a) = Value::Num(idx);
                            r!(a + 3) = Value::Num(idx);
                            pc = (pc as i64 + i64::from(d.sbx)) as usize;
                        }
                    }
                    // SETLIST: R(A)[(C-1)*50 + i] := R(A+i), 1 <= i <= B
                    64 => {
                        let n = if b == 0 { top - (base + a) - 1 } else { b };
                        let block = if c == 0 {
                            let w = p.code.get(pc).copied().unwrap_or(0) as usize;
                            pc += 1;
                            w
                        } else {
                            c
                        };
                        let Value::Table(t) = r!(a).clone() else {
                            fail!(LuaError::new("SETLIST on a non-table"));
                        };
                        let mut t = t.borrow_mut();
                        for i in 1..=n {
                            let v = self.stack[base + a + i].clone();
                            t.set(Value::Num(((block - 1) * FIELDS_PER_FLUSH + i) as f32), v);
                        }
                    }
                    // CLOSE
                    65 => {
                        self.close_upvals(base + a);
                    }
                    // CLOSURE (+ one DATA word per upvalue)
                    66 => {
                        let child = p.protos[d.bx as usize].clone();
                        let mut upvals = Vec::with_capacity(child.upvalues as usize);
                        for _ in 0..child.upvalues {
                            let dw = decode(p.code.get(pc).copied().unwrap_or(0));
                            pc += 1;
                            if dw.a == 1 {
                                upvals.push(self.find_upval(base + dw.c as usize));
                            } else {
                                upvals.push(cl.upvals[dw.c as usize].clone());
                            }
                        }
                        r!(a) = Value::Func(Rc::new(Closure {
                            proto: child,
                            upvals,
                        }));
                    }
                    // VARARG: R(A), R(A+1), ..., R(A+B-1) = vararg
                    67 => {
                        if b == 0 {
                            let end = base + a + varargs.len();
                            if self.stack.len() < end + 1 {
                                self.stack.resize(end + 1, Value::Nil);
                            }
                            for (i, v) in varargs.iter().enumerate() {
                                self.stack[base + a + i] = v.clone();
                            }
                            top = end;
                        } else {
                            for i in 0..b - 1 {
                                r!(a + i) = varargs.get(i).cloned().unwrap_or(Value::Nil);
                            }
                        }
                    }
                    // DATA words are skipped by their owners; one met here is a no-op.
                    76 => {}
                    op => fail!(LuaError::new(format!(
                        "opcode {} ({}) not run",
                        op,
                        crate::op_name(op)
                    ))),
                }
            }
        }
    }
}

/// What the call at `pc` calls, read back from the instruction that loaded
/// its register (`method 'x'`, `field 'x'`, `global 'x'`).
fn callee_name(p: &crate::Proto, k: &[Value], pc: usize) -> Option<String> {
    let call = crate::decode(*p.code.get(pc)?);
    let name = |i: usize| k.get(i).map(ToString::to_string);
    for at in (pc.saturating_sub(16)..pc).rev() {
        let d = crate::decode(p.code[at]);
        if d.a != call.a {
            continue;
        }
        return match crate::op_name(d.op) {
            "SELF" if d.c >= 256 => name(usize::from(d.c - 256)).map(|n| format!("method '{n}'")),
            "GETFIELD" | "GETFIELD_R1" => name(usize::from(d.c)).map(|n| format!("field '{n}'")),
            "GETGLOBAL" => name(d.bx as usize).map(|n| format!("global '{n}'")),
            "DATA" => continue,
            _ => None,
        };
    }
    None
}

/// The functions `T6LUA_CALLS` names (hash/index pairs), read once.
fn traced_functions() -> Option<&'static Vec<(u32, u32)>> {
    static WANT: std::sync::OnceLock<Option<Vec<(u32, u32)>>> = std::sync::OnceLock::new();
    WANT.get_or_init(|| {
        let v = std::env::var("T6LUA_CALLS").ok()?;
        // `all`: every function.
        if v == "all" {
            return Some(vec![(0, 0)]);
        }
        Some(
            v.split(',')
                .filter_map(|f| {
                    let (h, i) = f.split_once('/')?;
                    Some((u32::from_str_radix(h.trim_start_matches("0x"), 16).ok()?, i.parse().ok()?))
                })
                .collect(),
        )
    })
    .as_ref()
}
