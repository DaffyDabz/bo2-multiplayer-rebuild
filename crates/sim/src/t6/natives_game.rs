//! Game-wide builtins: tables, session and game-mode queries, precache,
//! players lists, HUD element stand-ins, traces, and quiet stubs for what
//! has no engine side yet (client fields, effects, sounds) - those are
//! listed once in the log by `unbound` when a script reaches one not here.

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;
use gsc_t6::{ObjKind, ObjRef, Value, Vm};

use super::{Zm, arg, entnum, frame, int, is_player, list, text, vec3};
use crate::world::ClientId;

/// Notify `name` on `obj` after `ms` (server time).
pub(super) fn notify_later(
    _vm: &mut Vm<World>,
    world: &mut World,
    obj: ObjRef,
    name: &str,
    ms: i64,
) {
    let mut zm = world.resource_mut::<Zm>();
    let at = zm.now_ms + ms;
    zm.timers.push((at, obj, name.to_owned()));
}

/// Deliver due `notify_later`s.
pub(super) fn deliver_timers(vm: &mut Vm<World>, world: &mut World, now: i64) {
    let due: Vec<(i64, ObjRef, String)> = {
        let mut zm = world.resource_mut::<Zm>();
        let (due, keep): (Vec<_>, Vec<_>) = zm.timers.drain(..).partition(|t| t.0 <= now);
        zm.timers = keep;
        due
    };
    for (_, o, n) in due {
        if vm.alive(o) {
            vm.notify_str(world, o, &n, &[]);
        }
    }
    let replies: Vec<(i64, ObjRef, String, String)> = {
        let mut zm = world.resource_mut::<Zm>();
        let (due, keep): (Vec<_>, Vec<_>) = zm.menu_replies.drain(..).partition(|t| t.0 <= now);
        zm.menu_replies = keep;
        due
    };
    for (_, o, menu, response) in replies {
        if vm.alive(o) {
            let (m, r) = (vm.string(&menu), vm.string(&response));
            diag::info!(Sim, "bo2mp menu response (test aid): {menu} {response}");
            vm.notify_str(world, o, "menuresponse", &[m, r]);
        }
    }
}

fn table_cell(world: &World, table: &str, row: usize, col: usize) -> Option<String> {
    let zm = world.resource::<Zm>();
    let t = zm.tables.get(&table.to_ascii_lowercase())?;
    if row >= t.rows || col >= t.columns {
        return None;
    }
    t.cells.get(row * t.columns + col).cloned()
}

fn table_find(world: &World, table: &str, col: usize, key: &str) -> Option<usize> {
    let zm = world.resource::<Zm>();
    let t = zm.tables.get(&table.to_ascii_lowercase())?;
    (0..t.rows).find(|&r| {
        t.cells
            .get(r * t.columns + col)
            .is_some_and(|c| c.eq_ignore_ascii_case(key))
    })
}

/// bo2mp: a multiplayer game type setting from the settings files; one
/// they do not name is 0 (as the engine's default for an unset setting).
/// BO2MP_SETTING_<name>=<value> overrides one (a test aid: a short time
/// limit, a low score limit).
fn mp_setting(settings: &BTreeMap<String, String>, name: &str) -> Value {
    let key = name.to_ascii_lowercase();
    let over = std::env::var(format!("BO2MP_SETTING_{key}")).ok();
    let raw = over.as_deref().or(settings.get(&key).map(String::as_str));
    match raw {
        Some(v) if v.contains('.') => Value::Float(v.parse().unwrap_or(0.0)),
        Some(v) => Value::Int(v.parse().unwrap_or(0)),
        None => Value::Int(0),
    }
}

