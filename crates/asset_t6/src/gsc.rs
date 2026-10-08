//! bo2zm: Black Ops II compiled scripts, read-only.
//!
//! A zone keeps each script as a `ScriptParseTree` buffer: a GSC object
//! (`\x80GSC\r\n\0\x06`) with a header, include, export, import and string
//! reference tables and the code segment. The tables say what each code
//! address means (an import's call sites, a string's references), so the
//! decoder is checked against them: walking every exported function must
//! read each string reference and each import call site at exactly the
//! address the table names ([`GscObject::coverage`]).
//!
//! What the map scripts are read for is data, not behaviour: the calls they
//! make with constant arguments (ambient rooms, sounds on effects, loops at
//! points), the effect table (`level._effect[name] = loadfx(path)`) and the
//! placed effects (`ent = createoneshoteffect(id); ent.v["origin"] = ...`).
//! [`GscObject::run`] walks each function straight through (jumps are not
//! taken) keeping a value stack, and records every call and every
//! assignment into a script object.

use std::collections::HashMap;

const MAGIC: &[u8; 8] = b"\x80GSC\r\n\x00\x06";

/// Opcodes by value (Black Ops II, PC).
#[allow(dead_code)]
pub mod op {
    pub const END: u8 = 0x00;
    pub const RETURN: u8 = 0x01;
    pub const GET_UNDEFINED: u8 = 0x02;
    pub const GET_ZERO: u8 = 0x03;
    pub const GET_BYTE: u8 = 0x04;
    pub const GET_NEG_BYTE: u8 = 0x05;
    pub const GET_UNSIGNED_SHORT: u8 = 0x06;
    pub const GET_NEG_UNSIGNED_SHORT: u8 = 0x07;
    pub const GET_INTEGER: u8 = 0x08;
    pub const GET_FLOAT: u8 = 0x09;
    pub const GET_STRING: u8 = 0x0a;
    pub const GET_ISTRING: u8 = 0x0b;
    pub const GET_VECTOR: u8 = 0x0c;
    pub const GET_LEVEL_OBJECT: u8 = 0x0d;
    pub const GET_ANIM_OBJECT: u8 = 0x0e;
    pub const GET_SELF: u8 = 0x0f;
    pub const GET_LEVEL: u8 = 0x10;
    pub const GET_GAME: u8 = 0x11;
    pub const GET_ANIM: u8 = 0x12;
    pub const GET_ANIMATION: u8 = 0x13;
    pub const GET_GAME_REF: u8 = 0x14;
    pub const GET_FUNCTION: u8 = 0x15;
    pub const CREATE_LOCAL_VARIABLE: u8 = 0x16;
    pub const SAFE_CREATE_LOCAL_VARIABLES: u8 = 0x17;
    pub const REMOVE_LOCAL_VARIABLES: u8 = 0x18;
    pub const EVAL_LOCAL_VARIABLE_CACHED: u8 = 0x19;
    pub const EVAL_ARRAY: u8 = 0x1a;
    pub const EVAL_LOCAL_ARRAY_REF_CACHED: u8 = 0x1b;
    pub const EVAL_ARRAY_REF: u8 = 0x1c;
    pub const CLEAR_ARRAY: u8 = 0x1d;
    pub const EMPTY_ARRAY: u8 = 0x1e;
    pub const GET_SELF_OBJECT: u8 = 0x1f;
    pub const EVAL_FIELD_VARIABLE: u8 = 0x20;
    pub const EVAL_FIELD_VARIABLE_REF: u8 = 0x21;
    pub const CLEAR_FIELD_VARIABLE: u8 = 0x22;
    pub const SAFE_SET_VARIABLE_FIELD_CACHED: u8 = 0x23;
    pub const SAFE_SET_WAITTILL_VARIABLE_FIELD_CACHED: u8 = 0x24;
    pub const CLEAR_PARAMS: u8 = 0x25;
    pub const CHECK_CLEAR_PARAMS: u8 = 0x26;
    pub const EVAL_LOCAL_VARIABLE_REF_CACHED: u8 = 0x27;
    pub const SET_VARIABLE_FIELD: u8 = 0x28;
    pub const CALL_BUILTIN: u8 = 0x29;
    pub const CALL_BUILTIN_METHOD: u8 = 0x2a;
    pub const WAIT: u8 = 0x2b;
    pub const WAIT_TILL_FRAME_END: u8 = 0x2c;
    pub const PRE_SCRIPT_CALL: u8 = 0x2d;
    pub const SCRIPT_FUNCTION_CALL: u8 = 0x2e;
    pub const SCRIPT_FUNCTION_CALL_POINTER: u8 = 0x2f;
    pub const SCRIPT_METHOD_CALL: u8 = 0x30;
    pub const SCRIPT_METHOD_CALL_POINTER: u8 = 0x31;
    pub const SCRIPT_THREAD_CALL: u8 = 0x32;
    pub const SCRIPT_THREAD_CALL_POINTER: u8 = 0x33;
    pub const SCRIPT_METHOD_THREAD_CALL: u8 = 0x34;
    pub const SCRIPT_METHOD_THREAD_CALL_POINTER: u8 = 0x35;
    pub const DEC_TOP: u8 = 0x36;
    pub const CAST_FIELD_OBJECT: u8 = 0x37;
    pub const CAST_BOOL: u8 = 0x38;
    pub const BOOL_NOT: u8 = 0x39;
    pub const BOOL_COMPLEMENT: u8 = 0x3a;
    pub const JUMP_ON_FALSE: u8 = 0x3b;
    pub const JUMP_ON_TRUE: u8 = 0x3c;
    pub const JUMP_ON_FALSE_EXPR: u8 = 0x3d;
    pub const JUMP_ON_TRUE_EXPR: u8 = 0x3e;
    pub const JUMP: u8 = 0x3f;
    pub const JUMP_BACK: u8 = 0x40;
    pub const INC: u8 = 0x41;
    pub const DEC: u8 = 0x42;
    pub const BIT_OR: u8 = 0x43;
    pub const BIT_XOR: u8 = 0x44;
    pub const BIT_AND: u8 = 0x45;
    pub const EQUAL: u8 = 0x46;
    pub const NOT_EQUAL: u8 = 0x47;
    pub const LESS_THAN: u8 = 0x48;
    pub const GREATER_THAN: u8 = 0x49;
    pub const LESS_THAN_OR_EQUAL: u8 = 0x4a;
    pub const GREATER_THAN_OR_EQUAL: u8 = 0x4b;
    pub const SHIFT_LEFT: u8 = 0x4c;
    pub const SHIFT_RIGHT: u8 = 0x4d;
    pub const PLUS: u8 = 0x4e;
    pub const MINUS: u8 = 0x4f;
    pub const MULTIPLY: u8 = 0x50;
    pub const DIVIDE: u8 = 0x51;
    pub const MODULUS: u8 = 0x52;
    pub const SIZE_OF: u8 = 0x53;
    pub const WAIT_TILL_MATCH: u8 = 0x54;
    pub const WAIT_TILL: u8 = 0x55;
    pub const NOTIFY: u8 = 0x56;
    pub const END_ON: u8 = 0x57;
    pub const VOID_CODE_POS: u8 = 0x58;
    pub const SWITCH: u8 = 0x59;
    pub const END_SWITCH: u8 = 0x5a;
    pub const VECTOR: u8 = 0x5b;
    pub const GET_HASH: u8 = 0x5c;
    pub const REAL_WAIT: u8 = 0x5d;
    pub const VECTOR_CONSTANT: u8 = 0x5e;
    pub const IS_DEFINED: u8 = 0x5f;
    pub const VECTOR_SCALE: u8 = 0x60;
    pub const ANGLES_TO_UP: u8 = 0x61;
    pub const ANGLES_TO_RIGHT: u8 = 0x62;
    pub const ANGLES_TO_FORWARD: u8 = 0x63;
    pub const ANGLE_CLAMP_180: u8 = 0x64;
    pub const VECTOR_TO_ANGLES: u8 = 0x65;
    pub const ABS: u8 = 0x66;
    pub const GET_TIME: u8 = 0x67;
    pub const GET_DVAR: u8 = 0x68;
    pub const GET_DVAR_INT: u8 = 0x69;
    pub const GET_DVAR_FLOAT: u8 = 0x6a;
    pub const GET_DVAR_VECTOR: u8 = 0x6b;
    pub const GET_DVAR_COLOR_RED: u8 = 0x6c;
    pub const GET_DVAR_COLOR_GREEN: u8 = 0x6d;
    pub const GET_DVAR_COLOR_BLUE: u8 = 0x6e;
    pub const GET_DVAR_COLOR_ALPHA: u8 = 0x6f;
    pub const FIRST_ARRAY_KEY: u8 = 0x70;
    pub const NEXT_ARRAY_KEY: u8 = 0x71;
    pub const PROFILE_START: u8 = 0x72;
    pub const PROFILE_STOP: u8 = 0x73;
    pub const SAFE_DEC_TOP: u8 = 0x74;
    pub const NOP: u8 = 0x75;
    pub const ABORT: u8 = 0x76;
    pub const OBJECT: u8 = 0x77;
    pub const THREAD_OBJECT: u8 = 0x78;
    pub const EVAL_LOCAL_VARIABLE: u8 = 0x79;
    pub const EVAL_LOCAL_VARIABLE_REF: u8 = 0x7a;
    pub const DEVBLOCK_BEGIN: u8 = 0x7b;
    pub const DEVBLOCK_END: u8 = 0x7c;
}

