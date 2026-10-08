//! bo2mp: GSC source compiled into the decoded form a zone's scripts read as.
//!
//! Black Ops II's PC zones lack a few scripts the game's own scripts call
//! (multiplayer's `maps/mp/gametypes/_globallogic`); the engine provides
//! them. [`compile`] turns GSC source text into a [`ScriptObject`] that links
//! and runs beside BO2's own, with the same waits, threads, notifies and
//! endons. The code follows BO2's own code generation, read off the zones'
//! compiled scripts: arguments pushed last first, a call's value always left
//! on the stack (`DecTop` after a call statement), locals counted from the
//! last, keys pushed before the reference they index, `foreach` over the
//! first/next array-key opcodes, `waittill` variables taken off the notify's
//! values then `ClearParams`.
//!
//! The language is GSC as BO2's scripts use it: functions, `#include`,
//! developer blocks (`/# ... #/`, skipped), `if`/`else`, `while`, `for`,
//! `foreach`, `switch`, `break`, `continue`, `return`, `wait`,
//! `waittillframeend`, `waittill`, `waittillmatch`, `notify`, `endon`,
//! `thread`, method calls, `[[ pointer ]]` calls, `::function` references,
//! `&"LOCALIZED"`, `#"hashed"`, vectors and the usual operators. No
//! animations (`%anim`, `#using_animtree`).

use crate::object::{Case, CaseValue, DecodedFunction, Import, Insn, ScriptObject, op};

/// Compile one script's source. `name` is its path (`maps/mp/gametypes/_globallogic`).
pub fn compile(name: &str, source: &str) -> Result<ScriptObject, String> {
    let tokens = lex(source).map_err(|e| format!("{name}: {e}"))?;
    let mut p = Parser { t: tokens, pos: 0 };
    let mut includes = Vec::new();
    let mut functions = Vec::new();
    let mut address = 0u32;
    while !p.at_end() {
        if p.eat_punct("#") {
            let word = p.ident()?;
            match word.as_str() {
                "include" => {
                    includes.push(p.path()?);
                    p.expect(";")?;
                }
                other => return Err(p.err(&format!("unsupported directive #{other}"))),
            }
            continue;
        }
        let f = p.function().map_err(|e| format!("{name}: {e}"))?;
        let mut g = Gen::new(&f.params);
        g.block(&f.body)
            .map_err(|e| format!("{name}::{}: {e}", f.name))?;
        let (code, next) = g.finish(address);
        functions.push(DecodedFunction {
            name: f.name,
            params: f.params.len() as u8,
            flags: 0,
            address,
            code,
        });
        address = next;
    }
    Ok(ScriptObject {
        name: format!("{name}.gsc"),
        includes,
        functions,
        string_refs: (0, 0),
        import_sites: (0, 0),
    })
}

// ---- lexer ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(i32),
    Float(f32),
    Str(String),
    IStr(String),
    Hash(String),
    Punct(&'static str),
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: u32,
}

/// Longest first.
const PUNCTS: [&str; 46] = [
    "<<=", ">>=", "::", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=",
    "%=", "|=", "&=", "^=", "<<", ">>", "[[", "]]", "(", ")", "{", "}", "[", "]", ",", ";", ".",
    ":", "=", "+", "-", "*", "/", "%", "<", ">", "!", "~", "&", "|", "^",
];

fn lex(src: &str) -> Result<Vec<Token>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut line = 1u32;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if src[i..].starts_with("/*") {
            let end = src[i + 2..].find("*/").ok_or("unterminated comment")?;
            line += src[i..i + 2 + end].matches('\n').count() as u32;
            i += end + 4;
            continue;
        }
        // Developer blocks: retail builds skip them.
        if src[i..].starts_with("/#") {
            let end = src[i + 2..].find("#/").ok_or("unterminated /# block")?;
            line += src[i..i + 2 + end].matches('\n').count() as u32;
            i += end + 4;
            continue;
        }
        if c == b'"' || ((c == b'&' || c == b'#') && b.get(i + 1) == Some(&b'"')) {
            let start = if c == b'"' { i + 1 } else { i + 2 };
            let mut j = start;
            let mut s = String::new();
            while j < b.len() && b[j] != b'"' {
                if b[j] == b'\\' && j + 1 < b.len() {
                    j += 1;
                    s.push(match b[j] {
                        b'n' => '\n',
                        b't' => '\t',
                        other => other as char,
                    });
                } else {
                    s.push(b[j] as char);
                }
                j += 1;
            }
            if j >= b.len() {
                return Err(format!("line {line}: unterminated string"));
            }
            let tok = match c {
                b'&' => Tok::IStr(s),
                b'#' => Tok::Hash(s),
                _ => Tok::Str(s),
            };
            out.push(Token { tok, line });
            i = j + 1;
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let mut j = i;
            let mut float = false;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                float |= b[j] == b'.';
                j += 1;
            }
            let text = &src[i..j];
            let tok = if float {
                Tok::Float(
                    text.parse()
                        .map_err(|_| format!("line {line}: bad number {text}"))?,
                )
            } else {
                Tok::Int(
                    text.parse()
                        .map_err(|_| format!("line {line}: bad number {text}"))?,
                )
            };
            out.push(Token { tok, line });
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            out.push(Token {
                tok: Tok::Ident(src[i..j].to_ascii_lowercase()),
                line,
            });
            i = j;
            continue;
        }
        if c == b'#' {
            out.push(Token {
                tok: Tok::Punct("#"),
                line,
            });
            i += 1;
            continue;
        }
        match PUNCTS.iter().find(|p| src[i..].starts_with(**p)) {
            Some(p) => {
                out.push(Token {
                    tok: Tok::Punct(p),
                    line,
                });
                i += p.len();
            }
            None => return Err(format!("line {line}: unexpected '{}'", c as char)),
        }
    }
    Ok(out)
}

