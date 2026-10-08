//! bo2mp: a player's class as Black Ops II's class script reads it
//! (`getloadoutweapon`, `getloadoutitem`, `getloadoutitemref`,
//! `getloadoutperks`, `isbonuscardactive`, `getweaponattachments`).
//!
//! Items come from his `mp/statstable.csv` (index, group, name, reference,
//! attachments, default classes, slot). A bot's class is what its own
//! scripts built (`botclassadditem`, `botclassaddattachment`); a player's
//! is BO2's default class for that slot (mp/reset_classes.cfg: the five
//! default custom classes, then the same five again) until his saved
//! classes are read: with his stats file (`stats`), the local player's
//! class is his own, as the front end's Create-a-Class saved it.

use bevy_ecs::prelude::World;
use gsc_t6::{Value, Vm};

use super::{Zm, arg, entnum, list, text};

const TABLE: &str = "mp/statstable.csv";

/// BO2's default custom classes in class order.
const DEFAULT_CLASSES: [&str; 5] = [
    "class_custom_assault",
    "class_custom_smg",
    "class_custom_lmg",
    "class_custom_cqb",
    "class_custom_sniper",
];

#[derive(Clone, Debug, Default)]
struct Item {
    index: i32,
    name: String,
    reference: String,
    attachments: Vec<String>,
    /// Default classes it is in, each with its extra words (a count).
    classes: Vec<(String, Vec<String>)>,
    slot: String,
    /// The HUD picture (a scorestreak's) and its momentum cost.
    icon: String,
    cost: i32,
}

fn items(world: &World) -> Vec<Item> {
    let zm = world.resource::<Zm>();
    let Some(t) = zm.tables.get(TABLE) else {
        return Vec::new();
    };
    let cell = |r: usize, c: usize| t.cells.get(r * t.columns + c).map_or("", String::as_str);
    let mut out = Vec::new();
    for r in 0..t.rows {
        let Ok(index) = cell(r, 0).parse::<i32>() else {
            continue;
        };
        if cell(r, 4).is_empty() {
            continue;
        }
        let mut classes: Vec<(String, Vec<String>)> = Vec::new();
        for w in cell(r, 11).split_whitespace() {
            if w.starts_with("class_") || w == "all" {
                classes.push((w.to_owned(), Vec::new()));
            } else if let Some(last) = classes.last_mut() {
                last.1.push(w.to_owned());
            }
        }
        out.push(Item {
            index,
            name: cell(r, 3).to_owned(),
            reference: cell(r, 4).to_owned(),
            attachments: cell(r, 8).split_whitespace().map(str::to_owned).collect(),
            classes,
            slot: cell(r, 13).to_owned(),
            icon: cell(r, 6).to_owned(),
            cost: cell(r, 16).parse().unwrap_or(0),
        });
    }
    out
}

/// A player's scorestreaks as BO2's HUD column lists them: `icon:cost`
/// by `;`, the three he picked (the default ones, in table order, until
/// his own are read), the dearest first (the column lists it top down).
pub(crate) fn streak_picks(world: &World) -> String {
    let mut picks: Vec<(String, i32)> = items(world)
        .into_iter()
        .filter(|i| i.slot.starts_with("killstreak") && i.classes.iter().any(|(c, _)| c == "all") && i.cost > 0)
        .map(|i| (i.icon, i.cost))
        .collect();
    picks.truncate(3);
    picks.sort_by_key(|p| std::cmp::Reverse(p.1));
    picks.iter().map(|(i, c)| format!("{i}:{c}")).collect::<Vec<_>>().join(";")
}

/// A player's perks as BO2's perk list draws them (the spawn list, the
/// killcam's killer card): `icon|name` by `;`, perk 1 to 3.
pub(crate) fn perk_card(vm: &mut Vm<World>, world: &mut World, who: &Value, class: i32) -> String {
    let all = items(world);
    let mut picks: Vec<i32> = Vec::new();
    if let Some(p) = his(vm, world, who, "specialty1") {
        picks = (1..=3).map(|n| p.loadout_item(class, &format!("specialty{n}"))).collect();
    } else {
        for n in 1..=3 {
            let slot = format!("specialty{n}");
            picks.push(slot_item(vm, world, who, class, &slot).map_or(0, |(i, _)| i.index));
        }
    }
    let rows: Vec<String> = picks
        .into_iter()
        .filter_map(|index| all.iter().find(|i| i.index == index && !i.reference.is_empty() && i.reference != "specialty_null"))
        .map(|it| format!("{}|{}", it.icon, it.name))
        .collect();
    rows.join(";")
}