/// Opcode names by value, for listings.
const OP_NAMES: [&str; 125] = [
    "End",
    "Return",
    "GetUndefined",
    "GetZero",
    "GetByte",
    "GetNegByte",
    "GetUnsignedShort",
    "GetNegUnsignedShort",
    "GetInteger",
    "GetFloat",
    "GetString",
    "GetIstring",
    "GetVector",
    "GetLevelObject",
    "GetAnimObject",
    "GetSelf",
    "GetLevel",
    "GetGame",
    "GetAnim",
    "GetAnimation",
    "GetGameRef",
    "GetFunction",
    "CreateLocalVariable",
    "SafeCreateLocalVariables",
    "RemoveLocalVariables",
    "EvalLocalVariableCached",
    "EvalArray",
    "EvalLocalArrayRefCached",
    "EvalArrayRef",
    "ClearArray",
    "EmptyArray",
    "GetSelfObject",
    "EvalFieldVariable",
    "EvalFieldVariableRef",
    "ClearFieldVariable",
    "SafeSetVariableFieldCached",
    "SafeSetWaittillVariableFieldCached",
    "ClearParams",
    "CheckClearParams",
    "EvalLocalVariableRefCached",
    "SetVariableField",
    "CallBuiltin",
    "CallBuiltinMethod",
    "Wait",
    "WaitTillFrameEnd",
    "PreScriptCall",
    "ScriptFunctionCall",
    "ScriptFunctionCallPointer",
    "ScriptMethodCall",
    "ScriptMethodCallPointer",
    "ScriptThreadCall",
    "ScriptThreadCallPointer",
    "ScriptMethodThreadCall",
    "ScriptMethodThreadCallPointer",
    "DecTop",
    "CastFieldObject",
    "CastBool",
    "BoolNot",
    "BoolComplement",
    "JumpOnFalse",
    "JumpOnTrue",
    "JumpOnFalseExpr",
    "JumpOnTrueExpr",
    "Jump",
    "JumpBack",
    "Inc",
    "Dec",
    "BitOr",
    "BitXor",
    "BitAnd",
    "Equal",
    "NotEqual",
    "LessThan",
    "GreaterThan",
    "LessThanOrEqual",
    "GreaterThanOrEqual",
    "ShiftLeft",
    "ShiftRight",
    "Plus",
    "Minus",
    "Multiply",
    "Divide",
    "Modulus",
    "SizeOf",
    "WaitTillMatch",
    "WaitTill",
    "Notify",
    "EndOn",
    "VoidCodePos",
    "Switch",
    "EndSwitch",
    "Vector",
    "GetHash",
    "RealWait",
    "VectorConstant",
    "IsDefined",
    "VectorScale",
    "AnglesToUp",
    "AnglesToRight",
    "AnglesToForward",
    "AngleClamp180",
    "VectorToAngles",
    "Abs",
    "GetTime",
    "GetDvar",
    "GetDvarInt",
    "GetDvarFloat",
    "GetDvarVector",
    "GetDvarColorRed",
    "GetDvarColorGreen",
    "GetDvarColorBlue",
    "GetDvarColorAlpha",
    "FirstArrayKey",
    "NextArrayKey",
    "ProfileStart",
    "ProfileStop",
    "SafeDecTop",
    "Nop",
    "Abort",
    "Object",
    "ThreadObject",
    "EvalLocalVariable",
    "EvalLocalVariableRef",
    "DevblockBegin",
    "DevblockEnd",
];

