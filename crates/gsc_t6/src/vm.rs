//! The virtual machine: threads, the scheduler and the interpreter.
//!
//! Semantics (Black Ops II as its compiled code shows them, measured on the
//! PC zones 2026-10-01, plus the IW-family VM conventions noted here):
//! - Arguments are pushed last first after a marker (`PreScriptCall`, or
//!   `VoidCodePos` for a notify); the callee's prologue pops them into its
//!   locals, parameters first. Locals are addressed from the newest
//!   (`EvalLocalVariableCached 0` is the last declared).
//! - `wait` rounds to 50 ms server frames (at least one); `waittillframeend`
//!   runs after every other thread this frame.
//! - `notify` runs the threads it wakes at once, before the notifier goes on
//!   (endons first). A fired endon unwinds its thread to the function that
//!   registered it; the caller sees that call return undefined.
//! - Deleting an entity ends the threads that run on it or wait on it.
//! - Runtime errors are reported once per place and the operation yields
//!   undefined; the thread goes on.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::program::{BinOp, CallKind, Op, Program};
use crate::value::{
    Array, ArrayRef, FuncRef, Key, ObjKind, ObjRef, Object, Str, Strings, Value, format_float,
};

/// A builtin: `(vm, host, self, args) -> result`.
pub type Native<H> = fn(&mut Vm<H>, &mut H, &Value, &[Value]) -> Result<Value, String>;

/// Engine hooks: an engine-owned field of a host object (`Entity`,
/// `HudElem`; `None` = use the script field), writing one (`true` when
/// handled), and a builtin nobody bound.
pub type GetFieldHook<H> = fn(&mut Vm<H>, &mut H, ObjRef, ObjKind, Str) -> Option<Value>;
pub type SetFieldHook<H> = fn(&mut Vm<H>, &mut H, ObjRef, ObjKind, Str, &Value) -> bool;
pub type UnboundHook<H> = fn(&mut Vm<H>, &mut H, u32, &Value, &[Value]) -> Value;

pub struct Hooks<H> {
    pub get_field: Option<GetFieldHook<H>>,
    pub set_field: Option<SetFieldHook<H>>,
    pub unbound: Option<UnboundHook<H>>,
}

impl<H> Default for Hooks<H> {
    fn default() -> Self {
        Self {
            get_field: None,
            set_field: None,
            unbound: None,
        }
    }
}

/// The hash Black Ops II scripts use for `#"name"` (dvar names, ...):
/// djb2 over the lower-cased name (measured: `ui_zm_mapstartlocation` ->
/// 0xc955b4cd, `ui_gametype` -> 0x0041651e, as the compiled scripts carry).
pub fn hash_name(name: &str) -> u32 {
    let mut h: u32 = 5381;
    for c in name.bytes() {
        h = h
            .wrapping_mul(33)
            .wrapping_add(u32::from(c.to_ascii_lowercase()));
    }
    h
}

/// Script truth: numbers non-zero; undefined false; other values true.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Undefined | Value::Marker => false,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThreadId {
    pub index: u32,
    pub generation: u32,
}

#[derive(Debug)]
struct Frame {
    func: u32,
    pc: u32,
    locals: Vec<Value>,
    self_: Value,
    base: u32,
    args: Vec<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ready,
    Timer,
    Notify,
    FrameEnd,
    Running,
}

#[derive(Debug)]
struct Thread {
    frames: Vec<Frame>,
    stack: Vec<Value>,
    /// (object, name, frame depth).
    endons: Vec<(ObjRef, Str, u32)>,
    waiting_on: Option<(ObjRef, Str)>,
    timer_seq: u64,
    state: State,
}

#[derive(Clone, Debug)]
enum WaiterKind {
    Waittill,
    Match(Vec<Value>),
    Endon(u32),
}

#[derive(Clone, Debug)]
struct Waiter {
    thread: ThreadId,
    obj_gen: u32,
    kind: WaiterKind,
}

#[derive(Clone, Debug, Default)]
enum RefBase {
    #[default]
    None,
    Local(u8),
    Field(ObjRef, Str),
    Game,
}

#[derive(Clone, Debug, Default)]
struct Ref {
    base: RefBase,
    path: Vec<Key>,
}

enum Outcome {
    Yield,
    Done,
}

const MAX_FRAMES: usize = 256;
const MAX_NEST: u32 = 48;
const STEP_LIMIT: u64 = 5_000_000;
pub const FRAME_MS: i64 = 50;

pub struct Vm<H> {
    pub strings: Strings,
    pub program: Arc<Program>,
    natives: Vec<Option<Native<H>>>,
    pub hooks: Hooks<H>,
    objects: Vec<Object>,
    free_objects: Vec<u32>,
    pub level: ObjRef,
    pub anim: ObjRef,
    pub game: Value,
    threads: Vec<Option<Box<Thread>>>,
    thread_gens: Vec<u32>,
    free_threads: Vec<u32>,
    timers: BinaryHeap<Reverse<(i64, u64, u32, u32)>>,
    next_seq: u64,
    frame_end: Vec<ThreadId>,
    ready: VecDeque<ThreadId>,
    waiters: FxHashMap<(u32, Str), Vec<Waiter>>,
    running: Vec<ThreadId>,
    pending_unwind: FxHashMap<ThreadId, u32>,
    nest: u32,
    pub time_ms: i64,
    pub dvars: FxHashMap<String, String>,
    reported: FxHashSet<String>,
    /// Messages for the host to show (errors, unbound builtins).
    pub messages: Vec<String>,
    pub steps: u64,
    pub rng: u64,
    /// Builtins whose calls are logged (debugging): id -> calls left.
    pub trace: FxHashMap<u32, u32>,
    /// Script functions whose calls are logged (`trace_functions`).
    pub ftrace: FxHashSet<u32>,
}

impl<H> Vm<H> {
    pub fn new(program: Program, strings: Strings) -> Self {
        let n = program.builtins.len();
        let mut vm = Self {
            strings,
            program: Arc::new(program),
            natives: vec![None; n],
            hooks: Hooks::default(),
            objects: Vec::new(),
            free_objects: Vec::new(),
            level: ObjRef {
                index: 0,
                generation: 0,
            },
            anim: ObjRef {
                index: 0,
                generation: 0,
            },
            game: Value::array(Array::new()),
            threads: Vec::new(),
            thread_gens: Vec::new(),
            free_threads: Vec::new(),
            timers: BinaryHeap::new(),
            next_seq: 0,
            frame_end: Vec::new(),
            ready: VecDeque::new(),
            waiters: FxHashMap::default(),
            running: Vec::new(),
            pending_unwind: FxHashMap::default(),
            nest: 0,
            time_ms: 0,
            dvars: FxHashMap::default(),
            reported: FxHashSet::default(),
            messages: Vec::new(),
            steps: 0,
            rng: 0x2545_f491_4f6c_dd1d,
            trace: FxHashMap::default(),
            ftrace: FxHashSet::default(),
        };
        vm.level = vm.alloc_object(ObjKind::Level);
        vm.anim = vm.alloc_object(ObjKind::Anim);
        vm
    }

    // ---- builtins -------------------------------------------------------

    /// Bind a builtin by name (`method` for `self name(...)` calls). Returns
    /// whether the program uses it.
    pub fn bind(&mut self, name: &str, method: bool, native: Native<H>) -> bool {
        let Some(s) = self.strings.find(name) else {
            return false;
        };
        match self.program.builtin_index.get(&(s, method)) {
            Some(&id) => {
                self.natives[id as usize] = Some(native);
                true
            }
            None => false,
        }
    }

    /// Builtins the program calls that nothing bound: (name, method).
    pub fn unbound_builtins(&self) -> Vec<(String, bool)> {
        self.program
            .builtins
            .iter()
            .enumerate()
            .filter(|(i, _)| self.natives[*i].is_none())
            .map(|(_, (n, m))| (self.strings.get(*n).to_owned(), *m))
            .collect()
    }

    pub fn intern(&mut self, s: &str) -> Str {
        self.strings.intern(s)
    }

    pub fn str(&self, s: Str) -> &str {
        self.strings.get(s)
    }

