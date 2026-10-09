//! bo2mp: Black Ops II's third-person player animation script
//! (`mp/playeranim.script` in common_mp, with `mp/playeranimtypes.txt`), read
//! as the game reads it: per movement type (idle, walk, run, sprint, ...) and
//! per event (fireweapon, reload, jump, land, DEATH, ...) a list of items,
//! each a set of conditions (`playerAnimType hold, stance prone`) and the
//! animations that play when they are the first to match (one at random
//! when several are listed).
//!
//! The server picks the animations as the game does (`legsAnim` /
//! `torsoAnim` in the player's state) and every client plays them; both
//! sides read the same script, so an animation goes over the wire as its
//! place in [`T6PlayerAnims::anims`].

/// A packed animation value (the player state's `legs_anim` / `torso_anim`):
/// the animation's number (its index in [`T6PlayerAnims::anims`] plus one,
/// 0 = none) and a bit that flips each time it is started again.
pub const T6_ANIM_INDEX_MASK: i32 = 0x0fff;
pub const T6_ANIM_TOGGLE: i32 = 0x1000;

/// The parsed script as a resource (each machine reads it from the zones
/// it loaded; `None` when they hold none).
#[derive(bevy_ecs::prelude::Resource, Clone, Default)]
pub struct T6PlayerAnimsRes(pub Option<std::sync::Arc<T6PlayerAnims>>);

impl T6PlayerAnimsRes {
    /// Parse `(mp/playeranim.script, mp/playeranimtypes.txt)`; a script that
    /// does not parse is logged and left out.
    pub fn from_text(text: Option<&(String, String)>, speeds: &[(String, f32)]) -> Self {
        let Some((script, types)) = text else {
            return Self(None);
        };
        match T6PlayerAnims::parse(script, types) {
            Ok(mut parsed) => {
                parsed.speeds = parsed
                    .anims
                    .iter()
                    .map(|a| speeds.iter().find(|(n, _)| n == a).map_or(0.0, |(_, v)| *v))
                    .collect();
                // A climb that moves is a ladder animation: it plays at his
                // climbing speed (T5 BG_AnimParseAnimScript flags a
                // climbup/climbdown animation with a move speed).
                let mut ladder = vec![false; parsed.anims.len()];
                for (movetype, items) in &parsed.moves {
                    if movetype == "climbup" || movetype == "climbdown" {
                        for c in items.iter().flat_map(|i| &i.commands) {
                            let i = usize::from(c.anim);
                            if parsed.speeds.get(i).is_some_and(|v| *v != 0.0) {
                                ladder[i] = true;
                            }
                        }
                    }
                }
                parsed.ladder = ladder;
                Self(Some(std::sync::Arc::new(parsed)))
            }
            Err(e) => {
                diag::warn!(Sim, "bo2mp: mp/playeranim.script not read: {e}");
                Self(None)
            }
        }
    }
}