/// The name of opcode `code` (`?` past the table).
pub fn op_name(code: u8) -> &'static str {
    OP_NAMES.get(usize::from(code)).copied().unwrap_or("?")
}

/// A constant (or not) a script works with.
#[derive(Clone, Debug, PartialEq)]
pub enum GscValue {
    Undefined,
    Int(i32),
    Float(f32),
    Str(String),
    Vec3([f32; 3]),
    /// The result of call `n` (`GscRun::calls[n]`).
    Call(usize),
    Level,
    /// Anything the walk does not follow (self, arrays, arithmetic, ...).
    Other,
}

impl GscValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Self::Int(i) => Some(i as f32),
            Self::Float(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_vec3(&self) -> Option<[f32; 3]> {
        match *self {
            Self::Vec3(v) => Some(v),
            _ => None,
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Int(i) => i.to_string(),
            _ => "?".to_owned(),
        }
    }
}

/// One call a script makes.
#[derive(Clone, Debug, PartialEq)]
pub struct GscCall {
    /// The exported function the call is in.
    pub caller: String,
    /// The called function, `namespace::name` for another script's.
    pub function: String,
    /// Arguments, first first.
    pub args: Vec<GscValue>,
}

impl GscCall {
    /// The called function's name without its script.
    pub fn name(&self) -> &str {
        self.function.rsplit("::").next().unwrap_or(&self.function)
    }
}

/// What an assignment writes into.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GscBase {
    Level,
    /// The object a call returned (`GscRun::calls[n]`).
    Call(usize),
    Other,
}

/// One assignment: `base.path = value` (array keys and fields joined by
/// dots: `level._effect.fx_fire_xsm`, `<call>.v.origin`).
#[derive(Clone, Debug, PartialEq)]
pub struct GscAssign {
    /// The exported function the assignment is in.
    pub caller: String,
    pub base: GscBase,
    pub path: String,
    pub value: GscValue,
}

/// What a walk of every exported function recorded.
#[derive(Clone, Debug, Default)]
pub struct GscRun {
    pub calls: Vec<GscCall>,
    pub assigns: Vec<GscAssign>,
}

impl GscRun {
    /// Calls to a function by its bare name.
    pub fn calls_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (usize, &'a GscCall)> + 'a {
        self.calls
            .iter()
            .enumerate()
            .filter(move |(_, c)| c.name().eq_ignore_ascii_case(name))
    }

    /// The fields written into the object call `n` returned.
    pub fn fields_of(&self, n: usize) -> impl Iterator<Item = (&str, &GscValue)> {
        self.assigns
            .iter()
            .filter(move |a| a.base == GscBase::Call(n))
            .map(|a| (a.path.as_str(), &a.value))
    }

    /// `level.<array>[key] = value` for every key.
    pub fn level_array<'a>(&'a self, array: &'a str) -> impl Iterator<Item = (&'a str, &'a GscValue)> + 'a {
        self.assigns.iter().filter_map(move |a| {
            let key = a.path.strip_prefix(array)?.strip_prefix('.')?;
            (a.base == GscBase::Level).then_some((key, &a.value))
        })
    }
}

/// An export: a function's code address and name.
#[derive(Clone, Debug)]
pub struct GscExport {
    pub address: usize,
    pub name: String,
    pub params: u8,
}

/// One GSC object.
#[derive(Clone, Debug)]
pub struct GscObject {
    bytes: Vec<u8>,
    pub name: String,
    pub includes: Vec<String>,
    pub exports: Vec<GscExport>,
    /// Call site address -> (`namespace::name`, parameter count).
    pub imports: HashMap<usize, (String, u8)>,
    /// Call site address -> the import's flags byte.
    pub import_flags: HashMap<usize, u8>,
    /// Operand address -> string.
    pub strings: HashMap<usize, String>,
    code_end: usize,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("u16 at {o:#x} past the end"))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("u32 at {o:#x} past the end"))
}

fn cstr_at(b: &[u8], o: usize) -> Result<String, String> {
    let tail = b.get(o..).ok_or_else(|| format!("string at {o:#x} past the end"))?;
    let end = tail.iter().position(|&c| c == 0).ok_or("unterminated string")?;
    Ok(tail[..end].iter().map(|&c| char::from(c)).collect())
}

const fn align(p: usize, n: usize) -> usize {
    (p + n - 1) & !(n - 1)
}

