//! Builtins that need no engine: numbers, strings, arrays, structs, dvars.

use crate::math;
use crate::value::{Array, Key, Value};
use crate::vm::{Vm, truthy};

fn arg<'a>(args: &'a [Value], i: usize) -> &'a Value {
    args.get(i).unwrap_or(&Value::Undefined)
}

fn num(args: &[Value], i: usize, what: &str) -> Result<f32, String> {
    arg(args, i).as_float().ok_or_else(|| {
        format!(
            "{what}: argument {} is {}, not a number",
            i + 1,
            arg(args, i).type_name()
        )
    })
}

fn vec3(args: &[Value], i: usize, what: &str) -> Result<[f32; 3], String> {
    arg(args, i).as_vec3().ok_or_else(|| {
        format!(
            "{what}: argument {} is {}, not a vector",
            i + 1,
            arg(args, i).type_name()
        )
    })
}

/// A copy of an array argument's contents.
fn array(args: &[Value], i: usize, what: &str) -> Result<Array, String> {
    match arg(args, i) {
        Value::Array(a) => Ok(a.snapshot()),
        other => Err(format!(
            "{what}: argument {} is {}, not an array",
            i + 1,
            other.type_name()
        )),
    }
}

fn text<H>(vm: &Vm<H>, args: &[Value], i: usize) -> String {
    match arg(args, i) {
        Value::Undefined => String::new(),
        v => vm.to_text(v),
    }
}

fn flag(args: &[Value], i: usize, default: bool) -> bool {
    match arg(args, i) {
        Value::Undefined => default,
        v => truthy(v),
    }
}

fn list(values: Vec<Value>) -> Value {
    let mut a = Array::new();
    for v in values {
        a.push(v);
    }
    Value::array(a)
}

/// Array values with int keys re-numbered 0.. and other keys kept, in
/// insertion order.
fn renumber(src: impl Iterator<Item = (Key, Value)>, keep_keys: bool) -> Array {
    let mut a = Array::new();
    let mut n = 0;
    for (k, v) in src {
        match k {
            Key::Int(_) if !keep_keys => {
                a.set(Key::Int(n), v);
                n += 1;
            }
            k => a.set(k, v),
        }
    }
    a
}

