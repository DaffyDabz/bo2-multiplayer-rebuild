//! bo2mp lane C: Black Ops II's multiplayer front end with no map
//! (`iw4l frontend t6mp`), run by `lui_hud` from BO2's own scripts: the
//! multiplayer answers (`hks_t6::mp`) and his stats file go in before CoD's
//! base loads, then `T6.Main`; the engine opens `main` (the background)
//! and sends it `open_menu MainMenu`. START MATCH in the private match
//! lobby (ONLINE -> CUSTOM GAMES) hands the lobby's map, game type and bot
//! settings to the match (`session::Bo2mpMatchSetup`) and asks for the
//! swap to `t6:<map>`. His class choices are saved as they change.

use bevy::prelude::*;
use hks_t6::host::Host;
use hks_t6::mp::Mp;
use hks_t6::value::Value;

/// The game booted into BO2's multiplayer front end; his stats file (the
/// shape the menus read, `bo2_profile`) beside settings.cfg.
#[derive(Resource, Clone, Debug)]
pub struct Bo2mpFrontend {
    pub stats_path: std::path::PathBuf,
}

/// Where he was when he started a match, to come back to after it as BO2
/// does: the lobby menu (Custom Games: `PrivateOnlineGameLobby`) and the
/// front end's state then (session and game modes, dvars, the game type's
/// settings from SETUP GAME).
#[derive(Resource, Clone, Debug, Default)]
pub struct LobbyReturn {
    pub menu: String,
    pub session_modes: Vec<f32>,
    pub game_modes: Vec<i32>,
    pub dvars: Vec<(String, String)>,
    pub settings: Vec<(String, f32)>,
    /// Combat Training: the playlist, and his stats before the match (the
    /// After Action Report's "before", BO2's STATS_LOCATION_STABLE).
    pub playlist: Option<i32>,
    pub stats_before: Option<String>,
}

/// The front end's own scripts, pictures and fonts, kept to come back to
/// after a match (the match's replace them).
#[derive(Resource, Clone)]
pub struct Bo2mpFrontendAssets {
    pub ui: assets::T6Ui,
    pub icons: assets::T6HudIcons,
    pub fonts: assets::T6HudFonts,
    pub front: Bo2mpFrontend,
}

/// After a match (its exitLevel swaps back to the menu screen): BO2's front
/// end again, at its main menu.
pub(crate) fn restore_after_match(
    mut commands: Commands,
    screen: Res<frame::AppScreen>,
    saved: Option<Res<Bo2mpFrontendAssets>>,
    front: Option<Res<Bo2mpFrontend>>,
    mut played: Local<bool>,
) {
    let Some(saved) = saved else { return };
    match *screen {
        frame::AppScreen::MainMenu if *played && front.is_none() => {
            *played = false;
            diag::info!(Ui, "bo2mp front end: back from the match");
            commands.insert_resource(saved.ui.clone());
            commands.insert_resource(saved.icons.clone());
            commands.insert_resource(saved.fonts.clone());
            commands.insert_resource(saved.front.clone());
            commands.remove_resource::<session::Bo2mpMatchSetup>();
        }
        frame::AppScreen::MainMenu => {}
        _ => *played = true,
    }
}

/// Before CoD's base loads: BO2's multiplayer answers, his saved stats.
pub(crate) fn install(host: &mut Host, front: &Bo2mpFrontend) -> Mp {
    let mp = hks_t6::mp::install(host);
    match std::fs::read_to_string(&front.stats_path) {
        Ok(text) => {
            mp.borrow_mut().load_text(&text);
            diag::info!(Ui, "bo2mp front end: stats from {}", front.stats_path.display());
        }
        Err(_) => diag::info!(Ui, "bo2mp front end: no stats file yet ({})", front.stats_path.display()),
    }
    mp
}

/// After CoD's base: the front end's values (no map running, his PC, an
/// offline session until ONLINE), the engine's roots (the scripts add to
/// them while they load), then `T6.Main`.
pub(crate) fn load(host: &mut Host) {
    hks_t6::mp::after_base(host);
    let offline = host.field("CoD", "SESSIONMODE_OFFLINE").as_num();
    {
        let mut v = host.values.borrow_mut();
        for (k, x) in [
            ("r_fontResolution", "720"),
            ("ui_gametype", "tdm"),
            ("ui_gameType", "tdm"),
            ("g_gametype", "tdm"),
            ("ui_mapname", "mp_la"),
            ("sv_running", "0"),
        ] {
            v.dvars.insert(k.to_owned(), x.to_owned());
        }
        v.session_modes = offline.into_iter().collect();
    }
    // The starting game type's settings (Team Deathmatch).
    hks_t6::mp::set_gametype_defaults(&host.values, "tdm");
    host.root(16.0 / 9.0);
    host.require("T6.Main");
}

/// Open the front end as the engine does: `main`, then `MainMenu`.
pub(crate) fn open(host: &mut Host, mp: Option<&Mp>, back: Option<&LobbyReturn>) {
    host.open_menu("main");
    // Back from a match: the lobby he started it from, the front end as it
    // was (BO2's engine reopens the party's lobby the same way).
    if let (Some(back), Some(mp)) = (back, mp) {
        {
            let mut v = host.values.borrow_mut();
            v.session_modes.clone_from(&back.session_modes);
            v.dvars.extend(back.dvars.iter().cloned());
            v.settings.extend(back.settings.iter().cloned());
        }
        {
            let mut m = mp.borrow_mut();
            m.game_modes.extend(back.game_modes.iter().copied());
            m.playlist = back.playlist;
            // Combat Training's lobby counts down to the next match.
            if back.playlist.is_some() {
                m.public_start_in = Some(hks_t6::playlists::NEXT_MATCH_MS);
            }
            if let Some(text) = &back.stats_before {
                m.set_stable_text(text);
            }
        }
        diag::info!(Ui, "bo2mp front end: back to `{}`", back.menu);
        host.root_event("open_menu", &[("menuName", Value::str(&back.menu))]);
        return;
    }
    host.root_event("open_menu", &[("menuName", Value::str("MainMenu"))]);
}

/// A Custom Game's START MATCH countdown (ms).
const PRIVATE_COUNTDOWN_MS: f64 = 5000.0;