/// Which part of the body a command plays on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T6AnimPart {
    Legs,
    Torso,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct T6AnimCommand {
    pub part: T6AnimPart,
    /// Index into [`T6PlayerAnims::anims`].
    pub anim: u16,
    pub blend_ms: Option<i32>,
    pub blend_out_ms: Option<i32>,
    pub duration_ms: Option<i32>,
    /// `weaponTimeScale`: the animation plays over the weapon's own time
    /// for the action (its raise or drop time), not its own length.
    pub weapon_time_scale: bool,
    /// `grenadeAnim`: the animation handles the grenade or piece of
    /// equipment (a throw, a plant): that item is in his hand.
    pub grenade_anim: bool,
    /// `animrate`: the animation always plays at this rate, never at his
    /// speed over its own (BO2's dives say `animrate 1`).
    pub anim_rate: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Cond {
    /// Lower-case condition name (`playeranimtype`, `stance`, ...).
    name: String,
    /// Values that match (lower case).
    values: Vec<String>,
    /// `all NOT a NOT b`: every value but these.
    all_but: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Item {
    /// Empty = `default` (always matches).
    conds: Vec<Cond>,
    commands: Vec<T6AnimCommand>,
}

/// What a player is doing, in the script's own words (all lower case).
#[derive(Clone, Copy, Debug)]
pub struct T6AnimFacts<'a> {
    /// The weapon's `playerAnimType` (`default`, `hold`, `dualwield`, ...).
    pub anim_type: &'a str,
    /// The weapon's class (`rifle`, `smg`, `mg`, `pistol`, `spread`, ...).
    pub weapon_class: &'a str,
    /// `stand`, `crouch` or `prone`.
    pub stance: &'a str,
    /// `forward`, `backward`, `left` or `right`.
    pub direction: &'a str,
    /// `stationary`, `walk` or `run`.
    pub movestatus: &'a str,
    /// `hip` or `ads`.
    pub weapon_position: &'a str,
    /// Death only: `explosive`, `headshot`, `melee`, `normal_shotgun`, ...
    pub dmg_type: &'a str,
    /// Death only: `front`, `back`, `left`, `right`.
    pub dmg_direction: &'a str,
}

impl Default for T6AnimFacts<'_> {
    fn default() -> Self {
        Self {
            anim_type: "default",
            weapon_class: "rifle",
            stance: "stand",
            direction: "forward",
            movestatus: "stationary",
            weapon_position: "hip",
            dmg_type: "",
            dmg_direction: "",
        }
    }
}

/// Black Ops II's player animation script, parsed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct T6PlayerAnims {
    /// Every animation the script names, in first-named order.
    pub anims: Vec<String>,
    /// Each one's move speed (units/s of root motion; 0 = in place), when
    /// known: a moving legs animation plays at the player's speed over it.
    pub speeds: Vec<f32>,
    /// Each one is a ladder climb (its rate follows his climbing speed).
    pub ladder: Vec<bool>,
    /// The `playerAnimType` names in `playeranimtypes.txt` order (a weapon's
    /// `playerAnimType` is its place here).
    pub anim_types: Vec<String>,
    moves: Vec<(String, Vec<Item>)>,
    events: Vec<(String, Vec<Item>)>,
    /// `set <cond> <alias> = ...`: (cond, alias, condition it stands for).
    defines: Vec<(String, String, Cond)>,
}

/// One token of the script: a word, `,`, `{`, `}`, or a line break.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Word(String),
    Comma,
    Open,
    Close,
    Eol,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        };
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut Vec<Tok>| {
            if !word.is_empty() {
                out.push(Tok::Word(std::mem::take(word)));
            }
        };
        for c in line.chars() {
            match c {
                '{' => {
                    flush(&mut word, &mut out);
                    out.push(Tok::Open);
                }
                '}' => {
                    flush(&mut word, &mut out);
                    out.push(Tok::Close);
                }
                ',' => {
                    flush(&mut word, &mut out);
                    out.push(Tok::Comma);
                }
                c if c.is_whitespace() => flush(&mut word, &mut out),
                c => word.push(c),
            }
        }
        flush(&mut word, &mut out);
        out.push(Tok::Eol);
    }
    out
}

