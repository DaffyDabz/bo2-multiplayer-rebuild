//! Linking decoded script objects into one program.
//!
//! Every exported function becomes a [`Function`] whose instructions are
//! [`Op`]s with jumps as instruction indices. Each call site resolves once:
//! a namespaced call to that script's function; an unqualified call to the
//! caller's own function, then its includes' (in include order); anything
//! else is a builtin, function or method by the call's kind. A namespaced
//! call into a script the zones do not carry also becomes a builtin, named
//! `path::function`, for the host to provide (Zombies' missing
//! `maps/mp/gametypes_zm/_globallogic`).

use rustc_hash::{FxHashMap, FxHashSet};

use crate::object::{CaseValue, Insn, ScriptObject, op};
use crate::value::{FuncRef, Str, Strings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Func,
    Method,
    Thread,
    MethodThread,
}

impl CallKind {
    pub fn method(self) -> bool {
        matches!(self, CallKind::Method | CallKind::MethodThread)
    }

    pub fn thread(self) -> bool {
        matches!(self, CallKind::Thread | CallKind::MethodThread)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    Xor,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// A linked instruction. Jump targets are indices into the function's code.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    End,
    Return,
    Undefined,
    Int(i32),
    Float(f32),
    Str(Str),
    IStr(Str),
    /// Index into [`Program::vectors`].
    Vector(u32),
    Hash(u32),
    LevelObject,
    AnimObject,
    SelfObject,
    SelfValue,
    Level,
    Game,
    AnimValue,
    GameRef,
    Anim(u32),
    AnimTree(u32),
    /// Push the function a call site names.
    FuncRef(u32),
    /// The prologue: this many locals, the first `params` from the call.
    Locals(u8),
    EvalLocal(u8),
    EvalLocalRef(u8),
    EvalLocalArrayRef(u8),
    SetLocal(u8),
    EvalArray,
    EvalArrayRef,
    ClearArray,
    EmptyArray,
    EvalField(Str),
    EvalFieldRef(Str),
    ClearField(Str),
    WaittillVar(u8),
    ClearParams,
    CheckClearParams,
    Set,
    Wait,
    RealWait,
    FrameEnd,
    PreCall,
    VoidCodePos,
    Call(u32, CallKind),
    CallPointer(CallKind),
    DecTop,
    CastObject,
    CastBool,
    Not,
    Complement,
    JumpFalse(u32),
    JumpTrue(u32),
    JumpFalseExpr(u32),
    JumpTrueExpr(u32),
    Jump(u32),
    Inc,
    Dec,
    Bin(BinOp),
    Size,
    WaittillMatch(u8),
    Waittill,
    Notify,
    Endon,
    /// Index into [`Program::switches`].
    Switch(u32),
    MakeVector,
    VectorConst(u8),
    IsDefined,
    VectorScale,
    AnglesToUp,
    AnglesToRight,
    AnglesToForward,
    AngleClamp180,
    VectorToAngles,
    Abs,
    GetTime,
    GetDvar,
    GetDvarInt,
    GetDvarFloat,
    GetDvarVector,
    GetDvarColor(u8),
    FirstKey,
    NextKey,
    Nop,
    /// An opcode the VM does not run (never seen in reachable code).
    Bad(u8),
}

#[derive(Clone, Debug)]
pub struct SwitchTable {
    pub ints: FxHashMap<i32, u32>,
    pub strs: FxHashMap<Str, u32>,
    pub default: Option<u32>,
    pub end: u32,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub script: u32,
    pub name: Str,
    pub params: u8,
    pub code: Vec<Op>,
    /// Byte address of each instruction (for messages).
    pub addrs: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub callee: FuncRef,
}

#[derive(Clone, Debug)]
pub struct ScriptInfo {
    /// Normalized name: lower case, `/`, no `.gsc`.
    pub name: String,
    pub includes: Vec<String>,
    pub functions: Vec<u32>,
}

#[derive(Default, Debug)]
pub struct Program {
    pub scripts: Vec<ScriptInfo>,
    pub script_index: FxHashMap<String, u32>,
    pub functions: Vec<Function>,
    /// (script, lower-case function name) -> function.
    pub function_index: FxHashMap<(u32, Str), u32>,
    pub sites: Vec<Site>,
    /// Builtin id -> (name, method).
    pub builtins: Vec<(Str, bool)>,
    pub builtin_index: FxHashMap<(Str, bool), u32>,
    /// Animation id -> (tree, animation).
    pub anims: Vec<(Str, Str)>,
    anim_index: FxHashMap<(Str, Str), u32>,
    pub trees: Vec<Str>,
    tree_index: FxHashMap<Str, u32>,
    pub vectors: Vec<[f32; 3]>,
    pub switches: Vec<SwitchTable>,
    /// Bare names that bind to the engine even where a script defines one.
    engine_first: FxHashSet<(Str, bool)>,
}

/// Normalize a script path: lower case, `/`, no `.gsc`.
pub fn normalize(name: &str) -> String {
    let s = name.replace('\\', "/").to_ascii_lowercase();
    s.strip_suffix(".gsc")
        .map_or_else(|| s.clone(), str::to_owned)
}

impl Program {
    /// Link server scripts (client `.csc` objects are skipped). Later
    /// objects replace earlier ones with the same name.
    pub fn link(objects: Vec<ScriptObject>, strings: &mut Strings) -> Result<Self, String> {
        Self::link_with(objects, strings, &[])
    }