/// The match is loading: BO2's own loading screen (`ui_mp/t6/hud/loading.lua`,
/// LUI.createMenu.Loading) over the front end, as the engine opens it: the
/// front end's menus go, the map and game type go into the dvars it reads
/// (`ui_mapname`, `ui_gametype`; `ls_mapname`, `ls_maplocation`,
/// `ls_gametype`: their names as mp/mapsTable.csv and
/// mp/gametypesTable.csv give them), then `Loading` opens and is told
/// `start_loading` (it fades in the map's picture `loadscreen_<map>`, its
/// name, place and game type, and a Did You Know tip).
pub(crate) fn open_loading(host: &mut Host, setup: &session::Bo2mpMatchSetup) {
    {
        let mut v = host.values.borrow_mut();
        let text = |k: &str| v.localize.get(&k.to_ascii_uppercase()).cloned().unwrap_or_default();
        let maps = hks_t6::mp::table_rows(&v, "mp/mapstable.csv");
        let map_key = maps.iter().find(|r| r.first().is_some_and(|c| *c == setup.map)).and_then(|r| r.get(3)).cloned().unwrap_or_default();
        let types = hks_t6::mp::table_rows(&v, "mp/gametypestable.csv");
        let type_key = types
            .iter()
            .find(|r| r.first().is_some_and(|c| c == "0") && r.get(1).is_some_and(|c| *c == setup.gametype))
            .and_then(|r| r.get(2))
            .cloned()
            .unwrap_or_default();
        let name = [format!("{map_key}_CAPS"), map_key.clone()].iter().map(|k| text(k)).find(|t| !t.is_empty()).unwrap_or_default();
        let place = [format!("{map_key}_LOC"), format!("{map_key}_LOCATION"), format!("{map_key}_LOC_CAPS")]
            .iter()
            .map(|k| text(k))
            .find(|t| !t.is_empty())
            .unwrap_or_default();
        let gametype = text(&type_key);
        diag::info!(Ui, "bo2mp loading: `{}` {name:?} at {place:?}, {gametype:?} (keys {map_key} {type_key})", setup.map);
        if let Ok(probe) = std::env::var("BO2MP_LOC_GREP") {
            let probe = probe.to_ascii_uppercase();
            let mut hits: Vec<_> = v.localize.iter().filter(|(k, _)| k.contains(&probe)).collect();
            hits.sort();
            for (k, t) in hits.into_iter().take(60) {
                diag::info!(Ui, "bo2mp localize {k} = {t:?}");
            }
        }
        for (k, x) in [
            ("ui_mapname", setup.map.clone()),
            ("mapname", setup.map.clone()),
            ("ui_gametype", setup.gametype.clone()),
            ("g_gametype", setup.gametype.clone()),
            ("ls_mapname", name),
            ("ls_maplocation", place),
            ("ls_gametype", gametype),
        ] {
            v.dvars.insert(k.to_owned(), x);
        }
    }
    // (T6.Main does not load it: the in-game HUD's scripts do.)
    let t = std::time::Instant::now();
    host.require("T6.HUD.Loading");
    let made = host.field("LUI", "createMenu");
    let has = matches!(host.vm.index(&made, &Value::str("Loading")), Ok(Value::Nil)).then_some("no").unwrap_or("yes");
    diag::info!(Ui, "bo2mp loading: T6.HUD.Loading required in {:.0} ms (LUI.createMenu.Loading: {has})", t.elapsed().as_secs_f64() * 1000.0);
    host.open_menu("Loading");
    diag::info!(Ui, "bo2mp loading: Loading opened ({:.0} ms)", t.elapsed().as_secs_f64() * 1000.0);
    // (The spinner is not started here: the script's own timer raises
    // `start_spinner` after `SpinnerDelayTime`, as in BO2 - a loading screen
    // under 19 s shows no spinner.)
    host.root_event("start_loading", &[]);
    // The engine writes its connect status under the tip while a map loads
    // (BO2's own: "Awaiting challenge...5").
    let status = host.values.borrow().localize.get("EXE_AWAITINGCHALLENGE").cloned().unwrap_or_default().replace("&&1", "1");
    if !status.is_empty() {
        host.set_menu_label_text("Menu.Loading", "statusLabel", &status);
    }
    diag::info!(Ui, "bo2mp loading: started ({:.0} ms)", t.elapsed().as_secs_f64() * 1000.0);
}

/// Whether an element is a 3D widget the 2D layer cannot draw (the globe,
/// the holotable grids turned into the screen).
pub(crate) fn is_3d(d: &hks_t6::host::Drawn) -> bool {
    d.kind == "globe" || d.x_rot.abs() > 0.01 || d.y_rot.abs() > 0.01
}

/// Each frame: save his stats when they changed; START MATCH starts the
/// match the lobby set up.
pub(crate) fn tick(
    host: &Host,
    mp: &Mp,
    front: &Bo2mpFrontend,
    commands: &mut Commands,
    swap: Option<&mut session::SessionSwapRequest>,
) {
    for line in std::mem::take(&mut mp.borrow_mut().stat_log) {
        diag::info!(Ui, "bo2mp stats command: {line}");
    }
    if mp.borrow_mut().take_dirty() {
        match mp.borrow().profile.save_file(&front.stats_path) {
            Ok(()) => diag::info!(Ui, "bo2mp front end: stats saved to {}", front.stats_path.display()),
            Err(e) => diag::warn!(Ui, "bo2mp front end: stats not saved ({}): {e}", front.stats_path.display()),
        }
    }
    // ZOMBIES: our Nuketown Zombies, swapped in as START MATCH swaps a map.
    if mp.borrow().zombies_requested {
        mp.borrow_mut().zombies_requested = false;
        let zone = "t6:zm_nuked".to_owned();
        diag::info!(Ui, "bo2mp front end: ZOMBIES");
        commands.remove_resource::<Bo2mpFrontend>();
        match swap {
            Some(swap) => match swap.request_zone(zone.clone()) {
                Ok(id) => diag::info!(Ui, "bo2mp front end: starting `{zone}` (swap #{id})"),
                Err(e) => diag::warn!(Ui, "bo2mp front end: `{zone}` not started: {e}"),
            },
            None => diag::warn!(Ui, "bo2mp front end: no session to start `{zone}`"),
        }
        return;
    }
    // Combat Training: the public lobby's countdown (from the front end's
    // first frame back from a match), run out.
    // (LUI's clock reads 0 until the new front end's first frame.)
    let wait = if host.now_ms() > 0.0 { mp.borrow_mut().public_start_in.take() } else { None };
    if let Some(wait) = wait {
        mp.borrow_mut().public_start_ms = Some(host.now_ms() + wait);
    }
    let training = {
        let m = mp.borrow();
        m.public_start_ms.filter(|at| host.now_ms() >= *at).and(m.playlist).and_then(hks_t6::playlists::by_index)
    };
    if training.is_none() && !mp.borrow().start_requested {
        mp.borrow_mut().private_start_ms = None;
        return;
    }
    // A Custom Game counts down "Game starting in N" before it starts.
    if training.is_none() {
        let now = host.now_ms();
        let at = *mp.borrow_mut().private_start_ms.get_or_insert(now + PRIVATE_COUNTDOWN_MS);
        if now < at {
            return;
        }
        mp.borrow_mut().private_start_ms = None;
    }
    mp.borrow_mut().start_requested = false;
    mp.borrow_mut().public_start_ms = None;
    let mut setup = {
        let v = host.values.borrow();
        let dvar = |k: &str, d: &str| v.dvars.get(k).filter(|x| !x.is_empty()).cloned().unwrap_or_else(|| d.to_owned());
        let setting = |k: &str| v.settings.get(k).copied().unwrap_or(0.0).max(0.0) as u32;
        // SETUP BOTS keeps the bots in dvars (BO2's gameoptions.lua:
        // addDvarLeftRightSelector "bot_friends" / "bot_enemies" /
        // "bot_difficulty"), their defaults from BO2's default_private.cfg.
        let count = |k: &str| {
            v.dvars.get(k).and_then(|x| x.trim().parse::<f32>().ok()).map_or_else(|| setting(k), |n| n.max(0.0) as u32)
        };
        session::Bo2mpMatchSetup {
            map: dvar("ui_mapname", "mp_la"),
            gametype: dvar("ui_gametype", "tdm"),
            bot_friends: count("bot_friends"),
            bot_enemies: count("bot_enemies"),
            bot_difficulty: count("bot_difficulty"),
        }
    };
    // A Combat Training playlist: its game type, map and teams
    // (`hks_t6::playlists`, ours), BO2's bot difficulty setting.
    if let Some(p) = training {
        setup.map = hks_t6::playlists::MAP.to_owned();
        setup.gametype = p.gametype.to_owned();
        setup.bot_friends = p.friends;
        setup.bot_enemies = p.enemies;
        setup.bot_difficulty = p.difficulty;
    }
    let zone = setup.zone();
    diag::info!(Ui, "bo2mp front end: START MATCH {setup:?} ranked {}", training.is_some());
    // The match's server reads and writes his stats file (his classes, XP);
    // XP counts in Combat Training, not in Custom Games.
    sim::set_local_stats_path(front.stats_path.clone());
    sim::set_local_match_ranked(training.is_some());
    // Where to come back to (Custom Games' lobby, or Combat Training's
    // public lobby, as it is now).
    {
        let v = host.values.borrow();
        commands.insert_resource(LobbyReturn {
            menu: if training.is_some() { "PublicGameLobby" } else { "PrivateOnlineGameLobby" }.to_owned(),
            session_modes: v.session_modes.clone(),
            game_modes: mp.borrow().game_modes.iter().copied().collect(),
            dvars: v.dvars.iter().map(|(k, x)| (k.clone(), x.clone())).collect(),
            settings: v.settings.iter().map(|(k, x)| (k.clone(), *x)).collect(),
            playlist: training.map(|p| p.index),
            stats_before: training.map(|_| mp.borrow().to_text()),
        });
    }
    commands.insert_resource(setup);
    // The match's HUD is the game's own, not the front end.
    commands.remove_resource::<Bo2mpFrontend>();
    match swap {
        Some(swap) => match swap.request_zone(zone.clone()) {
            Ok(id) => diag::info!(Ui, "bo2mp front end: starting `{zone}` (swap #{id})"),
            Err(e) => diag::warn!(Ui, "bo2mp front end: `{zone}` not started: {e}"),
        },
        None => diag::warn!(Ui, "bo2mp front end: no session to start `{zone}`"),
    }
}

