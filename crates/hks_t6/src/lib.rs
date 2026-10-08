//! bo2zm M4: Black Ops II's UI scripts are HavokScript, a Lua 5.1
//! derivative, compiled and stored as rawfiles (`ui/t6/*.lua`). This crate
//! reads that bytecode; it runs nothing.
//!
//! Layout, measured on all 296 of his UI scripts (each read exactly to its
//! last byte):
//!
//! ```text
//! header   1B 4C 75 61  "\x1bLua"
//!          51           Lua 5.1
//!          0D           HavokScript's format
//!          01           little-endian
//!          04 04 04 04  int, size_t, instruction, number: 4 bytes
//!          00           numbers are floats (32-bit)
//!          00 00        two flag bytes (0 in every file)
//! types    u32 count, then {u32 id, u32 length, name with NUL}:
//!          TNIL TBOOLEAN TLIGHTUSERDATA TNUMBER TSTRING TTABLE TFUNCTION
//!          TUSERDATA TTHREAD TIFUNCTION TCFUNCTION TUI64 TSTRUCT
//! function u32 upvalues, u32 parameters, u8 vararg, u32 stack size,
//!          u32 instruction count, `_` (5F) padding to a 4-byte offset,
//!          the instructions, u32 constant count, each {u8 type, value},
//!          u32 debug flag (1), u32 name hash, u32 child count, children
//! ```
//!
//! An instruction is A (bits 0-7), C (8-16), B (17-24), opcode (25-31);
//! Bx = C and B together, sBx biased by 0xFFFF. The empty script is one
//! `RETURN 0 1` (0x12020000).

use core::fmt;

pub mod aar;
pub mod host;
pub mod lui;
pub mod mp;
pub mod pattern;
pub mod playlists;
pub mod stdlib;
pub mod value;
pub mod vm;

/// A constant.
#[derive(Clone, Debug, PartialEq)]
pub enum Const {
    Nil,
    Bool(bool),
    Number(f32),
    /// The bytes, without the trailing NUL.
    String(Vec<u8>),
}

impl fmt::Display for Const {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Const::Nil => f.write_str("nil"),
            Const::Bool(b) => write!(f, "{b}"),
            Const::Number(n) => write!(f, "{n}"),
            Const::String(s) => write!(f, "{:?}", String::from_utf8_lossy(s)),
        }
    }
}

/// One compiled function.
#[derive(Clone, Debug, Default)]
pub struct Proto {
    pub upvalues: u32,
    pub params: u32,
    pub vararg: u8,
    pub stack: u32,
    pub code: Vec<u32>,
    pub consts: Vec<Const>,
    /// The script's hash (every function in a script shares it).
    pub name_hash: u32,
    /// This function's number in its script, depth first (the main chunk
    /// is 0): with the hash, what an error trace names (`hash/index:pc`).
    pub index: u32,
    pub protos: Vec<std::rc::Rc<Proto>>,
}

impl Proto {
    /// This function and every one inside it.
    pub fn count(&self) -> usize {
        1 + self.protos.iter().map(|p| p.count()).sum::<usize>()
    }
}

/// A whole compiled script.
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    /// (type id, type name) as the file lists them.
    pub types: Vec<(u32, String)>,
    pub main: Proto,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not "\x1bLua" 5.1 in HavokScript's format.
    Header,
    /// Ran out of bytes at this offset.
    Short(usize),
    /// A padding byte that is not `_`.
    Padding(usize),
    /// A constant of an unknown type.
    ConstType(u8, usize),
    /// The debug flag is not 1.
    Debug(u32, usize),
    /// Bytes left after the main function.
    Trailing(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Header => f.write_str("not a HavokScript 5.1 chunk"),
            Error::Short(at) => write!(f, "ends early at {at}"),
            Error::Padding(at) => write!(f, "padding is not '_' at {at}"),
            Error::ConstType(t, at) => write!(f, "constant type {t} at {at}"),
            Error::Debug(v, at) => write!(f, "debug flag {v} at {at}"),
            Error::Trailing(n) => write!(f, "{n} bytes after the main function"),
        }
    }
}

impl std::error::Error for Error {}

const HEADER: [u8; 12] = [0x1B, b'L', b'u', b'a', 0x51, 0x0D, 0x01, 4, 4, 4, 4, 0];