/// One decoded instruction.
#[derive(Clone, Debug, PartialEq)]
pub struct GscOp {
    pub at: usize,
    pub op: u8,
    pub arg: GscArg,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GscArg {
    None,
    Int(i32),
    Float(f32),
    Vec3([f32; 3]),
    /// A string operand, with its operand address.
    Str(String, usize),
    /// Local variable names (`SafeCreateLocalVariables`), with their
    /// operand addresses.
    Names(Vec<(String, usize)>),
    /// A call: `namespace::name` (or `local@addr`) and parameter count.
    Call(String, u8),
    Jump(i32),
}

impl GscObject {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..8) != Some(MAGIC.as_slice()) {
            return Err("not a Black Ops II script object".into());
        }
        let b = bytes;
        let include_off = u32_at(b, 0x0c)? as usize;
        let code = u32_at(b, 0x14)? as usize;
        let strings_off = u32_at(b, 0x18)? as usize;
        let exports_off = u32_at(b, 0x1c)? as usize;
        let imports_off = u32_at(b, 0x20)? as usize;
        let code_size = u32_at(b, 0x2c)? as usize;
        let name = cstr_at(b, usize::from(u16_at(b, 0x30)?))?;
        let n_strings = usize::from(u16_at(b, 0x32)?);
        let n_exports = usize::from(u16_at(b, 0x34)?);
        let n_imports = usize::from(u16_at(b, 0x36)?);
        let n_includes = usize::from(*b.get(0x3c).ok_or("short header")?);
        let includes = (0..n_includes)
            .map(|i| cstr_at(b, u32_at(b, include_off + 4 * i)? as usize))
            .collect::<Result<Vec<_>, _>>()?;
        let mut exports = Vec::with_capacity(n_exports);
        for i in 0..n_exports {
            let o = exports_off + 12 * i;
            exports.push(GscExport {
                address: u32_at(b, o + 4)? as usize,
                name: cstr_at(b, usize::from(u16_at(b, o + 8)?))?,
                params: *b.get(o + 10).ok_or("short export")?,
            });
        }
        exports.sort_by_key(|e| e.address);
        let mut imports = HashMap::new();
        let mut import_flags = HashMap::new();
        let mut o = imports_off;
        for _ in 0..n_imports {
            let name = cstr_at(b, usize::from(u16_at(b, o)?))?;
            let ns = usize::from(u16_at(b, o + 2)?);
            let n = usize::from(u16_at(b, o + 4)?);
            let params = *b.get(o + 6).ok_or("short import")?;
            let flags = *b.get(o + 7).ok_or("short import")?;
            o += 8;
            let full = if ns == 0 {
                name
            } else {
                format!("{}::{name}", cstr_at(b, ns)?)
            };
            for k in 0..n {
                let at = u32_at(b, o + 4 * k)? as usize;
                imports.insert(at, (full.clone(), params));
                import_flags.insert(at, flags);
            }
            o += 4 * n;
        }
        // An animtree table (scripts using animations) may sit between the
        // imports and the string references.
        if o > strings_off {
            return Err(format!("import table ends at {o:#x}, past the strings at {strings_off:#x}"));
        }
        let mut o = strings_off;
        let mut strings = HashMap::new();
        for _ in 0..n_strings {
            let s = cstr_at(b, usize::from(u16_at(b, o)?))?;
            let n = usize::from(*b.get(o + 2).ok_or("short string ref")?);
            o += 4;
            for k in 0..n {
                strings.insert(u32_at(b, o + 4 * k)? as usize, s.clone());
            }
            o += 4 * n;
        }
        if o > b.len() {
            return Err(format!("string table ends at {o:#x}, past the end"));
        }
        Ok(Self {
            bytes: bytes.to_vec(),
            name,
            includes,
            exports,
            imports,
            import_flags,
            strings,
            code_end: code + code_size,
        })
    }

    /// The code of export `i`: from its address to the next export's.
    fn function_span(&self, i: usize) -> (usize, usize) {
        let start = self.exports[i].address;
        let end = self
            .exports
            .get(i + 1)
            .map_or(self.code_end, |e| e.address)
            .min(self.bytes.len());
        (start, end)
    }

    /// Decode export `i`. Stops at an unknown opcode (the padding after a
    /// function's last `End`) or at its end.
    pub fn decode(&self, i: usize) -> Vec<GscOp> {
        let (start, end) = self.function_span(i);
        let b = &self.bytes;
        let mut out = Vec::new();
        let mut pc = start;
        let byte = |p: usize| b.get(p).copied();
        while pc < end {
            let at = pc;
            let Some(code) = byte(pc) else { break };
            pc += 1;
            let arg = match code {
                op::GET_BYTE
                | op::EVAL_LOCAL_VARIABLE_CACHED
                | op::EVAL_LOCAL_ARRAY_REF_CACHED
                | op::SAFE_SET_VARIABLE_FIELD_CACHED
                | op::SAFE_SET_WAITTILL_VARIABLE_FIELD_CACHED
                | op::EVAL_LOCAL_VARIABLE_REF_CACHED
                | op::REMOVE_LOCAL_VARIABLES
                | op::VECTOR_CONSTANT
                | op::CREATE_LOCAL_VARIABLE
                | op::WAIT_TILL_MATCH
                | op::SCRIPT_FUNCTION_CALL_POINTER
                | op::SCRIPT_METHOD_CALL_POINTER
                | op::SCRIPT_THREAD_CALL_POINTER
                | op::SCRIPT_METHOD_THREAD_CALL_POINTER => {
                    let Some(v) = byte(pc) else { break };
                    pc += 1;
                    GscArg::Int(i32::from(v))
                }
                op::GET_NEG_BYTE => {
                    let Some(v) = byte(pc) else { break };
                    pc += 1;
                    GscArg::Int(-i32::from(v))
                }
                op::GET_UNSIGNED_SHORT | op::GET_NEG_UNSIGNED_SHORT | op::DEVBLOCK_BEGIN => {
                    pc = align(pc, 2);
                    let Ok(v) = u16_at(b, pc) else { break };
                    pc += 2;
                    let v = i32::from(v);
                    GscArg::Int(if code == op::GET_NEG_UNSIGNED_SHORT { -v } else { v })
                }
                op::GET_INTEGER | op::SWITCH | op::GET_HASH | op::GET_ANIMATION => {
                    pc = align(pc, 4);
                    let Ok(v) = u32_at(b, pc) else { break };
                    pc += 4;
                    GscArg::Int(v as i32)
                }
                op::GET_FLOAT => {
                    pc = align(pc, 4);
                    let Ok(v) = u32_at(b, pc) else { break };
                    pc += 4;
                    GscArg::Float(f32::from_bits(v))
                }
                op::GET_VECTOR => {
                    pc = align(pc, 4);
                    let mut v = [0.0; 3];
                    for (k, c) in v.iter_mut().enumerate() {
                        let Ok(x) = u32_at(b, pc + 4 * k) else { break };
                        *c = f32::from_bits(x);
                    }
                    pc += 12;
                    GscArg::Vec3(v)
                }
                op::GET_STRING
                | op::GET_ISTRING
                | op::EVAL_FIELD_VARIABLE
                | op::EVAL_FIELD_VARIABLE_REF
                | op::CLEAR_FIELD_VARIABLE => {
                    pc = align(pc, 2);
                    let s = self.strings.get(&pc).cloned().unwrap_or_default();
                    let a = pc;
                    pc += 2;
                    GscArg::Str(s, a)
                }
                op::SAFE_CREATE_LOCAL_VARIABLES => {
                    let Some(n) = byte(pc) else { break };
                    pc += 1;
                    let mut names = Vec::with_capacity(usize::from(n));
                    for _ in 0..n {
                        pc = align(pc, 2);
                        names.push((self.strings.get(&pc).cloned().unwrap_or_default(), pc));
                        pc += 2;
                    }
                    GscArg::Names(names)
                }
                op::GET_FUNCTION => {
                    pc = align(pc, 4);
                    pc += 4;
                    let name = self.imports.get(&at).map_or_else(String::new, |i| i.0.clone());
                    GscArg::Call(name, 0)
                }
                op::CALL_BUILTIN
                | op::CALL_BUILTIN_METHOD
                | op::SCRIPT_FUNCTION_CALL
                | op::SCRIPT_METHOD_CALL
                | op::SCRIPT_THREAD_CALL
                | op::SCRIPT_METHOD_THREAD_CALL => {
                    pc += 1;
                    pc = align(pc, 4);
                    let Ok(target) = u32_at(b, pc) else { break };
                    pc += 4;
                    match self.imports.get(&at) {
                        Some((name, params)) => GscArg::Call(name.clone(), *params),
                        None => GscArg::Call(format!("local@{target:#x}"), 0),
                    }
                }
                op::JUMP_ON_FALSE
                | op::JUMP_ON_TRUE
                | op::JUMP_ON_FALSE_EXPR
                | op::JUMP_ON_TRUE_EXPR
                | op::JUMP
                | op::JUMP_BACK => {
                    pc = align(pc, 2);
                    let Ok(v) = u16_at(b, pc) else { break };
                    pc += 2;
                    GscArg::Jump(i32::from(v as i16))
                }
                op::END_SWITCH => {
                    pc = align(pc, 4);
                    let Ok(n) = u32_at(b, pc) else { break };
                    pc += 4 + 8 * n as usize;
                    GscArg::Int(n as i32)
                }
                c if c > op::DEVBLOCK_END => break,
                _ => GscArg::None,
            };
            out.push(GscOp { at, op: code, arg });
        }
        out
    }

    /// (string references read, string references, import call sites
    /// read, import call sites) over every export: the decoder's check.
    pub fn coverage(&self) -> (usize, usize, usize, usize) {
        let mut strings = std::collections::HashSet::new();
        let mut imports = std::collections::HashSet::new();
        for i in 0..self.exports.len() {
            for o in self.decode(i) {
                match &o.arg {
                    GscArg::Str(_, a) if self.strings.contains_key(a) => {
                        strings.insert(*a);
                    }
                    GscArg::Names(names) => {
                        for (_, a) in names {
                            if self.strings.contains_key(a) {
                                strings.insert(*a);
                            }
                        }
                    }
                    GscArg::Call(..) if self.imports.contains_key(&o.at) => {
                        imports.insert(o.at);
                    }
                    _ => {}
                }
            }
        }
        (strings.len(), self.strings.len(), imports.len(), self.imports.len())
    }

    /// Walk every export straight through; record calls and assignments.
    pub fn run(&self) -> GscRun {
        let mut run = GscRun::default();
        for i in 0..self.exports.len() {
            Walk::new(&self.exports[i].name).walk(&self.decode(i), &mut run);
        }
        run
    }
}