    pub fn report_once(&mut self, msg: String) {
        if self.reported.insert(msg.clone()) {
            self.messages.push(msg);
        }
    }

    // ---- objects --------------------------------------------------------

    pub fn alloc_object(&mut self, kind: ObjKind) -> ObjRef {
        if let Some(i) = self.free_objects.pop() {
            let o = &mut self.objects[i as usize];
            o.generation = o.generation.wrapping_add(1);
            o.alive = true;
            o.kind = kind;
            o.fields.clear();
            return ObjRef {
                index: i,
                generation: o.generation,
            };
        }
        let i = self.objects.len() as u32;
        self.objects.push(Object {
            generation: 0,
            alive: true,
            kind,
            fields: FxHashMap::default(),
        });
        ObjRef {
            index: i,
            generation: 0,
        }
    }

    pub fn new_struct(&mut self) -> Value {
        Value::Object(self.alloc_object(ObjKind::Struct))
    }

    pub fn alive(&self, o: ObjRef) -> bool {
        self.objects
            .get(o.index as usize)
            .is_some_and(|x| x.alive && x.generation == o.generation)
    }

    pub fn kind(&self, o: ObjRef) -> Option<ObjKind> {
        self.objects
            .get(o.index as usize)
            .filter(|x| x.alive && x.generation == o.generation)
            .map(|x| x.kind)
    }

    /// The script field (not the engine's).
    pub fn raw_field(&self, o: ObjRef, f: Str) -> Value {
        match self.objects.get(o.index as usize) {
            Some(x) if x.alive && x.generation == o.generation => {
                x.fields.get(&f).cloned().unwrap_or_default()
            }
            _ => Value::Undefined,
        }
    }

    pub fn set_raw_field(&mut self, o: ObjRef, f: Str, v: Value) {
        if let Some(x) = self.objects.get_mut(o.index as usize)
            && x.alive
            && x.generation == o.generation
        {
            if v.is_undefined() {
                x.fields.remove(&f);
            } else {
                x.fields.insert(f, v);
            }
        }
    }