    /// As `link`, but a bare call to one of `engine_first` (name, method)
    /// binds to the engine builtin before the script's own or included
    /// functions, as Black Ops II's linker does for every name the engine
    /// knows.
    pub fn link_with(
        objects: Vec<ScriptObject>,
        strings: &mut Strings,
        engine_first: &[(&str, bool)],
    ) -> Result<Self, String> {
        let mut p = Program::default();
        for &(name, method) in engine_first {
            p.engine_first
                .insert((strings.intern(&name.to_ascii_lowercase()), method));
        }
        let mut by_name: FxHashMap<String, ScriptObject> = FxHashMap::default();
        let mut order: Vec<String> = Vec::new();
        for o in objects {
            if o.name.to_ascii_lowercase().ends_with(".csc") {
                continue;
            }
            let key = normalize(&o.name);
            if !by_name.contains_key(&key) {
                order.push(key.clone());
            }
            by_name.insert(key, o);
        }
        // Pass 1: scripts and function slots.
        for key in &order {
            let o = &by_name[key];
            let si = p.scripts.len() as u32;
            let mut fns = Vec::new();
            for f in &o.functions {
                let fi = p.functions.len() as u32;
                let name = strings.intern(&f.name.to_ascii_lowercase());
                p.function_index.insert((si, name), fi);
                p.functions.push(Function {
                    script: si,
                    name,
                    params: f.params,
                    code: Vec::new(),
                    addrs: Vec::new(),
                });
                fns.push(fi);
            }
            p.script_index.insert(key.clone(), si);
            p.scripts.push(ScriptInfo {
                name: key.clone(),
                includes: o.includes.iter().map(|i| normalize(i)).collect(),
                functions: fns,
            });
        }
        // Pass 2: code.
        for key in &order {
            let o = &by_name[key];
            let si = p.script_index[key];
            for (k, f) in o.functions.iter().enumerate() {
                let fi = p.scripts[si as usize].functions[k];
                let (code, addrs) = p.lower(si, &f.code, strings)?;
                let func = &mut p.functions[fi as usize];
                func.code = code;
                func.addrs = addrs;
            }
        }
        Ok(p)
    }

    /// A function by script path and name.
    pub fn find(&self, strings: &Strings, script: &str, name: &str) -> Option<u32> {
        let si = *self.script_index.get(&normalize(script))?;
        let n = strings.find(&name.to_ascii_lowercase())?;
        self.function_index.get(&(si, n)).copied()
    }

    fn builtin(&mut self, name: Str, method: bool) -> u32 {
        if let Some(&id) = self.builtin_index.get(&(name, method)) {
            return id;
        }
        let id = self.builtins.len() as u32;
        self.builtins.push((name, method));
        self.builtin_index.insert((name, method), id);
        id
    }

