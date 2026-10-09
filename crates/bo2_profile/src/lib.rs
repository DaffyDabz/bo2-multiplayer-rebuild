//! bo2mp: Black Ops II multiplayer's items, classes and the player's
//! stats, shared by the front end's menus (`hks_t6::mp`) and the server
//! (the loadout a player spawns with). Std only.
//!
//! Items come from his `mp/statstable.csv` (column 0 index, 2 group, 3 name
//! key, 4 reference - a perk's specialties joined by `|` -, 6 image, 7
//! description key, 8 the attachments a weapon takes, 9 order within the
//! group, 10 unlock rank (0 = level 1), 11 the default classes it is in,
//! 12 Pick-10 cost, 13 loadout slot, 16 scorestreak cost) and
//! `mp/attachmenttable.csv` (0 index, 3 name key, 4 reference, 6 image, 7
//! description key, 12 cost).
//!
//! His stats are one store of values by stats path, the names the scripts
//! use, lower case, array indices as numbers: `playerstatslist.rank.
//! statvalue`, `custommatchcacloadouts.customclass.0.primary`. A value never
//! written reads 0. A class slot holds an item index, a weapon's
//! `<slot>attachment<n>` the attachment's number in that weapon's own list
//! (1 first, 0 none), a grenade's `<slot>count` the count.

use std::collections::HashMap;
use std::path::Path;

/// The class slots (the menus' CoD.CACUtility.loadoutSlotNames and the
/// attachment, option and count slots beside them).
pub const SLOTS: [&str; 24] = [
    "primary",
    "primaryattachment1",
    "primaryattachment2",
    "primaryattachment3",
    "primarycamo",
    "primaryreticle",
    "secondary",
    "secondaryattachment1",
    "secondaryattachment2",
    "secondarycamo",
    "secondaryreticle",
    "primarygrenade",
    "primarygrenadecount",
    "specialgrenade",
    "specialgrenadecount",
    "specialty1",
    "specialty2",
    "specialty3",
    "specialty4",
    "specialty5",
    "specialty6",
    "bonuscard1",
    "bonuscard2",
    "bonuscard3",
];

/// BO2's default custom classes, in class order (mp/reset_classes.cfg:
/// `equipdefaultclass 0 class_custom_assault` ... and the same five again
/// for classes 5-9; the offline profile, custom match and league sets take
/// the first five).
pub const DEFAULT_CLASSES: [&str; 5] =
    ["class_custom_assault", "class_custom_smg", "class_custom_lmg", "class_custom_cqb", "class_custom_sniper"];

/// The class sets and how many classes each holds.
pub const CLASS_SETS: [(&str, usize); 4] =
    [("profile.cacloadouts", 5), ("cacloadouts", 10), ("custommatchcacloadouts", 5), ("leaguecacloadouts", 5)];

/// The set a local match uses: BO2's ONLINE -> CUSTOM GAMES private match
/// (online session + private match game mode; see `class_set`).
pub const LOCAL_MATCH_SET: &str = "custommatchcacloadouts";

/// The class set the menus edit and a match gives, as BO2's scripts choose
/// it (CACUtility's `cacRoot`): offline, the exe profile's classes; online,
/// the custom match set in a private match, the league set in league play,
/// else the public one.
pub fn class_set(online: bool, private_match: bool, league: bool) -> &'static str {
    if !online {
        "profile.cacloadouts"
    } else if private_match {
        "custommatchcacloadouts"
    } else if league {
        "leaguecacloadouts"
    } else {
        "cacloadouts"
    }
}

/// BO2's unlock token pool (`unlocks[0]`).
pub const UNLOCK_TOKENS: &str = "unlocks.0";

/// One `mp/statstable.csv` row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub index: i32,
    pub group: String,
    pub name: String,
    pub reference: String,
    pub image: String,
    pub desc: String,
    /// The attachments a weapon takes, in the order the menus number them
    /// (1 first).
    pub attachments: Vec<String>,
    /// Its place within its group (column 9).
    pub sort: i32,
    /// The rank (0 = level 1) it unlocks at.
    pub unlock_rank: i32,
    /// The default classes it is in, with each one's extra words
    /// (attachments, or a grenade count).
    pub classes: Vec<(String, Vec<String>)>,
    pub cost: i32,
    pub slot: String,
    pub momentum: i32,
    /// Bought with an unlock token once its rank is reached (column 17
    /// set); the rest (the level-4 starter set: MP7, MTAR, Five-seven,
    /// Lightweight, UAV, ...) come free with their rank.
    pub token: bool,
}