/// The value stack entry for a call's arguments boundary.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    Value(GscValue),
    Mark,
}

#[derive(Clone, Debug)]
struct Ref {
    base: RefBase,
    path: Vec<String>,
}

#[derive(Clone, Debug)]
enum RefBase {
    Local(usize),
    Value(GscValue),
}

struct Walk<'a> {
    caller: &'a str,
    stack: Vec<Slot>,
    locals: Vec<GscValue>,
    field_object: GscValue,
    target: Option<Ref>,
}

impl<'a> Walk<'a> {
    fn new(caller: &'a str) -> Self {
        Self {
            caller,
            stack: Vec::new(),
            locals: Vec::new(),
            field_object: GscValue::Other,
            target: None,
        }
    }

    fn push(&mut self, v: GscValue) {
        self.stack.push(Slot::Value(v));
    }

    fn pop(&mut self) -> GscValue {
        match self.stack.pop() {
            Some(Slot::Value(v)) => v,
            Some(Slot::Mark) => {
                self.stack.push(Slot::Mark);
                GscValue::Other
            }
            None => GscValue::Other,
        }
    }

    fn pop_n(&mut self, n: usize) {
        for _ in 0..n {
            self.pop();
        }
    }

    /// Locals are cached in creation order; index 0 is the newest.
    fn local(&self, n: usize) -> GscValue {
        self.locals
            .len()
            .checked_sub(n + 1)
            .and_then(|i| self.locals.get(i))
            .cloned()
            .unwrap_or(GscValue::Other)
    }

    fn set_local(&mut self, n: usize, v: GscValue) {
        if let Some(i) = self.locals.len().checked_sub(n + 1)
            && let Some(slot) = self.locals.get_mut(i)
        {
            *slot = v;
        }
    }

    fn assign(&mut self, run: &mut GscRun, value: GscValue) {
        let Some(target) = self.target.take() else {
            return;
        };
        let (base, path) = match target.base {
            RefBase::Local(n) if target.path.is_empty() => {
                self.set_local(n, value);
                return;
            }
            RefBase::Local(n) => (self.local(n), target.path),
            RefBase::Value(v) => (v, target.path),
        };
        let base = match base {
            GscValue::Level => GscBase::Level,
            GscValue::Call(n) => GscBase::Call(n),
            _ => GscBase::Other,
        };
        run.assigns.push(GscAssign {
            caller: self.caller.to_owned(),
            base,
            path: path.join("."),
            value,
        });
    }

    fn call(&mut self, run: &mut GscRun, name: &str, params: u8, method: bool) -> GscValue {
        if method {
            self.pop();
        }
        let mut args = Vec::new();
        while args.len() < usize::from(params) {
            match self.stack.last() {
                Some(Slot::Value(_)) => args.push(self.pop()),
                _ => break,
            }
        }
        if self.stack.last() == Some(&Slot::Mark) {
            self.stack.pop();
        }
        run.calls.push(GscCall {
            caller: self.caller.to_owned(),
            function: name.to_owned(),
            args,
        });
        GscValue::Call(run.calls.len() - 1)
    }