    fn resolve(
        &mut self,
        script: u32,
        imp: &crate::object::Import,
        method: bool,
        strings: &mut Strings,
    ) -> FuncRef {
        let lname = imp.name.to_ascii_lowercase();
        let name = strings.intern(&lname);
        if !imp.ns.is_empty() {
            let ns = normalize(&imp.ns);
            if let Some(&si) = self.script_index.get(&ns) {
                if let Some(&fi) = self.function_index.get(&(si, name)) {
                    return FuncRef::Script(fi);
                }
            }
            let full = strings.intern(&format!("{ns}::{lname}"));
            return FuncRef::Builtin(self.builtin(full, method));
        }
        if self.engine_first.contains(&(name, method)) {
            return FuncRef::Builtin(self.builtin(name, method));
        }
        if let Some(&fi) = self.function_index.get(&(script, name)) {
            return FuncRef::Script(fi);
        }
        let includes = self.scripts[script as usize].includes.clone();
        for inc in &includes {
            if let Some(&si) = self.script_index.get(inc) {
                if let Some(&fi) = self.function_index.get(&(si, name)) {
                    return FuncRef::Script(fi);
                }
            }
        }
        FuncRef::Builtin(self.builtin(name, method))
    }

    fn site(&mut self, callee: FuncRef) -> u32 {
        let id = self.sites.len() as u32;
        self.sites.push(Site { callee });
        id
    }