/// Game type settings Zombies reads (`getgametypesetting`); others are 0.
fn gametype_setting(name: &str) -> Value {
    match name.to_ascii_lowercase().as_str() {
        "teamcount" => Value::Int(1),
        "roundlimit" => Value::Int(1),
        "playernumlives" => Value::Int(0),
        "allowhitmarkers" => Value::Int(1),
        "allowfinalkillcam" => Value::Int(0),
        // IW4L_T6_START_ROUND=<n>: start there (a test aid: runners and
        // sprinters at once).
        "startround" => Value::Int(
            std::env::var("IW4L_T6_START_ROUND")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        ),
        "zmdifficulty" => Value::Int(1),
        "magic" => Value::Int(1),
        // His zm/gamesettings_zstandard.cfg (and _zclassic): no hellhound
        // rounds in Nuketown (the scripts' dog AI never ran here: dog rounds
        // stood two still dogs far off for minutes).
        "allowdogs" => Value::Int(0),
        "headshotsonly" => Value::Int(0),
        "allowpowerups" => Value::Int(1),
        "cleansedloadout" => Value::Int(0),
        _ => Value::Int(0),
    }
}

/// Path nodes whose `key` (targetname by default) is `value`.
fn nodes_matching(vm: &mut Vm<World>, world: &mut World, a: &[Value]) -> Vec<Value> {
    let want = text(vm, a, 0);
    let key = match arg(a, 1) {
        Value::Undefined => "targetname".to_owned(),
        v => vm.to_text(v).to_ascii_lowercase(),
    };
    let k = vm.intern(&key);
    let objs: Vec<ObjRef> = world.resource::<Zm>().node_objs.clone();
    objs.into_iter()
        .filter(|o| matches!(vm.raw_field(*o, k), Value::Str(s) if vm.str(s) == want))
        .map(Value::Object)
        .collect()
}

/// Vehicle path nodes (`info_vehicle_node`) whose `key` is `value`.
fn vehicle_nodes(vm: &mut Vm<World>, world: &mut World, a: &[Value]) -> Vec<Value> {
    let want = text(vm, a, 0);
    let key = match arg(a, 1) {
        Value::Undefined => "targetname".to_owned(),
        v => vm.to_text(v).to_ascii_lowercase(),
    };
    let k = vm.intern(&key);
    let nodes: Vec<ObjRef> = world
        .resource::<Zm>()
        .ents
        .values()
        .filter(|e| e.classname.starts_with("info_vehicle_node"))
        .filter_map(|e| e.obj)
        .collect();
    nodes
        .into_iter()
        .filter(|o| matches!(vm.raw_field(*o, k), Value::Str(s) if vm.str(s) == want))
        .map(Value::Object)
        .collect()
}