    pub fn field_names(&self, o: ObjRef) -> Vec<Str> {
        match self.objects.get(o.index as usize) {
            Some(x) if x.alive && x.generation == o.generation => {
                x.fields.keys().copied().collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn get_field(&mut self, host: &mut H, o: ObjRef, f: Str) -> Value {
        let Some(kind) = self.kind(o) else {
            return Value::Undefined;
        };
        if matches!(kind, ObjKind::Entity(_) | ObjKind::HudElem(_))
            && let Some(hook) = self.hooks.get_field
            && let Some(v) = hook(self, host, o, kind, f)
        {
            return v;
        }
        self.raw_field(o, f)
    }

    pub fn set_field(&mut self, host: &mut H, o: ObjRef, f: Str, v: Value) {
        let Some(kind) = self.kind(o) else {
            self.report_once(format!("field '{}' set on a deleted object", self.str(f)));
            return;
        };
        if matches!(kind, ObjKind::Entity(_) | ObjKind::HudElem(_))
            && let Some(hook) = self.hooks.set_field
            && hook(self, host, o, kind, f, &v)
        {
            return;
        }
        self.set_raw_field(o, f, v);
    }

    /// Free an object (an entity deleted): threads that run on it, wait on
    /// it or end on it end.
    pub fn free_object(&mut self, host: &mut H, o: ObjRef) {
        if !self.alive(o) {
            return;
        }
        // Endons on it unwind; waits on it end. Threads running on it (its
        // `self`) go on, as in Black Ops II: a care package's crate thread
        // runs on the helicopter that dropped it, long after the helicopter
        // is gone (a call on a removed entity is that thread's own error).
        let keys: Vec<(u32, Str)> = self
            .waiters
            .keys()
            .filter(|k| k.0 == o.index)
            .copied()
            .collect();
        let mut kill: Vec<ThreadId> = Vec::new();
        let mut unwind: Vec<(ThreadId, u32)> = Vec::new();
        for k in keys {
            if let Some(list) = self.waiters.remove(&k) {
                for w in list {
                    if w.obj_gen != o.generation {
                        continue;
                    }
                    match w.kind {
                        WaiterKind::Endon(d) => unwind.push((w.thread, d)),
                        _ => kill.push(w.thread),
                    }
                }
            }
        }
        // Running threads on it (this call may come from one of them).
        for &t in &self.running {
            if let Some(Some(_)) = self.threads.get(t.index as usize) {
                continue;
            }
            let _ = t;
        }
        let x = &mut self.objects[o.index as usize];
        x.alive = false;
        x.fields.clear();
        self.free_objects.push(o.index);
        for t in kill {
            self.unwind(host, t, 0);
        }
        for (t, d) in unwind {
            self.unwind(host, t, d);
        }
    }

    // ---- threads --------------------------------------------------------

    fn new_thread(&mut self, func: u32, self_: Value, args: Vec<Value>) -> ThreadId {
        let th = Box::new(Thread {
            frames: vec![Frame {
                func,
                pc: 0,
                locals: Vec::new(),
                self_,
                base: 0,
                args,
            }],
            stack: Vec::with_capacity(16),
            endons: Vec::new(),
            waiting_on: None,
            timer_seq: 0,
            state: State::Ready,
        });
        if let Some(i) = self.free_threads.pop() {
            self.thread_gens[i as usize] = self.thread_gens[i as usize].wrapping_add(1);
            self.threads[i as usize] = Some(th);
            return ThreadId {
                index: i,
                generation: self.thread_gens[i as usize],
            };
        }
        let i = self.threads.len() as u32;
        self.threads.push(Some(th));
        self.thread_gens.push(0);
        ThreadId {
            index: i,
            generation: 0,
        }
    }

    fn thread_live(&self, t: ThreadId) -> bool {
        self.thread_gens.get(t.index as usize) == Some(&t.generation)
            && (self.threads[t.index as usize].is_some() || self.running.contains(&t))
    }

    /// Start `func` as a new thread on `self_` and run it until it yields.
    pub fn spawn(&mut self, host: &mut H, func: u32, self_: Value, args: Vec<Value>) -> ThreadId {
        let t = self.new_thread(func, self_, args);
        self.exec(host, t);
        t
    }

    /// Start a script function by path and name; `None` if it does not exist.
    pub fn spawn_named(
        &mut self,
        host: &mut H,
        script: &str,
        name: &str,
        self_: Value,
        args: Vec<Value>,
    ) -> Option<ThreadId> {
        let f = self.program.find(&self.strings, script, name)?;
        Some(self.spawn(host, f, self_, args))
    }

    /// Number of live threads.
    pub fn thread_count(&self) -> usize {
        self.threads.iter().filter(|t| t.is_some()).count() + self.running.len()
    }

    fn free_thread(&mut self, t: ThreadId, th: &mut Thread) {
        self.cancel_wait(t, th);
        for (o, n, _) in th.endons.drain(..) {
            if let Some(list) = self.waiters.get_mut(&(o.index, n)) {
                list.retain(|w| w.thread != t);
                if list.is_empty() {
                    self.waiters.remove(&(o.index, n));
                }
            }
        }
        self.threads[t.index as usize] = None;
        self.free_threads.push(t.index);
        self.thread_gens[t.index as usize] = self.thread_gens[t.index as usize].wrapping_add(1);
    }

    fn cancel_wait(&mut self, t: ThreadId, th: &mut Thread) {
        if let Some((o, n)) = th.waiting_on.take()
            && let Some(list) = self.waiters.get_mut(&(o.index, n))
        {
            list.retain(|w| !(w.thread == t && !matches!(w.kind, WaiterKind::Endon(_))));
            if list.is_empty() {
                self.waiters.remove(&(o.index, n));
            }
        }
        th.timer_seq = th.timer_seq.wrapping_add(1);
        if th.state == State::FrameEnd {
            self.frame_end.retain(|x| *x != t);
        }
        th.state = State::Ready;
    }

    /// Unwind thread `t` to frame `depth` (0 ends it); the caller of the
    /// unwound frame sees undefined and the thread runs on now.
    fn unwind(&mut self, host: &mut H, t: ThreadId, depth: u32) {
        if self.running.contains(&t) {
            let d = self.pending_unwind.entry(t).or_insert(depth);
            *d = (*d).min(depth);
            return;
        }
        if self.thread_gens.get(t.index as usize) != Some(&t.generation) {
            return;
        }
        let Some(mut th) = self.threads[t.index as usize].take() else {
            return;
        };
        self.cancel_wait(t, &mut th);
        let alive = self.apply_unwind(t, &mut th, depth);
        if !alive {
            self.threads[t.index as usize] = Some(th);
            let mut th = self.threads[t.index as usize].take().unwrap();
            self.free_thread(t, &mut th);
            return;
        }
        th.state = State::Ready;
        self.threads[t.index as usize] = Some(th);
        self.run_or_defer(host, t);
    }

    /// Pop frames down to `depth`; false when nothing is left.
    fn apply_unwind(&mut self, t: ThreadId, th: &mut Thread, depth: u32) -> bool {
        let depth = depth as usize;
        if depth >= th.frames.len() {
            return true;
        }
        let base = th.frames[depth].base as usize;
        th.frames.truncate(depth);
        th.stack.truncate(base);
        let mut removed = Vec::new();
        th.endons.retain(|e| {
            if e.2 as usize >= depth {
                removed.push((e.0, e.1));
                false
            } else {
                true
            }
        });
        for (o, n) in removed {
            if let Some(list) = self.waiters.get_mut(&(o.index, n)) {
                list.retain(|w| {
                    !(w.thread == t
                        && matches!(w.kind, WaiterKind::Endon(d) if d as usize >= depth))
                });
                if list.is_empty() {
                    self.waiters.remove(&(o.index, n));
                }
            }
        }
        if th.frames.is_empty() {
            return false;
        }
        th.stack.push(Value::Undefined);
        true
    }

    fn run_or_defer(&mut self, host: &mut H, t: ThreadId) {
        if self.nest >= MAX_NEST {
            self.ready.push_back(t);
        } else {
            self.exec(host, t);
        }
    }

    /// Notify `name` on `obj` with `args`: endons fire, then waiting threads
    /// run.
    pub fn notify(&mut self, host: &mut H, obj: ObjRef, name: Str, args: &[Value]) {
        let Some(list) = self.waiters.remove(&(obj.index, name)) else {
            return;
        };
        let mut keep = Vec::new();
        let mut endons: Vec<(ThreadId, u32)> = Vec::new();
        let mut wakes: Vec<ThreadId> = Vec::new();
        for w in list {
            if w.obj_gen != obj.generation {
                continue;
            }
            if !self.thread_live(w.thread) {
                continue;
            }
            match &w.kind {
                WaiterKind::Endon(d) => {
                    match endons.iter_mut().find(|e| e.0 == w.thread) {
                        Some(e) => e.1 = e.1.min(*d),
                        None => endons.push((w.thread, *d)),
                    }
                    keep.push(w);
                }
                WaiterKind::Waittill => wakes.push(w.thread),
                WaiterKind::Match(vals) => {
                    let ok = vals
                        .iter()
                        .enumerate()
                        .all(|(i, v)| args.get(i).is_some_and(|a| self.equal(a, v)));
                    if ok {
                        wakes.push(w.thread);
                    } else {
                        keep.push(w);
                    }
                }
            }
        }
        if !keep.is_empty() {
            self.waiters.insert((obj.index, name), keep);
        }
        for (t, d) in endons {
            self.unwind(host, t, d);
        }
        // The newest waiter wakes first, as in the engine: the zombies
        // scripts rely on it (_zm::init_player_levelvars starts waiting for
        // "start_zombie_round_logic" a frame-end after _zm_powerups'
        // watch_for_drop, yet must set the score values it reads first).
        for t in wakes.into_iter().rev() {
            self.wake(host, t, obj, name, args);
        }
    }

    /// Notify by string.
    pub fn notify_str(&mut self, host: &mut H, obj: ObjRef, name: &str, args: &[Value]) {
        let n = self.intern(name);
        self.notify(host, obj, n, args);
    }

    fn wake(&mut self, host: &mut H, t: ThreadId, obj: ObjRef, name: Str, args: &[Value]) {
        if self.thread_gens.get(t.index as usize) != Some(&t.generation) {
            return;
        }
        let Some(th) = self.threads[t.index as usize].as_mut() else {
            return;
        };
        // Still waiting on this?
        if th.state != State::Notify || th.waiting_on != Some((obj, name)) {
            return;
        }
        th.waiting_on = None;
        th.state = State::Ready;
        th.stack.push(Value::Marker);
        for a in args.iter().rev() {
            th.stack.push(a.clone());
        }
        self.run_or_defer(host, t);
    }

    /// Advance one server frame: due timers, then frame-end threads.
    pub fn run_frame(&mut self, host: &mut H) {
        self.time_ms += FRAME_MS;
        self.run_due(host);
    }

    /// Run everything due at the current time.
    pub fn run_due(&mut self, host: &mut H) {
        loop {
            while let Some(t) = self.ready.pop_front() {
                self.exec(host, t);
            }
            let Some(&Reverse((at, seq, index, generation))) = self.timers.peek() else {
                break;
            };
            if at > self.time_ms {
                break;
            }
            self.timers.pop();
            let t = ThreadId { index, generation };
            let ok = self.thread_gens.get(index as usize) == Some(&generation)
                && self.threads[index as usize]
                    .as_ref()
                    .is_some_and(|th| th.state == State::Timer && th.timer_seq == seq);
            if ok {
                if let Some(th) = self.threads[index as usize].as_mut() {
                    th.state = State::Ready;
                }
                self.exec(host, t);
            }
        }
        for _ in 0..8 {
            while let Some(t) = self.ready.pop_front() {
                self.exec(host, t);
            }
            if self.frame_end.is_empty() {
                break;
            }
            let list = std::mem::take(&mut self.frame_end);
            for t in list {
                if let Some(Some(th)) = self.threads.get_mut(t.index as usize)
                    && self.thread_gens[t.index as usize] == t.generation
                    && th.state == State::FrameEnd
                {
                    th.state = State::Ready;
                    self.exec(host, t);
                }
            }
        }
    }

    fn exec(&mut self, host: &mut H, t: ThreadId) {
        if self.thread_gens.get(t.index as usize) != Some(&t.generation) {
            return;
        }
        let Some(mut th) = self.threads[t.index as usize].take() else {
            return;
        };
        if th.state != State::Ready {
            self.threads[t.index as usize] = Some(th);
            return;
        }
        th.state = State::Running;
        self.running.push(t);
        self.nest += 1;
        let program = self.program.clone();
        let outcome = self.interp(host, t, &mut th, &program);
        self.nest -= 1;
        self.running.pop();
        match outcome {
            Outcome::Done => {
                self.threads[t.index as usize] = Some(th);
                let mut th = self.threads[t.index as usize].take().unwrap();
                self.free_thread(t, &mut th);
            }
            Outcome::Yield => {
                self.threads[t.index as usize] = Some(th);
            }
        }
    }

    fn fail(&mut self, program: &Program, th: &Thread, msg: &str) {
        let (f, pc) = th.frames.last().map_or((0, 0), |f| (f.func, f.pc));
        let func = &program.functions[f as usize];
        let script = &program.scripts[func.script as usize].name;
        let at = func
            .addrs
            .get(pc.saturating_sub(1) as usize)
            .copied()
            .unwrap_or(0);
        let m = format!("{script}::{} @{at:#x}: {msg}", self.strings.get(func.name));
        self.report_once(m);
    }

    // ---- values ---------------------------------------------------------

    pub fn equal(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Undefined, Value::Undefined) => true,
            (Value::Int(x), Value::Int(y)) => x == y,
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                a.as_float() == b.as_float()
            }
            (Value::Str(x) | Value::IStr(x), Value::Str(y) | Value::IStr(y)) => x == y,
            (Value::Hash(x), Value::Hash(y)) => x == y,
            (Value::Vec3(x), Value::Vec3(y)) => x == y,
            (Value::Object(x), Value::Object(y)) => x == y && self.alive(*x),
            (Value::Object(x), Value::Undefined) | (Value::Undefined, Value::Object(x)) => {
                !self.alive(*x)
            }
            (Value::Array(x), Value::Array(y)) => ArrayRef::ptr_eq(x, y),
            (Value::Func(x), Value::Func(y)) => x == y,
            (Value::Anim(x), Value::Anim(y)) | (Value::AnimTree(x), Value::AnimTree(y)) => x == y,
            _ => false,
        }
    }