/// A class's items by slot: (slot, item, extra words or attachments).
fn class_items(vm: &mut Vm<World>, world: &World, who: &Value, class: i32) -> Vec<(String, Item, Vec<String>)> {
    let all = items(world);
    let n = entnum(vm, who).unwrap_or(u32::MAX);
    let zm = world.resource::<Zm>();
    if let Some(bot) = zm.bots.get(&n)
        && let Some(c) = bot.classes.get(&class)
        && !c.items.is_empty()
    {
        let mut out = Vec::new();
        for name in &c.items {
            let Some(it) = all.iter().find(|i| i.name.eq_ignore_ascii_case(name) || i.reference == *name) else {
                continue;
            };
            let atts: Vec<String> = c
                .attachments
                .iter()
                .filter(|(w, _, _)| w.eq_ignore_ascii_case(&it.name) || *w == it.reference)
                .map(|(_, a, _)| a.clone())
                .collect();
            out.push((it.slot.clone(), it.clone(), atts));
        }
        return out;
    }
    let default = DEFAULT_CLASSES[(class.max(0) as usize) % DEFAULT_CLASSES.len()];
    all.iter()
        .filter_map(|it| {
            // "all": in every default class (the default scorestreaks).
            let (_, words) = it.classes.iter().find(|(c, _)| c == default || c == "all")?;
            Some((it.slot.clone(), it.clone(), words.clone()))
        })
        .collect()
}

fn slot_item(
    vm: &mut Vm<World>,
    world: &World,
    who: &Value,
    class: i32,
    slot: &str,
) -> Option<(Item, Vec<String>)> {
    // Scorestreaks are the player's, not a class's: `killstreakN` is the
    // Nth he picked - a bot's own picks (its scripts add them to class 0),
    // else the default ones, in table order.
    if let Some(n) = slot.strip_prefix("killstreak").and_then(|n| n.parse::<usize>().ok()) {
        return class_items(vm, world, who, 0)
            .into_iter()
            .filter(|(s, _, _)| s.starts_with("killstreak"))
            .nth(n.saturating_sub(1))
            .map(|(_, i, w)| (i, w));
    }
    class_items(vm, world, who, class)
        .into_iter()
        .find(|(s, _, _)| s == slot)
        .map(|(_, i, w)| (i, w))
}

