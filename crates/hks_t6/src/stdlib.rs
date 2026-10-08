//! The parts of Lua's standard library the UI scripts use: the base
//! functions, `string`, `table` and `math`.

use std::rc::Rc;

use crate::value::{Table, TableRef, Value, format_number};
use crate::vm::{LuaError, Vm};

type Res<T> = Result<T, LuaError>;

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Nil)
}

fn num(a: &[Value], i: usize, f: &str) -> Res<f32> {
    arg(a, i).as_num().ok_or_else(|| {
        LuaError::new(format!(
            "bad argument #{} to '{f}' (number expected)",
            i + 1
        ))
    })
}

fn string(a: &[Value], i: usize, f: &str) -> Res<String> {
    match arg(a, i) {
        Value::Str(s) => Ok(s.to_string()),
        Value::Num(n) => Ok(format_number(n)),
        _ => Err(LuaError::new(format!(
            "bad argument #{} to '{f}' (string expected)",
            i + 1
        ))),
    }
}

fn table(a: &[Value], i: usize, f: &str) -> Res<TableRef> {
    match arg(a, i) {
        Value::Table(t) => Ok(t),
        _ => Err(LuaError::new(format!(
            "bad argument #{} to '{f}' (table expected)",
            i + 1
        ))),
    }
}

fn set(
    vm: &mut Vm,
    t: &TableRef,
    name: &'static str,
    f: impl Fn(&mut Vm, Vec<Value>) -> Res<Vec<Value>> + 'static,
) {
    let v = vm.native(name, f);
    t.borrow_mut().set_str(name, v);
}

/// Lua string positions (1-based, negative from the end) to a byte range.
fn span(len: usize, i: f32, j: f32) -> (usize, usize) {
    let fix = |x: f32| -> i64 {
        let x = x as i64;
        if x < 0 {
            (len as i64 + x + 1).max(0)
        } else {
            x
        }
    };
    let (i, j) = (fix(i).max(1), fix(j).min(len as i64));
    if i > j {
        (0, 0)
    } else {
        (i as usize - 1, j as usize)
    }
}

/// Captures as values: spans as strings, positions as numbers.
fn captures_of(src: &str, caps: &[crate::pattern::Capture]) -> Vec<Value> {
    let b = src.as_bytes();
    caps.iter()
        .map(|c| match *c {
            crate::pattern::Capture::Span(x, y) => Value::str(&String::from_utf8_lossy(&b[x..y])),
            crate::pattern::Capture::Position(p) => Value::Num(p as f32),
        })
        .collect()
}

/// `string.find` (`find`) and `string.match`.
fn str_find(a: &[Value], find: bool) -> Res<Vec<Value>> {
    let name = if find { "find" } else { "match" };
    let src = string(a, 0, name)?;
    let pat = string(a, 1, name)?;
    let len = src.len() as i64;
    let init = arg(a, 2).as_num().map_or(1, |n| n as i64);
    let init = if init < 0 { (len + init + 1).max(1) } else { init.max(1) };
    if init - 1 > len {
        return Ok(vec![Value::Nil]);
    }
    let from = (init - 1) as usize;
    let plain = arg(a, 3).truthy();
    if find && (plain || !crate::pattern::has_specials(pat.as_bytes())) {
        if pat.is_empty() {
            return Ok(vec![Value::Num((from + 1) as f32), Value::Num(from as f32)]);
        }
        return Ok(match find_plain(&src, &pat, from) {
            Some((b, e)) => vec![Value::Num((b + 1) as f32), Value::Num(e as f32)],
            None => vec![Value::Nil],
        });
    }
    match crate::pattern::find(src.as_bytes(), pat.as_bytes(), from).map_err(LuaError::new)? {
        Some((s, e, m)) if find => {
            let mut out = vec![Value::Num((s + 1) as f32), Value::Num(e as f32)];
            out.extend(captures_of(&src, &m.own_captures(s, e)));
            Ok(out)
        }
        Some((s, e, m)) => Ok(captures_of(&src, &m.captures(s, e))),
        None => Ok(vec![Value::Nil]),
    }
}

/// Plain substring find (Lua patterns' special characters taken literally
/// unless the pattern has none).
fn find_plain(hay: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    hay.get(from..)?
        .find(needle)
        .map(|p| (from + p, from + p + needle.len()))
}