/// One `mp/attachmenttable.csv` row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachment {
    pub index: i32,
    pub reference: String,
    pub name: String,
    pub image: String,
    pub desc: String,
    pub cost: i32,
}

/// A stat: a number or text.
#[derive(Clone, Debug, PartialEq)]
pub enum StatValue {
    Num(f32),
    Str(String),
}

impl StatValue {
    pub fn as_num(&self) -> Option<f32> {
        match self {
            StatValue::Num(n) => Some(*n),
            StatValue::Str(s) => s.trim().parse().ok(),
        }
    }
}

fn cell(row: &[String], i: usize) -> String {
    row.get(i).map_or_else(String::new, |c| c.trim().to_owned())
}

fn int(row: &[String], i: usize) -> i32 {
    cell(row, i).parse::<i32>().unwrap_or(0)
}

/// A number as the stats file writes it (whole numbers without a point).
fn number_text(n: f32) -> String {
    if n == n.trunc() && n.abs() < 1e9 { format!("{}", n as i64) } else { format!("{n}") }
}

/// The items, the attachments and his stats.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    /// The items by index (gaps are empty items).
    pub items: Vec<Item>,
    pub attachments: Vec<Attachment>,
    /// His stats by path (see the crate notes).
    pub stats: HashMap<String, StatValue>,
    /// A stat changed since the owner last saved (`take_dirty`).
    pub dirty: bool,
    /// The class set `loadout_*` read (`LOCAL_MATCH_SET` unless set).
    pub match_set: String,
}

impl Profile {
    /// The items from his string tables (rows of cells), the default
    /// classes filled into every class set (only slots `stats` lacks: read
    /// his file with `load_text` first), the class names from `localize`
    /// (`CLASS_SLOT1` -> "Custom 1").
    pub fn from_tables(
        statstable: &[Vec<String>],
        attachmenttable: &[Vec<String>],
        localize: &dyn Fn(&str) -> Option<String>,
        stats: HashMap<String, StatValue>,
    ) -> Profile {
        let mut p = Profile { stats, match_set: LOCAL_MATCH_SET.to_owned(), ..Profile::default() };
        for row in statstable {
            let Ok(index) = cell(row, 0).parse::<i32>() else { continue };
            if cell(row, 4).is_empty() {
                continue;
            }
            // Column 11: `class_smg steadyaim class_custom_smg` - each
            // class name, then its words.
            let mut classes: Vec<(String, Vec<String>)> = Vec::new();
            for w in cell(row, 11).split_whitespace() {
                if w.starts_with("class_") || w == "all" {
                    classes.push((w.to_owned(), Vec::new()));
                } else if let Some(last) = classes.last_mut() {
                    last.1.push(w.to_owned());
                }
            }
            let at = index.max(0) as usize;
            if p.items.len() <= at {
                p.items.resize(at + 1, Item::default());
            }
            p.items[at] = Item {
                index,
                group: cell(row, 2),
                name: cell(row, 3),
                reference: cell(row, 4),
                image: cell(row, 6),
                desc: cell(row, 7),
                attachments: cell(row, 8).split_whitespace().map(str::to_owned).collect(),
                sort: int(row, 9),
                unlock_rank: int(row, 10),
                classes,
                cost: int(row, 12),
                slot: cell(row, 13),
                momentum: int(row, 16),
                token: !cell(row, 17).is_empty(),
            };
        }
        for row in attachmenttable {
            let Ok(index) = cell(row, 0).parse::<i32>() else { continue };
            p.attachments.push(Attachment {
                index,
                reference: cell(row, 4),
                name: cell(row, 3),
                image: cell(row, 6),
                desc: cell(row, 7),
                cost: int(row, 12),
            });
        }
        p.default_loadouts(localize);
        p
    }