    fn lower(
        &mut self,
        script: u32,
        code: &[(u32, Insn)],
        strings: &mut Strings,
    ) -> Result<(Vec<Op>, Vec<u32>), String> {
        let index: FxHashMap<u32, u32> = code
            .iter()
            .enumerate()
            .map(|(i, (a, _))| (*a, i as u32))
            .collect();
        // Targets that are not instructions (a switch's end after its last
        // case, at the end of the function) go to an implicit End.
        let end_index = code.len() as u32;
        let to = |a: u32| -> u32 { index.get(&a).copied().unwrap_or(end_index) };
        let mut ops = Vec::with_capacity(code.len() + 1);
        let mut addrs = Vec::with_capacity(code.len() + 1);
        for (at, insn) in code {
            let o = match insn {
                Insn::Plain(c) => match *c {
                    op::END => Op::End,
                    op::RETURN => Op::Return,
                    op::GET_UNDEFINED => Op::Undefined,
                    op::GET_ZERO => Op::Int(0),
                    op::GET_LEVEL_OBJECT => Op::LevelObject,
                    op::GET_ANIM_OBJECT => Op::AnimObject,
                    op::GET_SELF => Op::SelfValue,
                    op::GET_LEVEL => Op::Level,
                    op::GET_GAME => Op::Game,
                    op::GET_ANIM => Op::AnimValue,
                    op::GET_GAME_REF => Op::GameRef,
                    op::EVAL_ARRAY => Op::EvalArray,
                    op::EVAL_ARRAY_REF => Op::EvalArrayRef,
                    op::CLEAR_ARRAY => Op::ClearArray,
                    op::EMPTY_ARRAY => Op::EmptyArray,
                    op::GET_SELF_OBJECT => Op::SelfObject,
                    op::CLEAR_PARAMS => Op::ClearParams,
                    op::CHECK_CLEAR_PARAMS => Op::CheckClearParams,
                    op::SET_VARIABLE_FIELD => Op::Set,
                    op::WAIT => Op::Wait,
                    op::WAIT_TILL_FRAME_END => Op::FrameEnd,
                    op::PRE_SCRIPT_CALL => Op::PreCall,
                    op::DEC_TOP => Op::DecTop,
                    op::CAST_FIELD_OBJECT => Op::CastObject,
                    op::CAST_BOOL => Op::CastBool,
                    op::BOOL_NOT => Op::Not,
                    op::BOOL_COMPLEMENT => Op::Complement,
                    op::INC => Op::Inc,
                    op::DEC => Op::Dec,
                    op::BIT_OR => Op::Bin(BinOp::Or),
                    op::BIT_XOR => Op::Bin(BinOp::Xor),
                    op::BIT_AND => Op::Bin(BinOp::And),
                    op::EQUAL => Op::Bin(BinOp::Eq),
                    op::NOT_EQUAL => Op::Bin(BinOp::Ne),
                    op::LESS_THAN => Op::Bin(BinOp::Lt),
                    op::GREATER_THAN => Op::Bin(BinOp::Gt),
                    op::LESS_THAN_OR_EQUAL => Op::Bin(BinOp::Le),
                    op::GREATER_THAN_OR_EQUAL => Op::Bin(BinOp::Ge),
                    op::SHIFT_LEFT => Op::Bin(BinOp::Shl),
                    op::SHIFT_RIGHT => Op::Bin(BinOp::Shr),
                    op::PLUS => Op::Bin(BinOp::Add),
                    op::MINUS => Op::Bin(BinOp::Sub),
                    op::MULTIPLY => Op::Bin(BinOp::Mul),
                    op::DIVIDE => Op::Bin(BinOp::Div),
                    op::MODULUS => Op::Bin(BinOp::Mod),
                    op::SIZE_OF => Op::Size,
                    op::WAIT_TILL => Op::Waittill,
                    op::NOTIFY => Op::Notify,
                    op::END_ON => Op::Endon,
                    op::VOID_CODE_POS => Op::VoidCodePos,
                    op::VECTOR => Op::MakeVector,
                    op::REAL_WAIT => Op::RealWait,
                    op::IS_DEFINED => Op::IsDefined,
                    op::VECTOR_SCALE => Op::VectorScale,
                    op::ANGLES_TO_UP => Op::AnglesToUp,
                    op::ANGLES_TO_RIGHT => Op::AnglesToRight,
                    op::ANGLES_TO_FORWARD => Op::AnglesToForward,
                    op::ANGLE_CLAMP_180 => Op::AngleClamp180,
                    op::VECTOR_TO_ANGLES => Op::VectorToAngles,
                    op::ABS => Op::Abs,
                    op::GET_TIME => Op::GetTime,
                    op::GET_DVAR => Op::GetDvar,
                    op::GET_DVAR_INT => Op::GetDvarInt,
                    op::GET_DVAR_FLOAT => Op::GetDvarFloat,
                    op::GET_DVAR_VECTOR => Op::GetDvarVector,
                    op::GET_DVAR_COLOR_RED => Op::GetDvarColor(0),
                    op::GET_DVAR_COLOR_GREEN => Op::GetDvarColor(1),
                    op::GET_DVAR_COLOR_BLUE => Op::GetDvarColor(2),
                    op::GET_DVAR_COLOR_ALPHA => Op::GetDvarColor(3),
                    op::FIRST_ARRAY_KEY => Op::FirstKey,
                    op::NEXT_ARRAY_KEY => Op::NextKey,
                    op::PROFILE_START | op::PROFILE_STOP | op::NOP | op::DEVBLOCK_END => Op::Nop,
                    op::SAFE_DEC_TOP => Op::DecTop,
                    c => Op::Bad(c),
                },
                Insn::Int(c, v) => match *c {
                    op::GET_BYTE | op::GET_INTEGER => Op::Int(*v),
                    op::EVAL_LOCAL_VARIABLE_CACHED => Op::EvalLocal(*v as u8),
                    op::EVAL_LOCAL_VARIABLE_REF_CACHED => Op::EvalLocalRef(*v as u8),
                    op::EVAL_LOCAL_ARRAY_REF_CACHED => Op::EvalLocalArrayRef(*v as u8),
                    op::SAFE_SET_VARIABLE_FIELD_CACHED => Op::SetLocal(*v as u8),
                    op::SAFE_SET_WAITTILL_VARIABLE_FIELD_CACHED => Op::WaittillVar(*v as u8),
                    op::VECTOR_CONSTANT => Op::VectorConst(*v as u8),
                    op::WAIT_TILL_MATCH => Op::WaittillMatch(*v as u8),
                    op::SCRIPT_FUNCTION_CALL_POINTER => Op::CallPointer(CallKind::Func),
                    op::SCRIPT_METHOD_CALL_POINTER => Op::CallPointer(CallKind::Method),
                    op::SCRIPT_THREAD_CALL_POINTER => Op::CallPointer(CallKind::Thread),
                    op::SCRIPT_METHOD_THREAD_CALL_POINTER => {
                        Op::CallPointer(CallKind::MethodThread)
                    }
                    c => Op::Bad(c),
                },
                Insn::Float(f) => Op::Float(*f),
                Insn::Vector(v) => {
                    self.vectors.push(*v);
                    Op::Vector(self.vectors.len() as u32 - 1)
                }
                Insn::Str(c, s) => {
                    match *c {
                        op::GET_STRING => Op::Str(strings.intern(s)),
                        op::GET_ISTRING => Op::IStr(strings.intern(s)),
                        // Field names are case-insensitive: canonical lower case.
                        op::EVAL_FIELD_VARIABLE => {
                            Op::EvalField(strings.intern(&s.to_ascii_lowercase()))
                        }
                        op::EVAL_FIELD_VARIABLE_REF => {
                            Op::EvalFieldRef(strings.intern(&s.to_ascii_lowercase()))
                        }
                        op::CLEAR_FIELD_VARIABLE => {
                            Op::ClearField(strings.intern(&s.to_ascii_lowercase()))
                        }
                        c => Op::Bad(c),
                    }
                }
                Insn::Locals(names) => Op::Locals(names.len() as u8),
                Insn::Hash(h) => Op::Hash(*h),
                Insn::Call(c, imp) => {
                    let kind = match *c {
                        op::SCRIPT_METHOD_CALL | op::CALL_BUILTIN_METHOD => CallKind::Method,
                        op::SCRIPT_THREAD_CALL => CallKind::Thread,
                        op::SCRIPT_METHOD_THREAD_CALL => CallKind::MethodThread,
                        _ => CallKind::Func,
                    };
                    let callee = self.resolve(script, imp, kind.method(), strings);
                    let site = self.site(callee);
                    if *c == op::GET_FUNCTION {
                        Op::FuncRef(site)
                    } else {
                        Op::Call(site, kind)
                    }
                }
                Insn::Jump(c, a) => {
                    let t = to(*a);
                    match *c {
                        op::JUMP_ON_FALSE => Op::JumpFalse(t),
                        op::JUMP_ON_TRUE => Op::JumpTrue(t),
                        op::JUMP_ON_FALSE_EXPR => Op::JumpFalseExpr(t),
                        op::JUMP_ON_TRUE_EXPR => Op::JumpTrueExpr(t),
                        _ => Op::Jump(t),
                    }
                }
                Insn::Switch { cases, end } => {
                    let mut table = SwitchTable {
                        ints: FxHashMap::default(),
                        strs: FxHashMap::default(),
                        default: None,
                        end: to(*end),
                    };
                    for c in cases {
                        let t = to(c.target);
                        match &c.value {
                            CaseValue::Int(i) => {
                                table.ints.entry(*i).or_insert(t);
                            }
                            CaseValue::Str(s) => {
                                table.strs.entry(strings.intern(s)).or_insert(t);
                            }
                            CaseValue::Default => table.default = Some(t),
                        }
                    }
                    self.switches.push(table);
                    Op::Switch(self.switches.len() as u32 - 1)
                }
                Insn::EndSwitch { end } => Op::Jump(to(*end)),
                Insn::Anim { tree, anim } => {
                    let key = (strings.intern(tree), strings.intern(anim));
                    let id = match self.anim_index.get(&key) {
                        Some(&id) => id,
                        None => {
                            let id = self.anims.len() as u32;
                            self.anims.push(key);
                            self.anim_index.insert(key, id);
                            id
                        }
                    };
                    Op::Anim(id)
                }
                Insn::AnimTree(tree) => {
                    let t = strings.intern(tree);
                    let id = match self.tree_index.get(&t) {
                        Some(&id) => id,
                        None => {
                            let id = self.trees.len() as u32;
                            self.trees.push(t);
                            self.tree_index.insert(t, id);
                            id
                        }
                    };
                    Op::AnimTree(id)
                }
                Insn::DevBlock { to: a } => Op::Jump(to(*a)),
            };
            ops.push(o);
            addrs.push(*at);
        }
        ops.push(Op::End);
        addrs.push(u32::MAX);
        Ok((ops, addrs))
    }
}