struct Reader<'a> {
    b: &'a [u8],
    o: usize,
    /// The next function's number in the script (depth first).
    next: u32,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        let s = self.b.get(self.o..self.o + n).ok_or(Error::Short(self.o))?;
        self.o += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let s = self.bytes(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn proto(&mut self) -> Result<Proto, Error> {
        let index = self.next;
        self.next += 1;
        let upvalues = self.u32()?;
        let params = self.u32()?;
        let vararg = self.u8()?;
        let stack = self.u32()?;
        let ncode = self.u32()? as usize;
        while !self.o.is_multiple_of(4) {
            let at = self.o;
            if self.u8()? != 0x5F {
                return Err(Error::Padding(at));
            }
        }
        let mut code = Vec::with_capacity(ncode.min(1 << 16));
        for _ in 0..ncode {
            code.push(self.u32()?);
        }
        let nk = self.u32()? as usize;
        let mut consts = Vec::with_capacity(nk.min(1 << 16));
        for _ in 0..nk {
            let at = self.o;
            consts.push(match self.u8()? {
                0 => Const::Nil,
                1 => Const::Bool(self.u8()? != 0),
                3 => Const::Number(self.f32()?),
                4 => {
                    let n = self.u32()? as usize;
                    let s = self.bytes(n)?;
                    Const::String(s.strip_suffix(&[0]).unwrap_or(s).to_vec())
                }
                t => return Err(Error::ConstType(t, at)),
            });
        }
        let at = self.o;
        let debug = self.u32()?;
        if debug != 1 {
            return Err(Error::Debug(debug, at));
        }
        let name_hash = self.u32()?;
        let nchild = self.u32()? as usize;
        let mut protos = Vec::with_capacity(nchild.min(1 << 12));
        for _ in 0..nchild {
            protos.push(std::rc::Rc::new(self.proto()?));
        }
        Ok(Proto {
            upvalues,
            params,
            vararg,
            stack,
            code,
            consts,
            name_hash,
            index,
            protos,
        })
    }
}

/// Read a compiled UI script.
pub fn parse(bytes: &[u8]) -> Result<Chunk, Error> {
    if bytes.get(..12) != Some(&HEADER[..]) {
        return Err(Error::Header);
    }
    let mut r = Reader { b: bytes, o: 14, next: 0 };
    let n = r.u32()? as usize;
    let mut types = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        let id = r.u32()?;
        let len = r.u32()? as usize;
        let name = r.bytes(len)?;
        let name = name.strip_suffix(&[0]).unwrap_or(name);
        types.push((id, String::from_utf8_lossy(name).into_owned()));
    }
    let main = r.proto()?;
    if r.o != bytes.len() {
        return Err(Error::Trailing(bytes.len() - r.o));
    }
    Ok(Chunk { types, main })
}

/// HavokScript's opcodes in its enum order, as Black Ops II's UI scripts use
/// them (0-76 occur). Names to be proven by the VM on real scripts; measured
/// so far: RETURN is 9 (the empty script), GETFIELD (0), GETTABLE_S (10),
/// GETFIELD_R1 (73) and CLOSURE (66) are followed by DATA (76) words.
pub const OP_NAMES: [&str; 77] = [
    "GETFIELD",
    "TEST",
    "CALL_I",
    "CALL_C",
    "EQ",
    "EQ_BK",
    "GETGLOBAL",
    "MOVE",
    "SELF",
    "RETURN",
    "GETTABLE_S",
    "GETTABLE_N",
    "GETTABLE",
    "LOADBOOL",
    "TFORLOOP",
    "SETFIELD",
    "SETTABLE_S",
    "SETTABLE_S_BK",
    "SETTABLE_N",
    "SETTABLE_N_BK",
    "SETTABLE",
    "SETTABLE_BK",
    "TAILCALL_I",
    "TAILCALL_C",
    "TAILCALL_M",
    "LOADK",
    "LOADNIL",
    "SETGLOBAL",
    "JMP",
    "CALL_M",
    "CALL",
    "INTRINSIC_INDEX",
    "INTRINSIC_NEWINDEX",
    "INTRINSIC_SELF",
    "INTRINSIC_LITERAL",
    "INTRINSIC_NEWINDEX_LITERAL",
    "INTRINSIC_SELF_LITERAL",
    "TAILCALL",
    "GETUPVAL",
    "SETUPVAL",
    "ADD",
    "ADD_BK",
    "SUB",
    "SUB_BK",
    "MUL",
    "MUL_BK",
    "DIV",
    "DIV_BK",
    "MOD",
    "MOD_BK",
    "POW",
    "POW_BK",
    "NEWTABLE",
    "UNM",
    "NOT",
    "LEN",
    "LT",
    "LT_BK",
    "LE",
    "LE_BK",
    "CONCAT",
    "TESTSET",
    "FORPREP",
    "FORLOOP",
    "SETLIST",
    "CLOSE",
    "CLOSURE",
    "VARARG",
    "TAILCALL_I_R1",
    "CALL_I_R1",
    "SETUPVAL_R1",
    "TEST_R1",
    "NOT_R1",
    "GETFIELD_R1",
    "SETFIELD_R1",
    "NEWSTRUCT",
    "DATA",
];