pub fn bind_core<H>(vm: &mut Vm<H>) {
    macro_rules! f {
        ($name:literal, $body:expr) => {
            vm.bind($name, false, $body);
        };
    }
    f!("int", |vm, _, _, a| Ok(match arg(a, 0) {
        Value::Int(i) => Value::Int(*i),
        Value::Float(f) => Value::Int(*f as i32),
        Value::Str(_) | Value::IStr(_) => Value::Int(math::parse_int(&vm.to_text(arg(a, 0)))),
        Value::Undefined => Value::Int(0),
        other => return Err(format!("int of a {}", other.type_name())),
    }));
    f!("float", |vm, _, _, a| Ok(match arg(a, 0) {
        Value::Int(i) => Value::Float(*i as f32),
        Value::Float(f) => Value::Float(*f),
        Value::Str(_) | Value::IStr(_) =>
            Value::Float(vm.to_text(arg(a, 0)).trim().parse::<f32>().unwrap_or(0.0)),
        Value::Undefined => Value::Float(0.0),
        other => return Err(format!("float of a {}", other.type_name())),
    }));
    f!("randomint", |vm, _, _, a| {
        let n = num(a, 0, "randomint")? as i32;
        Ok(Value::Int(if n <= 0 {
            0
        } else {
            (vm.rand_u32() % n as u32) as i32
        }))
    });
    f!("randomintrange", |vm, _, _, a| {
        let lo = num(a, 0, "randomintrange")? as i32;
        let hi = num(a, 1, "randomintrange")? as i32;
        Ok(Value::Int(if hi <= lo {
            lo
        } else {
            lo + (vm.rand_u32() % (hi - lo) as u32) as i32
        }))
    });
    f!("randomfloat", |vm, _, _, a| {
        let n = num(a, 0, "randomfloat")?;
        Ok(Value::Float(vm.rand_unit() * n))
    });
    f!("randomfloatrange", |vm, _, _, a| {
        let lo = num(a, 0, "randomfloatrange")?;
        let hi = num(a, 1, "randomfloatrange")?;
        Ok(Value::Float(lo + vm.rand_unit() * (hi - lo)))
    });
    f!("distance", |_, _, _, a| Ok(Value::Float(math::length(
        math::sub(vec3(a, 0, "distance")?, vec3(a, 1, "distance")?)
    ))));
    f!("distancesquared", |_, _, _, a| {
        let d = math::sub(
            vec3(a, 0, "distancesquared")?,
            vec3(a, 1, "distancesquared")?,
        );
        Ok(Value::Float(math::dot(d, d)))
    });
    f!("distance2d", |_, _, _, a| {
        let d = math::sub(vec3(a, 0, "distance2d")?, vec3(a, 1, "distance2d")?);
        Ok(Value::Float((d[0] * d[0] + d[1] * d[1]).sqrt()))
    });
    f!("distance2dsquared", |_, _, _, a| {
        let d = math::sub(
            vec3(a, 0, "distance2dsquared")?,
            vec3(a, 1, "distance2dsquared")?,
        );
        Ok(Value::Float(d[0] * d[0] + d[1] * d[1]))
    });
    f!("vectornormalize", |_, _, _, a| Ok(Value::Vec3(
        math::normalize(vec3(a, 0, "vectornormalize")?)
    )));
    f!("vectordot", |_, _, _, a| Ok(Value::Float(math::dot(
        vec3(a, 0, "vectordot")?,
        vec3(a, 1, "vectordot")?
    ))));
    f!("vectorcross", |_, _, _, a| Ok(Value::Vec3(math::cross(
        vec3(a, 0, "vectorcross")?,
        vec3(a, 1, "vectorcross")?
    ))));
    f!("length", |_, _, _, a| Ok(Value::Float(math::length(vec3(
        a, 0, "length"
    )?))));
    f!("lengthsquared", |_, _, _, a| {
        let v = vec3(a, 0, "lengthsquared")?;
        Ok(Value::Float(math::dot(v, v)))
    });
    f!("length2d", |_, _, _, a| {
        let v = vec3(a, 0, "length2d")?;
        Ok(Value::Float((v[0] * v[0] + v[1] * v[1]).sqrt()))
    });
    f!("vectorlerp", |_, _, _, a| {
        let x = vec3(a, 0, "vectorlerp")?;
        let y = vec3(a, 1, "vectorlerp")?;
        let t = num(a, 2, "vectorlerp")?;
        Ok(Value::Vec3(math::add(x, math::scale(math::sub(y, x), t))))
    });
    f!("vectorscale", |_, _, _, a| Ok(Value::Vec3(math::scale(
        vec3(a, 0, "vectorscale")?,
        num(a, 1, "vectorscale")?
    ))));
    f!("anglestoforward", |_, _, _, a| Ok(Value::Vec3(
        math::angle_vectors(vec3(a, 0, "anglestoforward")?).0
    )));
    f!("anglestoright", |_, _, _, a| Ok(Value::Vec3(
        math::angle_vectors(vec3(a, 0, "anglestoright")?).1
    )));
    f!("anglestoup", |_, _, _, a| Ok(Value::Vec3(
        math::angle_vectors(vec3(a, 0, "anglestoup")?).2
    )));
    f!("vectortoangles", |_, _, _, a| Ok(Value::Vec3(
        math::vector_to_angles(vec3(a, 0, "vectortoangles")?)
    )));
    f!("vectortoyaw", |_, _, _, a| Ok(Value::Float(
        math::vector_to_angles(vec3(a, 0, "vectortoyaw")?)[1]
    )));
    f!("angleclamp180", |_, _, _, a| Ok(Value::Float(
        math::angle_clamp180(num(a, 0, "angleclamp180")?)
    )));
    f!("angleclamp", |_, _, _, a| Ok(Value::Float(
        math::angle_clamp(num(a, 0, "angleclamp")?)
    )));
    f!("absangleclamp180", |_, _, _, a| Ok(Value::Float(
        math::angle_clamp180(num(a, 0, "absangleclamp180")?).abs()
    )));
    f!("combineangles", |_, _, _, a| {
        let x = vec3(a, 0, "combineangles")?;
        let y = vec3(a, 1, "combineangles")?;
        Ok(Value::Vec3([x[0] + y[0], x[1] + y[1], x[2] + y[2]]))
    });
    f!("sin", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "sin")?.to_radians().sin()
    )));
    f!("cos", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "cos")?.to_radians().cos()
    )));
    f!("tan", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "tan")?.to_radians().tan()
    )));
    f!("asin", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "asin")?.clamp(-1.0, 1.0).asin().to_degrees()
    )));
    f!("acos", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "acos")?.clamp(-1.0, 1.0).acos().to_degrees()
    )));
    f!("atan", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "atan")?.atan().to_degrees()
    )));
    f!("sqrt", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "sqrt")?.max(0.0).sqrt()
    )));
    f!("pow", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "pow")?.powf(num(a, 1, "pow")?)
    )));
    f!("exp", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "exp")?.exp()
    )));
    f!("log", |_, _, _, a| Ok(Value::Float(num(a, 0, "log")?.ln())));
    f!("floor", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "floor")?.floor()
    )));
    f!("ceil", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "ceil")?.ceil()
    )));
    f!("round", |_, _, _, a| Ok(Value::Float(
        num(a, 0, "round")?.round()
    )));
    f!("abs", |_, _, _, a| Ok(match arg(a, 0) {
        Value::Int(i) => Value::Int(i.wrapping_abs()),
        v => Value::Float(v.as_float().unwrap_or(0.0).abs()),
    }));
    f!("min", |_, _, _, a| Ok(match (arg(a, 0), arg(a, 1)) {
        (Value::Int(x), Value::Int(y)) => Value::Int(*x.min(y)),
        _ => Value::Float(num(a, 0, "min")?.min(num(a, 1, "min")?)),
    }));
    f!("max", |_, _, _, a| Ok(match (arg(a, 0), arg(a, 1)) {
        (Value::Int(x), Value::Int(y)) => Value::Int(*x.max(y)),
        _ => Value::Float(num(a, 0, "max")?.max(num(a, 1, "max")?)),
    }));
    f!("clamp", |_, _, _, a| {
        let v = num(a, 0, "clamp")?;
        let lo = num(a, 1, "clamp")?;
        let hi = num(a, 2, "clamp")?;
        Ok(Value::Float(v.max(lo).min(hi)))
    });
    f!("isstring", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::Str(_)
    ))));
    f!("isistring", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::IStr(_)
    ))));
    f!("isint", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::Int(_)
    ))));
    f!("isfloat", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::Float(_)
    ))));
    f!("isarray", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::Array(_)
    ))));
    f!("isvec", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::Vec3(_)
    ))));
    f!("isfunctionptr", |_, _, _, a| Ok(Value::bool(matches!(
        arg(a, 0),
        Value::Func(_)
    ))));
    f!("issubstr", |vm, _, _, a| {
        let s = text(vm, a, 0);
        let sub = text(vm, a, 1);
        Ok(Value::bool(s.contains(&sub)))
    });
    f!("getsubstr", |vm, _, _, a| {
        let s: Vec<char> = text(vm, a, 0).chars().collect();
        let start = arg(a, 1).as_int().unwrap_or(0).clamp(0, s.len() as i32) as usize;
        let end = arg(a, 2)
            .as_int()
            .map_or(s.len(), |e| e.clamp(0, s.len() as i32) as usize);
        let out: String = if end > start {
            s[start..end].iter().collect()
        } else {
            String::new()
        };
        Ok(vm.string(&out))
    });
    f!("strtok", |vm, _, _, a| {
        let s = text(vm, a, 0);
        let delims = text(vm, a, 1);
        let parts: Vec<String> = s
            .split(|c| delims.contains(c))
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        let vals = parts.iter().map(|p| vm.string(p)).collect();
        Ok(list(vals))
    });
    f!("tolower", |vm, _, _, a| {
        let s = text(vm, a, 0).to_ascii_lowercase();
        Ok(vm.string(&s))
    });
    f!("toupper", |vm, _, _, a| {
        let s = text(vm, a, 0).to_ascii_uppercase();
        Ok(vm.string(&s))
    });
    f!("strstartswith", |vm, _, _, a| Ok(Value::bool(
        text(vm, a, 0).starts_with(&text(vm, a, 1))
    )));
    f!("strendswith", |vm, _, _, a| Ok(Value::bool(
        text(vm, a, 0).ends_with(&text(vm, a, 1))
    )));
    f!("spawnstruct", |vm, _, _, _| Ok(vm.new_struct()));
    f!("array", |_, _, _, a| Ok(list(a.to_vec())));
    f!("getarraykeys", |_, _, _, a| {
        let arr = array(a, 0, "getarraykeys")?;
        Ok(list(arr.keys().map(Key::value).collect()))
    });
    f!("getfirstarraykey", |_, _, _, a| {
        let arr = array(a, 0, "getfirstarraykey")?;
        Ok(arr.first_key().map(Key::value).unwrap_or_default())
    });
    f!("getnextarraykey", |vm, _, _, a| {
        let arr = array(a, 0, "getnextarraykey")?;
        let k = vm.to_key(arg(a, 1))?;
        Ok(arr.next_key(&k).map(Key::value).unwrap_or_default())
    });
    f!("getlastarraykey", |_, _, _, a| {
        let arr = array(a, 0, "getlastarraykey")?;
        Ok(arr.map.first().map(|(k, _)| k.value()).unwrap_or_default())
    });
    f!("arraycopy", |_, _, _, a| {
        let arr = array(a, 0, "arraycopy")?;
        Ok(Value::array(arr))
    });
    f!("isinarray", |vm, _, _, a| {
        let arr = array(a, 0, "isinarray")?;
        let v = arg(a, 1);
        Ok(Value::bool(arr.values_in_order().any(|x| vm.equal(x, v))))
    });
    f!("arraycombine", |vm, _, _, a| {
        let x = array(a, 0, "arraycombine")?;
        let y = array(a, 1, "arraycombine")?;
        let dupes = flag(a, 2, true);
        let keep_keys = flag(a, 3, false);
        let mut items: Vec<(Key, Value)> = x.map.iter().map(|(k, v)| (*k, v.clone())).collect();
        for (k, v) in &y.map {
            if !dupes && items.iter().any(|(_, w)| vm.equal(w, v)) {
                continue;
            }
            items.push((*k, v.clone()));
        }
        Ok(Value::array(renumber(items.into_iter(), keep_keys)))
    });
    // The engine's array edits change the array in place (scripts call
    // them without using the result) and return it.
    f!("arrayinsert", |_, _, _, a| {
        let Value::Array(rc) = arg(a, 0) else {
            return Err(format!(
                "arrayinsert: argument 1 is {}, not an array",
                arg(a, 0).type_name()
            ));
        };
        let arr = rc.snapshot();
        let v = arg(a, 1).clone();
        let at = arg(a, 2).as_int().unwrap_or(arr.len() as i32).max(0) as usize;
        let mut vals: Vec<Value> = arr.values_in_order().cloned().collect();
        vals.insert(at.min(vals.len()), v);
        let Value::Array(new) = list(vals) else {
            unreachable!()
        };
        *rc.write() = new.snapshot();
        Ok(Value::Array(rc.clone()))
    });
    f!("arrayremoveindex", |vm, _, _, a| {
        let Value::Array(rc) = arg(a, 0) else {
            return Err(format!(
                "arrayremoveindex: argument 1 is {}, not an array",
                arg(a, 0).type_name()
            ));
        };
        let arr = rc.snapshot();
        let k = vm.to_key(arg(a, 1))?;
        let keep_keys = flag(a, 2, false);
        let items = arr
            .map
            .iter()
            .filter(|(kk, _)| **kk != k)
            .map(|(k, v)| (*k, v.clone()));
        *rc.write() = renumber(items, keep_keys);
        Ok(Value::Array(rc.clone()))
    });
    f!("arrayremovevalue", |vm, _, _, a| {
        let Value::Array(rc) = arg(a, 0) else {
            return Err(format!(
                "arrayremovevalue: argument 1 is {}, not an array",
                arg(a, 0).type_name()
            ));
        };
        let arr = rc.snapshot();
        let v = arg(a, 1).clone();
        let keep_keys = flag(a, 2, false);
        let items: Vec<(Key, Value)> = arr
            .map
            .iter()
            .filter(|(_, w)| !vm.equal(w, &v))
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        *rc.write() = renumber(items.into_iter(), keep_keys);
        Ok(Value::Array(rc.clone()))
    });
    f!("arraysort", |vm, host, _, a| {
        let arr = array(a, 0, "arraysort")?;
        let origin = vec3(a, 1, "arraysort")?;
        let closest_first = flag(a, 2, true);
        let max = arg(a, 3).as_int().map(|n| n.max(0) as usize);
        let radius = arg(a, 4).as_float();
        let origin_s = vm.intern("origin");
        let mut items: Vec<(f32, Value)> = Vec::new();
        for v in arr.values_in_order() {
            let p = match v {
                Value::Object(o) => vm.get_field(host, *o, origin_s).as_vec3(),
                Value::Vec3(p) => Some(*p),
                _ => None,
            };
            let Some(p) = p else { continue };
            let d = math::length(math::sub(p, origin));
            if radius.is_some_and(|r| d > r) {
                continue;
            }
            items.push((d, v.clone()));
        }
        items.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        if !closest_first {
            items.reverse();
        }
        if let Some(m) = max {
            items.truncate(m);
        }
        Ok(list(items.into_iter().map(|(_, v)| v).collect()))
    });
    f!("getdvar", |vm, _, _, a| {
        let v = vm.dvar_text(arg(a, 0));
        Ok(vm.string(&v))
    });
    f!("getdvarint", |vm, _, _, a| Ok(Value::Int(math::parse_int(
        &vm.dvar_text(arg(a, 0))
    ))));
    f!("getdvarfloat", |vm, _, _, a| {
        Ok(Value::Float(
            vm.dvar_text(arg(a, 0)).trim().parse().unwrap_or(0.0),
        ))
    });
    f!("setdvar", |vm, _, _, a| {
        let n = vm.dvar_name(arg(a, 0));
        let v = text(vm, a, 1);
        vm.dvars.insert(n, v);
        Ok(Value::Undefined)
    });
    f!("gettime", |vm, _, _, _| Ok(Value::Int(vm.time_ms as i32)));
    for name in [
        "assert",
        "assertmsg",
        "assertex",
        "println",
        "print",
        "profilelog_begintiming",
        "profilelog_endtiming",
        "pixbeginevent",
        "pixendevent",
        "pixmarker",
        "incrementcounter",
        "bbprint",
        "adddebugcommand",
        "print3d",
        "line",
        "circle",
        "box",
        "sphere",
        "debugstar",
        "recordline",
        "recordsphere",
        "recordenttext",
    ] {
        vm.bind(name, false, |_, _, _, _| Ok(Value::Undefined));
    }
}
