//! bo2mp lane C: the engine side of Black Ops II's multiplayer front end
//! (main menu, lobbies, Create-a-Class), beside `host`'s zombies answers.
//! `install` runs after `Host::new` and before `load_base`: CoDBase reads
//! the session mode (`CoD.isZombie`, `CoD.isMultiplayer`) while it loads.
//!
//! The items, classes and his stats are `bo2_profile` (shared with the
//! server): one store of values by stats path, shaped like BO2's stats
//! layout (ddl_mp/stats.ddl), the items and default classes from his
//! `mp/statstable.csv` and `mp/attachmenttable.csv`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use bo2_profile::{Attachment, Item, Profile, SLOTS, StatValue};

use crate::host::{EngineValues, Host};
use crate::value::{Table, Value};
use crate::vm::Vm;

/// The engine's own dvars the front end reads (`Dvar.<name>:get()`), with
/// the values an offline PC game has; set where the host has none. Mostly
/// online-service switches (streaming, matchmaking search, probation),
/// all off.
const ENGINE_DVARS: &[(&str, &str)] = &[
    // The kill feed's name colours: yours and the other side's.
    // Measured from the original game's kill feed: your side is drawn
    // blue-grey and the other side tan, so BO2's MyTeam/EnemyTeam dvars are
    // taken to default to these two colours.
    ("g_TeamColor_MyTeam", "0.60000002 0.63999999 0.69"),
    ("g_TeamColor_EnemyTeam", "0.64999998 0.56999999 0.41"),
    ("loc_language", "0"),
    ("hiDef", "1"),
    ("widescreen", "1"),
    ("wideScreen", "1"),
    ("developer", "0"),
    ("cl_paused", "0"),
    ("gpad_enabled", "0"),
    ("r_dualPlayEnable", "0"),
    ("r_stereo3DOn", "0"),
    ("r_txaaSupported", "0"),
    ("r_aaSamplesMax", "8"),
    ("vid_xpos", "0"),
    ("vid_ypos", "0"),
    ("ui_errorMessage", ""),
    ("ui_errorTitle", ""),
    ("ui_friendsListOpen", "0"),
    ("ui_playerListOpen", "0"),
    ("ui_xboxLivePartyListOpen", "0"),
    ("ui_totalDLCReleased", "4"),
    ("ui_inGameStoreVisible", "1"),
    ("ui_storeButtonPressed", "0"),
    ("ui_hideLeaderboards", "0"),
    ("ui_scrollByRow", "0"),
    ("ui_changed_exe", "0"),
    ("ui_keyboardtitle", ""),
    ("ui_keyboard_dvar_edit", ""),
    ("selectedPlayerXuid", "0"),
    ("g_customTeamName_Allies", ""),
    ("g_customTeamName_Axis", ""),
    ("g_TeamName_Allies", ""),
    ("g_TeamName_Axis", ""),
    ("g_TeamName_Three", ""),
    ("party_maxplayers", "18"),
    // One local player on PC (the lobbies check the controller count).
    ("party_maxlocalplayers", "1"),
    ("party_maxlocalplayers_mainlobby", "1"),
    ("party_maxlocalplayers_playermatch", "1"),
    ("party_maxlocalplayers_privatematch", "1"),
    ("party_maxlocalplayers_leaguematch", "1"),
    ("party_maxlocalplayers_systemlink", "1"),
    ("party_maxlocalplayers_local_splitscreen", "1"),
    ("party_maxplayers_playermatch", "18"),
    ("party_maxplayers_privatematch", "18"),
    ("party_maxplayers_leaguematch", "18"),
    ("party_maxplayers_partylobby", "18"),
    ("party_hostname", ""),
    ("party_maxplayers_systemlink", "18"),
    ("party_maxplayers_local_splitscreen", "4"),
    ("party_maxplayers_theater", "18"),
    ("party_maxlocalplayers_theater", "1"),
    ("party_isPreviousMapVoted", "0"),
    ("demo_recordPrivateMatch", "0"),
    ("tu7_usePCmatchmaking", "1"),
    ("tu7_usePingSlider", "0"),
    // The PC scoreboard's Ping column is numbers (his real game).
    ("tu7_scoreboardPingAsNumbers", "1"),
    // Class sets bought: his real game shows all ten pips under CLASS SET 1.
    ("tu8_purchasedClassSetCount", "10"),
    ("tu10_searchSessionIgnoreMapPacks", "0"),
    ("tu12_searchSessionOverrideGeoLocation", "0"),
    ("tu13_recordContentAvailable", "0"),
    ("excellentPing", "30"),
    ("goodPing", "100"),
    ("terriblePing", "200"),
    ("live_minMatchMakingPing", "0"),
    ("live_maxMatchMakingPing", "200"),
    ("probation_public_probationTime", "0"),
    ("probation_league_probationTime", "0"),
    ("fbEnabled", "0"),
    ("fshMtxName", ""),
    ("fshCustomGameName", ""),
    ("webm_httpAuthMode", "0"),
    ("webm_encStreamEnabled", "0"),
    ("webm_encStatus", "0"),
    ("webm_httpAuthToken", ""),
    ("webm_httpAuthLogin", ""),
    ("webm_httpAuthPass", ""),
    ("webm_twitchLasterror", ""),
    ("YouTube_minViewersToStartStream", "0"),
    ("sd_xa2_num_devices", "1"),
];

/// The multiplayer front end's state: his profile (the items, classes and
/// stats, `bo2_profile`), the session.
#[derive(Default)]
pub struct MpState {
    /// The items and his stats, made from the string tables on first use.
    pub profile: Profile,
    /// His stats file's values until the tables arrive (`load_text`).
    saved: Option<HashMap<String, StatValue>>,
    loaded: bool,
    /// An online session (CoD.SESSIONMODE_ONLINE), as of the last engine
    /// call that read the items.
    online: bool,
    /// The game modes set (`CoD.GAMEMODE_*`: 1 = private match).
    pub game_modes: HashSet<i32>,
    /// START MATCH was pressed (`Engine.PartyHostToggleStart`) and the
    /// owner has not started it yet.
    pub start_requested: bool,
    /// YES on the main menu's ZOMBIES prompt (`startZombies`): our
    /// Nuketown Zombies starts.
    pub zombies_requested: bool,
    /// Game timers counting down (`setTimeLeft`): the element and the LUI
    /// time it reaches 0.
    timers: Vec<(Rc<crate::value::UserData>, f64)>,
    /// A match's scoreboard rows (the scoreboard index = the place here)
    /// and each team's score (`CoD.TEAM_*` number -> score).
    pub board: Vec<BoardRow>,
    pub team_scores: Vec<(i32, f32)>,
    /// The engine's message windows (`setupGameMessages(index)`: 0 the
    /// obituaries, the kill feed): the element and its index.
    pub windows: Vec<(Value, i32)>,
    /// The match clock's text elements (`setupGameTimer`) and the LUI time
    /// the match ends at (none = no clock).
    game_timers: Vec<Rc<crate::value::UserData>>,
    pub game_end_ms: Option<f64>,
    /// The match is over (the intermission): the scoreboard marks every row
    /// with the dead icon, as BO2's does.
    pub game_ended: bool,
    /// The game type's settings (`Engine.GetGametypeSetting`), lower case.
    pub settings: HashMap<String, f32>,
    /// Each weapon's kill icon for the kill feed: weapon (no `_mp`) ->
    /// (its picture's name, width per height); and whether they are in.
    pub kill_icons: HashMap<String, (String, f32)>,
    pub kill_icons_loaded: bool,
    /// The player the killcam (or the Killed By card) shows: the killer
    /// (`Engine.GetPredictedClientNum`).
    pub callout: Option<i32>,
    /// The Combat Training playlist picked (`Engine.SetPlaylistID`,
    /// `crate::playlists`) and, once FIND MATCH opened the public lobby,
    /// the LUI time its countdown ends.
    pub playlist: Option<i32>,
    pub public_start_ms: Option<f64>,
    /// START MATCH in a Custom Game: the LUI time its "Game starting in"
    /// countdown ends.
    pub private_start_ms: Option<f64>,
    /// A countdown to start on the front end's first frame (ms).
    pub public_start_in: Option<f64>,
    /// His stats before his last ranked match (`set_stable_text`).
    stable: Option<HashMap<String, StatValue>>,
    /// The emblem the Emblem Editor is editing (the engine's `emblem*`
    /// commands change it, `GetSelectedLayer*` / `GetUsedLayerCount` read it).
    pub emblem: EmblemEdit,
}

/// One emblem layer: its icon (-1 = empty, `EMBLEMS_INVALID_ICON_ID`) and
/// its colour (red, green, blue, alpha, 0 to 1; none = the editor's default).
#[derive(Clone, Copy, Debug)]
pub struct EmblemLayer {
    pub icon: i32,
    pub rgba: Option<[f32; 4]>,
}

impl Default for EmblemLayer {
    fn default() -> Self {
        EmblemLayer { icon: -1, rgba: None }
    }
}

/// The emblem being edited: its layers (a new emblem has none; they are
/// added as `emblemSelect n` reaches them) and the layer selected. The
/// layer limit (32) is the script's (`CoD.EmblemEditor.TotalLayers`).
#[derive(Clone, Debug, Default)]
pub struct EmblemEdit {
    pub layers: Vec<EmblemLayer>,
    pub selected: usize,
    /// `emblemSetScaleMode n` (CoD.EmblemEditor.EMBLEM_FIXED_SCALE 0 /
    /// EMBLEM_FREE_SCALE 1): fixed, as a fresh editor.
    pub scale_mode: i32,
    /// The icons he has looked at (`Engine.SetEmblemIconAsOld`): the ones
    /// not here and unlocked are NEW.
    pub old: HashSet<i32>,
}

impl EmblemEdit {
    /// `emblemClearAll`: every layer empty again, the first selected.
    pub fn clear_all(&mut self) {
        self.layers.clear();
        self.selected = 0;
    }
    /// `emblemSelect n`.
    pub fn select(&mut self, n: usize) {
        self.selected = n;
    }
    /// The selected layer, made when the emblem has not reached it yet.
    pub fn current(&mut self) -> &mut EmblemLayer {
        if self.layers.len() <= self.selected {
            self.layers.resize(self.selected + 1, EmblemLayer::default());
        }
        &mut self.layers[self.selected]
    }
    /// `emblemIcon id` on the selected layer.
    pub fn set_icon(&mut self, id: i32) {
        self.current().icon = id;
    }
    /// `emblemPalette r g b a` on the selected layer.
    pub fn set_color(&mut self, rgba: [f32; 4]) {
        self.current().rgba = Some(rgba);
    }
    /// A layer's icon (-1 for one the emblem has not reached).
    pub fn icon(&self, layer: usize) -> i32 {
        self.layers.get(layer).map_or(-1, |l| l.icon)
    }
    /// `emblemSetScaleMode n`.
    pub fn set_scale_mode(&mut self, n: i32) {
        self.scale_mode = n;
    }
    /// The layers with an icon.
    pub fn used(&self) -> usize {
        self.layers.iter().filter(|l| l.icon >= 0).count()
    }
}

/// The icons of one selector filter, in file order. A filter id is the place
/// of a `category` row in mp/emblemCategoriesOrLayers.csv; the icons are the
/// `icon` rows of mp/emblemsOrBackings.csv (id column 2, name key 4, rank
/// level 6, challenge flag 8, category 9) with that category.
fn emblem_filter_icons<'a>(v: &'a EngineValues, filter: i32) -> Vec<&'a Vec<String>> {
    let Some(name) = usize::try_from(filter).ok().and_then(|n| {
        table_rows(v, "mp/emblemcategoriesorlayers.csv").iter().filter(|r| cell(r, 0) == "category").nth(n).map(|r| cell(r, 1))
    }) else {
        return Vec::new();
    };
    table_rows(v, "mp/emblemsorbackings.csv").iter().filter(|r| cell(r, 0) == "icon" && cell(r, 9) == name).collect()
}

/// An emblem icon or backing's challenge (kind 0 = icon, 1 = backing, as
/// CoD.EMBLEM / CoD.BACKING): the mp/statsMilestones1-4.csv row (column 12)
/// that names it, as (row, table number from 0, row). A milestone naming a
/// prefix ends in `_` (one per tier or weapon).
fn emblem_challenge(v: &EngineValues, kind: i32, id: i32) -> Option<(usize, usize, Vec<String>)> {
    let want = if kind == 0 { "icon" } else { "background" };
    let key = table_rows(v, "mp/emblemsorbackings.csv")
        .iter()
        .find(|r| cell(r, 0) == want && int(r, 2) == id)
        .map(|r| cell(r, 4).to_ascii_lowercase())
        .filter(|k| !k.is_empty())?;
    let find = |exact: bool| {
        (0..4).find_map(|t| {
            let rows = table(v, &format!("mp/statsmilestones{}.csv", t + 1))?;
            rows.iter().enumerate().find_map(|(i, r)| {
                let n = cell(r, 12).to_ascii_lowercase();
                let hit = !n.is_empty() && if exact { n == key } else { n.ends_with('_') && key.starts_with(&n) };
                hit.then(|| (i, t, r.clone()))
            })
        })
    };
    find(true).or_else(|| find(false))
}

