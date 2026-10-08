//! A Black Ops II (PC) compiled script object, read and decoded.
//!
//! A zone keeps each script as a `ScriptParseTree` buffer: a GSC object
//! (`\x80GSC\r\n\0\x06`) with a header, include, animation tree, export,
//! import and string reference tables and the code segment. Each exported
//! function is decoded by following its control flow from its address
//! (jumps, switch tables, the skip over developer blocks), so the alignment
//! padding between functions is never read as code.
//!
//! Layouts (measured on the PC zones, 2026-10-01):
//! - header: crc 0x08, include table 0x0c, animtree table 0x10, code 0x14,
//!   string fixups 0x18, exports 0x1c, imports 0x20, code size 0x2c, name
//!   0x30 (u16 offset), counts u16 at 0x32 (strings) 0x34 (exports) 0x36
//!   (imports), u8 at 0x3c (includes) 0x3d (animtrees).
//! - export: crc u32, address u32, name u16, params u8, flags u8.
//! - import: name u16, namespace u16, count u16, params u8, flags u8, then
//!   `count` call-site addresses (u32). Flags low nibble: 1 function
//!   reference, 2 call, 3 thread, 4 method, 5 method thread; 0x10 developer.
//! - string fixup: string u16, count u8, type u8, then `count` addresses.
//! - animtree: name u16, tree refs u16, anim refs u16, pad u16, then tree
//!   ref addresses (u32; a `GetInteger -1` operand), then anim refs
//!   (name offset u32, operand address u32; a `GetAnimation` operand).
//! - switch table (the `Switch` operand is relative to its own end): count
//!   u32, then (value u32, offset i32) per case, offset relative to the end
//!   of the case. A string case has a string fixup at its value + 2; an int
//!   case is `0x800000 + n`; the default case is 0 and comes last.

use rustc_hash::FxHashMap;

pub const MAGIC: &[u8; 8] = b"\x80GSC\r\n\x00\x06";

/// Opcodes by value (Black Ops II, PC).
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
    "GetIString",
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

/// An import record: who a call site names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// The namespace (a script path such as `maps/mp/zombies/_zm_utility`),
    /// or empty.
    pub ns: String,
    pub name: String,
    pub params: u8,
    pub flags: u8,
}

impl Import {
    pub fn full(&self) -> String {
        if self.ns.is_empty() {
            self.name.clone()
        } else {
            format!("{}::{}", self.ns, self.name)
        }
    }