    /// The default classes in every class set, as mp/reset_classes*.cfg
    /// equip them (only slots not saved): the offline profile's 5
    /// (`equipdefaultclasstoprofile`), the public 10 (the five twice), the
    /// custom match and league 5. Names are BO2's "CLASS_SLOT<n>" text
    /// (`setprofilelocclass` / `setStatFromLocString` store the text).
    fn default_loadouts(&mut self, localize: &dyn Fn(&str) -> Option<String>) {
        for (set, count) in CLASS_SETS {
            for n in 0..count {
                for (slot, v) in self.default_class(DEFAULT_CLASSES[n % 5]) {
                    self.stats.entry(format!("{set}.customclass.{n}.{slot}")).or_insert(StatValue::Num(v as f32));
                }
                let loc = format!("CLASS_SLOT{}", n + 1);
                let name = localize(&loc).unwrap_or(loc);
                self.stats.entry(format!("{set}.customclassname.{n}")).or_insert(StatValue::Str(name));
            }
        }
        // A match's preset classes (the class menu's default classes,
        // `GetGametypeSettings().cacLoadouts[team]`), in the menu's order:
        // the statstable's class_smg, class_cqb, class_assault, class_lmg,
        // class_sniper and their names (CLASS_SMG = "Operative", ...).
        let presets = [("class_smg", "CLASS_SMG"), ("class_cqb", "CLASS_CQB"), ("class_assault", "CLASS_ASSAULT"), ("class_lmg", "CLASS_LMG"), ("class_sniper", "CLASS_SNIPER")];
        for team in 0..3 {
            let set = format!("gametypesettings.cacloadouts.{team}");
            for (n, (class, loc)) in presets.iter().enumerate() {
                for (slot, v) in self.default_class(class) {
                    self.stats.entry(format!("{set}.customclass.{n}.{slot}")).or_insert(StatValue::Num(v as f32));
                }
                let name = localize(loc).unwrap_or_else(|| (*loc).to_owned());
                self.stats.entry(format!("{set}.customclassname.{n}")).or_insert(StatValue::Str(name));
            }
        }
        // A new profile's stats were never reset: no "Your online stats
        // have been reset" notice (MainLobby shows it while this is 0).
        self.stats.entry("cacloadouts.resetwarningdisplayed".to_owned()).or_insert(StatValue::Num(1.0));
    }

    /// A default class's slots: (slot, item index or count). `class` is a
    /// statstable class name (`class_custom_smg`, `class_smg`).
    pub fn default_class(&self, class: &str) -> Vec<(String, i32)> {
        let mut out: Vec<(String, i32)> = Vec::new();
        for it in &self.items {
            let Some((_, words)) = it.classes.iter().find(|(c, _)| c == class) else { continue };
            let slot = it.slot.as_str();
            match slot {
                "primary" | "secondary" | "primarygrenade" | "specialgrenade" => {
                    out.push((slot.to_owned(), it.index));
                    if slot.ends_with("grenade") {
                        let count = words.first().and_then(|w| w.parse::<i32>().ok()).unwrap_or(1);
                        out.push((format!("{slot}count"), count));
                    } else {
                        // Its attachments, numbered as the weapon lists them.
                        for (i, w) in words.iter().enumerate().take(3) {
                            if let Some(a) = it.attachments.iter().position(|x| x == w) {
                                out.push((format!("{slot}attachment{}", i + 1), a as i32 + 1));
                            }
                        }
                    }
                }
                // A perk's slot is its specialty number (specialty1-3).
                s if s.starts_with("specialty") => out.push((s.to_owned(), it.index)),
                // The default scorestreaks ("all" classes carry them, in
                // table order): killstreak1..3 whatever the slot is named.
                s if s.starts_with("killstreak") => {
                    let n = out.iter().filter(|(k, _)| k.starts_with("killstreak")).count() + 1;
                    out.push((format!("killstreak{n}"), it.index));
                }
                // Wildcards: no default class carries one.
                _ => {}
            }
        }
        out
    }

    /// The stats path of a class slot in a set.
    pub fn class_key(set: &str, class: i32, slot: &str) -> String {
        format!("{set}.customclass.{class}.{}", slot.to_ascii_lowercase())
    }

    pub fn stat(&self, path: &str) -> Option<&StatValue> {
        self.stats.get(path)
    }

    /// A stat as a number (0 when never written).
    pub fn stat_num(&self, path: &str) -> f32 {
        self.stat(path).and_then(StatValue::as_num).unwrap_or(0.0)
    }

    pub fn set_stat(&mut self, path: String, v: StatValue) {
        self.stats.insert(path, v);
        self.dirty = true;
    }

    /// Whether a stat changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// His rank (0 = level 1).
    pub fn rank(&self) -> i32 {
        self.stat_num(RANK) as i32
    }

    pub fn item(&self, i: i32) -> Option<&Item> {
        self.items.get(usize::try_from(i).ok()?).filter(|it| !it.reference.is_empty())
    }

    pub fn item_by_ref(&self, reference: &str) -> Option<&Item> {
        self.items.iter().find(|it| it.reference == reference)
    }