// ---- syntax --------------------------------------------------------------

#[derive(Clone, Debug)]
enum Expr {
    Int(i32),
    Float(f32),
    Str(String),
    IStr(String),
    Hash(String),
    Undefined,
    Level,
    SelfE,
    Game,
    Anim,
    EmptyArray,
    Vector(Box<[Expr; 3]>),
    Local(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Size(Box<Expr>),
    FuncRef(String, String),
    Call {
        obj: Option<Box<Expr>>,
        callee: Callee,
        args: Vec<Expr>,
        thread: bool,
    },
    Not(Box<Expr>),
    Complement(Box<Expr>),
    Neg(Box<Expr>),
    Bin(u8, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug)]
enum Callee {
    Named(String, String),
    Pointer(Box<Expr>),
}

#[derive(Clone, Debug)]
enum Stmt {
    Expr(Expr),
    Assign(Expr, Expr),
    Compound(u8, Expr, Expr),
    Inc(Expr),
    Dec(Expr),
    Wait(Expr),
    FrameEnd,
    Waittill(Expr, Expr, Vec<String>),
    WaittillMatch(Expr, Expr, Expr),
    Notify(Expr, Expr, Vec<Expr>),
    Endon(Expr, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Option<Expr>, Vec<Stmt>),
    For(Vec<Stmt>, Option<Expr>, Vec<Stmt>, Vec<Stmt>),
    Foreach(Option<String>, String, Expr, Vec<Stmt>),
    Switch(Expr, Vec<(Vec<CaseValue>, Vec<Stmt>)>),
    Break,
    Continue,
    Return(Option<Expr>),
}

struct FunctionDef {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
}

struct Parser {
    t: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn at_end(&self) -> bool {
        self.pos >= self.t.len()
    }

    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.pos).map(|t| &t.tok)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.t.get(self.pos + k).map(|t| &t.tok)
    }

    fn err(&self, msg: &str) -> String {
        let line = self.t.get(self.pos).or(self.t.last()).map_or(0, |t| t.line);
        format!("line {line}: {msg} (at {:?})", self.peek())
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::Punct(q)) if *q == p)
    }

    fn is_punct_at(&self, k: usize, p: &str) -> bool {
        matches!(self.peek_at(k), Some(Tok::Punct(q)) if *q == p)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == w)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.is_punct(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.is_word(w) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Result<(), String> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(self.err(&format!("expected '{p}'")))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            _ => Err(self.err("expected a name")),
        }
    }

    /// A script path: `maps/mp/gametypes/_hud_util` (our scripts write `/`).
    fn path(&mut self) -> Result<String, String> {
        let mut s = self.ident()?;
        while self.eat_punct("/") {
            s.push('/');
            s.push_str(&self.ident()?);
        }
        Ok(s)
    }

    fn function(&mut self) -> Result<FunctionDef, String> {
        let name = self.ident()?;
        self.expect("(")?;
        let mut params = Vec::new();
        if !self.eat_punct(")") {
            loop {
                params.push(self.ident()?);
                if self.eat_punct(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        let body = self.block()?;
        Ok(FunctionDef { name, params, body })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect("{")?;
        let mut out = Vec::new();
        while !self.eat_punct("}") {
            if self.at_end() {
                return Err(self.err("unterminated block"));
            }
            out.push(self.statement()?);
        }
        Ok(out)
    }

    /// A statement or a `{ block }`, as a list.
    fn body(&mut self) -> Result<Vec<Stmt>, String> {
        if self.is_punct("{") {
            self.block()
        } else {
            Ok(vec![self.statement()?])
        }
    }

    fn statement(&mut self) -> Result<Stmt, String> {
        if self.is_punct("{") {
            // A bare block: run as an always-true if.
            let b = self.block()?;
            return Ok(Stmt::If(Expr::Int(1), b, Vec::new()));
        }
        if self.eat_word("if") {
            self.expect("(")?;
            let c = self.expr()?;
            self.expect(")")?;
            let then = self.body()?;
            let other = if self.eat_word("else") {
                self.body()?
            } else {
                Vec::new()
            };
            return Ok(Stmt::If(c, then, other));
        }
        if self.eat_word("while") {
            self.expect("(")?;
            let c = self.expr()?;
            self.expect(")")?;
            let b = self.body()?;
            let c = match c {
                Expr::Int(n) if n != 0 => None,
                c => Some(c),
            };
            return Ok(Stmt::While(c, b));
        }
        if self.eat_word("for") {
            self.expect("(")?;
            let init = if self.eat_punct(";") {
                Vec::new()
            } else {
                let s = self.simple()?;
                self.expect(";")?;
                vec![s]
            };
            let cond = if self.is_punct(";") {
                None
            } else {
                Some(self.expr()?)
            };
            self.expect(";")?;
            let step = if self.is_punct(")") {
                Vec::new()
            } else {
                vec![self.simple()?]
            };
            self.expect(")")?;
            let b = self.body()?;
            return Ok(Stmt::For(init, cond, step, b));
        }
        if self.eat_word("foreach") {
            self.expect("(")?;
            let first = self.ident()?;
            let (key, value) = if self.eat_punct(",") {
                (Some(first), self.ident()?)
            } else {
                (None, first)
            };
            if !self.eat_word("in") {
                return Err(self.err("expected 'in'"));
            }
            let arr = self.expr()?;
            self.expect(")")?;
            let b = self.body()?;
            return Ok(Stmt::Foreach(key, value, arr, b));
        }
        if self.eat_word("switch") {
            self.expect("(")?;
            let v = self.expr()?;
            self.expect(")")?;
            self.expect("{")?;
            let mut arms: Vec<(Vec<CaseValue>, Vec<Stmt>)> = Vec::new();
            while !self.eat_punct("}") {
                let mut labels = Vec::new();
                loop {
                    if self.eat_word("case") {
                        let neg = self.eat_punct("-");
                        let label = match self.peek().cloned() {
                            Some(Tok::Int(n)) => CaseValue::Int(if neg { -n } else { n }),
                            Some(Tok::Str(s)) => CaseValue::Str(s),
                            _ => return Err(self.err("expected a case value")),
                        };
                        self.pos += 1;
                        self.expect(":")?;
                        labels.push(label);
                    } else if self.eat_word("default") {
                        self.expect(":")?;
                        labels.push(CaseValue::Default);
                    } else {
                        break;
                    }
                }
                if labels.is_empty() {
                    return Err(self.err("expected 'case' or 'default'"));
                }
                let mut stmts = Vec::new();
                while !self.is_word("case") && !self.is_word("default") && !self.is_punct("}") {
                    stmts.push(self.statement()?);
                }
                arms.push((labels, stmts));
            }
            return Ok(Stmt::Switch(v, arms));
        }
        if self.eat_word("break") {
            self.expect(";")?;
            return Ok(Stmt::Break);
        }
        if self.eat_word("continue") {
            self.expect(";")?;
            return Ok(Stmt::Continue);
        }
        if self.eat_word("return") {
            if self.eat_punct(";") {
                return Ok(Stmt::Return(None));
            }
            let e = self.expr()?;
            self.expect(";")?;
            return Ok(Stmt::Return(Some(e)));
        }
        if self.eat_word("wait") {
            let e = self.expr()?;
            self.expect(";")?;
            return Ok(Stmt::Wait(e));
        }
        if self.eat_word("waittillframeend") {
            self.expect(";")?;
            return Ok(Stmt::FrameEnd);
        }
        let s = self.simple()?;
        self.expect(";")?;
        Ok(s)
    }

    /// An expression statement, assignment, `++`/`--`, or an object's
    /// `waittill` / `waittillmatch` / `notify` / `endon`.
    fn simple(&mut self) -> Result<Stmt, String> {
        // `self waittill(...)`: an object then the keyword.
        let keyword_at = |p: &Parser, k: usize| -> Option<String> {
            match p.peek_at(k) {
                Some(Tok::Ident(w))
                    if matches!(
                        w.as_str(),
                        "waittill" | "waittillmatch" | "notify" | "endon"
                    ) && p.is_punct_at(k + 1, "(") =>
                {
                    Some(w.clone())
                }
                _ => None,
            }
        };
        let lhs = self.postfix()?;
        if let Some(word) = keyword_at(self, 0) {
            self.pos += 2;
            let name = self.expr()?;
            let mut rest = Vec::new();
            while self.eat_punct(",") {
                rest.push(self.expr()?);
            }
            self.expect(")")?;
            return match word.as_str() {
                "waittill" => {
                    let vars = rest
                        .into_iter()
                        .map(|e| match e {
                            Expr::Local(n) => Ok(n),
                            _ => Err(self.err("waittill takes variable names")),
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Stmt::Waittill(lhs, name, vars))
                }
                "waittillmatch" => {
                    let v = rest
                        .into_iter()
                        .next()
                        .ok_or_else(|| self.err("waittillmatch needs a value"))?;
                    Ok(Stmt::WaittillMatch(lhs, name, v))
                }
                "notify" => Ok(Stmt::Notify(lhs, name, rest)),
                _ => Ok(Stmt::Endon(lhs, name)),
            };
        }
        if self.eat_punct("=") {
            let v = self.expr()?;
            return Ok(Stmt::Assign(lhs, v));
        }
        for (p, code) in [
            ("+=", op::PLUS),
            ("-=", op::MINUS),
            ("*=", op::MULTIPLY),
            ("/=", op::DIVIDE),
            ("%=", op::MODULUS),
            ("|=", op::BIT_OR),
            ("&=", op::BIT_AND),
            ("^=", op::BIT_XOR),
            ("<<=", op::SHIFT_LEFT),
            (">>=", op::SHIFT_RIGHT),
        ] {
            if self.eat_punct(p) {
                let v = self.expr()?;
                return Ok(Stmt::Compound(code, lhs, v));
            }
        }
        if self.eat_punct("++") {
            return Ok(Stmt::Inc(lhs));
        }
        if self.eat_punct("--") {
            return Ok(Stmt::Dec(lhs));
        }
        match lhs {
            Expr::Call { .. } => Ok(Stmt::Expr(lhs)),
            _ => Err(self.err("expected a call or an assignment")),
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.binary(0)
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        // Lowest first.
        const LEVELS: [&[(&str, u8)]; 9] = [
            &[("||", 0)],
            &[("&&", 0)],
            &[("|", op::BIT_OR)],
            &[("^", op::BIT_XOR)],
            &[("&", op::BIT_AND)],
            &[("==", op::EQUAL), ("!=", op::NOT_EQUAL)],
            &[
                ("<=", op::LESS_THAN_OR_EQUAL),
                (">=", op::GREATER_THAN_OR_EQUAL),
                ("<", op::LESS_THAN),
                (">", op::GREATER_THAN),
            ],
            &[("<<", op::SHIFT_LEFT), (">>", op::SHIFT_RIGHT)],
            &[("+", op::PLUS), ("-", op::MINUS)],
        ];
        if level == LEVELS.len() {
            return self.multiplicative();
        }
        let mut lhs = self.binary(level + 1)?;
        'outer: loop {
            for (p, code) in LEVELS[level] {
                if self.is_punct(p) {
                    self.pos += 1;
                    let rhs = self.binary(level + 1)?;
                    lhs = match *p {
                        "||" => Expr::Or(Box::new(lhs), Box::new(rhs)),
                        "&&" => Expr::And(Box::new(lhs), Box::new(rhs)),
                        _ => Expr::Bin(*code, Box::new(lhs), Box::new(rhs)),
                    };
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, String> {
        let mut lhs = self.unary()?;
        loop {
            let code = if self.is_punct("*") {
                op::MULTIPLY
            } else if self.is_punct("/") {
                op::DIVIDE
            } else if self.is_punct("%") {
                op::MODULUS
            } else {
                return Ok(lhs);
            };
            self.pos += 1;
            let rhs = self.unary()?;
            lhs = Expr::Bin(code, Box::new(lhs), Box::new(rhs));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat_punct("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat_punct("~") {
            return Ok(Expr::Complement(Box::new(self.unary()?)));
        }
        if self.eat_punct("-") {
            return Ok(match self.unary()? {
                Expr::Int(n) => Expr::Int(-n),
                Expr::Float(f) => Expr::Float(-f),
                e => Expr::Neg(Box::new(e)),
            });
        }
        self.postfix()
    }

    fn args(&mut self) -> Result<Vec<Expr>, String> {
        self.expect("(")?;
        let mut out = Vec::new();
        if self.eat_punct(")") {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.eat_punct(")") {
                return Ok(out);
            }
            self.expect(",")?;
        }
    }

    /// A call's target after `thread` (if any): `name(`, `path::name(`,
    /// `[[ e ]](`. `None` when the tokens are not a call.
    fn callee(&mut self) -> Result<Option<Callee>, String> {
        if self.is_punct("[[") {
            self.pos += 1;
            let e = self.expr()?;
            self.expect("]]")?;
            return Ok(Some(Callee::Pointer(Box::new(e))));
        }
        if self.is_punct("::") {
            if let Some(Tok::Ident(n)) = self.peek_at(1).cloned()
                && self.is_punct_at(2, "(")
            {
                self.pos += 2;
                return Ok(Some(Callee::Named(String::new(), n)));
            }
            return Ok(None);
        }
        let Some(Tok::Ident(first)) = self.peek().cloned() else {
            return Ok(None);
        };
        // name(  or  a/b/c::name(
        let mut k = 1;
        let mut path = first.clone();
        while self.is_punct_at(k, "/")
            && let Some(Tok::Ident(seg)) = self.peek_at(k + 1)
        {
            path.push('/');
            path.push_str(seg);
            k += 2;
        }
        if self.is_punct_at(k, "::")
            && let Some(Tok::Ident(n)) = self.peek_at(k + 1).cloned()
            && self.is_punct_at(k + 2, "(")
        {
            self.pos += k + 2;
            return Ok(Some(Callee::Named(path, n)));
        }
        if k == 1 && self.is_punct_at(1, "(") {
            self.pos += 1;
            return Ok(Some(Callee::Named(String::new(), first)));
        }
        Ok(None)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        // A call with no object: `[thread] callee(args)`.
        let save = self.pos;
        let thread = self.eat_word("thread");
        if let Some(callee) = self.callee()? {
            let args = self.args()?;
            return Ok(Expr::Call {
                obj: None,
                callee,
                args,
                thread,
            });
        }
        self.pos = save;
        let Some(tok) = self.peek().cloned() else {
            return Err(self.err("expected an expression"));
        };
        self.pos += 1;
        Ok(match tok {
            Tok::Int(n) => Expr::Int(n),
            Tok::Float(f) => Expr::Float(f),
            Tok::Str(s) => Expr::Str(s),
            Tok::IStr(s) => Expr::IStr(s),
            Tok::Hash(s) => Expr::Hash(s),
            Tok::Punct("(") => {
                let a = self.expr()?;
                if self.eat_punct(",") {
                    let b = self.expr()?;
                    self.expect(",")?;
                    let c = self.expr()?;
                    self.expect(")")?;
                    Expr::Vector(Box::new([a, b, c]))
                } else {
                    self.expect(")")?;
                    a
                }
            }
            Tok::Punct("[") => {
                self.expect("]")?;
                Expr::EmptyArray
            }
            Tok::Punct("::") => Expr::FuncRef(String::new(), self.ident()?),
            Tok::Ident(w) => match w.as_str() {
                "undefined" => Expr::Undefined,
                "true" => Expr::Int(1),
                "false" => Expr::Int(0),
                "level" => Expr::Level,
                "self" => Expr::SelfE,
                "game" => Expr::Game,
                "anim" => Expr::Anim,
                _ => {
                    // path::name (a function reference)
                    let mut path = w.clone();
                    let save = self.pos;
                    while self.is_punct("/")
                        && let Some(Tok::Ident(seg)) = self.peek_at(1).cloned()
                    {
                        self.pos += 2;
                        path.push('/');
                        path.push_str(&seg);
                    }
                    if self.eat_punct("::") {
                        Expr::FuncRef(path, self.ident()?)
                    } else {
                        self.pos = save;
                        Expr::Local(w)
                    }
                }
            },
            _ => {
                self.pos -= 1;
                return Err(self.err("expected an expression"));
            }
        })
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            if self.eat_punct(".") {
                let f = self.ident()?;
                e = if f == "size" {
                    Expr::Size(Box::new(e))
                } else {
                    Expr::Field(Box::new(e), f)
                };
                continue;
            }
            if self.is_punct("[") {
                self.pos += 1;
                let k = self.expr()?;
                // `a[b[c]]`: the lexer read the two closing brackets as one.
                if self.is_punct("]]") {
                    self.t[self.pos].tok = Tok::Punct("]");
                } else {
                    self.expect("]")?;
                }
                e = Expr::Index(Box::new(e), Box::new(k));
                continue;
            }
            // A method call: `obj [thread] callee(args)`.
            let save = self.pos;
            let thread = self.eat_word("thread");
            let is_keyword = matches!(self.peek(), Some(Tok::Ident(w))
                if matches!(w.as_str(), "waittill" | "waittillmatch" | "notify" | "endon"));
            if !is_keyword && let Some(callee) = self.callee()? {
                let args = self.args()?;
                e = Expr::Call {
                    obj: Some(Box::new(e)),
                    callee,
                    args,
                    thread,
                };
                continue;
            }
            self.pos = save;
            return Ok(e);
        }
    }
}

// ---- code ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Label(usize);

enum I {
    Op(Insn),
    /// A local-variable opcode by slot (params first); the operand is
    /// counted from the last local once their number is known.
    Local(u8, usize),
    Jump(u8, Label),
    Switch(Vec<(CaseValue, Label)>, Label),
    Here(Label),
}

struct Gen {
    locals: Vec<String>,
    code: Vec<I>,
    labels: usize,
    /// (break, continue) per enclosing loop or switch (`None` continue for a switch).
    loops: Vec<(Label, Option<Label>)>,
    hidden: usize,
}

/// Builtins BO2 compiles to opcodes (its scripts never call them by name).
fn opcode_builtin(name: &str) -> Option<(u8, usize)> {
    Some(match name {
        "isdefined" => (op::IS_DEFINED, 1),
        "gettime" => (op::GET_TIME, 0),
        "getdvar" => (op::GET_DVAR, 1),
        "getdvarint" => (op::GET_DVAR_INT, 1),
        "getdvarfloat" => (op::GET_DVAR_FLOAT, 1),
        "getdvarvector" => (op::GET_DVAR_VECTOR, 1),
        "abs" => (op::ABS, 1),
        "anglestoforward" => (op::ANGLES_TO_FORWARD, 1),
        "anglestoright" => (op::ANGLES_TO_RIGHT, 1),
        "anglestoup" => (op::ANGLES_TO_UP, 1),
        "angleclamp180" => (op::ANGLE_CLAMP_180, 1),
        "vectortoangles" => (op::VECTOR_TO_ANGLES, 1),
        "vectorscale" => (op::VECTOR_SCALE, 2),
        "getfirstarraykey" => (op::FIRST_ARRAY_KEY, 1),
        "getnextarraykey" => (op::NEXT_ARRAY_KEY, 2),
        _ => return None,
    })
}

impl Gen {
    fn new(params: &[String]) -> Self {
        Self {
            locals: params.to_vec(),
            code: Vec::new(),
            labels: 0,
            loops: Vec::new(),
            hidden: 0,
        }
    }

    fn label(&mut self) -> Label {
        self.labels += 1;
        Label(self.labels)
    }

    fn here(&mut self, l: Label) {
        self.code.push(I::Here(l));
    }

    fn plain(&mut self, code: u8) {
        self.code.push(I::Op(Insn::Plain(code)));
    }

    fn slot(&mut self, name: &str) -> usize {
        match self.locals.iter().position(|l| l == name) {
            Some(i) => i,
            None => {
                self.locals.push(name.to_owned());
                self.locals.len() - 1
            }
        }
    }

    fn hidden_local(&mut self, what: &str) -> usize {
        self.hidden += 1;
        let name = format!("__{what}{}", self.hidden);
        self.slot(&name)
    }

    fn import(&mut self, code: u8, ns: &str, name: &str, params: usize, flags: u8) {
        self.code.push(I::Op(Insn::Call(
            code,
            Import {
                ns: ns.to_owned(),
                name: name.to_owned(),
                params: params as u8,
                flags,
            },
        )));
    }

    /// Instructions with addresses from `base`; returns them and the next
    /// free address.
    fn finish(mut self, base: u32) -> (Vec<(u32, Insn)>, u32) {
        self.plain(op::END);
        let n = self.locals.len();
        let prologue = if n == 0 {
            Insn::Plain(op::CHECK_CLEAR_PARAMS)
        } else {
            Insn::Locals(self.locals.clone())
        };
        // Addresses: one per real instruction; a label is the address of
        // the next one.
        let mut addr = base;
        let mut label_addr: Vec<u32> = vec![u32::MAX; self.labels + 1];
        let mut pending: Vec<usize> = Vec::new();
        let mut addrs: Vec<u32> = Vec::with_capacity(self.code.len() + 1);
        let start = addr;
        addr += 4;
        for ins in &self.code {
            match ins {
                I::Here(l) => pending.push(l.0),
                _ => {
                    for l in pending.drain(..) {
                        label_addr[l] = addr;
                    }
                    addrs.push(addr);
                    addr += 4;
                }
            }
        }
        for l in pending.drain(..) {
            label_addr[l] = addr;
        }
        let at = |l: Label| label_addr[l.0];
        let mut out = vec![(start, prologue)];
        let mut k = 0;
        for ins in self.code {
            let insn = match ins {
                I::Here(_) => continue,
                I::Op(insn) => insn,
                I::Local(code, slot) => Insn::Int(code, (n - 1 - slot) as i32),
                I::Jump(code, l) => Insn::Jump(code, at(l)),
                I::Switch(cases, end) => Insn::Switch {
                    cases: cases
                        .into_iter()
                        .map(|(value, l)| Case {
                            value,
                            target: at(l),
                        })
                        .collect(),
                    end: at(end),
                },
            };
            out.push((addrs[k], insn));
            k += 1;
        }
        (out, addr + 4)
    }

    fn block(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        for s in stmts {
            self.stmt(s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), String> {
        match s {
            Stmt::Expr(e) => {
                self.expr(e)?;
                self.plain(op::DEC_TOP);
            }
            Stmt::Assign(lv, v) => {
                self.expr(v)?;
                self.reference(lv)?;
                self.plain(op::SET_VARIABLE_FIELD);
            }
            Stmt::Compound(code, lv, v) => {
                self.expr(lv)?;
                self.expr(v)?;
                self.plain(*code);
                self.reference(lv)?;
                self.plain(op::SET_VARIABLE_FIELD);
            }
            Stmt::Inc(lv) => {
                self.reference(lv)?;
                self.plain(op::INC);
            }
            Stmt::Dec(lv) => {
                self.reference(lv)?;
                self.plain(op::DEC);
            }
            Stmt::Wait(e) => {
                self.expr(e)?;
                self.plain(op::WAIT);
            }
            Stmt::FrameEnd => self.plain(op::WAIT_TILL_FRAME_END),
            Stmt::Waittill(obj, name, vars) => {
                self.expr(name)?;
                self.expr(obj)?;
                self.plain(op::WAIT_TILL);
                for v in vars {
                    let slot = self.slot(v);
                    self.code
                        .push(I::Local(op::SAFE_SET_WAITTILL_VARIABLE_FIELD_CACHED, slot));
                }
                self.plain(op::CLEAR_PARAMS);
            }
            Stmt::WaittillMatch(obj, name, value) => {
                self.expr(value)?;
                self.expr(name)?;
                self.expr(obj)?;
                self.code.push(I::Op(Insn::Int(op::WAIT_TILL_MATCH, 1)));
                self.plain(op::CLEAR_PARAMS);
            }
            Stmt::Notify(obj, name, args) => {
                self.plain(op::VOID_CODE_POS);
                for a in args.iter().rev() {
                    self.expr(a)?;
                }
                self.expr(name)?;
                self.expr(obj)?;
                self.plain(op::NOTIFY);
            }
            Stmt::Endon(obj, name) => {
                self.expr(name)?;
                self.expr(obj)?;
                self.plain(op::END_ON);
            }
            Stmt::If(c, then, other) => {
                let l_else = self.label();
                self.expr(c)?;
                self.code.push(I::Jump(op::JUMP_ON_FALSE, l_else));
                self.block(then)?;
                if other.is_empty() {
                    self.here(l_else);
                } else {
                    let l_end = self.label();
                    self.code.push(I::Jump(op::JUMP, l_end));
                    self.here(l_else);
                    self.block(other)?;
                    self.here(l_end);
                }
            }
            Stmt::While(c, body) => {
                let (top, end) = (self.label(), self.label());
                self.here(top);
                if let Some(c) = c {
                    self.expr(c)?;
                    self.code.push(I::Jump(op::JUMP_ON_FALSE, end));
                }
                self.loops.push((end, Some(top)));
                self.block(body)?;
                self.loops.pop();
                self.code.push(I::Jump(op::JUMP_BACK, top));
                self.here(end);
            }
            Stmt::For(init, c, step, body) => {
                self.block(init)?;
                let (top, cont, end) = (self.label(), self.label(), self.label());
                self.here(top);
                if let Some(c) = c {
                    self.expr(c)?;
                    self.code.push(I::Jump(op::JUMP_ON_FALSE, end));
                }
                self.loops.push((end, Some(cont)));
                self.block(body)?;
                self.loops.pop();
                self.here(cont);
                self.block(step)?;
                self.code.push(I::Jump(op::JUMP_BACK, top));
                self.here(end);
            }
            Stmt::Foreach(key, value, arr, body) => {
                let a = self.hidden_local("array");
                let k = self.hidden_local("key");
                let v = self.slot(value);
                let named_key = key.as_ref().map(|n| self.slot(n));
                self.expr(arr)?;
                self.code
                    .push(I::Local(op::EVAL_LOCAL_VARIABLE_REF_CACHED, a));
                self.plain(op::SET_VARIABLE_FIELD);
                self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, a));
                self.plain(op::FIRST_ARRAY_KEY);
                self.code
                    .push(I::Local(op::EVAL_LOCAL_VARIABLE_REF_CACHED, k));
                self.plain(op::SET_VARIABLE_FIELD);
                let (top, cont, end) = (self.label(), self.label(), self.label());
                self.here(top);
                self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, k));
                self.plain(op::IS_DEFINED);
                self.code.push(I::Jump(op::JUMP_ON_FALSE, end));
                self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, k));
                self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, a));
                self.plain(op::EVAL_ARRAY);
                self.code
                    .push(I::Local(op::EVAL_LOCAL_VARIABLE_REF_CACHED, v));
                self.plain(op::SET_VARIABLE_FIELD);
                if let Some(nk) = named_key {
                    self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, k));
                    self.code
                        .push(I::Local(op::EVAL_LOCAL_VARIABLE_REF_CACHED, nk));
                    self.plain(op::SET_VARIABLE_FIELD);
                }
                self.loops.push((end, Some(cont)));
                self.block(body)?;
                self.loops.pop();
                self.here(cont);
                self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, k));
                self.code.push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, a));
                self.plain(op::NEXT_ARRAY_KEY);
                self.code
                    .push(I::Local(op::EVAL_LOCAL_VARIABLE_REF_CACHED, k));
                self.plain(op::SET_VARIABLE_FIELD);
                self.code.push(I::Jump(op::JUMP_BACK, top));
                self.here(end);
            }
            Stmt::Switch(v, arms) => {
                let end = self.label();
                let labels: Vec<Label> = arms.iter().map(|_| self.label()).collect();
                let mut cases = Vec::new();
                for ((values, _), l) in arms.iter().zip(&labels) {
                    for value in values {
                        cases.push((value.clone(), *l));
                    }
                }
                self.expr(v)?;
                self.code.push(I::Switch(cases, end));
                self.loops.push((end, None));
                for ((_, body), l) in arms.iter().zip(&labels) {
                    self.here(*l);
                    self.block(body)?;
                }
                self.loops.pop();
                self.here(end);
            }
            Stmt::Break => {
                let (end, _) = *self.loops.last().ok_or("break outside a loop")?;
                self.code.push(I::Jump(op::JUMP, end));
            }
            Stmt::Continue => {
                let cont = self
                    .loops
                    .iter()
                    .rev()
                    .find_map(|(_, c)| *c)
                    .ok_or("continue outside a loop")?;
                self.code.push(I::Jump(op::JUMP_BACK, cont));
            }
            Stmt::Return(e) => match e {
                Some(e) => {
                    self.expr(e)?;
                    self.plain(op::RETURN);
                }
                None => self.plain(op::END),
            },
        }
        Ok(())
    }

    /// Point the field register at an object expression.
    fn object(&mut self, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Level => self.plain(op::GET_LEVEL_OBJECT),
            Expr::SelfE => self.plain(op::GET_SELF_OBJECT),
            Expr::Anim => self.plain(op::GET_ANIM_OBJECT),
            other => {
                self.expr(other)?;
                self.plain(op::CAST_FIELD_OBJECT);
            }
        }
        Ok(())
    }

    /// A reference to assign through: keys first, then the base.
    fn reference(&mut self, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Local(n) => {
                let slot = self.slot(n);
                self.code
                    .push(I::Local(op::EVAL_LOCAL_VARIABLE_REF_CACHED, slot));
            }
            Expr::Field(base, f) => {
                self.object(base)?;
                self.code
                    .push(I::Op(Insn::Str(op::EVAL_FIELD_VARIABLE_REF, f.clone())));
            }
            Expr::Index(base, k) => {
                self.expr(k)?;
                match &**base {
                    Expr::Game => self.plain(op::GET_GAME_REF),
                    Expr::Local(n) => {
                        let slot = self.slot(n);
                        self.code
                            .push(I::Local(op::EVAL_LOCAL_ARRAY_REF_CACHED, slot));
                    }
                    other => self.reference(other)?,
                }
                self.plain(op::EVAL_ARRAY_REF);
            }
            other => return Err(format!("cannot assign to {other:?}")),
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Int(n) => self.code.push(I::Op(Insn::Int(op::GET_INTEGER, *n))),
            Expr::Float(f) => self.code.push(I::Op(Insn::Float(*f))),
            Expr::Str(s) => self.code.push(I::Op(Insn::Str(op::GET_STRING, s.clone()))),
            Expr::IStr(s) => self.code.push(I::Op(Insn::Str(op::GET_ISTRING, s.clone()))),
            Expr::Hash(s) => self.code.push(I::Op(Insn::Hash(crate::vm::hash_name(
                &s.to_ascii_lowercase(),
            )))),
            Expr::Undefined => self.plain(op::GET_UNDEFINED),
            Expr::Level => self.plain(op::GET_LEVEL),
            Expr::SelfE => self.plain(op::GET_SELF),
            Expr::Game => self.plain(op::GET_GAME),
            Expr::Anim => self.plain(op::GET_ANIM),
            Expr::EmptyArray => self.plain(op::EMPTY_ARRAY),
            Expr::Vector(v) => {
                // Popped x first: pushed z, y, x.
                self.expr(&v[2])?;
                self.expr(&v[1])?;
                self.expr(&v[0])?;
                self.plain(op::VECTOR);
            }
            Expr::Local(n) => {
                let slot = self.slot(n);
                self.code
                    .push(I::Local(op::EVAL_LOCAL_VARIABLE_CACHED, slot));
            }
            Expr::Field(base, f) => {
                self.object(base)?;
                self.code
                    .push(I::Op(Insn::Str(op::EVAL_FIELD_VARIABLE, f.clone())));
            }
            Expr::Index(base, k) => {
                self.expr(k)?;
                self.expr(base)?;
                self.plain(op::EVAL_ARRAY);
            }
            Expr::Size(base) => {
                self.expr(base)?;
                self.plain(op::SIZE_OF);
            }
            Expr::FuncRef(ns, name) => self.import(op::GET_FUNCTION, ns, name, 0, 1),
            Expr::Call {
                obj,
                callee,
                args,
                thread,
            } => self.call(obj.as_deref(), callee, args, *thread)?,
            Expr::Not(x) => {
                self.expr(x)?;
                self.plain(op::BOOL_NOT);
            }
            Expr::Complement(x) => {
                self.expr(x)?;
                self.plain(op::BOOL_COMPLEMENT);
            }
            Expr::Neg(x) => {
                self.code.push(I::Op(Insn::Int(op::GET_INTEGER, 0)));
                self.expr(x)?;
                self.plain(op::MINUS);
            }
            Expr::Bin(code, a, b) => {
                self.expr(a)?;
                self.expr(b)?;
                self.plain(*code);
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                let end = self.label();
                self.expr(a)?;
                let jump = if matches!(e, Expr::And(..)) {
                    op::JUMP_ON_FALSE_EXPR
                } else {
                    op::JUMP_ON_TRUE_EXPR
                };
                self.code.push(I::Jump(jump, end));
                self.expr(b)?;
                self.plain(op::CAST_BOOL);
                self.here(end);
            }
        }
        Ok(())
    }

    fn call(
        &mut self,
        obj: Option<&Expr>,
        callee: &Callee,
        args: &[Expr],
        thread: bool,
    ) -> Result<(), String> {
        if let (None, false, Callee::Named(ns, name)) = (obj, thread, callee)
            && ns.is_empty()
            && let Some((code, n)) = opcode_builtin(name)
        {
            if args.len() != n {
                return Err(format!("{name} takes {n} arguments"));
            }
            // Popped first-argument first for the two-argument ones
            // (vectorscale: vector then scale; next key: array then key).
            for a in args.iter().rev() {
                self.expr(a)?;
            }
            self.plain(code);
            return Ok(());
        }
        self.plain(op::PRE_SCRIPT_CALL);
        for a in args.iter().rev() {
            self.expr(a)?;
        }
        if let Some(o) = obj {
            self.expr(o)?;
        }
        match callee {
            Callee::Named(ns, name) => {
                let (code, flags) = match (obj.is_some(), thread) {
                    (false, false) => (op::SCRIPT_FUNCTION_CALL, 2),
                    (false, true) => (op::SCRIPT_THREAD_CALL, 3),
                    (true, false) => (op::SCRIPT_METHOD_CALL, 4),
                    (true, true) => (op::SCRIPT_METHOD_THREAD_CALL, 5),
                };
                self.import(code, ns, name, args.len(), flags);
            }
            Callee::Pointer(f) => {
                self.expr(f)?;
                let code = match (obj.is_some(), thread) {
                    (false, false) => op::SCRIPT_FUNCTION_CALL_POINTER,
                    (false, true) => op::SCRIPT_THREAD_CALL_POINTER,
                    (true, false) => op::SCRIPT_METHOD_CALL_POINTER,
                    (true, true) => op::SCRIPT_METHOD_THREAD_CALL_POINTER,
                };
                self.code.push(I::Op(Insn::Int(code, args.len() as i32)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Program, Strings, Value, Vm, compile};

    /// Compile `src` as one script, run `main` on the level, then `frames`
    /// server frames; returns the VM.
    fn run(src: &str, frames: usize) -> Vm<()> {
        let obj = compile("test/t", src).expect("compiles");
        let mut strings = Strings::default();
        let program = Program::link(vec![obj], &mut strings).expect("links");
        let mut vm = Vm::new(program, strings);
        crate::natives::bind_core(&mut vm);
        let level = Value::Object(vm.level);
        vm.spawn_named(&mut (), "test/t", "main", level, vec![])
            .expect("main exists");
        for _ in 0..frames {
            vm.run_frame(&mut ());
        }
        vm
    }

    fn level_int(vm: &mut Vm<()>, field: &str) -> Option<i32> {
        let f = vm.intern(field);
        vm.raw_field(vm.level, f).as_int()
    }

    /// bo2mp: the engine's multiplayer `_globallogic` compiles.
    #[test]
    fn engine_globallogic_compiles() {
        let src = include_str!("../../sim/src/t6/scripts/mp_globallogic.gsc");
        let obj = compile("maps/mp/gametypes/_globallogic", src).expect("compiles");
        assert!(
            obj.functions.len() > 80,
            "{} functions",
            obj.functions.len()
        );
    }

    #[test]
    fn arithmetic_loops_and_calls() {
        let mut vm = run(
            "add(a, b) { return a + b; }
             main() {
                 total = 0;
                 for (i = 0; i < 5; i++)
                     total += i;
                 n = 0;
                 while (1) { n++; if (n >= 3) break; }
                 level.total = add(total, n * 10);
                 level.neg = -4 + 1;
                 level.both = (1 && 0) || (2 > 1 && !0);
             }",
            0,
        );
        assert_eq!(level_int(&mut vm, "total"), Some(40));
        assert_eq!(level_int(&mut vm, "neg"), Some(-3));
        assert_eq!(level_int(&mut vm, "both"), Some(1));
    }

    #[test]
    fn arrays_foreach_switch_and_fields() {
        let mut vm = run(
            "main() {
                 level.teams = [];
                 level.teams[\"allies\"] = 3;
                 level.teams[\"axis\"] = 4;
                 sum = 0;
                 foreach (team, n in level.teams) {
                     if (team == \"axis\") sum += n * 100; else sum += n;
                 }
                 level.sum = sum;
                 level.count = level.teams.size;
                 switch (\"tdm\") {
                     case \"dm\": level.kind = 1; break;
                     case \"tdm\":
                     case \"war\": level.kind = 2; break;
                     default: level.kind = 3;
                 }
                 game[\"state\"] = \"playing\";
                 if (game[\"state\"] == \"playing\" && isdefined(level.teams[\"axis\"]))
                     level.ok = 1;
             }",
            0,
        );
        assert_eq!(level_int(&mut vm, "sum"), Some(403));
        assert_eq!(level_int(&mut vm, "count"), Some(2));
        assert_eq!(level_int(&mut vm, "kind"), Some(2));
        assert_eq!(level_int(&mut vm, "ok"), Some(1));
    }

    #[test]
    fn threads_waits_notifies_and_endons() {
        let mut vm = run(
            "waiter() {
                 level waittill(\"go\", amount, who);
                 level.got = amount * 10 + who;
             }
             ticker() {
                 level endon(\"stop\");
                 for (;;) { level.ticks++; wait 0.05; }
             }
             main() {
                 level.ticks = 0;
                 level thread waiter();
                 thread ticker();
                 wait 0.25;
                 level notify(\"go\", 4, 2);
                 level notify(\"stop\");
                 f = ::double;
                 level.ptr = [[ f ]](21);
             }
             double(x) { return x * 2; }",
            12,
        );
        assert_eq!(level_int(&mut vm, "got"), Some(42));
        assert_eq!(level_int(&mut vm, "ptr"), Some(42));
        let ticks = level_int(&mut vm, "ticks").unwrap_or(0);
        assert!((5..=7).contains(&ticks), "ticks {ticks}");
    }
}