    /// Imported only by developer code.
    pub fn developer(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaseValue {
    Int(i32),
    Str(String),
    Default,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub value: CaseValue,
    /// Code address.
    pub target: u32,
}

/// A decoded instruction (addresses are byte offsets in the object).
#[derive(Clone, Debug, PartialEq)]
pub enum Insn {
    /// An opcode with no operand.
    Plain(u8),
    /// An opcode with an integer operand: the constant for the `Get*` integer
    /// opcodes (sign applied), the local index for local-variable opcodes,
    /// the count for `WaitTillMatch` and pointer calls, the flags for
    /// `VectorConstant`.
    Int(u8, i32),
    Float(f32),
    Vector([f32; 3]),
    /// GetString / GetIString / EvalFieldVariable(Ref) / ClearFieldVariable.
    Str(u8, String),
    /// SafeCreateLocalVariables: the local names, parameters first.
    Locals(Vec<String>),
    Hash(u32),
    /// A call or a function reference (GetFunction).
    Call(u8, Import),
    /// A jump to a code address.
    Jump(u8, u32),
    /// A switch: its cases and the address just past its table.
    Switch {
        cases: Vec<Case>,
        end: u32,
    },
    /// The end of a switch reached by falling through its last case.
    EndSwitch {
        end: u32,
    },
    /// `%anim`.
    Anim {
        tree: String,
        anim: String,
    },
    /// `#animtree`.
    AnimTree(String),
    /// A developer block's start: retail code skips to `to`.
    DevBlock {
        to: u32,
    },
}

#[derive(Clone, Debug)]
pub struct DecodedFunction {
    pub name: String,
    pub params: u8,
    pub flags: u8,
    pub address: u32,
    /// Reachable instructions in address order.
    pub code: Vec<(u32, Insn)>,
}

#[derive(Clone, Debug)]
pub struct ScriptObject {
    pub name: String,
    pub includes: Vec<String>,
    pub functions: Vec<DecodedFunction>,
    /// Number of string references the fixup table lists, and how many the
    /// decoder read (a check: they should match).
    pub string_refs: (usize, usize),
    /// Import call sites the table lists, and how many the decoder read.
    pub import_sites: (usize, usize),
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
    let tail = b
        .get(o..)
        .ok_or_else(|| format!("string at {o:#x} past the end"))?;
    let end = tail
        .iter()
        .position(|&c| c == 0)
        .ok_or("unterminated string")?;
    Ok(tail[..end].iter().map(|&c| char::from(c)).collect())
}

const fn align(p: usize, n: usize) -> usize {
    (p + n - 1) & !(n - 1)
}

struct Tables {
    imports: FxHashMap<usize, Import>,
    strings: FxHashMap<usize, String>,
    trees: FxHashMap<usize, String>,
    anims: FxHashMap<usize, (String, String)>,
    code_start: usize,
    code_end: usize,
}

impl ScriptObject {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..8) != Some(MAGIC.as_slice()) {
            return Err("not a Black Ops II script object".into());
        }
        let b = bytes;
        let include_off = u32_at(b, 0x0c)? as usize;
        let animtree_off = u32_at(b, 0x10)? as usize;
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
        let n_trees = usize::from(*b.get(0x3d).ok_or("short header")?);
        let includes = (0..n_includes)
            .map(|i| cstr_at(b, u32_at(b, include_off + 4 * i)? as usize))
            .collect::<Result<Vec<_>, _>>()?;

        let mut imports = FxHashMap::default();
        let mut o = imports_off;
        for _ in 0..n_imports {
            let name = cstr_at(b, usize::from(u16_at(b, o)?))?;
            let ns = cstr_at(b, usize::from(u16_at(b, o + 2)?))?;
            let n = usize::from(u16_at(b, o + 4)?);
            let params = *b.get(o + 6).ok_or("short import")?;
            let flags = *b.get(o + 7).ok_or("short import")?;
            o += 8;
            let imp = Import {
                ns,
                name,
                params,
                flags,
            };
            for k in 0..n {
                imports.insert(u32_at(b, o + 4 * k)? as usize, imp.clone());
            }
            o += 4 * n;
        }
        let mut strings = FxHashMap::default();
        let mut o = strings_off;
        for _ in 0..n_strings {
            let s = cstr_at(b, usize::from(u16_at(b, o)?))?;
            let n = usize::from(*b.get(o + 2).ok_or("short string ref")?);
            o += 4;
            for k in 0..n {
                strings.insert(u32_at(b, o + 4 * k)? as usize, s.clone());
            }
            o += 4 * n;
        }
        let mut trees = FxHashMap::default();
        let mut anims = FxHashMap::default();
        let mut o = animtree_off;
        for _ in 0..n_trees {
            let tree = cstr_at(b, usize::from(u16_at(b, o)?))?;
            let nt = usize::from(u16_at(b, o + 2)?);
            let na = usize::from(u16_at(b, o + 4)?);
            o += 8;
            for k in 0..nt {
                trees.insert(u32_at(b, o + 4 * k)? as usize, tree.clone());
            }
            o += 4 * nt;
            for k in 0..na {
                let anim = cstr_at(b, u32_at(b, o + 8 * k)? as usize)?;
                anims.insert(u32_at(b, o + 8 * k + 4)? as usize, (tree.clone(), anim));
            }
            o += 8 * na;
        }
        let tables = Tables {
            imports,
            strings,
            trees,
            anims,
            code_start: code,
            code_end: (code + code_size).min(b.len()),
        };

        let mut functions = Vec::with_capacity(n_exports);
        for i in 0..n_exports {
            let o = exports_off + 12 * i;
            let address = u32_at(b, o + 4)?;
            let fname = cstr_at(b, usize::from(u16_at(b, o + 8)?))?;
            let params = *b.get(o + 10).ok_or("short export")?;
            let flags = *b.get(o + 11).ok_or("short export")?;
            let code = decode_function(b, &tables, address as usize)
                .map_err(|e| format!("{name}::{fname}: {e}"))?;
            functions.push(DecodedFunction {
                name: fname,
                params,
                flags,
                address,
                code,
            });
        }
        functions.sort_by_key(|f| f.address);

        // The decoder's check: every string and import site the tables list
        // read by some instruction (switch case strings sit in tables).
        let mut seen_strings = 0usize;
        let mut seen_imports = 0usize;
        let string_sites: usize = tables.strings.len();
        let import_sites: usize = tables.imports.len();
        {
            let mut s = rustc_hash::FxHashSet::default();
            let mut im = rustc_hash::FxHashSet::default();
            for f in &functions {
                for (at, insn) in &f.code {
                    let at = *at as usize;
                    match insn {
                        Insn::Str(..) => {
                            s.insert(align(at + 1, 2));
                        }
                        Insn::Locals(names) => {
                            let mut p = at + 2;
                            for _ in names {
                                p = align(p, 2);
                                s.insert(p);
                                p += 2;
                            }
                        }
                        Insn::Call(..) => {
                            im.insert(at);
                        }
                        Insn::Switch { .. } => {}
                        _ => {}
                    }
                }
            }
            // Switch case strings: counted from the tables directly.
            for f in &functions {
                for (at, insn) in &f.code {
                    if let Insn::Switch { cases, .. } = insn {
                        let table = switch_table_addr(b, *at as usize)?;
                        for k in 0..cases.len() {
                            let a = table + 4 + 8 * k + 2;
                            if tables.strings.contains_key(&a) {
                                s.insert(a);
                            }
                        }
                    }
                }
            }
            seen_strings += s.iter().filter(|a| tables.strings.contains_key(a)).count();
            seen_imports += im.iter().filter(|a| tables.imports.contains_key(a)).count();
        }

        Ok(Self {
            name,
            includes,
            functions,
            string_refs: (string_sites, seen_strings),
            import_sites: (import_sites, seen_imports),
        })
    }
}

