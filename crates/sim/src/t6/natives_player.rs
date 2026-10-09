//! Player methods: spawn, weapons, ammo, perks, controls, buttons, pose.

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::{
    Zm, arg, client, entnum, flag, frame, int, list, num, origin_of, text, tick, vec3, weapon,
    weapon_text,
};
use crate::script_player;
use crate::world::ClientId;

type R = Result<Value, String>;

/// A client system's state: "musicCmd" is the music (zm_nuked_amb.csc's
/// states: WAVE loops Nuketown's underscore, the rest are silence); others
/// are not drawn yet.
fn client_sys_state(world: &mut World, id: i32, state: &str, who: Option<u32>) {
    let name = usize::try_from(id)
        .ok()
        .and_then(|i| world.resource::<Zm>().client_sys.get(i).cloned())
        .unwrap_or_default();
    // A level notify for the client scripts (maps/mp/_utility::clientnotify
    // sends it this way): "znfg", Nuketown's game over, turns every local
    // player's world fog to bank 2 (zm_nuked.csc intermission_settings), so
    // the rocket shot sees the town.
    if name.eq_ignore_ascii_case("levelnotify") {
        if state == "znfg" {
            let clients: Vec<u32> = match who {
                Some(n) => vec![n],
                None => world.resource::<Zm>().players.keys().copied().collect(),
            };
            for n in clients {
                super::set_client_dvar(world, n, "bo2zm_fogbank", "2");
            }
        }
        return;
    }
    if !name.eq_ignore_ascii_case("musiccmd") {
        return;
    }
    let alias = match state.to_ascii_uppercase().as_str() {
        "WAVE" => "mus_nuked_underscore",
        _ => "",
    };
    let clients: Vec<u32> = match who {
        Some(n) => vec![n],
        None => world.resource::<Zm>().players.keys().copied().collect(),
    };
    for n in clients {
        super::set_client_dvar(world, n, "bo2zm_music", alias);
    }
}

fn controls(
    vm: &Vm<World>,
    world: &mut World,
    s: &Value,
    set: impl FnOnce(&mut crate::match_state::ScriptControls),
) -> R {
    let id = client(vm, world, s)?;
    let mut f = frame(world);
    if f.client_meta(id).is_some() {
        set(&mut f.client_meta_mut(id).controls);
    }
    Ok(Value::Undefined)
}

/// A shellshock ends (`stopshellshock` or its time ran out): the timer, the
/// SHELLSHOCKED flag and the speed factor it put on the scale go.
pub(super) fn end_shock(world: &mut World, n: u32, why: &str) {
    let id = ClientId(n);
    let taken = world
        .resource_mut::<Zm>()
        .players
        .get_mut(&n)
        .and_then(|p| {
            let name = p.shock.take()?.0;
            let factor = std::mem::replace(&mut p.shock_factor, 1.0);
            Some((name, factor))
        });
    let Some((name, factor)) = taken else {
        // Nothing this script started: still clear the timer.
        if let Some(ps) = frame(world).player_mut(id) {
            ps.shellshock_duration = 0;
            ps.pm_flags &= !playerstate_iw4::pm_flags::SHELLSHOCKED;
        }
        return;
    };
    let level = crate::level_time_ms(tick(world));
    let mut f = frame(world);
    if let Some(ps) = f.player_mut(id) {
        ps.shellshock_duration = 0;
        ps.pm_flags &= !playerstate_iw4::pm_flags::SHELLSHOCKED;
        ps.move_speed_scale_multiplier /= factor;
    }
    f.client_meta_mut(id).shellshock = None;
    diag::info!(
        Sim,
        "bo2mp shellshock end: client {n} '{name}' at {level} ms ({why}), speed x{factor} off"
    );
}