/// Whether he has not earned an icon: its rank level (column 6) is above his
/// rank, or its challenge (column 8 = 1) is not done. A private match
/// unlocks everything.
fn emblem_icon_locked(m: &MpState, v: &EngineValues, id: i32) -> bool {
    if m.all_free() {
        return false;
    }
    let Some(row) = table_rows(v, "mp/emblemsorbackings.csv").iter().find(|r| cell(r, 0) == "icon" && int(r, 2) == id) else {
        return true;
    };
    if int(row, 6) > m.profile.rank() {
        return true;
    }
    cell(row, 8) == "1" && emblem_challenge(v, 0, id).map_or(true, |(_, _, c)| emblem_milestone_locked(m, &c))
}

/// A milestone he has not finished: his count on its stat (column 4,
/// playerstatslist.<stat>) is under its target (column 2); a digital unlock
/// (a purchase or pre-order, no count) is not his.
fn emblem_milestone_locked(m: &MpState, c: &[String]) -> bool {
    let stat = cell(c, 4);
    if stat == "digital_unlock" {
        return true;
    }
    m.stat(&format!("playerstatslist.{stat}.statvalue")).as_num().unwrap_or(0.0) < int(c, 2) as f32
}

/// A fresh layer's colour: the white swatch of the game's own colour table
/// (`mp/emblemswatchcolors.csv`, cells `0xRRGGBBAA`), the brightest cell.
fn emblem_default_color(v: &EngineValues) -> [f32; 4] {
    let cells = table(v, "mp/emblemswatchcolors.csv").into_iter().flatten().flatten();
    let best = cells
        .filter_map(|c| u32::from_str_radix(c.trim().trim_start_matches("0x"), 16).ok())
        .max_by_key(|c| (c >> 24) + ((c >> 16) & 255) + ((c >> 8) & 255));
    let c = best.unwrap_or(0xFFFF_FFFF);
    [(c >> 24) as f32 / 255.0, ((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

/// One scoreboard row: his client number, team (`CoD.TEAM_*`), name and
/// the game type's columns' values.
#[derive(Clone, Debug, Default)]
pub struct BoardRow {
    pub client: i32,
    pub team: i32,
    pub name: String,
    pub cols: Vec<f32>,
}

pub type Mp = Rc<RefCell<MpState>>;

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Nil)
}

fn num(a: &[Value], i: usize) -> i32 {
    arg(a, i).as_num().map_or(0, |n| n as i32)
}

/// A stats path part: a name in lower case, a number as an integer.
fn part(v: &Value) -> String {
    match v {
        Value::Num(n) => format!("{}", *n as i64),
        other => other.to_string().to_ascii_lowercase(),
    }
}

/// A string table by name, case-blind as the engine's.
fn table<'a>(v: &'a EngineValues, name: &str) -> Option<&'a Vec<Vec<String>>> {
    v.tables.get(name).or_else(|| v.tables.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, r)| r))
}

/// A string table's rows (none when it is not loaded).
pub fn table_rows<'a>(v: &'a EngineValues, name: &str) -> &'a [Vec<String>] {
    table(v, name).map_or(&[], Vec::as_slice)
}

fn cell(row: &[String], i: usize) -> String {
    row.get(i).map_or_else(String::new, |c| c.trim().to_owned())
}

fn int(row: &[String], i: usize) -> i32 {
    cell(row, i).parse::<i32>().unwrap_or(0)
}

fn stat_value(v: &Value) -> StatValue {
    match v {
        Value::Num(n) => StatValue::Num(*n),
        Value::Bool(b) => StatValue::Num(if *b { 1.0 } else { 0.0 }),
        other => StatValue::Str(other.to_string()),
    }
}

impl MpState {
    /// Make the profile from the string tables once (they arrive after
    /// `install`), his saved stats first.
    fn load(&mut self, values: &EngineValues) {
        if self.loaded {
            return;
        }
        let Some(stats) = table(values, "mp/statstable.csv") else {
            return;
        };
        self.loaded = true;
        let atts = table(values, "mp/attachmenttable.csv").cloned().unwrap_or_default();
        let localize = |k: &str| values.localize.get(k).cloned();
        self.profile = Profile::from_tables(stats, &atts, &localize, self.saved.take().unwrap_or_default());
        self.online = values.session_modes.contains(&2.0);
    }

    /// His stats file's text (`Profile::to_text`), read before the menus
    /// open: the defaults fill only what it lacks.
    pub fn load_text(&mut self, text: &str) {
        let stats = Profile::stats_from_text(text);
        if self.loaded {
            self.profile.stats.extend(stats);
        } else {
            self.saved = Some(stats);
        }
    }

    pub fn to_text(&self) -> String {
        self.profile.to_text()
    }

    /// Whether a stat changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        self.profile.take_dirty()
    }

    fn set_dirty(&mut self) {
        self.profile.dirty = true;
    }

    /// The class set the menus edit (`bo2_profile::class_set`).
    fn class_set(&self) -> &'static str {
        bo2_profile::class_set(self.online, self.game_modes.contains(&1), self.game_modes.contains(&6))
    }

    fn class_key(&self, class: i32, slot: &str) -> String {
        Profile::class_key(self.class_set(), class, slot)
    }

    /// A stat as it was when his last ranked match started (else now).
    pub fn stable_stat(&self, path: &str) -> Value {
        match self.stable.as_ref() {
            Some(s) => match s.get(path) {
                Some(StatValue::Num(n)) => Value::Num(*n),
                Some(StatValue::Str(t)) => Value::str(t),
                None => Value::Num(0.0),
            },
            None => self.stat(path),
        }
    }

    /// The items his ranks since the last ranked match started unlocked
    /// (unlock rank above his rank then, up to his rank now): index,
    /// picture, name and description keys, whether a feature
    /// (Create-a-Class, scorestreaks...).
    pub fn recent_unlocks(&self) -> Vec<(i32, String, String, String, bool)> {
        let Some(before) = self.stable.as_ref() else { return Vec::new() };
        let then = before.get("playerstatslist.rank.statvalue").and_then(StatValue::as_num).unwrap_or(0.0) as i32;
        let now = self.profile.rank();
        self.items()
            .iter()
            .filter(|it| !it.reference.is_empty() && it.unlock_rank > then && it.unlock_rank <= now)
            .map(|it| (it.index, it.image.clone(), it.name.clone(), it.desc.clone(), it.group == "feature"))
            .collect()
    }

    /// His stats file's text as it was before a ranked match (the front
    /// end keeps it over the match), for the After Action Report.
    pub fn set_stable_text(&mut self, text: &str) {
        self.stable = Some(Profile::stats_from_text(text));
    }

    pub fn stat(&self, path: &str) -> Value {
        match self.profile.stat(path) {
            Some(StatValue::Num(n)) => Value::Num(*n),
            Some(StatValue::Str(s)) => Value::str(s),
            None => Value::Num(0.0),
        }
    }

    fn set_stat(&mut self, path: String, v: &Value) {
        self.profile.set_stat(path, stat_value(v));
    }

    /// Everything is free in a private match (BO2's custom games unlock
    /// every item); else an item unlocks at its rank.
    fn all_free(&self) -> bool {
        self.game_modes.contains(&1)
    }

    fn items(&self) -> &[Item] {
        &self.profile.items
    }

    fn item(&self, i: i32) -> Option<&Item> {
        self.profile.item(i)
    }

    fn locked(&self, i: i32) -> bool {
        self.profile.locked(i, self.all_free())
    }

    fn attachment(&self, item: i32, n: i32) -> Option<&Attachment> {
        self.profile.attachment(item, n)
    }

    fn default_class(&self, class: &str) -> Vec<(String, i32)> {
        self.profile.default_class(class)
    }
}

/// A stats node: `.name` / `[i]` go down a level, `:get()` / `:set(v)`
/// read and write the value at its path.
fn node(vm: &mut Vm, mp: &Mp, path: String) -> Value {
    node_at(vm, mp, path, false)
}

/// A stats node over his stats now, or (`stable`) as they were when his
/// last ranked match started (BO2's STATS_LOCATION_STABLE: the After
/// Action Report's "before").
fn node_at(vm: &mut Vm, mp: &Mp, path: String, stable: bool) -> Value {
    let t = Table::new_ref();
    let meta = Table::new_ref();
    let m = mp.clone();
    let index = vm.native("stats_index", move |vm, a| {
        let key = arg(&a, 1);
        let k = part(&key);
        let v = match k.as_str() {
            "get" if stable => {
                let (m, p) = (m.clone(), path.clone());
                vm.native("stats_get", move |_, _| Ok(vec![m.borrow().stable_stat(&p)]))
            }
            "get" => {
                let (m, p) = (m.clone(), path.clone());
                vm.native("stats_get", move |_, _| Ok(vec![m.borrow().stat(&p)]))
            }
            "set" => {
                let (m, p) = (m.clone(), path.clone());
                vm.native("stats_set", move |_, a| {
                    m.borrow_mut().set_stat(p.clone(), &arg(&a, 1));
                    Ok(vec![])
                })
            }
            _ => node_at(vm, &m, if path.is_empty() { k } else { format!("{path}.{k}") }, stable),
        };
        if let Value::Table(t) = arg(&a, 0) {
            t.borrow_mut().set(key, v.clone());
        }
        Ok(vec![v])
    });
    meta.borrow_mut().set_str("__index", index);
    t.borrow_mut().meta = Some(meta);
    Value::Table(t)
}

/// After `load_base`: BO2's engine puts the element natives behind
/// `LUI.UIElement` itself (the list classes call
/// `LUI.UIElement.removeElement(self, child)`).
/// Each frame: the game timers' text (`m:ss`, BO2's countdown) and the
/// match clock's.
pub fn update_timers(mp: &Mp) {
    let now = crate::lui::now_ms();
    let m = mp.borrow();
    let clock = m.game_end_ms.map(|end| {
        let left = ((end - now).max(0.0) / 1000.0).ceil() as i64;
        format!("{}:{:02}", left / 60, left % 60)
    });
    for u in &m.game_timers {
        if let Some(e) = u.data.borrow_mut().downcast_mut::<crate::lui::Element>() {
            e.state.text = clock.clone();
        }
    }
    for (u, end) in &m.timers {
        let left = ((end - now).max(0.0) / 1000.0).ceil() as i64;
        let text = format!("{}:{:02}", left / 60, left % 60);
        if let Some(e) = u.data.borrow_mut().downcast_mut::<crate::lui::Element>() {
            e.state.text = Some(text);
        }
    }
}

pub fn after_base(host: &mut Host) {
    // BO2's settings as a fresh profile has them (its dvar defaults): the
    // Settings tabs read these, and a name nobody answers reads as "off".
    {
        let mut v = host.values.borrow_mut();
        for (k, x) in [
            ("ragdoll_enable", "1"),
            ("cg_chatHeight", "5"),
            ("cl_voice", "1"),
            ("snd_menu_voice", "1.00"),
            ("snd_menu_music", "1.00"),
            ("snd_menu_sfx", "1.00"),
            ("snd_menu_cinematic", "1.00"),
            ("snd_shoutcast_game", "0.25"),
            ("snd_shoutcast_voip", "1.00"),
            ("snd_voicechat_volume", "1.00"),
            ("snd_voicechat_record_level", "1.00"),
        ] {
            v.dvars.entry(k.to_owned()).or_insert_with(|| x.to_owned());
        }
    }
    // A screen the next one covers is not drawn.
    crate::lui::set_hide_occluded(true);
    if let Some(t) = host.field("LUI", "UIElement").table()
        && t.borrow().meta.is_none()
    {
        let meta = Table::new_ref();
        meta.borrow_mut().set_str("__index", Value::Table(crate::lui::natives()));
        t.borrow_mut().meta = Some(meta);
    }
}

/// Bind the multiplayer front end's engine functions; the state they
/// share comes back (the game saves `stats`).
#[allow(clippy::too_many_lines)]
/// A game type's settings as BO2 sets them with the game type:
/// mp/gamesettings_default.cfg, then mp/gamesettings_<type>.cfg
/// (`gametype_setting <name> <value>` lines).
pub fn set_gametype_defaults(v: &RefCell<EngineValues>, gametype: &str) {
    crate::host::run_command(v, &format!("exec mp/gamesettings_default.cfg; exec mp/gamesettings_{gametype}.cfg"));
}

/// His row in the lobby's player lists (CoD.PlayerListRow reads these):
/// his name (BO2's `name` dvar), his level and its icon
/// (mp/rankIconTable.csv: the row of his rank, the column of his prestige),
/// the party's host, ready; in a private match on the allies (a team game
/// lists its teams), else on no team.
fn lobby_member(m: &MpState, v: &EngineValues, (allies, free): (f32, f32)) -> crate::value::TableRef {
    let rank = m.stat("playerstatslist.rank.statvalue").as_num().unwrap_or(0.0).max(0.0) as usize;
    let prestige = m.stat("playerstatslist.plevel.statvalue").as_num().unwrap_or(0.0).max(0.0) as usize;
    let icon = table_rows(v, "mp/rankicontable.csv")
        .iter()
        .find(|r| r.first().is_some_and(|c| c.trim() == rank.to_string()))
        .and_then(|r| r.get(prestige + 1))
        .cloned()
        .unwrap_or_default();
    let name = v.dvars.get("name").filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| "Player".to_owned());
    let private = m.game_modes.contains(&1);
    let e = Table::new_ref();
    {
        let mut t = e.borrow_mut();
        t.set_str("xuid", Value::str("1"));
        t.set_str("gamertag", Value::str(&name));
        t.set_str("clean_gamertag", Value::str(&name));
        t.set_str("clantag", Value::str(""));
        t.set_str("team", Value::Num(if private { allies } else { free }));
        t.set_str("isLocal", Value::Num(1.0));
        t.set_str("controller", Value::Num(0.0));
        t.set_str("clientNum", Value::Num(0.0));
        t.set_str("isHost", Value::Bool(true));
        // (A score only in a game lobby: the party's list shows none.)
        if private {
            t.set_str("score", Value::Num(0.0));
        }
        t.set_str("isInParty", Value::Bool(false));
        t.set_str("subparty", Value::Num(0.0));
        t.set_str("isGuest", Value::Bool(false));
        t.set_str("rank", Value::Num((rank + 1) as f32));
        t.set_str("prestige", Value::Num(prestige as f32));
        t.set_str("rankIcon", Value::str(&icon));
        t.set_str("isReady", Value::Bool(true));
        t.set_str("missingMapPacks", Value::Num(0.0));
        t.set_str("leagueTeamID", Value::str("0"));
    }
    e
}