fn switch_table_addr(b: &[u8], at: usize) -> Result<usize, String> {
    let p = align(at + 1, 4);
    let rel = u32_at(b, p)? as i32;
    Ok((p as i64 + 4 + i64::from(rel)) as usize)
}

/// Decode one instruction at `pc`: (instruction, next pc, extra successors).
fn decode_one(b: &[u8], t: &Tables, pc: usize) -> Result<(Insn, usize, Vec<u32>), String> {
    let at = pc;
    let code = *b.get(pc).ok_or("code past the end")?;
    let mut pc = pc + 1;
    let byte = |p: usize| -> Result<u8, String> {
        b.get(p)
            .copied()
            .ok_or_else(|| "operand past the end".to_owned())
    };
    let mut succ = Vec::new();
    let insn = match code {
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
            let v = byte(pc)?;
            pc += 1;
            Insn::Int(code, i32::from(v))
        }
        op::GET_NEG_BYTE => {
            let v = byte(pc)?;
            pc += 1;
            Insn::Int(op::GET_BYTE, -i32::from(v))
        }
        op::GET_UNSIGNED_SHORT | op::GET_NEG_UNSIGNED_SHORT => {
            pc = align(pc, 2);
            let v = i32::from(u16_at(b, pc)?);
            pc += 2;
            Insn::Int(
                op::GET_INTEGER,
                if code == op::GET_NEG_UNSIGNED_SHORT {
                    -v
                } else {
                    v
                },
            )
        }
        op::DEVBLOCK_BEGIN => {
            pc = align(pc, 2);
            let v = i32::from(u16_at(b, pc)? as i16);
            pc += 2;
            let to = (pc as i64 + i64::from(v)) as u32;
            succ.push(to);
            Insn::DevBlock { to }
        }
        op::GET_INTEGER => {
            pc = align(pc, 4);
            let v = u32_at(b, pc)? as i32;
            let operand = pc;
            pc += 4;
            match t.trees.get(&operand) {
                Some(tree) => Insn::AnimTree(tree.clone()),
                None => Insn::Int(op::GET_INTEGER, v),
            }
        }
        op::GET_HASH => {
            pc = align(pc, 4);
            let v = u32_at(b, pc)?;
            pc += 4;
            Insn::Hash(v)
        }
        op::GET_ANIMATION => {
            pc = align(pc, 4);
            let operand = pc;
            pc += 4;
            let (tree, anim) = t
                .anims
                .get(&operand)
                .cloned()
                .ok_or_else(|| format!("animation at {at:#x} not in the animtree table"))?;
            Insn::Anim { tree, anim }
        }
        op::GET_FLOAT => {
            pc = align(pc, 4);
            let v = f32::from_bits(u32_at(b, pc)?);
            pc += 4;
            Insn::Float(v)
        }
        op::GET_VECTOR => {
            pc = align(pc, 4);
            let v = [
                f32::from_bits(u32_at(b, pc)?),
                f32::from_bits(u32_at(b, pc + 4)?),
                f32::from_bits(u32_at(b, pc + 8)?),
            ];
            pc += 12;
            Insn::Vector(v)
        }
        op::GET_STRING
        | op::GET_ISTRING
        | op::EVAL_FIELD_VARIABLE
        | op::EVAL_FIELD_VARIABLE_REF
        | op::CLEAR_FIELD_VARIABLE => {
            pc = align(pc, 2);
            let s = t
                .strings
                .get(&pc)
                .cloned()
                .ok_or_else(|| format!("{} at {at:#x} has no string", op_name(code)))?;
            pc += 2;
            Insn::Str(code, s)
        }
        op::SAFE_CREATE_LOCAL_VARIABLES => {
            let n = byte(pc)?;
            pc += 1;
            let mut names = Vec::with_capacity(usize::from(n));
            for _ in 0..n {
                pc = align(pc, 2);
                names.push(t.strings.get(&pc).cloned().unwrap_or_default());
                pc += 2;
            }
            Insn::Locals(names)
        }
        op::GET_FUNCTION => {
            pc = align(pc, 4);
            pc += 4;
            let imp = t
                .imports
                .get(&at)
                .cloned()
                .ok_or_else(|| format!("function reference at {at:#x} has no import"))?;
            Insn::Call(code, imp)
        }
        op::CALL_BUILTIN
        | op::CALL_BUILTIN_METHOD
        | op::SCRIPT_FUNCTION_CALL
        | op::SCRIPT_METHOD_CALL
        | op::SCRIPT_THREAD_CALL
        | op::SCRIPT_METHOD_THREAD_CALL => {
            pc += 1;
            pc = align(pc, 4);
            pc += 4;
            let imp = t
                .imports
                .get(&at)
                .cloned()
                .ok_or_else(|| format!("call at {at:#x} has no import"))?;
            Insn::Call(code, imp)
        }
        op::JUMP_ON_FALSE
        | op::JUMP_ON_TRUE
        | op::JUMP_ON_FALSE_EXPR
        | op::JUMP_ON_TRUE_EXPR
        | op::JUMP
        | op::JUMP_BACK => {
            pc = align(pc, 2);
            let v = i32::from(u16_at(b, pc)? as i16);
            pc += 2;
            let to = (pc as i64 + i64::from(v)) as u32;
            succ.push(to);
            Insn::Jump(code, to)
        }
        op::SWITCH => {
            pc = align(pc, 4);
            let rel = u32_at(b, pc)? as i32;
            pc += 4;
            let table = (pc as i64 + i64::from(rel)) as usize;
            let n = u32_at(b, table)? as usize;
            let mut cases = Vec::with_capacity(n);
            for k in 0..n {
                let a = table + 4 + 8 * k;
                let v = u32_at(b, a)?;
                let off = u32_at(b, a + 4)? as i32;
                let target = (a as i64 + 8 + i64::from(off)) as u32;
                let value = if let Some(s) = t.strings.get(&(a + 2)) {
                    CaseValue::Str(s.clone())
                } else if v == 0 {
                    CaseValue::Default
                } else {
                    CaseValue::Int(v as i32 - 0x80_0000)
                };
                succ.push(target);
                cases.push(Case { value, target });
            }
            let end = (table + 4 + 8 * n) as u32;
            if !cases.iter().any(|c| c.value == CaseValue::Default) {
                succ.push(end);
            }
            Insn::Switch { cases, end }
        }
        op::END_SWITCH => {
            pc = align(pc, 4);
            let n = u32_at(b, pc)? as usize;
            pc += 4 + 8 * n;
            Insn::EndSwitch { end: pc as u32 }
        }
        c if c > op::DEVBLOCK_END => return Err(format!("unknown opcode {c:#04x} at {at:#x}")),
        _ => Insn::Plain(code),
    };
    Ok((insn, pc, succ))
}