pub(super) fn bind(vm: &mut Vm<World>) {
    macro_rules! m {
        ($name:literal, $body:expr) => {
            vm.bind($name, true, $body);
        };
    }
    m!("spawn", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let origin = vec3(a, 0)?;
        let angles = super::arg(a, 1).as_vec3().unwrap_or([0.0; 3]);
        let state = world
            .resource::<Zm>()
            .players
            .get(&id.0)
            .map_or_else(|| "playing".to_owned(), |p| p.sessionstate.clone());
        let t = tick(world);
        script_player::spawn(&mut frame(world), t, id, origin, angles, &state);
        Ok(Value::Undefined)
    });
    m!("giveweapon", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let name = text(vm, a, 0);
        if ["knife", "bowie", "tazer", "sickle"]
            .iter()
            .any(|k| name.contains(k))
            && let Some(p) = world.resource_mut::<Zm>().players.get_mut(&id.0)
        {
            p.melee_weapon = Some(name.clone());
        }
        let w = weapon(world, &name)?;
        if w == 0 {
            return Ok(Value::Undefined);
        }
        script_player::give_weapon(&mut frame(world), id, w, false)?;
        Ok(Value::Undefined)
    });
    m!("takeweapon", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::take_weapon(&mut frame(world), id, w);
        Ok(Value::Undefined)
    });
    m!("takeallweapons", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        script_player::take_all_weapons(&mut frame(world), id);
        Ok(Value::Undefined)
    });
    m!("hasweapon", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let Ok(w) = weapon(world, &text(vm, a, 0)) else {
            return Ok(Value::Int(0));
        };
        Ok(Value::bool(
            w != 0 && script_player::has_weapon(&frame(world), id, w),
        ))
    });
    m!("getcurrentweapon", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        let w = frame(world).player(id).map_or(0, |ps| ps.weapon);
        let n = weapon_text(world, w);
        Ok(vm.string(&n))
    });
    m!("getcurrentoffhand", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        let w = frame(world)
            .player(id)
            .map_or(0, |ps| ps.offhand_primary as u32);
        let n = weapon_text(world, w);
        Ok(vm.string(&n))
    });
    // True when he has it to switch to (a bot calling in a map-picked
    // scorestreak goes on only then).
    m!("switchtoweapon", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::switch_to_weapon(&mut frame(world), id, w);
        Ok(Value::bool(
            w != 0 && script_player::has_weapon(&frame(world), id, w),
        ))
    });
    m!("switchtoweaponimmediate", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::switch_to_weapon(&mut frame(world), id, w);
        Ok(Value::Undefined)
    });
    m!("setspawnweapon", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::switch_to_weapon(&mut frame(world), id, w);
        Ok(Value::Undefined)
    });
    m!("getweaponslist", |vm, world, s, _| weapon_list(
        vm,
        world,
        s,
        script_player::WeaponList::All
    ));
    m!("getweaponslistprimaries", |vm, world, s, _| weapon_list(
        vm,
        world,
        s,
        script_player::WeaponList::Primaries
    ));
    m!("setweaponammoclip", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        let mut f = frame(world);
        // The left gun of a held dual-wield pair: its clip.
        match script_player::dual_wield_right_of(&f, id, w) {
            Some(right) => script_player::set_left_clip(&mut f, id, right, int(a, 1)?),
            None => script_player::set_ammo_clip(&mut f, id, w, int(a, 1)?),
        }
        Ok(Value::Undefined)
    });
    m!("setweaponammostock", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::set_ammo_stock(&mut frame(world), id, w, int(a, 1)?);
        Ok(Value::Undefined)
    });
    m!("getweaponammoclip", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        let f = frame(world);
        Ok(Value::Int(
            match script_player::dual_wield_right_of(&f, id, w) {
                Some(right) => script_player::left_clip(&f, id, right),
                None => script_player::ammo_clip(&f, id, w),
            },
        ))
    });
    m!("getweaponammostock", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        Ok(Value::Int(script_player::ammo_stock(&frame(world), id, w)))
    });
    m!("getammocount", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        let f = frame(world);
        Ok(Value::Int(
            script_player::ammo_clip(&f, id, w) + script_player::ammo_stock(&f, id, w),
        ))
    });
    m!("givemaxammo", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::give_max_ammo(&mut frame(world), id, w);
        Ok(Value::Undefined)
    });
    m!("givestartammo", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        script_player::give_start_ammo(&mut frame(world), id, w);
        Ok(Value::Undefined)
    });
    m!("setperk", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let p = text(vm, a, 0);
        script_player::set_perk(&mut frame(world), id, &p, true);
        if let Some(pl) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            pl.perks.insert(p);
        }
        Ok(Value::Undefined)
    });
    m!("unsetperk", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let p = text(vm, a, 0);
        script_player::set_perk(&mut frame(world), id, &p, false);
        if let Some(pl) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            pl.perks.remove(&p);
        }
        Ok(Value::Undefined)
    });
    m!("clearperks", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        script_player::clear_perks(&mut frame(world), id);
        if let Some(pl) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            pl.perks.clear();
        }
        Ok(Value::Undefined)
    });
    m!("hasperk", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let p = text(vm, a, 0);
        Ok(Value::bool(
            world
                .resource::<Zm>()
                .players
                .get(&id.0)
                .is_some_and(|pl| pl.perks.contains(&p)),
        ))
    });
    m!("freezecontrols", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.frozen = on)
    });
    m!("disableweapons", |vm, world, s, _| controls(
        vm,
        world,
        s,
        |c| c.weapons_disabled = true
    ));
    m!("enableweapons", |vm, world, s, _| controls(
        vm,
        world,
        s,
        |c| c.weapons_disabled = false
    ));
    m!("disableoffhandweapons", |vm, world, s, _| controls(
        vm,
        world,
        s,
        |c| c.offhands_disabled = true
    ));
    m!("enableoffhandweapons", |vm, world, s, _| controls(
        vm,
        world,
        s,
        |c| c.offhands_disabled = false
    ));
    m!("disableweaponcycling", |vm, world, s, _| controls(
        vm,
        world,
        s,
        |c| c.switch_disabled = true
    ));
    m!("enableweaponcycling", |vm, world, s, _| controls(
        vm,
        world,
        s,
        |c| c.switch_disabled = false
    ));
    // bo2mp: BO2's `allow*` (false drops the request in constrain_cmd).
    m!("allowprone", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.prone_disabled = !on)
    });
    m!("allowcrouch", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.crouch_disabled = !on)
    });
    m!("allowstand", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.stand_disabled = !on)
    });
    m!("allowsprint", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.sprint_disabled = !on)
    });
    m!("allowmelee", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.melee_disabled = !on)
    });
    m!("allowads", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.ads_disabled = !on)
    });
    // bo2mp: `setmovespeedscale(f)` is the player's own scale; a shellshock's
    // `bg_shock_movement` rides on top while it lasts (`getmovespeedscale`
    // gives his own back). The feel window reads
    // `PlayerState::move_speed_scale_multiplier` (the product).
    m!("setmovespeedscale", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let scale = num(a, 0)?;
        let factor = world
            .resource::<Zm>()
            .players
            .get(&id.0)
            .map_or(1.0, |p| p.shock_factor);
        if let Some(ps) = frame(world).player_mut(id) {
            ps.move_speed_scale_multiplier = scale * factor;
        }
        diag::info!(
            Sim,
            "bo2mp setmovespeedscale: client {} scale {scale} (shock x{factor} = {})",
            id.0,
            scale * factor
        );
        Ok(Value::Undefined)
    });
    m!("getmovespeedscale", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        let factor = world
            .resource::<Zm>()
            .players
            .get(&id.0)
            .map_or(1.0, |p| p.shock_factor);
        Ok(Value::Float(
            frame(world)
                .player(id)
                .map_or(1.0, |ps| ps.move_speed_scale_multiplier / factor),
        ))
    });
    // bo2mp: BO2's shellshock (shock/<name>.shock): the timer and duration
    // on the player state, the file's screen / look / sound / movement
    // values to his client. Our flashed overlay, look limits and tinnitus
    // run from those; the movement scale is the file's, not a constant.
    m!("shellshock", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let name = text(vm, a, 0).to_ascii_lowercase();
        let seconds = num(a, 1)?;
        if seconds < 0.0 {
            return Err(format!("shellshock duration {seconds} is negative"));
        }
        let Some(mut shock) = frame(world).shock(&name).cloned() else {
            return Err(format!("no shock file for shellshock '{name}'"));
        };
        let now = crate::level_time_ms(tick(world));
        let index = {
            let mut zm = world.resource_mut::<Zm>();
            let at = match zm.shock_names.iter().position(|n| *n == name) {
                Some(at) => at,
                None => {
                    zm.shock_names.push(name.clone());
                    zm.shock_names.len() - 1
                }
            };
            at as i32 + 1
        };
        let new_factor = if shock.movement_scale > 0.0 {
            shock.movement_scale
        } else {
            1.0
        };
        let old_factor = world
            .resource::<Zm>()
            .players
            .get(&id.0)
            .map_or(1.0, |p| p.shock_factor);
        let duration = (seconds * 1000.0) as i32;
        // BO2's file gives a scale, not a switch: IW4's fixed 0.4 stays off.
        shock.movement = false;
        let (look, sound, kick) = (
            shock.look.affect,
            shock.sound.loop_alias.clone(),
            shock.view_kick_radius,
        );
        let flashed = shock.screen_type == hud_iw4::SCREEN_BLEND_FLASHED;
        let mut f = frame(world);
        let Some(ps) = f.player_mut(id) else {
            return Ok(Value::Undefined);
        };
        ps.shellshock_index = index;
        ps.shellshock_time = now;
        ps.shellshock_duration = duration;
        ps.pm_flags |= playerstate_iw4::pm_flags::SHELLSHOCKED;
        ps.move_speed_scale_multiplier = ps.move_speed_scale_multiplier / old_factor * new_factor;
        f.client_meta_mut(id).shellshock = Some(shock);
        if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            p.shock = Some((name.clone(), now.wrapping_add(duration)));
            p.shock_factor = new_factor;
        }
        diag::info!(
            Sim,
            "bo2mp shellshock: client {} '{name}' {seconds}s from {now} ms (index {index}, flash {flashed}, look {look}, loop '{sound}', speed x{new_factor}, view kick {kick})",
            id.0
        );
        Ok(Value::Undefined)
    });
    m!("stopshellshock", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        end_shock(world, id.0, "stopshellshock");
        Ok(Value::Undefined)
    });
    m!("allowjump", |vm, world, s, a| {
        let on = flag(a, 0, true);
        controls(vm, world, s, |c| c.jump_disabled = !on)
    });
    m!("getplayerangles", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        Ok(Value::Vec3(
            frame(world).player(id).map_or([0.0; 3], |ps| ps.viewangles),
        ))
    });
    // Last stand: up again (`reviveplayer`: full health, standing).
    m!("reviveplayer", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            p.laststand = false;
        }
        if let Some(ps) = frame(world).player_mut(id) {
            use playerstate_iw4::eflags::{DUCK, PRONE};
            ps.e_flags &= !(DUCK | PRONE);
            ps.health = ps.max_health.max(1);
        }
        Ok(Value::Undefined)
    });
    m!("undolaststand", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            p.laststand = false;
        }
        Ok(Value::Undefined)
    });
    m!("depthinwater", |_, _, _, _| Ok(Value::Float(0.0)));
    // bo2mp: the gun on his back (BO2's _weapons.gsc stow_on_back): every
    // client draws its world model on his tag_stowed_back.
    m!("setstowedweapon", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let name = text(vm, a, 0);
        let stowed = super::weapon(world, &name).ok().filter(|&w| w != 0).map(|w| (w, name));
        super::playeranim::edit_body(world, id.0, |l| l.stowed = stowed);
        Ok(Value::Undefined)
    });
    m!("clearstowedweapon", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        super::playeranim::edit_body(world, id.0, |l| l.stowed = None);
        Ok(Value::Undefined)
    });
    vm.bind("getweaponstowedmodel", false, |vm, _, _, _| {
        Ok(vm.string(""))
    });
    for name in [
        "startrevive",
        "stoprevive",
        "docowardswayanims",
        "setrevivehintstring",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    // Client systems (maps/mp/_utility::registerclientsys and
    // setclientsysstate call these): each registered name gets an id; a
    // state set on one goes to every client, or as a method to one.
    vm.bind("clientsysregister", false, |vm, world, _, a| {
        let name = text(vm, a, 0);
        let mut zm = world.resource_mut::<Zm>();
        let id = match zm
            .client_sys
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&name))
        {
            Some(i) => i,
            None => {
                zm.client_sys.push(name);
                zm.client_sys.len() - 1
            }
        };
        Ok(Value::Int(id as i32))
    });
    vm.bind("clientsyssetstate", false, |vm, world, _, a| {
        let id = arg(a, 0).as_int().unwrap_or(-1);
        let state = text(vm, a, 1);
        client_sys_state(world, id, &state, None);
        Ok(Value::Undefined)
    });
    vm.bind("clientsyssetstate", true, |vm, world, s, a| {
        let id = arg(a, 0).as_int().unwrap_or(-1);
        let state = text(vm, a, 1);
        let who = entnum(vm, s);
        client_sys_state(world, id, &state, who);
        Ok(Value::Undefined)
    });
    // A player moved by a script (spawning, a game-over fall).
    m!("setorigin", |vm, world, s, a| {
        let to = vec3(a, 0)?;
        match client(vm, world, s) {
            Ok(id) => super::teleport_player(world, id, to),
            Err(_) => {
                if let Some(n) = super::entnum(vm, s)
                    && let Some(e) = world.resource_mut::<Zm>().ents.get_mut(&n)
                {
                    e.origin = to;
                }
            }
        }
        Ok(Value::Undefined)
    });
    m!("setplayerangles", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        super::set_player_view(world, id, vec3(a, 0)?);
        Ok(Value::Undefined)
    });
    m!("setvelocity", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let v = vec3(a, 0)?;
        if let Some(ps) = frame(world).player_mut(id) {
            ps.velocity = v;
        }
        Ok(Value::Undefined)
    });
    m!("getstance", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        let st = script_player::stance(&frame(world), id);
        Ok(vm.string(st))
    });
    m!("setstance", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let st = text(vm, a, 0);
        if let Some(ps) = frame(world).player_mut(id) {
            use playerstate_iw4::eflags::{DUCK, PRONE};
            ps.e_flags &= !(DUCK | PRONE);
            ps.e_flags |= match st.as_str() {
                "crouch" => DUCK,
                "prone" => PRONE,
                _ => 0,
            };
        }
        Ok(Value::Undefined)
    });
    m!("isonground", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        Ok(Value::bool(frame(world).player(id).is_some_and(|ps| {
            ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE
        })))
    });
    m!("issprinting", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        Ok(Value::bool(frame(world).player(id).is_some_and(|ps| {
            ps.pm_flags & playerstate_iw4::pm_flags::SPRINTING != 0
        })))
    });
    macro_rules! button {
        ($($name:literal => $mask:expr),* $(,)?) => {$(
            vm.bind($name, true, |vm, world, s, _| {
                let id = client(vm, world, s)?;
                let held = script_player::buttons(&mut frame(world), id);
                Ok(Value::bool(held & ($mask) != 0))
            });
        )*};
    }
    {
        use playerstate_iw4::buttons::*;
        button!(
            "attackbuttonpressed" => ATTACK,
            "usebuttonpressed" => USE | USE_RELOAD,
            "meleebuttonpressed" => MELEE_CHARGE,
            "fragbuttonpressed" => FRAG,
            "secondaryoffhandbuttonpressed" => SMOKE,
            "adsbuttonpressed" => ADS,
            "jumpbuttonpressed" => JUMP,
            "sprintbuttonpressed" => SPRINT,
        );
    }
    m!("geteye", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        let f = frame(world);
        let (o, h) = f
            .player(id)
            .map_or(([0.0; 3], 60.0), |ps| (ps.origin, ps.view_height_current));
        Ok(Value::Vec3([o[0], o[1], o[2] + h]))
    });
    m!("getvelocity", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        Ok(Value::Vec3(
            frame(world).player(id).map_or([0.0; 3], |ps| ps.velocity),
        ))
    });
    m!("setmaxhealth", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let n = int(a, 0)?.max(1);
        let mut f = frame(world);
        f.client_meta_mut(id).max_health = n;
        if let Some(ps) = f.player_mut(id) {
            ps.max_health = n;
            ps.health = ps.health.min(n);
        }
        Ok(Value::Undefined)
    });
    m!("setnormalhealth", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let frac = super::num(a, 0)?;
        if let Some(ps) = frame(world).player_mut(id) {
            ps.health = ((ps.max_health as f32 * frac) as i32).max(1);
        }
        Ok(Value::Undefined)
    });
    m!("getguid", |vm, _, _, _| Ok(vm.string("0")));
    m!("getxuid", |vm, _, _, _| Ok(vm.string("0")));
    m!("ishost", |_, _, _, _| Ok(Value::Int(1)));
    m!("isplayeronsamemachine", |_, _, _, _| Ok(Value::Int(0)));
    m!("isthrowinggrenade", |_, _, _, _| Ok(Value::Int(0)));
    m!("isswitchingweapons", |_, _, _, _| Ok(Value::Int(0)));
    m!("isinmovemode", |_, _, _, _| Ok(Value::Int(0)));
    m!("isinvehicle", |_, _, _, _| Ok(Value::Int(0)));
    m!("isremotecontrolling", |_, _, _, _| Ok(Value::Int(0)));
    m!("getsnapshotackindex", |_, _, _, _| Ok(Value::Int(0)));
    // How full a gun is (the scripts' grenade award gives 2, 3 or 4 by it).
    m!("getfractionstartammo", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        Ok(Value::Float(script_player::ammo_fraction(
            &frame(world),
            id,
            w,
            true,
        )))
    });
    m!("getfractionmaxammo", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let w = weapon(world, &text(vm, a, 0))?;
        Ok(Value::Float(script_player::ammo_fraction(
            &frame(world),
            id,
            w,
            false,
        )))
    });
    for name in [
        "setclientuivisibilityflag",
        "setclientminiscoreboardhide",
        "setclientammocounterhide",
        "setclientscriptmainmenu",
        "setclientthirdperson",
        "setclientthirdpersonangle",
        "setclientcgobjectivetext",
        "setclientdvar",
        "setclientdvars",
        "openmenu",
        "closemenu",
        "closeingamemenu",
        "setblur",
        "startfadingblur",
        "enableinvulnerability",
        "disableinvulnerability",
        "allowlean",
        "allowspectateteam",
        "allowpitchangle",
        "allowedstances",
        "setspectatepermissions",
        "playrumbleonentity",
        "stoprumble",
        "setlowready",
        "setweaponoverheating",
        "setsprintduration",
        "setsprintcooldown",
        "resetfov",
        "setburn",
        "addplayerstat",
        "addweaponstat",
        "adddstat",
        "setdstat",
        "incrementplayerstat",
        "addplayerstatwithgametype",
        "updatestatratio",
        "setplayercurrentstreak",
        "recordplayerdeathzombies",
        "recordplayerrevivezombies",
        "recordzombiezone",
        "recordkillmodifier",
        "setclientflag",
        "clearclientflag",
        "sendfaceevent",
        "setlaststandprevweap",
        "setteamfortrigger",
        "setspawnerteam",
        "vibrate",
        "setviewmodel",
        "useweaponhidetags",
        "dropscavengeritem",
        "setempjammed",
        "setfreecameralockonallowed",
        "luinotifyevent",
        "luinotifyeventtoplayer",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    // bo2mp: BO2's own menus open on the player's screen (lane C); the
    // server only hears his answer (`menuresponse`). BO2MP_AUTOCLASS=<class>
    // answers the class menu a second after it opens (a test aid: a hidden
    // test match spawns without anyone at the keyboard).
    m!("openmenu", |vm, world, s, a| {
        let menu = text(vm, a, 0);
        let client = super::entnum(vm, s).unwrap_or(u32::MAX);
        diag::info!(Sim, "bo2mp menu open: player {client} {menu}");
        if let (Value::Object(o), Ok(class)) = (s, std::env::var("BO2MP_AUTOCLASS"))
            && menu.contains("changeclass")
        {
            let mut zm = world.resource_mut::<Zm>();
            let due = zm.now_ms + 1000;
            zm.menu_replies.push((due, *o, menu, class));
        }
        Ok(Value::Undefined)
    });
    // BO2's own end-of-match blur (`roundenddof`: 0, 128, 512, 4000, 6, 1.8):
    // the view effect the renderer's depth-of-field pass reads. The engine
    // takes it at once. Bad numbers are logged and ignored (not a script
    // error), so no BO2 script can be stopped by it.
    m!("setdepthoffield", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let n = |i: usize| num(a, i);
        let dof = match (n(0), n(1), n(2), n(3), n(4), n(5)) {
            (Ok(a0), Ok(a1), Ok(a2), Ok(a3), Ok(a4), Ok(a5)) => {
                crate::script::host::client_effects::depth_of_field_values(a0, a1, a2, a3, a4, a5)
            }
            _ => Err("six numbers expected".to_string()),
        };
        match dof {
            Ok(dof) => {
                let mut f = frame(world);
                if f.client_meta(id).is_some() {
                    f.client_meta_mut(id).view_effects.depth_of_field = dof;
                }
            }
            Err(e) => diag::warn!(Sim, "bo2mp setdepthoffield ignored: {e}"),
        }
        Ok(Value::Undefined)
    });
    m!("getdstat", |_, _, _, _| Ok(Value::Int(0)));
    // bo2zm M4: setactionslot(slot, kind[, weapon]): what his d-pad slot
    // holds (BO2's HUD draws it: `bo2zm_actionslots`); "" empties it.
    m!("setactionslot", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let slot = arg(a, 0).as_int().unwrap_or(0) as i32;
        let kind = text(vm, a, 1);
        let held = if kind == "weapon" { text(vm, a, 2) } else { kind };
        let mut zm = world.resource_mut::<super::Zm>();
        let slots = zm.action_slots.entry(id.0).or_default();
        slots.retain(|(n, _)| *n != slot);
        if !held.is_empty() {
            slots.push((slot, held));
            slots.sort();
        }
        Ok(Value::Undefined)
    });
    m!("getnormalizedmovement", |_, _, _, _| Ok(Value::Vec3(
        [0.0; 3]
    )));
    m!("getnormalizedcameramovement", |_, _, _, _| Ok(Value::Vec3(
        [0.0; 3]
    )));
    m!("iprintln", |vm, _, _, a| {
        let t = text(vm, a, 0);
        diag::info!(Sim, "bo2zm t6 iprintln: {t}");
        Ok(Value::Undefined)
    });
    m!("iprintlnbold", |vm, _, _, a| {
        let t = text(vm, a, 0);
        diag::info!(Sim, "bo2zm t6 iprintlnbold: {t}");
        Ok(Value::Undefined)
    });
    // bo2zm M3: going down and the game's end.
    m!("playerads", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        Ok(Value::Float(
            frame(world)
                .player(id)
                .map_or(0.0, |ps| ps.f_weapon_pos_frac),
        ))
    });
    m!("getweaponmuzzlepoint", |vm, world, s, _| {
        // A zombie's killer may be no player (another zombie, the world):
        // where it stands.
        let Ok(id) = client(vm, world, s) else {
            return Ok(origin_of(vm, world, s).map_or(Value::Undefined, Value::Vec3));
        };
        let Some(ps) = frame(world).player(id).copied() else {
            return Ok(Value::Undefined);
        };
        let (f, _, _) = gsc_t6::math::angle_vectors(ps.viewangles);
        let eye = [
            ps.origin[0],
            ps.origin[1],
            ps.origin[2] + ps.view_height_current,
        ];
        Ok(Value::Vec3(std::array::from_fn(|i| eye[i] + f[i] * 16.0)))
    });
    m!("getweaponforwarddir", |vm, world, s, _| {
        let id = client(vm, world, s)?;
        let angles = frame(world).player(id).map_or([0.0; 3], |ps| ps.viewangles);
        Ok(Value::Vec3(gsc_t6::math::angle_vectors(angles).0))
    });
    // Linked to an entity: the player goes where it goes (Nuketown's
    // game-over fall is a moving script_origin).
    m!("playerlinkto", |vm, world, s, a| {
        let id = client(vm, world, s)?;
        let to = entnum(vm, arg(a, 0));
        if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
            p.linked = to;
        }
        Ok(Value::Undefined)
    });
    for name in [
        "playerlinktodelta",
        "playerlinktoabsolute",
        "playerlinkedoffsetenable",
        "playerlinktoblend",
    ] {
        vm.bind(name, true, |vm, world, s, a| {
            let id = client(vm, world, s)?;
            let to = entnum(vm, arg(a, 0));
            if let Some(p) = world.resource_mut::<Zm>().players.get_mut(&id.0) {
                p.linked = to;
            }
            Ok(Value::Undefined)
        });
    }
    m!("getlinkedent", |vm, world, s, _| {
        let Some(n) = entnum(vm, s) else {
            return Ok(Value::Undefined);
        };
        let linked = {
            let zm = world.resource::<Zm>();
            zm.players
                .get(&n)
                .and_then(|p| p.linked)
                .or_else(|| zm.ents.get(&n).and_then(|e| e.link.map(|l| l.0)))
        };
        Ok(linked
            .and_then(|l| world.resource::<Zm>().ents.get(&l).and_then(|e| e.obj))
            .map_or(Value::Undefined, Value::Object))
    });
    for name in [
        "fakedamagefrom",
        "recordplayerdownzombies",
        "recordplayerrevivezombies",
        "recordplayerdeathzombies",
        "stoprumble",
        "playrumbleonentity",
        "playrumblelooponentity",
        "setclientuivisibilityflag",
        "setclientminiscoreboardhide",
        "setblur",
        "setburn",
        "setelectrified",
        "setlowready",
    ] {
        vm.bind(name, true, |_, _, _, _| Ok(Value::Undefined));
    }
    let _ = arg;
    let _ = list;
}

fn weapon_list(
    vm: &mut Vm<World>,
    world: &mut World,
    s: &Value,
    which: script_player::WeaponList,
) -> R {
    let id = client(vm, world, s)?;
    let ws = script_player::weapons(&frame(world), id, which);
    let mut out = Vec::new();
    for w in ws {
        let n = weapon_text(world, w);
        out.push(vm.string(&n));
    }
    Ok(list(out))
}