    /// A value made fit to read: dead objects read as undefined.
    fn live(&self, v: Value) -> Value {
        match v {
            Value::Object(o) if !self.alive(o) => Value::Undefined,
            v => v,
        }
    }

    pub fn is_defined(&self, v: &Value) -> bool {
        match v {
            Value::Undefined | Value::Marker => false,
            Value::Object(o) => self.alive(*o),
            _ => true,
        }
    }

    pub fn to_key(&self, v: &Value) -> Result<Key, String> {
        match v {
            Value::Int(i) => Ok(Key::Int(*i)),
            Value::Float(f) => Ok(Key::Int(*f as i32)),
            Value::Str(s) | Value::IStr(s) => Ok(Key::Str(*s)),
            Value::Object(o) => Ok(Key::Obj(*o)),
            Value::Hash(h) => Ok(Key::Int(*h as i32)),
            other => Err(format!("{} is not an array index", other.type_name())),
        }
    }

    pub fn to_text(&self, v: &Value) -> String {
        match v {
            Value::Undefined => "undefined".into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Str(s) | Value::IStr(s) => self.strings.get(*s).to_owned(),
            Value::Vec3(v) => format!(
                "({}, {}, {})",
                format_float(v[0]),
                format_float(v[1]),
                format_float(v[2])
            ),
            Value::Hash(h) => format!("#{h:08x}"),
            Value::Object(_) => "[object]".into(),
            Value::Array(_) => "[array]".into(),
            Value::Func(_) => "[function]".into(),
            Value::Anim(_) | Value::AnimTree(_) => "[anim]".into(),
            Value::Marker => "[marker]".into(),
        }
    }

    pub fn string(&mut self, s: &str) -> Value {
        Value::Str(self.strings.intern(s))
    }