/// bo2mp lane C: a multiplayer match's screen values from his client dvars
/// (sim `natives_mp::publish_screen`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MatchValues {
    /// "<allies>,<axis>".
    pub scores: String,
    /// Whole seconds left ("" = no clock).
    pub timeleft: String,
    pub team: String,
    pub gametype: String,
    /// "<n>:<menu>" ("<n>:" = his menus closed).
    pub menu: String,
    /// "<n>|<event>|<arg>..." by `;`.
    pub lui: String,
    /// "<n>|<attacker>|<victim>|<weapon>|<mod>" by `;`.
    pub feed: String,
    /// Everyone's team ("<client>:<team>,..."), the scoreboard's columns,
    /// and every player's row from the snapshot
    /// ("<client>|<name>|<score>|<kills>|<deaths>" by `;`).
    pub teams: String,
    pub cols: String,
    pub rows: String,
    /// The game type's settings ("name=value,...").
    pub settings: String,
    /// The minimap ("<material>,<ul x>,<ul y>,<lr x>,<lr y>,<north yaw>,
    /// <range>").
    pub minimap: String,
    /// His team's radar ("<uav> <satellite> <jammed>", `bo2mp_radar`).
    pub radar: String,
    /// A spot being picked on the map (lane D's `bo2mp_locsel`:
    /// "<selector material> <radius>", "" when not).
    pub locsel: String,
    /// bo2mp scope lane: his killcam ("<kind>|<killer>": kind 1 killcam,
    /// 2 final killcam; "" none): BIT_IN_KILLCAM, BIT_FINAL_KILLCAM and the
    /// killer's name card.
    pub killcam: String,
    /// bo2mp scope lane: his script took the HUD away (`setclientuivisibilityflag
    /// hud_visible 0`, as at the match's end) while a killcam still plays:
    /// BIT_HUD_VISIBLE goes, the killcam's own widget stays.
    pub hud_hidden: bool,
    /// bo2mp scope lane: he looks through a scope (a gun's overlay is up):
    /// BO2's BIT_IS_SCOPED (its HUD hides the minimap and voice dock).
    /// Not a dvar: the HUD sets it from his player state.
    pub scoped: bool,
    /// bo2mp vehicle screens: the scorestreak vehicle he rides ("<vehicle
    /// type> <seat>", sim `ride.rs`'s `bo2mp_vehicle`; "" on foot):
    /// BIT_IN_VEHICLE and BO2's killstreak HUD (`hud_update_killstreak_hud`).
    pub vehicle: String,
    /// bo2mp vehicle screens: the server's vision for him (`bo2mp_vision`,
    /// `setvisionsetforplayer` while `useservervisionset` is on; "" the
    /// map's own): his frame's grade.
    pub vision: String,
    /// bo2mp vehicle screens: his vehicle screen's infrared is on
    /// (`bo2mp_infrared` "1", `setinfraredvision`): BO2's HUD lights FLIR,
    /// else OPT.
    pub infrared: String,
}

/// His minimap this frame: the map's picture and the world corners it
/// covers (BO2's `setminimap`), north, him and his team (position, yaw).
#[derive(Clone, Debug)]
pub(crate) struct MinimapView {
    pub material: String,
    pub ul: [f32; 2],
    pub lr: [f32; 2],
    pub north_yaw: f32,
    /// The world distance across the minimap, edge to edge (the map's
    /// `compassmaxrange`; measured against real BO2 on Aftermath: the HUD
    /// box spans 2048 units, 512 map pixels = 7840 units, 1.1 box px per
    /// map pixel).
    pub range: f32,
    pub me: ([f32; 2], f32),
    pub friends: Vec<([f32; 2], f32)>,
    /// Enemies the map shows: spotted by his team's radar, or firing
    /// (position, yaw, firing).
    pub enemies: Vec<([f32; 2], f32, bool)>,
}

impl MinimapView {
    pub(crate) fn of(m: &MatchValues, me: ([f32; 2], f32), friends: Vec<([f32; 2], f32)>, enemies: Vec<([f32; 2], f32, bool)>) -> Option<MinimapView> {
        let f: Vec<&str> = m.minimap.split(',').collect();
        let (material, a, b, c, d, n, r) = match f[..] {
            [material, a, b, c, d, n, r] => (material, a, b, c, d, n, r),
            [material, a, b, c, d, n] => (material, a, b, c, d, n, "2048"),
            _ => return None,
        };
        let num = |x: &str| x.trim().parse::<f32>().unwrap_or(0.0);
        Some(MinimapView {
            material: material.to_owned(),
            ul: [num(a), num(b)],
            lr: [num(c), num(d)],
            north_yaw: num(n),
            range: num(r).max(100.0),
            me,
            friends,
            enemies,
        })
    }