/// Decode a function by following its control flow from `start`.
fn decode_function(b: &[u8], t: &Tables, start: usize) -> Result<Vec<(u32, Insn)>, String> {
    let mut seen: std::collections::BTreeMap<u32, Insn> = std::collections::BTreeMap::new();
    let mut work = vec![start as u32];
    while let Some(mut pc) = work.pop() {
        loop {
            if seen.contains_key(&pc) {
                break;
            }
            if (pc as usize) < t.code_start || (pc as usize) >= t.code_end {
                return Err(format!("code address {pc:#x} outside the code segment"));
            }
            let (insn, next, succ) = decode_one(b, t, pc as usize)?;
            work.extend(succ);
            let stop = match &insn {
                Insn::Plain(c) => matches!(*c, op::END | op::RETURN | op::ABORT),
                Insn::Jump(c, _) => matches!(*c, op::JUMP | op::JUMP_BACK),
                Insn::Switch { .. } => true,
                _ => false,
            };
            seen.insert(pc, insn);
            if stop {
                break;
            }
            pc = next as u32;
        }
    }
    // Every jump must land on an instruction.
    for insn in seen.values() {
        let targets: Vec<u32> = match insn {
            Insn::Jump(_, to) | Insn::DevBlock { to } => vec![*to],
            Insn::Switch { cases, end } => cases.iter().map(|c| c.target).chain([*end]).collect(),
            Insn::EndSwitch { end } => vec![*end],
            _ => Vec::new(),
        };
        for to in targets {
            if !seen.contains_key(&to) {
                // A switch end with a default case, or an EndSwitch end, may
                // be the function's last byte with nothing after it.
                if matches!(insn, Insn::Switch { .. } | Insn::EndSwitch { .. }) {
                    continue;
                }
                return Err(format!("jump to {to:#x} is not an instruction"));
            }
        }
    }
    Ok(seen.into_iter().collect())
}