    fn walk(mut self, ops: &[GscOp], run: &mut GscRun) {
        for o in ops {
            let int = |a: &GscArg| match *a {
                GscArg::Int(i) => i,
                _ => 0,
            };
            match o.op {
                op::END | op::RETURN => {
                    self.stack.clear();
                    self.target = None;
                }
                op::GET_UNDEFINED => self.push(GscValue::Undefined),
                op::GET_ZERO => self.push(GscValue::Int(0)),
                op::GET_BYTE
                | op::GET_NEG_BYTE
                | op::GET_UNSIGNED_SHORT
                | op::GET_NEG_UNSIGNED_SHORT
                | op::GET_INTEGER => self.push(GscValue::Int(int(&o.arg))),
                op::GET_FLOAT => {
                    if let GscArg::Float(f) = o.arg {
                        self.push(GscValue::Float(f));
                    }
                }
                op::GET_STRING | op::GET_ISTRING => {
                    if let GscArg::Str(s, _) = &o.arg {
                        self.push(GscValue::Str(s.clone()));
                    }
                }
                op::GET_VECTOR => {
                    if let GscArg::Vec3(v) = o.arg {
                        self.push(GscValue::Vec3(v));
                    }
                }
                op::GET_LEVEL => self.push(GscValue::Level),
                op::GET_SELF
                | op::GET_GAME
                | op::GET_ANIM
                | op::GET_ANIMATION
                | op::GET_FUNCTION
                | op::GET_HASH
                | op::EMPTY_ARRAY
                | op::GET_TIME
                | op::VECTOR_CONSTANT => self.push(GscValue::Other),
                op::GET_LEVEL_OBJECT => self.field_object = GscValue::Level,
                op::GET_SELF_OBJECT | op::GET_ANIM_OBJECT => self.field_object = GscValue::Other,
                op::GET_GAME_REF => {
                    self.target = Some(Ref {
                        base: RefBase::Value(GscValue::Other),
                        path: Vec::new(),
                    });
                }
                op::CREATE_LOCAL_VARIABLE => self.locals.push(GscValue::Undefined),
                op::SAFE_CREATE_LOCAL_VARIABLES => {
                    if let GscArg::Names(names) = &o.arg {
                        self.locals = vec![GscValue::Undefined; names.len()];
                    }
                }
                op::EVAL_LOCAL_VARIABLE_CACHED => {
                    let v = self.local(int(&o.arg) as usize);
                    self.push(v);
                }
                op::EVAL_LOCAL_VARIABLE_REF_CACHED | op::EVAL_LOCAL_ARRAY_REF_CACHED => {
                    self.target = Some(Ref {
                        base: RefBase::Local(int(&o.arg) as usize),
                        path: Vec::new(),
                    });
                }
                op::SAFE_SET_VARIABLE_FIELD_CACHED => {
                    let v = self.pop();
                    self.set_local(int(&o.arg) as usize, v);
                }
                op::SAFE_SET_WAITTILL_VARIABLE_FIELD_CACHED => {
                    self.set_local(int(&o.arg) as usize, GscValue::Other);
                }
                op::EVAL_ARRAY => {
                    self.pop();
                    self.pop();
                    self.push(GscValue::Other);
                }
                op::EVAL_ARRAY_REF => {
                    let key = self.pop().key();
                    if let Some(t) = self.target.as_mut() {
                        t.path.push(key);
                    }
                }
                op::CLEAR_ARRAY => {
                    self.pop();
                    self.target = None;
                }
                op::CAST_FIELD_OBJECT => self.field_object = self.pop(),
                op::EVAL_FIELD_VARIABLE => self.push(GscValue::Other),
                op::EVAL_FIELD_VARIABLE_REF => {
                    if let GscArg::Str(field, _) = &o.arg {
                        self.target = Some(Ref {
                            base: RefBase::Value(self.field_object.clone()),
                            path: vec![field.clone()],
                        });
                    }
                }
                op::CLEAR_FIELD_VARIABLE => {}
                op::SET_VARIABLE_FIELD => {
                    let v = self.pop();
                    self.assign(run, v);
                }
                op::PRE_SCRIPT_CALL => self.stack.push(Slot::Mark),
                op::CALL_BUILTIN
                | op::SCRIPT_FUNCTION_CALL
                | op::SCRIPT_THREAD_CALL
                | op::CALL_BUILTIN_METHOD
                | op::SCRIPT_METHOD_CALL
                | op::SCRIPT_METHOD_THREAD_CALL => {
                    if let GscArg::Call(name, params) = &o.arg {
                        let method = matches!(
                            o.op,
                            op::CALL_BUILTIN_METHOD | op::SCRIPT_METHOD_CALL | op::SCRIPT_METHOD_THREAD_CALL
                        );
                        let v = self.call(run, name, *params, method);
                        self.push(v);
                    }
                }
                op::SCRIPT_FUNCTION_CALL_POINTER
                | op::SCRIPT_THREAD_CALL_POINTER
                | op::SCRIPT_METHOD_CALL_POINTER
                | op::SCRIPT_METHOD_THREAD_CALL_POINTER => {
                    self.pop();
                    while let Some(Slot::Value(_)) = self.stack.last() {
                        self.stack.pop();
                    }
                    if self.stack.last() == Some(&Slot::Mark) {
                        self.stack.pop();
                    }
                    self.push(GscValue::Other);
                }
                op::DEC_TOP | op::SAFE_DEC_TOP | op::WAIT | op::REAL_WAIT | op::SWITCH => {
                    self.pop();
                }
                op::JUMP_ON_FALSE | op::JUMP_ON_TRUE | op::JUMP_ON_FALSE_EXPR | op::JUMP_ON_TRUE_EXPR => {
                    self.pop();
                }
                op::CAST_BOOL
                | op::BOOL_NOT
                | op::BOOL_COMPLEMENT
                | op::SIZE_OF
                | op::IS_DEFINED
                | op::ANGLES_TO_UP
                | op::ANGLES_TO_RIGHT
                | op::ANGLES_TO_FORWARD
                | op::ANGLE_CLAMP_180
                | op::VECTOR_TO_ANGLES
                | op::ABS
                | op::GET_DVAR
                | op::GET_DVAR_INT
                | op::GET_DVAR_FLOAT
                | op::GET_DVAR_VECTOR
                | op::GET_DVAR_COLOR_RED
                | op::GET_DVAR_COLOR_GREEN
                | op::GET_DVAR_COLOR_BLUE
                | op::GET_DVAR_COLOR_ALPHA
                | op::FIRST_ARRAY_KEY => {
                    self.pop();
                    self.push(GscValue::Other);
                }
                op::BIT_OR
                | op::BIT_XOR
                | op::BIT_AND
                | op::EQUAL
                | op::NOT_EQUAL
                | op::LESS_THAN
                | op::GREATER_THAN
                | op::LESS_THAN_OR_EQUAL
                | op::GREATER_THAN_OR_EQUAL
                | op::SHIFT_LEFT
                | op::SHIFT_RIGHT
                | op::PLUS
                | op::MINUS
                | op::MULTIPLY
                | op::DIVIDE
                | op::MODULUS
                | op::VECTOR_SCALE
                | op::NEXT_ARRAY_KEY => {
                    self.pop();
                    self.pop();
                    self.push(GscValue::Other);
                }
                op::VECTOR => {
                    // Components are pushed last first: x is on top.
                    let x = self.pop().as_f32();
                    let y = self.pop().as_f32();
                    let z = self.pop().as_f32();
                    match (x, y, z) {
                        (Some(x), Some(y), Some(z)) => self.push(GscValue::Vec3([x, y, z])),
                        _ => self.push(GscValue::Other),
                    }
                }
                op::WAIT_TILL | op::END_ON => self.pop_n(2),
                op::WAIT_TILL_MATCH => self.pop_n(2),
                op::NOTIFY => {
                    self.pop_n(2);
                    while let Some(Slot::Value(_)) = self.stack.last() {
                        self.stack.pop();
                    }
                    if self.stack.last() == Some(&Slot::Mark) {
                        self.stack.pop();
                    }
                }
                op::INC | op::DEC => self.target = None,
                _ => {}
            }
        }
    }
}

