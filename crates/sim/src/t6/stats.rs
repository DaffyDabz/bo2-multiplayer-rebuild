//! bo2mp lane C: the players' stats on the server, as BO2's scripts read
//! and write them (its DDL stats: `getdstat`, `setdstat`, `adddstat`,
//! `addplayerstat`, `addweaponstat`, ...), and his own classes from them.
//!
//! The local player's stats are his stats file (`bo2mp_stats.txt`, the
//! front end's: `bo2_profile`'s store by stats path), read when the match
//! first asks and, in a ranked match, written back as they change and at
//! its end (merged into the file, so a path the match never touched stays
//! as the front end left it). A bot's stats start empty and are never
//! saved.
//!
//! The file: what the front end sets (`set_local_stats_path`), else
//! `BO2MP_STATS`, else `iw4l-artifacts/bo2mp_stats.txt` when there is one
//! (a match started straight from the command line).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use bevy_ecs::prelude::{Resource, World};
use bo2_profile::{Profile, StatValue};
use gsc_t6::{Value, Vm};

use super::{Zm, arg, entnum, text};

static LOCAL_STATS: Mutex<Option<PathBuf>> = Mutex::new(None);
static RANKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the next match is ranked (Combat Training: XP counts, the
/// public class set), as the front end's playlist chose; `BO2MP_RANKED=1`
/// for a match started from the command line.
pub fn set_local_match_ranked(ranked: bool) {
    RANKED.store(ranked, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn ranked() -> bool {
    RANKED.load(std::sync::atomic::Ordering::Relaxed) || std::env::var("BO2MP_RANKED").is_ok_and(|v| v == "1")
}

/// His stats file for the matches this process plays (the front end calls
/// this at START MATCH).
pub fn set_local_stats_path(path: PathBuf) {
    if let Ok(mut p) = LOCAL_STATS.lock() {
        *p = Some(path);
    }
}

fn local_path() -> Option<PathBuf> {
    if let Some(p) = LOCAL_STATS.lock().ok().and_then(|p| p.clone()) {
        return Some(p);
    }
    if let Ok(p) = std::env::var("BO2MP_STATS") {
        return Some(PathBuf::from(p));
    }
    let p = PathBuf::from("iw4l-artifacts").join("bo2mp_stats.txt");
    p.exists().then_some(p)
}

/// How often changed stats are written (ms of match time).
const SAVE_EVERY_MS: i64 = 10000;

/// One player's stats.
pub(crate) struct PlayerStats {
    /// The items (his tables) and the stats; his classes read from them.
    pub profile: Profile,
    /// His stats file (the local player only).
    file: Option<PathBuf>,
    /// The paths changed since the last save.
    changed: BTreeSet<String>,
}

#[derive(Resource, Default)]
pub(crate) struct ServerStats {
    players: HashMap<u32, PlayerStats>,
    last_save: i64,
}

fn table_rows(world: &World, name: &str) -> Vec<Vec<String>> {
    let zm = world.resource::<Zm>();
    let Some(t) = zm.tables.get(name) else {
        return Vec::new();
    };
    t.cells.chunks(t.columns.max(1)).map(<[String]>::to_vec).collect()
}

/// Whether a client is the local player (the listen server's own, not a
/// bot).
fn is_local(world: &World, client: u32) -> bool {
    client == 0 && !world.resource::<Zm>().bots.contains_key(&client)
}

/// A player's stats, read the first time they are asked for.
pub(crate) fn player(world: &mut World, client: u32) -> &mut PlayerStats {
    if !world.contains_resource::<ServerStats>() {
        world.insert_resource(ServerStats::default());
    }
    if !world.resource::<ServerStats>().players.contains_key(&client) {
        let file = is_local(world, client).then(local_path).flatten();
        let stats = file.as_deref().map(Profile::read_stats_file).unwrap_or_default();
        let (items, atts) = (table_rows(world, "mp/statstable.csv"), table_rows(world, "mp/attachmenttable.csv"));
        let mut profile = Profile::from_tables(&items, &atts, &|_| None, stats);
        profile.match_set = class_set(world).to_owned();
        if let Some(f) = &file {
            diag::info!(
                Sim,
                "bo2mp stats: player {client} from {} ({} stats, rank {}, classes from {})",
                f.display(),
                profile.stats.len(),
                profile.rank(),
                profile.match_set
            );
        }
        world
            .resource_mut::<ServerStats>()
            .players
            .insert(client, PlayerStats { profile, file, changed: BTreeSet::new() });
    }
    world.resource_mut::<ServerStats>().into_inner().players.get_mut(&client).expect("inserted above")
}

/// The class set a match gives (BO2's `cacRoot`): a ranked match (Combat
/// Training) the public one, else the custom match set.
fn class_set(world: &World) -> &'static str {
    if world.resource::<Zm>().ranked_match { "cacloadouts" } else { bo2_profile::LOCAL_MATCH_SET }
}

/// The local player's profile, when the match has his stats (his classes).
pub(crate) fn local_profile(world: &mut World, client: u32) -> Option<&Profile> {
    if !is_local(world, client) {
        return None;
    }
    let p = player(world, client);
    p.file.is_some().then_some(&p.profile)
}

fn get(world: &mut World, client: u32, path: &str) -> Option<StatValue> {
    player(world, client).profile.stat(path).cloned()
}

fn set(world: &mut World, client: u32, path: String, v: StatValue) {
    let p = player(world, client);
    if p.profile.stat(&path) != Some(&v) {
        p.changed.insert(path.clone());
        p.profile.set_stat(path, v);
    }
}

fn add(world: &mut World, client: u32, path: String, delta: f32) {
    let now = get(world, client, &path).and_then(|v| v.as_num()).unwrap_or(0.0);
    set(world, client, path, StatValue::Num(now + delta));
}

/// A stats path from the scripts' words (`"PlayerStatsList", "RANKXP",
/// "StatValue"` -> `playerstatslist.rankxp.statvalue`).
fn path_of(vm: &mut Vm<World>, words: &[Value]) -> String {
    words
        .iter()
        .map(|w| match w {
            Value::Int(n) => n.to_string(),
            Value::Float(f) => (*f as i64).to_string(),
            other => vm.to_text(other).to_ascii_lowercase(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn to_value(vm: &mut Vm<World>, v: Option<StatValue>) -> Value {
    match v {
        Some(StatValue::Num(n)) if n == n.trunc() && n.abs() < 2.0e9 => Value::Int(n as i32),
        Some(StatValue::Num(n)) => Value::Float(n),
        Some(StatValue::Str(s)) => vm.string(&s),
        None => Value::Int(0),
    }
}

fn of_value(vm: &mut Vm<World>, v: &Value) -> StatValue {
    match v {
        Value::Int(n) => StatValue::Num(*n as f32),
        Value::Float(f) => StatValue::Num(*f),
        other => {
            let t = vm.to_text(other);
            t.trim().parse::<f32>().map_or(StatValue::Str(t), StatValue::Num)
        }
    }
}

/// A weapon's stats path start (`itemstats.<item index>.stats`): its
/// item from the statstable by reference (`mp7_mp+reflex` -> MP7).
fn weapon_item(world: &mut World, client: u32, weapon: &str) -> Option<i32> {
    let base = weapon.split('+').next().unwrap_or(weapon);
    let reference = base.strip_suffix("_mp").unwrap_or(base);
    player(world, client).profile.item_by_ref(reference).map(|it| it.index)
}

/// Each tick: write the local player's changed stats to his file (merged
/// over what is there now). Only a ranked match's (Combat Training): as in
/// BO2, a custom game keeps its stats to itself (his Combat Record and
/// weapon stats count only ranked play).
pub(crate) fn save(world: &mut World, force: bool) {
    if !world.resource::<Zm>().ranked_match {
        return;
    }
    let now = world.resource::<Zm>().now_ms;
    let Some(mut s) = world.get_resource_mut::<ServerStats>() else { return };
    if !force && now - s.last_save < SAVE_EVERY_MS {
        return;
    }
    s.last_save = now;
    for (client, p) in &mut s.players {
        let Some(file) = p.file.clone() else { continue };
        if p.changed.is_empty() {
            continue;
        }
        let mut on_disk = Profile { stats: Profile::read_stats_file(&file), ..Profile::default() };
        for path in std::mem::take(&mut p.changed) {
            if let Some(v) = p.profile.stat(&path) {
                on_disk.stats.insert(path, v.clone());
            }
        }
        match on_disk.save_file(&file) {
            Ok(()) => diag::info!(Sim, "bo2mp stats: player {client}'s saved to {}", file.display()),
            Err(e) => diag::warn!(Sim, "bo2mp stats: player {client}'s not saved ({}): {e}", file.display()),
        }
    }
}

fn client_of(vm: &mut Vm<World>, s: &Value) -> Option<u32> {
    entnum(vm, s)
}

// Ranks and XP: the engine's half of BO2's `_rank.gsc` (its `addrankxp`
// and `CodeCallback_RankUp`). The scripts' own half (giverankxp,
// incrankxp, updaterank, syncxpstat) runs as written.

/// A table by name, any case (`mp/rankTable.csv`).
fn table<'a>(zm: &'a Zm, name: &str) -> Option<&'a super::T6Table> {
    zm.tables.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, t)| t)
}

/// mp/rankTable.csv: each rank's id, the XP it starts at, the XP it ends
/// at, and the unlock tokens reaching it gives (column 17: `token` is one,
/// `token 4` four).
struct RankRow {
    id: i32,
    min: f32,
    max: f32,
    tokens: i32,
}

fn rank_rows(world: &World) -> Vec<RankRow> {
    let zm = world.resource::<Zm>();
    let Some(t) = table(zm, "mp/rankTable.csv") else { return Vec::new() };
    t.cells
        .chunks(t.columns.max(1))
        .filter_map(|r| {
            let cell = |i: usize| r.get(i).map_or("", |c| c.trim());
            Some(RankRow {
                id: cell(0).parse().ok()?,
                min: cell(2).parse().ok()?,
                max: cell(7).parse().ok()?,
                tokens: cell(17)
                    .strip_prefix("token")
                    .map_or(0, |n| n.trim().parse().unwrap_or(1)),
            })
        })
        .collect()
}

fn rank_for_xp(rows: &[RankRow], xp: f32) -> i32 {
    rows.iter().filter(|r| r.min <= xp).map(|r| r.id).max().unwrap_or(0)
}

/// mp/scoreInfo.csv: an event's XP in this game type (its `<gametype> XP`
/// column) and whether it also counts toward the weapon (its GunXP).
fn event_xp(world: &World, gametype: &str, event: &str) -> Option<(f32, bool)> {
    let zm = world.resource::<Zm>();
    let t = table(zm, "mp/scoreInfo.csv")?;
    let mut rows = t.cells.chunks(t.columns.max(1));
    let header = rows.next()?;
    let col = header.iter().position(|h| h.trim().eq_ignore_ascii_case(&format!("{gametype} XP")))?;
    let gun = header.iter().position(|h| h.trim().eq_ignore_ascii_case("GunXP"));
    let row = rows.find(|r| r.first().is_some_and(|c| c.trim().eq_ignore_ascii_case(event)))?;
    let xp = row.get(col)?.trim().parse::<f32>().ok()?;
    let gunxp = gun.and_then(|g| row.get(g)).is_some_and(|c| c.trim().eq_ignore_ascii_case("TRUE"));
    Some((xp, gunxp))
}

/// What a stat reads as a number.
fn num(world: &mut World, client: u32, path: &str) -> f32 {
    get(world, client, path).and_then(|v| v.as_num()).unwrap_or(0.0)
}

/// The unlock tokens he has to spend in Create-a-Class (Pick 10): BO2's
/// `unlocks[]` stat, the pool its menus read (CoD.CAC.GetUnlockCountForGroup:
/// `GetDStat(controller, "unlocks", GetUnlockIndexFromGroupName(group))`,
/// one pool, index 0).
use bo2_profile::UNLOCK_TOKENS;
const RANK_XP: &str = "playerstatslist.rankxp.statvalue";

/// XP for a player in a ranked match: his rank XP (capped at the last
/// rank's end), the weapon's XP, and a new rank: its stats, its unlock
/// tokens, and BO2's `CodeCallback_RankUp(rank, prestige, tokens added)`
/// (the scripts' rank-up popup).
fn give_xp(vm: &mut Vm<World>, world: &mut World, who: &Value, client: u32, xp: f32, weapon: Option<(&str, bool)>) {
    if xp <= 0.0 {
        return;
    }
    let rows = rank_rows(world);
    let cap = rows.iter().map(|r| r.max).fold(0.0f32, f32::max);
    let old = num(world, client, "playerstatslist.rankxp.statvalue");
    let new = if cap > 0.0 { (old + xp).min(cap) } else { old + xp };
    set(world, client, "playerstatslist.rankxp.statvalue".to_owned(), StatValue::Num(new));
    if let Some((w, true)) = weapon
        && let Some(item) = weapon_item(world, client, w)
    {
        add(world, client, format!("itemstats.{item}.xp.statvalue"), xp);
    }
    // The scripts' own count (pers["rankxp"]) stays with the stat: their
    // syncxpstat writes it back.
    if let Value::Object(o) = who {
        let pers = vm.intern("pers");
        if let Value::Array(a) = vm.raw_field(*o, pers) {
            let key = gsc_t6::Key::Str(vm.intern("rankxp"));
            a.write().set(key, Value::Int(new as i32));
        }
    }
    rank_up(vm, world, who, client, old, new);
}

/// His rank XP went from `old` to `new` (the engine's addrankxp, or the
/// scripts' giverankxp writing it with setdstat): a new rank gets its
/// stats, its unlock tokens, and BO2's `CodeCallback_RankUp(rank,
/// prestige, tokens added)` (the scripts' rank-up popup).
fn rank_up(vm: &mut Vm<World>, world: &mut World, who: &Value, client: u32, old: f32, new: f32) {
    let rows = rank_rows(world);
    let (from, to) = (rank_for_xp(&rows, old), rank_for_xp(&rows, new));
    if to <= from {
        return;
    }
    let tokens: i32 = rows.iter().filter(|r| r.id > from && r.id <= to).map(|r| r.tokens).sum();
    add(world, client, UNLOCK_TOKENS.to_owned(), tokens as f32);
    set(world, client, "playerstatslist.rank.statvalue".to_owned(), StatValue::Num(to as f32));
    if let Some(r) = rows.iter().find(|r| r.id == to) {
        set(world, client, "playerstatslist.minxp.statvalue".to_owned(), StatValue::Num(r.min));
        set(world, client, "playerstatslist.maxxp.statvalue".to_owned(), StatValue::Num(r.max));
    }
    let prestige = num(world, client, "playerstatslist.plevel.statvalue") as i32;
    diag::info!(Sim, "bo2mp rank: player {client} rank {from} -> {to} ({new} XP, {tokens} unlock tokens)");
    vm.spawn_named(
        world,
        "maps/mp/gametypes/_rank",
        "codecallback_rankup",
        who.clone(),
        vec![Value::Int(to), Value::Int(prestige), Value::Int(tokens)],
    );
}

pub(super) fn bind(vm: &mut Vm<World>) {
    // The match's end: his stats written first, then the session takes it
    // from here (as natives_game's exitlevel).
    vm.bind("exitlevel", false, |_, world, _, _| {
        save(world, true);
        world.resource_mut::<crate::script::Runtime>().exit_level = true;
        Ok(Value::Undefined)
    });
    // A ranked match (Combat Training): XP counts.
    for name in ["gamemodeisusingxp", "gamemodeisusingstats"] {
        vm.bind(name, false, |_, world, _, _| Ok(Value::Int(i32::from(world.resource::<Zm>().ranked_match))));
    }
    // addrankxp(event, weapon, ...): the event's XP (mp/scoreInfo.csv) in a
    // ranked match, scaled by scr_xpscale.
    vm.bind("addrankxp", true, |vm, world, s, a| {
        if !world.resource::<Zm>().ranked_match {
            return Ok(Value::Undefined);
        }
        let Some(c) = client_of(vm, s) else { return Ok(Value::Undefined) };
        let event = text(vm, a, 0);
        let weapon = text(vm, a, 1);
        let gametype = vm.dvars.get("g_gametype").cloned().unwrap_or_default();
        let Some((xp, gun)) = event_xp(world, &gametype, &event) else { return Ok(Value::Undefined) };
        let scale = vm.dvars.get("scr_xpscale").and_then(|v| v.parse::<f32>().ok()).filter(|x| *x > 0.0).unwrap_or(1.0);
        give_xp(vm, world, s, c, (xp * scale).round(), Some((&weapon, gun)));
        Ok(Value::Undefined)
    });
    // addrankxpvalue(event, value): that much XP.
    vm.bind("addrankxpvalue", true, |vm, world, s, a| {
        if !world.resource::<Zm>().ranked_match {
            return Ok(Value::Undefined);
        }
        let Some(c) = client_of(vm, s) else { return Ok(Value::Undefined) };
        let xp = arg(a, 1).as_float().unwrap_or(0.0);
        give_xp(vm, world, s, c, xp, None);
        Ok(Value::Undefined)
    });
    // getdstat(path words...): the stat (0 never written).
    vm.bind("getdstat", true, |vm, world, s, a| {
        let Some(c) = client_of(vm, s) else { return Ok(Value::Int(0)) };
        let path = path_of(vm, a);
        let v = get(world, c, &path);
        Ok(to_value(vm, v))
    });
    // setdstat(path words..., value).
    vm.bind("setdstat", true, |vm, world, s, a| {
        if let (Some(c), Some((value, words))) = (client_of(vm, s), a.split_last()) {
            let path = path_of(vm, words);
            let v = of_value(vm, value);
            // The scripts' own XP (giverankxp's syncxpstat): a new rank
            // as the engine sees one.
            // (His own only: BO2's bot scripts write the bots' made-up
            // ranks the same way.)
            let xp = (path == RANK_XP && world.resource::<Zm>().ranked_match && is_local(world, c))
                .then(|| (num(world, c, RANK_XP), v.as_num().unwrap_or(0.0)));
            set(world, c, path, v);
            if let Some((old, new)) = xp {
                rank_up(vm, world, s, c, old, new);
            }
        }
        Ok(Value::Undefined)
    });
    // adddstat(path words..., amount).
    vm.bind("adddstat", true, |vm, world, s, a| {
        if let (Some(c), Some((value, words))) = (client_of(vm, s), a.split_last()) {
            let path = path_of(vm, words);
            let n = value.as_float().unwrap_or(0.0);
            add(world, c, path, n);
        }
        Ok(Value::Undefined)
    });
    // addplayerstat(name, amount): PlayerStatsList.<name>.StatValue.
    vm.bind("addplayerstat", true, |vm, world, s, a| {
        if let Some(c) = client_of(vm, s) {
            let name = text(vm, a, 0).to_ascii_lowercase();
            let n = arg(a, 1).as_float().unwrap_or(0.0);
            add(world, c, format!("playerstatslist.{name}.statvalue"), n);
        }
        Ok(Value::Undefined)
    });
    // addplayerstatwithgametype(name, amount): its game type's row too
    // (PlayerStatsByGameType.<gametype>.<name>.StatValue).
    vm.bind("addplayerstatwithgametype", true, |vm, world, s, a| {
        if let Some(c) = client_of(vm, s) {
            let name = text(vm, a, 0).to_ascii_lowercase();
            let n = arg(a, 1).as_float().unwrap_or(0.0);
            let gametype = vm.dvars.get("g_gametype").cloned().unwrap_or_default().to_ascii_lowercase();
            add(world, c, format!("playerstatsbygametype.{gametype}.{name}.statvalue"), n);
        }
        Ok(Value::Undefined)
    });
    // addweaponstat(weapon, stat, amount): ItemStats.<item>.stats.<stat>.
    vm.bind("addweaponstat", true, |vm, world, s, a| {
        if let Some(c) = client_of(vm, s) {
            let weapon = text(vm, a, 0);
            let stat = text(vm, a, 1).to_ascii_lowercase();
            let n = arg(a, 2).as_float().unwrap_or(0.0);
            if let Some(item) = weapon_item(world, c, &weapon) {
                add(world, c, format!("itemstats.{item}.stats.{stat}.statvalue"), n);
            }
        }
        Ok(Value::Undefined)
    });
    // getrankxpstat() / getcodpointsstat(): PlayerStatsList.rankxp / codpoints.
    vm.bind("getrankxpstat", true, |vm, world, s, _| {
        let Some(c) = client_of(vm, s) else { return Ok(Value::Int(0)) };
        let v = get(world, c, "playerstatslist.rankxp.statvalue");
        Ok(to_value(vm, v))
    });
    vm.bind("getcodpointsstat", true, |vm, world, s, _| {
        let Some(c) = client_of(vm, s) else { return Ok(Value::Int(0)) };
        let v = get(world, c, "playerstatslist.codpoints.statvalue");
        Ok(to_value(vm, v))
    });
}