    /// Locked: above his rank, unless everything is free (BO2's custom
    /// games unlock every item) or a prestige token unlocked it for good.
    pub fn locked(&self, i: i32, all_free: bool) -> bool {
        !all_free && self.item(i).is_some_and(|it| it.unlock_rank > self.rank()) && !self.permanently_unlocked(i)
    }

    /// His unlock tokens (BO2's `unlocks[0]`, the pool its Create-a-Class
    /// spends from).
    pub fn tokens(&self) -> i32 {
        self.stat_num(UNLOCK_TOKENS) as i32
    }

    /// An item's own: free with its rank, or bought with a token
    /// (`itemstats.<i>.purchased`).
    pub fn purchased(&self, i: i32, all_free: bool) -> bool {
        all_free
            || self.item(i).is_some_and(|it| !it.token || self.stat_num(&format!("itemstats.{i}.purchased")) != 0.0)
            || self.permanently_unlocked(i)
    }

    /// Spend `cost` tokens on an item his rank has reached (BO2's
    /// `Engine.PurchaseItem`); false when he can't.
    pub fn purchase(&mut self, i: i32, cost: i32) -> bool {
        if self.locked(i, false) || self.purchased(i, false) || self.tokens() < cost {
            return false;
        }
        let left = self.tokens() - cost;
        self.set_stat(UNLOCK_TOKENS.to_owned(), StatValue::Num(left as f32));
        self.set_stat(format!("itemstats.{i}.purchased"), StatValue::Num(1.0));
        true
    }

    /// A weapon's attachment by its number (0 = none, then the weapon's
    /// list). A list word the table lacks (`reflex_pistol`) is its base
    /// attachment (`reflex`).
    pub fn attachment(&self, item: i32, n: i32) -> Option<&Attachment> {
        let reference = if n <= 0 {
            "none".to_owned()
        } else {
            self.item(item)?.attachments.get(n as usize - 1)?.clone()
        };
        self.attachments
            .iter()
            .find(|a| a.reference == reference)
            .or_else(|| self.attachments.iter().find(|a| reference.starts_with(&format!("{}_", a.reference))))
    }

