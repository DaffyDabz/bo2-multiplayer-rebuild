//! bo2mp lane C: Combat Training, BO2's ranked bot matches (PUBLIC MATCH
//! -> FIND MATCH -> the playlist popup's COMBAT TRAINING category -> a
//! playlist -> the public game lobby, its countdown, the match).
//!
//! Treyarch's playlist file (`playlists.info`, sent by its servers) is
//! not on his disk, so the playlist data here is OURS:
//! - the categories: Combat Training by BO2's bot difficulties, RECRUIT,
//!   REGULAR, HARDENED and VETERAN (BO2's names for them), each described
//!   with BO2's Combat Training text and each holding the same playlists;
//!   the public lobby is BO2's COMBAT TRAINING lobby. BO2's popup needs two
//!   or more categories: with one it goes straight to that category's
//!   playlists but leaves their list's input switched off
//!   (playlistselectionpopup.lua), so a lone category can't be picked; and
//!   its category column is narrow (a longer name runs under the
//!   playlists),
//! - their ids (1-4), the playlists' numbers (difficulty * 4 + 1..4) and
//!   their order,
//! - which game types each offers: Team Deathmatch, Domination, Kill
//!   Confirmed and Free-for-All (BO2's own basic-training rows in
//!   mp/gametypesTable.csv are TDM, hardcore TDM, FFA and hardcore FFA),
//! - the teams: him plus 5 bots against 6 (BO2's 6 v 6), Free-for-All him
//!   against 7,
//! - the map: Nuketown 2025 (the map this build has),
//! - the players-in-playlist counts (0: no online service) and the
//!   lobby's countdowns (10 s; 30 s before the next match).
//!
//! BO2's own: the names, descriptions and icons (mp/gametypesTable.csv:
//! MPUI_*_CAPS, MENU_* descriptions, playlist_* pictures), the category's
//! name and description (MPUI_BASICTRAINING_CAPS / _DESC), the lobby's
//! status text (MP_MATCH_STARTING_IN), the bot difficulty setting
//! (bot_difficulty, RECRUIT .. VETERAN) and every menu around them.

use crate::host::Host;
use crate::mp::Mp;
use crate::value::{Table, Value};

/// One Combat Training playlist (ours, see the module's notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playlist {
    pub index: i32,
    /// Its mp/gametypesTable.csv game type (the base rows, `0,<ref>,...`).
    pub gametype: &'static str,
    /// Bots on his team and against him.
    pub friends: u32,
    pub enemies: u32,
    /// BO2's bot difficulty (`bot_difficulty`: 0 RECRUIT .. 3 VETERAN).
    pub difficulty: u32,
}

/// Each category's game types and teams.
const MODES: [(&str, u32, u32); 4] = [("tdm", 5, 6), ("dom", 5, 6), ("conf", 5, 6), ("dm", 0, 7)];

/// BO2's bot difficulty names (gameoptions.lua's COMBAT TRAINING bot
/// difficulty selector), easiest first.
pub const DIFFICULTIES: [&str; 4] = [
    "MENU_BASICTRAINING_EASY_CAPS",
    "MENU_BASICTRAINING_NORMAL_CAPS",
    "MENU_BASICTRAINING_HARD_CAPS",
    "MENU_BASICTRAINING_FU_CAPS",
];

/// The popup's categories, top first: REGULAR first, as BO2 starts its
/// bots there (default_private.cfg, "for a private game (privatematch,
/// combat training, theater)": bot_difficulty 1) and the popup opens on
/// its first category; then the rest easiest first.
const CATEGORY_ORDER: [u32; 4] = [1, 0, 2, 3];

/// Every playlist, category by category.
pub fn all() -> impl Iterator<Item = Playlist> {
    (0..DIFFICULTIES.len() as u32).flat_map(|d| {
        MODES.iter().enumerate().map(move |(n, &(gametype, friends, enemies))| Playlist {
            index: (d * MODES.len() as u32) as i32 + n as i32 + 1,
            gametype,
            friends,
            enemies,
            difficulty: d,
        })
    })
}

/// The map every playlist plays (ours: the one this build has).
pub const MAP: &str = "mp_nuketown_2020";

/// The public game lobby's countdown before the match (ours), and before
/// the next one when he comes back from a match (time for the After Action
/// Report, as BO2's lobby gives).
pub const COUNTDOWN_MS: f64 = 10_000.0;
pub const NEXT_MATCH_MS: f64 = 30_000.0;

pub fn by_index(index: i32) -> Option<Playlist> {
    all().find(|p| p.index == index)
}

/// A category's name: its bot difficulty ("RECRUIT").
fn category_name(text: &dyn Fn(&str) -> String, difficulty: u32) -> String {
    text(DIFFICULTIES[difficulty as usize])
}