/// A placed effect from a map's createfx script.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedFx {
    /// `oneshotfx`, `loopfx`, `exploder` (the create function's kind).
    pub kind: String,
    /// The effect table key (`level._effect[fxid]`).
    pub fxid: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// Seconds: negative = already running that long (pre-roll); for a
    /// loop, its repeat interval.
    pub delay: f32,
    /// An exploder's id: it plays only when the game fires it.
    pub exploder: Option<String>,
}

/// What a Black Ops II map's scripts say about its effects and ambience.
#[derive(Clone, Debug, Default)]
pub struct MapScriptFacts {
    /// `level._effect[key] = loadfx(path)`.
    pub effects: HashMap<String, String>,
    /// The createfx script's placed effects.
    pub placed: Vec<PlacedFx>,
    /// `snd_play_auto_fx(fxid, alias, x, y, z, ...)`: a loop on every placed
    /// effect with that id, offset by (x, y, z).
    pub sounds_on_fx: Vec<(String, String, [f32; 3])>,
    /// `playloopat(alias, origin)`.
    pub loops_at: Vec<(String, [f32; 3])>,
    /// The default ambient room's tone (`declareambientroom(room, true)`,
    /// `setambientroomtone(room, alias, ...)`).
    pub room_tone: Option<String>,
    /// Sounds a loop plays now and then at one of a few places
    /// (`wait randomintrange(a, b); playsound(0, alias, place[...])`):
    /// alias, wait range in seconds, places.
    pub random_sounds: Vec<(String, [f32; 2], Vec<[f32; 3]>)>,
}