/// The local player's own classes (his stats file), for a class slot his
/// classes hold (scorestreaks are not a class's: those stay as below).
fn his<'w>(vm: &Vm<World>, world: &'w mut World, who: &Value, slot: &str) -> Option<&'w bo2_profile::Profile> {
    let n = entnum(vm, who)?;
    let p = super::stats::local_profile(world, n)?;
    // Scorestreaks are class 0's, shared by every class; his picks count
    // only once he has made some (else the defaults, as below).
    (!slot.starts_with("killstreak") || p.has_streak_picks()).then_some(p)
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
    // The gun in a slot: `<reference>_mp`, a bot's attachments after it
    // (`+reflex+fmj`; the engine gives the base gun until attachments draw).
    m!("getloadoutweapon", |vm, world, s, a| {
        let class = arg(a, 0).as_int().unwrap_or(0);
        let slot = text(vm, a, 1);
        if let Some(p) = his(vm, world, s, &slot) {
            let w = p.loadout_weapon(class, &slot);
            return Ok(vm.string(&w));
        }
        let Some((it, extra)) = slot_item(vm, world, s, class, &slot) else {
            return Ok(vm.string("weapon_null_mp"));
        };
        let mut name = format!("{}_mp", it.reference);
        let is_bot = world
            .resource::<Zm>()
            .bots
            .contains_key(&entnum(vm, s).unwrap_or(u32::MAX));
        if is_bot {
            for att in extra.iter().filter(|a| it.attachments.contains(a)) {
                name.push('+');
                name.push_str(att);
            }
        }
        Ok(vm.string(&name))
    });
    m!("getloadoutitem", |vm, world, s, a| {
        let class = arg(a, 0).as_int().unwrap_or(0);
        let slot = text(vm, a, 1);
        let class = if slot.starts_with("killstreak") { 0 } else { class };
        if let Some(p) = his(vm, world, s, &slot) {
            return Ok(Value::Int(p.loadout_item(class, &slot)));
        }
        let item = slot_item(vm, world, s, class, &slot);
        // A grenade slot's count rides on its own name ("<slot>count").
        if let Some(base) = slot.strip_suffix("count") {
            let n = slot_item(vm, world, s, class, base).map_or(0, |(_, w)| {
                w.first().and_then(|w| w.parse::<i32>().ok()).unwrap_or(1)
            });
            return Ok(Value::Int(n));
        }
        Ok(Value::Int(item.map_or(0, |(i, _)| i.index)))
    });
    m!("getloadoutitemref", |vm, world, s, a| {
        let class = arg(a, 0).as_int().unwrap_or(0);
        let slot = text(vm, a, 1);
        let class = if slot.starts_with("killstreak") { 0 } else { class };
        if let Some(p) = his(vm, world, s, &slot) {
            let r = p.loadout_item_ref(class, &slot);
            return Ok(vm.string(&r));
        }
        Ok(match slot_item(vm, world, s, class, &slot) {
            Some((it, _)) => vm.string(&it.reference),
            None => vm.string(""),
        })
    });
    // The class's perks as the engine gives them (setperk names): every
    // specialty a perk item names (`specialty_movefaster|specialty_fallheight`).
    m!("getloadoutperks", |vm, world, s, a| {
        let class = arg(a, 0).as_int().unwrap_or(0);
        if let Some(p) = his(vm, world, s, "specialty1") {
            let perks = p.loadout_perks(class);
            return Ok(list(perks.iter().map(|n| vm.string(n)).collect()));
        }
        let mut out = Vec::new();
        for (slot, it, _) in class_items(vm, world, s, class) {
            if slot.starts_with("specialty") {
                for p in it.reference.split('|').filter(|p| !p.is_empty()) {
                    out.push(vm.string(p));
                }
            }
        }
        Ok(list(out))
    });
    m!("isbonuscardactive", |vm, world, s, a| {
        let card = arg(a, 0).as_int().unwrap_or(-1);
        let class = arg(a, 1).as_int().unwrap_or(0);
        if let Some(p) = his(vm, world, s, "bonuscard1") {
            return Ok(Value::bool(p.bonus_card_active(card, class)));
        }
        Ok(Value::bool(
            class_items(vm, world, s, class)
                .iter()
                .any(|(slot, it, _)| slot.starts_with("bonuscard") && it.index == card),
        ))
    });
    // A gun's item number from its weapon name (`mp7_mp+reflex` -> MP7's).
    f!("getbaseweaponitemindex", |vm, world, _, a| {
        let w = text(vm, a, 0);
        let base = w.split('+').next().unwrap_or(&w);
        let reference = base.strip_suffix("_mp").unwrap_or(base);
        Ok(Value::Int(
            items(world)
                .iter()
                .find(|i| i.reference == reference)
                .map_or(0, |i| i.index),
        ))
    });
    f!("getreffromitemindex", |vm, world, _, a| {
        let Some(index) = arg(a, 0).as_int() else {
            return Ok(vm.string(""));
        };
        let r = items(world)
            .into_iter()
            .find(|i| i.index == index)
            .map(|i| i.reference)
            .unwrap_or_default();
        Ok(vm.string(&r))
    });
    // A gun's attachments by its item name (`WEAPON_MK48`) or reference.
    f!("getweaponattachments", |vm, world, _, a| {
        let want = text(vm, a, 0);
        let atts = items(world)
            .into_iter()
            .find(|i| i.name.eq_ignore_ascii_case(&want) || i.reference == want)
            .map(|i| i.attachments)
            .unwrap_or_default();
        let v = atts.iter().map(|s| vm.string(s)).collect();
        Ok(list(v))
    });
}