    /// His stats as text, one `path<TAB>value` per line (text starts with
    /// `s:`), sorted: the local stats file.
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self
            .stats
            .iter()
            .map(|(k, v)| match v {
                StatValue::Num(n) => format!("{k}\t{}", number_text(*n)),
                StatValue::Str(t) => format!("{k}\ts:{t}"),
            })
            .collect();
        lines.sort();
        lines.join("\n") + "\n"
    }

    /// Stats from a file written by `to_text`.
    pub fn stats_from_text(text: &str) -> HashMap<String, StatValue> {
        let mut out = HashMap::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('\t') else { continue };
            let value = match v.strip_prefix("s:") {
                Some(t) => StatValue::Str(t.to_owned()),
                None => v.trim().parse::<f32>().map_or_else(|_| StatValue::Str(v.to_owned()), StatValue::Num),
            };
            out.insert(k.to_owned(), value);
        }
        out
    }

    /// His stats file (none yet = no stats).
    pub fn read_stats_file(path: &Path) -> HashMap<String, StatValue> {
        std::fs::read_to_string(path).map(|t| Profile::stats_from_text(&t)).unwrap_or_default()
    }

    /// Write his stats file (a temporary file, then renamed over it).
    pub fn save_file(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }

    // The server's answers for a player's class (`class` = the class
    // number the scripts pass, 0 the first custom class), in `match_set`.

    /// `getloadoutitem(class, slot)`: the slot's item index (0 = none).
    pub fn loadout_item(&self, class: i32, slot: &str) -> i32 {
        self.stat_num(&Profile::class_key(&self.match_set, class, slot)) as i32
    }

    /// Whether he has picked scorestreaks (Clear All counts: none picked):
    /// they sit in class 0, shared by every class, in `match_set`.
    pub fn has_streak_picks(&self) -> bool {
        (1..=3).any(|n| self.stat(&Profile::class_key(&self.match_set, 0, &format!("killstreak{n}"))).is_some())
    }

    /// `getloadoutitemref(class, slot)`: the slot's item reference ("" for
    /// none).
    pub fn loadout_item_ref(&self, class: i32, slot: &str) -> String {
        self.item(self.loadout_item(class, slot)).map(|it| it.reference.clone()).unwrap_or_default()
    }

    /// `getloadoutweapon(class, "primary" | "secondary")`: the weapon BO2
    /// gives, `<reference>_mp` with its attachments as `+<attachment>`, in
    /// the attachment table's order (alphabetical in his table); no weapon
    /// = `weapon_null_mp`.
    pub fn loadout_weapon(&self, class: i32, slot: &str) -> String {
        let index = self.loadout_item(class, slot);
        let Some(it) = self.item(index).filter(|it| it.reference != "weapon_null") else {
            return "weapon_null_mp".to_owned();
        };
        let mut atts: Vec<&Attachment> = (1..=3)
            .filter_map(|n| {
                let a = self.loadout_item(class, &format!("{slot}attachment{n}"));
                self.attachment(index, a).filter(|_| a > 0)
            })
            .collect();
        atts.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.reference.cmp(&b.reference)));
        atts.dedup_by(|a, b| a.reference == b.reference);
        let mut name = format!("{}_mp", it.reference);
        for a in atts {
            name.push('+');
            name.push_str(&a.reference);
        }
        name
    }

    /// `getloadoutperks(class)`: the specialties of the class's perks
    /// (specialty1-6), each perk's `specialty_*` names in table order.
    pub fn loadout_perks(&self, class: i32) -> Vec<String> {
        let mut out = Vec::new();
        for n in 1..=6 {
            let i = self.loadout_item(class, &format!("specialty{n}"));
            if let Some(it) = self.item(i).filter(|it| it.group == "specialty") {
                out.extend(it.reference.split('|').map(str::trim).filter(|s| !s.is_empty() && *s != "specialty_null").map(str::to_owned));
            }
        }
        out
    }

    /// `isbonuscardactive(card, class)`: a wildcard slot holds that card.
    /// `card` numbers the statstable's wildcards in index order (0 primary
    /// gunfighter, 1 secondary gunfighter, 2 overkill, 3-5 perk greeds,
    /// 6 danger close, 7 two tacticals), as _class.gsc's 0 / 1 / 2 checks
    /// read them.
    pub fn bonus_card_active(&self, card: i32, class: i32) -> bool {
        let Some(want) = self.items.iter().filter(|it| it.group == "bonuscard").nth(usize::try_from(card).unwrap_or(usize::MAX)) else {
            return false;
        };
        (1..=3).any(|n| self.loadout_item(class, &format!("bonuscard{n}")) == want.index)
    }
}

/// His rank (0 = level 1), rank XP and prestige: BO2's
/// `playerstatslist.<RANK|RANKXP|PLEVEL>.statvalue` (CoDBase's
/// `UIExpression.GetStatByName(controller, "PLEVEL")`).
pub const RANK: &str = "playerstatslist.rank.statvalue";
pub const RANK_XP: &str = "playerstatslist.rankxp.statvalue";
pub const PLEVEL: &str = "playerstatslist.plevel.statvalue";

/// Prestige. BO2's menus decide when he may enter it (CoDBase's
/// PrestigeAvail: his prestige under mp/rankIconTable.csv's `maxprestige`,
/// his rank XP at mp/rankTable.csv's last rank's end) and run
/// mp/prestige_reset.cfg, whose stats commands land here
/// (`run_stat_command`). Each prestige gives one prestige token (the
/// Barracks' `Engine.IsPrestigeTokenSpent(controller)` asks after the
/// current prestige's one), spent on a permanent unlock (Create-a-Class's
/// `Engine.PermanentlyUnlockItem(controller, item)`): kept as
/// `prestigetokens.<prestige>.tokenspent` and `.itemunlocked`.
impl Profile {
    /// His prestige (0 = none).
    pub fn plevel(&self) -> i32 {
        self.stat_num(PLEVEL) as i32
    }

    fn token_key(level: i32, field: &str) -> String {
        format!("prestigetokens.{level}.{field}")
    }

    fn token_unspent(&self, level: i32) -> bool {
        self.stat_num(&Profile::token_key(level, "tokenspent")) == 0.0
    }

    /// Whether his current prestige's token is spent (none before his first
    /// prestige).
    pub fn prestige_token_spent(&self) -> bool {
        let p = self.plevel();
        p <= 0 || !self.token_unspent(p)
    }

    /// His prestige tokens not yet spent (one per prestige reached).
    pub fn prestige_tokens(&self) -> i32 {
        (1..=self.plevel()).filter(|l| self.token_unspent(*l)).count() as i32
    }

