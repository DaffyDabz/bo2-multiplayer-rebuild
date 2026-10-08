//! ZBarriers: a map entity made of pieces (the magic box: the fake box,
//! the box, its lid, the weapon riser and the teddy), each a model with
//! two animations from the entity's own keys (`zbarrierboardmodelN`,
//! `zbarriertearanimN` = opening, `zbarrierboardanimN` = closing). A
//! piece's state is "closed", "opening", "open" or "closing"; opening and
//! closing play their animation and end in open / closed. Scripts number
//! pieces from 0, the keys from 1.

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::{Zm, arg, entnum, list};

#[derive(Clone, Debug, Default)]
pub(crate) struct Piece {
    pub model: String,
    pub open_anim: String,
    pub close_anim: String,
    pub state: String,
    pub shown: bool,
    /// The playing animation: (clip, start ms, length ms).
    pub playing: Option<(String, i64, i64)>,
    /// The engine shows cycling weapons on this piece while it opens.
    pub box_rise: bool,
    /// While a box-rise piece opens: the box's guns it cycles through
    /// (world model, left-hand world model for two-gun pistols).
    pub rise: Vec<(String, Option<String>)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ZBarrier {
    pub pieces: Vec<Piece>,
}

impl ZBarrier {
    /// From the map entity's keys (lower-cased names).
    pub(crate) fn from_keys(keys: &[(String, String)]) -> Self {
        let get = |k: &str| {
            keys.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let n: usize = get("zbarriernumboards").trim().parse().unwrap_or(0);
        let pieces = (1..=n)
            .map(|i| Piece {
                model: get(&format!("zbarrierboardmodel{i}")),
                open_anim: get(&format!("zbarriertearanim{i}")).to_ascii_lowercase(),
                close_anim: get(&format!("zbarrierboardanim{i}")).to_ascii_lowercase(),
                state: "closed".to_owned(),
                shown: false,
                playing: None,
                box_rise: false,
                rise: Vec::new(),
            })
            .collect();
        Self { pieces }
    }
}

fn anim_ms(world: &World, anim: &str) -> i64 {
    world
        .resource::<Zm>()
        .anims
        .get(anim)
        .map_or(1000, super::actors::anim_length_ms)
}

/// Each tick: an opening or closing piece whose animation ended is open or
/// closed.
pub(crate) fn advance(world: &mut World, now: i64) {
    // IW4L_T6_ZBLOG: every 10 s, each zbarrier's shown pieces and states.
    if std::env::var("IW4L_T6_ZBLOG").is_ok() && now % 10_000 < i64::from(crate::MATCH_TICK_MS) {
        let zm = world.resource::<Zm>();
        for (n, e) in &zm.ents {
            let Some(z) = &e.zbarrier else { continue };
            let pieces: Vec<String> = z
                .pieces
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    format!(
                        "{i}:{}{}({})",
                        p.state,
                        if p.shown { "*" } else { "" },
                        p.model
                    )
                })
                .collect();
            diag::info!(
                Sim,
                "bo2zm t6 zbarrier ent{n} at ({:.0} {:.0} {:.0}): {}",
                e.origin[0],
                e.origin[1],
                e.origin[2],
                pieces.join(" ")
            );
        }
    }
    let mut zm = world.resource_mut::<Zm>();
    for e in zm.ents.values_mut() {
        let Some(z) = e.zbarrier.as_mut() else {
            continue;
        };
        for p in &mut z.pieces {
            let Some((_, start, len)) = &p.playing else {
                continue;
            };
            if now - start >= *len {
                match p.state.as_str() {
                    "opening" => p.state = "open".to_owned(),
                    "closing" => p.state = "closed".to_owned(),
                    _ => {}
                }
            }
        }
    }
}

/// What a piece shows: (model, clip, fraction, rate) at `now`.
pub(crate) fn piece_pose(p: &Piece, now: i64) -> (String, Option<(String, f32, f32)>) {
    let anim = p.playing.as_ref().map(|(clip, start, len)| {
        let frac = ((now - start) as f32 / (*len).max(1) as f32).clamp(0.0, 1.0);
        let rate = if frac >= 1.0 {
            0.0
        } else {
            1000.0 / (*len).max(1) as f32
        };
        (clip.clone(), frac, rate)
    });
    (p.model.clone(), anim)
}