    fn binop(&mut self, op: BinOp, a: Value, b: Value) -> Result<Value, String> {
        use Value::{Float, Int, Vec3};
        let num = |v: &Value| v.as_float();
        Ok(match op {
            BinOp::Eq => Value::bool(self.equal(&a, &b)),
            BinOp::Ne => Value::bool(!self.equal(&a, &b)),
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let r = match (&a, &b) {
                    (Int(x), Int(y)) => match op {
                        BinOp::Lt => x < y,
                        BinOp::Gt => x > y,
                        BinOp::Le => x <= y,
                        _ => x >= y,
                    },
                    _ => {
                        let (Some(x), Some(y)) = (num(&a), num(&b)) else {
                            return Err(format!(
                                "cannot compare {} with {}",
                                a.type_name(),
                                b.type_name()
                            ));
                        };
                        match op {
                            BinOp::Lt => x < y,
                            BinOp::Gt => x > y,
                            BinOp::Le => x <= y,
                            _ => x >= y,
                        }
                    }
                };
                Value::bool(r)
            }
            BinOp::Or | BinOp::Xor | BinOp::And | BinOp::Shl | BinOp::Shr => {
                let (Some(x), Some(y)) = (a.as_int(), b.as_int()) else {
                    return Err(format!(
                        "bit operation on {} and {}",
                        a.type_name(),
                        b.type_name()
                    ));
                };
                Int(match op {
                    BinOp::Or => x | y,
                    BinOp::Xor => x ^ y,
                    BinOp::And => x & y,
                    BinOp::Shl => x.wrapping_shl(y as u32 & 31),
                    _ => x.wrapping_shr(y as u32 & 31),
                })
            }
            BinOp::Add => match (&a, &b) {
                (Int(x), Int(y)) => Int(x.wrapping_add(*y)),
                // bo2mp: a hashed dvar name plus an index (`_class`'s
                // exclusion lists walk `#"..." + n` until one is unset): a
                // hash no dvar of ours has, so it reads as unset.
                (Value::Hash(h), Int(y)) | (Int(y), Value::Hash(h)) => {
                    Value::Hash(h.wrapping_add(*y as u32))
                }
                (Vec3(x), Vec3(y)) => Vec3([x[0] + y[0], x[1] + y[1], x[2] + y[2]]),
                (Value::Str(_) | Value::IStr(_), _) | (_, Value::Str(_) | Value::IStr(_)) => {
                    let s = format!("{}{}", self.to_text(&a), self.to_text(&b));
                    self.string(&s)
                }
                _ => match (num(&a), num(&b)) {
                    (Some(x), Some(y)) => Float(x + y),
                    _ => {
                        return Err(format!(
                            "cannot add {} and {}",
                            a.type_name(),
                            b.type_name()
                        ));
                    }
                },
            },
            BinOp::Sub => match (&a, &b) {
                (Int(x), Int(y)) => Int(x.wrapping_sub(*y)),
                (Vec3(x), Vec3(y)) => Vec3([x[0] - y[0], x[1] - y[1], x[2] - y[2]]),
                _ => match (num(&a), num(&b)) {
                    (Some(x), Some(y)) => Float(x - y),
                    _ => {
                        return Err(format!(
                            "cannot subtract {} and {}",
                            a.type_name(),
                            b.type_name()
                        ));
                    }
                },
            },
            BinOp::Mul => match (&a, &b) {
                (Int(x), Int(y)) => Int(x.wrapping_mul(*y)),
                (Vec3(x), Vec3(y)) => Vec3([x[0] * y[0], x[1] * y[1], x[2] * y[2]]),
                (Vec3(v), s) | (s, Vec3(v)) => match num(s) {
                    Some(k) => Vec3([v[0] * k, v[1] * k, v[2] * k]),
                    None => return Err(format!("cannot multiply a vector by {}", s.type_name())),
                },
                _ => match (num(&a), num(&b)) {
                    (Some(x), Some(y)) => Float(x * y),
                    _ => {
                        return Err(format!(
                            "cannot multiply {} and {}",
                            a.type_name(),
                            b.type_name()
                        ));
                    }
                },
            },
            BinOp::Div => match (&a, &b) {
                (Vec3(v), s) => match num(s) {
                    Some(k) if k != 0.0 => Vec3([v[0] / k, v[1] / k, v[2] / k]),
                    _ => return Err("vector divided by zero or a non-number".into()),
                },
                _ => match (num(&a), num(&b)) {
                    (Some(_), Some(y)) if y == 0.0 => return Err("divide by zero".into()),
                    (Some(x), Some(y)) => Float(x / y),
                    _ => {
                        return Err(format!(
                            "cannot divide {} by {}",
                            a.type_name(),
                            b.type_name()
                        ));
                    }
                },
            },
            BinOp::Mod => match (&a, &b) {
                (Int(_), Int(0)) => return Err("modulus by zero".into()),
                (Int(x), Int(y)) => Int(x.wrapping_rem(*y)),
                _ => match (num(&a), num(&b)) {
                    (Some(_), Some(y)) if y == 0.0 => return Err("modulus by zero".into()),
                    (Some(x), Some(y)) => Float(x % y),
                    _ => {
                        return Err(format!(
                            "cannot take {} modulo {}",
                            a.type_name(),
                            b.type_name()
                        ));
                    }
                },
            },
        })
    }

    fn index(&mut self, container: &Value, key: &Value) -> Result<Value, String> {
        match container {
            Value::Array(a) => {
                let k = self.to_key(key)?;
                Ok(self.live(a.get(&k).unwrap_or_default()))
            }
            Value::Str(s) | Value::IStr(s) => {
                let i = key.as_int().ok_or("string index is not a number")?;
                let text = self.strings.get(*s);
                let c = text
                    .chars()
                    .nth(i.max(0) as usize)
                    .ok_or("string index out of range")?;
                Ok(self.string(&c.to_string()))
            }
            Value::Vec3(v) => {
                let i = key.as_int().ok_or("vector index is not a number")?;
                v.get(i as usize)
                    .map(|x| Value::Float(*x))
                    .ok_or_else(|| "vector index out of range".into())
            }
            Value::Undefined => Err("index into undefined".into()),
            other => Err(format!("cannot index a {}", other.type_name())),
        }
    }

    fn write_path(slot: &mut Value, path: &[Key], v: Value) -> Result<(), String> {
        let Some((&k, rest)) = path.split_first() else {
            *slot = v;
            return Ok(());
        };
        if slot.is_undefined() {
            *slot = Value::array(Array::new());
        }
        let Value::Array(rc) = slot else {
            return Err(format!("{} is not an array", slot.type_name()));
        };
        rc.make_unique();
        let mut arr = rc.write();
        if rest.is_empty() {
            arr.set(k, v);
            return Ok(());
        }
        let child = arr.map.entry(k).or_insert(Value::Undefined);
        let r = Self::write_path(child, rest, v);
        if child.is_undefined() {
            arr.map.shift_remove(&k);
        }
        r
    }

    fn read_path(&mut self, mut v: Value, path: &[Key]) -> Value {
        for k in path {
            v = match &v {
                Value::Array(a) => a.get(k).unwrap_or_default(),
                _ => return Value::Undefined,
            };
        }
        self.live(v)
    }

    fn read_ref(&mut self, host: &mut H, th: &Thread, r: &Ref) -> Value {
        let base = match &r.base {
            RefBase::None => Value::Undefined,
            RefBase::Local(k) => {
                let f = th.frames.last().unwrap();
                let n = f.locals.len();
                f.locals
                    .get(n.wrapping_sub(1 + *k as usize))
                    .cloned()
                    .unwrap_or_default()
            }
            RefBase::Field(o, f) => self.get_field(host, *o, *f),
            RefBase::Game => self.game.clone(),
        };
        self.read_path(base, &r.path)
    }

    fn write_ref(
        &mut self,
        host: &mut H,
        th: &mut Thread,
        r: &Ref,
        v: Value,
    ) -> Result<(), String> {
        match &r.base {
            RefBase::None => Err("assignment with no target".into()),
            RefBase::Local(k) => {
                let f = th.frames.last_mut().unwrap();
                let n = f.locals.len();
                let slot = f
                    .locals
                    .get_mut(n.wrapping_sub(1 + *k as usize))
                    .ok_or("local variable out of range")?;
                Self::write_path(slot, &r.path, v)
            }
            RefBase::Field(o, f) => {
                if r.path.is_empty() {
                    self.set_field(host, *o, *f, v);
                    return Ok(());
                }
                let mut cur = self.get_field(host, *o, *f);
                // Take the script field out so the array is not shared.
                self.set_raw_field(*o, *f, Value::Undefined);
                let res = Self::write_path(&mut cur, &r.path, v);
                self.set_field(host, *o, *f, cur);
                res
            }
            RefBase::Game => {
                let mut g = std::mem::take(&mut self.game);
                let res = Self::write_path(&mut g, &r.path, v);
                self.game = g;
                res
            }
        }
    }

    fn pop_args(th: &mut Thread) -> Vec<Value> {
        let mut args = Vec::new();
        while let Some(v) = th.stack.pop() {
            if matches!(v, Value::Marker) {
                return args;
            }
            args.push(v);
        }
        args
    }

    /// Log the next `count` calls of each named builtin (both forms).
    /// Log every call of these script functions (by bare name).
    pub fn trace_functions(&mut self, names: &str) {
        for n in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let want = n.to_ascii_lowercase();
            for (i, f) in self.program.functions.iter().enumerate() {
                if self.strings.get(f.name) == want {
                    self.ftrace.insert(i as u32);
                }
            }
        }
    }

    pub fn trace_builtins(&mut self, names: &str, count: u32) {
        for n in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let Some(s) = self.strings.find(&n.to_ascii_lowercase()) else {
                continue;
            };
            for method in [false, true] {
                if let Some(&id) = self.program.builtin_index.get(&(s, method)) {
                    self.trace.insert(id, count);
                }
            }
        }
    }

    fn call_native(&mut self, host: &mut H, id: u32, self_: &Value, args: &[Value]) -> Value {
        if let Some(left) = self.trace.get_mut(&id)
            && *left > 0
        {
            *left -= 1;
            let r = self.call_native_inner(host, id, self_, args);
            let (n, m) = self.program.builtins[id as usize];
            let a: Vec<String> = args.iter().map(|a| self.to_text(a)).collect();
            let who = match self_ {
                Value::Object(o) => match self.kind(*o) {
                    Some(ObjKind::Entity(e)) => format!("ent{e} "),
                    Some(ObjKind::Level) => "level ".to_owned(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            let line = format!(
                "trace {who}{}{}({}) = {}",
                if m { "." } else { "" },
                self.strings.get(n),
                a.join(", "),
                self.to_text(&r)
            );
            self.messages.push(line);
            return r;
        }
        self.call_native_inner(host, id, self_, args)
    }

    fn call_native_inner(&mut self, host: &mut H, id: u32, self_: &Value, args: &[Value]) -> Value {
        match self.natives.get(id as usize).copied().flatten() {
            Some(f) => match f(self, host, self_, args) {
                Ok(v) => v,
                Err(e) => {
                    let (n, m) = self.program.builtins[id as usize];
                    let msg = format!(
                        "{}{}: {e}",
                        if m { "method " } else { "" },
                        self.strings.get(n)
                    );
                    self.report_once(msg);
                    Value::Undefined
                }
            },
            None => match self.hooks.unbound {
                Some(hook) => hook(self, host, id, self_, args),
                None => {
                    let (name, method) = self.program.builtins[id as usize];
                    let n = self.strings.get(name).to_owned();
                    self.report_once(format!(
                        "unbound builtin {}{n}",
                        if method { "method " } else { "" }
                    ));
                    Value::Undefined
                }
            },
        }
    }

    /// Call a function value now with `self_` and `args` (from a builtin):
    /// a script function runs as a new thread until it yields.
    pub fn call_value(&mut self, host: &mut H, f: &Value, self_: Value, args: Vec<Value>) -> Value {
        match f {
            Value::Func(FuncRef::Script(fi)) => {
                self.spawn(host, *fi, self_, args);
                Value::Undefined
            }
            Value::Func(FuncRef::Builtin(b)) => self.call_native(host, *b, &self_, &args),
            _ => Value::Undefined,
        }
    }

    fn register(&mut self, t: ThreadId, obj: ObjRef, name: Str, kind: WaiterKind) {
        self.waiters
            .entry((obj.index, name))
            .or_default()
            .push(Waiter {
                thread: t,
                obj_gen: obj.generation,
                kind,
            });
    }

    fn wait_frames(seconds: f32) -> i64 {
        let frames = (seconds * 20.0 + 0.5).floor() as i64;
        frames.max(1)
    }

    fn timer(&mut self, t: ThreadId, th: &mut Thread, ms: i64) {
        self.next_seq += 1;
        th.timer_seq = self.next_seq;
        th.state = State::Timer;
        self.timers.push(Reverse((
            self.time_ms + ms,
            self.next_seq,
            t.index,
            t.generation,
        )));
    }

    #[allow(clippy::too_many_lines)]
    fn interp(&mut self, host: &mut H, t: ThreadId, th: &mut Thread, program: &Program) -> Outcome {
        let mut r = Ref::default();
        let mut obj: Option<ObjRef> = None;
        let mut steps: u64 = 0;
        macro_rules! pop {
            () => {
                th.stack.pop().unwrap_or_default()
            };
        }
        macro_rules! fail {
            ($($arg:tt)*) => {{
                let m = format!($($arg)*);
                self.fail(program, th, &m);
            }};
        }
        loop {
            if let Some(d) = self.pending_unwind.remove(&t) {
                if !self.apply_unwind(t, th, d) {
                    return Outcome::Done;
                }
            }
            steps += 1;
            if steps > STEP_LIMIT {
                let chain: Vec<String> = th
                    .frames
                    .iter()
                    .map(|fr| {
                        let func = &program.functions[fr.func as usize];
                        let at = func
                            .addrs
                            .get(fr.pc.saturating_sub(1) as usize)
                            .copied()
                            .unwrap_or(0);
                        format!(
                            "{}::{}@{at:#x}",
                            program.scripts[func.script as usize].name,
                            self.strings.get(func.name)
                        )
                    })
                    .collect();
                fail!(
                    "thread ran {STEP_LIMIT} instructions without waiting; ended ({})",
                    chain.join(" > ")
                );
                return Outcome::Done;
            }
            let Some(frame) = th.frames.last_mut() else {
                return Outcome::Done;
            };
            let func = &program.functions[frame.func as usize];
            let pc = frame.pc as usize;
            let op = func.code.get(pc).copied().unwrap_or(Op::End);
            frame.pc += 1;
            match op {
                Op::End | Op::Return => {
                    let v = if matches!(op, Op::Return) {
                        pop!()
                    } else {
                        Value::Undefined
                    };
                    let f = th.frames.pop().unwrap();
                    th.stack.truncate(f.base as usize);
                    let depth = th.frames.len() as u32;
                    if th.endons.iter().any(|e| e.2 >= depth) {
                        let mut removed = Vec::new();
                        th.endons.retain(|e| {
                            if e.2 >= depth {
                                removed.push((e.0, e.1));
                                false
                            } else {
                                true
                            }
                        });
                        for (o, n) in removed {
                            if let Some(list) = self.waiters.get_mut(&(o.index, n)) {
                                list.retain(|w| {
                                    !(w.thread == t
                                        && matches!(w.kind, WaiterKind::Endon(d) if d >= depth))
                                });
                                if list.is_empty() {
                                    self.waiters.remove(&(o.index, n));
                                }
                            }
                        }
                    }
                    if th.frames.is_empty() {
                        return Outcome::Done;
                    }
                    th.stack.push(v);
                    obj = None;
                }
                Op::Undefined => th.stack.push(Value::Undefined),
                Op::Int(i) => th.stack.push(Value::Int(i)),
                Op::Float(f) => th.stack.push(Value::Float(f)),
                Op::Str(s) => th.stack.push(Value::Str(s)),
                Op::IStr(s) => th.stack.push(Value::IStr(s)),
                Op::Vector(i) => th.stack.push(Value::Vec3(program.vectors[i as usize])),
                Op::Hash(h) => th.stack.push(Value::Hash(h)),
                Op::LevelObject => obj = Some(self.level),
                Op::AnimObject => obj = Some(self.anim),
                Op::SelfObject => {
                    let s = th.frames.last().unwrap().self_.clone();
                    match s {
                        Value::Object(o) => obj = Some(o),
                        other => {
                            obj = None;
                            fail!("self is {}, not an object", other.type_name());
                        }
                    }
                }
                Op::SelfValue => {
                    let s = th.frames.last().unwrap().self_.clone();
                    let s = self.live(s);
                    th.stack.push(s);
                }
                Op::Level => th.stack.push(Value::Object(self.level)),
                Op::Game => th.stack.push(self.game.clone()),
                Op::AnimValue => th.stack.push(Value::Object(self.anim)),
                Op::GameRef => {
                    r.base = RefBase::Game;
                    r.path.clear();
                }
                Op::Anim(i) => th.stack.push(Value::Anim(i)),
                Op::AnimTree(i) => th.stack.push(Value::AnimTree(i)),
                Op::FuncRef(site) => th
                    .stack
                    .push(Value::Func(program.sites[site as usize].callee)),
                Op::Locals(n) => {
                    let params = usize::from(func.params);
                    let frame = th.frames.last_mut().unwrap();
                    let args = std::mem::take(&mut frame.args);
                    let mut locals = vec![Value::Undefined; usize::from(n)];
                    for (i, a) in args
                        .into_iter()
                        .enumerate()
                        .take(params.min(usize::from(n)))
                    {
                        locals[i] = a;
                    }
                    frame.locals = locals;
                }
                Op::CheckClearParams => {
                    th.frames.last_mut().unwrap().args.clear();
                }
                Op::EvalLocal(k) => {
                    let f = th.frames.last().unwrap();
                    let n = f.locals.len();
                    let v = f
                        .locals
                        .get(n.wrapping_sub(1 + k as usize))
                        .cloned()
                        .unwrap_or_default();
                    let v = self.live(v);
                    th.stack.push(v);
                }
                Op::EvalLocalRef(k) | Op::EvalLocalArrayRef(k) => {
                    r.base = RefBase::Local(k);
                    r.path.clear();
                }
                Op::SetLocal(k) => {
                    let v = pop!();
                    let f = th.frames.last_mut().unwrap();
                    let n = f.locals.len();
                    if let Some(slot) = f.locals.get_mut(n.wrapping_sub(1 + k as usize)) {
                        *slot = v;
                    }
                }
                Op::EvalArray => {
                    let a = pop!();
                    let k = pop!();
                    match self.index(&a, &k) {
                        Ok(v) => th.stack.push(v),
                        Err(e) => {
                            th.stack.push(Value::Undefined);
                            if !a.is_undefined() {
                                fail!("{e}");
                            }
                        }
                    }
                }
                Op::EvalArrayRef => {
                    let k = pop!();
                    match self.to_key(&k) {
                        Ok(k) => r.path.push(k),
                        Err(e) => {
                            fail!("{e}");
                            r.base = RefBase::None;
                        }
                    }
                }
                Op::ClearArray => {
                    let k = pop!();
                    if let Ok(k) = self.to_key(&k) {
                        let cur = self.read_ref(host, th, &r);
                        if let Value::Array(mut rc) = cur
                            && rc.get(&k).is_some()
                        {
                            rc.make_unique();
                            rc.write().set(k, Value::Undefined);
                            let rr = r.clone();
                            let _ = self.write_ref(host, th, &rr, Value::Array(rc));
                        }
                    }
                }
                Op::EmptyArray => th.stack.push(Value::array(Array::new())),
                Op::EvalField(f) => {
                    let v = match obj {
                        Some(o) => self.get_field(host, o, f),
                        None => Value::Undefined,
                    };
                    let v = self.live(v);
                    th.stack.push(v);
                }
                Op::EvalFieldRef(f) => {
                    match obj {
                        Some(o) => r.base = RefBase::Field(o, f),
                        None => {
                            r.base = RefBase::None;
                            fail!("field '{}' of a non-object", self.strings.get(f));
                        }
                    }
                    r.path.clear();
                }
                Op::ClearField(f) => {
                    if let Some(o) = obj {
                        self.set_field(host, o, f, Value::Undefined);
                    }
                }
                Op::WaittillVar(k) => {
                    let v = if matches!(th.stack.last(), Some(Value::Marker) | None) {
                        Value::Undefined
                    } else {
                        pop!()
                    };
                    let f = th.frames.last_mut().unwrap();
                    let n = f.locals.len();
                    if let Some(slot) = f.locals.get_mut(n.wrapping_sub(1 + k as usize)) {
                        *slot = v;
                    }
                }
                Op::ClearParams => {
                    while let Some(v) = th.stack.pop() {
                        if matches!(v, Value::Marker) {
                            break;
                        }
                    }
                }
                Op::Set => {
                    let v = pop!();
                    let rr = std::mem::take(&mut r);
                    if let Err(e) = self.write_ref(host, th, &rr, v) {
                        fail!("{e}");
                    }
                    r = rr;
                }
                Op::Wait | Op::RealWait => {
                    let v = pop!();
                    let secs = v.as_float().unwrap_or_else(|| 0.05);
                    let ms = Self::wait_frames(secs) * FRAME_MS;
                    self.timer(t, th, ms);
                    return Outcome::Yield;
                }
                Op::FrameEnd => {
                    th.state = State::FrameEnd;
                    self.frame_end.push(t);
                    return Outcome::Yield;
                }
                Op::PreCall | Op::VoidCodePos => th.stack.push(Value::Marker),
                Op::Call(site, kind) => {
                    let callee = program.sites[site as usize].callee;
                    let self_v = if kind.method() {
                        pop!()
                    } else {
                        th.frames.last().unwrap().self_.clone()
                    };
                    let args = Self::pop_args(th);
                    if let Some(o) = self.call(host, t, th, program, callee, kind, self_v, args) {
                        return o;
                    }
                    obj = None;
                }
                Op::CallPointer(kind) => {
                    let f = pop!();
                    let self_v = if kind.method() {
                        pop!()
                    } else {
                        th.frames.last().unwrap().self_.clone()
                    };
                    let args = Self::pop_args(th);
                    match f {
                        Value::Func(callee) => {
                            if let Some(o) =
                                self.call(host, t, th, program, callee, kind, self_v, args)
                            {
                                return o;
                            }
                        }
                        other => {
                            fail!("call through a {}, not a function", other.type_name());
                            th.stack.push(Value::Undefined);
                        }
                    }
                    obj = None;
                }
                Op::DecTop => {
                    th.stack.pop();
                }
                Op::CastObject => {
                    let v = pop!();
                    match v {
                        Value::Object(o) if self.alive(o) => obj = Some(o),
                        other => {
                            obj = None;
                            if !matches!(other, Value::Object(_)) {
                                fail!("{} is not an object", other.type_name());
                            }
                        }
                    }
                }
                Op::CastBool => {
                    let v = pop!();
                    th.stack.push(Value::bool(truthy(&v)));
                }
                Op::Not => {
                    let v = pop!();
                    th.stack.push(Value::bool(!truthy(&v)));
                }
                Op::Complement => {
                    let v = pop!();
                    th.stack.push(Value::Int(!v.as_int().unwrap_or(0)));
                }
                Op::JumpFalse(to) => {
                    let v = pop!();
                    if !truthy(&v) {
                        th.frames.last_mut().unwrap().pc = to;
                    }
                }
                Op::JumpTrue(to) => {
                    let v = pop!();
                    if truthy(&v) {
                        th.frames.last_mut().unwrap().pc = to;
                    }
                }
                Op::JumpFalseExpr(to) => {
                    if th.stack.last().is_some_and(|v| !truthy(v)) {
                        th.frames.last_mut().unwrap().pc = to;
                    } else {
                        th.stack.pop();
                    }
                }
                Op::JumpTrueExpr(to) => {
                    if th.stack.last().is_some_and(truthy) {
                        th.frames.last_mut().unwrap().pc = to;
                    } else {
                        th.stack.pop();
                    }
                }
                Op::Jump(to) => th.frames.last_mut().unwrap().pc = to,
                Op::Inc | Op::Dec => {
                    let cur = self.read_ref(host, th, &r);
                    let d = if matches!(op, Op::Inc) { 1 } else { -1 };
                    let nv = match cur {
                        Value::Int(i) => Value::Int(i.wrapping_add(d)),
                        Value::Float(f) => Value::Float(f + d as f32),
                        Value::Undefined => {
                            fail!("++/-- on undefined");
                            Value::Int(d)
                        }
                        other => {
                            fail!("++/-- on a {}", other.type_name());
                            other
                        }
                    };
                    let rr = r.clone();
                    if let Err(e) = self.write_ref(host, th, &rr, nv) {
                        fail!("{e}");
                    }
                }
                Op::Bin(b) => {
                    let y = pop!();
                    let x = pop!();
                    match self.binop(b, x, y) {
                        Ok(v) => th.stack.push(v),
                        Err(e) => {
                            fail!("{e}");
                            th.stack.push(Value::Undefined);
                        }
                    }
                }
                Op::Size => {
                    let v = pop!();
                    let n = match &v {
                        Value::Array(a) => a.len() as i32,
                        Value::Str(s) | Value::IStr(s) => {
                            self.strings.get(*s).chars().count() as i32
                        }
                        Value::Undefined => {
                            fail!("size of undefined");
                            0
                        }
                        other => {
                            fail!("size of a {}", other.type_name());
                            0
                        }
                    };
                    th.stack.push(Value::Int(n));
                }
                Op::Waittill | Op::WaittillMatch(_) => {
                    let o = pop!();
                    let n = pop!();
                    let mut vals = Vec::new();
                    if let Op::WaittillMatch(k) = op {
                        for _ in 0..k {
                            vals.push(pop!());
                        }
                    }
                    match (o, n.as_str_id()) {
                        (Value::Object(o), Some(n)) if self.alive(o) => {
                            let kind = if matches!(op, Op::Waittill) {
                                WaiterKind::Waittill
                            } else {
                                WaiterKind::Match(vals)
                            };
                            self.register(t, o, n, kind);
                            th.waiting_on = Some((o, n));
                            th.state = State::Notify;
                            return Outcome::Yield;
                        }
                        (Value::Object(_), Some(_)) => {
                            // Waiting on a deleted object ends the thread.
                            return Outcome::Done;
                        }
                        (o, n) => {
                            fail!(
                                "waittill on {} for {:?}",
                                o.type_name(),
                                n.map(|s| self.strings.get(s).to_owned())
                            );
                            th.stack.push(Value::Marker);
                        }
                    }
                }
                Op::Notify => {
                    let o = pop!();
                    let n = pop!();
                    let args = Self::pop_args(th);
                    match (o, n.as_str_id()) {
                        (Value::Object(o), Some(n)) => {
                            if self.alive(o) {
                                self.notify(host, o, n, &args);
                            }
                        }
                        (o, _) => fail!("notify on a {}", o.type_name()),
                    }
                }
                Op::Endon => {
                    let o = pop!();
                    let n = pop!();
                    match (o, n.as_str_id()) {
                        (Value::Object(o), Some(n)) if self.alive(o) => {
                            let depth = th.frames.len() as u32 - 1;
                            th.endons.push((o, n, depth));
                            self.register(t, o, n, WaiterKind::Endon(depth));
                        }
                        (Value::Object(_), Some(_)) => return Outcome::Done,
                        (o, _) => fail!("endon on a {}", o.type_name()),
                    }
                }
                Op::Switch(i) => {
                    let v = pop!();
                    let table = &program.switches[i as usize];
                    let hit = match &v {
                        Value::Int(n) => table.ints.get(n).copied(),
                        Value::Float(f) if f.fract() == 0.0 => {
                            table.ints.get(&(*f as i32)).copied()
                        }
                        Value::Str(s) | Value::IStr(s) => table.strs.get(s).copied(),
                        _ => None,
                    };
                    let to = hit.or(table.default).unwrap_or(table.end);
                    th.frames.last_mut().unwrap().pc = to;
                }
                Op::MakeVector => {
                    let x = pop!();
                    let y = pop!();
                    let z = pop!();
                    match (x.as_float(), y.as_float(), z.as_float()) {
                        (Some(x), Some(y), Some(z)) => th.stack.push(Value::Vec3([x, y, z])),
                        _ => {
                            fail!("vector of non-numbers");
                            th.stack.push(Value::Undefined);
                        }
                    }
                }
                Op::VectorConst(flags) => {
                    let c = |b: u8| match b & 3 {
                        1 => -1.0,
                        2 => 1.0,
                        _ => 0.0,
                    };
                    th.stack
                        .push(Value::Vec3([c(flags >> 4), c(flags >> 2), c(flags)]));
                }
                Op::IsDefined => {
                    let v = pop!();
                    th.stack.push(Value::bool(self.is_defined(&v)));
                }
                Op::VectorScale => {
                    let v = pop!();
                    let s = pop!();
                    match (v.as_vec3(), s.as_float()) {
                        (Some(v), Some(s)) => {
                            th.stack.push(Value::Vec3([v[0] * s, v[1] * s, v[2] * s]))
                        }
                        _ => {
                            fail!("vectorscale of {} by {}", v.type_name(), s.type_name());
                            th.stack.push(Value::Undefined);
                        }
                    }
                }
                Op::AnglesToUp | Op::AnglesToRight | Op::AnglesToForward => {
                    let v = pop!();
                    match v.as_vec3() {
                        Some(a) => {
                            let (f, rgt, up) = crate::math::angle_vectors(a);
                            th.stack.push(Value::Vec3(match op {
                                Op::AnglesToUp => up,
                                Op::AnglesToRight => rgt,
                                _ => f,
                            }));
                        }
                        None => {
                            fail!("angles of a {}", v.type_name());
                            th.stack.push(Value::Undefined);
                        }
                    }
                }
                Op::AngleClamp180 => {
                    let v = pop!();
                    let a = v.as_float().unwrap_or(0.0);
                    th.stack.push(Value::Float(crate::math::angle_clamp180(a)));
                }
                Op::VectorToAngles => {
                    let v = pop!();
                    match v.as_vec3() {
                        Some(d) => th.stack.push(Value::Vec3(crate::math::vector_to_angles(d))),
                        None => {
                            fail!("vectortoangles of a {}", v.type_name());
                            th.stack.push(Value::Undefined);
                        }
                    }
                }
                Op::Abs => {
                    let v = pop!();
                    th.stack.push(match v {
                        Value::Int(i) => Value::Int(i.wrapping_abs()),
                        Value::Float(f) => Value::Float(f.abs()),
                        _ => Value::Undefined,
                    });
                }
                Op::GetTime => th.stack.push(Value::Int(self.time_ms as i32)),
                Op::GetDvar
                | Op::GetDvarInt
                | Op::GetDvarFloat
                | Op::GetDvarVector
                | Op::GetDvarColor(_) => {
                    let n = pop!();
                    let text = self.dvar_text(&n);
                    let v = match op {
                        Op::GetDvar => self.string(&text),
                        Op::GetDvarInt => Value::Int(crate::math::parse_int(&text)),
                        Op::GetDvarFloat => Value::Float(text.trim().parse::<f32>().unwrap_or(0.0)),
                        Op::GetDvarVector => {
                            let p: Vec<f32> = text
                                .split_whitespace()
                                .filter_map(|x| x.parse().ok())
                                .collect();
                            Value::Vec3([
                                p.first().copied().unwrap_or(0.0),
                                p.get(1).copied().unwrap_or(0.0),
                                p.get(2).copied().unwrap_or(0.0),
                            ])
                        }
                        Op::GetDvarColor(c) => {
                            let p: Vec<f32> = text
                                .split_whitespace()
                                .filter_map(|x| x.parse().ok())
                                .collect();
                            Value::Float(p.get(usize::from(c)).copied().unwrap_or(if c == 3 {
                                1.0
                            } else {
                                0.0
                            }))
                        }
                        _ => Value::Undefined,
                    };
                    th.stack.push(v);
                }
                Op::FirstKey => {
                    let a = pop!();
                    let v = match &a {
                        Value::Array(a) => a.read().first_key().map(Key::value).unwrap_or_default(),
                        Value::Undefined => {
                            fail!("foreach over undefined");
                            Value::Undefined
                        }
                        other => {
                            fail!("foreach over a {}", other.type_name());
                            Value::Undefined
                        }
                    };
                    th.stack.push(v);
                }
                Op::NextKey => {
                    let a = pop!();
                    let k = pop!();
                    let v = match (&a, self.to_key(&k)) {
                        (Value::Array(a), Ok(k)) => {
                            a.read().next_key(&k).map(Key::value).unwrap_or_default()
                        }
                        _ => Value::Undefined,
                    };
                    th.stack.push(v);
                }
                Op::Nop => {}
                Op::Bad(c) => {
                    fail!("unsupported opcode {}", crate::object::op_name(c));
                }
            }
        }
    }

    /// Perform a call; `Some(outcome)` if the thread must stop here.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        host: &mut H,
        t: ThreadId,
        th: &mut Thread,
        program: &Program,
        callee: FuncRef,
        kind: CallKind,
        self_v: Value,
        args: Vec<Value>,
    ) -> Option<Outcome> {
        match callee {
            FuncRef::Script(fi) => {
                if !self.ftrace.is_empty() && self.ftrace.contains(&fi) {
                    let func = &program.functions[fi as usize];
                    let a: Vec<String> = args.iter().map(|v| self.to_text(v)).collect();
                    // The calling function, so a trace says who asked.
                    let from = th.frames.last().map_or(String::from("-"), |f| {
                        let cf = &program.functions[f.func as usize];
                        format!(
                            "{}::{}",
                            program.scripts[cf.script as usize].name,
                            self.strings.get(cf.name)
                        )
                    });
                    let m = format!(
                        "ftrace {}::{}({}) self {} from {from}",
                        program.scripts[func.script as usize].name,
                        self.strings.get(func.name),
                        a.join(", "),
                        self.to_text(&self_v)
                    );
                    self.messages.push(m);
                }
                if kind.thread() {
                    self.spawn(host, fi, self_v, args);
                    th.stack.push(Value::Undefined);
                } else {
                    if th.frames.len() >= MAX_FRAMES {
                        self.fail(program, th, "call stack too deep");
                        th.stack.push(Value::Undefined);
                        return None;
                    }
                    let base = th.stack.len() as u32;
                    th.frames.push(Frame {
                        func: fi,
                        pc: 0,
                        locals: Vec::new(),
                        self_: self_v,
                        base,
                        args,
                    });
                }
            }
            FuncRef::Builtin(b) => {
                let v = self.call_native(host, b, &self_v, &args);
                th.stack
                    .push(if kind.thread() { Value::Undefined } else { v });
            }
            FuncRef::Missing(n) => {
                let m = format!("missing function {}", self.strings.get(n));
                self.report_once(m);
                th.stack.push(Value::Undefined);
            }
        }
        let _ = t;
        None
    }

    /// One line per waiting thread: where it is and what it waits for
    /// (debugging: "what are the scripts stuck on?").
    pub fn describe_threads(&self) -> Vec<String> {
        let mut out = Vec::new();
        for th in self.threads.iter().flatten() {
            let Some(f) = th.frames.last() else { continue };
            let func = &self.program.functions[f.func as usize];
            let script = &self.program.scripts[func.script as usize].name;
            let chain: Vec<String> = th
                .frames
                .iter()
                .map(|fr| {
                    self.strings
                        .get(self.program.functions[fr.func as usize].name)
                        .to_owned()
                })
                .collect();
            let what = match th.state {
                State::Timer => "wait".to_owned(),
                State::FrameEnd => "waittillframeend".to_owned(),
                State::Notify => match th.waiting_on {
                    Some((o, n)) => {
                        let who = match self.kind(o) {
                            Some(ObjKind::Level) => "level".to_owned(),
                            Some(ObjKind::Entity(e)) => format!("ent{e}"),
                            Some(k) => format!("{k:?}"),
                            None => "dead".to_owned(),
                        };
                        format!("waittill {who} \"{}\"", self.strings.get(n))
                    }
                    None => "notify?".to_owned(),
                },
                s => format!("{s:?}"),
            };
            out.push(format!(
                "{script}::{} [{}] {what}",
                self.strings.get(func.name),
                chain.join(" > ")
            ));
        }
        out.sort();
        out
    }

    /// The frame-0 self of a thread (tests and tools).
    pub fn thread_self(&self, t: ThreadId) -> Option<Value> {
        self.threads
            .get(t.index as usize)?
            .as_ref()
            .and_then(|th| th.frames.first())
            .map(|f| f.self_.clone())
    }

    /// A dvar's text by name or by `#"name"` hash ("" when unset).
    pub fn dvar_text(&self, name: &Value) -> String {
        match name {
            Value::Hash(h) => self
                .dvars
                .iter()
                .find(|(k, _)| hash_name(k) == *h)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
            v => self
                .dvars
                .get(&self.to_text(v).to_ascii_lowercase())
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The dvar name a value names (a hash maps to a set dvar's name).
    pub fn dvar_name(&self, name: &Value) -> String {
        match name {
            Value::Hash(h) => self
                .dvars
                .keys()
                .find(|k| hash_name(k) == *h)
                .cloned()
                .unwrap_or_else(|| format!("#{h:08x}")),
            v => self.to_text(v).to_ascii_lowercase(),
        }
    }

    /// Next pseudo-random u32 (xorshift; the host may reseed `rng`).
    pub fn rand_u32(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 32) as u32
    }

    /// A pseudo-random float in [0, 1).
    pub fn rand_unit(&mut self) -> f32 {
        (self.rand_u32() >> 8) as f32 / 16_777_216.0
    }
}