impl MapScriptFacts {
    /// Read a map's scripts. `script` finds a script object's bytes by name
    /// (`clientscripts/mp/createfx/<map>_fx.csc`, ...).
    pub fn read(map: &str, script: impl Fn(&str) -> Option<Vec<u8>>) -> Self {
        let load = |name: String| -> Option<GscRun> {
            let bytes = script(&name)?;
            GscObject::parse(&bytes).ok().map(|g| g.run())
        };
        let mut facts = Self::default();
        if let Some(run) = load(format!("clientscripts/mp/{map}_fx.csc")) {
            for (key, value) in run.level_array("_effect") {
                if let GscValue::Call(n) = value
                    && let Some(call) = run.calls.get(*n)
                    && call.name().eq_ignore_ascii_case("loadfx")
                    && let Some(path) = call.args.first().and_then(GscValue::as_str)
                {
                    facts.effects.insert(key.to_owned(), path.to_owned());
                }
            }
        }
        if let Some(run) = load(format!("clientscripts/mp/createfx/{map}_fx.csc")) {
            for (n, call) in run.calls.iter().enumerate() {
                let kind = match call.name() {
                    "createoneshoteffect" => "oneshotfx",
                    "createloopeffect" => "loopfx",
                    "createexploder" => "exploder",
                    _ => continue,
                };
                let mut fx = PlacedFx {
                    kind: kind.to_owned(),
                    fxid: call.args.first().and_then(GscValue::as_str).unwrap_or("").to_owned(),
                    origin: [0.0; 3],
                    angles: [0.0; 3],
                    delay: 0.0,
                    exploder: None,
                };
                for (field, value) in run.fields_of(n) {
                    match field {
                        "v.origin" => fx.origin = value.as_vec3().unwrap_or(fx.origin),
                        "v.angles" => fx.angles = value.as_vec3().unwrap_or(fx.angles),
                        "v.delay" => fx.delay = value.as_f32().unwrap_or(fx.delay),
                        "v.fxid" => {
                            if let Some(id) = value.as_str() {
                                fx.fxid = id.to_owned();
                            }
                        }
                        "v.exploder" => {
                            fx.exploder = match value {
                                GscValue::Str(s) => Some(s.clone()),
                                GscValue::Int(i) => Some(i.to_string()),
                                _ => Some("?".to_owned()),
                            }
                        }
                        _ => {}
                    }
                }
                facts.placed.push(fx);
            }
        }
        if let Some(run) = load(format!("clientscripts/mp/{map}_amb.csc")) {
            let mut default_room = None;
            for (_, call) in run.calls_named("declareambientroom") {
                let is_default = call.args.get(1).and_then(GscValue::as_f32).is_some_and(|v| v != 0.0);
                if is_default {
                    default_room = call.args.first().and_then(GscValue::as_str).map(str::to_owned);
                }
            }
            for (_, call) in run.calls_named("setambientroomtone") {
                if call.args.first().and_then(GscValue::as_str) == default_room.as_deref()
                    && let Some(alias) = call.args.get(1).and_then(GscValue::as_str)
                {
                    facts.room_tone = Some(alias.to_owned());
                }
            }
            for (_, call) in run.calls_named("snd_play_auto_fx") {
                let (Some(fxid), Some(alias)) = (
                    call.args.first().and_then(GscValue::as_str),
                    call.args.get(1).and_then(GscValue::as_str),
                ) else {
                    continue;
                };
                let off = [2, 3, 4].map(|i| call.args.get(i).and_then(GscValue::as_f32).unwrap_or(0.0));
                facts.sounds_on_fx.push((fxid.to_owned(), alias.to_owned(), off));
            }
            for (_, call) in run.calls_named("playsound") {
                let Some(alias) = call.args.get(1).and_then(GscValue::as_str) else {
                    continue;
                };
                let places: Vec<[f32; 3]> = run
                    .assigns
                    .iter()
                    .filter(|a| a.caller == call.caller)
                    .filter_map(|a| a.value.as_vec3())
                    .collect();
                let wait = run
                    .calls_named("randomintrange")
                    .find(|(_, c)| c.caller == call.caller)
                    .map(|(_, c)| {
                        let at = |i: usize| c.args.get(i).and_then(GscValue::as_f32).unwrap_or(0.0);
                        [at(0), at(1)]
                    });
                if let Some(wait) = wait
                    && !places.is_empty()
                {
                    facts.random_sounds.push((alias.to_owned(), wait, places));
                }
            }
            for (_, call) in run.calls_named("playloopat") {
                if let (Some(alias), Some(origin)) = (
                    call.args.first().and_then(GscValue::as_str),
                    call.args.get(1).and_then(GscValue::as_vec3),
                ) {
                    facts.loops_at.push((alias.to_owned(), origin));
                }
            }
        }
        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny object: one export `main` calling `f("a", (1, 2, 3))` and
    /// storing the result's `v["k"] = 5`.
    fn tiny() -> Vec<u8> {
        let mut b = vec![0u8; 0x40];
        b[..8].copy_from_slice(MAGIC);
        let mut strings = Vec::new();
        let mut push_str = |b: &mut Vec<u8>, s: &str| {
            let at = b.len();
            b.extend_from_slice(s.as_bytes());
            b.push(0);
            strings.push(at);
            at
        };
        let name = push_str(&mut b, "t.csc");
        let main = push_str(&mut b, "main");
        let f = push_str(&mut b, "f");
        let a = push_str(&mut b, "a");
        let v = push_str(&mut b, "v");
        let k = push_str(&mut b, "k");
        while b.len() % 4 != 0 {
            b.push(0);
        }
        let code = b.len();
        // SafeCreateLocalVariables 1 name (code is 4-aligned: the name
        // operand lands 2-aligned right after the count)
        b.extend_from_slice(&[op::SAFE_CREATE_LOCAL_VARIABLES, 1]);
        b.extend_from_slice(&(v as u16).to_le_bytes());
        // PreScriptCall; GetVector (1,2,3)
        b.push(op::PRE_SCRIPT_CALL);
        b.push(op::GET_VECTOR);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        for x in [1.0f32, 2.0, 3.0] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        // GetString "a"
        b.push(op::GET_STRING);
        if b.len() % 2 != 0 {
            b.push(0);
        }
        let a_ref = b.len();
        b.extend_from_slice(&(a as u16).to_le_bytes());
        // ScriptFunctionCall f
        let call_at = b.len();
        b.extend_from_slice(&[op::SCRIPT_FUNCTION_CALL, 0]);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        // SafeSetVariableFieldCached 0
        b.extend_from_slice(&[op::SAFE_SET_VARIABLE_FIELD_CACHED, 0]);
        // GetByte 5; GetString "k"; EvalLocalVariableCached 0; CastFieldObject;
        // EvalFieldVariableRef "v"; EvalArrayRef; SetVariableField; End
        b.extend_from_slice(&[op::GET_BYTE, 5, op::GET_STRING]);
        if b.len() % 2 != 0 {
            b.push(0);
        }
        let k_ref = b.len();
        b.extend_from_slice(&(k as u16).to_le_bytes());
        b.extend_from_slice(&[op::EVAL_LOCAL_VARIABLE_CACHED, 0, op::CAST_FIELD_OBJECT, op::EVAL_FIELD_VARIABLE_REF]);
        if b.len() % 2 != 0 {
            b.push(0);
        }
        let v_ref = b.len();
        b.extend_from_slice(&(v as u16).to_le_bytes());
        b.extend_from_slice(&[op::EVAL_ARRAY_REF, op::SET_VARIABLE_FIELD, op::END]);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        let code_size = b.len() - code;
        let exports = b.len();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&(code as u32).to_le_bytes());
        b.extend_from_slice(&(main as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        let imports = b.len();
        b.extend_from_slice(&(f as u16).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&[2, 2]);
        b.extend_from_slice(&(call_at as u32).to_le_bytes());
        let string_refs = b.len();
        for (s, at) in [(a, a_ref), (k, k_ref), (v, v_ref)] {
            b.extend_from_slice(&(s as u16).to_le_bytes());
            b.extend_from_slice(&[1, 0]);
            b.extend_from_slice(&(at as u32).to_le_bytes());
        }
        let fixups = b.len();
        let put32 = |b: &mut Vec<u8>, o: usize, v: usize| b[o..o + 4].copy_from_slice(&(v as u32).to_le_bytes());
        put32(&mut b, 0x0c, code);
        put32(&mut b, 0x14, code);
        put32(&mut b, 0x18, string_refs);
        put32(&mut b, 0x1c, exports);
        put32(&mut b, 0x20, imports);
        put32(&mut b, 0x24, fixups);
        put32(&mut b, 0x28, fixups);
        put32(&mut b, 0x2c, code_size);
        b[0x30..0x32].copy_from_slice(&(name as u16).to_le_bytes());
        b[0x32..0x34].copy_from_slice(&3u16.to_le_bytes());
        b[0x34..0x36].copy_from_slice(&1u16.to_le_bytes());
        b[0x36..0x38].copy_from_slice(&1u16.to_le_bytes());
        b
    }

    #[test]
    fn decodes_and_records_calls_and_fields() {
        let g = GscObject::parse(&tiny()).expect("parse");
        assert_eq!(g.name, "t.csc");
        assert_eq!(g.coverage(), (3, 3, 1, 1));
        let run = g.run();
        assert_eq!(run.calls.len(), 1);
        assert_eq!(run.calls[0].function, "f");
        assert_eq!(
            run.calls[0].args,
            vec![GscValue::Str("a".into()), GscValue::Vec3([1.0, 2.0, 3.0])]
        );
        let fields: Vec<_> = run.fields_of(0).collect();
        assert_eq!(fields, vec![("v.k", &GscValue::Int(5))]);
    }
}