pub fn install(host: &mut Host) -> Mp {
    let mp: Mp = Rc::default();
    let values = host.values.clone();
    // Read the items the first time a function needs them.
    let state = move |mp: &Mp| {
        let v = values.borrow();
        let mut m = mp.borrow_mut();
        m.load(&v);
        m.online = v.session_modes.contains(&2.0);
        drop(m);
        mp.clone()
    };

    // setupGameMessages(index): one of the engine's message windows (the
    // kill feed is window 0); the owner fills it.
    let m = mp.clone();
    crate::lui::add_native(&mut host.vm, "setupGameMessages", move |_, a| {
        let el = arg(&a, 0);
        if let Some(u) = crate::lui::element(&el) {
            if let Some(e) = u.data.borrow_mut().downcast_mut::<crate::lui::Element>() {
                e.kind = "messages";
            }
            m.borrow_mut().windows.push((el, num(&a, 1)));
        }
        Ok(vec![])
    });
    // setupGameTimer(): the match clock (the engine writes its text).
    let m = mp.clone();
    crate::lui::add_native(&mut host.vm, "setupGameTimer", move |_, a| {
        if let Some(u) = crate::lui::element(&arg(&a, 0)) {
            if let Some(e) = u.data.borrow_mut().downcast_mut::<crate::lui::Element>() {
                e.kind = "text";
            }
            m.borrow_mut().game_timers.push(u);
        }
        Ok(vec![])
    });
    // GetGametypeSetting(name): the match's game type settings (any case).
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetGametypeSetting", move |_, a| {
        let k = arg(&a, 0).to_string();
        let m = m.borrow();
        let hit = m.settings.get(&k.to_ascii_lowercase()).copied().or_else(|| {
            let v = v.borrow();
            v.settings.get(&k).or_else(|| v.settings.get(&k.to_ascii_lowercase())).copied()
        });
        Ok(vec![Value::Num(hit.unwrap_or(0.0))])
    });
    // setTimeLeft(ms) on a game timer: the engine counts it down.
    let m = mp.clone();
    crate::lui::add_native(&mut host.vm, "setTimeLeft", move |_, a| {
        if let Some(u) = crate::lui::element(&arg(&a, 0)) {
            let end = crate::lui::now_ms() + f64::from(arg(&a, 1).as_num().unwrap_or(0.0));
            let mut m = m.borrow_mut();
            m.timers.retain(|(t, _)| !Rc::ptr_eq(t, &u));
            m.timers.push((u, end));
        }
        Ok(vec![])
    });
    // setupMiniIdentity(xuid, bool): his playercard (backing, rank icon, name),
    // drawn from child elements.
    let (m, v) = (mp.clone(), host.values.clone());
    crate::lui::add_native(&mut host.vm, "setupMiniIdentity", move |vm, a| {
        let Some(u) = crate::lui::element(&arg(&a, 0)) else { return Ok(vec![]) };
        let (name, icon) = {
            let t = lobby_member(&m.borrow(), &v.borrow(), (1.0, 0.0));
            let t = t.borrow();
            (
                t.get(&Value::str("gamertag")).as_str().unwrap_or("").to_owned(),
                t.get(&Value::str("rankIcon")).as_str().unwrap_or("").to_owned(),
            )
        };
        let font = crate::lui::font_of(&u);
        // The card is his calling card (the default's carbon hexagons)
        // with his emblem on it: until he draws one, his rank's own, in
        // the rank green (the PFC chevron).
        crate::lui::add_drawn_child(vm, &u, [0.0, 0.0, 250.0, 62.0], [1.0, 1.0, 1.0, 1.0], Some("emblem_bg_default"), None, None);
        if !icon.is_empty() {
            let emblem = match icon.strip_prefix("rank_") {
                Some("pfc") | None => "em_rank_pvt_full".to_owned(),
                Some(r) => format!("em_rank_{r}_full"),
            };
            crate::lui::add_drawn_child(vm, &u, [5.0, 14.0, 57.0, 40.0], [0.431, 0.584, 0.055, 1.0], Some(&emblem), None, None);
        }
        crate::lui::add_drawn_child(vm, &u, [63.0, 10.0, 245.0, 32.0], [1.0, 1.0, 1.0, 1.0], None, Some(&name), font);
        Ok(vec![])
    });
    // The front end's own widgets (engine-drawn; not drawn yet).
    for (name, kind) in [
        ("setupGlobe", "globe"),
        ("setupLiveStreamLegal", "text"),
        ("setupBitchinFX", "element"),
        // The voice chat dock (who is talking): nobody talks here.
        ("setupIngameTalkers", "element"),
    ] {
        crate::lui::add_kind_native(&mut host.vm, name, kind);
    }
    // The minimap (drawn by the engine): its kind says which compass,
    // `setupCompassUnderlay(controller, type)`: type 0 = the HUD minimap
    // (COMPASS_TYPE_PARTIAL), 1 = the pause menu's whole map (FULL).
    for (name, partial, full) in [
        ("setupCompassUnderlay", "compass_under", "compass_under_full"),
        ("setupCompassOverlay", "compass_over", "compass_over_full"),
        ("setupCompassItems", "compass_items", "compass_items_full"),
    ] {
        crate::lui::add_native(&mut host.vm, name, move |_, a| {
            let Some(Value::User(u)) = a.first() else { return Ok(vec![]) };
            let t = match a.get(2) {
                Some(Value::Num(n)) => *n,
                _ => a.get(1).and_then(Value::as_num).unwrap_or(0.0),
            };
            let kind = if t == 1.0 { full } else { partial };
            crate::lui::with(u, |e| e.kind = kind);
            Ok(vec![])
        });
    }
    {
        let mut v = host.values.borrow_mut();
        for &(k, x) in ENGINE_DVARS {
            v.dvars.entry(k.to_owned()).or_insert_with(|| x.to_owned());
        }
    }
    host.bind("UIExpression", "SessionMode_IsZombiesGame", |_, _| Ok(vec![Value::Num(0.0)]));
    host.bind("UIExpression", "GetCurrentMapTableName", |_, _| Ok(vec![Value::str("mp/mapstable.csv")]));
    host.bind("Engine", "CanPauseZombiesGame", |_, _| Ok(vec![Value::Bool(false)]));
    // His PC: one local player (keyboard and mouse, or one pad), stats
    // read from his saved file at start, no online service (Demonware):
    // the front end's local (LAN) path.
    let one = |_: &mut Vm, _: Vec<Value>| Ok(vec![Value::Num(1.0)]);
    let zero = |_: &mut Vm, _: Vec<Value>| Ok(vec![Value::Num(0.0)]);
    let yes = |_: &mut Vm, _: Vec<Value>| Ok(vec![Value::Bool(true)]);
    let no = |_: &mut Vm, _: Vec<Value>| Ok(vec![Value::Bool(false)]);
    let nothing = |_: &mut Vm, _: Vec<Value>| Ok(vec![]);
    for name in [
        "GetMaxControllerCount",
        "AreStatsFetched",
        "IsStableStatsBufferInitialized",
        "IsPrimaryLocalClient",
        "GetUsedControllerCount",
        "IsSignedIn",
        "IsDemonwareFetchingDone",
        "IsContentRatingAllowed",
        "PrivatePartyHost",
        "PrivatePartyHostInLobby",
        "InPrivateParty",
        "AloneInPartyIgnoreSplitscreen",
        "GameHost",
        "InLobby",
        // Signed in: BO2's ONLINE path (the lobbies, CUSTOM GAMES) needs
        // it; the rebuild's own service stands in for the online one.
        "IsSignedInToLive",
        "AnySignedInToLive",
    ] {
        host.bind("UIExpression", name, one);
    }
    for name in [
        "GetPrimaryController",
        "IsSubUser",
        "IsAnyControllerMPRestricted",
        "IsItemLockedForAll",
        "IsPlayerJoinable",
    ] {
        host.bind("UIExpression", name, zero);
    }
    // In a game (`cl_ingame` 1) or the front end (the options menu opens
    // its front-end form there).
    let v = host.values.clone();
    host.bind("UIExpression", "IsInGame", move |_, _| {
        let on = v.borrow().dvars.get("cl_ingame").is_some_and(|x| x.trim() == "1");
        Ok(vec![Value::Num(if on { 1.0 } else { 0.0 })])
    });
    host.bind("UIExpression", "GetXUID", |_, _| Ok(vec![Value::str("1")]));
    // His clan name: none ("" - the Weapon Tag tile then reads "[CLAN TAG]",
    // as BO2's own shot does; unanswered, the tab's script errors on a nil).
    host.bind("UIExpression", "GetClanName", |_, _| Ok(vec![Value::str("")]));
    // He owns every cosmetic (DLC camos and reticles: BO2's DLC tab shows
    // them as his, with no purchase arrow).
    host.bind("Engine", "HasMTX", yes);
    // GametypeName / GametypeDescription: the pause menu's mode block
    // ("Team Deathmatch" and its description; &&1 is the score limit).
    for (name, col) in [("GametypeName", 2usize), ("GametypeDescription", 3)] {
        let v = host.values.clone();
        host.bind("UIExpression", name, move |_, _| {
            let vb = v.borrow();
            let g = vb.dvars.get("ui_gametype").filter(|g| !g.is_empty()).cloned().unwrap_or_else(|| "tdm".to_owned());
            let key = table_rows(&vb, "mp/gametypestable.csv")
                .iter()
                .find(|r| cell(r, 0) == "0" && cell(r, 1) == g)
                .map(|r| cell(r, col))
                .unwrap_or_default();
            let up = key.to_ascii_uppercase();
            let mut cands: Vec<String> = Vec::new();
            if col == 2 {
                // mixed-case name: the same key without its _CAPS suffix
                if let Some(base) = up.strip_suffix("_CAPS") { cands.push(base.to_owned()); }
            } else {
                cands.push(format!("OBJECTIVES_{}_SCORE", g.to_ascii_uppercase()));
                cands.push(format!("OBJECTIVES_{}", g.to_ascii_uppercase()));
            }
            cands.push(up);
            let text = cands.iter().find_map(|k| vb.localize.get(k).cloned()).unwrap_or(key);
            let limit = vb.dvars.get("ui_scorelimit").and_then(|x| x.trim().parse::<f32>().ok()).or_else(|| vb.settings.get("scorelimit").copied()).unwrap_or(0.0);
            Ok(vec![Value::str(&text.replace("&&1", &format!("{}", limit as i64)))])
        });
    }
    // The name card's background (his emblem background; the default's).
    host.bind("UIExpression", "EmblemPlayerBackgroundMaterial", |_, _| Ok(vec![Value::str("emblem_bg_default")]));
    // FormatNumberWithCommas(controller, n): "12,345".
    host.bind("UIExpression", "FormatNumberWithCommas", |_, a| {
        let n = arg(&a, 1).as_num().unwrap_or(0.0).round() as i64;
        let digits = n.unsigned_abs().to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        Ok(vec![Value::str(&if n < 0 { format!("-{out}") } else { out })])
    });
    for name in [
        "CheckNetConnection",
        "CanViewContent",
        "HasMPPrivileges",
        "OwnSeasonPass",
        "HasDLCForItem",
        "DoesPartyHaveDLCForMap",
        "IsContentAvailableByPakName",
        // No game setting changed from the mode's own defaults (the
        // options list marks changed ones).
        "IsGametypeSettingDefault",
        "IsSignedInToDemonware",
        "IsPresetClassDefault",
    ] {
        host.bind("Engine", name, yes);
    }
    for name in [
        "IsBetaBuild",
        "SystemNeedsUpdate",
        "IsFeatureBanned",
        "IsLivestreamEnabled",
        "IsItemIndexRestricted",
        "IsAttachmentIndexRestricted",
        "IsLoadoutSlotNew",
        "IsWeaponOptionGroupNew",
        "IsWeaponOptionNew",
        "WeaponGroupHasNewItem",
        "AreAnyAttachmentsNew",
        "IsEliteAvailable",
        "IsEliteButtonAvailable",
        "IsPlayerEliteRegistered",
        "ShouldShowMOTD",
        "IsCodtvContentLoaded",
        "IsChatRestricted",
        "IsGuestByXuid",
        "IsMemberInParty",
        "PartyIsWaiting",
        "PartyLockedIn",
        "ProbationCheckForProbation",
        "ProbationCheckInProbation",
        "ProbationCheckParty",
        "ProbationCheckForDashboardWarning",
        "ECACImport_ShouldShow",
        "ERegPopup_ShouldShow",
        "EWelcomePopup_ShouldShow",
        "EMarketingOptInPopup_ShouldShow",
        "IsYouTubeAccountChecked",
        "IsYouTubeAccountRegistered",
    ] {
        host.bind("Engine", name, no);
    }
    for name in [
        "PartyHostSetUIState",
        "PartyHostClearUIState",
        "SetStartCheckoutTimestampUTC",
        "SendDLCMenusViewedRecordEvent",
        "PartySetMaxPlayerCount",
        "SetItemAsOld",
    ] {
        host.bind("Engine", name, nothing);
    }
    for name in ["GetProbationTime", "GetCurrentTokens"] {
        host.bind("Engine", name, zero);
    }
    for name in [
        "IsAnythingInCACNew",
        "IsItemNew",
        "IsVacBanned",
        "IsMTXAvailable",
        "SkipMTXItem",
        "OwnDLC1Only",
        "ShouldShowDSPPromotion",
        "ShouldShowGhostUpsellPopup",
        "ShouldShowSPReminder",
        "IsPlayerEliteFounder",
        "IsCustomElementScrollLanguageOverrideActive",
    ] {
        host.bind("Engine", name, no);
    }
    // A weapon's camo option group is new to a fresh profile (Edit Class
    // shows NEW beside "Personalize Weapon"); nothing else is.
    host.bind("Engine", "IsWeaponOptionGroupNew", |vm, a| {
        let cod = vm.global("CoD");
        let util = vm.index(&cod, &Value::str("CACUtility")).unwrap_or(Value::Nil);
        let camo = vm.index(&util, &Value::str("WEAPONOPTION_GROUP_CAMO")).ok().and_then(|x| x.as_num());
        Ok(vec![Value::Bool(camo.is_some() && arg(&a, 2).as_num() == camo)])
    });
    host.bind("Engine", "GetDLC0PublisherOfferId", zero);
    host.bind("Engine", "ServerListUpdateFilter", nothing);
    host.bind("Engine", "SetMouseCursor", nothing);
    host.bind("Engine", "GetLanguage", |_, _| Ok(vec![Value::str("english")]));
    host.bind("Engine", "GetDLCNameForItem", |_, _| Ok(vec![Value::str("")]));
    host.bind("UIExpression", "IsFFOTDFetched", one);
    // The other stats layouts the menus read: gametype settings
    // (ddl_mp/gametype_settings.ddl) and the exe's profile
    // (ddl_mp/profile_mp.ddl), in the same store under their own roots.
    for (name, root) in [("GetGametypeSettings", "gametypesettings"), ("GetPlayerExeGamerProfile", "profile")] {
        let m = mp.clone();
        host.bind("Engine", name, move |vm, _| Ok(vec![node(vm, &m, root.to_owned())]));
    }

    // Gun levels (mp/gunlevels.csv: rank, xp to reach it, weapon,
    // attachment it unlocks): a weapon's level from its itemstats xp.
    let gun = {
        let (st, m, v) = (state.clone(), mp.clone(), host.values.clone());
        move |a: &[Value]| -> (i32, i32, Vec<i32>) {
            let m = st(&m);
            let m = m.borrow();
            let i = num(a, 1);
            let xp = m.stat(&format!("itemstats.{i}.xp")).as_num().unwrap_or(0.0) as i32;
            let r = m.item(i).map(|it| it.reference.clone()).unwrap_or_default();
            let vb = v.borrow();
            let steps: Vec<i32> = table(&vb, "mp/gunlevels.csv")
                .into_iter()
                .flatten()
                .filter(|row| row.get(2) == Some(&r))
                .map(|row| int(row, 1))
                .collect();
            let rank = steps.iter().filter(|s| **s <= xp).count() as i32;
            (rank, xp, steps)
        }
    };
    let g = gun.clone();
    host.bind("Engine", "GetGunCurrentRank", move |_, a| Ok(vec![Value::Num(g(&a).0 as f32)]));
    let g = gun.clone();
    host.bind("Engine", "GetGunNextRank", move |_, a| {
        let (rank, _, steps) = g(&a);
        Ok(vec![Value::Num((rank + 1).min(steps.len() as i32) as f32)])
    });
    // ui/t6/weaponlevel.lua's bar: (xp - PrevRankXP) / (CurrentRankXP -
    // PrevRankXP), CurrentRankXP the xp his next level needs (the last one
    // once he has them all), PrevRankXP the xp his level started at.
    let g = gun.clone();
    host.bind("Engine", "GetGunCurrentRankXP", move |_, a| {
        let (rank, _, steps) = g(&a);
        let at = steps.get(rank as usize).or(steps.last()).copied().unwrap_or(0);
        Ok(vec![Value::Num(at as f32)])
    });
    let g = gun;
    host.bind("Engine", "GetGunPrevRankXP", move |_, a| {
        let (rank, _, steps) = g(&a);
        let at = usize::try_from(rank - 1).ok().and_then(|r| steps.get(r)).copied().unwrap_or(0);
        Ok(vec![Value::Num(at as f32)])
    });
    // GetMaxAmmoForItem(index): the most of a grenade a class can carry
    // (its weapon file's maxAmmo, bo2mp/maxammo.csv; 1 when unknown).
    let (st, m, v) = (state.clone(), mp.clone(), host.values.clone());
    host.bind("Engine", "GetMaxAmmoForItem", move |_, a| {
        let m = st(&m);
        let r = m.borrow().item(num(&a, 0)).map(|it| it.reference.clone()).unwrap_or_default();
        let vb = v.borrow();
        let max = table(&vb, "bo2mp/maxammo.csv")
            .into_iter()
            .flatten()
            .find(|row| row.first().is_some_and(|n| *n == format!("{r}_mp") || *n == r))
            .map_or(1, |row| int(row, 1).max(1));
        Ok(vec![Value::Num(max as f32)])
    });
    host.bind("Engine", "GetPermanentUnlockCount", zero);
    host.bind("UIExpression", "GetUnlockIndexFromGroupName", zero);
    host.bind("UIExpression", "GetUnlockLocString", |_, _| Ok(vec![Value::str("")]));

    // Factions: GetFactionForTeam(team) = the map's faction for that team
    // (mp/mapstable.csv: column 1 the allies, 2 the axis); its colour from
    // mp/factiontable.csv (0-255).
    let v = host.values.clone();
    host.bind("Engine", "GetFactionForTeam", move |vm, a| {
        let team = arg(&a, 0).as_num();
        let cod = vm.global("CoD");
        let mut col = 0;
        for (name, c) in [("TEAM_ALLIES", 1), ("TEAM_AXIS", 2)] {
            if vm.index(&cod, &Value::str(name))?.as_num().is_some_and(|n| Some(n) == team) {
                col = c;
            }
        }
        // (Free for all: the table's "free" row, black.)
        if col == 0 && vm.index(&cod, &Value::str("TEAM_FREE"))?.as_num().is_some_and(|n| Some(n) == team) {
            return Ok(vec![Value::str("free")]);
        }
        let vb = v.borrow();
        let map = vb.dvars.get("ui_mapname").cloned().unwrap_or_default();
        let hit = (col > 0)
            .then(|| table(&vb, "mp/mapstable.csv"))
            .flatten()
            .and_then(|rows| rows.iter().find(|r| r.first() == Some(&map)))
            .map(|r| cell(r, col));
        Ok(vec![Value::str(&hit.unwrap_or_default())])
    });
    // GetIString(value, "CS_LOCALIZED_STRINGS"): a localized string the
    // server sent by its config-string index. Here the server sends the key
    // itself (`&"SCORE_KILL"` as "SCORE_KILL"), so it is the answer (the
    // score popup's label; nil for a bare number).
    host.bind("Engine", "GetIString", |_, a| {
        Ok(vec![match arg(&a, 0) {
            Value::Str(s) => Value::Str(s),
            _ => Value::Nil,
        }])
    });
    // GetScoreBoardColumnName(controller, i): the game type's i-th column
    // (`setscoreboardcolumns`) as BO2's string key, CGAME_SB_<NAME>
    // ("Score", "Kills", "Deaths", "Ratio", "Assists").
    let v = host.values.clone();
    host.bind("Engine", "GetScoreBoardColumnName", move |_, a| {
        let i = arg(&a, 1).as_num().map_or(0, |n| n.max(0.0) as usize);
        let key = v.borrow().columns.get(i).map(|c| format!("CGAME_SB_{}", c.to_ascii_uppercase())).unwrap_or_default();
        Ok(vec![Value::str(&key)])
    });
    // The scoreboard (Tab): teams by score, then each team's rows by
    // score; a row's index is its place in `board`.
    let m = mp.clone();
    host.bind("Engine", "GetTeamPositions", move |_, _| {
        let mut teams = m.borrow().team_scores.clone();
        teams.sort_by(|a, b| b.1.total_cmp(&a.1));
        let t = Table::new_ref();
        for (i, (team, score)) in teams.iter().enumerate() {
            let row = Table::new_ref();
            row.borrow_mut().set_str("team", Value::Num(*team as f32));
            row.borrow_mut().set_str("score", Value::Num(*score));
            t.borrow_mut().set(Value::Num((i + 1) as f32), Value::Table(row));
        }
        Ok(vec![Value::Table(t)])
    });
    let m = mp.clone();
    host.bind("Engine", "GetMatchScoreboardClientCount", move |_, a| {
        let m = m.borrow();
        let n = match arg(&a, 0).as_num() {
            Some(t) => m.board.iter().filter(|r| r.team == t as i32).count(),
            None => m.board.len(),
        };
        Ok(vec![Value::Num(n as f32)])
    });
    // (i, team, sort): the i-th row of that team -> index, client.
    let m = mp.clone();
    host.bind("Engine", "GetMatchScoreboardIndexAndClientNumForTeam", move |_, a| {
        let m = m.borrow();
        let (i, team) = (num(&a, 0), num(&a, 1));
        let hit = m.board.iter().enumerate().filter(|(_, r)| r.team == team).nth(usize::try_from(i).unwrap_or(usize::MAX));
        Ok(match hit {
            Some((index, r)) => vec![Value::Num(index as f32), Value::Num(r.client as f32)],
            None => vec![Value::Nil, Value::Nil],
        })
    });
    let m = mp.clone();
    host.bind("Engine", "GetFullGamertagForScoreboardIndex", move |_, a| {
        let m = m.borrow();
        let r = usize::try_from(num(&a, 0)).ok().and_then(|i| m.board.get(i));
        Ok(vec![Value::str(r.map_or("", |r| r.name.as_str()))])
    });
    // (The Ratio column prints two decimals, "1.20", as BO2's own.)
    let m = mp.clone();
    let v = host.values.clone();
    host.bind("Engine", "GetScoreboardColumnForScoreboardIndex", move |_, a| {
        let m = m.borrow();
        let r = usize::try_from(num(&a, 0)).ok().and_then(|i| m.board.get(i));
        let c = usize::try_from(num(&a, 1)).ok();
        let x = r.and_then(|r| c.and_then(|c| r.cols.get(c))).copied().unwrap_or(0.0);
        let ratio = c.and_then(|c| v.borrow().columns.get(c).map(|n| n.to_ascii_lowercase().contains("ratio"))).unwrap_or(false);
        Ok(vec![if ratio { Value::str(&format!("{x:.2}")) } else { Value::Num(x) }])
    });
    // The Ping column (his real game: every bot 0, himself 13-14 ms - the
    // listen server's own client). We keep no per-client latency, so he gets
    // a steady 12-16 ms fixed by his name and the bots 0.
    let m = mp.clone();
    host.bind("Engine", "GetPingForScoreboardIndex", move |_, a| {
        let m = m.borrow();
        let r = usize::try_from(num(&a, 0)).ok().and_then(|i| m.board.get(i));
        let ping = r.filter(|r| r.client == 0).map_or(0, |r| 12 + r.name.bytes().map(u32::from).sum::<u32>() % 5);
        Ok(vec![Value::Num(ping as f32)])
    });
    for name in ["GetPrestigeForScoreboardIndex", "GetRoundsPlayed"] {
        host.bind("Engine", name, zero);
    }
    // (Every bot is level 1.)
    host.bind("Engine", "GetRankForScoreboardIndex", |_, _| Ok(vec![Value::Num(1.0)]));
    // (A bot's level 1 icon, its row of mp/rankIconTable.csv.)
    let v = host.values.clone();
    host.bind("Engine", "GetRankIconForScoreboardIndex", move |_, _| {
        let icon = table_rows(&v.borrow(), "mp/rankicontable.csv")
            .iter()
            .find(|r| r.first().is_some_and(|c| c.trim() == "1"))
            .and_then(|r| r.get(1))
            .cloned()
            .unwrap_or_default();
        Ok(vec![Value::str(&icon)])
    });
    // The status icon beside a row: the dead icon once the match is over.
    let m = mp.clone();
    host.bind("Engine", "GetStatusIconForClient", move |_, _| {
        Ok(vec![Value::str(if m.borrow().game_ended { "hud_status_dead" } else { "" })])
    });
    host.bind("Engine", "GetMatchScoreboardClientXuid", |_, _| Ok(vec![Value::str("0")]));
    host.bind("Engine", "IsPlayerMuteToggled", no);
    host.bind("Engine", "BlockGameFromKeyEvent", nothing);
    host.bind("Engine", "TogglePlayerMute", nothing);
    // GetFactionForClient(client): his team's faction (one local player).
    let v = host.values.clone();
    host.bind("Engine", "GetFactionForClient", move |vm, _| {
        let team = v.borrow().team as f32;
        let f = vm.global("Engine");
        let g = vm.index(&f, &Value::str("GetFactionForTeam"))?;
        vm.call(g, vec![Value::Num(team)])
    });
    // GetPredictedClientNum(controller): the player the killcam watches (the
    // killer); GetCalloutPlayerData(controller, client): his name card
    // (CoD.NamePlate reads it): name, clan tag, rank and its icon.
    let m = mp.clone();
    host.bind("Engine", "GetPredictedClientNum", move |_, _| {
        Ok(vec![Value::Num(m.borrow().callout.unwrap_or(0) as f32)])
    });
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetCalloutPlayerData", move |_, a| {
        let client = arg(&a, a.len().saturating_sub(1)).as_num().map_or(0, |n| n as i32);
        let name = m.borrow().board.iter().find(|r| r.client == client).map(|r| r.name.clone());
        let icon = table_rows(&v.borrow(), "mp/rankicontable.csv")
            .iter()
            .find(|r| r.first().is_some_and(|c| c.trim() == "1"))
            .and_then(|r| r.get(1))
            .cloned()
            .unwrap_or_default();
        let t = Table::new_ref();
        {
            let mut t = t.borrow_mut();
            t.set_str("xuid", Value::str("0"));
            // The board's name carries his clan tag as "[tag]name"; the card
            // sets the tag on its own line under the name.
            let full = name.unwrap_or_else(|| "Player".to_owned());
            let (tag, bare) = match full.strip_prefix('[').and_then(|r| r.split_once(']')) {
                Some((tag, rest)) if !tag.is_empty() => (tag.to_owned(), rest.to_owned()),
                _ => (String::new(), full.clone()),
            };
            t.set_str("playerName", Value::str(&bare));
            t.set_str("clanTag", Value::str(&if tag.is_empty() { String::new() } else { format!("[{tag}]") }));
            t.set_str("playerClientNum", Value::Num(client as f32));
            t.set_str("rank", Value::Num(1.0));
            t.set_str("prestige", Value::Num(0.0));
            t.set_str("rankIcon", Value::str(&icon));
        }
        Ok(vec![Value::Table(t)])
    });
    // GetTeamID(controller, client): his team (`CoD.TEAM_*`), another
    // player's from the scoreboard.
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetTeamID", move |_, a| {
        let client = arg(&a, a.len().saturating_sub(1)).as_num().map_or(0, |n| n as i32);
        let row = m.borrow().board.iter().find(|r| r.client == client).map(|r| r.team);
        Ok(vec![Value::Num(row.unwrap_or(v.borrow().team) as f32)])
    });
    let v = host.values.clone();
    host.bind("Engine", "GetFactionColor", move |_, a| {
        let f = arg(&a, 0).to_string();
        let vb = v.borrow();
        let row = table(&vb, "mp/factiontable.csv").and_then(|rows| rows.iter().find(|r| r.first() == Some(&f)));
        let Some(r) = row else { return Ok(vec![]) };
        Ok((2..5).map(|i| Value::Num(int(r, i) as f32 / 255.0)).collect())
    });
    // His party (him alone, its host) and the lobby around it: no friends
    // online, no invites, not after a match; a private match shows its
    // teams and runs a game lobby.
    host.bind("Engine", "GetTitleFriendsOfAllLocalPlayers", |_, _| Ok(vec![Value::Table(Table::new_ref())]));
    for name in ["IsJoiningAnotherParty", "PartyIsPostGame"] {
        host.bind("Engine", name, |_, _| Ok(vec![Value::Bool(false)]));
    }
    for name in ["IsPartyLobbyRunning", "IsXuidPrivatePartyHost", "PartyShowTruePlayerInfo"] {
        host.bind("Engine", name, |_, _| Ok(vec![Value::Bool(true)]));
    }
    host.bind("UIExpression", "AcceptingInvite", |_, _| Ok(vec![Value::Num(0.0)]));
    host.bind("Engine", "GetMaxUserPlayerCount", |_, _| Ok(vec![Value::Num(18.0)]));
    for name in ["PartyShowTeams", "IsGameLobbyRunning"] {
        let m = mp.clone();
        host.bind("Engine", name, move |_, _| Ok(vec![Value::Bool(m.borrow().game_modes.contains(&1))]));
    }
    // GetProfileVarInt(controller, name): his profile's number.
    let (v, m) = (host.values.clone(), mp.clone());
    host.bind("Engine", "GetProfileVarInt", move |_, a| {
        let name = arg(&a, 1).as_str().unwrap_or("").to_owned();
        // Party Privacy defaults: Open in the public lobby, Closed in a private match.
        let dflt = if name == "party_privacyStatus" { if m.borrow().game_modes.contains(&1) { 3.0 } else { 0.0 } } else { 0.0 };
        let n = v.borrow().profile.get(&name).and_then(|x| x.trim().parse::<f32>().ok()).unwrap_or(dflt);
        Ok(vec![Value::Num(n)])
    });
    // The lobby's members (CoD.LobbyPlayerLists:Update): him alone, the
    // party's host (BO2's bots are not lobby members).
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetPlayersInLobby", move |vm, _| {
        let cod = vm.global("CoD");
        let field = |vm: &mut Vm, k: &str, d: f32| vm.index(&cod, &Value::str(k)).ok().and_then(|x| x.as_num()).unwrap_or(d);
        let teams = (field(vm, "TEAM_ALLIES", 1.0), field(vm, "TEAM_FREE", 0.0));
        let members = Table::new_ref();
        members.borrow_mut().set(Value::Num(1.0), Value::Table(lobby_member(&m.borrow(), &v.borrow(), teams)));
        Ok(vec![Value::Table(members)])
    });
    // A selected player's card (the lobby's overview pane): his own row's
    // facts, and no league team (BO2 offline has none).
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetPlayerInfoByXuid", move |vm, _| {
        let cod = vm.global("CoD");
        let field = |vm: &mut Vm, k: &str, d: f32| vm.index(&cod, &Value::str(k)).ok().and_then(|x| x.as_num()).unwrap_or(d);
        let teams = (field(vm, "TEAM_ALLIES", 1.0), field(vm, "TEAM_FREE", 0.0));
        Ok(vec![Value::Table(lobby_member(&m.borrow(), &v.borrow(), teams))])
    });
    host.bind("Engine", "GetLeagueTeamInfo", |_, _| Ok(vec![Value::Table(Table::new_ref())]));
    // League teams, offline: only the Solo team (id 1), his own, with no
    // placement yet (the Barracks tab's card and "Awaiting Placement").
    host.bind("Engine", "GetLeagueTeams", |_, _| {
        let solo = Table::new_ref();
        solo.borrow_mut().set_str("teamID", Value::Num(1.0));
        let t = Table::new_ref();
        t.borrow_mut().set_str("soloTeamID", Value::Num(1.0));
        t.borrow_mut().set_str("numTeams", Value::Num(0.0));
        t.borrow_mut().set(Value::Num(1.0), Value::Table(solo));
        Ok(vec![Value::Table(t)])
    });
    let v = host.values.clone();
    host.bind("Engine", "GetLeagueTeamMemberInfo", move |_, _| {
        let name = v.borrow().dvars.get("name").filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| "Player".to_owned());
        let me = Table::new_ref();
        me.borrow_mut().set_str("userName", Value::str(&name));
        let members = Table::new_ref();
        members.borrow_mut().set(Value::Num(1.0), Value::Table(me));
        let t = Table::new_ref();
        t.borrow_mut().set_str("members", Value::Table(members));
        t.borrow_mut().set_str("numMembers", Value::Num(1.0));
        Ok(vec![Value::Table(t)])
    });
    // (fetch status, divisions): none yet.
    host.bind("Engine", "GetLeagueTeamSubdivisionInfos", |_, _| {
        Ok(vec![Value::str("fetched"), Value::Table(Table::new_ref())])
    });
    host.bind("Engine", "PartyShowTruePlayerInfoByXuid", |_, _| Ok(vec![Value::Bool(false)]));
    // GetSystemInfo(controller, what): the lobby footer's NAT line.
    host.bind("UIExpression", "GetSystemInfo", |_, a| {
        let nat_type_lobby = 10.0;
        Ok(vec![Value::str(if arg(&a, 1).as_num() == Some(nat_type_lobby) { "NAT: Moderate" } else { "" })])
    });
    // An online session (the lobby's NAT / Party Privacy footer, the
    // "N Online" count). Engine.GetPlayerGroupCount(name): the count text.
    let v = host.values.clone();
    host.bind("UIExpression", "SessionMode_IsOnlineGame", move |_, _| {
        Ok(vec![Value::Num(if v.borrow().session_modes.contains(&2.0) { 1.0 } else { 0.0 })])
    });
    host.bind("Engine", "GetPlayerGroupCount", |_, _| Ok(vec![Value::str("1")]));

    // The private match's choices. GetGametypesBase(): the base game
    // types (mp/gametypestable.csv rows marked 0: ref, name, description,
    // icon, sort, ..., category); GetMaps(): every map of
    // mp/mapstable.csv (load name, name, image, order, description, ...,
    // map pack). Entries as the change-mode / change-map popups read them.
    let v = host.values.clone();
    host.bind("Engine", "GetGametypesBase", move |_, _| {
        let t = Table::new_ref();
        let vb = v.borrow();
        let mut n = 0;
        // In the table's sort order (TDM, FFA, DOM, DEM, KC, KOTH, HQ, CTF, S&D).
        let mut rows: Vec<&Vec<String>> = table(&vb, "mp/gametypestable.csv").into_iter().flatten().filter(|r| cell(r, 0) == "0").collect();
        rows.sort_by_key(|r| int(r, 5));
        for r in rows {
            let e = Table::new_ref();
            for (k, x) in [("gametype", 1), ("ref", 1), ("name", 2), ("description", 3), ("icon", 4), ("category", 9)] {
                e.borrow_mut().set_str(k, Value::str(&cell(r, x)));
            }
            e.borrow_mut().set_str("sortIndex", Value::Num(int(r, 5) as f32));
            e.borrow_mut().set_str("locked", Value::Bool(false));
            n += 1;
            t.borrow_mut().set(Value::Num(n as f32), Value::Table(e));
        }
        Ok(vec![Value::Table(t)])
    });
    // Combat Record: no stats are tracked, so the sorted list is empty
    // (the card code reads itemIndex/itemValue of an entry without a
    // nil check, so the entry exists with a zero value).
    host.bind("Engine", "SortItemsForCombatRecord", |_, _| Ok(vec![]));
    host.bind("Engine", "GetCombatRecordSortedItemInfo", |_, _| {
        let e = Table::new_ref();
        e.borrow_mut().set_str("itemIndex", Value::Num(0.0));
        e.borrow_mut().set_str("itemValue", Value::Num(0.0));
        Ok(vec![Value::Table(e)])
    });
    let v = host.values.clone();
    host.bind("Engine", "GetMaps", move |_, _| {
        let t = Table::new_ref();
        let vb = v.borrow();
        let mut n = 0;
        for r in table(&vb, "mp/mapstable.csv").into_iter().flatten().filter(|r| cell(r, 0).starts_with("mp_")) {
            let e = Table::new_ref();
            for (k, x) in [("loadName", 0), ("name", 3), ("icon", 4), ("description", 6)] {
                e.borrow_mut().set_str(k, Value::str(&cell(r, x)));
            }
            e.borrow_mut().set_str("sortIndex", Value::Num(int(r, 5) as f32));
            e.borrow_mut().set_str("mapPackTypeIndex", Value::Num(int(r, 11) as f32));
            e.borrow_mut().set_str("locked", Value::Bool(false));
            n += 1;
            t.borrow_mut().set(Value::Num(n as f32), Value::Table(e));
        }
        Ok(vec![Value::Table(t)])
    });
    let v = host.values.clone();
    host.bind("Engine", "IsMapValid", move |_, a| {
        let map = arg(&a, 0).to_string();
        let vb = v.borrow();
        let hit = table(&vb, "mp/mapstable.csv").is_some_and(|rows| rows.iter().any(|r| r.first() == Some(&map)));
        Ok(vec![Value::Bool(hit)])
    });
    // His new account: every emblem icon and calling card is new (the
    // barracks tiles show NEW, as the real front end does for a fresh
    // player).
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "IsAnyEmblemIconNew", move |_, _| {
        let (m, vb) = (m.borrow(), v.borrow());
        let new = table_rows(&vb, "mp/emblemsorbackings.csv")
            .iter()
            .filter(|r| cell(r, 0) == "icon")
            .any(|r| !m.emblem.old.contains(&int(r, 2)) && !emblem_icon_locked(&m, &vb, int(r, 2)));
        Ok(vec![Value::Bool(new)])
    });
    host.bind("Engine", "IsAnyEmblemBackgroundNew", yes);
    // The Emblem Editor card's list screen (the CODTv menu, root
    // `emblems`): the file list is folders by index. 1 = the root (title
    // EMBLEMS, two lists), 2 = his emblem slots (an online file list: the
    // answer is empty), 3 = NEW EMBLEM (one button).
    host.bind("Engine", "GetCodtvRoot", |_, a| Ok(vec![if arg(&a, 0).to_string() == "emblems" { Value::Num(1.0) } else { Value::Nil }]));
    host.bind("Engine", "GetCodtvContent", |_, a| {
        let folder = |name: &str, index: f32, kind: &str, subs: f32| {
            let e = Table::new_ref();
            e.borrow_mut().set_str("name", Value::str(name));
            e.borrow_mut().set_str("folderIndex", Value::Num(index));
            e.borrow_mut().set_str("type", Value::str(kind));
            e.borrow_mut().set_str("subfolderCount", Value::Num(subs));
            e
        };
        match arg(&a, 0).as_num().unwrap_or(0.0) as i64 {
            1 => {
                let r = folder("EMBLEMS", 1.0, "folder", 2.0);
                r.borrow_mut().set(Value::Num(1.0), Value::Table(folder("EMBLEMS", 2.0, "dwfolder", 0.0)));
                r.borrow_mut().set(Value::Num(2.0), Value::Table(folder("NEW EMBLEM", 3.0, "folder", 1.0)));
                return Ok(vec![Value::Table(r)]);
            }
            2 => return Ok(vec![Value::Table(folder("EMBLEMS", 2.0, "dwfolder", 0.0))]),
            3 => {
                let r = folder("NEW EMBLEM", 3.0, "folder", 1.0);
                let b = folder("NEW EMBLEM", 4.0, "custombutton", 0.0);
                b.borrow_mut().set_str("action", Value::str("newemblem"));
                b.borrow_mut().set_str("imageName", Value::str("white"));
                b.borrow_mut().set_str("imageType", Value::str("material"));
                b.borrow_mut().set_str("parentFolderIndex", Value::Num(3.0));
                r.borrow_mut().set(Value::Num(1.0), Value::Table(b));
                return Ok(vec![Value::Table(r)]);
            }
            _ => {}
        }
        Ok(vec![Value::Nil])
    });
    // The emblem slots: none used, 15 free; the next free one is the first.
    host.bind("Engine", "GetFileshareCategories", |_, _| {
        let c = Table::new_ref();
        c.borrow_mut().set_str("occupied", Value::Num(0.0));
        c.borrow_mut().set_str("remaining", Value::Num(15.0));
        let t = Table::new_ref();
        t.borrow_mut().set(Value::Num(1.0), Value::Table(c));
        Ok(vec![Value::Table(t)])
    });
    host.bind("Engine", "GetFileshareNextSlot", |_, _| Ok(vec![Value::Num(1.0)]));
    host.bind("Engine", "IsEmblemEmpty", no);
    // The emblem being edited (`MpState::emblem`): the layer chosen's icon
    // (-1 = empty) and colour, and how many layers have an icon.
    let m = mp.clone();
    host.bind("Engine", "GetSelectedLayerIconID", move |_, a| {
        let m = m.borrow();
        let layer = arg(&a, 1).as_num().map_or(m.emblem.selected, |n| n.max(0.0) as usize);
        Ok(vec![Value::Num(m.emblem.icon(layer) as f32)])
    });
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetSelectedLayerColor", move |_, _| {
        let m = m.borrow();
        let rgba = m.emblem.layers.get(m.emblem.selected).and_then(|l| l.rgba).unwrap_or_else(|| emblem_default_color(&v.borrow()));
        let t = Table::new_ref();
        for (k, c) in ["red", "green", "blue", "alpha"].into_iter().zip(rgba) {
            t.borrow_mut().set_str(k, Value::Num(c));
        }
        Ok(vec![Value::Table(t)])
    });
    let m = mp.clone();
    host.bind("Engine", "GetUsedLayerCount", move |_, _| Ok(vec![Value::Num(m.borrow().emblem.used() as f32)]));
    // The icon selector (menus/EmblemEditorIconSelector): GetEmblemScaleMode
    // = the editor's scale mode; GetEmblemIconIndexInCategory(controller,
    // filter, icon) = the icon's place in that filter (-1 = not in it);
    // UIExpression.EmblemFilterCount(controller, 0, filter) and
    // EmblemFilterIconID(controller, 0, filter, n) = the filter's size and
    // its n-th icon id; EmblemIconIsLocked(controller, icon);
    // SetEmblemIconAsOld(controller, icon) ends its NEW tag.
    let m = mp.clone();
    host.bind("Engine", "GetEmblemScaleMode", move |_, _| Ok(vec![Value::Num(m.borrow().emblem.scale_mode as f32)]));
    let v = host.values.clone();
    host.bind("Engine", "GetEmblemIconIndexInCategory", move |_, a| {
        let id = num(&a, 2);
        let at = emblem_filter_icons(&v.borrow(), num(&a, 1)).iter().position(|r| int(r, 2) == id);
        Ok(vec![Value::Num(at.map_or(-1.0, |i| i as f32))])
    });
    let v = host.values.clone();
    host.bind("UIExpression", "EmblemFilterCount", move |_, a| {
        Ok(vec![Value::Num(emblem_filter_icons(&v.borrow(), num(&a, 2)).len() as f32)])
    });
    let v = host.values.clone();
    host.bind("UIExpression", "EmblemFilterIconID", move |_, a| {
        let n = usize::try_from(num(&a, 3)).ok();
        let id = n.and_then(|n| emblem_filter_icons(&v.borrow(), num(&a, 2)).get(n).map(|r| int(r, 2)));
        Ok(vec![Value::Num(id.map_or(-1.0, |i| i as f32))])
    });
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "EmblemIconIsLocked", move |_, a| Ok(vec![Value::Bool(emblem_icon_locked(&m.borrow(), &v.borrow(), num(&a, 1)))]));
    let m = mp.clone();
    host.bind("Engine", "SetEmblemIconAsOld", move |_, a| {
        m.borrow_mut().emblem.old.insert(num(&a, 1));
        Ok(vec![])
    });
    // GetChallengeInfoByEmblemOrBackingId(controller, id, kind) = a list with
    // the challenge that gives it (none = nil): the milestone's row and
    // table number, its kind (a game mode's challenge names the mode by its
    // place in mp/gametypesTable.csv; the rest read as global), whether it
    // is still locked. GetGametypeName(n) = that mode's name key without
    // its MPUI_.
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetChallengeInfoByEmblemOrBackingId", move |_, a| {
        let vb = v.borrow();
        let Some((row, t, c)) = emblem_challenge(&vb, num(&a, 2), num(&a, 1)) else { return Ok(vec![Value::Nil]) };
        let modes: Vec<&Vec<String>> = table_rows(&vb, "mp/gametypestable.csv").iter().filter(|r| cell(r, 0) == "0").collect();
        let mode = (cell(&c, 3) == "gamemode")
            .then(|| cell(&c, 13).split_whitespace().next().and_then(|g| modes.iter().position(|r| cell(r, 1) == g)))
            .flatten();
        let e = Table::new_ref();
        {
            let mut e = e.borrow_mut();
            e.set_str("challengeRow", Value::Num(row as f32));
            e.set_str("tableNum", Value::Num(t as f32));
            e.set_str("challengeType", Value::Num(if mode.is_some() { 2.0 } else { 0.0 }));
            e.set_str("isDefault", Value::Bool(false));
            e.set_str("isLocked", Value::Bool(emblem_milestone_locked(&m.borrow(), &c)));
            if let Some(i) = mode {
                e.set_str("itemIndex", Value::Num(i as f32));
            }
        }
        let list = Table::new_ref();
        list.borrow_mut().set(Value::Num(1.0), Value::Table(e));
        Ok(vec![Value::Table(list)])
    });
    let v = host.values.clone();
    host.bind("Engine", "GetGametypeName", move |_, a| {
        let vb = v.borrow();
        let name = usize::try_from(num(&a, 0)).ok().and_then(|n| {
            table_rows(&vb, "mp/gametypestable.csv").iter().filter(|r| cell(r, 0) == "0").nth(n).map(|r| cell(r, 7))
        });
        let name = name.unwrap_or_default();
        Ok(vec![Value::str(name.strip_prefix("MPUI_").unwrap_or(&name))])
    });
    // The calling card (emblem background) he has chosen: the default's.
    host.bind("Engine", "GetEmblemBackgroundId", |_, _| Ok(vec![Value::Num(1.0)]));
    host.bind("UIExpression", "EmblemBackgroundMaterial", |_, _| Ok(vec![Value::str("emblem_bg_default")]));

    // TableLookup(controller, table, column, value, [column, value, ...]
    // return column): the first row matching every pair (the front end
    // looks game types up by two keys: 0 = base, then the ref).
    let v = host.values.clone();
    host.bind("UIExpression", "TableLookup", move |_, a| {
        let vb = v.borrow();
        let keys: Vec<(usize, String)> = a
            .get(2..a.len().saturating_sub(1))
            .unwrap_or_default()
            .chunks(2)
            .filter(|p| p.len() == 2)
            .map(|p| (p[0].as_num().unwrap_or(0.0).max(0.0) as usize, p[1].to_string()))
            .collect();
        let ret = a.last().and_then(Value::as_num).unwrap_or(0.0).max(0.0) as usize;
        let hit = table(&vb, &arg(&a, 1).to_string())
            .and_then(|rows| rows.iter().find(|r| keys.iter().all(|(c, x)| r.get(*c).is_some_and(|y| y == x))))
            .and_then(|r| r.get(ret).cloned())
            .unwrap_or_default();
        Ok(vec![Value::str(&hit)])
    });
    // String tables by row (row 0 = the table's first row).
    let v = host.values.clone();
    host.bind("Engine", "GetTableRowCount", move |_, a| {
        let n = table(&v.borrow(), &arg(&a, 0).to_string()).map_or(0, Vec::len);
        Ok(vec![Value::Num(n as f32)])
    });
    let v = host.values.clone();
    host.bind("UIExpression", "TableLookupGetColumnValueForRow", move |_, a| {
        // (table, row, column), a controller first at some sites.
        let at = usize::from(a.len() >= 4);
        let row = num(&a, at + 1).max(0) as usize;
        let col = num(&a, at + 2).max(0) as usize;
        let vb = v.borrow();
        let hit = table(&vb, &arg(&a, at).to_string()).and_then(|t| t.get(row)).and_then(|r| r.get(col));
        Ok(vec![Value::str(hit.map_or("", String::as_str))])
    });
    // TableFindRows(table, column, value): the matching rows' numbers.
    let v = host.values.clone();
    host.bind("Engine", "TableFindRows", move |_, a| {
        let col = num(&a, 1).max(0) as usize;
        let want = arg(&a, 2).to_string();
        let vb = v.borrow();
        let Some(rows) = table(&vb, &arg(&a, 0).to_string()) else { return Ok(vec![Value::Nil]) };
        let t = Table::new_ref();
        let mut n = 0;
        for (i, r) in rows.iter().enumerate() {
            if r.get(col).is_some_and(|c| *c == want) {
                n += 1;
                t.borrow_mut().set(Value::Num(n as f32), Value::Num(i as f32));
            }
        }
        Ok(vec![if n == 0 { Value::Nil } else { Value::Table(t) }])
    });
    // Challenge progress lists (camos, reticles, calling cards): none
    // started on a fresh profile. (Stub: the challenge tables are not
    // read yet.)
    host.bind("Engine", "GetChallengeInfoForImages", |_, _| Ok(vec![Value::Table(Table::new_ref())]));
    // CommitProfileChanges: the owner saves what changed.
    let m = mp.clone();
    host.bind("Engine", "CommitProfileChanges", move |_, _| {
        m.borrow_mut().set_dirty();
        Ok(vec![])
    });
    // SetDvar(name, value) (a controller first at some sites) and
    // SetGametype(type): the lobby's map and game type (ui_mapname,
    // ui_gametype).
    let v = host.values.clone();
    host.bind("Engine", "SetDvar", move |_, a| {
        if a.len() >= 2 {
            let (k, x) = (arg(&a, a.len() - 2).to_string(), arg(&a, a.len() - 1).to_string());
            v.borrow_mut().dvars.insert(k, x);
        }
        Ok(vec![])
    });
    let v = host.values.clone();
    host.bind("Engine", "SetGametype", move |_, a| {
        let g = arg(&a, a.len().saturating_sub(1)).to_string();
        {
            let mut v = v.borrow_mut();
            v.dvars.insert("ui_gametype".to_owned(), g.clone());
            v.dvars.insert("ui_gameType".to_owned(), g.clone());
        }
        // The game type's own settings, as the engine loads them with it:
        // every type's defaults, then its own (TDM: 10 minutes, 75 points).
        set_gametype_defaults(&v, &g);
        Ok(vec![])
    });
    // START MATCH (PartyHostToggleStart): the owner starts it; cancelled
    // by PartyHostCancelStartMatch.
    let m = mp.clone();
    host.bind("Engine", "PartyHostToggleStart", move |_, _| {
        let mut m = m.borrow_mut();
        m.start_requested = !m.start_requested;
        Ok(vec![])
    });
    // Ready to start = the start is under way (the lobby greys START
    // MATCH out meanwhile).
    for name in ["PartyHostIsReadyToStart", "PartyIsReadyToStart"] {
        let m = mp.clone();
        host.bind("Engine", name, move |_, _| Ok(vec![Value::Bool(m.borrow().start_requested)]));
    }
    let m = mp.clone();
    host.bind("Engine", "PartyHostCancelStartMatch", move |_, _| {
        m.borrow_mut().start_requested = false;
        Ok(vec![])
    });
    // Pick 10: ten points per class.
    host.bind("Engine", "GetMaxAllocation", |_, _| Ok(vec![Value::Num(10.0)]));
    // Five custom classes before prestige classes unlock.
    host.bind("Engine", "GetCustomClassCount", |_, _| Ok(vec![Value::Num(5.0)]));

    // Game modes (CoD.GAMEMODE_*): GameModeSetMode(mode, on).
    let m = mp.clone();
    host.bind("Engine", "GameModeSetMode", move |_, a| {
        let mode = num(&a, 0);
        if arg(&a, 1).truthy() {
            m.borrow_mut().game_modes.insert(mode);
        } else {
            m.borrow_mut().game_modes.remove(&mode);
        }
        Ok(vec![])
    });
    let m = mp.clone();
    host.bind("Engine", "GameModeIsMode", move |_, a| Ok(vec![Value::Bool(m.borrow().game_modes.contains(&num(&a, 0)))]));
    let m = mp.clone();
    host.bind("Engine", "GameModeResetModes", move |_, _| {
        m.borrow_mut().game_modes.clear();
        Ok(vec![])
    });
    // Session modes (CoD.SESSIONMODE_*: 0 offline, 1 system link, 2
    // online, 3 private, 4 zombies), kept in the host's `session_modes`.
    for (name, mode) in [
        ("SessionModeSetOffline", 0.0f32),
        ("SessionModeSetSystemlink", 1.0),
        ("SessionModeSetOnlineGame", 2.0),
        ("SessionModeSetPrivate", 3.0),
        ("SessionModeSetZombiesGame", 4.0),
    ] {
        let v = host.values.clone();
        host.bind("Engine", name, move |_, a| {
            let mut v = v.borrow_mut();
            v.session_modes.retain(|m| *m != mode);
            if arg(&a, 0).truthy() {
                v.session_modes.push(mode);
            }
            Ok(vec![])
        });
    }
    // The session's kind as the lobbies ask it: system link (1), and a
    // public online game (online, not private): Combat Training's lobby.
    // (A system link game hides the lobby rows' ranks.)
    let v = host.values.clone();
    host.bind("UIExpression", "SessionMode_IsSystemlinkGame", move |_, _| {
        Ok(vec![Value::Num(if v.borrow().session_modes.contains(&1.0) { 1.0 } else { 0.0 })])
    });
    let v = host.values.clone();
    host.bind("UIExpression", "SessionMode_IsPublicOnlineGame", move |_, _| {
        let m = &v.borrow().session_modes;
        Ok(vec![Value::Num(if m.contains(&2.0) && !m.contains(&3.0) && !m.contains(&1.0) { 1.0 } else { 0.0 })])
    });
    let v = host.values.clone();
    host.bind("Engine", "SessionModeResetModes", move |_, _| {
        v.borrow_mut().session_modes.clear();
        Ok(vec![])
    });
    // SetGametypeSetting(name, value): the private match's settings.
    let v = host.values.clone();
    host.bind("Engine", "SetGametypeSetting", move |_, a| {
        let value = arg(&a, 1).as_num().unwrap_or(0.0);
        v.borrow_mut().settings.insert(arg(&a, 0).to_string(), value);
        Ok(vec![])
    });

    // His stats.
    let st = state.clone();
    let m = mp.clone();
    host.bind("Engine", "GetPlayerStats", move |vm, a| {
        st(&m);
        // (controller, location): CoD.STATS_LOCATION_STABLE (3) = before
        // his last ranked match.
        let stable = arg(&a, 1).as_num() == Some(3.0);
        Ok(vec![node_at(vm, &m, String::new(), stable)])
    });
    // GetDStat(controller, part, part, ...): the value at that path.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "GetDStat", move |_, a| {
        st(&m);
        let path: Vec<String> = a.iter().skip(1).map(part).collect();
        Ok(vec![m.borrow().stat(&path.join("."))])
    });
    // GetStatByName(controller, name): PlayerStatsList.<name>.StatValue.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "GetStatByName", move |_, a| {
        st(&m);
        let path = format!("playerstatslist.{}.statvalue", part(&arg(&a, 1)));
        Ok(vec![m.borrow().stat(&path)])
    });

    // Classes: GetClassItem(controller, class, slot) / SetClassItem(
    // controller, class, slot, value) in the set the menus edit.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetClassItem", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let (class, slot) = (num(&a, 1), arg(&a, 2).to_string());
        // A grenade's `<slot>status<n>` (each one carried: Create-a-Class
        // writes them beside `<slot>count`): a class set by BO2's defaults
        // (count only) carries the first `count` of them.
        if let Some((base, n)) = slot.rsplit_once("status").and_then(|(b, n)| Some((b, n.parse::<i32>().ok()?)))
            && base.ends_with("grenade")
            && m.profile.stat(&m.class_key(class, &slot)).is_none()
        {
            let count = m.stat(&m.class_key(class, &format!("{base}count"))).as_num().unwrap_or(0.0) as i32;
            let item = m.stat(&m.class_key(class, base)).as_num().unwrap_or(0.0) as i32;
            return Ok(vec![Value::Num(if item > 0 && n <= count { 1.0 } else { 0.0 })]);
        }
        // Scorestreaks he has not picked yet are the defaults ("all" classes
        // carry them), as the match gives them.
        let lower = slot.to_ascii_lowercase();
        if lower.starts_with("killstreak")
            && m.profile.stat(&m.class_key(class, &lower)).is_none()
            && !(1..=3).any(|n| m.profile.stat(&m.class_key(class, &format!("killstreak{n}"))).is_some())
            && let Some((_, item)) = m.default_class("all").into_iter().find(|(s, _)| *s == lower)
        {
            return Ok(vec![Value::Num(item as f32)]);
        }
        Ok(vec![m.stat(&m.class_key(class, &slot))])
    });
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "SetClassItem", move |_, a| {
        let m = st(&m);
        let mut m = m.borrow_mut();
        let key = m.class_key(num(&a, 1), &arg(&a, 2).to_string());
        m.set_stat(key, &arg(&a, 3));
        Ok(vec![])
    });
    // GetCustomClass(controller, class): its slots by name and the points
    // spent.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetCustomClass", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let class = num(&a, 1);
        let t = Table::new_ref();
        let mut spent = 0;
        for slot in SLOTS {
            let v = m.stat(&m.class_key(class, slot));
            let n = v.as_num().map_or(0, |n| n as i32);
            if !slot.ends_with("count") && !slot.ends_with("camo") && !slot.ends_with("reticle") && n > 0 {
                // (Each grenade carried is a point: two concussions, two.)
                let each = if slot.ends_with("grenade") {
                    m.stat(&m.class_key(class, &format!("{slot}count"))).as_num().map_or(1, |c| c as i32)
                } else {
                    1
                };
                spent += if slot.contains("attachment") { 1 } else { m.item(n).map_or(0, |it| it.cost) * each };
            }
            // An empty item slot (weapon, grenade, perk, wildcard) is nil,
            // as the scripts test it (`class.specialty4 == nil`: no second
            // perk unless Perk Greed); an attachment's 0 is "none".
            let item_slot = !slot.contains("attachment") && !slot.ends_with("count") && !slot.ends_with("camo") && !slot.ends_with("reticle");
            if item_slot && n <= 0 {
                continue;
            }
            t.borrow_mut().set_str(slot, v);
        }
        t.borrow_mut().set_str("allocationSpent", Value::Num(spent as f32));
        Ok(vec![Value::Table(t)])
    });
    // GetDefaultClassSlot(controller, class, slot): a default class's item.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "GetDefaultClassSlot", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let slot = arg(&a, 2).to_string().to_ascii_lowercase();
        let hit = m.default_class(&arg(&a, 1).to_string()).into_iter().find(|(s, _)| *s == slot);
        Ok(vec![Value::Num(hit.map_or(0, |h| h.1) as f32)])
    });

    // Items (mp/statstable.csv): UIExpression.<fn>(controller, index).
    type Field = fn(&Item) -> Value;
    let fields: [(&'static str, Field); 7] = [
        ("GetItemName", |it| Value::str(&it.name)),
        ("GetItemImage", |it| Value::str(&it.image)),
        ("GetItemDesc", |it| Value::str(&it.desc)),
        ("GetItemRef", |it| Value::str(&it.reference)),
        ("GetItemGroup", |it| Value::str(&it.group)),
        ("GetItemAllocationCost", |it| Value::Num(it.cost as f32)),
        ("GetItemMomentumCost", |it| Value::Num(it.momentum as f32)),
    ];
    for (name, f) in fields {
        let (st, m) = (state.clone(), mp.clone());
        host.bind("UIExpression", name, move |_, a| {
            let m = st(&m);
            let m = m.borrow();
            // (controller, index); a few sites pass the index alone.
            let i = if a.len() >= 2 { num(&a, 1) } else { num(&a, 0) };
            Ok(vec![m.item(i).map_or(Value::Nil, f)])
        });
    }
    // GetItemIndex(controller, reference).
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "GetItemIndex", move |_, a| {
        let m = st(&m);
        let r = arg(&a, 1).to_string();
        let i = m.borrow().items().iter().find(|it| it.reference == r || (!it.name.is_empty() && it.name == r)).map_or(0, |it| it.index);
        Ok(vec![Value::Num(i as f32)])
    });
    // GetLoadoutSlotForItem(index): its slot name.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetLoadoutSlotForItem", move |_, a| {
        let m = st(&m);
        Ok(vec![m.borrow().item(num(&a, 0)).map_or(Value::Nil, |it| Value::str(&it.slot))])
    });
    // IsItemLocked(controller, index): his rank has not reached it;
    // IsItemPurchased: it is his (free with its rank, or bought with a
    // token). A private match frees everything.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "IsItemLocked", move |_, a| {
        let m = st(&m);
        Ok(vec![Value::Num(if m.borrow().locked(num(&a, 1)) { 1.0 } else { 0.0 })])
    });
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "IsItemPurchased", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let i = num(&a, 1);
        Ok(vec![Value::Num(if !m.locked(i) && m.profile.purchased(i, m.all_free()) { 1.0 } else { 0.0 })])
    });
    // PurchaseItem(controller, index): one unlock token for it (BO2's
    // UIExpression.GetItemCost is 1 for every item).
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "PurchaseItem", move |_, a| {
        let m = st(&m);
        let i = num(&a, 1);
        m.borrow_mut().profile.purchase(i, 1);
        Ok(vec![])
    });
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "ItemIndexValid", move |_, a| Ok(vec![Value::Bool(st(&m).borrow().item(num(&a, 0)).is_some())]));
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "AreAllItemsFree", move |_, _| Ok(vec![Value::Bool(st(&m).borrow().all_free())]));
    host.bind("UIExpression", "GetItemCost", one);

    // Attachments: Engine.<fn>(weapon index, attachment number).
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetNumAttachments", move |_, a| {
        let m = st(&m);
        let n = m.borrow().item(num(&a, 0)).map_or(0, |it| it.attachments.len() + 1);
        Ok(vec![Value::Num(n as f32)])
    });
    // GetUnlockablesByGroupName(group): the item indices of a statstable
    // group that go in a class slot (scorestreaks "killstreak", wildcards
    // "bonuscard", "weapon_smg", ...); the scripts sort them by
    // GetItemSortKey (column 9, the order within the group). A Pick-10 cost
    // (column 12) of -1 marks an item no class can hold (War Machine,
    // minigun, dual-wield pistols): not listed.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetUnlockablesByGroupName", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let group = arg(&a, 0).to_string();
        let t = Table::new_ref();
        let mut n = 0;
        for it in m.items().iter().filter(|it| it.cost >= 0 && it.group == group && !it.slot.is_empty()) {
            n += 1;
            t.borrow_mut().set(Value::Num(n as f32), Value::Num(it.index as f32));
        }
        Ok(vec![Value::Table(t)])
    });
    // Weapon options (Personalize Weapon: camo, reticle, lens, tag,
    // emblem): mp/attachmenttable.csv's "weaponoption" rows, each by its
    // own index (column 0), in groups (column 1) numbered as CACUtility's
    // WEAPONOPTION_GROUP_*.
    fn group_name(g: i32) -> &'static str {
        ["camo", "tag", "emblem", "reticle", "lens", "reticle_color"].get(g.max(0) as usize).copied().unwrap_or("")
    }
    fn options(vb: &EngineValues) -> Vec<Vec<String>> {
        table(vb, "mp/attachmenttable.csv")
            .into_iter()
            .flatten()
            .filter(|r| r.get(2).is_some_and(|c| c.trim() == "weaponoption"))
            .cloned()
            .collect()
    }
    fn option(vb: &EngineValues, index: i32) -> Option<Vec<String>> {
        options(vb).into_iter().find(|r| int(r, 0) == index)
    }
    // GetNumWeaponOptions(group): how many the group has.
    let v = host.values.clone();
    host.bind("Engine", "GetNumWeaponOptions", move |_, a| {
        let g = group_name(num(&a, 0));
        let n = options(&v.borrow()).iter().filter(|r| cell(r, 1) == g).count();
        Ok(vec![Value::Num(n as f32)])
    });
    // GetWeaponOptionGroupIndex(controller, n, group): the group's n-th
    // option (from 0) as its own index.
    let v = host.values.clone();
    host.bind("UIExpression", "GetWeaponOptionGroupIndex", move |_, a| {
        let (n, g) = (num(&a, 1), group_name(num(&a, 2)));
        let hit = options(&v.borrow()).into_iter().filter(|r| cell(r, 1) == g).nth(n.max(0) as usize).map_or(0, |r| int(&r, 0));
        Ok(vec![Value::Num(hit as f32)])
    });
    // GetWeaponOptionImage / GetWeaponOptionName(controller, option): its
    // picture (column 6) and its name's text key (column 3).
    for (name, col) in [("GetWeaponOptionImage", 6usize), ("GetWeaponOptionName", 3)] {
        let v = host.values.clone();
        host.bind("UIExpression", name, move |_, a| {
            let hit = option(&v.borrow(), num(&a, 1)).map(|r| cell(&r, col)).unwrap_or_default();
            Ok(vec![Value::str(&hit)])
        });
    }
    host.bind("Engine", "GetWeaponOptionUnlockPLevel", zero);
    // A camo's challenge (mp/statsmilestones1-4.csv: column 3 the weapon's
    // group, 9 the camo it gives): GetChallengeForItemOption(weapon,
    // option) = (row, table number from 0); GetItemOptionChallengeValue
    // (controller, weapon, option) = his count on its stat (column 4,
    // itemstats.<weapon>.stats.<stat>.statvalue); GetItemOptionLocked
    // (controller, weapon, option) = a camo whose challenge he has not
    // finished (column 2 its target). Every other option is his.
    let challenge = {
        let (st, m, v) = (state.clone(), mp.clone(), host.values.clone());
        move |weapon: i32, opt: i32| -> Option<(usize, usize, String, i32, i32)> {
            let m = st(&m);
            let m = m.borrow();
            let group = m.item(weapon)?.group.clone();
            let vb = v.borrow();
            let reference = option(&vb, opt).map(|r| cell(&r, 4))?;
            if reference.is_empty() || reference == "camo_none" {
                return None;
            }
            (0..4).find_map(|t| {
                let rows = table(&vb, &format!("mp/statsmilestones{}.csv", t + 1))?;
                let (row, r) = rows.iter().enumerate().find(|(_, r)| cell(r, 3) == group && cell(r, 9) == reference)?;
                let stat = cell(r, 4);
                let have = m.stat(&format!("itemstats.{weapon}.stats.{stat}.statvalue")).as_num().unwrap_or(0.0) as i32;
                Some((row, t, stat, int(r, 2), have))
            })
        }
    };
    let c = challenge.clone();
    host.bind("Engine", "GetChallengeForItemOption", move |_, a| {
        Ok(match c(num(&a, 0), num(&a, 1)) {
            Some((row, t, ..)) => vec![Value::Num(row as f32), Value::Num(t as f32)],
            None => vec![Value::Nil, Value::Nil],
        })
    });
    let c = challenge.clone();
    host.bind("Engine", "GetItemOptionChallengeValue", move |_, a| {
        Ok(vec![Value::Num(c(num(&a, 1), num(&a, 2)).map_or(0, |h| h.4) as f32)])
    });
    let c = challenge;
    host.bind("Engine", "GetItemOptionLocked", move |_, a| {
        let locked = c(num(&a, 1), num(&a, 2)).is_some_and(|(.., target, have)| have < target);
        Ok(vec![Value::Bool(locked)])
    });
    // A weapon's n-th attachment (Create-a-Class's attachment grid):
    // GetItemAttachment(weapon, n) = its attachment table index;
    // GetItemAttachmentRank / Reward(weapon, n) = the weapon level that
    // unlocks it and its xp (mp/gunlevels.csv: rank r, from 0, is weapon
    // level r + 2; column 4 the xp); GetAttachmentAttachPoint(weapon, n) =
    // where it goes (attachment table column 1: top, trigger, ...).
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetItemAttachment", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        Ok(vec![m.attachment(num(&a, 0), num(&a, 1)).map_or(Value::Nil, |at| Value::Num(at.index as f32))])
    });
    for (name, rank) in [("GetItemAttachmentRank", true), ("GetItemAttachmentReward", false)] {
        let (st, m, v) = (state.clone(), mp.clone(), host.values.clone());
        host.bind("Engine", name, move |_, a| {
            let m = st(&m);
            let m = m.borrow();
            let (w, n) = (num(&a, 0), num(&a, 1));
            let Some(weapon) = m.item(w).map(|it| it.reference.clone()) else { return Ok(vec![Value::Nil]) };
            let Some(att) = m.item(w).and_then(|it| it.attachments.get((n - 1).max(0) as usize).cloned()) else {
                return Ok(vec![Value::Nil]);
            };
            let vb = v.borrow();
            let row = table(&vb, "mp/gunlevels.csv").into_iter().flatten().find(|r| cell(r, 2) == weapon && (cell(r, 3) == att || att.starts_with(&format!("{}_", cell(r, 3)))));
            Ok(vec![row.map_or(Value::Nil, |r| Value::Num(if rank { int(r, 0) + 2 } else { int(r, 4) } as f32))])
        });
    }
    // GetItemAttachmentLocked(controller, weapon, n): 0 once the weapon has
    // the xp its gun level asks for (mp/gunlevels.csv column 1 on the
    // attachment's row, against itemstats.<weapon>.xp), else 1. An
    // attachment with no gun-level row is never locked. The grid's script
    // compares the answer with the number 0.
    let (st, m, v) = (state.clone(), mp.clone(), host.values.clone());
    host.bind("Engine", "GetItemAttachmentLocked", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let (w, n) = (num(&a, 1), num(&a, 2));
        let Some(weapon) = m.item(w).map(|it| it.reference.clone()) else { return Ok(vec![Value::Num(0.0)]) };
        let Some(att) = m.item(w).and_then(|it| it.attachments.get((n - 1).max(0) as usize).cloned()) else {
            return Ok(vec![Value::Num(0.0)]);
        };
        let xp = m.stat(&format!("itemstats.{w}.xp")).as_num().unwrap_or(0.0) as i32;
        let vb = v.borrow();
        let row = table(&vb, "mp/gunlevels.csv").into_iter().flatten().find(|r| cell(r, 2) == weapon && (cell(r, 3) == att || att.starts_with(&format!("{}_", cell(r, 3)))));
        Ok(vec![Value::Num(row.is_some_and(|r| int(r, 1) > xp) as i32 as f32)])
    });
    let (st, m, v) = (state.clone(), mp.clone(), host.values.clone());
    host.bind("Engine", "GetAttachmentAttachPoint", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let reference = m.attachment(num(&a, 0), num(&a, 1)).map(|at| at.reference.clone()).unwrap_or_default();
        let vb = v.borrow();
        let point = table(&vb, "mp/attachmenttable.csv")
            .into_iter()
            .flatten()
            .find(|r| cell(r, 2) == "attachment" && cell(r, 4) == reference)
            .map(|r| cell(r, 1))
            .unwrap_or_default();
        Ok(vec![Value::str(&point)])
    });
    host.bind("Engine", "IsAttachmentNew", |_, _| Ok(vec![Value::Bool(false)]));
    host.bind("Engine", "GetPreReqChallengeValue", zero);
    // Class sets (CHOOSE CLASS's "CLASS SET 1" header, its 10 pips and
    // Class Set Options; his real game shows them): the classes he edits
    // are always the class set root's (cacloadouts, ...); the others wait
    // in classsets.<root>.<n>.<path>, swapped in when he picks their set.
    // A set never used starts as a copy of the one he is on.
    const CLASS_SETS: i32 = 10;
    fn set_paths(m: &MpState) -> Vec<String> {
        let root = m.class_set().to_owned();
        m.profile
            .stats
            .keys()
            .filter(|k| k.starts_with(&format!("{root}.customclass")))
            .cloned()
            .collect()
    }
    fn current_set(m: &MpState) -> i32 {
        m.profile.stat_num(&format!("classsets.{}.current", m.class_set())) as i32
    }
    host.bind("Engine", "GetNumberOfClassSetsOwned", |_, _| Ok(vec![Value::Num(CLASS_SETS as f32)]));
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetCurrentClassSetIndex", move |_, _| Ok(vec![Value::Num(current_set(&st(&m).borrow()) as f32)]));
    // SetActiveClassSet(controller, n): put the classes away, bring set n's.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "SetActiveClassSet", move |_, a| {
        let m = st(&m);
        let mut m = m.borrow_mut();
        let to = num(&a, 1).clamp(0, CLASS_SETS - 1);
        let from = current_set(&m);
        if to == from {
            return Ok(vec![]);
        }
        let root = m.class_set().to_owned();
        let store = |n: i32, path: &str| format!("classsets.{root}.{n}.{}", &path[root.len() + 1..]);
        for path in set_paths(&m) {
            if let Some(v) = m.profile.stat(&path).cloned() {
                m.profile.set_stat(store(from, &path), v);
            }
        }
        let prefix = format!("classsets.{root}.{to}.");
        let saved: Vec<(String, StatValue)> = m
            .profile
            .stats
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix) && k[prefix.len()..].starts_with("customclass"))
            .map(|(k, v)| (format!("{root}.{}", &k[prefix.len()..]), v.clone()))
            .collect();
        if !saved.is_empty() {
            for path in set_paths(&m) {
                m.profile.stats.remove(&path);
            }
            for (k, v) in saved {
                m.profile.set_stat(k, v);
            }
        }
        m.profile.set_stat(format!("classsets.{root}.current"), StatValue::Num(to as f32));
        Ok(vec![])
    });
    // GetClassSetName / SetClassSetName(controller, n[, name]).
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetClassSetName", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let n = num(&a, 1);
        let name = match m.profile.stat(&format!("classsets.{}.{n}.name", m.class_set())) {
            Some(StatValue::Str(s)) if !s.is_empty() => s.clone(),
            _ => format!("CLASS SET {}", n + 1),
        };
        Ok(vec![Value::str(&name)])
    });
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "SetClassSetName", move |_, a| {
        let m = st(&m);
        let mut m = m.borrow_mut();
        let key = format!("classsets.{}.{}.name", m.class_set(), num(&a, 1));
        m.profile.set_stat(key, StatValue::Str(arg(&a, 2).to_string()));
        Ok(vec![])
    });
    // GetUnlockablesBySlotName(slot): the item indices that go in that
    // class slot (statstable column 13: "specialty1", "primarygrenade"):
    // Create-a-Class's perk and grenade pickers. Column 12 (the Pick-10
    // cost) of -1 marks an item no class can hold (the War Machine, the
    // minigun, the dual-wield pistols, the held knife), so the pickers do
    // not list it.
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetUnlockablesBySlotName", move |_, a| {
        let m = st(&m);
        let m = m.borrow();
        let slot = arg(&a, 0).to_string();
        let t = Table::new_ref();
        for (n, it) in m.items().iter().filter(|it| it.cost >= 0 && it.slot.eq_ignore_ascii_case(&slot)).enumerate() {
            t.borrow_mut().set(Value::Num((n + 1) as f32), Value::Num(it.index as f32));
        }
        Ok(vec![Value::Table(t)])
    });
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "GetItemUnlockLevel", move |_, a| {
        let m = st(&m);
        Ok(vec![Value::Num(m.borrow().item(num(&a, 1)).map_or(0, |it| it.unlock_rank) as f32)])
    });
    // GetRankName(rank): mp/ranktable.csv's full rank name text key (column 5: "Sergeant", as the unlock hints show it).
    let v = host.values.clone();
    host.bind("Engine", "GetRankName", move |_, a| {
        let want = num(&a, 0).to_string();
        let vb = v.borrow();
        let hit = table(&vb, "mp/ranktable.csv").and_then(|rows| rows.iter().find(|r| r.first() == Some(&want))).map(|r| cell(r, 5));
        Ok(vec![Value::str(&hit.unwrap_or_default())])
    });
    let (st, m) = (state.clone(), mp.clone());
    host.bind("Engine", "GetItemSortKey", move |_, a| {
        let m = st(&m);
        Ok(vec![Value::Num(m.borrow().item(num(&a, 0)).map_or(0, |it| it.sort) as f32)])
    });
    // UIExpression.GetAttachmentName / Image(controller, n): the n-th row
    // of mp/attachmenttable.csv ("" past the end ends the scripts' loops).
    for (name, f) in [("GetAttachmentName", 1usize), ("GetAttachmentImage", 2)] {
        let (st, m) = (state.clone(), mp.clone());
        host.bind("UIExpression", name, move |_, a| {
            let m = st(&m);
            let m = m.borrow();
            let row = usize::try_from(num(&a, 1)).ok().and_then(|i| m.profile.attachments.get(i));
            Ok(vec![Value::str(row.map_or("", |r| if f == 1 { r.name.as_str() } else { r.image.as_str() }))])
        });
    }
    // GetNumItemAttachmentsWithAttachPoint(controller, weapon, point): his
    // scripts only pass point 0 (every point) and take 1 off for "none".
    let (st, m) = (state.clone(), mp.clone());
    host.bind("UIExpression", "GetNumItemAttachmentsWithAttachPoint", move |_, a| {
        let m = st(&m);
        let n = m.borrow().item(num(&a, 1)).map_or(1, |it| it.attachments.len() + 1);
        Ok(vec![Value::Num(n as f32)])
    });
    type AttField = fn(&Attachment) -> Value;
    let att_fields: [(&'static str, AttField); 5] = [
        ("GetAttachmentRef", |a| Value::str(&a.reference)),
        ("GetAttachmentName", |a| Value::str(&a.name)),
        ("GetAttachmentImage", |a| Value::str(&a.image)),
        ("GetAttachmentDesc", |a| Value::str(&a.desc)),
        ("GetAttachmentAllocationCost", |a| Value::Num(a.cost as f32)),
    ];
    for (name, f) in att_fields {
        let (st, m) = (state.clone(), mp.clone());
        host.bind("Engine", name, move |_, a| {
            let m = st(&m);
            let m = m.borrow();
            Ok(vec![m.attachment(num(&a, 0), num(&a, 1)).map_or(Value::Nil, f)])
        });
    }
    crate::playlists::install(host, &mp);
    crate::aar::install(host, &mp);
    mp
}