    /// An item one of his prestige tokens unlocked for good: unlocked and
    /// his whatever his rank, through every later prestige.
    pub fn permanently_unlocked(&self, i: i32) -> bool {
        (1..=self.plevel()).any(|l| {
            self.stat(&Profile::token_key(l, "itemunlocked")).and_then(StatValue::as_num).is_some_and(|n| n as i32 == i)
        })
    }

    /// Spend a prestige token on an item (BO2's `PermanentlyUnlockItem`):
    /// his current prestige's token first, else the latest one unspent.
    /// False when he has none or the item is his for good already.
    pub fn permanently_unlock(&mut self, i: i32) -> bool {
        if self.item(i).is_none() || self.permanently_unlocked(i) {
            return false;
        }
        let Some(level) = (1..=self.plevel()).rev().find(|l| self.token_unspent(*l)) else {
            return false;
        };
        self.set_stat(Profile::token_key(level, "tokenspent"), StatValue::Num(1.0));
        self.set_stat(Profile::token_key(level, "itemunlocked"), StatValue::Num(i as f32));
        true
    }

    /// A class back to a default class (`equipdefaultclass <n> <class>`):
    /// every slot cleared, then the default class's slots.
    pub fn reset_class(&mut self, set: &str, n: i32, class: &str) {
        let prefix = format!("{set}.customclass.{n}.");
        self.stats.retain(|k, _| !k.starts_with(&prefix));
        for (slot, v) in self.default_class(class) {
            self.stats.insert(format!("{prefix}{slot}"), StatValue::Num(v as f32));
        }
        self.dirty = true;
    }

    /// `PrestigeStatsReset` (mp/prestige_reset.cfg: "Mark all of the items,
    /// attachments and options as unpurchased"): every item bought with an
    /// unlock token is unbought. His unspent unlock tokens, weapon levels
    /// and permanent unlocks stay (the file touches none of them).
    pub fn prestige_stats_reset(&mut self) {
        self.stats.retain(|k, _| !(k.starts_with("itemstats.") && k.ends_with(".purchased")));
        self.dirty = true;
    }

    /// `prestigerequest`: the next prestige, up to `max_prestige`
    /// (mp/rankIconTable.csv `maxprestige`). His rank XP is not checked
    /// here: mp/prestige_reset.cfg zeroes it on the line before, and the
    /// menus offer Prestige only at the last rank's end.
    pub fn prestige_request(&mut self, max_prestige: i32) -> bool {
        let p = self.plevel();
        if p >= max_prestige {
            return false;
        }
        self.set_stat(PLEVEL.to_owned(), StatValue::Num((p + 1) as f32));
        true
    }