pub fn open(vm: &mut Vm) {
    let g = vm.globals.clone();
    vm.set_global("_G", Value::Table(g.clone()));
    set(vm, &g, "type", |_, a| {
        Ok(vec![Value::str(arg(&a, 0).type_name())])
    });
    set(vm, &g, "tostring", |vm, a| {
        let s = vm.tostring(&arg(&a, 0))?;
        Ok(vec![Value::str(&s)])
    });
    set(vm, &g, "tonumber", |_, a| {
        let v = arg(&a, 0);
        let base = arg(&a, 1).as_num().unwrap_or(10.0) as u32;
        let r = if base == 10 {
            v.as_num()
        } else {
            v.as_str()
                .and_then(|s| i64::from_str_radix(s.trim(), base).ok())
                .map(|n| n as f32)
        };
        Ok(vec![r.map_or(Value::Nil, Value::Num)])
    });
    set(vm, &g, "print", |vm, a| {
        let parts: Vec<String> = a
            .iter()
            .map(|v| vm.tostring(v).unwrap_or_else(|_| String::new()))
            .collect();
        eprintln!("[lua] {}", parts.join("\t"));
        Ok(vec![])
    });
    set(vm, &g, "error", |_, a| {
        let v = arg(&a, 0);
        let mut e = LuaError::new(v.to_string());
        e.value = v;
        Err(e)
    });
    set(vm, &g, "assert", |_, a| {
        if arg(&a, 0).truthy() {
            Ok(a)
        } else {
            Err(LuaError::new(
                arg(&a, 1)
                    .as_str()
                    .unwrap_or("assertion failed!")
                    .to_owned(),
            ))
        }
    });
    set(vm, &g, "pcall", |vm, mut a| {
        if a.is_empty() {
            return Err(LuaError::new("bad argument #1 to 'pcall'"));
        }
        let f = a.remove(0);
        match vm.call(f, a) {
            Ok(mut r) => {
                r.insert(0, Value::Bool(true));
                Ok(r)
            }
            Err(e) => Ok(vec![Value::Bool(false), e.value]),
        }
    });
    set(vm, &g, "select", |_, a| match arg(&a, 0) {
        Value::Str(s) if &*s == "#" => Ok(vec![Value::Num((a.len().saturating_sub(1)) as f32)]),
        v => {
            let n = v.as_num().unwrap_or(1.0) as i64;
            let start = if n < 0 {
                (a.len() as i64 + n).max(1)
            } else {
                n
            } as usize;
            Ok(a.into_iter().skip(start).collect())
        }
    });
    set(vm, &g, "rawget", |_, a| {
        Ok(vec![table(&a, 0, "rawget")?.borrow().get(&arg(&a, 1))])
    });
    set(vm, &g, "rawset", |_, a| {
        table(&a, 0, "rawset")?
            .borrow_mut()
            .set(arg(&a, 1), arg(&a, 2));
        Ok(vec![arg(&a, 0)])
    });
    set(vm, &g, "rawequal", |_, a| {
        Ok(vec![Value::Bool(arg(&a, 0).raw_eq(&arg(&a, 1)))])
    });
    set(vm, &g, "setmetatable", |_, a| {
        let t = table(&a, 0, "setmetatable")?;
        t.borrow_mut().meta = arg(&a, 1).table().cloned();
        Ok(vec![arg(&a, 0)])
    });
    set(vm, &g, "getmetatable", |vm, a| {
        let m = vm.metatable(&arg(&a, 0));
        Ok(vec![match m {
            Some(m) => {
                let protect = m.borrow().get_str("__metatable");
                if matches!(protect, Value::Nil) {
                    Value::Table(m)
                } else {
                    protect
                }
            }
            None => Value::Nil,
        }])
    });
    set(vm, &g, "next", |_, a| {
        let t = table(&a, 0, "next")?;
        Ok(match t.borrow().next(&arg(&a, 1)) {
            Some((k, v)) => vec![k, v],
            None => vec![Value::Nil],
        })
    });
    let next = g.borrow().get_str("next");
    let next_for_pairs = next.clone();
    set(vm, &g, "pairs", move |_, a| {
        table(&a, 0, "pairs")?;
        Ok(vec![next_for_pairs.clone(), arg(&a, 0), Value::Nil])
    });
    let inext = vm.native("ipairs_next", |vm, a| {
        let i = arg(&a, 1).as_num().unwrap_or(0.0) + 1.0;
        let v = vm.index(&arg(&a, 0), &Value::Num(i))?;
        Ok(if matches!(v, Value::Nil) {
            vec![Value::Nil]
        } else {
            vec![Value::Num(i), v]
        })
    });
    set(vm, &g, "ipairs", move |_, a| {
        Ok(vec![inext.clone(), arg(&a, 0), Value::Num(0.0)])
    });
    let unpack = vm.native("unpack", |_, a| {
        let t = table(&a, 0, "unpack")?;
        let t = t.borrow();
        let i = arg(&a, 1).as_num().unwrap_or(1.0) as usize;
        let j = arg(&a, 2).as_num().map_or(t.len(), |j| j as usize);
        Ok((i..=j).map(|n| t.get(&Value::Num(n as f32))).collect())
    });
    g.borrow_mut().set_str("unpack", unpack.clone());
    // collectgarbage(opt): memory is Rust's; "count" answers 0 KB.
    let gc = vm.native("collectgarbage", |_, _| Ok(vec![Value::Num(0.0)]));
    g.borrow_mut().set_str("collectgarbage", gc);

    // ---- string -----------------------------------------------------------
    let s = Table::new_ref();
    set(vm, &s, "len", |_, a| {
        Ok(vec![Value::Num(string(&a, 0, "len")?.len() as f32)])
    });
    set(vm, &s, "sub", |_, a| {
        let st = string(&a, 0, "sub")?;
        let (i, j) = span(
            st.len(),
            arg(&a, 1).as_num().unwrap_or(1.0),
            arg(&a, 2).as_num().unwrap_or(-1.0),
        );
        Ok(vec![Value::str(st.get(i..j).unwrap_or(""))])
    });
    set(vm, &s, "upper", |_, a| {
        Ok(vec![Value::str(&string(&a, 0, "upper")?.to_uppercase())])
    });
    set(vm, &s, "lower", |_, a| {
        Ok(vec![Value::str(&string(&a, 0, "lower")?.to_lowercase())])
    });
    set(vm, &s, "rep", |_, a| {
        let st = string(&a, 0, "rep")?;
        Ok(vec![Value::str(
            &st.repeat(num(&a, 1, "rep")?.max(0.0) as usize),
        )])
    });
    set(vm, &s, "byte", |_, a| {
        let st = string(&a, 0, "byte")?;
        let i = arg(&a, 1).as_num().unwrap_or(1.0);
        let (i, j) = span(st.len(), i, arg(&a, 2).as_num().unwrap_or(i));
        Ok(st.as_bytes()[i..j]
            .iter()
            .map(|b| Value::Num(f32::from(*b)))
            .collect())
    });
    set(vm, &s, "char", |_, a| {
        let st: String = a
            .iter()
            .filter_map(Value::as_num)
            .map(|n| n as u8 as char)
            .collect();
        Ok(vec![Value::str(&st)])
    });
    set(vm, &s, "find", |_, a| str_find(&a, true));
    set(vm, &s, "match", |_, a| str_find(&a, false));
    set(vm, &s, "gmatch", |vm, a| {
        let src = string(&a, 0, "gmatch")?;
        let pat = string(&a, 1, "gmatch")?;
        let pos = std::cell::Cell::new(0usize);
        let f = vm.native("gmatch_iter", move |_, _| {
            let (sb, pb) = (src.as_bytes(), pat.as_bytes());
            let mut m = crate::pattern::Matcher::new(sb, pb);
            let mut at = pos.get();
            while at <= sb.len() {
                m.reset();
                if let Some(e) = m.do_match(at, 0) {
                    pos.set(if e == at { e + 1 } else { e });
                    return Ok(captures_of(&src, &m.captures(at, e)));
                }
                if let Some(err) = m.error.take() {
                    return Err(LuaError::new(err));
                }
                at += 1;
            }
            pos.set(sb.len() + 1);
            Ok(vec![Value::Nil])
        });
        Ok(vec![f])
    });
    set(vm, &s, "format", |vm, a| {
        let fmt = string(&a, 0, "format")?;
        let mut out = String::new();
        let mut it = a.iter().skip(1);
        let mut chars = fmt.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                out.push(ch);
                continue;
            }
            let mut spec = String::new();
            while let Some(&n) = chars.peek() {
                if n.is_ascii_digit() || matches!(n, '.' | '-' | '+' | ' ' | '#') {
                    spec.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let conv = chars.next().unwrap_or('%');
            let prec = spec.split('.').nth(1).and_then(|p| p.parse::<usize>().ok());
            let width = spec.split('.').next().and_then(|w| {
                w.trim_start_matches(['-', '+', ' ', '#', '0'])
                    .parse::<usize>()
                    .ok()
            });
            let piece = match conv {
                '%' => "%".to_owned(),
                'd' | 'i' => format!(
                    "{}",
                    it.next().and_then(Value::as_num).unwrap_or(0.0) as i64
                ),
                'f' => format!(
                    "{:.*}",
                    prec.unwrap_or(6),
                    it.next().and_then(Value::as_num).unwrap_or(0.0)
                ),
                'g' => format_number(it.next().and_then(Value::as_num).unwrap_or(0.0)),
                'x' => format!(
                    "{:x}",
                    it.next().and_then(Value::as_num).unwrap_or(0.0) as i64
                ),
                'X' => format!(
                    "{:X}",
                    it.next().and_then(Value::as_num).unwrap_or(0.0) as i64
                ),
                'c' => {
                    ((it.next().and_then(Value::as_num).unwrap_or(0.0) as u8) as char).to_string()
                }
                's' => {
                    let v = it.next().cloned().unwrap_or(Value::Nil);
                    let s = vm.tostring(&v)?;
                    match prec {
                        Some(p) => s.chars().take(p).collect(),
                        None => s,
                    }
                }
                'q' => format!(
                    "{:?}",
                    it.next()
                        .map(ToString::to_string)
                        .unwrap_or_else(String::new)
                ),
                other => format!("%{other}"),
            };
            match width {
                Some(w) if piece.len() < w => {
                    let pad = " ".repeat(w - piece.len());
                    if spec.starts_with('-') {
                        out.push_str(&piece);
                        out.push_str(&pad);
                    } else if spec.starts_with('0') || spec.contains('0') && !spec.contains('.') {
                        out.push_str(&"0".repeat(w - piece.len()));
                        out.push_str(&piece);
                    } else {
                        out.push_str(&pad);
                        out.push_str(&piece);
                    }
                }
                _ => out.push_str(&piece),
            }
        }
        Ok(vec![Value::str(&out)])
    });
    set(vm, &s, "gsub", |vm, a| {
        use crate::pattern::Capture;
        let src = string(&a, 0, "gsub")?;
        let pat = string(&a, 1, "gsub")?;
        let repl = arg(&a, 2);
        let max_n = arg(&a, 3).as_num().map_or(usize::MAX, |n| n.max(0.0) as usize);
        let (sb, pb) = (src.as_bytes(), pat.as_bytes());
        let (anchor, p0) = if pb.first() == Some(&b'^') { (true, 1) } else { (false, 0) };
        let mut m = crate::pattern::Matcher::new(sb, pb);
        let mut out: Vec<u8> = Vec::with_capacity(sb.len());
        let (mut at, mut n) = (0usize, 0usize);
        while n < max_n {
            m.reset();
            let e = m.do_match(at, p0);
            if let Some(err) = m.error.take() {
                return Err(LuaError::new(err));
            }
            if let Some(e) = e {
                n += 1;
                let caps = m.captures(at, e);
                let whole = &sb[at..e];
                let value = match &repl {
                    Value::Str(r) => {
                        let rb = r.as_bytes();
                        let mut i = 0;
                        while i < rb.len() {
                            if rb[i] == b'%' && i + 1 < rb.len() {
                                i += 1;
                                let d = rb[i];
                                if d == b'0' {
                                    out.extend_from_slice(whole);
                                } else if d.is_ascii_digit() {
                                    let k = usize::from(d - b'1');
                                    match caps.get(k).copied() {
                                        Some(Capture::Span(x, y)) => out.extend_from_slice(&sb[x..y]),
                                        Some(Capture::Position(p)) => out.extend_from_slice(p.to_string().as_bytes()),
                                        None => return Err(LuaError::new("invalid capture index")),
                                    }
                                } else {
                                    out.push(d);
                                }
                            } else {
                                out.push(rb[i]);
                            }
                            i += 1;
                        }
                        None
                    }
                    Value::Num(x) => {
                        out.extend_from_slice(format_number(*x).as_bytes());
                        None
                    }
                    Value::Table(t) => {
                        let key = captures_of(&src, &caps).into_iter().next().unwrap_or(Value::Nil);
                        Some(vm.index(&Value::Table(t.clone()), &key)?)
                    }
                    f @ (Value::Func(_) | Value::Native(_)) => {
                        Some(vm.call(f.clone(), captures_of(&src, &caps))?.into_iter().next().unwrap_or(Value::Nil))
                    }
                    _ => return Err(LuaError::new("bad argument #3 to 'gsub' (string/function/table expected)")),
                };
                match value {
                    None => {}
                    Some(Value::Nil | Value::Bool(false)) => out.extend_from_slice(whole),
                    Some(v @ (Value::Str(_) | Value::Num(_))) => out.extend_from_slice(v.to_string().as_bytes()),
                    Some(v) => {
                        return Err(LuaError::new(format!("invalid replacement value (a {})", v.type_name())));
                    }
                }
                if e > at {
                    at = e;
                } else if at < sb.len() {
                    out.push(sb[at]);
                    at += 1;
                } else {
                    break;
                }
            } else if at < sb.len() {
                out.push(sb[at]);
                at += 1;
            } else {
                break;
            }
            if anchor {
                break;
            }
        }
        out.extend_from_slice(&sb[at.min(sb.len())..]);
        Ok(vec![Value::str(&String::from_utf8_lossy(&out)), Value::Num(n as f32)])
    });
    let string_table = s.clone();
    g.borrow_mut().set_str("string", Value::Table(s));
    let meta = Table::new_ref();
    meta.borrow_mut()
        .set_str("__index", Value::Table(string_table));
    vm.string_meta = Some(meta);

    // ---- table ------------------------------------------------------------
    let t = Table::new_ref();
    set(vm, &t, "insert", |_, a| {
        let tab = table(&a, 0, "insert")?;
        let mut tab = tab.borrow_mut();
        let n = tab.len();
        if a.len() >= 3 {
            let pos = num(&a, 1, "insert")? as usize;
            for i in (pos..=n).rev() {
                let v = tab.get(&Value::Num(i as f32));
                tab.set(Value::Num((i + 1) as f32), v);
            }
            tab.set(Value::Num(pos as f32), arg(&a, 2));
        } else {
            tab.set(Value::Num((n + 1) as f32), arg(&a, 1));
        }
        Ok(vec![])
    });
    set(vm, &t, "remove", |_, a| {
        let tab = table(&a, 0, "remove")?;
        let mut tab = tab.borrow_mut();
        let n = tab.len();
        if n == 0 {
            return Ok(vec![Value::Nil]);
        }
        let pos = arg(&a, 1).as_num().map_or(n, |p| p as usize);
        let removed = tab.get(&Value::Num(pos as f32));
        for i in pos..n {
            let v = tab.get(&Value::Num((i + 1) as f32));
            tab.set(Value::Num(i as f32), v);
        }
        tab.set(Value::Num(n as f32), Value::Nil);
        Ok(vec![removed])
    });
    set(vm, &t, "getn", |_, a| {
        Ok(vec![
            Value::Num(table(&a, 0, "getn")?.borrow().len() as f32),
        ])
    });
    set(vm, &t, "concat", |vm, a| {
        let tab = table(&a, 0, "concat")?;
        let sep = arg(&a, 1).as_str().unwrap_or("").to_owned();
        let items: Vec<Value> = {
            let tab = tab.borrow();
            (1..=tab.len())
                .map(|i| tab.get(&Value::Num(i as f32)))
                .collect()
        };
        let parts: Vec<String> = items
            .iter()
            .map(|v| vm.tostring(v).unwrap_or_else(|_| String::new()))
            .collect();
        Ok(vec![Value::str(&parts.join(&sep))])
    });
    set(vm, &t, "sort", |vm, a| {
        let tab = table(&a, 0, "sort")?;
        let cmp = arg(&a, 1);
        let mut items: Vec<Value> = {
            let tab = tab.borrow();
            (1..=tab.len())
                .map(|i| tab.get(&Value::Num(i as f32)))
                .collect()
        };
        // Insertion sort with the script's comparator (lists are short).
        for i in 1..items.len() {
            let mut j = i;
            while j > 0 {
                let less = if matches!(cmp, Value::Nil) {
                    match (&items[j], &items[j - 1]) {
                        (Value::Num(x), Value::Num(y)) => x < y,
                        (Value::Str(x), Value::Str(y)) => x < y,
                        _ => false,
                    }
                } else {
                    vm.call(cmp.clone(), vec![items[j].clone(), items[j - 1].clone()])?
                        .first()
                        .is_some_and(Value::truthy)
                };
                if !less {
                    break;
                }
                items.swap(j, j - 1);
                j -= 1;
            }
        }
        let mut tab = tab.borrow_mut();
        for (i, v) in items.into_iter().enumerate() {
            tab.set(Value::Num((i + 1) as f32), v);
        }
        Ok(vec![])
    });
    g.borrow_mut().set_str("table", Value::Table(t));

    // ---- math -------------------------------------------------------------
    let m = Table::new_ref();
    let one = |name: &'static str, f: fn(f32) -> f32| (name, f);
    for (name, f) in [
        one("floor", f32::floor),
        one("ceil", f32::ceil),
        one("abs", f32::abs),
        one("sqrt", f32::sqrt),
        one("sin", f32::sin),
        one("cos", f32::cos),
        one("tan", f32::tan),
        one("asin", f32::asin),
        one("acos", f32::acos),
        one("exp", f32::exp),
        one("log", f32::ln),
        one("rad", f32::to_radians),
        one("deg", f32::to_degrees),
    ] {
        set(vm, &m, name, move |_, a| {
            Ok(vec![Value::Num(f(num(&a, 0, name)?))])
        });
    }
    set(vm, &m, "atan", |_, a| {
        Ok(vec![Value::Num(num(&a, 0, "atan")?.atan())])
    });
    set(vm, &m, "atan2", |_, a| {
        Ok(vec![Value::Num(
            num(&a, 0, "atan2")?.atan2(num(&a, 1, "atan2")?),
        )])
    });
    set(vm, &m, "pow", |_, a| {
        Ok(vec![Value::Num(
            num(&a, 0, "pow")?.powf(num(&a, 1, "pow")?),
        )])
    });
    set(vm, &m, "fmod", |_, a| {
        Ok(vec![Value::Num(num(&a, 0, "fmod")? % num(&a, 1, "fmod")?)])
    });
    set(vm, &m, "mod", |_, a| {
        Ok(vec![Value::Num(num(&a, 0, "mod")? % num(&a, 1, "mod")?)])
    });
    set(vm, &m, "min", |_, a| {
        Ok(vec![Value::Num(
            a.iter()
                .filter_map(Value::as_num)
                .fold(f32::INFINITY, f32::min),
        )])
    });
    set(vm, &m, "max", |_, a| {
        Ok(vec![Value::Num(
            a.iter()
                .filter_map(Value::as_num)
                .fold(f32::NEG_INFINITY, f32::max),
        )])
    });
    let seed = Rc::new(std::cell::Cell::new(0x2545_f491_u32));
    let s2 = seed.clone();
    set(vm, &m, "random", move |_, a| {
        // xorshift: the UI only uses it for flicker and shuffles.
        let mut x = s2.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        s2.set(x);
        let r = (x >> 8) as f32 / (1u32 << 24) as f32;
        Ok(vec![Value::Num(
            match (arg(&a, 0).as_num(), arg(&a, 1).as_num()) {
                (Some(lo), Some(hi)) => (lo + (r * (hi - lo + 1.0)).floor()).min(hi),
                (Some(hi), None) => (1.0 + (r * hi).floor()).min(hi),
                _ => r,
            },
        )])
    });
    set(vm, &m, "randomseed", move |_, a| {
        seed.set((arg(&a, 0).as_num().unwrap_or(1.0) as u32).max(1));
        Ok(vec![])
    });
    m.borrow_mut()
        .set_str("pi", Value::Num(std::f32::consts::PI));
    m.borrow_mut().set_str("huge", Value::Num(f32::INFINITY));
    g.borrow_mut().set_str("math", Value::Table(m));

    // ---- os (bo2mp: the clock; BO2's menus seed math.random with
    // os.time(), the After Action Report's match rating) -----------------
    let o = Table::new_ref();
    set(vm, &o, "time", |_, _| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // (f32 holds whole seconds to 2^24: the low bits, a seed.)
        Ok(vec![Value::Num((secs % (1 << 24)) as f32)])
    });
    let start = std::time::Instant::now();
    set(vm, &o, "clock", move |_, _| Ok(vec![Value::Num(start.elapsed().as_secs_f32())]));
    g.borrow_mut().set_str("os", Value::Table(o));
}