    /// The picture's change per world unit east and per world unit south.
    fn uv_steps(&self) -> (f32, f32) {
        let (north, east) = self.axes();
        let span = [self.lr[0] - self.ul[0], self.lr[1] - self.ul[1]];
        let se = span[0] * east[0] + span[1] * east[1];
        let sn = span[0] * north[0] + span[1] * north[1];
        let safe = |x: f32| if x.abs() < 1.0 { 1.0 } else { x };
        (1.0 / safe(se), -1.0 / safe(sn))
    }

    /// BO2's HUD minimap as an `n` x `n` RGBA8 picture: the map around him,
    /// `range` world units across, turned so he faces up (bilinear
    /// samples of the map's picture; off the picture is see-through).
    pub(crate) fn render(&self, src: &[u8], sw: usize, sh: usize, n: usize) -> Vec<u8> {
        let mut out = vec![0u8; n * n * 4];
        if sw < 2 || sh < 2 || src.len() < sw * sh * 4 {
            return out;
        }
        let [u0, v0] = self.uv(self.me.0);
        let (du, dv) = self.uv_steps();
        let h = self.heading(self.me.1);
        let (c, s) = (h.cos(), h.sin());
        let world_per_px = self.range / n as f32;
        let texel = |x: usize, y: usize, k: usize| f32::from(src[(y * sw + x) * 4 + k]);
        for j in 0..n {
            for i in 0..n {
                // Right and down from the middle (world units), turned back
                // to the map's east and south.
                let sx = (i as f32 + 0.5 - n as f32 * 0.5) * world_per_px;
                let sy = (j as f32 + 0.5 - n as f32 * 0.5) * world_per_px;
                let (ex, so) = (sx * c - sy * s, sx * s + sy * c);
                let (u, v) = (u0 + ex * du, v0 + so * dv);
                if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                    continue;
                }
                let (fx, fy) = (u * sw as f32 - 0.5, v * sh as f32 - 0.5);
                let (x0, y0) = (fx.floor().clamp(0.0, (sw - 1) as f32) as usize, fy.floor().clamp(0.0, (sh - 1) as f32) as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
                let o = (j * n + i) * 4;
                for k in 0..4 {
                    let top = texel(x0, y0, k) * (1.0 - tx) + texel(x1, y0, k) * tx;
                    let bottom = texel(x0, y1, k) * (1.0 - tx) + texel(x1, y1, k) * tx;
                    out[o + k] = (top * (1.0 - ty) + bottom * ty).round() as u8;
                }
            }
        }
        out
    }

    /// The map's north and east in the world.
    fn axes(&self) -> ([f32; 2], [f32; 2]) {
        let r = self.north_yaw.to_radians();
        ([r.cos(), r.sin()], [r.sin(), -r.cos()])
    }

    /// A world point on the picture (0..1 across, 0..1 down).
    pub(crate) fn uv(&self, p: [f32; 2]) -> [f32; 2] {
        let (north, east) = self.axes();
        let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
        let d = [p[0] - self.ul[0], p[1] - self.ul[1]];
        let span = [self.lr[0] - self.ul[0], self.lr[1] - self.ul[1]];
        let u = dot(d, east) / dot(span, east).abs().max(1.0) * dot(span, east).signum();
        let v = -dot(d, north) / dot(span, north).abs().max(1.0) * (-dot(span, north)).signum();
        [u, v]
    }

    /// How far clockwise from the map's north a yaw faces (radians).
    pub(crate) fn heading(&self, yaw: f32) -> f32 {
        let (north, east) = self.axes();
        let f = [yaw.to_radians().cos(), yaw.to_radians().sin()];
        (f[0] * east[0] + f[1] * east[1]).atan2(f[0] * north[0] + f[1] * north[1])
    }

    /// The picture's size in world units (across, down).
    pub(crate) fn span_size(&self) -> [f32; 2] {
        let (se, ss) = self.uv_steps();
        [(1.0 / se).abs(), (1.0 / ss).abs()]
    }

    /// A world point around him on the minimap: right and down from its
    /// centre in world units, the map turned so he faces up.
    pub(crate) fn screen_offset(&self, p: [f32; 2]) -> [f32; 2] {
        let (north, east) = self.axes();
        let d = [p[0] - self.me.0[0], p[1] - self.me.0[1]];
        let (x, y) = (d[0] * east[0] + d[1] * east[1], -(d[0] * north[0] + d[1] * north[1]));
        let phi = -self.heading(self.me.1);
        [x * phi.cos() - y * phi.sin(), x * phi.sin() + y * phi.cos()]
    }
}

impl MatchValues {
    pub(crate) fn read(dvars: &sim::ScriptDvars<'_>) -> Option<MatchValues> {
        let s = |k: &str| dvars.string(k).unwrap_or_default().to_owned();
        dvars.string("bo2mp_scores")?;
        Some(MatchValues {
            scores: s("bo2mp_scores"),
            timeleft: s("bo2mp_timeleft"),
            team: s("bo2mp_team"),
            gametype: s("bo2mp_gametype"),
            menu: s("bo2mp_menu"),
            lui: s("bo2mp_lui"),
            feed: s("bo2mp_feed"),
            teams: s("bo2mp_teams"),
            cols: s("bo2mp_cols"),
            rows: String::new(),
            settings: s("bo2mp_settings"),
            minimap: s("bo2mp_minimap"),
            radar: s("bo2mp_radar"),
            locsel: s("bo2mp_locsel"),
            killcam: s("bo2mp_killcam"),
            hud_hidden: dvars.string("bo2zm_hud_hidden") == Some("1"),
            scoped: false,
            vehicle: s("bo2mp_vehicle"),
            vision: s("bo2mp_vision"),
            infrared: s("bo2mp_infrared"),
        })
    }