pub(super) fn bind(vm: &mut Vm<World>) {
    macro_rules! f {
        ($name:literal, $body:expr) => {
            vm.bind($name, false, $body);
        };
    }
    macro_rules! m {
        ($name:literal, $body:expr) => {
            vm.bind($name, true, $body);
        };
    }
    f!("tablelookup", |vm, world, _, a| {
        let table = text(vm, a, 0);
        let col = int(a, 1)? as usize;
        let key = text(vm, a, 2);
        let out = int(a, 3)? as usize;
        let v = table_find(world, &table, col, &key)
            .and_then(|r| table_cell(world, &table, r, out))
            .unwrap_or_default();
        Ok(vm.string(&v))
    });
    f!("tablelookupistring", |vm, world, _, a| {
        let table = text(vm, a, 0);
        let col = int(a, 1)? as usize;
        let key = text(vm, a, 2);
        let out = int(a, 3)? as usize;
        let v = table_find(world, &table, col, &key)
            .and_then(|r| table_cell(world, &table, r, out))
            .unwrap_or_default();
        Ok(Value::IStr(vm.intern(&v)))
    });
    f!("tablelookuprownum", |vm, world, _, a| {
        let table = text(vm, a, 0);
        let col = int(a, 1)? as usize;
        let key = text(vm, a, 2);
        Ok(Value::Int(
            table_find(world, &table, col, &key).map_or(-1, |r| r as i32),
        ))
    });
    f!("tablelookupcolumnforrow", |vm, world, _, a| {
        let table = text(vm, a, 0);
        let row = int(a, 1)? as usize;
        let col = int(a, 2)? as usize;
        let v = table_cell(world, &table, row, col).unwrap_or_default();
        Ok(vm.string(&v))
    });
    f!("tablelookuprowcount", |vm, world, _, a| {
        let table = text(vm, a, 0).to_ascii_lowercase();
        Ok(Value::Int(
            world
                .resource::<Zm>()
                .tables
                .get(&table)
                .map_or(0, |t| t.rows as i32),
        ))
    });
    f!("getgametypesetting", |vm, world, _, a| {
        let name = text(vm, a, 0);
        // bo2mp: a multiplayer game type's own settings (his files'
        // mp/gamesettings_default.cfg then mp/gamesettings_<type>.cfg).
        let zm = world.resource::<Zm>();
        if zm.mp {
            return Ok(mp_setting(&zm.gamesettings, &name));
        }
        Ok(gametype_setting(&name))
    });
    f!("setgametypesetting", |_, _, _, _| Ok(Value::Undefined));
    f!("sessionmodeisonlinegame", |_, _, _, _| Ok(Value::Int(0)));
    f!("sessionmodeisprivate", |_, _, _, _| Ok(Value::Int(0)));
    f!("sessionmodeisprivateonlinegame", |_, _, _, _| Ok(
        Value::Int(0)
    ));
    f!("sessionmodeissystemlink", |_, _, _, _| Ok(Value::Int(0)));
    f!("sessionmodeiszombiesgame", |_, world, _, _| Ok(Value::Int(
        i32::from(!world.resource::<Zm>().mp)
    )));
    f!("sessionmodeiscampaigngame", |_, _, _, _| Ok(Value::Int(0)));
    f!("gamemodeisusingxp", |_, _, _, _| Ok(Value::Int(0)));
    f!("gamemodeisusingstats", |_, _, _, _| Ok(Value::Int(0)));
    f!("gamemodeismode", |_, _, _, _| Ok(Value::Int(0)));
    f!("ispregame", |_, _, _, _| Ok(Value::Int(0)));
    // Zombies runs on the multiplayer side (_createfx's fx_init sets up
    // the exploders only there).
    f!("ismp", |_, _, _, _| Ok(Value::Int(1)));
    f!("issplitscreen", |_, _, _, _| Ok(Value::Int(0)));
    f!("isonlinegame", |_, _, _, _| Ok(Value::Int(0)));
    f!("getnumexpectedplayers", |_, _, _, _| Ok(Value::Int(1)));
    f!("getnumconnectedplayers", |_, world, _, _| Ok(Value::Int(
        world.resource::<Zm>().players.len() as i32
    )));
    f!("getlocalprofileint", |_, _, _, _| Ok(Value::Int(0)));
    f!("hasdlcavailable", |_, _, _, _| Ok(Value::Int(1)));
    f!("getnorthyaw", |_, _, _, _| Ok(Value::Float(0.0)));
    f!("getaitriggerflags", |_, _, _, _| Ok(Value::Int(2)));
    f!("getvehicletriggerflags", |_, _, _, _| Ok(Value::Int(4)));
    f!("getfreeactorcount", |_, _, _, _| Ok(Value::Int(32)));
    f!("aretexturesloaded", |_, _, _, _| Ok(Value::Int(1)));
    f!("getutc", |_, _, _, _| {
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Ok(Value::Int(s as i32))
    });
    f!("getminbitcountfornum", |_, _, _, a| {
        let n = int(a, 0)?.max(0) as u32;
        Ok(Value::Int((32 - n.leading_zeros()) as i32))
    });
    f!("loadfx", |vm, world, _, a| {
        let name = text(vm, a, 0);
        let mut zm = world.resource_mut::<Zm>();
        if let Some(i) = zm.fx.iter().position(|f| *f == name) {
            return Ok(Value::Int(i as i32 + 1));
        }
        zm.fx.push(name);
        Ok(Value::Int(zm.fx.len() as i32))
    });
    f!("getplayers", |_, world, _, _| {
        let ps: Vec<Value> = world
            .resource::<Zm>()
            .players
            .values()
            .map(|p| Value::Object(p.obj))
            .collect();
        Ok(list(ps))
    });
    f!("isplayer", |vm, world, _, a| Ok(Value::bool(is_player(
        vm,
        world,
        arg(a, 0)
    ))));
    f!("isai", |_, _, _, _| Ok(Value::Int(0)));
    f!("isalive", |vm, world, _, a| {
        let v = arg(a, 0);
        if !vm.is_defined(v) {
            return Ok(Value::Int(0));
        }
        if let Some(n) = entnum(vm, v)
            && let Some(p) = world.resource::<Zm>().players.get(&n).cloned()
        {
            let h = frame(world).player(ClientId(n)).map_or(0, |ps| ps.health);
            return Ok(Value::bool(p.sessionstate == "playing" && h > 0));
        }
        Ok(Value::bool(entnum(vm, v).is_some()))
    });
    f!("getwatcherweapons", |_, _, _, _| Ok(list(Vec::new())));
    // Weapons a player picks back up (their weapon file's bRetrievable),
    // as `<name>_mp` (BO2's _weaponobjects strips the suffix itself).
    f!("getretrievableweapons", |vm, world, _, _| {
        let names = world.resource::<Zm>().retrievable_weapons.clone();
        let v = names.iter().map(|n| vm.string(n)).collect();
        Ok(list(v))
    });
    f!("getplayerspawnid", |vm, _, _, a| Ok(
        entnum(vm, arg(a, 0)).map_or(Value::Int(0), |n| Value::Int(n as i32))
    ));
    f!("getweaponindexfromname", |vm, world, _, a| {
        let n = text(vm, a, 0);
        Ok(Value::Int(super::weapon(world, &n).map_or(0, |w| w as i32)))
    });
    for name in [
        "getweaponfiresound",
        "getweaponfiresoundplayer",
        "getweaponpickupsound",
        "getweaponpickupsoundplayer",
    ] {
        vm.bind(name, false, |vm, _, _, _| Ok(vm.string("")));
    }
    f!("iprintln", |vm, _, _, a| {
        let t = text(vm, a, 0);
        diag::info!(Sim, "bo2zm t6 iprintln: {t}");
        Ok(Value::Undefined)
    });
    f!("iprintlnbold", |vm, _, _, a| {
        let t = text(vm, a, 0);
        diag::info!(Sim, "bo2zm t6 iprintlnbold: {t}");
        Ok(Value::Undefined)
    });
    for method in [false, true] {
        vm.bind("blank", method, |_, _, _, _| Ok(Value::Undefined));
        vm.bind("issplitscreen", method, |_, _, _, _| Ok(Value::Int(0)));
    }
    for name in [
        "setspawnpointrandomvariation",
        "setentertime",
        "setclientcompass",
        "setclienthudhardcore",
        "setclientplayerpushamount",
        "setclientaimlockonpitchstrength",
        "setclientplayersprinttime",
        "setclientnumlives",
        "disabledeathstreak",
    ] {
        vm.bind(name, false, |_, _, _, _| Ok(Value::Undefined));
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    f!("getvehiclenode", |vm, world, _, a| {
        let v = vehicle_nodes(vm, world, a);
        Ok(v.into_iter().next().unwrap_or_default())
    });
    f!("getvehiclenodearray", |vm, world, _, a| {
        let v = vehicle_nodes(vm, world, a);
        Ok(list(v))
    });
    f!("weaponfuellife", |_, _, _, _| Ok(Value::Int(0)));
    f!("numremoteclients", |_, _, _, _| Ok(Value::Int(0)));
    f!("addsphereinfluencer", |_, _, _, _| Ok(Value::Int(1)));
    f!("addcylinderinfluencer", |_, _, _, _| Ok(Value::Int(1)));
    f!("addboxinfluencer", |_, _, _, _| Ok(Value::Int(1)));
    for name in [
        "adddemobookmark",
        "setinitialplayersconnected",
        "removeinfluencer",
        "enableinfluencer",
        "addinfluencerspawnpoints",
        "clearspawnpoints",
        "setenableinfluencer",
        "spawnpointsetrandomvariation",
    ] {
        vm.bind(name, false, |_, _, _, _| Ok(Value::Undefined));
    }
    for name in [
        "stopburning",
        "playerknockback",
        "cameraactivate",
        "camerasetposition",
        "camerasetlookat",
        "logstring",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    vm.bind("enableusability", true, |vm, world, s, _| {
        if let Ok(id) = super::client(vm, world, s) {
            let mut f = frame(world);
            if f.client_meta(id).is_some() {
                f.client_meta_mut(id).controls.usability_disabled = false;
            }
        }
        Ok(Value::Undefined)
    });
    vm.bind("disableusability", true, |vm, world, s, _| {
        if let Ok(id) = super::client(vm, world, s) {
            let mut f = frame(world);
            if f.client_meta(id).is_some() {
                f.client_meta_mut(id).controls.usability_disabled = true;
            }
        }
        Ok(Value::Undefined)
    });
    f!("getnode", |vm, world, _, a| {
        let v = nodes_matching(vm, world, a);
        Ok(v.into_iter().next().unwrap_or_default())
    });
    f!("getnodearray", |vm, world, _, a| {
        let v = nodes_matching(vm, world, a);
        Ok(list(v))
    });
    f!("getallnodes", |_, world, _, _| {
        let v = world
            .resource::<Zm>()
            .node_objs
            .iter()
            .map(|o| Value::Object(*o))
            .collect();
        Ok(list(v))
    });
    for name in ["getnodesinradius", "getnodesinradiussorted"] {
        vm.bind(name, false, |_, world, _, a| {
            let center = arg(a, 0).as_vec3().unwrap_or([0.0; 3]);
            let radius = arg(a, 1).as_float().unwrap_or(0.0);
            let min = arg(a, 2).as_float().unwrap_or(0.0);
            let height = arg(a, 3).as_float();
            let zm = world.resource::<Zm>();
            let mut hits: Vec<(f32, ObjRef)> = zm
                .nav
                .nodes
                .iter()
                .zip(&zm.node_objs)
                .filter_map(|(n, o)| {
                    let d = gsc_t6::math::sub(n.origin, center);
                    let d2 = (d[0] * d[0] + d[1] * d[1]).sqrt();
                    let ok = d2 <= radius && d2 >= min && height.is_none_or(|h| d[2].abs() <= h);
                    ok.then_some((d2, *o))
                })
                .collect();
            hits.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
            Ok(list(
                hits.into_iter().map(|(_, o)| Value::Object(o)).collect(),
            ))
        });
    }
    // bo2zm M3: weapon facts the scripts ask (class, fire type).
    f!("weaponclass", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let class = super::frame(world)
            .combat_facts_for(w)
            .map_or(-1, |f| f.weap_class);
        let name = match class {
            0 => "rifle",
            1 => "mg",
            2 => "smg",
            3 => "spread",
            4 => "spread",
            5 => "pistol",
            6 => "grenade",
            7 => "rocketlauncher",
            8 => "turret",
            9 => "non-player",
            10 => "item",
            _ => "none",
        };
        Ok(vm.string(name))
    });
    // A weapon's world model (the box's floating gun, a wall buy's).
    f!("getweaponmodel", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let model = super::frame(world)
            .weapon_world_model(w)
            .map(|(m, _)| m.to_owned())
            .unwrap_or_default();
        Ok(vm.string(&model))
    });
    // Weapon facts the scripts ask about a gun by name.
    fn facts(
        vm: &Vm<World>,
        world: &mut World,
        a: &[Value],
    ) -> Option<weapon_iw4::WeaponCombatFacts> {
        let w = super::weapon(world, &text(vm, a, 0)).ok()?;
        super::frame(world).combat_facts_for(w)
    }
    f!("isweaponprimary", |vm, world, _, a| {
        Ok(Value::bool(
            facts(vm, world, a).is_some_and(|f| f.inventory_type == 0),
        ))
    });
    f!("isweaponequipment", |vm, world, _, a| {
        Ok(Value::bool(
            facts(vm, world, a).is_some_and(|f| f.inventory_type == 2),
        ))
    });
    f!("weaponclipsize", |vm, world, _, a| {
        Ok(Value::Int(facts(vm, world, a).map_or(0, |f| f.clip_size)))
    });
    f!("weaponmaxammo", |vm, world, _, a| {
        Ok(Value::Int(facts(vm, world, a).map_or(0, |f| f.max_ammo)))
    });
    f!("weaponstartammo", |vm, world, _, a| {
        Ok(Value::Int(facts(vm, world, a).map_or(0, |f| f.start_ammo)))
    });
    f!("weaponfiretime", |vm, world, _, a| {
        Ok(Value::Float(
            facts(vm, world, a).map_or(0.0, |f| f.fire_time_ms as f32 / 1000.0),
        ))
    });
    f!("weaponreloadtime", |vm, world, _, a| {
        Ok(Value::Float(
            facts(vm, world, a).map_or(0.0, |f| f.reload_time_ms as f32 / 1000.0),
        ))
    });
    f!("weaponisboltaction", |vm, world, _, a| {
        Ok(Value::bool(
            facts(vm, world, a).is_some_and(|f| f.bolt_action),
        ))
    });
    f!("weaponaltweaponname", |vm, world, _, a| {
        let alt = facts(vm, world, a).map_or(0, |f| f.alternate_weapon);
        let name = if alt == 0 {
            "none".to_owned()
        } else {
            super::weapon_text(world, alt)
        };
        Ok(vm.string(&name))
    });
    // A Black Ops II dual-wield weapon's left-hand weapon ("none").
    f!("weapondualwieldweaponname", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let left = frame(world)
            .combat_facts_for(w)
            .map_or(0, |f| f.dual_wield_weapon);
        if left == 0 {
            return Ok(vm.string("none"));
        }
        let name = super::weapon_text(world, left);
        Ok(vm.string(&name))
    });
    f!("weaponisgasweapon", |_, _, _, _| Ok(Value::Int(0)));
    f!("weapontype", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let ty = super::frame(world)
            .combat_facts_for(w)
            .map_or(-1, |f| f.weap_type);
        let name = match ty {
            0 => "bullet",
            1 => "grenade",
            2 => "projectile",
            3 => "riotshield",
            _ => "none",
        };
        Ok(vm.string(name))
    });
    f!("weaponinventorytype", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let ty = super::frame(world)
            .combat_facts_for(w)
            .map_or(-1, |f| f.inventory_type);
        let name = match ty {
            0 => "primary",
            1 => "offhand",
            2 => "item",
            3 => "altmode",
            _ => "none",
        };
        Ok(vm.string(name))
    });
    f!("weaponissemiauto", |vm, world, _, a| {
        let w = super::weapon(world, &text(vm, a, 0)).unwrap_or(0);
        let semi = super::frame(world)
            .combat_facts_for(w)
            .is_some_and(|f| f.fire_type == 1);
        Ok(Value::bool(semi))
    });
    f!("setroundsplayed", |vm, world, _, a| {
        let n = arg(a, 0).as_int().unwrap_or(0);
        world.resource_mut::<Zm>().rounds_played = n;
        let _ = vm;
        Ok(Value::Undefined)
    });
    for name in [
        "reportmtu",
        "recordzombieroundstart",
        "recordzombieroundend",
        "stopallrumbles",
        "recordmatchsummaryzombieendgamedata",
        "recordplayermatchend",
        "recordzombiezone",
        "uploadstats",
        "setmatchtalkflag",
    ] {
        vm.bind(name, false, |_, _, _, _| Ok(Value::Undefined));
    }
    // HUD elements start with the engine's defaults (scripts do
    // `elem.y = elem.y - 130`).
    fn hud(vm: &mut Vm<World>) -> Value {
        let o = vm.alloc_object(ObjKind::HudElem(0));
        for (k, v) in [
            ("x", Value::Int(0)),
            ("y", Value::Int(0)),
            ("z", Value::Int(0)),
            ("alpha", Value::Float(1.0)),
            ("fontscale", Value::Float(1.0)),
            ("sort", Value::Int(0)),
            ("color", Value::Vec3([1.0, 1.0, 1.0])),
            ("glowalpha", Value::Float(0.0)),
            ("glowcolor", Value::Vec3([0.0, 0.0, 0.0])),
            ("width", Value::Int(0)),
            ("height", Value::Int(0)),
        ] {
            let f = vm.intern(k);
            vm.set_raw_field(o, f, v);
        }
        Value::Object(o)
    }
    for name in [
        "newhudelem",
        "newclienthudelem",
        "newteamhudelem",
        "newscorehudelem",
        "newdebughudelem",
        "newdamageindicatorhudelem",
    ] {
        vm.bind(name, false, |vm, _, _, _| Ok(hud(vm)));
    }
    m!("destroy", |vm, world, s, _| {
        if let Some(o) = s.as_obj()
            && matches!(vm.kind(o), Some(ObjKind::HudElem(_)))
        {
            vm.free_object(world, o);
        }
        Ok(Value::Undefined)
    });
    for name in [
        "settext",
        "setvalue",
        "setshader",
        "settimer",
        "settimerup",
        "settenthstimer",
        "settenthstimerup",
        "setclock",
        "setclockup",
        "fadeovertime",
        "moveovertime",
        "scaleovertime",
        "changefontscaleovertime",
        "setwaypoint",
        "settargetent",
        "cleartargetent",
        "setpulsefx",
        "setcod7decodefx",
        "setredeemfx",
        "reset",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    // Traces against the map (entities aside): bullettrace and
    // groundtrace give the engine's result array, the physics traces the
    // stopping point, the "passed" ones whether nothing was in the way.
    fn world_trace(
        world: &mut World,
        start: [f32; 3],
        end: [f32; 3],
        mask: u32,
    ) -> (f32, [f32; 3], [f32; 3]) {
        let t = frame(world).trace_static_world(start, end, [0.0; 3], [0.0; 3], mask);
        let f = t.fraction.clamp(0.0, 1.0);
        let pos = std::array::from_fn(|i| start[i] + (end[i] - start[i]) * f);
        (f, pos, t.normal)
    }
    fn trace_array(
        vm: &mut Vm<World>,
        (fraction, position, normal): (f32, [f32; 3], [f32; 3]),
    ) -> Value {
        let mut arr = gsc_t6::Array::new();
        let k = |vm: &mut Vm<World>, s: &str| gsc_t6::Key::Str(vm.intern(s));
        let surface = vm.string(if fraction < 1.0 { "default" } else { "none" });
        arr.set(k(vm, "fraction"), Value::Float(fraction));
        arr.set(k(vm, "position"), Value::Vec3(position));
        arr.set(
            k(vm, "normal"),
            Value::Vec3(if fraction < 1.0 { normal } else { [0.0; 3] }),
        );
        arr.set(k(vm, "surfacetype"), surface);
        Value::array(arr)
    }
    const SHOT: u32 = crate::bullet_collision::MASK_SHOT;
    const SOLID: u32 = crate::bullet_collision::MASK_PLAYER_SOLID;
    f!("bullettrace", |vm, world, _, a| {
        let (s, e) = (vec3(a, 0)?, vec3(a, 1)?);
        let r = world_trace(world, s, e, SHOT);
        Ok(trace_array(vm, r))
    });
    f!("groundtrace", |vm, world, _, a| {
        let (s, e) = (vec3(a, 0)?, vec3(a, 1)?);
        let r = world_trace(world, s, e, SOLID);
        Ok(trace_array(vm, r))
    });
    f!("bullettracepassed", |_, world, _, a| {
        let (s, e) = (vec3(a, 0)?, vec3(a, 1)?);
        Ok(Value::bool(world_trace(world, s, e, SHOT).0 >= 1.0))
    });
    f!("sighttracepassed", |_, world, _, a| {
        let (s, e) = (vec3(a, 0)?, vec3(a, 1)?);
        Ok(Value::bool(world_trace(world, s, e, SHOT).0 >= 1.0))
    });
    f!("physicstrace", |_, world, _, a| {
        let (s, e) = (vec3(a, 0)?, vec3(a, 1)?);
        Ok(Value::Vec3(world_trace(world, s, e, SOLID).1))
    });
    f!("playerphysicstrace", |_, world, _, a| {
        let (s, e) = (vec3(a, 0)?, vec3(a, 1)?);
        let t = frame(world).trace_static_world(
            s,
            e,
            crate::bullet_collision::PLAYER_MINS,
            crate::bullet_collision::PLAYER_MAXS,
            SOLID,
        );
        let f = if t.startsolid != 0 {
            0.0
        } else {
            t.fraction.clamp(0.0, 1.0)
        };
        Ok(Value::Vec3(std::array::from_fn(|i| {
            s[i] + (e[i] - s[i]) * f
        })))
    });
    // bo2mp: a spawn on top of a living player (within a player's width,
    // a player's height) would telefrag him: the spawn scripts pick another.
    f!("positionwouldtelefrag", |_, world, _, a| {
        let p = arg(a, 0).as_vec3().unwrap_or([0.0; 3]);
        Ok(Value::bool(telefrag(world, p)))
    });
    f!("boundswouldtelefrag", |_, world, _, a| {
        let p = arg(a, 0).as_vec3().unwrap_or([0.0; 3]);
        Ok(Value::bool(telefrag(world, p)))
    });
    fn telefrag(world: &mut World, p: [f32; 3]) -> bool {
        let ids: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
        let f = frame(world);
        ids.into_iter().any(|n| {
            f.player(ClientId(n)).is_some_and(|ps| {
                ps.health > 0
                    && ((ps.origin[0] - p[0]).powi(2) + (ps.origin[1] - p[1]).powi(2)).sqrt() < 32.0
                    && (ps.origin[2] - p[2]).abs() < 72.0
            })
        })
    }
    // bo2mp: `setmatchflag("game_ended", 1)`: every client's `bo2mp_game_ended`
    // dvar, which opens the scoreboard once his HUD is back (the intermission).
    f!("setmatchflag", |vm, world, _, a| {
        if text(vm, a, 0) == "game_ended" {
            let on = if int(a, 1).unwrap_or(0) != 0 { "1" } else { "" };
            let ids: Vec<u32> = world.resource::<Zm>().players.keys().copied().collect();
            for n in ids {
                super::set_client_dvar(world, n, "bo2mp_game_ended", on);
            }
        }
        Ok(Value::Undefined)
    });
    // bo2mp: a script's vision (mpOutro at the match's end, then the map's
    // own): the frame's grade switches to it at once (its fade is not run).
    f!("visionsetnaked", |vm, _, _, a| {
        let name = text(vm, a, 0);
        let found = asset_world::set_t6_vision(&name);
        diag::info!(Sim, "bo2mp: visionsetnaked {name} (vision found: {found})");
        Ok(Value::Undefined)
    });
    // The game's end (end_game): the session takes it from here.
    f!("exitlevel", |_, world, _, _| {
        world.resource_mut::<crate::script::Runtime>().exit_level = true;
        Ok(Value::Undefined)
    });
    for name in [
        "precachemodel",
        "precacheshader",
        "precachestring",
        "precacheitem",
        "precacherumble",
        "precachemenu",
        "precacheshellshock",
        "precacheanimstatedef",
        "precachevehicle",
        "precacheleaderboards",
        "precachestatusicon",
        "precacheheadicon",
        "precachelocationselector",
        "precacheturret",
        "registerclientfield",
        "codesetclientfield",
        "codesetworldclientfield",
        "codesetplayerstateclientfield",
        "makedvarserverinfo",
        "setmatchtalkflag",
        "setscoreboardcolumns",
        "setdemointermissionpoint",
        "setminimap",
        "visionsetnight",
        "setvisionsetnaked",
        "setsunlight",
        "resetsunlight",
        "setexpfog",
        "setvolfog",
        "logstring",
        "logprint",
        "recordmatchbegin",
        "gamehistorystartmatch",
        "gamehistoryfinishmatch",
        "bbpostdemostreamstatsforround",
        "uploadstats",
        "recordplayerstats",
        "resettimeout",
        "setslowmotion",
        "objective_add",
        "objective_state",
        "objective_position",
        "objective_onentity",
        "objective_delete",
        "objective_visibleteams",
        "objective_icon",
        "objective_team",
        "playfx",
        "playfxontag",
        "playfxontagforclients",
        "stopfx",
        "playsoundatposition",
        "playsound",
        "physicsexplosionsphere",
        "physicsexplosioncylinder",
        "earthquake",
        "playrumbleonposition",
        "setclientnamemode",
        "setvoipattachment",
        "deactivateclientexploder",
        "scriptmodelsuseanimtree",
        "disablegrenadesuicide",
        "enablezombies",
        "disablezombies",
        "setdvarbool",
        "sethideonclientwhendead",
    ] {
        vm.bind(name, false, |_, _, _, _| Ok(Value::Undefined));
    }
}