/// Bind the playlist answers: Engine.GetPlaylistCategories (the popup's
/// categories and playlists), SetPlaylistID / GetPlaylistID, the names
/// the lobbies show, and GetGameLobbyStatus (the countdown).
pub fn install(host: &mut Host, mp: &Mp) {
    let v = host.values.clone();
    host.bind("Engine", "GetPlaylistCategories", move |_, _| {
        let vb = v.borrow();
        let text = |k: &str| vb.localize.get(&k.to_ascii_uppercase()).cloned().unwrap_or_else(|| k.to_owned());
        let rows = crate::mp::table_rows(&vb, "mp/gametypestable.csv");
        let cats = Table::new_ref();
        for (slot, d) in CATEGORY_ORDER.into_iter().enumerate() {
            let lists = Table::new_ref();
            for (n, p) in all().filter(|p| p.difficulty == d).enumerate() {
                let row = rows
                    .iter()
                    .find(|r| r.first().map(String::as_str) == Some("0") && r.get(1).map(String::as_str) == Some(p.gametype));
                let cell = |i: usize| row.and_then(|r| r.get(i)).cloned().unwrap_or_default();
                let e = Table::new_ref();
                {
                    let mut e = e.borrow_mut();
                    e.set_str("index", Value::Num(p.index as f32));
                    e.set_str("id", Value::Num(p.index as f32));
                    e.set_str("name", Value::str(&text(&cell(2))));
                    e.set_str("description", Value::str(&text(&cell(3))));
                    e.set_str("icon", Value::str(&cell(4)));
                    e.set_str("playerCount", Value::Num(0.0));
                    e.set_str("locked", Value::Bool(false));
                    e.set_str("filter", Value::str("playermatch"));
                }
                lists.borrow_mut().set(Value::Num((n + 1) as f32), Value::Table(e));
            }
            let cat = Table::new_ref();
            {
                let mut c = cat.borrow_mut();
                c.set_str("id", Value::Num((d + 1) as f32));
                c.set_str("name", Value::str(&category_name(&text, d)));
                c.set_str("description", Value::str(&text("MPUI_BASICTRAINING_DESC")));
                c.set_str("filter", Value::str("playermatch"));
                c.set_str("playerCount", Value::Num(0.0));
                c.set_str("locked", Value::Bool(false));
                c.set_str("playlists", Value::Table(lists));
            }
            cats.borrow_mut().set(Value::Num((slot + 1) as f32), Value::Table(cat));
        }
        Ok(vec![Value::Table(cats)])
    });
    let m = mp.clone();
    host.bind("Engine", "SetPlaylistID", move |_, a| {
        let id = a.iter().rev().find_map(Value::as_num).map(|n| n as i32);
        m.borrow_mut().playlist = id.and_then(by_index).map(|p| p.index);
        Ok(vec![])
    });
    let m = mp.clone();
    host.bind("Engine", "GetPlaylistID", move |_, _| Ok(vec![Value::Num(m.borrow().playlist.unwrap_or(0) as f32)]));
    host.bind("Engine", "IsPlaylistLocked", |_, _| Ok(vec![Value::Bool(false)]));
    // The lobby's names for the playlist and its category.
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetPlaylistName", move |_, _| {
        let vb = v.borrow();
        let g = m.borrow().playlist.and_then(by_index).map_or("tdm", |p| p.gametype);
        let rows = crate::mp::table_rows(&vb, "mp/gametypestable.csv");
        let key = rows
            .iter()
            .find(|r| r.first().map(String::as_str) == Some("0") && r.get(1).map(String::as_str) == Some(g))
            .and_then(|r| r.get(2).cloned())
            .unwrap_or_default();
        Ok(vec![Value::str(vb.localize.get(&key.to_ascii_uppercase()).map_or(key.as_str(), String::as_str))])
    });
    // The public lobby's title: BO2's COMBAT TRAINING lobby.
    let v = host.values.clone();
    host.bind("Engine", "GetPlaylistCategoryName", move |_, _| {
        let vb = v.borrow();
        Ok(vec![Value::str(vb.localize.get("MPUI_BASICTRAINING_LOBBY_CAPS").map_or("COMBAT TRAINING", String::as_str))])
    });
    host.bind("Engine", "GetPlaylistCategoryFilter", |_, _| Ok(vec![Value::str("playermatch")]));
    // The lobbies ask some of these of UIExpression.
    for name in ["GetPlaylistID", "GetPlaylistName", "GetPlaylistCategoryName", "GetPlaylistCategoryFilter", "IsPlaylistLocked"] {
        let f = host.field("Engine", name);
        let t = host.vm.global("UIExpression");
        crate::host::set_field(&t, name, f);
    }
    // GetGameLobbyStatus(): the public lobby's line and the seconds left
    // ("MATCH BEGINS IN:  7"); nothing before FIND MATCH.
    let (m, v) = (mp.clone(), host.values.clone());
    host.bind("Engine", "GetGameLobbyStatus", move |_, _| {
        // A Custom Game's START MATCH: "Game starting in N" (the number
        // is in the line, no time to append).
        if let Some(at) = m.borrow().private_start_ms {
            let left = ((at - crate::lui::now_ms()) / 1000.0).ceil().max(0.0);
            let vb = v.borrow();
            let line = vb.localize.get("MENU_GAME_STARTING_IN").cloned().unwrap_or_else(|| "Game starting in &&1".to_owned());
            return Ok(vec![Value::str(&line.replace("&&1", &format!("{}", left as i64)))]);
        }
        let Some(at) = m.borrow().public_start_ms else { return Ok(vec![Value::str("")]) };
        let left = ((at - crate::lui::now_ms()) / 1000.0).ceil().max(0.0);
        let vb = v.borrow();
        let line = vb.localize.get("MP_MATCH_STARTING_IN").cloned().unwrap_or_else(|| "MATCH BEGINS IN".to_owned());
        Ok(vec![Value::str(&line), Value::Num(left as f32)])
    });
}