impl T6PlayerAnims {
    /// Parse the script text and the anim type list. Errors name the line
    /// shape that was not understood.
    pub fn parse(script: &str, anim_types: &str) -> Result<Self, String> {
        let mut out = Self {
            anim_types: anim_types
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect(),
            ..Self::default()
        };
        let toks = tokenize(script);
        let mut i = 0usize;
        #[derive(PartialEq)]
        enum Section {
            None,
            Defines,
            Animations,
            Events,
        }
        let mut section = Section::None;
        while i < toks.len() {
            match &toks[i] {
                Tok::Eol | Tok::Comma => i += 1,
                Tok::Word(w) if w == "DEFINES" => {
                    section = Section::Defines;
                    i += 1;
                }
                Tok::Word(w) if w == "ANIMATIONS" => {
                    section = Section::Animations;
                    i += 1;
                }
                Tok::Word(w) if w == "EVENTS" => {
                    section = Section::Events;
                    i += 1;
                }
                Tok::Word(w) if section == Section::Defines => {
                    // One line: `set <cond> <alias> = v AND v` or a `#...`.
                    let mut line = Vec::new();
                    while i < toks.len() && toks[i] != Tok::Eol {
                        if let Tok::Word(w) = &toks[i] {
                            line.push(w.to_ascii_lowercase());
                        }
                        i += 1;
                    }
                    if w.eq_ignore_ascii_case("set") && line.len() >= 5 && line[3] == "=" {
                        let cond = out.cond_from(&line[1], &line[4..]);
                        out.defines.push((line[1].clone(), line[2].clone(), cond));
                    }
                }
                Tok::Word(w) if section == Section::Animations && w == "STATE" => {
                    // STATE COMBAT { <move> { items } ... }
                    i += 1;
                    while i < toks.len() && toks[i] != Tok::Open {
                        i += 1;
                    }
                    i += 1;
                    loop {
                        while i < toks.len() && toks[i] == Tok::Eol {
                            i += 1;
                        }
                        match toks.get(i) {
                            None => break,
                            Some(Tok::Close) => {
                                i += 1;
                                break;
                            }
                            Some(Tok::Word(name)) => {
                                let name = name.to_ascii_lowercase();
                                i += 1;
                                let items = out.parse_block(&toks, &mut i)?;
                                out.moves.push((name, items));
                            }
                            Some(t) => return Err(format!("movement list: unexpected {t:?}")),
                        }
                    }
                }
                Tok::Word(name) if section == Section::Events => {
                    let name = name.to_ascii_lowercase();
                    i += 1;
                    if name == "forceload" {
                        // A list of names, not conditions.
                        let mut depth = 0;
                        while i < toks.len() {
                            match toks[i] {
                                Tok::Open => depth += 1,
                                Tok::Close => {
                                    depth -= 1;
                                    if depth <= 0 {
                                        i += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                        continue;
                    }
                    let items = out.parse_block(&toks, &mut i)?;
                    out.events.push((name, items));
                }
                _ => i += 1,
            }
        }
        if out.moves.is_empty() {
            return Err("no movement types".into());
        }
        // The engine's own picks beside the script's: the mantle's climb up
        // to a ledge (by its height) and the fast-mantle copies.
        for height in [57, 51, 45, 39, 33, 27, 21] {
            out.anim_index(&format!("mp_mantle_up_{height}"));
            out.anim_index(&format!("mp_mantle_up_{height}_fast"));
        }
        for over in ["high", "mid", "low"] {
            out.anim_index(&format!("mp_mantle_over_{over}_fast"));
        }
        Ok(out)
    }

    /// `{ <conds> { commands } ... }` from the opening brace.
    fn parse_block(&mut self, toks: &[Tok], i: &mut usize) -> Result<Vec<Item>, String> {
        while *i < toks.len() && toks[*i] == Tok::Eol {
            *i += 1;
        }
        if toks.get(*i) != Some(&Tok::Open) {
            return Err(format!("expected {{, found {:?}", toks.get(*i)));
        }
        *i += 1;
        let mut items = Vec::new();
        loop {
            while *i < toks.len() && toks[*i] == Tok::Eol {
                *i += 1;
            }
            match toks.get(*i) {
                None => return Err("unclosed block".into()),
                Some(Tok::Close) => {
                    *i += 1;
                    return Ok(items);
                }
                // A bare `{ }` with no conditions: commands that always
                // play (none in the shipped script, kept for safety).
                Some(Tok::Open) => {
                    let commands = self.parse_commands(toks, i)?;
                    items.push(Item {
                        conds: Vec::new(),
                        commands,
                    });
                }
                Some(_) => {
                    // Conditions up to the opening brace.
                    let mut groups: Vec<Vec<String>> = vec![Vec::new()];
                    while let Some(t) = toks.get(*i) {
                        match t {
                            Tok::Open => break,
                            Tok::Comma => groups.push(Vec::new()),
                            Tok::Word(w) => {
                                if let Some(g) = groups.last_mut() {
                                    g.push(w.to_ascii_lowercase());
                                }
                            }
                            Tok::Eol => {}
                            Tok::Close => return Err("} in a condition line".into()),
                        }
                        *i += 1;
                    }
                    let mut conds = Vec::new();
                    for g in groups.into_iter().filter(|g| !g.is_empty()) {
                        if g.len() == 1 && g[0] == "default" {
                            continue;
                        }
                        let name = g[0].clone();
                        conds.push(self.cond_from(&name, &g[1..]));
                    }
                    let commands = self.parse_commands(toks, i)?;
                    items.push(Item { conds, commands });
                }
            }
        }
    }

    /// `{ <part> <anim> [modifiers] ... }` from the opening brace.
    fn parse_commands(&mut self, toks: &[Tok], i: &mut usize) -> Result<Vec<T6AnimCommand>, String> {
        if toks.get(*i) != Some(&Tok::Open) {
            return Err("expected { before commands".into());
        }
        *i += 1;
        let mut out = Vec::new();
        let mut line: Vec<String> = Vec::new();
        let finish = |line: &mut Vec<String>, this: &mut Self, out: &mut Vec<T6AnimCommand>| {
            let words = std::mem::take(line);
            if words.len() < 2 {
                return;
            }
            let part = match words[0].to_ascii_lowercase().as_str() {
                "legs" => T6AnimPart::Legs,
                "torso" => T6AnimPart::Torso,
                "both" => T6AnimPart::Both,
                _ => return,
            };
            let anim = this.anim_index(&words[1]);
            let mut command = T6AnimCommand {
                part,
                anim,
                blend_ms: None,
                blend_out_ms: None,
                duration_ms: None,
                weapon_time_scale: false,
                grenade_anim: false,
                anim_rate: None,
            };
            let mut k = 2;
            while k < words.len() {
                let key = words[k].to_ascii_lowercase();
                let value = words.get(k + 1).and_then(|v| v.parse::<i32>().ok());
                match key.as_str() {
                    "blendtime" => command.blend_ms = value,
                    "blendouttime" => command.blend_out_ms = value,
                    "duration" => command.duration_ms = value,
                    "animrate" => {
                        command.anim_rate = words.get(k + 1).and_then(|v| v.parse::<f32>().ok()).filter(|r| *r > 0.0);
                    }
                    "weapontimescale" => {
                        command.weapon_time_scale = true;
                        k += 1;
                        continue;
                    }
                    "grenadeanim" => {
                        command.grenade_anim = true;
                        k += 1;
                        continue;
                    }
                    _ => {
                        k += 1;
                        continue;
                    }
                }
                k += 2;
            }
            out.push(command);
        };
        loop {
            match toks.get(*i) {
                None => return Err("unclosed command block".into()),
                Some(Tok::Close) => {
                    finish(&mut line, self, &mut out);
                    *i += 1;
                    return Ok(out);
                }
                Some(Tok::Eol) => finish(&mut line, self, &mut out),
                Some(Tok::Word(w)) => line.push(w.clone()),
                Some(Tok::Comma) => {}
                Some(Tok::Open) => return Err("{ inside commands".into()),
            }
            *i += 1;
        }
    }

    fn anim_index(&mut self, name: &str) -> u16 {
        let lower = name.to_ascii_lowercase();
        if let Some(i) = self.anims.iter().position(|a| *a == lower) {
            return i as u16;
        }
        self.anims.push(lower);
        (self.anims.len() - 1) as u16
    }

    /// A condition from its name and value words (`a AND b`, `all NOT a`,
    /// a defined alias).
    fn cond_from(&self, name: &str, words: &[String]) -> Cond {
        let mut cond = Cond {
            name: name.to_owned(),
            ..Cond::default()
        };
        let mut negate = false;
        for w in words {
            match w.as_str() {
                "and" => {}
                "not" => negate = true,
                "all" => cond.all_but = Some(Vec::new()),
                v => {
                    if negate {
                        cond.all_but.get_or_insert_with(Vec::new).push(v.to_owned());
                        negate = false;
                    } else if let Some((_, _, def)) = self
                        .defines
                        .iter()
                        .find(|(c, alias, _)| c == name && alias == v)
                    {
                        cond.values.extend(def.values.iter().cloned());
                        if def.all_but.is_some() {
                            cond.all_but = def.all_but.clone();
                        }
                    } else {
                        cond.values.push(v.to_owned());
                    }
                }
            }
        }
        cond
    }

    /// The animation's name for a packed value (`None` for none).
    pub fn name_of(&self, packed: i32) -> Option<&str> {
        let n = packed & T6_ANIM_INDEX_MASK;
        if n == 0 {
            return None;
        }
        self.anims.get((n - 1) as usize).map(String::as_str)
    }

    /// An animation's move speed (0 = in place or unknown).
    pub fn speed_of(&self, anim: u16) -> f32 {
        self.speeds.get(usize::from(anim)).copied().unwrap_or(0.0)
    }

    /// A ladder climb animation (see [`Self::ladder`]).
    pub fn is_ladder(&self, anim: u16) -> bool {
        self.ladder.get(usize::from(anim)).copied().unwrap_or(false)
    }

    /// The first command naming this animation (its blend and duration).
    pub fn command_of(&self, anim: u16) -> Option<&T6AnimCommand> {
        self.moves
            .iter()
            .chain(&self.events)
            .flat_map(|(_, items)| items)
            .flat_map(|item| &item.commands)
            .find(|c| c.anim == anim)
    }

    /// The `playerAnimType` name of a weapon's type number.
    pub fn anim_type_name(&self, index: i32) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.anim_types.get(i))
            .map_or("default", String::as_str)
    }

    pub fn has_move(&self, movetype: &str) -> bool {
        self.moves.iter().any(|(m, _)| m == movetype)
    }

    pub fn has_event(&self, event: &str) -> bool {
        self.events.iter().any(|(e, _)| e == event)
    }

    /// The commands of the first item of a movement type that matches:
    /// `None` when nothing matched (or no such type), else its commands
    /// (empty = matched, plays nothing).
    pub fn pick_move(&self, movetype: &str, facts: &T6AnimFacts<'_>) -> Option<&[T6AnimCommand]> {
        let (_, items) = self.moves.iter().find(|(m, _)| m == movetype)?;
        pick(items, facts)
    }

    pub fn pick_event(&self, event: &str, facts: &T6AnimFacts<'_>) -> Option<&[T6AnimCommand]> {
        let (_, items) = self.events.iter().find(|(e, _)| e == event)?;
        pick(items, facts)
    }

    pub fn report_line(&self) -> String {
        format!(
            "t6 playeranim.script: {} movement types, {} events, {} animations, {} anim types",
            self.moves.len(),
            self.events.len(),
            self.anims.len(),
            self.anim_types.len()
        )
    }
}

fn pick<'s>(items: &'s [Item], facts: &T6AnimFacts<'_>) -> Option<&'s [T6AnimCommand]> {
    items
        .iter()
        .find(|item| item.conds.iter().all(|c| cond_holds(c, facts)))
        .map(|item| item.commands.as_slice())
}

fn cond_holds(cond: &Cond, facts: &T6AnimFacts<'_>) -> bool {
    let value = match cond.name.as_str() {
        "playeranimtype" => facts.anim_type,
        "weaponclass" => facts.weapon_class,
        "stance" => facts.stance,
        "direction" => facts.direction,
        "movestatus" => facts.movestatus,
        "weapon_position" => facts.weapon_position,
        "dmgtype" => facts.dmg_type,
        "dmgdirection" => facts.dmg_direction,
        // Mounted guns, vehicles, perks, the next weapon, slopes...: states
        // these players are never in.
        _ => return false,
    };
    if value.is_empty() {
        return false;
    }
    if let Some(but) = &cond.all_but {
        return !but.iter().any(|v| v == value);
    }
    cond.values.iter().any(|v| v == value)
}

/// Pack an animation (index into the table) with its restart bit.
pub fn t6_pack_anim(anim: u16, toggle: bool) -> i32 {
    (i32::from(anim) + 1) & T6_ANIM_INDEX_MASK | if toggle { T6_ANIM_TOGGLE } else { 0 }
}

/// The legs' yaw offset from the view (degrees, 2-degree steps) packed in
/// bits 16..23 of a legs value: the legs turn in place behind the view and
/// twist to a diagonal move (the torso keeps to the view).
pub fn t6_with_legs_yaw(packed: i32, offset_deg: f32) -> i32 {
    let steps = (offset_deg / 2.0).round().clamp(-127.0, 127.0) as i32;
    (packed & 0xffff) | ((steps & 0xff) << 16)
}

/// The legs' yaw offset (degrees) a legs value carries.
pub fn t6_legs_yaw(packed: i32) -> f32 {
    f32::from(((packed >> 16) & 0xff) as u8 as i8) * 2.0
}

/// How long a torso animation is to take (ms, 40 ms steps, up to 10.2 s),
/// packed in bits 16..23 of a torso value: an animation timed to the
/// weapon (a reload, a raise or drop) plays over that time. 0 = its own
/// length.
pub fn t6_with_duration(packed: i32, ms: i32) -> i32 {
    let steps = ((ms + 20) / 40).clamp(0, 255);
    (packed & 0xffff) | (steps << 16)
}

/// The time a torso value's animation is to take (seconds), when it is
/// timed to the weapon.
pub fn t6_anim_duration(packed: i32) -> Option<f32> {
    let steps = (packed >> 16) & 0xff;
    (steps != 0).then(|| steps as f32 * 0.04)
}

/// The grenade or piece of equipment in his hand for a torso animation
/// that handles it (the script's grenadeAnim: a throw, a plant), packed in
/// bits 24..31 of a torso value (its weapon index, 1..255).
pub fn t6_with_offhand(packed: i32, weapon: u32) -> i32 {
    let w = weapon.min(255) as i32;
    (packed & 0x00ff_ffff) | (w << 24)
}

/// The grenade or piece of equipment a torso value puts in his hand.
pub fn t6_offhand(packed: i32) -> Option<u32> {
    let w = ((packed >> 24) & 0xff) as u32;
    (w != 0).then_some(w)
}

/// A packed value without the legs' yaw: the animation and its restart bit.
pub fn t6_anim_part(packed: i32) -> i32 {
    packed & (T6_ANIM_INDEX_MASK | T6_ANIM_TOGGLE)
}

/// The index into the table of a packed value (`None` for none).
pub fn t6_anim_index(packed: i32) -> Option<u16> {
    let n = packed & T6_ANIM_INDEX_MASK;
    (n != 0).then(|| (n - 1) as u16)
}

/// A weapon's class (the engine's IW4-numbered class) in the script's words.
pub fn t6_weapon_class_name(iw4_class: i32) -> &'static str {
    match iw4_class {
        0 => "rifle",
        1 => "sniper",
        2 => "mg",
        3 => "smg",
        4 => "spread",
        5 => "pistol",
        6 => "grenade",
        7 => "rocketlauncher",
        8 => "turret",
        10 => "non-player",
        11 => "item",
        _ => "rifle",
    }
}

/// The 4-way direction of a move (`forward`, `backward`, `left`, `right`)
/// from the velocity and the view's yaw (degrees).
pub fn t6_move_direction(velocity: [f32; 3], yaw_deg: f32) -> &'static str {
    let speed = (velocity[0] * velocity[0] + velocity[1] * velocity[1]).sqrt();
    if speed < 1.0 {
        return "forward";
    }
    let heading = velocity[1].atan2(velocity[0]).to_degrees();
    let mut rel = heading - yaw_deg;
    while rel > 180.0 {
        rel -= 360.0;
    }
    while rel < -180.0 {
        rel += 360.0;
    }
    if rel.abs() <= 50.0 {
        "forward"
    } else if rel.abs() >= 130.0 {
        "backward"
    } else if rel > 0.0 {
        "left"
    } else {
        "right"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "DEFINES\nset movestatus moving = walk AND run\nset playerAnimType grenadeonly = all NOT m203 NOT briefcase\n#ADD_VEHICLE x\nANIMATIONS\nSTATE COMBAT\n{\n\tidle\n\t{\n\t\tplayerAnimType hold, stance prone\n\t\t{\n\t\t\tboth pb_prone_hold grenadeAnim\n\t\t}\n\t\tweaponclass pistol AND pistol_spread\n\t\t{\n\t\t\tboth pb_stand_alert_pistol\n\t\t}\n\t\tdefault // two handed\n\t\t{\n\t\t\tboth pb_stand_alert\n\t\t}\n\t}\n}\nEVENTS\nfireweapon\n{\n\tmounted mg42\n\t{\n\t}\n\tplayerAnimType grenadeonly, movestatus moving\n\t{\n\t\ttorso pt_stand_shoot blendTime 200 duration 150\n\t}\n}\nforceload\n{\n\thead\n\tpf_death\n}\n";

    #[test]
    fn parses_and_picks() {
        let s = T6PlayerAnims::parse(SCRIPT, "none\ndefault\nhold\nm203").unwrap();
        assert_eq!(s.anims, ["pb_prone_hold", "pb_stand_alert_pistol", "pb_stand_alert", "pt_stand_shoot"]);
        let mut f = T6AnimFacts::default();
        assert_eq!(s.pick_move("idle", &f).unwrap()[0].anim, 2);
        f.weapon_class = "pistol";
        assert_eq!(s.pick_move("idle", &f).unwrap()[0].anim, 1);
        f.anim_type = "hold";
        f.stance = "prone";
        assert_eq!(s.pick_move("idle", &f).unwrap()[0].anim, 0);
        f.movestatus = "run";
        let fire = s.pick_event("fireweapon", &f).unwrap();
        assert_eq!(fire[0].part, T6AnimPart::Torso);
        assert_eq!(fire[0].duration_ms, Some(150));
        assert_eq!(fire[0].blend_ms, Some(200));
        f.anim_type = "m203";
        assert!(s.pick_event("fireweapon", &f).is_none());
        assert!(!s.has_event("forceload"));
        assert_eq!(s.anim_type_name(2), "hold");
        assert_eq!(t6_anim_index(t6_pack_anim(3, true)), Some(3));
        let v = t6_with_legs_yaw(t6_pack_anim(3, true), -45.0);
        assert_eq!(t6_anim_index(v), Some(3));
        assert_eq!(t6_legs_yaw(v), -46.0);
        assert_eq!(t6_anim_part(v), t6_pack_anim(3, true));
        let d = t6_with_duration(t6_pack_anim(3, false), 2500);
        assert_eq!(t6_anim_index(d), Some(3));
        assert_eq!(t6_anim_duration(d), Some(2.52));
        let o = t6_with_offhand(d, 200);
        assert_eq!(t6_offhand(o), Some(200));
        assert_eq!(t6_anim_index(o), Some(3));
        assert_eq!(t6_anim_duration(o), Some(2.52));
    }

    #[test]
    fn directions() {
        assert_eq!(t6_move_direction([100.0, 0.0, 0.0], 0.0), "forward");
        assert_eq!(t6_move_direction([-100.0, 0.0, 0.0], 0.0), "backward");
        assert_eq!(t6_move_direction([0.0, 100.0, 0.0], 0.0), "left");
        assert_eq!(t6_move_direction([0.0, -100.0, 0.0], 0.0), "right");
    }
}