/// The gun a rising box piece shows at `now` and where (its offset from
/// the box, in the box's frame), or none. BO2's engine spins the box's guns
/// on its weapon-rise pieces (the scripts' `zbarrierpieceuseboxriselogic`)
/// while they open: the piece's opening clip (o_zombie_magic_box_weapon_rise,
/// 3.9 s) carries the rise in its root motion and a "switch" note at each
/// change of gun (fast, then slower: the server's own 0.05 / 0.1 / 0.2 /
/// 0.3 s waits before it picks). The two-gun piece (the dual rise) shows
/// only the left gun of a two-gun pistol.
pub(crate) fn rise_gun(
    p: &Piece,
    anim: Option<&super::T6Anim>,
    now: i64,
) -> Option<(String, [f32; 3])> {
    let (_, start, len) = p.playing.as_ref()?;
    if !p.box_rise || !p.shown || p.state != "opening" || p.rise.is_empty() {
        return None;
    }
    let frac = ((now - start) as f32 / (*len).max(1) as f32).clamp(0.0, 1.0);
    let step = anim.map_or(0, |a| {
        a.notifies
            .iter()
            .filter(|(n, t)| n == "switch" && *t <= frac)
            .count()
    });
    // A fixed shuffle per opening (its start time), so every tick of one
    // step agrees.
    let mut h = (*start as u64) ^ (step as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    let (gun, left) = &p.rise[(h % p.rise.len() as u64) as usize];
    let model = if p.open_anim.contains("dual") {
        left.clone()?
    } else {
        gun.clone()
    };
    // The root motion at this frame (keys by frame, straight between).
    let offset = anim.map_or([0.0, 0.0, 40.0 * frac], |a| {
        let f = frac * f32::from(a.numframes);
        let keys = &a.delta_trans;
        let after = keys.iter().position(|(k, _)| f32::from(*k) >= f);
        match after {
            None => keys.last().map_or([0.0; 3], |k| k.1),
            Some(0) => keys.first().map_or([0.0; 3], |k| k.1),
            Some(i) => {
                let ((k0, a0), (k1, a1)) = (keys[i - 1], keys[i]);
                let t = (f - f32::from(k0)) / f32::from(k1.saturating_sub(k0).max(1));
                std::array::from_fn(|c| a0[c] + (a1[c] - a0[c]) * t)
            }
        }
    });
    Some((model, offset))
}

/// The box's guns (`level.zombie_weapons` marked `is_in_box`): world
/// models, and the left gun's for a two-gun pistol.
fn box_guns(vm: &mut Vm<World>, world: &mut World) -> Vec<(String, Option<String>)> {
    let (all_f, in_box_f) = (vm.intern("zombie_weapons"), vm.intern("is_in_box"));
    let Value::Array(all) = vm.raw_field(vm.level, all_f) else {
        return Vec::new();
    };
    let all = all.snapshot();
    let names: Vec<String> = all
        .keys()
        .filter_map(|k| {
            let gsc_t6::Key::Str(name) = k else {
                return None;
            };
            let Some(Value::Object(o)) = all.get(&k) else {
                return None;
            };
            vm.raw_field(*o, in_box_f)
                .as_int()
                .is_some_and(|v| v != 0)
                .then(|| vm.str(name).to_owned())
        })
        .collect();
    let mut out = Vec::new();
    for name in names {
        let Ok(w) = super::weapon(world, &name) else {
            continue;
        };
        let f = super::frame(world);
        let Some(model) = f.weapon_world_model(w).map(|(m, _)| m.to_owned()) else {
            continue;
        };
        let left = f.combat_facts_for(w).map_or(0, |c| c.dual_wield_weapon);
        let left = (left != 0)
            .then(|| f.weapon_world_model(left).map(|(m, _)| m.to_owned()))
            .flatten();
        if !model.is_empty() {
            out.push((model, left));
        }
    }
    out
}

fn with_piece<R>(
    vm: &Vm<World>,
    world: &mut World,
    s: &Value,
    a: &[Value],
    f: impl FnOnce(&mut Piece) -> R,
) -> Option<R> {
    let n = entnum(vm, s)?;
    let i = arg(a, 0).as_int()?;
    let mut zm = world.resource_mut::<Zm>();
    let z = zm.ents.get_mut(&n)?.zbarrier.as_mut()?;
    z.pieces.get_mut(usize::try_from(i).ok()?).map(f)
}

pub(super) fn bind(vm: &mut Vm<World>) {
    macro_rules! m {
        ($name:literal, $body:expr) => {
            vm.bind($name, true, $body);
        };
    }
    m!("iszbarrier", |vm, world, s, _| {
        let is = entnum(vm, s).is_some_and(|n| {
            world
                .resource::<Zm>()
                .ents
                .get(&n)
                .is_some_and(|e| e.zbarrier.is_some())
        });
        Ok(Value::bool(is))
    });
    m!("getnumzbarrierpieces", |vm, world, s, _| {
        let n = entnum(vm, s)
            .and_then(|n| {
                world
                    .resource::<Zm>()
                    .ents
                    .get(&n)
                    .and_then(|e| e.zbarrier.as_ref().map(|z| z.pieces.len()))
            })
            .unwrap_or(0);
        Ok(Value::Int(n as i32))
    });
    m!("setzbarrierpiecestate", |vm, world, s, a| {
        let state = vm.to_text(arg(a, 1));
        let now = world.resource::<Zm>().now_ms;
        let anims = with_piece(vm, world, s, a, |p| {
            (p.open_anim.clone(), p.close_anim.clone())
        });
        let Some((open, close)) = anims else {
            return Ok(Value::Undefined);
        };
        let (open_ms, close_ms) = (anim_ms(world, &open), anim_ms(world, &close));
        let rises = state == "opening" && with_piece(vm, world, s, a, |p| p.box_rise) == Some(true);
        let guns = if rises {
            box_guns(vm, world)
        } else {
            Vec::new()
        };
        if rises {
            diag::info!(
                Sim,
                "bo2zm t6 box spin: {} guns {:?}",
                guns.len(),
                guns.iter().take(4).collect::<Vec<_>>()
            );
        }
        with_piece(vm, world, s, a, |p| {
            if rises {
                p.rise = guns;
            }
            match state.as_str() {
                "opening" => p.playing = Some((open, now, open_ms)),
                "closing" => p.playing = Some((close, now, close_ms)),
                // Settled states show the end of the animation that led there.
                "open" => p.playing = Some((open, now - open_ms, open_ms)),
                "closed" => p.playing = Some((close, now - close_ms, close_ms)),
                _ => {}
            }
            p.state = state;
        });
        Ok(Value::Undefined)
    });
    m!("getzbarrierpiecestate", |vm, world, s, a| {
        let st = with_piece(vm, world, s, a, |p| p.state.clone()).unwrap_or_default();
        Ok(vm.string(&st))
    });
    m!("showzbarrierpiece", |vm, world, s, a| {
        with_piece(vm, world, s, a, |p| p.shown = true);
        Ok(Value::Undefined)
    });
    m!("hidezbarrierpiece", |vm, world, s, a| {
        with_piece(vm, world, s, a, |p| p.shown = false);
        Ok(Value::Undefined)
    });
    m!("zbarrierpieceuseboxriselogic", |vm, world, s, a| {
        with_piece(vm, world, s, a, |p| p.box_rise = true);
        Ok(Value::Undefined)
    });
    m!("getzbarrierpieceindicesinstate", |vm, world, s, a| {
        let want = vm.to_text(arg(a, 0));
        let idx: Vec<Value> = entnum(vm, s)
            .and_then(|n| {
                world.resource::<Zm>().ents.get(&n).and_then(|e| {
                    e.zbarrier.as_ref().map(|z| {
                        z.pieces
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| p.state == want)
                            .map(|(i, _)| Value::Int(i as i32))
                            .collect()
                    })
                })
            })
            .unwrap_or_default();
        Ok(list(idx))
    });
    m!("getzbarrierpieceanimlengthforstate", |vm, world, s, a| {
        let state = vm.to_text(arg(a, 1));
        let anims = with_piece(vm, world, s, a, |p| {
            (p.open_anim.clone(), p.close_anim.clone())
        });
        let Some((open, close)) = anims else {
            return Ok(Value::Float(0.0));
        };
        let anim = if state.starts_with("clos") {
            close
        } else {
            open
        };
        Ok(Value::Float(anim_ms(world, &anim) as f32 / 1000.0))
    });
    for name in [
        "zbarrierpieceusedefaultmodel",
        "zbarrierpieceusealternatemodel",
        "zbarrierpieceuseupgradedmodel",
        "setzbarriercolmodel",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    for name in [
        "zbarriersupportszombietaunts",
        "zbarriersupportszombiereachthroughattacks",
        "getzbarriernumattackslots",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Int(0)));
    }
    m!("getzbarrierattackslothorzoffset", |_, _, _, _| Ok(
        Value::Float(0.0)
    ));
    for name in [
        "getzbarriertauntanimstate",
        "getzbarrierreachthroughattackanimstate",
        "getzbarrierpieceanimstate",
        "getzbarrierpieceanimsubstate",
    ] {
        vm.bind(name, true, |vm, _, _, _| Ok(vm.string("")));
    }
}