/// An instruction's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ins {
    pub op: u8,
    pub a: u8,
    pub b: u16,
    pub c: u16,
    pub bx: u32,
    pub sbx: i32,
}

pub fn decode(word: u32) -> Ins {
    let bx = (word >> 8) & 0x1_FFFF;
    Ins {
        op: (word >> 25) as u8,
        a: (word & 0xFF) as u8,
        b: ((word >> 17) & 0xFF) as u16,
        c: ((word >> 8) & 0x1FF) as u16,
        bx,
        sbx: bx as i32 - 0xFFFF,
    }
}

pub fn op_name(op: u8) -> &'static str {
    OP_NAMES.get(op as usize).copied().unwrap_or("?")
}

/// A function's listing: one line per instruction (index, name, A B C, and
/// the constant a Bx or C names when it is one).
pub fn disassemble(p: &Proto, out: &mut String) {
    disassemble_one(p, out);
    for child in &p.protos {
        disassemble(child, out);
    }
}

/// One function's listing, without the functions inside it.
pub fn disassemble_one(p: &Proto, out: &mut String) {
    use fmt::Write;
    let _ = writeln!(
        out,
        "function {:#010x}/{}: {} params, {} upvalues, vararg {}, stack {}, {} constants",
        p.name_hash,
        p.index,
        p.params,
        p.upvalues,
        p.vararg,
        p.stack,
        p.consts.len()
    );
    let k = |i: usize| p.consts.get(i).map_or_else(|| "?".to_owned(), ToString::to_string);
    let rk = |i: u16| {
        if i >= 256 {
            k(usize::from(i - 256))
        } else {
            format!("r{i}")
        }
    };
    for (i, &w) in p.code.iter().enumerate() {
        let d = decode(w);
        let (a, b, c) = (d.a, d.b, d.c);
        let note = match op_name(d.op) {
            "LOADK" | "GETGLOBAL" | "SETGLOBAL" => k(d.bx as usize),
            "GETFIELD" | "GETFIELD_R1" => format!("r{a} = r{b}.{}", k(usize::from(c))),
            "SETFIELD" | "SETFIELD_R1" => format!("r{a}.{} = {}", k(usize::from(b)), rk(c)),
            "SELF" => format!("r{a} = r{b}:{}", rk(c)),
            "GETTABLE" | "GETTABLE_S" | "GETTABLE_N" => format!("r{a} = r{b}[{}]", rk(c)),
            "SETTABLE" | "SETTABLE_S" | "SETTABLE_N" => format!("r{a}[r{b}] = {}", rk(c)),
            "SETTABLE_BK" | "SETTABLE_S_BK" | "SETTABLE_N_BK" => format!("r{a}[{}] = {}", k(usize::from(b)), rk(c)),
            "EQ" | "LT" | "LE" => format!("r{b} vs {}", rk(c)),
            "EQ_BK" | "LT_BK" | "LE_BK" => format!("{} vs {}", k(usize::from(b)), rk(c)),
            "JMP" | "FORPREP" | "FORLOOP" => format!("-> {}", i as i64 + 1 + i64::from(d.sbx)),
            "CALL" | "CALL_I" | "CALL_C" | "CALL_M" | "CALL_I_R1" => format!("r{a}({} args) -> {}", i32::from(b) - 1, i32::from(c) - 1),
            _ => String::new(),
        };
        let _ = write!(out, "  {i:4} {:<14} {:3} {:3} {:3}", op_name(d.op), d.a, d.b, d.c);
        if !note.is_empty() {
            let _ = write!(out, "  ; {note}");
        }
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_script_returns() {
        let d = decode(0x1202_0000);
        assert_eq!((op_name(d.op), d.a, d.b, d.c), ("RETURN", 0, 1, 0));
    }

    #[test]
    fn rejects_stock_lua() {
        let mut b = HEADER.to_vec();
        b[5] = 0;
        assert_eq!(parse(&b).unwrap_err(), Error::Header);
    }
}