    /// Every player's scoreboard values from the snapshot.
    pub(crate) fn rows_of(meta: &sim::SnapshotMeta) -> String {
        meta.clients
            .iter()
            .map(|(c, m)| {
                let name = String::from_utf8_lossy(&m.name).trim_end_matches(char::from(0)).replace(['|', ';'], " ");
                format!("{}|{name}|{}|{}|{}", c.0, m.score, m.kills, m.deaths)
            })
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// What a match's HUD has already been sent (each numbered item once).
#[derive(Default)]
pub(crate) struct MatchSeen {
    menu: String,
    lui: u32,
    feed: u32,
    /// The kill feed's lines (newest last) and when each came (LUI time);
    /// each row's elements in the obituary window (killer, icon, killed).
    feed_lines: std::collections::VecDeque<(FeedLine, f64)>,
    feed_rows: Vec<[Value; 3]>,
}

/// One kill feed line: the killer (none: a suicide or a fall), the
/// weapon's kill icon (picture, width per height) or else its name, the
/// one killed.
#[derive(Clone, Debug, Default)]
pub(crate) struct FeedLine {
    attacker: String,
    icon: Option<(String, f32)>,
    victim: String,
    /// Whether the killer and the one killed are on his team (`None`: no
    /// team known, the name keeps the font's white).
    attacker_mine: Option<bool>,
    victim_mine: Option<bool>,
}

/// How many kill feed lines show at once, how long each stays (ms), and its
/// fades. The values are approximations (4 lines, 5 s, 0.25 s fade-in,
/// 0.5 s fade-out): BO2's own counts are unconfirmed (docs/INDEX.md, KNOWN GAPS).
const FEED_LINES: usize = 4;
const FEED_MS: f64 = 5000.0;
const FEED_FADE_IN_MS: f64 = 250.0;
const FEED_FADE_OUT_MS: f64 = 500.0;

/// `obj:name(args...)`.
fn call_method(host: &mut Host, obj: &Value, name: &str, args: Vec<Value>) -> Option<Vec<Value>> {
    let f = host.vm.index(obj, &Value::str(name)).ok()?;
    let mut all = vec![obj.clone()];
    all.extend(args);
    host.call(f, all)
}

/// A kill feed line as the engine writes one: the killer, the weapon's
/// kill icon, the one killed (a suicide or a fall: the icon and him).
fn feed_line(_host: &Host, mp: &Mp, item: &str, my_team: &str) -> Option<FeedLine> {
    let mut f = item.split('|').skip(1);
    let (attacker, victim, weapon) = (f.next()?, f.next()?, f.next().unwrap_or(""));
    let mod_ = f.next().unwrap_or("");
    let own = f.next() == Some("self");
    // (The sides the sim saw: the killer's, the one killed's.)
    // A name is MyTeam's colour on your side, EnemyTeam's on the other, and
    // white unless both his side and the name's are allies or axis (a
    // free-for-all, a spectator, no side).
    let side = |t: &str| matches!(t, "allies" | "axis");
    let mine = |team: Option<&str>| team.filter(|t| side(t) && side(my_team)).map(|t| t == my_team);
    let (attacker_mine, victim_mine) = (mine(f.next()), mine(f.next()));
    let base = weapon.split('+').next().unwrap_or("").trim_end_matches("_mp");
    let m = mp.borrow();
    // A gun with a kill icon draws
    // it; a gun without one draws the suicide picture; no gun at all draws
    // the picture of how he died (`killicondied` unless the means says
    // otherwise). The gun's name is never written.
    let none = base.is_empty() || base == "none";
    let named = |n: &str| m.kill_icons.get(n).cloned();
    let icon = if none {
        match mod_ {
            "MOD_MELEE" => named("hud_obit_knife"),
            "MOD_HEAD_SHOT" => named("killiconheadshot"),
            "MOD_CRUSH" => named("hud_obit_death_crush"),
            "MOD_FALLING" => named("hud_obit_death_falling"),
            "MOD_SUICIDE" | "MOD_TRIGGER_HURT" => named("hud_obit_death_suicide"),
            "MOD_IMPACT" => named("hud_obit_death_grenade_round"),
            _ => None,
        }
        .or_else(|| named("killicondied"))
    } else {
        named(base).or_else(|| named("hud_obit_death_suicide"))
    };
    Some(FeedLine {
        attacker: if own { String::new() } else { attacker.to_owned() },
        icon,
        victim: victim.to_owned(),
        attacker_mine,
        victim_mine,
    })
}

/// The kill feed in the engine's obituary window (window 0), newest at the
/// bottom, each line fading out after a few seconds: the killer's name, the
/// weapon's kill icon, the name of the one killed, left to right.
fn draw_feed(host: &mut Host, mp: &Mp, seen: &mut MatchSeen) {
    let now = host.now_ms();
    seen.feed_lines.retain(|(_, at)| now - at < FEED_MS);
    let Some(window) = mp.borrow().windows.iter().find(|(_, i)| *i == 0).map(|(w, _)| w.clone()) else {
        return;
    };
    let fonts = host.field("CoD", "fonts");
    let font = host.vm.index(&fonts, &Value::str("ExtraSmall")).unwrap_or(Value::Nil);
    let sizes = host.field("CoD", "textSize");
    let h = host.vm.index(&sizes, &Value::str("ExtraSmall")).ok().and_then(|v| v.as_num()).unwrap_or(16.0);
    if seen.feed_rows.is_empty() {
        let ui_text = host.field("LUI", "UIText");
        let ui_image = host.field("LUI", "UIImage");
        let (Ok(new_text), Ok(new_image)) = (host.vm.index(&ui_text, &Value::str("new")), host.vm.index(&ui_image, &Value::str("new"))) else {
            return;
        };
        for _ in 0..FEED_LINES {
            let make = |host: &mut Host, new: &Value| host.call(new.clone(), vec![]).and_then(|r| r.into_iter().next());
            let (Some(a), Some(i), Some(v)) = (make(host, &new_text), make(host, &new_image), make(host, &new_text)) else { return };
            for t in [&a, &v] {
                call_method(host, t, "setFont", vec![font.clone()]);
                call_method(host, t, "setText", vec![Value::str("")]);
            }
            for e in [&a, &i, &v] {
                call_method(host, e, "setAlpha", vec![Value::Num(0.0)]);
                call_method(host, &window, "addElement", vec![e.clone()]);
            }
            seen.feed_rows.push([a, i, v]);
        }
    }
    let measure = host.vm.global("GetTextDimensions");
    let width = |host: &mut Host, text: &str| -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        host.call(measure.clone(), vec![Value::str(text), font.clone(), Value::Num(h)])
            .and_then(|r| r.get(2).and_then(Value::as_num))
            .unwrap_or(h * 0.5 * text.len() as f32)
    };
    // BO2's name colours: the scripts' `CoD.teamColorFriendly` / `teamColorEnemy` (the g_TeamColor_MyTeam / EnemyTeam dvars).
    let team_rgb = |host: &mut Host, mine: bool| -> Option<[f32; 3]> {
        let t = host.field("CoD", if mine { "teamColorFriendly" } else { "teamColorEnemy" });
        let mut c = [0.0; 3];
        for (i, k) in ["r", "g", "b"].iter().enumerate() {
            c[i] = host.vm.index(&t, &Value::str(k)).ok().and_then(|v| v.as_num())?;
        }
        Some(c)
    };
    let shown: Vec<(FeedLine, f64)> = seen.feed_lines.iter().rev().take(FEED_LINES).rev().cloned().collect();
    let pad = FEED_LINES - shown.len();
    // The icon sits between ordinary space glyphs.
    let gap = {
        let w = width(host, "i i") - width(host, "ii");
        if w > 0.5 { w } else { h * 0.3 }
    };
    for (i, [a, icon, v]) in seen.feed_rows.clone().iter().enumerate() {
        let bottom = -(((FEED_LINES - 1 - i) as f32) * h);
        let place = |host: &mut Host, e: &Value, x0: f32, x1: f32, top: f32| {
            call_method(host, e, "setLeftRight", vec![Value::Bool(true), Value::Bool(false), Value::Num(x0), Value::Num(x1)]);
            call_method(host, e, "setTopBottom", vec![Value::Bool(false), Value::Bool(true), Value::Num(top), Value::Num(bottom)]);
        };
        let Some((line, at)) = i.checked_sub(pad).and_then(|k| shown.get(k)) else {
            for e in [a, icon, v] {
                call_method(host, e, "setAlpha", vec![Value::Num(0.0)]);
            }
            continue;
        };
        let age = now - at;
        let alpha = (age / FEED_FADE_IN_MS).min((FEED_MS - age) / FEED_FADE_OUT_MS).clamp(0.0, 1.0) as f32;
        let mut x = 0.0;
        // The killer.
        let w = width(host, &line.attacker);
        call_method(host, a, "setText", vec![Value::str(&line.attacker)]);
        let c = line.attacker_mine.and_then(|m| team_rgb(host, m)).unwrap_or([1.0; 3]);
        call_method(host, a, "setRGB", vec![Value::Num(c[0]), Value::Num(c[1]), Value::Num(c[2])]);
        place(host, a, x, x + w, bottom - h);
        call_method(host, a, "setAlpha", vec![Value::Num(if w > 0.0 { alpha } else { 0.0 })]);
        if w > 0.0 {
            x += w + gap;
        }
        // The weapon's kill icon (a 2:1 picture is 2.8 x
        // 1.4 font heights, a 4:1 one 2.8 x 0.7, a square one 1.4 x 1.4).
        match &line.icon {
            Some((picture, wide)) => {
                let (iw, ih) = if *wide >= 4.0 { (h * 2.0, h * 0.5) } else { (h * wide, h) };
                let register = host.vm.global("RegisterMaterial");
                if let Some(mat) = host.call(register, vec![Value::str(picture)]).and_then(|r| r.into_iter().next()) {
                    call_method(host, icon, "setImage", vec![mat]);
                }
                let low = bottom - (h - ih) * 0.5;
                call_method(host, icon, "setLeftRight", vec![Value::Bool(true), Value::Bool(false), Value::Num(x), Value::Num(x + iw)]);
                call_method(host, icon, "setTopBottom", vec![Value::Bool(false), Value::Bool(true), Value::Num(low - ih), Value::Num(low)]);
                call_method(host, icon, "setAlpha", vec![Value::Num(alpha)]);
                x += iw + gap;
            }
            None => {
                call_method(host, icon, "setAlpha", vec![Value::Num(0.0)]);
            }
        }
        let victim = line.victim.clone();
        // The one killed.
        let w = width(host, &victim);
        call_method(host, v, "setText", vec![Value::str(&victim)]);
        let c = line.victim_mine.and_then(|m| team_rgb(host, m)).unwrap_or([1.0; 3]);
        call_method(host, v, "setRGB", vec![Value::Num(c[0]), Value::Num(c[1]), Value::Num(c[2])]);
        place(host, v, x, x + w, bottom - h);
        call_method(host, v, "setAlpha", vec![Value::Num(alpha)]);
    }
}

/// His stats file for a match's class menu (beside settings.cfg).
pub(crate) fn stats_path(identity: Option<&frame::LaunchIdentity>) -> std::path::PathBuf {
    identity.map_or_else(|| std::path::PathBuf::from("iw4l-artifacts"), |i| i.artifacts.clone()).join("bo2mp_stats.txt")
}

/// A match's HUD before CoD's base: the multiplayer answers and his stats
/// (the class menu shows his classes).
pub(crate) fn match_install(host: &mut Host, stats: &std::path::Path) -> Mp {
    let mp = hks_t6::mp::install(host);
    if let Ok(text) = std::fs::read_to_string(stats) {
        mp.borrow_mut().load_text(&text);
    }
    mp
}

/// After CoD's base: a local online private match (BO2's CUSTOM GAMES; its
/// classes are the custom match set the server gives), in game, his HUD
/// shown; then BO2's own HUD scripts (`T6.HUD`).
pub(crate) fn match_load(host: &mut Host, mp: &Mp, gametype: &str, map: &str) {
    hks_t6::mp::after_base(host);
    // The visibility bits a live match has on: the HUD, the kill feed,
    // the minimap, objective icons, popups (medals, score).
    let bits: Vec<i32> = [
        "BIT_HUD_VISIBLE",
        "BIT_HUD_OBITUARIES",
        "BIT_COMPASS_VISIBLE",
        "BIT_HUD_SHOWOBJICONS",
        "BIT_ENABLE_POPUPS",
        "BIT_POPUPS_VISIBLE",
    ]
    .iter()
    .filter_map(|b| host.field("CoD", b).as_num().map(|n| n as i32))
    .collect();
    let modes: Vec<f32> = ["SESSIONMODE_ONLINE", "SESSIONMODE_PRIVATE"]
        .iter()
        .filter_map(|m| host.field("CoD", m).as_num())
        .collect();
    let private = host.field("CoD", "GAMEMODE_PRIVATE_MATCH").as_num().map_or(1, |n| n as i32);
    mp.borrow_mut().game_modes.insert(private);
    {
        let mut v = host.values.borrow_mut();
        for (k, x) in [
            ("r_fontResolution", "720"),
            ("ui_gametype", gametype),
            ("ui_gameType", gametype),
            ("g_gametype", gametype),
            ("ui_mapname", map),
            ("mapname", map),
            ("sv_running", "1"),
            ("cl_ingame", "1"),
        ] {
            v.dvars.insert(k.to_owned(), x.to_owned());
        }
        v.session_modes = modes;
        v.bits.extend(bits);
    }
    host.require("T6.HUD");
}

/// The game type's settings ("name=value,...") for the HUD's
/// `GetGametypeSetting` (and the score limit dvar).
fn apply_settings(host: &mut Host, mp: &Mp, settings: &str) {
    mp.borrow_mut().settings = settings
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .filter_map(|(k, v)| Some((k.trim().to_ascii_lowercase(), v.trim().parse::<f32>().ok()?)))
        .collect();
    if let Some(limit) = mp.borrow().settings.get("scorelimit") {
        host.values.borrow_mut().dvars.insert("ui_scorelimit".to_owned(), format!("{limit}"));
    }
}

/// Before BO2's HUD is built: what it builds from (the scoreboard's column
/// headers and one block per team: the game type's columns and settings).
pub(crate) fn match_prime(host: &mut Host, mp: &Mp, m: &MatchValues) {
    host.values.borrow_mut().columns = m.cols.split(',').filter(|c| !c.is_empty()).map(str::to_owned).collect();
    apply_settings(host, mp, &m.settings);
}

/// Open BO2's HUD as the engine does.
pub(crate) fn match_open(host: &mut Host) {
    host.open_menu("HUD");
    host.root_event("first_snapshot", &[]);
    host.root_event("hud_update_refresh", &[]);
}

/// bo2mp vehicle screens: BO2's killstreak HUD event for the vehicle he
/// rides (airvehiclehud.lua UpdateKillstreakHUD: `infrared` lights FLIR,
/// else OPT).
fn killstreak_hud(host: &mut Host, new: &MatchValues) {
    let kind = new.vehicle.split_whitespace().next();
    host.root_event(
        "hud_update_killstreak_hud",
        &[
            ("controller", Value::Num(0.0)),
            ("chopperGunner", Value::Bool(kind == Some("heli_player_gunner_mp"))),
            ("reaper", Value::Bool(kind == Some("remote_mortar_mp"))),
            ("predator", Value::Bool(kind == Some("remote_missile_mp"))),
            ("infrared", Value::Bool(new.infrared == "1")),
        ],
    );
}

/// The match's events for what changed: the score bar, the clock, the menus
/// the server opened, the LUI notifies (score popups, medals).
pub(crate) fn match_events(host: &mut Host, seen: &mut MatchSeen, old: Option<&MatchValues>, new: &MatchValues, mp: &Mp) {
    hks_t6::mp::update_timers(mp);
    let changed = |f: fn(&MatchValues) -> &str| old.is_none_or(|o| f(o) != f(new));
    let team_num = |host: &mut Host, t: &str| match t {
        "allies" => host.field("CoD", "TEAM_ALLIES").as_num().unwrap_or(1.0),
        "axis" => host.field("CoD", "TEAM_AXIS").as_num().unwrap_or(2.0),
        _ => host.field("CoD", "TEAM_FREE").as_num().unwrap_or(0.0),
    };
    if changed(|v| &v.team) || changed(|v| &v.scores) || changed(|v| &v.gametype) {
        diag::info!(Ui, "bo2mp hud: team {:?} scores {:?} gametype {:?}", new.team, new.scores, new.gametype);
    }
    if changed(|v| &v.team) {
        let t = team_num(host, &new.team);
        host.values.borrow_mut().team = t as i32;
        host.root_event("hud_update_team_change", &[("team", Value::Num(t))]);
    }
    if changed(|v| &v.scores) || changed(|v| &v.team) {
        let (allies, axis) = new.scores.split_once(',').unwrap_or(("0", "0"));
        let (allies, axis) = (allies.trim().parse::<f32>().unwrap_or(0.0), axis.trim().parse::<f32>().unwrap_or(0.0));
        let (yours, enemy) = if new.team == "axis" { (axis, allies) } else { (allies, axis) };
        let t = team_num(host, &new.team);
        host.root_event(
            "hud_update_scores",
            &[("team", Value::Num(t)), ("yourScore", Value::Num(yours)), ("enemyScore", Value::Num(enemy))],
        );
    }
    if changed(|v| &v.rows) || changed(|v| &v.teams) || changed(|v| &v.cols) || changed(|v| &v.scores) {
        let cols: Vec<String> = new.cols.split(',').filter(|c| !c.is_empty()).map(str::to_owned).collect();
        let team_of = |c: &str| {
            new.teams
                .split(',')
                .find_map(|e| e.split_once(':').filter(|(k, _)| *k == c).map(|(_, t)| t.to_owned()))
        };
        let mut board = Vec::new();
        for row in new.rows.split(';') {
            let f: Vec<&str> = row.split('|').collect();
            let [c, name, score, kills, deaths] = f[..] else { continue };
            let Some(team) = team_of(c) else { continue };
            let num = |x: &str| x.trim().parse::<f32>().unwrap_or(0.0);
            let values: Vec<f32> = cols
                .iter()
                .map(|col| match col.as_str() {
                    "score" => num(score),
                    "kills" => num(kills),
                    "deaths" => num(deaths),
                    "kdratio" => num(kills) / num(deaths).max(1.0),
                    _ => 0.0,
                })
                .collect();
            let team = team_num(host, &team) as i32;
            board.push(hks_t6::mp::BoardRow { client: c.parse().unwrap_or(0), team, name: name.to_owned(), cols: values });
        }
        board.sort_by(|a, b| b.cols.first().unwrap_or(&0.0).total_cmp(a.cols.first().unwrap_or(&0.0)));
        let (allies, axis) = new.scores.split_once(',').unwrap_or(("0", "0"));
        let scores = vec![
            (team_num(host, "allies") as i32, allies.trim().parse::<f32>().unwrap_or(0.0)),
            (team_num(host, "axis") as i32, axis.trim().parse::<f32>().unwrap_or(0.0)),
        ];
        // (The After Action Report's scoreboard is the match's last.)
        hks_t6::aar::set_last_match(hks_t6::aar::LastMatch {
            gametype: new.gametype.clone(),
            columns: cols.clone(),
            rows: board.clone(),
        });
        {
            let mut m = mp.borrow_mut();
            m.board = board;
            m.team_scores = scores;
        }
        host.values.borrow_mut().columns = cols;
        host.root_event("update_scoreboard", &[]);
    }
    // Picking a spot on the map (Lightning Strike, the choppers): BO2's HUD
    // opens its whole map (its handler for BIT_SELECTING_LOCATION), the
    // engine draws the selector on it.
    if changed(|v| &v.locsel) {
        let bit = host.field("CoD", "BIT_SELECTING_LOCATION").as_num().map(|n| n as i32);
        if let Some(bit) = bit {
            let on = !new.locsel.trim().is_empty();
            {
                let mut v = host.values.borrow_mut();
                if on {
                    v.bits.insert(bit);
                } else {
                    v.bits.remove(&bit);
                }
            }
            host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
        }
    }
    // The HUD the script took away (the match's end) goes with its bit: a
    // killcam's widget stays (it is not in the HUD's container).
    if old.is_none_or(|o| o.hud_hidden != new.hud_hidden)
        && let Some(bit) = host.field("CoD", "BIT_HUD_VISIBLE").as_num().map(|n| n as i32)
    {
        {
            let mut v = host.values.borrow_mut();
            if new.hud_hidden {
                v.bits.remove(&bit);
            } else {
                v.bits.insert(bit);
            }
        }
        host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
    }
    // The killcam (BO2's engine sets these bits while one plays; its HUD's
    // Killcam widget draws the title, the bands and the killer's card).
    if changed(|v| &v.killcam) {
        let (kind, killer) = new
            .killcam
            .split_once('|')
            .map_or((0, None), |(k, c)| (k.parse::<u8>().unwrap_or(0), c.parse::<i32>().ok().filter(|c| *c >= 0)));
        mp.borrow_mut().callout = killer;
        diag::info!(Ui, "bo2mp lui: killcam state {:?}", new.killcam);
        // Kind 3 is the card at his death, before the killcam: BO2's engine
        // sends `player_obituary_callout` with the killer's name card and
        // "Killed By" (notificationpopups.lua PlayerObituaryCallout); the
        // HUD stays, and the card goes after 2.5 s or as the killcam starts.
        if kind == 3
            && let Some(killer) = killer
        {
            let data = host.field("Engine", "GetCalloutPlayerData");
            let card = host.call(data, vec![Value::Num(0.0), Value::Num(killer as f32)]).and_then(|r| r.into_iter().next());
            if let Some(Value::Table(card)) = card {
                let killed_by = host.values.borrow().localize.get("CGAME_KILLEDBY").cloned().unwrap_or_else(|| "Killed By".to_owned());
                card.borrow_mut().set_str("killString", Value::str(&killed_by));
                host.root_event_table("player_obituary_callout", card);
                diag::info!(Ui, "bo2mp lui: killed-by card sent (killer {killer})");
            } else {
                diag::warn!(Ui, "bo2mp lui: no killed-by card: no callout data for {killer}");
            }
        }
        // The final one's bit first: the widget reads it as it opens.
        // He is dead and watching the killer: the ammo, score and minimap
        // go (their own bits, set after the killcam's). Only a bit that
        // changes is sent: the Killed By card closes on any update of
        // BIT_IN_KILLCAM.
        let want = [
            ("BIT_FINAL_KILLCAM", kind == 2),
            ("BIT_IN_KILLCAM", (1..=2).contains(&kind)),
            ("BIT_SPECTATING_CLIENT", (1..=2).contains(&kind)),
            ("BIT_PLAYER_DEAD", (1..=2).contains(&kind)),
        ];
        for (name, on) in want {
            let Some(bit) = host.field("CoD", name).as_num().map(|n| n as i32) else { continue };
            let changed = {
                let mut v = host.values.borrow_mut();
                if on { v.bits.insert(bit) } else { v.bits.remove(&bit) }
            };
            if changed {
                host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
            }
        }
    }
    if old.is_none_or(|o| o.scoped != new.scoped)
        && let Some(bit) = host.field("CoD", "BIT_IS_SCOPED").as_num().map(|n| n as i32)
    {
        {
            let mut v = host.values.borrow_mut();
            if new.scoped {
                v.bits.insert(bit);
            } else {
                v.bits.remove(&bit);
            }
        }
        host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
    }
    // bo2mp vehicle screens: riding a scorestreak vehicle. BO2's HUD hides
    // its ammo, score, game type and scorestreak parts on BIT_IN_VEHICLE
    // (ui_mp/t6/hud/ammoarea.lua, scorearea.lua, scorebottomleft.lua,
    // gametypebase.lua, rewardselection.lua), and its own screen for the
    // VTOL Warship's gunner (heli_player_gunner_mp, _helicopter_gunner.gsc)
    // is CoD.ChopperGunnerHUD, which hud.lua opens on
    // `hud_update_killstreak_hud` with chopperGunner set and closes on it
    // unset (airvehiclehud.lua UpdateKillstreakHUD).
    if changed(|v| &v.vehicle) {
        let riding = !new.vehicle.trim().is_empty();
        let gunner = new.vehicle.split_whitespace().next() == Some("heli_player_gunner_mp");
        // The Hellstorm (remote_missile_mp, _remotemissile.gsc) is not a
        // vehicle seat: BO2's HUD hides its ammo and score parts on
        // BIT_IN_GUIDED_MISSILE and opens PredatorHUD (predatorhud.lua) on
        // `hud_update_killstreak_hud` with predator set.
        let missile = new.vehicle.split_whitespace().next() == Some("remote_missile_mp");
        // The Lodestar (remote_mortar_mp, _remotemortar.gsc: his view on its
        // drone) is ReaperHUD (reaperhud.lua), which hud.lua opens on
        // `hud_update_killstreak_hud` with reaper set; BIT_IN_VEHICLE hides
        // the same parts BIT_IN_REMOTE_KILLSTREAK_STATIC does.
        let reaper = new.vehicle.split_whitespace().next() == Some("remote_mortar_mp");
        diag::info!(
            Ui,
            "bo2mp hud: vehicle {:?} (chopper gunner screen {gunner}, hellstorm screen {missile}, \
             lodestar screen {reaper})",
            new.vehicle
        );
        // His keys for the vehicle's own commands: BO2 binds each (up, down,
        // change seat, attack, attack 2) on the keys he has for the command
        // its button stands for (CG_UpdateVehicleBindings), the prompts'
        // `[{+vehiclemoveup}]` and `[{+weapnext_inventory}]`.
        {
            let mut v = host.values.borrow_mut();
            v.vehicle_binds.clear();
            let mut words = new.vehicle.split_whitespace().skip(2);
            for command in ["+vehiclemoveup", "+vehiclemovedown", "+switchseat", "+vehicleattack", "+vehicleattacksecond"] {
                if let Some(from) = words.next().filter(|w| *w != "-") {
                    v.vehicle_binds.insert(command.to_owned(), from.to_owned());
                }
            }
        }
        for (bit_name, on) in [("BIT_IN_VEHICLE", riding && !missile), ("BIT_IN_GUIDED_MISSILE", missile)] {
            let Some(bit) = host.field("CoD", bit_name).as_num().map(|n| n as i32) else {
                continue;
            };
            {
                let mut v = host.values.borrow_mut();
                if on {
                    v.bits.insert(bit);
                } else {
                    v.bits.remove(&bit);
                }
            }
            host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
        }
        killstreak_hud(host, new);
        // The drones' own screens: hud.lua's `hud_update_vehicle` closes the
        // last vehicle screen and opens `LUI.createMenu[vehicleType]`, which
        // BO2 has for the Dragonfire (qrdrone_mp, hud/qrdrone.lua) and the
        // AGR (ai_tank_drone_mp, hud/aitank.lua); on foot there is no type.
        // Sent after the killstreak event, which AirVehicleHUD screens also
        // hear.
        let kind = new.vehicle.split_whitespace().next().filter(|_| !missile && !reaper).unwrap_or("");
        let mut fields = vec![("controller", Value::Num(0.0))];
        if !kind.is_empty() {
            fields.push(("vehicleType", Value::str(kind)));
        }
        host.root_event("hud_update_vehicle", &fields);
    }
    // His screen's infrared switched (the Lodestar's and the VTOL Warship's
    // change-view key): hud.lua opens a screen only when it has none, so the
    // event again just relights the boxes.
    if !changed(|v| &v.vehicle) && changed(|v| &v.infrared) {
        killstreak_hud(host, new);
    }
    if changed(|v| &v.vision) {
        let found = asset_world::set_t6_player_vision(&new.vision);
        diag::info!(Ui, "bo2mp hud: server vision {:?} (vision found: {found})", new.vision);
    }
    if changed(|v| &v.settings) {
        apply_settings(host, mp, &new.settings);
        host.root_event("hud_update_refresh", &[]);
    }
    if changed(|v| &v.timeleft) {
        let ms = new.timeleft.trim().parse::<f32>().map_or(0.0, |s| s * 1000.0);
        mp.borrow_mut().game_end_ms = (!new.timeleft.trim().is_empty()).then(|| host.now_ms() + f64::from(ms));
        host.root_event("hud_update_game_timer", &[("timeLeft", Value::Num(ms))]);
    }
    if new.menu != seen.menu {
        seen.menu = new.menu.clone();
        match new.menu.split_once(':') {
            Some((_, "")) => host.root_event("close_all_ingame_menus", &[]),
            Some((_, menu)) => host.root_event("open_ingame_menu", &[("menuName", Value::str(menu))]),
            None => {}
        }
    }
    // Each LUI notify once: `{name = <event>, data = {args}}`.
    for item in new.lui.split(';') {
        let mut parts = item.split('|');
        let Some(n) = parts.next().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if n <= seen.lui {
            continue;
        }
        seen.lui = n;
        let Some(name) = parts.next() else { continue };
        let data = hks_t6::value::Table::new_ref();
        for (i, a) in parts.enumerate() {
            let v = a.trim().parse::<f32>().map_or_else(|_| Value::str(a), Value::Num);
            data.borrow_mut().set(Value::Num((i + 1) as f32), v);
        }
        diag::info!(Ui, "bo2mp lui notify: {item}");
        host.root_event(name, &[("data", Value::Table(data))]);
    }
    for item in new.feed.split(';') {
        let Some(n) = item.split('|').next().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if n <= seen.feed {
            continue;
        }
        seen.feed = n;
        if let Some(line) = feed_line(host, mp, item, &new.team) {
            let now = host.now_ms();
            seen.feed_lines.push_back((line, now));
        }
    }
    draw_feed(host, mp, seen);
}