    /// Carry out one of BO2's stats commands (`hks_t6::host::STAT_COMMANDS`,
    /// as mp/prestige_reset.cfg and mp/reset_classes*.cfg write them):
    /// `equipdefaultclass <n> <class>` (the public set),
    /// `equipdefaultclasstoprofile <n> <class>` (the offline profile's),
    /// `setStatFromLocString <set> customclassname <n> <key>`,
    /// `setprofilelocclass <n> <key>`, `statwriteddl <path words> <number>`,
    /// `PrestigeStatsReset`, `prestigerequest`. False for one not carried
    /// out (a value BO2 computes, `( dvarint( ... ) )`; the Prestige
    /// Awards' commands).
    pub fn run_stat_command(&mut self, line: &str, max_prestige: i32, localize: &dyn Fn(&str) -> Option<String>) -> bool {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(verb) = words.first().map(|w| w.to_ascii_lowercase()) else { return false };
        let n = |i: usize| words.get(i).and_then(|w| w.parse::<i32>().ok());
        let text = |key: &str| localize(&key.to_ascii_uppercase()).unwrap_or_else(|| key.to_owned());
        match verb.as_str() {
            "equipdefaultclass" | "equipdefaultclasstoprofile" => {
                let (Some(class), Some(name)) = (n(1), words.get(2)) else { return false };
                let set = if verb == "equipdefaultclass" { "cacloadouts" } else { "profile.cacloadouts" };
                self.reset_class(set, class, &name.to_ascii_lowercase());
                true
            }
            "setstatfromlocstring" => {
                let (Some(set), Some(field), Some(i), Some(key)) = (words.get(1), words.get(2), n(3), words.get(4)) else {
                    return false;
                };
                let path = format!("{}.{}.{i}", set.to_ascii_lowercase(), field.to_ascii_lowercase());
                self.set_stat(path, StatValue::Str(text(key)));
                true
            }
            "setprofilelocclass" => {
                let (Some(i), Some(key)) = (n(1), words.get(2)) else { return false };
                self.set_stat(format!("profile.cacloadouts.customclassname.{i}"), StatValue::Str(text(key)));
                true
            }
            "statwriteddl" => {
                let Some((value, path)) = words[1..].split_last() else { return false };
                let Ok(value) = value.parse::<f32>() else { return false };
                if path.is_empty() || path.iter().any(|w| w.starts_with('(')) {
                    return false;
                }
                self.set_stat(path.join(".").to_ascii_lowercase(), StatValue::Num(value));
                true
            }
            "prestigestatsreset" => {
                self.prestige_stats_reset();
                true
            }
            "prestigerequest" => self.prestige_request(max_prestige),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[(usize, &str)]) -> Vec<String> {
        let mut r = vec![String::new(); 18];
        for (i, c) in cells {
            r[*i] = (*c).to_owned();
        }
        r
    }

    fn sample() -> Profile {
        let stats = vec![
            row(&[(0, "31"), (2, "weapon_assault"), (4, "hk416"), (8, "reflex acog dualclip"), (11, "class_custom_assault"), (12, "1"), (13, "primary")]),
            row(&[(0, "53"), (2, "weapon_launcher"), (4, "smaw"), (11, "class_custom_assault"), (12, "1"), (13, "secondary")]),
            row(&[(0, "71"), (2, "weapon_grenade"), (4, "concussion_grenade"), (11, "class_custom_assault 2"), (12, "1"), (13, "specialgrenade")]),
            row(&[(0, "153"), (2, "specialty"), (4, "specialty_immunecounteruav|specialty_immuneemp"), (11, "class_custom_assault"), (12, "1"), (13, "specialty2")]),
            row(&[(0, "178"), (2, "bonuscard"), (4, "bonuscard_primary_gunfighter"), (13, "bonuscard1")]),
            row(&[(0, "179"), (2, "bonuscard"), (4, "bonuscard_secondary_gunfighter"), (13, "bonuscard2")]),
            row(&[(0, "180"), (2, "bonuscard"), (4, "bonuscard_overkill"), (13, "bonuscard3")]),
        ];
        let atts = vec![
            row(&[(0, "0"), (4, "none")]),
            row(&[(0, "1"), (4, "acog")]),
            row(&[(0, "2"), (4, "dualclip")]),
            row(&[(0, "21"), (4, "reflex")]),
        ];
        Profile::from_tables(&stats, &atts, &|k| (k == "CLASS_SLOT1").then(|| "Custom 1".to_owned()), HashMap::new())
    }

    #[test]
    fn default_classes_fill_every_set() {
        let p = sample();
        assert_eq!(p.loadout_item(0, "primary"), 31);
        assert_eq!(p.loadout_item(0, "specialgrenadecount"), 2);
        assert_eq!(p.stat("profile.cacloadouts.customclassname.0"), Some(&StatValue::Str("Custom 1".into())));
        assert_eq!(p.loadout_weapon(0, "primary"), "hk416_mp");
        assert_eq!(p.loadout_perks(0), vec!["specialty_immunecounteruav", "specialty_immuneemp"]);
    }

    #[test]
    fn weapon_names_take_attachments_in_table_order() {
        let mut p = sample();
        // reflex (list 1), acog (list 2): acog first in the table.
        p.set_stat(Profile::class_key(LOCAL_MATCH_SET, 0, "primaryattachment1"), StatValue::Num(1.0));
        p.set_stat(Profile::class_key(LOCAL_MATCH_SET, 0, "primaryattachment2"), StatValue::Num(2.0));
        assert_eq!(p.loadout_weapon(0, "primary"), "hk416_mp+acog+reflex");
        assert_eq!(p.loadout_item_ref(0, "secondary"), "smaw");
    }

    #[test]
    fn items_unlock_at_their_rank_and_private_matches_free_them() {
        let stats = vec![
            row(&[(0, "10"), (2, "weapon_assault"), (4, "mtar"), (10, "0")]),
            row(&[(0, "11"), (2, "weapon_assault"), (4, "type25"), (10, "4")]),
            row(&[(0, "12"), (2, "weapon_assault"), (4, "xpr"), (10, "30")]),
        ];
        let mut p = Profile::from_tables(&stats, &[], &|_| None, HashMap::new());
        // A new profile (rank 0, shown as level 1): only rank-0 items.
        assert!(!p.locked(10, false) && p.locked(11, false) && p.locked(12, false));
        // Private matches free everything.
        assert!(!p.locked(12, true));
        // Rank 4 reached (rankxp is read through the rank table; set plevel/xp
        // directly would need that table, so only the free path is asserted).
        p.set_stat("playerstatslist.rankxp.statvalue".to_owned(), StatValue::Num(0.0));
        assert!(p.locked(11, false));
    }

    /// mp/prestige_reset.cfg's stats lines, as BO2 ships them (with
    /// mp/reset_classes.cfg's first class and its name).
    #[test]
    fn prestige_reset_cfg_resets_rank_purchases_and_classes() {
        let stats = vec![
            row(&[(0, "31"), (2, "weapon_assault"), (4, "hk416"), (8, "reflex acog"), (10, "0"), (11, "class_custom_assault"), (13, "primary")]),
            row(&[(0, "12"), (2, "weapon_assault"), (4, "xpr"), (10, "30"), (13, "primary"), (17, "token")]),
        ];
        let loc = |k: &str| (k == "CLASS_SLOT1").then(|| "Custom 1".to_owned());
        let mut p = Profile::from_tables(&stats, &[], &loc, HashMap::new());
        for (k, v) in [(RANK, 54.0), (RANK_XP, 1249100.0), (UNLOCK_TOKENS, 3.0), ("itemstats.12.purchased", 1.0)] {
            p.set_stat(k.to_owned(), StatValue::Num(v));
        }
        p.set_stat(Profile::class_key("cacloadouts", 0, "primary"), StatValue::Num(12.0));
        p.set_stat(Profile::class_key("cacloadouts", 0, "primaryattachment2"), StatValue::Num(2.0));
        p.set_stat("cacloadouts.customclassname.0".to_owned(), StatValue::Str("Mine".into()));
        assert!(p.purchased(12, false) && !p.locked(12, false));
        for line in [
            "equipdefaultclass 0 class_custom_assault",
            "setStatFromLocString cacloadouts customclassname 0 CLASS_SLOT1",
            "PrestigeStatsReset",
            "statwriteddl playerstatslist rankxp statvalue 0",
            "statwriteddl playerstatslist rank statvalue 0",
            "prestigerequest",
        ] {
            assert!(p.run_stat_command(line, 11, &loc), "{line}");
        }
        assert!(!p.run_stat_command("statwriteddl cacloadouts loadoutVersion ( dvarint( classVersionNumber ) )", 11, &loc));
        assert_eq!((p.rank(), p.stat_num(RANK_XP), p.plevel()), (0, 0.0, 1));
        assert!(p.locked(12, false) && !p.purchased(12, false));
        assert_eq!(p.tokens(), 3, "unspent unlock tokens stay");
        assert_eq!(p.stat_num(&Profile::class_key("cacloadouts", 0, "primary")), 31.0);
        assert_eq!(p.stat(&Profile::class_key("cacloadouts", 0, "primaryattachment2")), None);
        assert_eq!(p.stat("cacloadouts.customclassname.0"), Some(&StatValue::Str("Custom 1".into())));
        // One prestige token, spent on the XPR: unlocked and his for good.
        assert_eq!((p.prestige_tokens(), p.prestige_token_spent()), (1, false));
        assert!(p.permanently_unlock(12));
        assert!(!p.locked(12, false) && p.purchased(12, false));
        assert_eq!((p.prestige_tokens(), p.prestige_token_spent()), (0, true));
        assert!(!p.permanently_unlock(31));
        // The next prestige keeps it; Prestige Master is the last.
        p.set_stat(RANK.to_owned(), StatValue::Num(54.0));
        assert!(p.run_stat_command("PrestigeStatsReset", 11, &loc) && p.run_stat_command("prestigerequest", 11, &loc));
        assert!(p.permanently_unlocked(12) && p.prestige_tokens() == 1);
        p.set_stat(PLEVEL.to_owned(), StatValue::Num(11.0));
        assert!(!p.run_stat_command("prestigerequest", 11, &loc));
    }

    #[test]
    fn bonus_cards_by_number_and_stats_round_trip() {
        let mut p = sample();
        p.set_stat(Profile::class_key(LOCAL_MATCH_SET, 1, "bonuscard2"), StatValue::Num(180.0));
        assert!(p.bonus_card_active(2, 1));
        assert!(!p.bonus_card_active(0, 1));
        let back = Profile::stats_from_text(&p.to_text());
        assert_eq!(back, p.stats);
    }
}
