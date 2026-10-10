//! bo2zm M4: Black Ops II's own Zombies HUD, run from the game's UI
//! scripts: the HavokScript VM (`hks_t6`) loads LUI and CoD's base, then
//! `ui_mp/t6/hud.lua`; the root opens its `HUD` menu as the engine does,
//! and the game's values reach it as the engine's events
//! (`hud_update_rounds_played`, ...). Each frame its elements (pictures and
//! text, in root units 720 high) are drawn as Bevy UI over the game.
//!
//! On unless `BO2ZM_LUI=0`; the hand-made HUD in `zm_hud` keeps what the
//! engine itself draws in BO2 (the use hint, script HUD text, the hurt
//! overlay).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use frame::ClientSet;
use hks_t6::host::{Drawn, Host};
use hks_t6::value::Value;
use net::{LocalPresentClient, PresentedSnapshot};

use crate::bo2_font::Bo2Fonts;
use crate::layers::{UiLayer, UiLayerVisibility};

/// Font metrics the scripts' text measure reads: font name -> (pixel
/// height, advance per letter, space advance).
type Metrics = Rc<RefCell<HashMap<String, (f32, HashMap<char, f32>, f32)>>>;

/// The scripts' font files and the names `Bo2Fonts` keeps them by
/// (`ui/t6/codbase.lua`'s `fonts/<res>/<file>`).
const FONT_FILES: [(&str, &str); 7] = [
    ("normalFont", "Default"),
    ("smallFont", "Condensed"),
    ("bigFont", "Big"),
    ("extraBigFont", "Morris"),
    ("extraSmallFont", "ExtraSmall"),
    ("italicFont", "Italic"),
    ("smallItalicFont", "SmallItalic"),
];

fn font_name(file: &str) -> &'static str {
    let stem = file.rsplit('/').next().unwrap_or(file);
    FONT_FILES
        .iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(stem))
        .map_or("Default", |(_, n)| n)
}

/// On unless `BO2ZM_LUI=0` (then the hand-made HUD in `zm_hud` draws it all).
pub(crate) fn lui_enabled() -> bool {
    std::env::var("BO2ZM_LUI").map_or(true, |v| v != "0")
}

struct LuiHud {
    host: Host,
    metrics: Metrics,
    /// The scripts it was built from (their count).
    built_from: usize,
    opened: bool,
    /// The mouse in root units, as last sent.
    mouse_at: Option<Vec2>,
    /// A bind row asked for a key and the capture has not finished.
    binding: bool,
    /// His settings' revision last written into BO2's names, and what.
    settings_rev: Option<u64>,
    written: Vec<(String, String)>,
    /// BO2ZM_LUI_DUMP: how many elements the last dump listed.
    dumped: usize,
    /// BO2ZM_LUI_PAUSE_AT's progress, and when the HUD opened.
    test_step: usize,
    opened_at: f64,
    /// The values last sent (events go out when one changes).
    sent: Option<Values>,
    aspect: f32,
    /// bo2mp: the multiplayer front end's state (`lui_frontend`).
    mp: Option<hks_t6::mp::Mp>,
    /// bo2mp: built for a multiplayer match (BO2's MP HUD), and what its
    /// HUD was sent.
    match_mp: bool,
    seen: crate::lui_frontend::MatchSeen,
    /// bo2mp: the scoreboard is open (his scores key held).
    board_open: bool,
    ended_frames: u32,
    /// frames since the final killcam let go: the end scoreboard opened at once comes up empty
    calm_frames: u32,
    /// the end scoreboard's rows were seen drawn (logged once per opening)
    end_rows_logged: bool,
    scoreboard_bits: (bool, bool),
    /// bo2mp: frames until CAMPAIGN's prompt is closed for him (as its NO
    /// closes it; not while its YES is still running).
    back_in: u8,
    /// bo2mp: BO2's loading screen was opened for the match loading.
    loading_open: bool,
    /// bo2mp: when the front end's globe was last turned (`lui_scene`).
    scene_at: f64,
    /// bo2mp: when the backdrop floor was last redrawn.
    floor_at: f64,
    /// bo2mp: when each client was last seen firing (an
    /// enemy firing shows on the minimap for a moment).
    shots: std::collections::HashMap<u32, f64>,
}

impl LuiHud {
    /// The root's width in units (720 high).
    fn host_width(&self) -> f32 {
        720.0 * self.aspect.max(0.5)
    }
}

/// bo2mp: the test steps already run (front end, match).
static FRONT_STEPS_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MATCH_STEPS_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RETURN_STEPS_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How many times the front end was built (2 and more: back from a match).
static FRONT_BUILDS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The HUD's VM lives on the main thread (its values are `Rc`).
#[derive(Default)]
pub(crate) struct LuiHudSlot(Option<LuiHud>);

/// The LUI root node (true: BO2's loading screen's, on the loading layer).
#[derive(Component)]
struct LuiRoot(bool);

/// What BO2's menus take in and hand out: his keys and mouse, the
/// classic-menu handshake, the pause, and the engine calls they make.
#[derive(bevy::ecs::system::SystemParam)]
struct LuiIo<'w, 's> {
    /// His monitors (BO2's display options count them).
    monitors: Query<'w, 's, &'static bevy::window::Monitor>,
    /// His controller (Start opens the pause menu; A, B, the d-pad and the
    /// shoulders work its menus).
    pads: Query<'w, 's, &'static bevy::input::gamepad::Gamepad>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    /// His key presses in the order they came: a hitch (a menu's slow
    /// build) must not fold two presses of a key into one.
    key_events: MessageReader<'w, 's, bevy::input::keyboard::KeyboardInput>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    menus: ResMut<'w, frame::LuiMenus>,
    paused: ResMut<'w, frame::GamePaused>,
    exec: MessageWriter<'w, frame::UiExecCommand>,
    sounds: MessageWriter<'w, frame::UiPlaySound>,
    inbox: Option<ResMut<'w, net::ClientActionInbox>>,
    ids: Option<ResMut<'w, net::ActionRequestIds>>,
    role: Option<Res<'w, frame::RuntimeRole>>,
    /// The key capture BO2's bind rows start (`Engine.BindCommand`).
    bind: MessageWriter<'w, frame::UiBindRequest>,
    capture: Res<'w, frame::UiBindingCapture>,
    /// His settings, shown and changed by BO2's Settings and Controls.
    settings: ResMut<'w, frame::GameSettings>,
    /// How much the menus blur the world (`Engine.BlurWorld`).
    blur: ResMut<'w, frame::WorldBlur>,
    /// bo2mp: the black dims an in-match menu stacks over the world.
    dim: ResMut<'w, frame::WorldDim>,
    /// bo2mp: the same dims, counted however they are drawn.
    menu_dims: ResMut<'w, frame::MenuDims>,
    /// bo2mp: booted into BO2's multiplayer front end; its match swap.
    front: Option<Res<'w, crate::lui_frontend::Bo2mpFrontend>>,
    /// Where he left the front end for a match (its lobby, reopened).
    lobby_return: Option<Res<'w, crate::lui_frontend::LobbyReturn>>,
    identity: Option<Res<'w, frame::LaunchIdentity>>,
    swap: Option<ResMut<'w, session::SessionSwapRequest>>,
    /// bo2mp: a match loading (BO2's own loading screen draws it).
    loading: Option<Res<'w, assets::LoadingScreen>>,
    setup: Option<Res<'w, session::Bo2mpMatchSetup>>,
    /// bo2mp: since when the end scoreboard has been up (the match goes back
    /// to the front end only after BO2's 5 s intermission hold).
    end_board: Option<ResMut<'w, frame::EndBoardShown>>,
}

/// Frames after the final killcam lets go before the end scoreboard opens
/// (opened at once it comes up with no rows). Experiment: BO2MP_END_CALM.
fn calm_limit() -> u32 {
    static LIMIT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| std::env::var("BO2MP_END_CALM").ok().and_then(|v| v.parse().ok()).unwrap_or(45))
}

/// The resolutions BO2's video mode offers (his own first if it is not
/// one of them).
const RESOLUTIONS: [&str; 6] = ["1280x720", "1366x768", "1600x900", "1920x1080", "2560x1440", "3840x2160"];

/// His settings as BO2's profile and hardware-profile names.
fn profile_of(s: &frame::GameSettings) -> Vec<(String, String)> {
    let b = |v: bool| if v { "1" } else { "0" }.to_owned();
    vec![
        ("r_mode".into(), s.resolution.to_string()),
        ("r_fullscreen".into(), b(s.fullscreen)),
        ("r_vsync".into(), b(s.vsync)),
        ("cg_fov_default".into(), format!("{}", s.fov.round())),
        ("sm_enable".into(), b(s.shadows)),
        ("snd_menu_master".into(), format!("{:.2}", s.master_volume)),
        ("input_viewSensitivity".into(), format!("{:.2}", s.sensitivity)),
        ("input_invertpitch".into(), b(s.invert_mouse)),
        // The Controls tab's Look rows: BO2's own names for the same
        // settings (m_pitch's sign inverts the mouse; the mouse always
        // looks freely here).
        ("m_pitch".into(), if s.invert_mouse { "-0.022" } else { "0.022" }.to_owned()),
        ("cl_freelook".into(), "1".to_owned()),
        ("mouseSensitivity".into(), format!("{:.2}", s.sensitivity)),
        // His name (BO2's `name` dvar: the lobby's player list).
        ("name".into(), s.player_name.clone()),
    ]
}

/// One of BO2's names, changed by its menus, into his settings.
fn apply_profile(s: &mut frame::GameSettings, key: &str, value: &str) {
    let on = value.trim() != "0" && !value.trim().is_empty();
    let num = value.trim().parse::<f32>().ok();
    match key {
        "r_mode" => {
            if let Some((w, h)) = value.trim().split_once('x')
                && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
            {
                s.resolution = frame::DisplayResolution::new(w, h);
            }
        }
        "r_fullscreen" => s.fullscreen = on,
        "r_vsync" => s.vsync = on,
        "cg_fov_default" => s.fov = num.unwrap_or(s.fov),
        "sm_enable" => s.shadows = on,
        "snd_menu_master" => s.master_volume = num.unwrap_or(s.master_volume).clamp(0.0, 1.0),
        "input_viewSensitivity" => s.sensitivity = num.unwrap_or(s.sensitivity),
        "input_invertpitch" => s.invert_mouse = on,
        "m_pitch" => s.invert_mouse = num.is_some_and(|v| v < 0.0),
        "mouseSensitivity" => s.sensitivity = num.unwrap_or(s.sensitivity),
        _ => {}
    }
}

/// His settings <-> BO2's names: his go in when they change (not by the
/// menus), and a name the menus changed comes back into them.
fn settings_bridge(hud: &mut LuiHud, settings: &mut frame::GameSettings) {
    if hud.settings_rev != Some(settings.revision) {
        let pairs = profile_of(settings);
        let mut v = hud.host.values.borrow_mut();
        for (k, x) in &pairs {
            v.profile.insert(k.clone(), x.clone());
            v.dvars.insert(k.clone(), x.clone());
        }
        let mut modes: Vec<String> = RESOLUTIONS.iter().map(|r| (*r).to_owned()).collect();
        let mine = settings.resolution.to_string();
        if !modes.contains(&mine) {
            modes.insert(0, mine);
        }
        v.enums.insert("r_mode".to_owned(), modes);
        hud.written = pairs;
        hud.settings_rev = Some(settings.revision);
        return;
    }
    let changed: Vec<(String, String)> = {
        let v = hud.host.values.borrow();
        hud.written
            .iter()
            .filter_map(|(k, old)| {
                let now = v.profile.get(k).filter(|x| *x != old).or_else(|| v.dvars.get(k).filter(|x| *x != old))?;
                Some((k.clone(), now.clone()))
            })
            .collect()
    };
    if changed.is_empty() {
        return;
    }
    for (k, x) in &changed {
        diag::info!(Ui, "bo2zm lui: setting {k} = {x}");
        apply_profile(settings, k, x);
    }
    settings.touch();
}

/// His keys as BO2's pad buttons (what its menus listen for).
const MENU_KEYS: [(KeyCode, &str); 7] = [
    (KeyCode::Escape, "secondary"),
    (KeyCode::Enter, "primary"),
    (KeyCode::NumpadEnter, "primary"),
    (KeyCode::ArrowUp, "up"),
    (KeyCode::ArrowDown, "down"),
    (KeyCode::ArrowLeft, "left"),
    (KeyCode::ArrowRight, "right"),
];

/// A key's name as BO2's prompts give their shortcut ("TAB", "F").
fn shortcut_key(key: KeyCode) -> Option<String> {
    let name = format!("{key:?}");
    if key == KeyCode::Tab {
        return Some("TAB".to_owned());
    }
    name.strip_prefix("Key").filter(|l| l.len() == 1).map(str::to_owned)
}

/// One key shortcut press into BO2's menus: the key, and the command his
/// binds give it (`bind1`; "" for none - a prompt with no bind of its own
/// compares it too, so it must not be nil).
fn key_shortcut(host: &mut Host, key: &str) {
    let bind = host
        .values
        .borrow()
        .binds
        .iter()
        .find(|(_, keys)| keys.iter().any(|k| k.eq_ignore_ascii_case(key)))
        .map(|(cmd, _)| cmd.clone())
        .unwrap_or_default();
    host.root_event(
        "gamepad_button",
        &[
            ("button", Value::str("key_shortcut")),
            ("key", Value::str(key)),
            ("bind1", Value::str(&bind)),
            ("down", Value::Bool(true)),
            ("qualifier", Value::str("keyboard")),
            ("controller", Value::Num(0.0)),
        ],
    );
}

/// Keys and mouse into BO2's menus: Esc opens its pause menu (the HUD's
/// `open_ingame_menu`, menu `class`); with a menu open, keys become pad
/// buttons and the mouse its pointer (root units).
fn menu_input(hud: &mut LuiHud, io: &mut LuiIo<'_, '_>, console_open: bool, cursor: Option<Vec2>, scale: f32) {
    if console_open {
        return;
    }
    let key_events: Vec<_> = io.key_events.read().cloned().collect();
    use bevy::input::gamepad::GamepadButton as P;
    let pad_start = io.pads.iter().any(|p| p.just_pressed(P::Start));
    // bo2mp: the front end is all menus (its main menu is not a `Menu.*`
    // child of the root), so his keys and mouse always work them there.
    let front_end = io.front.is_some() && !hud.match_mp;
    if !front_end && hud.host.open_menus().is_empty() {
        if io.keys.just_pressed(KeyCode::Escape) || pad_start {
            hud.host.root_event("open_ingame_menu", &[("menuName", Value::str("class"))]);
        }
        return;
    }
    // (Each press and release as it came, so keys that arrive together
    // after a hitch all count; a held key's repeats do not.)
    for ev in &key_events {
        if let Some((_, button)) = MENU_KEYS.iter().find(|(k, _)| *k == ev.key_code) {
            match ev.state {
                bevy::input::ButtonState::Pressed if !ev.repeat => hud.host.button(button, true),
                bevy::input::ButtonState::Released => hud.host.button(button, false),
                _ => {}
            }
        }
    }
    // BO2 PC's key shortcuts on its button prompts ("TAB After Action
    // Report", "F Friends", "S Search Preferences"): the engine sends
    // gamepad_button "key_shortcut" with the key's name.
    for key in io.keys.get_just_pressed() {
        if let Some(name) = shortcut_key(*key) {
            key_shortcut(&mut hud.host, &name);
        }
    }
    // The pad's buttons by BO2's names (Start backs out like Esc).
    for pad in &io.pads {
        for (b, name) in [
            (P::South, "primary"),
            (P::East, "secondary"),
            (P::Start, "secondary"),
            (P::West, "alt1"),
            (P::North, "alt2"),
            (P::DPadUp, "up"),
            (P::DPadDown, "down"),
            (P::DPadLeft, "left"),
            (P::DPadRight, "right"),
            (P::LeftTrigger, "shoulderl"),
            (P::RightTrigger, "shoulderr"),
        ] {
            if pad.just_pressed(b) {
                hud.host.button(name, true);
            } else if pad.just_released(b) {
                hud.host.button(name, false);
            }
        }
    }
    if cursor.is_none() && io.mouse.get_just_pressed().next().is_some() {
        diag::info!(Ui, "bo2mp lui: mouse pressed with no pointer over the window");
    }
    if let Some(c) = cursor {
        let p = c / scale.max(1e-3);
        if io.mouse.just_pressed(MouseButton::Left) {
            diag::info!(Ui, "bo2mp lui: mouse at {:.0},{:.0} (window {:.0},{:.0}) pressed", p.x, p.y, c.x, c.y);
        }
        if hud.mouse_at != Some(p) {
            hud.mouse_at = Some(p);
            hud.host.mouse("mousemove", p.x, p.y, "");
        }
        for (b, name) in [(MouseButton::Left, "left"), (MouseButton::Right, "right")] {
            if io.mouse.just_pressed(b) {
                hud.host.mouse("mousedown", p.x, p.y, name);
            }
            if io.mouse.just_released(b) {
                hud.host.mouse("mouseup", p.x, p.y, name);
            }
        }
    }
}

/// Carry out what the menus asked of the engine.
fn engine_calls(hud: &mut LuiHud, io: &mut LuiIo<'_, '_>, local: Option<&LocalPresentClient>) {
    for call in hud.host.take_calls() {
        diag::info!(Ui, "bo2zm lui engine call: {call:?}");
        match call {
            hks_t6::host::EngineCall::MenuResponse(menu, response) => {
                let (Some(inbox), Some(ids), Some(local)) = (io.inbox.as_mut(), io.ids.as_mut(), local) else {
                    continue;
                };
                let (Some(m), Some(r)) = (sim::menu_response_field(&menu), sim::menu_response_field(&response))
                else {
                    diag::warn!(Ui, "bo2zm lui: menu response {menu} {response} does not fit");
                    continue;
                };
                let request_id = ids.allocate();
                if let Err(e) = inbox.push(
                    local.0,
                    sim::ClientAction::MenuResponse {
                        request_id,
                        menu: m,
                        response: r,
                    },
                ) {
                    diag::warn!(Ui, "bo2zm lui: menu response not queued: {e}");
                }
            }
            hks_t6::host::EngineCall::Exec(text) => {
                // BO2's config files (`exec default_private.cfg`) and dvar
                // sets, as ExecNow runs them.
                if hks_t6::host::run_command(&hud.host.values, &text) {
                    // (bo2mp: its stats commands on his stats.)
                    if let Some(mp) = hud.mp.as_ref() {
                        hks_t6::mp::apply_stat_commands(mp, &hud.host.values);
                    }
                    continue;
                }
                // BO2's commands as ours; the ones with nothing here to do
                // (pad rumble, party, on-screen keyboard) are dropped.
                let word = text.split_whitespace().next().unwrap_or("");
                // bo2mp: the main menu's prompts. ZOMBIES (YES runs
                // `startZombies`) starts our Nuketown Zombies; CAMPAIGN
                // (`startSingleplayer`) is not built: its prompt closes as
                // NO closes it, and he stays at the main menu.
                // The file list (the Emblem Editor card's CODTv screen) asks
                // for his storage slots: offline they are all there; and
                // its online list of files is answered empty at once.
                if word.eq_ignore_ascii_case("fileshareGetSlots") {
                    hud.host.root_event("fileshare_slots_available", &[("valid", Value::Bool(true)), ("controller", Value::Num(0.0))]);
                    continue;
                }
                if word.eq_ignore_ascii_case("bo2mpDwSearch") {
                    let folder = text.split_whitespace().nth(1).and_then(|n| n.parse::<f32>().ok()).unwrap_or(0.0);
                    hud.host.root_event(
                        "fileshare_search_complete",
                        &[
                            ("contextid", Value::Num(folder)),
                            ("numresults", Value::Num(0.0)),
                            ("startIndex", Value::Num(0.0)),
                            ("totalFiles", Value::Num(0.0)),
                            ("controller", Value::Num(0.0)),
                        ],
                    );
                    continue;
                }
                if word.eq_ignore_ascii_case("startZombies") {
                    if let Some(mp) = hud.mp.as_ref() {
                        mp.borrow_mut().zombies_requested = true;
                    }
                    continue;
                }
                // The party changed (a lobby opened): the engine tells the
                // lobbies `partylobby_update` with its member count (his
                // party: him alone), which enables FIND MATCH.
                // Its members (him) go with it, and to the game lobby's
                // lists (`gamelobby_update`): BO2's lobbies list who is in
                // them from these events.
                if word.eq_ignore_ascii_case("party_statechanged") {
                    let get = hud.host.field("Engine", "GetPlayersInLobby");
                    let members = hud.host.call(get, vec![]).and_then(|r| r.into_iter().next()).unwrap_or(Value::Nil);
                    for event in ["partylobby_update", "gamelobby_update"] {
                        hud.host.root_event(
                            event,
                            &[
                                ("controller", Value::Num(0.0)),
                                ("actualPartyMemberCount", Value::Num(1.0)),
                                ("members", members.clone()),
                            ],
                        );
                    }
                    continue;
                }
                // FIND MATCH (`xstartparty`, then the public game lobby):
                // a Combat Training playlist's lobby counts down to its
                // match (`lui_frontend::tick` starts it).
                if word.eq_ignore_ascii_case("xstartparty") {
                    if let Some(mp) = hud.mp.as_ref() {
                        let mut m = mp.borrow_mut();
                        if m.playlist.is_some() {
                            m.public_start_ms = Some(hud.host.now_ms() + hks_t6::playlists::COUNTDOWN_MS);
                            diag::info!(Ui, "bo2mp front end: FIND MATCH, Combat Training playlist {:?}", m.playlist);
                        }
                    }
                    continue;
                }
                // The Emblem Editor's commands change the emblem edited
                // (`MpState::emblem`; the editor reads it back through
                // `GetSelectedLayerIconID` / `GetSelectedLayerColor` /
                // `GetUsedLayerCount`).
                if word.get(..5).is_some_and(|w| w.eq_ignore_ascii_case("emble")) && word.len() > 5 {
                    let nums: Vec<f32> = text.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
                    if let Some(mp) = hud.mp.as_ref() {
                        let mut m = mp.borrow_mut();
                        let e = &mut m.emblem;
                        match word.to_ascii_lowercase().as_str() {
                            "emblemclearall" => e.clear_all(),
                            "emblemselect" => e.select(nums.first().map_or(0, |n| n.max(0.0) as usize)),
                            "emblemicon" => e.set_icon(nums.first().map_or(-1, |n| *n as i32)),
                            "emblemclear" => e.set_icon(-1),
                            "emblemsetselectedlayericonid" => e.set_icon(nums.first().map_or(-1, |n| *n as i32)),
                            "emblemsetscalemode" => e.set_scale_mode(nums.first().map_or(0, |n| *n as i32)),
                            "emblempalette" if nums.len() >= 4 => e.set_color([nums[0], nums[1], nums[2], nums[3]]),
                            _ => diag::info!(Ui, "bo2zm lui: emblem command `{text}` is not built yet"),
                        }
                    }
                    continue;
                }
                if word.eq_ignore_ascii_case("startSingleplayer") {
                    diag::info!(Ui, "bo2mp front end: CAMPAIGN is not built; its prompt closes");
                    hud.back_in = 3;
                    continue;
                }
                let ours = match word {
                    "fast_restart" | "map_restart" => Some("map_restart".to_owned()),
                    "disconnect" | "quit" => Some(word.to_owned()),
                    _ => None,
                };
                match ours {
                    Some(text) => {
                        io.exec.write(frame::UiExecCommand { text });
                    }
                    None => diag::info!(Ui, "bo2zm lui: engine command `{text}` has nothing to do here"),
                }
            }
            hks_t6::host::EngineCall::PlaySound(alias) => {
                io.sounds.write(frame::UiPlaySound { alias });
            }
            hks_t6::host::EngineCall::FetchLeagueTeams => {
                // Offline: the answer is at once, one result (his Solo card).
                hud.host.root_event(
                    "league_team_info_fetched",
                    &[("success", Value::Bool(true)), ("numResults", Value::Num(1.0))],
                );
            }
            hks_t6::host::EngineCall::BlurWorld(amount) => {
                io.blur.0 = amount.max(0.0);
            }
            hks_t6::host::EngineCall::BindCommand(command, _index) => {
                // The game's own capture binds the next key he presses.
                io.bind.write(frame::UiBindRequest { command });
                hud.binding = true;
            }
        }
    }
}

/// One drawn element (its LUI id) and what it shows (rebuilt on change;
/// its alpha is set in place).
#[derive(Component)]
struct LuiNode {
    id: usize,
    shows: String,
    alpha: f32,
}

/// bo2mp: a multiplayer match's HUD: its game type, map, his stats file.
struct MatchSpec {
    gametype: String,
    map: String,
    stats: std::path::PathBuf,
}

fn build(ui: &assets::T6Ui, front: Option<&crate::lui_frontend::Bo2mpFrontend>, mp_match: Option<&MatchSpec>) -> LuiHud {
    let metrics: Metrics = Rc::default();
    let m = metrics.clone();
    let measure = Box::new(move |text: &str, font: &str, h: f32| {
        let m = m.borrow();
        let Some((px, adv, space)) = m.get(font_name(font)) else {
            return text.chars().count() as f32 * h * 0.5;
        };
        let w: f32 = text.chars().map(|c| if c == ' ' { *space } else { adv.get(&c).copied().unwrap_or(*space) }).sum();
        w * h / px.max(1.0)
    });
    let scripts = ui.scripts.iter().map(|(n, b)| (n.clone(), b.to_vec())).collect();
    let mut host = Host::new(scripts, measure);
    if front.is_some() {
        FRONT_BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    // bo2mp: the multiplayer front end's answers go in before the base.
    let mp = match (front, mp_match) {
        (Some(f), _) => Some(crate::lui_frontend::install(&mut host, f)),
        (None, Some(m)) => Some(crate::lui_frontend::match_install(&mut host, &m.stats)),
        _ => None,
    };
    // The text and string tables first: CoD's base reads tables as it
    // loads (CoD.MAX_RANK, CoD.MAX_PRESTIGE from the rank tables).
    {
        let mut v = host.values.borrow_mut();
        v.localize = ui.strings.iter().map(|(k, t)| (k.to_ascii_uppercase(), t.clone())).collect();
        diag::info!(
            Ui,
            "bo2zm lui: string tables {}",
            ui.tables.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" ")
        );
        for (name, cols, _rows, cells) in ui.tables.iter() {
            let rows = cells.chunks((*cols).max(1)).map(<[String]>::to_vec).collect();
            v.tables.insert(name.clone(), rows);
        }
        v.configs = ui.configs.iter().map(|(k, t)| (k.to_ascii_lowercase(), t.clone())).collect();
    }
    host.load_base();
    if let (Some(m), Some(state), None) = (mp_match, mp.as_ref(), front) {
        crate::lui_frontend::match_load(&mut host, state, &m.gametype, &m.map);
    } else if mp.is_some() {
        crate::lui_frontend::load(&mut host);
    } else {
        zombies_values(&mut host);
        host.require("T6.HUD");
    }
    LuiHud {
        host,
        metrics,
        built_from: ui.scripts.len(),
        opened: false,
        mouse_at: None,
        test_step: 0,
        dumped: 0,
        opened_at: 0.0,
        binding: false,
        settings_rev: None,
        written: Vec::new(),
        sent: None,
        aspect: 0.0,
        match_mp: front.is_none() && mp.is_some(),
        mp,
        seen: crate::lui_frontend::MatchSeen::default(),
        board_open: false,
        ended_frames: 0,
        calm_frames: 0,
        end_rows_logged: false,
        scoreboard_bits: (false, false),
        back_in: 0,
        loading_open: false,
        scene_at: 0.0,
        floor_at: 0.0,
        shots: std::collections::HashMap::new(),
    }
}

/// Nuketown Zombies as the engine reports it: survival, standard rules,
/// the HUD shown, one player on the allies.
fn zombies_values(host: &mut Host) {
    let hud_bit = host.field("CoD", "BIT_HUD_VISIBLE").as_num().map_or(0, |n| n as i32);
    let allies = host.field("CoD", "TEAM_ALLIES").as_num().map_or(1, |n| n as i32);
    // An offline game (Restart Level in the pause menu).
    let offline = host.field("CoD", "SESSIONMODE_OFFLINE").as_num();
    let mut v = host.values.borrow_mut();
    for (k, x) in [
        ("r_fontResolution", "720"),
        ("ui_gametype", "zstandard"),
        ("g_gametype", "zstandard"),
        ("ui_zm_gamemodegroup", "zsurvival"),
        ("ui_zm_mapstartlocation", "nuked"),
        ("ui_mapname", "zm_nuked"),
        ("mapname", "zm_nuked"),
        // He hosts his own game (CoD.isHost: the leave-game popup ends it).
        ("sv_running", "1"),
    ] {
        v.dvars.insert(k.to_owned(), x.to_owned());
    }
    v.settings.insert("startRound".to_owned(), 1.0);
    v.bits.insert(hud_bit);
    v.team = allies;
    v.session_modes = offline.into_iter().collect();
}

/// The values the HUD shows, from the presented snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
struct Values {
    round: i32,
    score: i32,
    weapon: i32,
    clip: i32,
    /// A dual-wield weapon's left clip.
    clip_left: Option<i32>,
    stock: i32,
    /// His grenades and mines (`name:count` by `;`).
    offhand: String,
    /// His HUD client fields (`field:value` by `,`: perks, power-ups).
    icons: String,
    /// His d-pad slots (`slot:weapon` by `;`, the scripts' setactionslot).
    slots: String,
    /// bo2mp: his scorestreaks (`icon:cost` by `;`) and his momentum.
    streaks: String,
    momentum: i32,
    /// The held weapon's name and display-name key (`ZOMBIE_WEAPON_...`).
    weapon_name: String,
    weapon_key: String,
    /// bo2mp: the held gun's fire-rate picture (`hud_mp_firerate_auto`).
    fire_material: &'static str,
    /// bo2mp: a multiplayer match's values.
    mp: Option<crate::lui_frontend::MatchValues>,
}

/// A d-pad weapon's icon.
fn slot_icon(weapon: &str) -> Option<&'static str> {
    Some(match weapon {
        "claymore_zm" => "hud_icon_claymore",
        _ => return None,
    })
}

/// The HUD icon of a grenade the engine names in `hud_update_offhand`, and
/// its slot.
fn offhand_slot(weapon: &str) -> Option<(&'static str, &'static str)> {
    Some(match weapon {
        "frag_grenade_zm" => ("lethal", "hud_us_grenade"),
        "sticky_grenade_zm" => ("lethal", "hud_icon_sticky_grenade"),
        "cymbal_monkey_zm" => ("tactical", "hud_cymbal_monkey"),
        // bo2mp: a multiplayer class's equipment.
        // The MP frag's own hudIcon / ammoCounterIcon / killIcon (common_mp's
        // weapon def) is `hud_grenadeicon`, the green camo pineapple; the
        // blue steel ball `hud_us_grenade` is Zombies' frag.
        "frag_grenade_mp" => ("lethal", "hud_grenadeicon"),
        "sticky_grenade_mp" => ("lethal", "hud_icon_sticky_grenade"),
        "hatchet_mp" => ("lethal", "hud_hatchet"),
        "claymore_mp" => ("lethal", "hud_icon_claymore"),
        "satchel_charge_mp" => ("lethal", "hud_icon_satchelcharge"),
        "bouncingbetty_mp" => ("lethal", "hud_bounce_betty"),
        "flash_grenade_mp" => ("tactical", "hud_us_flashgrenade"),
        "concussion_grenade_mp" => ("tactical", "hud_us_stungrenade"),
        "willy_pete_mp" => ("tactical", "hud_willy_pete"),
        "emp_grenade_mp" => ("tactical", "hud_empgrenade"),
        "trophy_system_mp" => ("tactical", "hud_trophy_system"),
        "tactical_insertion_mp" => ("tactical", "hud_tact_insert"),
        _ => return None,
    })
}

/// (field, value) pairs of `bo2zm_icons`.
fn icon_fields(value: &str) -> Vec<(&str, i32)> {
    value
        .split(',')
        .filter_map(|e| {
            let (f, v) = e.split_once(':')?;
            Some((f, v.trim().parse().unwrap_or(0)))
        })
        .collect()
}

/// A clip from the player's clip table (rows of 12 bytes: the weapon, the
/// right hand's count, the left hand's).
fn clip_of(table: &[u8], weapon: i32, hand: usize) -> i32 {
    table
        .chunks_exact(12)
        .find(|row| i32::from_le_bytes([row[0], row[1], row[2], row[3]]) == weapon)
        .map_or(0, |row| {
            let o = 4 + hand.min(1) * 4;
            i32::from_le_bytes([row[o], row[o + 1], row[o + 2], row[o + 3]])
        })
}

fn read_values(presented: &PresentedSnapshot, local: &LocalPresentClient) -> Option<Values> {
    let snap = presented.snapshot()?;
    let dvars = snap.meta.script_dvars(local.0);
    // (bo2mp: a killcam still plays over the game-over shot: its widget
    // stays, the rest of the HUD goes with BIT_HUD_VISIBLE.)
    let killcam_on = dvars.string("bo2mp_killcam").is_some_and(|k| k.starts_with("1|") || k.starts_with("2|"));
    if dvars.string("bo2zm_hud_hidden") == Some("1") && !killcam_on {
        return None;
    }
    // bo2mp: a multiplayer match has no round.
    let mp = crate::lui_frontend::MatchValues::read(&dvars).map(|mut m| {
        m.rows = crate::lui_frontend::MatchValues::rows_of(&snap.meta);
        m
    });
    let round = match dvars.string("bo2zm_round") {
        Some(r) => r.trim().parse().unwrap_or(0),
        None if mp.is_some() => 0,
        None => return None,
    };
    let meta = snap.meta.for_client(local.0)?;
    let ps = presented.player(local.0);
    Some(Values {
        round,
        score: meta.score,
        weapon: ps.map_or(0, |p| p.weapon as i32),
        clip: meta.ammo_clip,
        clip_left: ps.filter(|p| p.last_weapon_hand == 1).map(|p| clip_of(&p.ammoclip, p.weapon as i32, 1)),
        stock: meta.ammo_stock,
        offhand: dvars.string("bo2zm_offhand").unwrap_or_default().to_owned(),
        icons: dvars.string("bo2zm_icons").unwrap_or_default().to_owned(),
        slots: dvars.string("bo2zm_actionslots").unwrap_or_default().to_owned(),
        streaks: dvars.string("bo2mp_streaks").unwrap_or_default().to_owned(),
        momentum: dvars.string("bo2mp_momentum").and_then(|m| m.trim().parse().ok()).unwrap_or(0),
        weapon_name: String::new(),
        weapon_key: String::new(),
        fire_material: "",
        mp,
    })
}

/// The engine's events for what changed since `old` (everything the first
/// time).
fn send_changes(host: &mut Host, old: Option<&Values>, new: &Values) {
    if old != Some(new) && std::env::var_os("BO2ZM_LUI_LOG").is_some() {
        diag::info!(Ui, "bo2zm lui values at {:.0} ms {new:?}", host.now_ms());
    }
    let changed = |f: fn(&Values) -> i64| old.is_none_or(|o| f(o) != f(new));
    if changed(|v| i64::from(v.round)) {
        host.root_event(
            "hud_update_rounds_played",
            &[("roundsPlayed", Value::Num(new.round as f32)), ("wasDemoJump", Value::Bool(false))],
        );
    }
    if changed(|v| i64::from(v.score)) {
        // One row per player (here the local one, client 0), ours first.
        let rows = hks_t6::value::Table::new_ref();
        let row = hks_t6::value::Table::new_ref();
        row.borrow_mut().set_str("score", Value::Num(new.score as f32));
        row.borrow_mut().set_str("clientNum", Value::Num(0.0));
        rows.borrow_mut().set(Value::Num(1.0), Value::Table(row));
        host.root_event(
            "hud_update_competitive_scoreboard",
            &[
                ("competitivescores", Value::Table(rows)),
                ("selfindex", Value::Num(1.0)),
                ("bWasDemoJump", Value::Bool(false)),
            ],
        );
    }
    if changed(|v| i64::from(v.weapon)) {
        let mut fields = vec![("weapon", Value::str(&new.weapon_name)), ("inventorytype", Value::Num(0.0))];
        if !new.fire_material.is_empty() {
            fields.push(("fireTypeMaterial", host.material(new.fire_material)));
        }
        host.root_event(
            "hud_update_weapon",
            &fields,
        );
        // A switch shows the new gun's name (not the first one he spawns with).
        if old.is_some() && !new.weapon_key.is_empty() {
            host.root_event("hud_update_weapon_select", &[("weaponDisplayName", Value::str(&new.weapon_key))]);
        }
    }
    if changed(|v| i64::from(v.clip) | (i64::from(v.stock) << 20) | (i64::from(v.clip_left.unwrap_or(-1)) << 40)) {
        let mut fields = vec![
            ("ammoInClip", Value::Num(new.clip as f32)),
            ("ammoStock", Value::Num(new.stock as f32)),
            ("lowClip", Value::Bool(false)),
        ];
        if let Some(left) = new.clip_left {
            fields.push(("ammoInDWClip", Value::Num(left as f32)));
        }
        host.root_event("hud_update_ammo", &fields);
    }
    if old.is_none_or(|o| o.offhand != new.offhand) {
        // Each slot: { material = <the icon>, ammo = <count> }.
        let mut fields = Vec::new();
        for e in new.offhand.split(';') {
            let Some((w, n)) = e.split_once(':') else { continue };
            let Some((slot, icon)) = offhand_slot(w) else { continue };
            let n: f32 = n.trim().parse().unwrap_or(0.0);
            if n <= 0.0 {
                continue;
            }
            let t = hks_t6::value::Table::new_ref();
            t.borrow_mut().set_str("material", host.material(icon));
            t.borrow_mut().set_str("ammo", Value::Num(n));
            fields.push((slot, Value::Table(t)));
        }
        host.root_event("hud_update_offhand", &fields);
    }
    if old.is_none_or(|o| o.slots != new.slots || o.offhand != new.offhand) {
        // actionSlotData[slot] = { material, ammo, aspectRatio }.
        let data = hks_t6::value::Table::new_ref();
        for e in new.slots.split(';') {
            let Some((n, w)) = e.split_once(':') else { continue };
            let (Ok(n), Some(icon)) = (n.trim().parse::<f32>(), slot_icon(w)) else { continue };
            let ammo: f32 = new
                .offhand
                .split(';')
                .find_map(|o| o.split_once(':').filter(|(k, _)| *k == w).and_then(|(_, c)| c.trim().parse().ok()))
                .unwrap_or(0.0);
            let t = hks_t6::value::Table::new_ref();
            t.borrow_mut().set_str("material", host.material(icon));
            t.borrow_mut().set_str("ammo", Value::Num(ammo));
            t.borrow_mut().set_str("aspectRatio", Value::Num(1.0));
            data.borrow_mut().set(Value::Num(n), Value::Table(t));
        }
        host.root_event("hud_update_actionslots", &[("actionSlotData", Value::Table(data))]);
    }
    if !new.streaks.is_empty() && old.is_none_or(|o| o.streaks != new.streaks || (o.momentum >= 0 && o.momentum != new.momentum)) {
        // bo2mp: the scorestreak column: each pick { slotIndex, material,
        // ammo (one once its cost is in), momentumCost }, then the bar.
        let mut items = Vec::new();
        for (i, e) in new.streaks.split(';').enumerate() {
            let Some((icon, cost)) = e.split_once(':') else { continue };
            let cost: i32 = cost.trim().parse().unwrap_or(0);
            let t = hks_t6::value::Table::new_ref();
            t.borrow_mut().set_str("slotIndex", Value::Num((i + 1) as f32));
            t.borrow_mut().set_str("material", host.material(icon));
            t.borrow_mut().set_str("ammo", Value::Num(if new.momentum >= cost { 1.0 } else { 0.0 }));
            t.borrow_mut().set_str("momentumCost", Value::Num(cost as f32));
            items.push(Value::Table(t));
        }
        host.root_event_list("hud_update_rewards", items);
        host.root_event("hud_update_momentum", &[("momentum", Value::Num(new.momentum as f32))]);
    }
    if old.is_none_or(|o| o.icons != new.icons) {
        // A client field's own event (`perk_juggernaut`, ...); one gone
        // from the list is back to 0.
        let now = icon_fields(&new.icons);
        if let Some(o) = old {
            for (f, _) in icon_fields(&o.icons) {
                if !now.iter().any(|(g, _)| *g == f) {
                    host.root_event(f, &[("newValue", Value::Num(0.0))]);
                }
            }
        }
        for (f, v) in now {
            let was = old.and_then(|o| icon_fields(&o.icons).into_iter().find(|(g, _)| *g == f).map(|(_, x)| x));
            if was != Some(v) {
                host.root_event(f, &[("newValue", Value::Num(v as f32)), ("oldValue", Value::Num(was.unwrap_or(0) as f32))]);
            }
        }
    }
}

/// Text without Black Ops II's colour codes (`^1`..`^9`) and button
/// pictures (`^BBUTTON_CYCLE_LEFT^`: a pad glyph, not drawn yet).
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c == '^' && it.peek().is_some_and(char::is_ascii_digit) {
            it.next();
            continue;
        }
        if c == '^' && it.peek() == Some(&'B') {
            // A button picture: the tab arrows are drawn (one private-use
            // letter each, see `ARROW_ICONS`); a pad glyph is not.
            let mut name = String::new();
            for d in it.by_ref() {
                if d == '^' {
                    break;
                }
                name.push(d);
            }
            let arrow = match name.as_str() {
                "BBUTTON_CYCLE_LEFT" | "BBUTTON_CYCLE_LEFT_ACTIVE" => Some(0),
                "BBUTTON_CYCLE_RIGHT" | "BBUTTON_CYCLE_RIGHT_ACTIVE" => Some(1),
                _ => None,
            };
            if let Some(i) = arrow {
                out.push(char::from_u32(0xE100 + i).unwrap_or('?'));
            } else if let Some(entry) = name.strip_prefix('B').filter(|n| !n.is_empty() && !n.contains('_')) {
                // A FontIcon entry's own name (the Controls rows' mouse
                // buttons): kept between two private-use letters for
                // `icon_pieces` to look up in BO2's icon set.
                out.push(GLYPH_OPEN);
                out.push_str(entry);
                out.push(GLYPH_CLOSE);
            }
            continue;
        }
        out.push(c);
    }
    out.trim().to_owned()
}

#[allow(clippy::too_many_arguments)]
fn lui_hud(
    mut commands: Commands,
    mut slot: NonSendMut<LuiHudSlot>,
    ui: Option<Res<assets::T6Ui>>,
    fonts: Option<Res<Bo2Fonts>>,
    icons: Option<Res<assets::T6HudIcons>>,
    mut image_assets: ResMut<Assets<Image>>,
    presented: Option<Res<PresentedSnapshot>>,
    local: Option<Res<LocalPresentClient>>,
    (window, time, world_cam): (
        Query<&Window, With<PrimaryWindow>>,
        Res<Time>,
        Query<Entity, (With<Camera3d>, With<bevy::camera::CompositingSpace>, Without<crate::UiCamera>)>,
    ),
    (keys, weapons, actions, cursor): (
        Option<Res<frame::HudInputView>>,
        Option<Res<assets::PreparedWeapons>>,
        Option<Res<net::ClientActionInput>>,
        Option<Res<net::LocationCursor>>,
    ),
    mut io: LuiIo,
    roots: Query<(Entity, &LuiRoot)>,
    mut nodes: Query<(Entity, &mut LuiNode, &mut Node, &mut ZIndex)>,
    mut image_nodes: Query<&mut ImageNode>,
    (children, computed): (Query<&Children>, Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility)>),
    (mut pictures, mut logged, mut additive, mut add_names, mut color_add_names): (
        Local<HashMap<String, Handle<Image>>>,
        Local<usize>,
        ResMut<Assets<crate::lui_additive::AdditiveUi>>,
        Local<Option<std::collections::HashSet<String>>>,
        Local<std::collections::HashSet<String>>,
    ),
) {
    if !lui_enabled() {
        return;
    }
    // bo2mp scope: the pictures kept for additive drawing (see lui_additive).
    if add_names.is_none() && icons.is_some() {
        *add_names = Some(
            icons
                .iter()
                .flat_map(|i| i.0.iter())
                .filter_map(|(k, _)| k.strip_prefix("additive:").map(str::to_owned))
                .collect(),
        );
        // bo2mp vehicle screens: the ones BO2 draws with sw4_2d_color_add.
        *color_add_names = icons
            .iter()
            .flat_map(|i| i.0.iter())
            .filter_map(|(k, _)| k.strip_prefix("coloradd:").map(str::to_owned))
            .collect();
    }
    // bo2mp: the front end has no game to read; its menus run alone.
    let front = io.front.as_deref().cloned();
    // bo2mp: the match he started is loading: the front end's scripts run
    // BO2's own loading screen until the match's HUD takes over.
    let bo2_loading = front.is_none()
        && io.loading.is_some()
        && io.setup.is_some()
        && slot.0.as_ref().is_some_and(|h| h.mp.is_some() && !h.match_mp);
    let values = match (presented.as_deref(), local.as_deref()) {
        _ if front.is_some() || bo2_loading => Some(Values::default()),
        (Some(p), Some(l)) => read_values(p, l),
        _ => None,
    };
    // The held weapon's names from the weapon table.
    let values = values.map(|mut v| {
        if let Some(w) = weapons.as_deref() {
            // bo2mp: a gun with a sight attachment (`svu_mp+acog`) is its
            // base gun's name here.
            let name = w.0.name_of(v.weapon as u32);
            v.weapon_name = name.split('+').next().unwrap_or(name).trim_end_matches("_mp").to_owned();
            v.weapon_key = w.0.display_name_key_of(v.weapon as u32).unwrap_or_default().to_owned();
            // bo2mp: its fire mode's picture (FULL-AUTO, SINGLE, BURST, ..).
            v.fire_material = w.0.facts_of(v.weapon as u32).map_or("", |f| match f.fire_type {
                0 => "hud_mp_firerate_auto",
                1 if f.bolt_action => "hud_mp_firerate_bolt",
                1 => "hud_mp_firerate_single",
                _ => "hud_mp_firerate_burst",
            });
            // bo2mp: looking through a scope (its overlay is up).
            if let (Some(m), Some(ps)) = (
                v.mp.as_mut(),
                presented.as_deref().zip(local.as_deref()).and_then(|(p, l)| p.player(l.0)),
            ) {
                m.scoped = ps.f_weapon_pos_frac >= 1.0 && w.0.overlay_is_hud_iris(ps.weapon);
            }
        }
        v
    });
    let (Some(ui), Some(values)) = (ui, values) else {
        for (e, _) in &roots {
            commands.entity(e).try_despawn();
        }
        let idle = frame::LuiMenus::default();
        if *io.menus != idle {
            *io.menus = idle;
        }
        if io.paused.0 {
            io.paused.0 = false;
        }
        if io.menu_dims.0 != 0 {
            io.menu_dims.0 = 0;
        }
        // Test aid: a match's `shot:` steps keep their time while its HUD is
        // hidden (the outcome, before the scoreboard comes back).
        if let Some(hud) = slot.0.as_mut()
            && hud.match_mp
            && !MATCH_STEPS_DONE.load(std::sync::atomic::Ordering::Relaxed)
            && let Ok(plan) = std::env::var("BO2MP_MATCH_STEPS")
        {
            let now = time.elapsed_secs_f64() * 1000.0 - hud.opened_at;
            let count = plan.split(',').count();
            if let Some((button, at)) = plan.split(',').nth(hud.test_step).and_then(|s| s.split_once('@'))
                && hud.test_step >= 1
                && button.trim().starts_with("shot:")
                && at.trim().parse::<f64>().is_ok_and(|t| now >= t)
            {
                hud.test_step += 1;
                if hud.test_step >= count {
                    MATCH_STEPS_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                diag::info!(Ui, "bo2zm lui test: {button} (hud hidden)");
                let text = format!("screenshot bo2mp/{}", button.trim().trim_start_matches("shot:"));
                io.exec.write(frame::UiExecCommand { text });
            }
        }
        return;
    };
    // (Re)build when the map's scripts arrive.
    // (bo2mp: and when the front end hands over to a match.)
    let match_spec = values.mp.as_ref().filter(|_| front.is_none()).map(|m| MatchSpec {
        gametype: m.gametype.clone(),
        map: io.identity.as_deref().map(|i| i.zone.rsplit(':').next().unwrap_or("").to_owned()).unwrap_or_default(),
        stats: crate::lui_frontend::stats_path(io.identity.as_deref()),
    });
    if !bo2_loading && slot.0.as_ref().is_none_or(|h| {
        h.built_from != ui.scripts.len()
            || (h.mp.is_some() && !h.match_mp) != front.is_some()
            // (a match HUD stays one: a moment without its values does
            // not rebuild it as the zombies HUD)
            || (match_spec.is_some() && !h.match_mp)
    }) && !ui.scripts.is_empty()
    {
        let t = std::time::Instant::now();
        let hud = build(&ui, front.as_ref(), match_spec.as_ref());
        diag::info!(
            Ui,
            "bo2zm lui: {} scripts, base + T6.HUD in {:.0} ms; {} script errors; missing modules {:?}",
            ui.scripts.len(),
            t.elapsed().as_secs_f64() * 1000.0,
            hud.host.errors.len(),
            hud.host.missing.borrow()
        );
        slot.0 = Some(hud);
        pictures.clear();
        for (e, _) in &roots {
            commands.entity(e).try_despawn();
        }
        // (bo2mp: the old root goes this frame; draw from the next.)
        return;
    }
    let Some(hud) = slot.0.as_mut() else {
        return;
    };
    // Font metrics for the scripts' text measure, once the fonts load.
    if hud.metrics.borrow().is_empty()
        && let Some(f) = fonts.as_deref()
    {
        *hud.metrics.borrow_mut() = f.advances();
    }
    // The keys his commands are bound to (the d-pad's key prompts).
    if let Some(k) = keys.as_deref() {
        let mut v = hud.host.values.borrow_mut();
        if v.binds.len() != k.binding_keys_all.len()
            || k.binding_keys_all.iter().any(|(c, keys)| v.binds.get(c) != Some(keys))
        {
            v.binds = k.binding_keys_all.iter().map(|(c, keys)| (c.clone(), keys.clone())).collect();
        }
    }
    // IW4L_HEADLESS=1: no window. The scripts still run, on BO2's own
    // 1280x720 screen with no pointer, so their errors and engine calls
    // still reach the log; nothing is drawn.
    let headless = frame::Headless::requested();
    let (ww, wh, pointer) = match window.single() {
        Ok(win) => (win.width(), win.height(), win.cursor_position()),
        Err(_) if headless => (1280.0, 720.0, None),
        Err(_) => return,
    };
    let aspect = ww / wh.max(1.0);
    let scale = wh / 720.0;
    if (hud.aspect - aspect).abs() > 1e-3 {
        hud.aspect = aspect;
        hud.host.root(aspect);
    }
    if !hud.opened {
        hud.opened = true;
        hud.opened_at = time.elapsed_secs_f64() * 1000.0;
        if front.is_some() {
            crate::lui_frontend::open(&mut hud.host, hud.mp.as_ref(), io.lobby_return.as_deref());
            commands.remove_resource::<crate::lui_frontend::LobbyReturn>();
        } else if hud.match_mp {
            // (BO2's HUD builds its scoreboard now, column headers and team
            // blocks: the game type's columns and settings go in first.)
            if let (Some(m), Some(state)) = (values.mp.as_ref(), hud.mp.clone()) {
                crate::lui_frontend::match_prime(&mut hud.host, &state, m);
            }
            crate::lui_frontend::match_open(&mut hud.host);
        } else {
            hud.host.open_menu("HUD");
            hud.host.root_event("first_snapshot", &[]);
            hud.host.root_event("hud_update_refresh", &[]);
        }
    }
    if bo2_loading && !hud.loading_open {
        hud.loading_open = true;
        if let Some(setup) = io.setup.as_deref() {
            crate::lui_frontend::open_loading(&mut hud.host, setup);
        }
    }
    if front.is_none() && !bo2_loading {
        send_changes(&mut hud.host, hud.sent.as_ref(), &values);
    }
    // bo2mp: BO2's scoreboard open while his scores key (+scores, Tab) is
    // held (the engine opens it; its scripts fill it).
    if hud.match_mp {
        // (And up for good once the game has ended and his HUD is back, the
        // intermission: setmatchflag "game_ended".)
        let raw_ended = presented
            .as_deref()
            .zip(local.as_deref())
            .and_then(|(p, l)| p.snapshot().map(|s| s.meta.script_dvars(l.0).string("bo2mp_game_ended") == Some("1")))
            .unwrap_or(false);
        hud.ended_frames = if raw_ended { hud.ended_frames + 1 } else { 0 };
        // (his board waits a moment for the HUD to settle after the script
        // gives it back; the rest of the HUD goes at once)
        let game_ended = raw_ended && hud.ended_frames > 12;
        if let Some(m) = hud.mp.as_ref() {
            m.borrow_mut().game_ended = game_ended;
        }
        // (bo2mp: the final killcam plays before the end scoreboard: BO2
        // shows only the killcam's widget over it, the board comes after.)
        let in_killcam = presented
            .as_deref()
            .zip(local.as_deref())
            .and_then(|(p, l)| {
                p.snapshot().map(|s| s.meta.script_dvars(l.0).string("bo2mp_killcam").is_some_and(|k| k.starts_with("1|") || k.starts_with("2|")))
            })
            .unwrap_or(false);
        hud.calm_frames = if game_ended && !in_killcam { hud.calm_frames + 1 } else { 0 };
        let game_ended_board = hud.calm_frames > calm_limit();
        if game_ended_board && !hud.board_open {
            diag::info!(Ui, "bo2mp scoreboard: game ended, opening it");
        }
        if !game_ended_board {
            hud.end_rows_logged = false;
        }
        let want_board = game_ended_board || actions.as_deref().is_some_and(|a| a.client.kb.scores.active);
        if want_board != hud.board_open {
            hud.board_open = want_board;
            let event = if want_board { "open_scoreboard_menu" } else { "close_scoreboard_menu" };
            hud.host.root_event(event, &[]);
            // (bo2mp: the end scoreboard has no "Killed By" card under it:
            // real/62 shows the death view bare.)
            if want_board
                && game_ended_board
                && let Some(bit) = hud.host.field("CoD", "BIT_IN_KILLCAM").as_num().map(|n| n as i32)
            {
                hud.host.values.borrow_mut().bits.remove(&bit);
                hud.host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
            }
        }
        // (BO2's HUD parts key off the scoreboard's bit: minimap, ammo,
        // crest and the rest go while it is up; the game's end has its own.)
        // (The match goes back to the front end 5 s after the board is first
        // up: BO2's `wait 5.0` before `exitlevel`.)
        if let Some(shown) = io.end_board.as_deref_mut() {
            if game_ended_board && hud.board_open {
                shown.0.get_or_insert_with(std::time::Instant::now);
            } else {
                shown.0 = None;
            }
        }
        let bits = (want_board || raw_ended, raw_ended);
        if bits != hud.scoreboard_bits {
            hud.scoreboard_bits = bits;
            for (name, on) in [("BIT_SCOREBOARD_OPEN", bits.0), ("BIT_GAME_ENDED", bits.1)] {
                let Some(bit) = hud.host.field("CoD", name).as_num().map(|n| n as i32) else { continue };
                {
                    let mut v = hud.host.values.borrow_mut();
                    if on {
                        v.bits.insert(bit);
                    } else {
                        v.bits.remove(&bit);
                    }
                }
                hud.host.root_event(&format!("hud_update_bit_{bit}"), &[("controller", Value::Num(0.0))]);
            }
        }
    }
    // bo2mp: the weapons' kill icons for the kill feed, once.
    if let (Some(state), Some(icons)) = (hud.mp.as_ref().filter(|_| hud.match_mp), icons.as_deref())
        && !state.borrow().kill_icons_loaded
    {
        let mut m = state.borrow_mut();
        for (name, _) in &icons.0 {
            let Some(rest) = name.strip_prefix("killicon:") else { continue };
            let Some((weapon, ratio)) = rest.rsplit_once(':') else { continue };
            let wide = match ratio {
                "1" => 2.0,
                "2" => 4.0,
                _ => 1.0,
            };
            m.kill_icons.insert(weapon.to_owned(), (name.clone(), wide));
        }
        m.kill_icons_loaded = true;
    }
    if let Some(m) = values.mp.as_ref().filter(|_| hud.match_mp) {
        let old = hud.sent.as_ref().and_then(|v| v.mp.as_ref());
        if let Some(state) = hud.mp.clone() {
            crate::lui_frontend::match_events(&mut hud.host, &mut hud.seen, old, m, &state);
        }
    }
    hud.sent = Some(values);
    // BO2ZM_LUI_PAUSE_AT=<ms after the HUD opened>[,<button>@<ms>...]: open
    // the pause menu then, and press pad buttons later (test aid).
    // bo2mp: a multiplayer match runs its own steps (BO2MP_MATCH_STEPS,
    // same form), so the front end's do not replay in it; each plan runs
    // once per process (coming back to the front end does not restart it).
    // BO2MP_RETURN_STEPS: the front end's once back from a match.
    let back = !hud.match_mp && FRONT_BUILDS.load(std::sync::atomic::Ordering::Relaxed) > 1;
    let (plan_var, plan_done) = if hud.match_mp {
        ("BO2MP_MATCH_STEPS", &MATCH_STEPS_DONE)
    } else if back {
        ("BO2MP_RETURN_STEPS", &RETURN_STEPS_DONE)
    } else {
        ("BO2ZM_LUI_PAUSE_AT", &FRONT_STEPS_DONE)
    };
    if let Ok(plan) = std::env::var(plan_var)
        && !plan_done.load(std::sync::atomic::Ordering::Relaxed)
    {
        let now = hud.host.now_ms() - hud.opened_at;
        let count = plan.split(',').count();
        let mut steps = plan.split(',');
        if let Some(at) = steps.next().and_then(|v| v.trim().parse::<f64>().ok())
            && now >= at
            && hud.test_step == 0
        {
            hud.test_step = 1;
            // (The front end's menus are open already; a match's test
            // opens menus by its steps.)
            if front.is_none() && !hud.match_mp {
                hud.host.root_event("open_ingame_menu", &[("menuName", Value::str("class"))]);
            }
        }
        for (i, step) in steps.enumerate() {
            let Some((button, at)) = step.split_once('@') else { continue };
            if hud.test_step == i + 1 && at.trim().parse::<f64>().is_ok_and(|t| now >= t) {
                hud.test_step = i + 2;
                // The plan's last step: done for this process (START MATCH
                // leaves the front end before another frame could see it).
                if hud.test_step >= count {
                    plan_done.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                diag::info!(Ui, "bo2zm lui test: {button}");
                // `click:x:y` = the mouse there, pressed and let go (root
                // units); else a pad button.
                let mut c = button.trim().split(':');
                let kind = c.next();
                if kind == Some("menu") {
                    // `menu:<name>` = the engine opens that menu (Esc's
                    // `class`); `event:<name>` = that root event.
                    let name = c.next().unwrap_or("class");
                    hud.host.root_event("open_ingame_menu", &[("menuName", Value::str(name))]);
                } else if kind == Some("loading") {
                    // `loading` = BO2's loading screen for the lobby's map
                    // and game type, over the front end (its load is too
                    // quick to photograph).
                    let setup = {
                        let v = hud.host.values.borrow();
                        let d = |k: &str, x: &str| v.dvars.get(k).cloned().unwrap_or_else(|| x.to_owned());
                        session::Bo2mpMatchSetup {
                            map: d("ui_mapname", "mp_nuketown_2020"),
                            gametype: d("ui_gametype", "tdm"),
                            ..Default::default()
                        }
                    };
                    crate::lui_frontend::open_loading(&mut hud.host, &setup);
                } else if kind == Some("event") {
                    hud.host.root_event(c.next().unwrap_or(""), &[]);
                } else if kind == Some("shot") {
                    // `shot:<name>` = a screenshot then (bo2mp/<name>).
                    let text = format!("screenshot bo2mp/{}", c.next().unwrap_or("step"));
                    io.exec.write(frame::UiExecCommand { text });
                } else if kind == Some("key") {
                    // `key:TAB` = that key's shortcut (a prompt's key).
                    key_shortcut(&mut hud.host, c.next().unwrap_or(""));
                } else if kind == Some("notify") {
                    // `notify:<event>:<arg>:...` = a LUI notify the server
                    // sends (`luinotifyevent`): the event with its data.
                    let name = c.next().unwrap_or("").to_owned();
                    let data = hks_t6::value::Table::new_ref();
                    for (i, a) in c.enumerate() {
                        let v = a.parse::<f32>().map_or_else(|_| Value::str(a), Value::Num);
                        data.borrow_mut().set(Value::Num((i + 1) as f32), v);
                    }
                    hud.host.root_event(&name, &[("data", Value::Table(data))]);
                } else if kind == Some("move") {
                    // `move:x:y` = the mouse there, no click.
                    let x: f32 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let y: f32 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    hud.host.mouse("mousemove", x, y, "");
                } else if kind == Some("click") {
                    let x: f32 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let y: f32 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    hud.host.mouse("mousemove", x, y, "");
                    hud.host.mouse("mousedown", x, y, "left");
                    hud.host.mouse("mouseup", x, y, "left");
                } else if let Some(b) = button.trim().strip_suffix('+') {
                    // `right+` presses and holds, `right-` lets go.
                    hud.host.button(b, true);
                } else if let Some(b) = button.trim().strip_suffix('-') {
                    hud.host.button(b, false);
                } else {
                    hud.host.button(button.trim(), true);
                    hud.host.button(button.trim(), false);
                }
            }
        }
    }
    // bo2mp: CAMPAIGN's prompt closed for him a few frames on.
    if hud.back_in > 0 {
        hud.back_in -= 1;
        // (BO2's confirm prompts are all `CoD.Menu.NewSmallPopup("SetDefaultPopup")`.)
        if hud.back_in == 0 && !hud.host.close_popup("Menu.SetDefaultPopup") {
            diag::warn!(Ui, "bo2mp front end: CAMPAIGN's prompt was not open to close");
        }
    }
    // His keys and mouse, then what the menus asked of the engine. While
    // a bind row waits for a key, the key is the capture's; when it is
    // done the rows show the new keys (`key_bound`).
    let console_open = keys.as_deref().is_some_and(|k| k.console_open);
    let capturing = io.capture.command.is_some() || io.capture.consumed_input;
    if hud.binding && !capturing {
        hud.binding = false;
        hud.host.root_event("key_bound", &[]);
    }
    if !capturing {
        menu_input(hud, &mut io, console_open, pointer, scale);
    }
    // (Presses the menus did not take are not theirs a frame later.)
    io.key_events.clear();
    engine_calls(hud, &mut io, local.as_deref());
    if let (Some(f), Some(mp)) = (front.as_ref(), hud.mp.clone()) {
        crate::lui_frontend::tick(&hud.host, &mp, f, &mut commands, io.swap.as_deref_mut());
    }
    let monitors = io.monitors.iter().count().max(1);
    {
        let mut v = hud.host.values.borrow_mut();
        v.dvars.insert("r_monitorCount".to_owned(), monitors.to_string());
        v.dvars.entry("r_monitor".to_owned()).or_insert_with(|| "0".to_owned());
    }
    settings_bridge(hud, &mut io.settings);
    let open = !hud.host.open_menus().is_empty();
    // bo2mp: the front end holds his keyboard and mouse (a free, shown
    // pointer) though its main menu is no `Menu.*` child of the root.
    let front_end = io.front.is_some() && !hud.match_mp;
    // (bo2mp: the loading screen holds nothing and pauses nothing: the
    // front end's menus under it are not open for the match.)
    let open = open && !bo2_loading;
    let state = frame::LuiMenus { active: true, open: open || front_end };
    if *io.menus != state {
        *io.menus = state;
    }
    // A solo game pauses under its menus (Black Ops II's solo pause).
    // (bo2mp: a multiplayer match never pauses.)
    let pause = open && io.role.as_deref() == Some(&frame::RuntimeRole::Listen) && !hud.match_mp;
    if io.paused.0 != pause {
        io.paused.0 = pause;
        diag::info!(Ui, "bo2zm lui: game {}", if pause { "paused" } else { "resumed" });
    }
    hud.host.frame(time.elapsed_secs_f64() * 1000.0);
    if hud.host.errors.len() > *logged {
        for e in &hud.host.errors[*logged..] {
            diag::warn!(Ui, "bo2zm lui script error: {e}");
        }
        *logged = hud.host.errors.len();
    }
    // Each engine field a script asked that the host does not answer
    // (once per name): a gap the scripts read as nil.
    {
        static TOLD: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());
        if let Ok(mut told) = TOLD.lock() {
            for k in hud.host.asked.borrow().keys() {
                if !told.contains(k) {
                    diag::info!(Ui, "bo2zm lui: engine field unanswered: {k}");
                    told.insert(k.clone());
                }
            }
        }
    }
    if headless {
        return;
    }
    // BO2ZM_LUI_DUMP=1: every element the scripts have, each time their
    // number changes (debugging aid).
    if std::env::var_os("BO2ZM_LUI_DUMP").is_some() {
        let all = hud.host.drawn();
        if all.len() != hud.dumped {
            hud.dumped = all.len();
            for d in &all {
                diag::info!(
                    Ui,
                    "bo2zm lui dump {}: {} {:?} rect {:?} alpha {:.2} rgb {:?} text {:?} align {} font {:?} behind {} blur {}",
                    d.id,
                    d.kind,
                    d.material,
                    d.rect,
                    d.alpha,
                    d.rgb,
                    d.text,
                    d.alignment,
                    d.font,
                    d.behind,
                    d.blur
                );
            }
        }
    }
    // bo2mp: his minimap this frame (the engine draws BO2's compass).
    let now_ms = hud.host.now_ms();
    let mut shots = std::mem::take(&mut hud.shots);
    let minimap = (hud.match_mp)
        .then(|| {
            let (p, l) = (presented.as_deref()?, local.as_deref()?);
            let m = hud.sent.as_ref()?.mp.as_ref()?;
            let ps = p.player(l.0)?;
            let me = ([ps.origin[0], ps.origin[1]], ps.viewangles[1]);
            let my_team = m.team.clone();
            let friends: Vec<([f32; 2], f32)> = m
                .teams
                .split(',')
                .filter_map(|e| e.split_once(':'))
                .filter(|(c, t)| *t == my_team && c.parse::<u32>().ok() != Some(l.0.0))
                .filter_map(|(c, _)| p.player(sim::ClientId(c.parse().ok()?)))
                .map(|ps| ([ps.origin[0], ps.origin[1]], ps.viewangles[1]))
                .collect();
            // BO2 shows an enemy only while his team's radar (UAV, satellite)
            // is up, or for a moment after he fires; a jammed map shows none.
            let radar: Vec<i32> = m.radar.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            let spotted = radar.first().is_some_and(|v| *v > 0) || radar.get(1).is_some_and(|v| *v > 0);
            let jammed = radar.get(2).is_some_and(|v| *v > 0);
            let enemies: Vec<([f32; 2], f32, bool)> = m
                .teams
                .split(',')
                .filter_map(|e| e.split_once(':'))
                .filter(|(c, t)| *t != my_team && matches!(*t, "allies" | "axis") && c.parse::<u32>().ok() != Some(l.0.0))
                .filter_map(|(c, _)| {
                    let id: u32 = c.parse().ok()?;
                    let es = p.alive_player(sim::ClientId(id))?;
                    // A shot: his weapon is in the firing state (WEAPON_FIRING).
                    let seen = shots.entry(id).or_insert(f64::NEG_INFINITY);
                    if es.weaponstate_primary == 6 {
                        *seen = now_ms;
                    }
                    let firing = now_ms - *seen < 2000.0;
                    (!jammed && (spotted || firing)).then(|| ([es.origin[0], es.origin[1]], es.viewangles[1], firing))
                })
                .collect();
            crate::lui_frontend::MinimapView::of(m, me, friends, enemies)
        })
        .flatten();
    hud.shots = shots;
    let mut drawn: Vec<Drawn> = hud
        .host
        .drawn()
        .into_iter()
        .map(button_glyph)
        .filter(|d| {
            d.alpha > 0.004
                && (d.material.is_some()
                    || d.kind == "image"
                    // bo2mp: `setupLoadingBar`: the engine's loading bar,
                    // two plain-colour strips under the tip box.
                    || d.kind == "loadingbar"
                    || d.text.as_deref().is_some_and(|t| !t.trim().is_empty())
                    || (d.kind == "dashes" && d.dashes.0 > 0)
                    || (minimap.is_some() && d.kind.starts_with("compass_") && !d.kind.starts_with("compass_over")))
                // bo2mp: of the 3D widgets, the globe and the holotable
                // floor (its first grid stands for all three) are drawn by
                // `lui_scene`; the rest not.
                && !(front.is_some()
                    && crate::lui_frontend::is_3d(d)
                    && !(d.kind == "globe" || d.material.as_deref() == Some("ui_holotable_grid")))
                // bo2mp: a menu under a blurring one (`Drawn::behind`) shows
                // blurred and dimmed: its pictures blurred, its text and
                // blocks as soft patches of their size (`soft_rect`).
                // bo2mp: no "Killed By" label under the end scoreboard.
                && !(hud.calm_frames > calm_limit() && d.text.as_deref().is_some_and(|t| t.trim().eq_ignore_ascii_case("killed by")))
        })
        .collect();
    // bo2mp: proof the end scoreboard drew its rows (the gate's check): every
    // player's name drawn on the board, logged once per opening.
    if hud.match_mp
        && hud.board_open
        && hud.calm_frames > calm_limit()
        && !hud.end_rows_logged
        && let Some(m) = hud.mp.as_ref()
    {
        let names: Vec<String> = m.borrow().board.iter().map(|r| r.name.clone()).collect();
        let seen = names
            .iter()
            .filter(|n| !n.is_empty() && drawn.iter().any(|d| d.text.as_deref().is_some_and(|t| t.contains(n.as_str()))))
            .count();
        if !names.is_empty() && seen == names.len() {
            diag::info!(Ui, "bo2mp end scoreboard: rows drawn {seen} of {}", names.len());
            hud.end_rows_logged = true;
        }
    }
    // (bo2mp: the loading screen's root is the loading layer's, over the
    // engine's own loading screen; a root on another layer goes.)
    let root = match roots.iter().next() {
        Some((r, _)) => r,
        None => commands
            .spawn((
                LuiRoot(bo2_loading),
                GlobalZIndex(if bo2_loading { 10_001 } else { 0 }),
                // bo2mp: the front end draws on the menu screen's layer.
                if bo2_loading {
                    UiLayer::Loading
                } else if front.is_some() {
                    UiLayer::Shell
                } else {
                    UiLayer::Hud
                },
                UiLayerVisibility,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ))
            .id(),
    };
    // (A root on the other layer goes with its nodes; draw from the next
    // frame, as nodes under it cannot be touched after its despawn.)
    if roots.iter().any(|(_, r)| r.0 != bo2_loading) {
        for (e, _) in &roots {
            commands.entity(e).try_despawn();
        }
        return;
    }
    let mut existing: HashMap<usize, Entity> = HashMap::new();
    for (e, n, ..) in &nodes {
        existing.insert(n.id, e);
    }
    let mut keep: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let now_s = time.elapsed_secs_f64();
    // bo2mp: a front-end menu over another blurs what is under it
    // (`setBlur`): the 3D backdrop is drawn blurred then.
    // (A menu is no picture or text: its flag is read before those are
    // picked out.)
    let blurred = front.is_some() && hud.host.drawn().iter().any(|d| d.blur && d.alpha > 0.004);
    // bo2mp: an in-match menu (a full-screen black dim is up) is drawn
    // with its colours as written: BO2's menu text and bars show the
    // script's 1.0 / 0.4 / 0.0 orange as 255 / 102 / 0, where the HUD's
    // linear colours go through the target's encode (G 170). The held Tab
    // scoreboard is drawn the same way (its rows' alpha 0.7 reads 0.46).
    let in_match_menu = front.is_none()
        && (hud.board_open || drawn.iter().any(|d| {
            d.kind == "image" && d.rgb == [0.0, 0.0, 0.0] && d.alpha > 0.3 && (d.rect[2] - d.rect[0]).abs() > 1200.0 && (d.rect[3] - d.rect[1]).abs() > 700.0
        }));
    // bo2mp: BO2 blends every multiplayer in-match picture and colour (the
    // match HUD as much as a menu over it) in the sRGB target's gamma space.
    let gamma_ui = in_match_menu || hud.match_mp;
    // bo2mp: BO2 blends its UI over the frame in the sRGB target's gamma
    // (encoded) space. The match world's camera keeps its frame in an
    // `Rgba8Unorm` target (`CompositingSpace::Srgb`), where a UI node's
    // values land as they are and blend as encoded values, so the match
    // UI is drawn onto that camera: colours and pictures go in as written
    // (as in the front end), with none of the linear-blend remaps below.
    // With no such camera (the overlay camera's sRGB target) the old
    // remaps stand.
    let lens = if gamma_ui { world_cam.iter().next() } else { None };
    let enc_ui = lens.is_some();
    UI_ENCODED.store(enc_ui, std::sync::atomic::Ordering::Relaxed);
    if UI_ENCODED_LAST.swap(enc_ui, std::sync::atomic::Ordering::Relaxed) != enc_ui {
        // (The nodes were built for the other target: rebuilt next frame.)
        commands.entity(root).try_despawn();
        return;
    }
    match lens {
        Some(cam) => {
            commands.entity(root).insert(bevy::ui::UiTargetCamera(cam));
        }
        None => {
            commands.entity(root).remove::<bevy::ui::UiTargetCamera>();
        }
    }
    IN_MATCH_MENU_PICS.store(gamma_ui && !enc_ui, std::sync::atomic::Ordering::Relaxed);
    IN_FRONT_END_PICS.store(front.is_some(), std::sync::atomic::Ordering::Relaxed);
    let is_dim = |d: &Drawn| {
        d.kind == "image" && d.rgb == [0.0, 0.0, 0.0] && d.alpha > 0.3 && (d.rect[2] - d.rect[0]).abs() > 1200.0 && (d.rect[3] - d.rect[1]).abs() > 700.0
    };
    let dims = if front.is_none() && !enc_ui { drawn.iter().filter(|d| is_dim(d)).count() } else { 0 };
    // (The count the HUD's own pieces read: the encoded path draws the dims
    // as UI, so `dims` is 0 there and cannot say a popup is up.)
    let stacked = if front.is_none() { drawn.iter().filter(|d| is_dim(d)).count() as u32 } else { 0 };
    // bo2mp: two or more black dims stacked over the match world (the pause
    // menu's, then a popup's): the target keeps eight bits of linear light,
    // so the second dim leaves the world at 0 (BO2's gamma blend leaves it
    // near 3). The world pass (`bo2_bloom`) applies the dims itself; here the
    // dims are not drawn, and what lies under a dim (as BO2 draws it) is
    // scaled by what the dims over it let through, which is the same blend.
    if dims >= 2 && in_match_menu {
        let alphas: Vec<(usize, f32)> = drawn.iter().enumerate().filter(|(_, d)| is_dim(d)).map(|(i, d)| (i, d.alpha.clamp(0.0, 1.0))).collect();
        let mut kept: Vec<Drawn> = Vec::with_capacity(drawn.len());
        for (i, d) in drawn.iter().enumerate() {
            if is_dim(d) {
                continue;
            }
            let f: f32 = alphas.iter().filter(|(k, _)| *k > i).map(|(_, a)| 1.0 - a).product();
            let mut d = d.clone();
            if f < 1.0 {
                d.rgb = [d.rgb[0] * f, d.rgb[1] * f, d.rgb[2] * f];
            }
            kept.push(d);
        }
        drawn = kept;
    }
    if io.menu_dims.0 != stacked {
        io.menu_dims.0 = stacked;
    }
    if io.dim.0 != dims as f32 {
        diag::info!(Ui, "bo2mp: {dims} full-screen black dims over the world");
        io.dim.0 = dims as f32;
    }
    let behind_up = drawn.iter().any(|d| d.behind);
    // bo2mp: under a blurring menu the whole backdrop (picture, globe, floor)
    // shows at about a quarter of its level, flat and neutral: real's blurred
    // shot has no hex mesh, ring or warm tint, and its left and right edges
    // match. The menu's own black 0.15 dim is raised to leave that share;
    // the backdrop picture is blurred.
    // (The blur flag sits on a menu container that is no picture or text, so it is read
    // from the host's full list, not from `drawn`.)
    let blur_amt = hud.host.drawn().iter().filter(|d| d.blur).map(|d| d.alpha).fold(0.0f32, f32::max).clamp(0.0, 1.0);
    let is_backdrop_dim = |d: &Drawn| {
        d.kind == "image"
            && d.material.is_none()
            && d.rgb == [0.0, 0.0, 0.0]
            && d.alpha <= 0.3
            && (d.rect[2] - d.rect[0]).abs() > 1200.0
            && (d.rect[3] - d.rect[1]).abs() > 700.0
    };
    // bo2mp: real's rule for what a blurring menu leaves under itself (one
    // rule for every menu that covers another: `updateBlur` calls
    // `setBlur(true)` on each alike, no menu is named): everything below it,
    // the backdrop and the covered menu's content together, shows blurred at
    // one share of its own level (real's blurred shots read 0.20 to 0.25 of
    // the unblurred menu's, whether the Quit prompt or Setup Bots covers it).
    // The share is one dim over the whole covered stack, after the last
    // covered element (the menu's own pictures follow it); before this the
    // dim sat under the covered pictures, which then needed a scale per menu.
    let dim_at = if blurred && front.is_some() {
        drawn.iter().rposition(|d| d.behind).map(|i| i + 1).or_else(|| drawn.iter().position(|d| is_backdrop_dim(d)).map(|i| i + 1))
    } else {
        None
    };
    if let Some(at) = dim_at {
        drawn.insert(
            at,
            Drawn {
                id: usize::MAX - 20,
                kind: "image",
                rect: [0.0, 0.0, hud.host_width(), 720.0],
                // (The blur's intensity: `ui_blur_vignette` makes the picture from it.)
                alpha: blur_amt,
                rgb: [0.0, 0.0, 0.0],
                material: None,
                text: None,
                font: None,
                alignment: 2,
                z_rot: 0.0,
                x_rot: 0.0,
                y_rot: 0.0,
                dashes: (0, 0, 0),
                dash_pitch: 0.0,
                tiles: 0.0,
                shader: [0.0; 4],
                blur: false,
                behind: false,
                    clip: None,
            },
        );
    }
    let first_blur = dim_at.unwrap_or(usize::MAX);
    for (order, d) in drawn.iter().enumerate() {
        let [x0, y0, x1, y1] = d.rect;
        // The scripts' colours are linear (BO2 draws into an sRGB target:
        // the chalk's 0.21 red shows as a mid blood red).
        let color = if enc_ui {
            Color::linear_rgba(d.rgb[0], d.rgb[1], d.rgb[2], d.alpha.clamp(0.0, 1.0))
        } else if gamma_ui {
            // bo2mp: BO2 blends in the sRGB target's gamma space; Bevy
            // blends in linear light, which lifts every see-through
            // thing (text at alpha 0.5 reads 186, BO2's 128; the dim
            // pips, the tile backs, the perk pictures, the focused
            // button's pulse low). Over the dark dimmed world the alpha
            // is remapped (a^2.2) so the encoded result is the gamma
            // blend's. The black dim (own remap), popups and the name
            // card's back (own scales) are left alone.
            let a = d.alpha.clamp(0.0, 1.0);
            let own = d.rgb == [0.0, 0.0, 0.0]
                || d.kind == "dashes"
                || d.material.as_deref().is_some_and(|m| m.starts_with("menu_mp_popup") || m.starts_with("menu_mp_dots") || m.starts_with("emblem_bg"));
            let a = if own { a } else { a.powf(2.2) };
            Color::srgba(d.rgb[0], d.rgb[1], d.rgb[2], a)
        } else {
            Color::linear_rgba(d.rgb[0], d.rgb[1], d.rgb[2], d.alpha.clamp(0.0, 1.0))
        };
        let color = if d.behind { color.with_alpha(color.alpha() * behind_dim()) } else { color };
        let backdrop_pic = blurred && front.is_some() && order < first_blur && d.kind == "image" && !d.behind && d.material.as_deref().is_some_and(|m| m.starts_with("lui_bkg"));
        let z = ZIndex(order as i32 + 1);
        if front.is_some() && crate::lui_frontend::is_3d(d) {
            // bo2mp: the 3D backdrop, drawn here (`lui_scene`): the floor
            // over the whole root, once per width; the globe in its
            // rectangle, turned on ten times a second.
            let Some(icons) = icons.as_deref() else { continue };
            let tex = |name: &str| icons.0.iter().find(|(k, _)| k == name).map(|(_, i)| i.clone());
            let globe = d.kind == "globe";
            // bo2mp: under Options / the Quit prompt the covered main menu's
            // soldiers stand over the backdrop: BO2's blurred shot has no
            // globe in it.
            // bo2mp: the globe is drawn once its map is in (the scripts
            // set its shader vector to 2 when it is: the main menu's stays
            // 0 and BO2 shows nothing there).
            if globe && (behind_up || blurred || d.shader[0] < 1.5) {
                continue;
            }
            let key = if globe { "\u{1}bo2mp globe".to_owned() } else { format!("\u{1}bo2mp floor {:.0}", hud.host_width()) };
            let key = if blurred { format!("{key} blurred") } else { key };
            let due = if globe { now_s - hud.scene_at >= 0.1 } else { now_s - hud.floor_at >= 0.25 };
            if pictures.get(&key).is_none_or(|h| image_assets.get(h).is_none()) || due {
                let made = if globe {
                    let (Some(map), mesh) = (tex("scene:globe_map"), tex("scene:globe_map_mesh")) else { continue };
                    let (Some(map), mesh) = (crate::lui_scene::Tex::of(&map), mesh.as_deref().and_then(crate::lui_scene::Tex::of)) else {
                        continue;
                    };
                    hud.scene_at = now_s;
                    let n = 384;
                    let spin = std::env::var("BO2ZM_GLOBE_SPIN").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(115.0 + now_s as f32 * 7.16);
                    let mut px = crate::lui_scene::globe(n, &map, mesh.as_ref(), spin, 23.5);
                    if blurred {
                        crate::lui_scene::blur(&mut px, n, n, ((blur_sigma() * n as f32 / (x1 - x0).abs().max(1.0)).round() as usize).max(1));
                    }
                    crate::lui_scene::image(n, n, px)
                } else {
                    let (Some(a), Some(b), Some(c)) = (tex("scene:plus_tile"), tex("scene:plus_grid2"), tex("scene:plus_grid3")) else {
                        continue;
                    };
                    let (Some(a), Some(b), Some(c)) =
                        (crate::lui_scene::Tex::of(&a), crate::lui_scene::Tex::of(&b), crate::lui_scene::Tex::of(&c))
                    else {
                        continue;
                    };
                    let tv = tex("scene:tv_lookup");
                    let tv = tv.as_deref().and_then(crate::lui_scene::Tex::of);
                    hud.floor_at = now_s;
                    let w = hud.host_width();
                    let (pw, ph) = ((w * 0.5) as usize, 270);
                    let mut px = crate::lui_scene::floor(w, pw, ph, now_s as f32, tv.as_ref(), &a, &b, &c);
                    if blurred {
                        crate::lui_scene::blur(&mut px, pw, ph, ((blur_sigma() * 0.5).round() as usize).max(1));
                    }
                    crate::lui_scene::image(pw, ph, px)
                };
                match pictures.get(&key) {
                    Some(h) if image_assets.get(h).is_some() => {
                        let h = h.clone();
                        image_assets.insert(&h, made).ok();
                    }
                    _ => {
                        let h = image_assets.add(made);
                        pictures.insert(key.clone(), h);
                    }
                }
            }
            let Some(handle) = pictures.get(&key).cloned() else { continue };
            keep.insert(d.id);
            // bo2mp: the real globe sits 24 units left of the widget's rectangle
            // (its right limb at 523, not 547) and a little smaller (R 354).
            let r = if globe {
                let (cx, cy) = ((x0 + x1) * 0.5 - 22.0, (y0 + y1) * 0.5);
                let (hw, hh) = ((x1 - x0).abs() * 0.5 * 0.985 * crate::lui_scene::GLOBE_PAD, (y1 - y0).abs() * 0.5 * 0.985 * crate::lui_scene::GLOBE_PAD);
                [cx - hw, cy - hh, cx + hw, cy + hh]
            } else {
                [0.0, 0.0, hud.host_width(), 720.0]
            };
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(r[0] * scale),
                top: Val::Px(r[1] * scale),
                width: Val::Px((r[2] - r[0]).abs() * scale),
                height: Val::Px((r[3] - r[1]).abs() * scale),
                ..default()
            };
            let shows = format!("scene {key}");
            if let Some(&e) = existing.get(&d.id)
                && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
                && n.shows == shows
            {
                *nn = node;
                *zi = z;
                if let Ok(mut img) = image_nodes.get_mut(e) {
                    img.color = Color::linear_rgba(1.0, 1.0, 1.0, color.alpha());
                }
                continue;
            }
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let e = commands
                .spawn((
                    LuiNode { id: d.id, shows, alpha: color.alpha() },
                    node,
                    ImageNode {
                        image: handle,
                        color: Color::linear_rgba(1.0, 1.0, 1.0, color.alpha()),
                        image_mode: bevy::ui::widget::NodeImageMode::Stretch,
                        ..default()
                    },
                    z,
                ))
                .id();
            commands.entity(root).add_child(e);
        } else if d.kind == "dashes" {
            // A slider's or attribute bar's pips, as BO2's engine draws them
            // from `setupDashes(count, filled, change, pitch)`: one engine
            // material per pip, the element's height square - the grey
            // filled pip (menu_mp_pip_blue) for each lit one, the hollow
            // outline (menu_mp_pip_outline) for the rest, and an
            // attachment's change in the green pip (better) or the red
            // pip (worse), the way CoD.AttributeBar reads its change
            // (PositiveBarColor / NegativeBarColor): positive adds that many
            // green pips after the lit ones, negative turns that many of the
            // lit ones' tail red.
            // A pip's picture is the element's own box (the slider's bar is
            // 16 wide by 32, the attribute bar 32 by 32), stepped along by
            // the pitch, as the script asks (sliders 8, Create-a-Class's bars
            // 18). Logged gap, not a rule: his real game's Create-a-Class
            // shots show about 14 for the 18 asked; no script, scale or
            // unit in the zone explains it (the engine's native
            // setupDashes is not in the scripts), so no fitted number here.
            // The first pip sits where the picture's own square does
            // (texel 10 of 32), no extra inset.
            let (count, lit, change) = d.dashes;
            let names = ["menu_mp_pip_blue", "menu_mp_pip_outline", "menu_mp_pip_green", "menu_mp_pip_red"];
            let mut handles = Vec::with_capacity(4);
            for n in names {
                match picture(&mut pictures, icons.as_deref(), &mut image_assets, n) {
                    Some(h) => handles.push(h),
                    None => break,
                }
            }
            if handles.len() < 4 {
                continue;
            }
            keep.insert(d.id);
            let pitch = d.dash_pitch;
            let qh = (y1 - y0).abs();
            let qw = (x1 - x0).abs();
            let (pitch, qw, qh) = (pitch * scale, qw * scale, qh * scale);
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x0 * scale),
                top: Val::Px(y0.min(y1) * scale),
                width: Val::Px(pitch * count as f32),
                height: Val::Px(qh),
                ..default()
            };
            let shows = format!("dashes {count} {lit} {change} {:.2} {:.2}", d.dash_pitch, d.alpha);
            if let Some(&e) = existing.get(&d.id)
                && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
                && n.shows == shows
            {
                *nn = node;
                *zi = z;
                continue;
            }
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let a = color.alpha();
            let e = commands
                .spawn((LuiNode { id: d.id, shows, alpha: a }, node, z))
                .with_children(|c| {
                    for i in 0..count {
                        let which = if change >= 0 {
                            if i < lit {
                                0
                            } else if i < (lit + change).min(count) {
                                2
                            } else {
                                1
                            }
                        } else if i < lit + change {
                            0
                        } else if i < lit {
                            3
                        } else {
                            1
                        };
                        c.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(pitch * i as f32),
                                top: Val::Px(0.0),
                                width: Val::Px(qw),
                                height: Val::Px(qh),
                                ..default()
                            },
                            ImageNode {
                                image: handles[which].clone(),
                                color: Color::WHITE.with_alpha(a),
                                image_mode: bevy::ui::widget::NodeImageMode::Stretch,
                                ..default()
                            },
                        ));
                    }
                })
                .id();
            commands.entity(root).add_child(e);
        } else if let (Some(view), "compass", Some(material)) = (minimap.as_ref(), d.kind, d.material.as_deref()) {
            // bo2mp: the strip under the minimap: its picture (ticks and
            // N E S W, 360 degrees across) scrolled to his heading, 0.61 of
            // a pixel to the degree as BO2 draws it, the wrap in two pieces.
            keep.insert(d.id);
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let Some(handle) = picture(&mut pictures, icons.as_deref(), &mut image_assets, material) else { continue };
            let Some(size) = image_assets.get(&handle).map(|i| i.size_f32()) else { continue };
            let (w, h) = ((x1 - x0).abs() * scale, (y1 - y0).abs() * scale);
            let shown = (w / (0.61 * scale) / 360.0 * size.x).min(size.x);
            let centre = (view.heading(view.me.1).to_degrees().rem_euclid(360.0)) / 360.0 * size.x;
            let lo = centre - shown * 0.5;
            // The picture repeats (it is 360 degrees round): a slice that runs
            // past either end just reads on from the other, no wrap pieces.
            if let Some(img) = image_assets.get(&handle)
                && !matches!(&img.sampler, bevy::image::ImageSampler::Descriptor(d) if d.address_mode_u == bevy::image::ImageAddressMode::Repeat)
                && let Some(mut img) = image_assets.get_mut(&handle)
            {
                img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
                    address_mode_u: bevy::image::ImageAddressMode::Repeat,
                    address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
                    ..bevy::image::ImageSamplerDescriptor::linear()
                });
            }
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x0 * scale),
                top: Val::Px(y0 * scale),
                width: Val::Px(w),
                height: Val::Px(h),
                overflow: Overflow::clip(),
                ..default()
            };
            let e = commands
                .spawn((LuiNode { id: d.id, shows: String::new(), alpha: color.alpha() }, node, z))
                .with_children(|c| {
                    // BO2 fades the strip's ticks and letters out towards both
                    // ends (measured on its shots: full to 35 units from the
                    // middle, none by 75, the strip being 148 across): drawn in
                    // 2-unit slices, each at its own alpha.
                    // Slices of whole screen pixels, each reading exactly the
                    // picture span under it (the old slices took the pieces' own
                    // fractional widths, which the layout rounded to pixels
                    // while their picture spans stayed exact: the letters
                    // landed 52 and 56 px apart, real is 55).
                    let step = (2.0 * scale).round().max(1.0);
                    let n = (w / step).ceil() as usize;
                    for i in 0..n {
                        let (x0s, x1s) = (i as f32 * step, ((i + 1) as f32 * step).min(w));
                        let d = ((x0s + x1s) * 0.5 - w * 0.5).abs() / scale;
                        let fade = (1.0 - (d - 35.0) / 40.0).clamp(0.0, 1.0);
                        if fade <= 0.0 {
                            continue;
                        }
                        let mut img = ImageNode { image: handle.clone(), color: color.with_alpha(color.alpha() * fade), ..default() };
                        img.rect = Some(Rect::new(lo + x0s / w * shown, 0.0, lo + x1s / w * shown, size.y));
                        c.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(x0s),
                                top: Val::Px(0.0),
                                width: Val::Px(x1s - x0s),
                                height: Val::Px(h),
                                ..default()
                            },
                            img,
                        ));
                    }
                })
                .id();
            commands.entity(root).add_child(e);
        } else if let (Some(view), true) = (minimap.as_ref(), d.kind.starts_with("compass_")) {
            // bo2mp: BO2's minimap, redrawn every frame. The HUD's (type
            // PARTIAL): the map around him turned so he faces up (made on
            // the CPU: the 2D layer cannot clip a turned picture), his arrow
            // in the middle, his team's around him. The pause menu's (FULL):
            // the whole map, north up, everyone where they are.
            keep.insert(d.id);
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let full = d.kind.ends_with("_full");
            let (w, h) = ((x1 - x0).abs() * scale, (y1 - y0).abs() * scale);
            let px_per_unit = w / view.range;
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x0 * scale),
                top: Val::Px(y0 * scale),
                width: Val::Px(w),
                height: Val::Px(h),
                overflow: Overflow::clip(),
                ..default()
            };
            let mut kids: Vec<(Node, ImageNode, UiTransform)> = Vec::new();
            let mut real_backing = false;
            // The whole map's square inside the widget (FULL).
            let fit = w.min(h);
            let (fx0, fy0) = ((w - fit) * 0.5, (h - fit) * 0.5);
            let flat = |left: f32, top: f32, width: f32, height: f32| Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
                top: Val::Px(top),
                width: Val::Px(width),
                height: Val::Px(height),
                ..default()
            };
            if d.kind.starts_with("compass_under") {
                if full {
                    let Some(handle) = picture(&mut pictures, icons.as_deref(), &mut image_assets, &view.material) else {
                        continue;
                    };
                    kids.push((flat(fx0, fy0, fit, fit), ImageNode { image: handle, color, ..default() }, UiTransform::IDENTITY));
                } else {
                    let Some(src) = icons.as_deref().and_then(|i| i.0.iter().find(|(k, _)| *k == view.material)).map(|(_, i)| i.clone())
                    else {
                        continue;
                    };
                    let (sw, sh) = (src.texture_descriptor.size.width as usize, src.texture_descriptor.size.height as usize);
                    let n = (w.round() as usize).clamp(32, 320);
                    let pixels = src.data.as_deref().map(|d| view.render(d, sw, sh, n)).unwrap_or_default();
                    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
                    let size = Extent3d { width: n as u32, height: n as u32, depth_or_array_layers: 1 };
                    let mut made =
                        Image::new(size, TextureDimension::D2, pixels, TextureFormat::Rgba8Unorm, bevy::asset::RenderAssetUsages::default());
                    made.sampler = bevy::image::ImageSampler::linear();
                    // One picture, rewritten every frame.
                    let handle = match pictures.get("\u{1}bo2mp minimap") {
                        Some(h) if image_assets.get(h).is_some() => {
                            let h = h.clone();
                            image_assets.insert(&h, made).ok();
                            h
                        }
                        _ => {
                            let h = image_assets.add(made);
                            pictures.insert("\u{1}bo2mp minimap".to_owned(), h.clone());
                            h
                        }
                    };
                    // bo2mp: the box's backing is BO2's own compass materials, not a
                    // fitted grey: `compass_map_color_underlay2` (black, alpha 60/255)
                    // and `compass_map_color_underlay` (32x32 dark green-grey, alpha
                    // 1.0 at the top to 0.09 at the bottom), stretched over the box
                    // under the map, both at the material's own alpha (the
                    // materials carry no constants: technique trivial_9z33feqw,
                    // so the texture alpha is the whole rule; no strength factor).
                    for name in ["compass_map_color_underlay2", "compass_map_color_underlay"] {
                        if let Some(under) = picture(&mut pictures, icons.as_deref(), &mut image_assets, name) {
                            kids.push((flat(0.0, 0.0, w, h), ImageNode { image: under, color: Color::WHITE.with_alpha(color.alpha()), ..default() }, UiTransform::IDENTITY));
                            real_backing = true;
                        }
                    }
                    kids.push((flat(0.0, 0.0, w, h), ImageNode { image: handle, color, ..default() }, UiTransform::IDENTITY));
                    // BO2's minimap overlay (`setupCompassOverlay`): the box's
                    // own faint hex mesh, fading in towards the bottom, over
                    // the map (the empty area below the map shows it).
                    if let Some(over) = picture(&mut pictures, icons.as_deref(), &mut image_assets, "mp_minimap_overlay") {
                        kids.push((
                            flat(0.0, 0.0, w, h),
                            ImageNode { image: over, color: Color::WHITE.with_alpha(color.alpha()), ..default() },
                            UiTransform::IDENTITY,
                        ));
                    }
                }
            } else {
                let arrow = if full { (fit * 0.085).max(8.0) } else { (w * 0.155).max(8.0) };
                // bo2mp: under the End Game popup the pause menu sits under two
                // black dims; the dims are not drawn (see above), the compass
                // widget's colour carries what they let through, and the map
                // markers take it too (BO2 shows them near 51 of 255 there,
                // not full bright).
                let ping_tint = if full && in_match_menu && d.rgb != [1.0, 1.0, 1.0] {
                    if enc_ui { Color::linear_rgba(d.rgb[0], d.rgb[1], d.rgb[2], 1.0) } else { Color::srgba(d.rgb[0], d.rgb[1], d.rgb[2], 1.0) }
                } else {
                    Color::WHITE
                };
                let mut ping = |name: &str, at: [f32; 2], turn: f32| {
                    if at[0] < 0.0 || at[1] < 0.0 || at[0] > w || at[1] > h {
                        return;
                    }
                    let Some(handle) = picture(&mut pictures, icons.as_deref(), &mut image_assets, name) else { return };
                    kids.push((
                        flat(at[0] - arrow * 0.5, at[1] - arrow * 0.5, arrow, arrow),
                        ImageNode { image: handle, color: ping_tint, ..default() },
                        UiTransform { rotation: Rot2::radians(turn), ..UiTransform::IDENTITY },
                    ));
                };
                // Where a world point sits in the widget, and which way a yaw
                // points there.
                let me_turn = view.heading(view.me.1);
                let at = |p: [f32; 2]| {
                    if full {
                        let [u, v] = view.uv(p);
                        [fx0 + u * fit, fy0 + v * fit]
                    } else {
                        let o = view.screen_offset(p);
                        [w * 0.5 + o[0] * px_per_unit, h * 0.5 + o[1] * px_per_unit]
                    }
                };
                let turn = |yaw: f32| if full { view.heading(yaw) } else { view.heading(yaw) - me_turn };
                for (p, yaw, firing) in &view.enemies {
                    ping(if *firing { "compassping_enemyfiring" } else { "compassping_enemy" }, at(*p), turn(*yaw));
                }
                for (p, yaw) in &view.friends {
                    ping("compassping_friendly_mp", at(*p), turn(*yaw));
                }
                ping("compassping_player", at(view.me.0), turn(view.me.1));
                // Picking a spot (bo2mp_locsel "<material> <radius>"): the
                // selector where his cursor is, its radius to the map's scale.
                let locsel = hud.sent.as_ref().and_then(|v| v.mp.as_ref()).map(|m| m.locsel.clone()).unwrap_or_default();
                if full
                    && let Some((material, radius)) = locsel.trim().split_once(' ')
                    && let Some(c) = cursor.as_deref()
                    && let Some(handle) = picture(&mut pictures, icons.as_deref(), &mut image_assets, material)
                {
                    let radius: f32 = radius.trim().parse().unwrap_or(0.0);
                    let span = view.span_size();
                    let (sw, sh) = ((2.0 * radius / span[0]).max(0.02) * fit, (2.0 * radius / span[1]).max(0.02) * fit);
                    let (cx, cy) = (fx0 + c.at[0] * fit, fy0 + c.at[1] * fit);
                    kids.push((
                        flat(cx - sw * 0.5, cy - sh * 0.5, sw, sh),
                        ImageNode { image: handle, ..default() },
                        UiTransform::IDENTITY,
                    ));
                }
            }
            // bo2mp: the HUD map's dark square and thin border (the real
            // map shows them under and around the picture).
            let mut node = node;
            let backdrop = if d.kind.starts_with("compass_under") && !full {
                node.border = UiRect::all(Val::Px(scale.max(1.0)));
                if real_backing {
                    // The backing is the underlay pictures; only the thin box edge
                    // (no source found, fitted) is still drawn here.
                    (BackgroundColor(Color::NONE), BorderColor::all(if enc_ui { Color::linear_rgba(0.85, 0.88, 0.88, 0.085 * color.alpha()) } else { Color::srgba(0.85, 0.88, 0.88, 0.085 * color.alpha()) }))
                } else if enc_ui {
                    (BackgroundColor(Color::linear_rgba(0.12, 0.12, 0.12, 0.72 * color.alpha())), BorderColor::all(Color::linear_rgba(0.85, 0.88, 0.88, 0.085 * color.alpha())))
                } else {
                    (BackgroundColor(Color::srgba(0.12, 0.12, 0.12, 0.72 * color.alpha())), BorderColor::all(Color::srgba(0.85, 0.88, 0.88, 0.085 * color.alpha())))
                }
            } else {
                (BackgroundColor(Color::NONE), BorderColor::all(Color::NONE))
            };
            let e = commands
                .spawn((LuiNode { id: d.id, shows: String::new(), alpha: color.alpha() }, node, z, backdrop))
                .with_children(|c| {
                    for k in kids {
                        c.spawn(k);
                    }
                })
                .id();
            commands.entity(root).add_child(e);
        } else if (d.kind == "image" || d.kind == "loadingbar")
            && d.material.as_deref().is_none_or(|m| {
                m == "white" || (m == "black" && picture(&mut pictures, icons.as_deref(), &mut image_assets, m).is_none())
            })
        {
            // A picture with no material (or the engine's plain `white`) is
            // a block of its colour: BO2's dim behind its menus. (A named
            // picture that was not loaded draws nothing: a white block in
            // its place - Create-a-Class's perk icons - is never BO2's.)
            keep.insert(d.id);
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x0 * scale),
                top: Val::Px(y0 * scale),
                width: Val::Px((x1 - x0).abs() * scale),
                height: Val::Px((y1 - y0).abs() * scale),
                ..default()
            };
            if d.id == usize::MAX - 20 {
                // bo2mp: the engine blur's vignette and darkening over the
                // covered stack (see `blur_vignette`): a black picture whose
                // alpha is one minus the shader's factor, stretched over the
                // whole screen.
                let q = (d.alpha.clamp(0.0, 1.0) * 100.0).round() as i32;
                let key = format!("\u{1}vignette {q}");
                let handle = pictures
                    .entry(key.clone())
                    .or_insert_with(|| image_assets.add(blur_vignette(q as f32 / 100.0)))
                    .clone();
                let tint = Color::srgba(0.0, 0.0, 0.0, 1.0);
                if let Some(&e) = existing.get(&d.id)
                    && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
                    && n.shows == key
                {
                    *nn = node;
                    *zi = z;
                    continue;
                }
                if let Some(&e) = existing.get(&d.id) {
                    commands.entity(e).try_despawn();
                }
                let e = commands
                    .spawn((
                        LuiNode { id: d.id, shows: key, alpha: 1.0 },
                        node,
                        ImageNode { image: handle, color: tint, image_mode: bevy::ui::widget::NodeImageMode::Stretch, ..default() },
                        z,
                    ))
                    .id();
                commands.entity(root).add_child(e);
                continue;
            }
            if d.behind {
                // A block under a blurring menu: blurred (a soft patch).
                let rect_u = [x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()];
                soft_patch(&mut commands, root, &mut pictures, &mut image_assets, &existing, &mut nodes, &mut image_nodes, d.id, rect_u, scale, color, z);
                continue;
            }
            let shows = "block".to_owned();
            let color = if d.material.as_deref() == Some("black") { Color::linear_rgba(0.0, 0.0, 0.0, color.alpha()) } else { color };
            // (The engine's loading bar has no material of its own in the
            // zones; `white` is plain alpha blend, src alpha / inverse src
            // alpha, image `$white` opaque, so its strips draw at the
            // script's colour and alpha as they are.)
            // bo2mp: BO2 blends its menus' black dim in the sRGB target's
            // gamma space; Bevy blends in linear light, where 0.8 black over
            // a bright world leaves it twice as bright. Over the match's
            // world (not the front end's tuned backdrop) the alpha is
            // remapped so the dimmed picture is the gamma blend's.
            let dim_over_world = !enc_ui && front.is_none() && d.rgb == [0.0, 0.0, 0.0] && color.alpha() > 0.3;
            // bo2mp: the killcam's top and bottom bands (a see-through red,
            // black in the final killcam; no material, 60-130 high at the
            // screen's edge) blend in BO2's gamma space too. Pure black gets
            // the exact remap; red the milder 1.5 power, which keeps its
            // green/blue right and its red from running away over the
            // world's dark parts (real/55's bands read pink, not grey).
            let band_h = (d.rect[3] - d.rect[1]).abs();
            let band_edge = d.rect[1].min(d.rect[3]) <= 1.0 || d.rect[1].max(d.rect[3]) >= 719.0;
            let killcam_band = !enc_ui && front.is_none() && !in_match_menu && d.material.is_none() && (60.0..=130.0).contains(&band_h) && band_edge;
            let color = if killcam_band && d.rgb == [1.0, 0.0, 0.0] {
                Color::linear_rgba(1.0, 0.0, 0.0, 1.0 - (1.0 - color.alpha()).powf(1.5))
            } else if killcam_band && d.rgb == [0.0, 0.0, 0.0] {
                Color::linear_rgba(0.0, 0.0, 0.0, 1.0 - (1.0 - color.alpha()).powf(2.2))
            } else if dim_over_world {
                Color::linear_rgba(0.0, 0.0, 0.0, 1.0 - (1.0 - color.alpha()).powf(2.2))
            } else {
                color
            };
            if let Some(&e) = existing.get(&d.id)
                && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
                && n.shows == shows
            {
                *nn = node;
                *zi = z;
                commands.entity(e).insert(BackgroundColor(color));
                continue;
            }
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let e = commands
                .spawn((LuiNode { id: d.id, shows, alpha: color.alpha() }, node, BackgroundColor(color), z))
                .id();
            commands.entity(root).add_child(e);
        } else if let Some(mat) = &d.material {
            // BO2ZM_LUI_FORCEMAT=<id>:<material> draws that element with
            // another picture (debugging aid).
            let forced = std::env::var("BO2ZM_LUI_FORCEMAT")
                .ok()
                .and_then(|v| v.split_once(':').map(|(i, m)| (i.parse::<usize>().ok(), m.to_owned())))
                .filter(|(i, _)| *i == Some(d.id))
                .map(|(_, m)| m);
            let mat = forced.as_ref().unwrap_or(mat);
            // bo2mp: the popup panel is a solid dark grey in BO2 (33 on every
            // panel and dots picture; what lies under it does not show).
            // The textures read raw give ~0.107 linear; the target's encode
            // lifts 0.107 x 0.142 = 0.0152 to 33. Drawn opaque (alpha 1)
            // at that scale; the old alpha x 0.116 left it see-through and
            // darker (7-16).
            let color = if !enc_ui && front.is_none() && (mat.starts_with("menu_mp_popup") || mat.starts_with("menu_mp_dots")) {
                let l = color.to_linear();
                Color::linear_rgba(l.red * 0.142, l.green * 0.142, l.blue * 0.142, l.alpha)
            } else {
                color
            };
            let (handle, pad_share) = if d.behind || (backdrop_pic && blur_bkg()) {
                behind_picture(&mut pictures, icons.as_deref(), &mut image_assets, mat, (x1 - x0).abs(), d.behind && d.clip.is_none())
                    .map_or((None, (0.0, 0.0)), |(h, px, py)| (Some(h), (px, py)))
            } else {
                (picture(&mut pictures, icons.as_deref(), &mut image_assets, mat), (0.0, 0.0))
            };
            let Some(handle) = handle else {
                continue;
            };
            // (The padded picture's border lies outside its rectangle.)
            let (pw, ph) = ((x1 - x0).abs() * pad_share.0, (y1 - y0).abs() * pad_share.1);
            let (x0, x1) = if x1 >= x0 { (x0 - pw, x1 + pw) } else { (x0 + pw, x1 - pw) };
            let (y0, y1) = if y1 >= y0 { (y0 - ph, y1 + ph) } else { (y0 + ph, y1 - ph) };
            // bo2mp: a stencil above it (the scorestreak bar) cuts the
            // picture to its rectangle: what shows, and the part of the
            // picture that lands there.
            let (nx0, ny0, nx1, ny1) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
            let (mut vx0, mut vy0, mut vx1, mut vy1) = (nx0, ny0, nx1, ny1);
            let mut cut: Option<Rect> = None;
            if let Some(c) = d.clip {
                (vx0, vy0, vx1, vy1) = (nx0.max(c[0]), ny0.max(c[1]), nx1.min(c[2]), ny1.min(c[3]));
                if vx1 <= vx0 || vy1 <= vy0 {
                    continue;
                }
                if let Some(img) = image_assets.get(&handle)
                    && nx1 > nx0
                    && ny1 > ny0
                    && (vx0, vy0, vx1, vy1) != (nx0, ny0, nx1, ny1)
                {
                    let (tw, th) = (img.width() as f32, img.height() as f32);
                    cut = Some(Rect::new(
                        (vx0 - nx0) / (nx1 - nx0) * tw,
                        (vy0 - ny0) / (ny1 - ny0) * th,
                        (vx1 - nx0) / (nx1 - nx0) * tw,
                        (vy1 - ny0) / (ny1 - ny0) * th,
                    ));
                }
            }
            keep.insert(d.id);
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(vx0 * scale),
                top: Val::Px(vy0 * scale),
                width: Val::Px((vx1 - vx0) * scale),
                height: Val::Px((vy1 - vy0) * scale),
                ..default()
            };
            // bo2mp: a rectangle drawn right to left (or bottom to top) is
            // the picture turned over: the right-hand gun panel is the left
            // one mirrored.
            // bo2mp: the name card's dark backing is drawn at 0.85 over the
            // world; blended in linear light it comes out twice as bright
            // as BO2's gamma-space blend, so the alpha is remapped as the
            // menus' black dim is.
            let color = if front.is_none() && mat.starts_with("emblem_bg") && color.alpha() < 1.0 {
                color.with_alpha(1.0 - (1.0 - color.alpha()).powf(2.2))
            } else {
                color
            };
            let (flip_x, flip_y) = (x1 < x0, y1 < y0);
            // bo2mp: setupTiles(n): the picture repeats across the width in
            // tiles n units wide, stretched down (setTileVertically(false)):
            // the menus' dot grid is an 8-wide picture tiled at 8.
            let image_mode = match image_assets.get(&handle) {
                Some(img) if d.tiles >= 1.0 && cut.is_none() && img.width() > 0 => bevy::ui::widget::NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: false,
                    stretch_value: d.tiles * scale / img.width() as f32,
                },
                _ => bevy::ui::widget::NodeImageMode::Stretch,
            };
            // bo2mp scope: an additive picture in the match HUD is added onto
            // the scene (see lui_additive), not alpha-blended.
            let add_pic = (front.is_none() && cut.is_none() && !in_match_menu && add_names.as_ref().is_some_and(|n| n.contains(mat.as_str())))
                .then(|| format!("additive:{mat}"))
                .and_then(|n| picture(&mut pictures, icons.as_deref(), &mut image_assets, &n));
            if let Some(pic) = add_pic {
                let lin = color.to_linear();
                let tint = lin;
                // (The colour is in the key: a fade respawns the node.)
                let shows = format!("additive {mat} {:.2} {:.2} {:.2} {:.2}", lin.red, lin.green, lin.blue, lin.alpha);
                // (A node reaching past the screen keeps to it, the picture
                // cut to match: the material pass drops such a node.)
                let (cx0, cy0, cx1, cy1) = (vx0.max(0.0), vy0.max(0.0), vx1.min(720.0 * aspect), vy1.min(720.0));
                if cx1 <= cx0 || cy1 <= cy0 {
                    continue;
                }
                let (w, h) = ((vx1 - vx0).max(0.001), (vy1 - vy0).max(0.001));
                let uv = Vec4::new((cx0 - vx0) / w, (cy0 - vy0) / h, (cx1 - cx0) / w, (cy1 - cy0) / h);
                // (A flipped picture reads its window from the far side, backwards.)
                let uv = Vec4::new(if flip_x { 1.0 - uv.x } else { uv.x }, if flip_y { 1.0 - uv.y } else { uv.y }, if flip_x { -uv.z } else { uv.z }, if flip_y { -uv.w } else { uv.w });
                let node = Node {
                    left: Val::Px(cx0 * scale),
                    top: Val::Px(cy0 * scale),
                    width: Val::Px((cx1 - cx0) * scale),
                    height: Val::Px((cy1 - cy0) * scale),
                    ..node
                };
                if let Some(&e) = existing.get(&d.id)
                    && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
                    && n.shows == shows
                {
                    *nn = node;
                    *zi = z;
                    continue;
                }
                if std::env::var_os("BO2ZM_LUI_LOG").is_some() {
                    diag::info!(Ui, "bo2mp lui additive #{} {shows} at {:?} node {:?} uv {:?}", d.id, d.rect, (cx0, cy0, cx1, cy1), uv);
                }
                if let Some(&e) = existing.get(&d.id) {
                    commands.entity(e).try_despawn();
                }
                let e = commands
                    .spawn((
                        LuiNode { id: d.id, shows, alpha: color.alpha() },
                        node,
                        MaterialNode(additive.add(crate::lui_additive::AdditiveUi { tint, picture: pic, uv, encoded: if enc_ui { 1.0 } else { 0.0 }, color_add: if color_add_names.contains(mat.as_str()) { 1.0 } else { 0.0 } })),
                        z,
                    ))
                    .id();
                commands.entity(root).add_child(e);
                continue;
            }
            let shows = if d.behind { format!("image {mat} behind") } else { format!("image {mat}") };
            if std::env::var("BO2ZM_LUI_WATCH").ok().and_then(|v| v.parse::<usize>().ok()) == Some(d.id) {
                diag::info!(
                    Ui,
                    "bo2zm lui watch #{} at {:.0} ms: rect {:?} colour {:?} entity {:?} z {} computed {:?}",
                    d.id,
                    hud.host.now_ms(),
                    d.rect,
                    color,
                    existing.get(&d.id),
                    order,
                    existing
                        .get(&d.id)
                        .and_then(|e| computed.get(*e).ok())
                        .map(|(c, t, v)| (c.size(), t.translation, v.get()))
                );
            }
            if let Some(&e) = existing.get(&d.id)
                && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
                && n.shows == shows
            {
                *nn = node;
                *zi = z;
                if let Ok(mut img) = image_nodes.get_mut(e) {
                    img.color = color;
                    img.flip_x = flip_x;
                    img.flip_y = flip_y;
                    img.rect = cut;
                    img.image_mode = image_mode.clone();
                }
                // setZRot(deg): a picture turned in its own plane (the
                // Selected Scorestreaks arrows: the up arrow at 270 reads
                // as a right arrow); LUI turns counter-clockwise.
                if d.z_rot.abs() > 0.01 {
                    commands.entity(e).insert(UiTransform { rotation: Rot2::degrees(-d.z_rot), ..UiTransform::IDENTITY });
                }
                continue;
            }
            if std::env::var_os("BO2ZM_LUI_LOG").is_some() {
                diag::info!(Ui, "bo2zm lui image #{} {shows} at {:?}", d.id, d.rect);
            }
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let e = commands
                .spawn((
                    LuiNode { id: d.id, shows, alpha: color.alpha() },
                    node,
                    // BO2 stretches every picture over its element (an 8x32
                    // gradient fills a whole box); bevy's Auto mode would
                    // keep the picture's shape inside it.
                    ImageNode { image: handle, color, image_mode, flip_x, flip_y, rect: cut, ..default() },
                    z,
                    UiTransform { rotation: Rot2::degrees(-d.z_rot), ..UiTransform::IDENTITY },
                ))
                .id();
            commands.entity(root).add_child(e);
        } else if let (Some(text), Some(bo2)) = (&d.text, fonts.as_deref()) {
            keep.insert(d.id);
            let text = plain(text);
            // bo2mp: black see-through text in the match HUD (the gun-name slab's
            // 0.5 black letters) is blended in gamma space by BO2: over a pale
            // slab the letters read dark (scene * (1 - a)). Bevy blends in linear
            // light and leaves them washed; the alpha is remapped as the black
            // dim's is.
            let color = if !enc_ui && front.is_none() && !in_match_menu && d.rgb == [0.0, 0.0, 0.0] && color.alpha() < 0.99 {
                color.with_alpha(1.0 - (1.0 - color.alpha()).powf(2.2))
            } else {
                color
            };
            let font = font_name(d.font.as_deref().unwrap_or("normalFont"));
            let px = (y1 - y0).abs() * scale;
            // A text with one of BO2's key pictures in it (the Controls rows'
            // mouse buttons: one private-use letter each) is a row of text
            // pieces and pictures.
            let pieces = {
                let values = hud.host.values.borrow();
                icon_pieces(&text, &|n| font_icon(&values.tables, n))
            };
            let has_icons = pieces.iter().any(|p| matches!(p, Piece::Icon(_)));
            let width = if has_icons {
                pieces
                    .iter()
                    .map(|p| match p {
                        Piece::Text(t) => bo2.line_width(font, t, px),
                        Piece::Icon(i) => px * i.x_scale,
                    })
                    .sum()
            } else {
                bo2.line_width(font, &text, px)
            };
            // A text wider than its box (a box at least 100 units wide)
            // wraps at its words onto lines below, as BO2's does (Create-
            // a-Class's weapon descriptions); a box of 100 or less (the
            // lobby's "Game starting in N" is exactly 100) is a place,
            // not a width.
            let box_w = (x1 - x0).abs() * scale;
            let lines = if has_icons {
                vec![text.clone()]
            } else if box_w > 100.5 * scale && width > box_w + 1.0 { wrap_words(bo2, font, &text, px, box_w) } else { vec![text.clone()] };
            let wrapped = lines.len() > 1;
            // LUI.Alignment: 2 centre, 3 right, else left (a text with
            // none set reads from its box's anchors, see `Drawn`).
            let left = match (wrapped, d.alignment) {
                (true, _) => x0.min(x1) * scale,
                (_, 2) => (x0 + x1) * 0.5 * scale - width * 0.5,
                (_, 3) => x1 * scale - width,
                _ => x0 * scale,
            };
            if d.behind {
                // A text under a blurring menu: its letters (the font sheet's
                // coverage, laid out as the lines below) blurred as BO2's
                // engine blurs the menu layer it was drawn into.
                let px_u = px / scale;
                let lefts: Vec<(String, f32)> = lines
                    .iter()
                    .map(|l| {
                        let w = bo2.line_width(font, l, px) / scale;
                        let l_left = match (wrapped, d.alignment) {
                            (true, 2) => (x0 + x1) * 0.5 - w * 0.5,
                            (true, 3) => x1 - w,
                            (true, _) => x0.min(x1),
                            _ => left / scale,
                        };
                        (l.clone(), l_left)
                    })
                    .collect();
                let bl = lefts.iter().map(|(_, l)| *l).fold(f32::MAX, f32::min);
                let br = lefts.iter().map(|(l, x)| *x + bo2.line_width(font, l, px) / scale).fold(f32::MIN, f32::max);
                let rel: Vec<(String, f32)> = lefts.iter().map(|(l, x)| (l.clone(), x - bl)).collect();
                let sigma = blur_sigma();
                let radius = ((sigma * 0.5).round() as usize).max(1);
                // Texels to the unit so that the box passes' Gaussian is `sigma` units.
                let res = ((radius * (radius + 1)) as f32).sqrt() / sigma;
                let pad = (sigma * 2.0).ceil();
                let key = format!("\u{1}softtext {font}|{px_u:.2}|{sigma:.2}|{}", rel.iter().map(|(l, x)| format!("{l}@{x:.1}")).collect::<Vec<_>>().join("|"));
                let w_u = (br - bl).max(1.0);
                let shows = format!("softtext {text}|{font}|{px:.1}|{w_u:.0}");
                if let Some(h) = pictures.get(&key).cloned() {
                    let rect_u = [bl, y0, w_u, bo2.coverage_height(font, px_u) * lines.len() as f32];
                    soft_patch_with(&mut commands, root, &existing, &mut nodes, &mut image_nodes, d.id, rect_u, scale, color, z, h, pad, shows);
                    continue;
                }
                if let Some((mut cov, tw, th, bh)) = bo2.coverage(font, &rel, px_u, w_u, pad, res) {
                    crate::lui_scene::blur(&mut cov, tw, th, radius);
                    for p in cov.chunks_exact_mut(4) {
                        p[..3].fill(255);
                    }
                    let h = image_assets.add(crate::lui_scene::image(tw, th, cov));
                    pictures.insert(key, h.clone());
                    let rect_u = [bl, y0, w_u, bh];
                    soft_patch_with(&mut commands, root, &existing, &mut nodes, &mut image_nodes, d.id, rect_u, scale, color, z, h, pad, shows);
                    continue;
                }
                // (No letters to read: the old patch of the text's extent.)
                let rect_u = [bl, y0 + 0.25 * (y1 - y0), w_u, ((lines.len() as f32 - 1.0) * 1.1 + 0.6) * (y1 - y0)];
                let tint = color.with_alpha(color.alpha() * behind_ink());
                soft_patch(&mut commands, root, &mut pictures, &mut image_assets, &existing, &mut nodes, &mut image_nodes, d.id, rect_u, scale, tint, z);
                continue;
            }
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
                top: Val::Px(y0 * scale),
                width: if wrapped { Val::Px(box_w) } else { Val::Auto },
                flex_direction: if has_icons { FlexDirection::Row } else { FlexDirection::Column },
                align_items: match (wrapped, d.alignment) {
                    (true, 2) => AlignItems::Center,
                    (true, 3) => AlignItems::FlexEnd,
                    _ => AlignItems::FlexStart,
                },
                ..default()
            };
            let shows = format!("text {text}|{font}|{px:.1}|{:.2}|{:.2}|{:.2}|{}|{}", d.rgb[0], d.rgb[1], d.rgb[2], lines.len(), in_match_menu);
            if let Some(&e) = existing.get(&d.id)
                && let Ok((_, mut n, mut nn, mut zi)) = nodes.get_mut(e)
                && n.shows == shows
            {
                *nn = node;
                *zi = z;
                // A fade: every letter's alpha.
                if (n.alpha - color.alpha()).abs() > 1e-3 {
                    n.alpha = color.alpha();
                    for g in children.iter_descendants(e) {
                        if let Ok(mut img) = image_nodes.get_mut(g) {
                            img.color.set_alpha(color.alpha());
                        }
                    }
                }
                continue;
            }
            if std::env::var_os("BO2ZM_LUI_LOG").is_some() {
                diag::info!(Ui, "bo2zm lui text #{} {shows} at {:?} align {} left {left} width {width}", d.id, d.rect, d.alignment);
            }
            if let Some(&e) = existing.get(&d.id) {
                commands.entity(e).try_despawn();
            }
            let handles: Vec<Option<Handle<Image>>> = pieces
                .iter()
                .map(|p| match p {
                    Piece::Icon(i) => picture(&mut pictures, icons.as_deref(), &mut image_assets, &i.material),
                    Piece::Text(_) => None,
                })
                .collect();
            let e = commands
                .spawn((LuiNode { id: d.id, shows, alpha: color.alpha() }, node, z))
                .with_children(|c| {
                    if has_icons {
                        for (p, h) in pieces.iter().zip(&handles) {
                            match (p, h) {
                                (Piece::Text(t), _) => {
                                    bo2.spawn_line(c, font, &[(t.clone(), color)], px, 0.0);
                                }
                                (Piece::Icon(i), Some(h)) => {
                                    c.spawn((
                                        Node {
                                            width: Val::Px(px * i.x_scale),
                                            height: Val::Px(px * i.y_scale),
                                            margin: UiRect::vertical(Val::Px(-px * (i.y_scale - 1.0) * 0.5)),
                                            ..default()
                                        },
                                        ImageNode { image: h.clone(), color: Color::linear_rgba(1.0, 1.0, 1.0, color.alpha()), image_mode: bevy::ui::widget::NodeImageMode::Stretch, ..default() },
                                    ));
                                }
                                _ => {}
                            }
                        }
                        return;
                    }
                    for line in &lines {
                        bo2.spawn_line(c, font, &[(line.clone(), color)], px, 0.0);
                    }
                })
                .id();
            commands.entity(root).add_child(e);
        }
    }
    // bo2mp: the build stamp the engine draws over every front-end screen
    // (not LUI: 43.1736.11, small and white, under the title bar's right end).
    // The engine's own stamp font and size live in the packed t6mp.exe, so
    // there is no data for them: the condensed HUD font (smallFont) at the
    // size that makes the string as wide as real's (40 px at 720p). The
    // normalFont this drew before was no font key at all, fell back to
    // Default, whose "3" overlaps the "6" at this size.
    // Checked 10-08: Condensed's pixelHeight is 25 and script text is drawn at
    // its element's own height (no pixelHeight rule), so no data gives 14.5.
    const STAMP: usize = usize::MAX - 7;
    // The Loading menu is not a front-end screen: the engine draws no stamp
    // over it (real BO2's loading screen has none).
    if front.is_some()
        && !hud.host.open_menus().iter().any(|m| m == "Menu.Loading")
        && let Some(bo2) = fonts.as_deref()
    {
        keep.insert(STAMP);
        if !existing.contains_key(&STAMP) {
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(1084.0 * scale),
                top: Val::Px(69.5 * scale),
                ..default()
            };
            let e = commands
                .spawn((LuiNode { id: STAMP, shows: String::new(), alpha: 1.0 }, node, ZIndex(i32::MAX / 2)))
                .with_children(|c| {
                    bo2.spawn_line(c, font_name("smallFont"), &[("43.1736.11".to_owned(), Color::WHITE)], 14.5 * scale, 0.0);
                })
                .id();
            commands.entity(root).add_child(e);
        }
    }
    for (id, e) in existing {
        if !keep.contains(&id) {
            commands.entity(e).try_despawn();
        }
    }
}

/// The marks `plain` puts round a FontIcon entry's name.
const GLYPH_OPEN: char = '\u{E200}';
const GLYPH_CLOSE: char = '\u{E201}';

/// The tab strips' arrows (`^BBUTTON_CYCLE_LEFT^` / `_RIGHT^`).
const ARROW_ICONS: [&str; 2] = ["ui_arrow_left", "ui_arrow_right"];

/// A picture in a line of text: its material and its size as a fraction of
/// the text's height (BO2's FontIcon entry: xScale across, yScale up).
#[derive(Clone)]
struct IconPic {
    material: String,
    x_scale: f32,
    y_scale: f32,
}

enum Piece {
    Text(String),
    Icon(IconPic),
}

/// BO2's icon set (the FontIcon asset's `bo2mp/fonticon.csv`: name, hash,
/// material, size, xScale, yScale): the picture a glyph name draws.
fn font_icon(tables: &HashMap<String, Vec<Vec<String>>>, name: &str) -> Option<IconPic> {
    let row = tables.get("bo2mp/fonticon.csv")?.iter().find(|r| r.first().is_some_and(|n| n == name))?;
    let material = row.get(2).filter(|m| !m.is_empty())?.clone();
    Some(IconPic { material, x_scale: row.get(4)?.parse().ok()?, y_scale: row.get(5)?.parse().ok()? })
}

/// A text's pieces: its words and BO2's pictures (the tab arrows' private-use
/// letters, and a FontIcon entry's name between `GLYPH_OPEN` / `GLYPH_CLOSE`).
fn icon_pieces(text: &str, glyph: &dyn Fn(&str) -> Option<IconPic>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut name: Option<String> = None;
    for c in text.chars() {
        if let Some(n) = name.as_mut() {
            if c == GLYPH_CLOSE {
                if let Some(pic) = glyph(n) {
                    if !cur.is_empty() {
                        out.push(Piece::Text(std::mem::take(&mut cur)));
                    }
                    out.push(Piece::Icon(pic));
                }
                name = None;
            } else {
                n.push(c);
            }
            continue;
        }
        if c == GLYPH_OPEN {
            name = Some(String::new());
            continue;
        }
        match (c as u32).checked_sub(0xE100).and_then(|i| ARROW_ICONS.get(i as usize)) {
            Some(m) => {
                if !cur.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut cur)));
                }
                out.push(Piece::Icon(IconPic { material: (*m).to_owned(), x_scale: 0.8, y_scale: 1.1 }));
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(Piece::Text(cur));
    }
    out
}

/// A text that is one button glyph (`^BBUTTON_CYCLE_LEFT^`) drawn as the
/// picture BO2 PC shows for it (CHOOSE CLASS's class set arrows, the
/// settings tabs' arrows: ui_arrow_left / ui_arrow_right).
fn button_glyph(mut d: Drawn) -> Drawn {
    let picture = match d.text.as_deref().map(str::trim) {
        Some("^BBUTTON_CYCLE_LEFT^") => "ui_arrow_left",
        Some("^BBUTTON_CYCLE_RIGHT^") => "ui_arrow_right",
        Some("^BBUTTON_MOUSE_CLICK^") => "mouse_click",
        Some("^BBUTTON_MOUSE_CLICK_ACTIVE^") => "mouse_click_active",
        Some("^BBUTTON_MOUSE_EDIT^") => "mouse_edit",
        Some("^BBUTTON_MOUSE_EDIT_ACTIVE^") => "mouse_edit_active",
        _ => return d,
    };
    d.kind = "image";
    d.material = Some(picture.to_owned());
    d.text = None;
    d.rgb = [1.0, 1.0, 1.0];
    d
}

/// `text` broken at its spaces into lines no wider than `max`, each word
/// measured with the space that follows it, as BO2's text layout counts it
/// (a word longer than that stands alone on its line).
fn wrap_words(bo2: &crate::bo2_font::Bo2Fonts, font: &str, text: &str, px: f32, max: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let next = if cur.is_empty() { word.to_owned() } else { format!("{cur} {word}") };
        if !cur.is_empty() && bo2.line_width(font, &format!("{next} "), px) > max {
            lines.push(std::mem::replace(&mut cur, word.to_owned()));
        } else {
            cur = next;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// A material's picture as an image handle (made once).
/// bo2mp: how much of a covered menu's pictures show before the blur's dim
/// falls on them (they are drawn as written: the dim is the one rule);
/// `BO2MP_BEHIND_DIM`.
fn behind_dim() -> f32 {
    std::env::var("BO2MP_BEHIND_DIM").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

/// bo2mp: the blur's sigma in screen units (the front end is 720 high). Black
/// Ops 1's engine (BO2's parent) blurs the menu layer by
/// `ui_blurAmount * intensity` virtual units of a 480-high screen (default
/// 4.0), and runs its Gaussian twice, so the two passes add in quadrature:
/// sigma = sqrt(2) * 4 * 720 / 480 = 8.5 units at full intensity.
/// `BO2MP_BEHIND_BLUR` overrides it (units).
fn blur_sigma() -> f32 {
    std::env::var("BO2MP_BEHIND_BLUR").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0f32.sqrt() * 4.0 * 720.0 / 480.0)
}

/// bo2mp: what the engine's UI blur leaves of the covered stack, per screen
/// position. BO2's blur material `ui_blur` (zone `code_post_gfx_mp`,
/// techniqueset `sw4_2d_ui_mp_blur`) holds `Radius` 0.42 and `Hardness` 1.0,
/// and its pixel shader (disassembled) computes, with `I` the blur
/// intensity, `D` the dvar `ui_blurDarkenAmount`, `d2` the squared distance
/// of the pixel's screen position from the screen's middle (both axes 0 to 1):
///   out = blurred * (1 - I * d2^Hardness / (1 - (1 - Radius) * I^Hardness)) * (1 - D * I)
/// so the middle keeps `1 - D*I` of its level and the corners fall off. The
/// blend there is replace, so the factor multiplies the whole covered stack.
const BLUR_RADIUS: f32 = 0.42;
const BLUR_HARDNESS: f32 = 1.0;
/// `ui_blurDarkenAmount`. The engine registers its default in code (not in the
/// zones, the scripts or the cfgs; the executable is packed), so this one
/// number is MEASURED, not read: BO2's own Quit shot, blurred main menu over
/// the unblurred one, reads 0.50 at the middle of the screen.
/// `BO2MP_BLUR_DARKEN` overrides it.
fn blur_darken() -> f32 {
    std::env::var("BO2MP_BLUR_DARKEN").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5)
}

/// bo2mp: the black picture whose alpha is one minus the blur shader's factor
/// (see `BLUR_RADIUS`) at intensity `i`; 128 x 72 texels over the whole screen.
fn blur_vignette(i: f32) -> Image {
    let (w, h) = (128usize, 72usize);
    let mut px = vec![0u8; w * h * 4];
    let d = blur_darken();
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let d2 = (0.5 - u).powi(2) + (0.5 - v).powi(2);
            let vig = (1.0 - i * d2.powf(BLUR_HARDNESS) / (1.0 - (1.0 - BLUR_RADIUS) * i.powf(BLUR_HARDNESS))).max(0.0);
            let f = (vig * (1.0 - d * i)).clamp(0.0, 1.0);
            px[(y * w + x) * 4 + 3] = ((1.0 - f) * 255.0).round() as u8;
        }
    }
    crate::lui_scene::image(w, h, px)
}

/// bo2mp: blur the backdrop picture under a blurring menu (`BO2MP_BLUR_BKG=0` turns it off).
fn blur_bkg() -> bool {
    std::env::var("BO2MP_BLUR_BKG").map(|v| v != "0").unwrap_or(true)
}

/// bo2mp: the share of a covered menu's text box that shows as ink once it is
/// blurred away (the sheet's letters cover about a third of their box);
/// `BO2MP_BEHIND_INK`.
fn behind_ink() -> f32 {
    std::env::var("BO2MP_BEHIND_INK").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5)
}

/// bo2mp: a white rectangle of `w` x `h` screen units blurred as the covered
/// menu's pictures are (`BO2MP_BEHIND_BLUR` units), padded so the blur has
/// room: BO2's blur of a text or a block that is not a picture. Returns the
/// picture and the padding (units). Two texels to the unit.
fn soft_rect(cache: &mut HashMap<String, Handle<Image>>, images: &mut Assets<Image>, w: f32, h: f32) -> (Handle<Image>, f32) {
    let screen: f32 = blur_sigma();
    let pad = (screen * 2.0).ceil();
    let (tw, th) = (((w + 2.0 * pad) * 0.5).ceil().max(1.0) as usize, ((h + 2.0 * pad) * 0.5).ceil().max(1.0) as usize);
    let key = format!("\u{1}soft {tw}x{th} {screen}");
    if let Some(k) = cache.get(&key) {
        return (k.clone(), pad);
    }
    let mut px = vec![0u8; tw * th * 4];
    let (x0, y0) = ((pad * 0.5) as usize, (pad * 0.5) as usize);
    let (x1, y1) = ((x0 + (w * 0.5).ceil() as usize).min(tw), (y0 + (h * 0.5).ceil() as usize).min(th));
    for y in y0..y1 {
        for x in x0..x1 {
            px[(y * tw + x) * 4..(y * tw + x) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    crate::lui_scene::blur(&mut px, tw, th, ((screen * 0.5).round() as usize).max(1));
    for p in px.chunks_exact_mut(4) {
        p[..3].fill(255);
    }
    let handle = images.add(crate::lui_scene::image(tw, th, px));
    cache.insert(key, handle.clone());
    (handle, pad)
}

/// bo2mp: draw `rect` (x, y, w, h in units) as a soft patch tinted `tint`,
/// keeping the element's node between frames.
#[allow(clippy::too_many_arguments)]
fn soft_patch(
    commands: &mut Commands,
    root: Entity,
    cache: &mut HashMap<String, Handle<Image>>,
    images: &mut Assets<Image>,
    existing: &HashMap<usize, Entity>,
    nodes: &mut Query<(Entity, &mut LuiNode, &mut Node, &mut ZIndex)>,
    image_nodes: &mut Query<&mut ImageNode>,
    id: usize,
    rect: [f32; 4],
    scale: f32,
    tint: Color,
    z: ZIndex,
) {
    let (handle, pad) = soft_rect(cache, images, rect[2].max(1.0), rect[3].max(1.0));
    let shows = format!("soft {:.0}x{:.0}", rect[2], rect[3]);
    soft_patch_with(commands, root, existing, nodes, image_nodes, id, rect, scale, tint, z, handle, pad, shows);
}

/// bo2mp: `soft_patch` with its blurred picture (`handle`, clear border
/// `pad` units) already made; `shows` names what it draws.
#[allow(clippy::too_many_arguments)]
fn soft_patch_with(
    commands: &mut Commands,
    root: Entity,
    existing: &HashMap<usize, Entity>,
    nodes: &mut Query<(Entity, &mut LuiNode, &mut Node, &mut ZIndex)>,
    image_nodes: &mut Query<&mut ImageNode>,
    id: usize,
    rect: [f32; 4],
    scale: f32,
    tint: Color,
    z: ZIndex,
    handle: Handle<Image>,
    pad: f32,
    shows: String,
) {
    let node = Node {
        position_type: PositionType::Absolute,
        left: Val::Px((rect[0] - pad) * scale),
        top: Val::Px((rect[1] - pad) * scale),
        width: Val::Px((rect[2].max(1.0) + 2.0 * pad) * scale),
        height: Val::Px((rect[3].max(1.0) + 2.0 * pad) * scale),
        ..default()
    };
    if let Some(&e) = existing.get(&id)
        && let Ok((_, n, mut nn, mut zi)) = nodes.get_mut(e)
        && n.shows == shows
    {
        *nn = node;
        *zi = z;
        if let Ok(mut img) = image_nodes.get_mut(e) {
            img.color = tint;
        }
        return;
    }
    if let Some(&e) = existing.get(&id) {
        commands.entity(e).try_despawn();
    }
    let e = commands
        .spawn((
            LuiNode { id, shows, alpha: tint.alpha() },
            node,
            ImageNode { image: handle, color: tint, image_mode: bevy::ui::widget::NodeImageMode::Stretch, ..default() },
            z,
        ))
        .id();
    commands.entity(root).add_child(e);
}

/// bo2mp: `name`'s picture blurred (premultiplied, so its clear edges do not
/// darken), for the covered main menu: about `BO2MP_BEHIND_BLUR` screen units
/// of blur over a `rect_w`-wide rectangle. With `pad` the picture gets a
/// clear border of twice the blur radius first, so its glow spills past its
/// rectangle as BO2's blur of the whole screen does (the BLACK OPS logo is a
/// soft glow beyond its box, not a cut-off block); returns the picture and
/// the border as a share of its width and height.
fn behind_picture(
    cache: &mut HashMap<String, Handle<Image>>,
    icons: Option<&assets::T6HudIcons>,
    images: &mut Assets<Image>,
    name: &str,
    rect_w: f32,
    pad: bool,
) -> Option<(Handle<Image>, f32, f32)> {
    use bevy::render::render_resource::TextureFormat as F;
    // (The pixels come from the loaded picture, not from the uploaded copy:
    // the GPU copy has given its data up.)
    let Some(src) = icons.and_then(|i| i.0.iter().find(|(k, _)| k == name).map(|(_, i)| i.clone())) else {
        return picture(cache, icons, images, name).map(|h| (h, 0.0, 0.0));
    };
    let w = src.width() as usize;
    let screen: f32 = blur_sigma();
    let radius = ((screen * w as f32 / rect_w.max(1.0)).round() as usize).clamp(1, 64);
    let key = format!("{name}|behind{radius}{}", if pad { "|pad" } else { "" });
    let border = if pad { radius * 2 } else { 0 };
    let shares = (border as f32 / w.max(1) as f32, border as f32 / (src.height() as usize).max(1) as f32);
    if let Some(k) = cache.get(&key) {
        return Some((k.clone(), shares.0, shares.1));
    }
    let mut out = if matches!(src.texture_descriptor.format, F::Rgba8Unorm | F::Rgba8UnormSrgb | F::Bgra8Unorm | F::Bgra8UnormSrgb) {
        (*src).clone()
    } else if let Some(dec) = assets::plain_rgba8(&src) {
        dec
    } else {
        return picture(cache, icons, images, name).map(|h| (h, 0.0, 0.0));
    };
    raw_texels(&mut out);
    // Only its top level: the blur and the clear border are made on that, and
    // a mip chain left on a grown picture no longer matches its size (wgpu
    // panicked on the Barracks' Prestige card behind the ConfirmPrestige
    // popup: 61680 bytes wanted, 49344 given).
    let top = out.width() as usize * out.height() as usize * 4;
    if let Some(d) = out.data.as_mut() {
        d.truncate(top);
    }
    out.texture_descriptor.mip_level_count = 1;
    let shares = if border > 0 && pad_clear(&mut out, border) { shares } else { (0.0, 0.0) };
    out.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
        ..bevy::image::ImageSamplerDescriptor::linear()
    });
    let (w, h) = (out.width() as usize, out.height() as usize);
    let plain = matches!(out.texture_descriptor.format, F::Rgba8Unorm | F::Rgba8UnormSrgb | F::Bgra8Unorm | F::Bgra8UnormSrgb);
    if plain && let Some(px) = out.data.as_mut().filter(|p| p.len() >= w * h * 4) {
        for p in px.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            for c in &mut p[..3] {
                *c = (u32::from(*c) * a / 255) as u8;
            }
        }
        crate::lui_scene::blur(px, w, h, radius);
        for p in px.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            if a > 0 {
                for c in &mut p[..3] {
                    *c = (u32::from(*c) * 255 / a).min(255) as u8;
                }
            }
        }
    }
    let handle = images.add(out);
    cache.insert(key, handle.clone());
    Some((handle, shares.0, shares.1))
}

/// bo2mp: `img` (plain RGBA8) grown by a clear border of `border` texels on
/// every side. False (untouched) when it is not plain RGBA8.
fn pad_clear(img: &mut Image, border: usize) -> bool {
    use bevy::render::render_resource::TextureFormat as F;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if !matches!(img.texture_descriptor.format, F::Rgba8Unorm | F::Rgba8UnormSrgb | F::Bgra8Unorm | F::Bgra8UnormSrgb) {
        return false;
    }
    let Some(src) = img.data.as_ref().filter(|p| p.len() >= w * h * 4) else { return false };
    let (nw, nh) = (w + 2 * border, h + 2 * border);
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..h {
        let (a, b) = (y * w * 4, ((y + border) * nw + border) * 4);
        out[b..b + w * 4].copy_from_slice(&src[a..a + w * 4]);
    }
    img.data = Some(out);
    img.texture_descriptor.size = bevy::render::render_resource::Extent3d { width: nw as u32, height: nh as u32, depth_or_array_layers: 1 };
    true
}

/// bo2mp: a BC3 picture's alpha raised to the 2.2 power (every 16-byte block
/// is independent, so the whole mip chain goes through one loop).
fn gamma_alpha_bc3(img: &mut Image) {
    use bevy::render::render_resource::TextureFormat as F;
    let fmt = img.texture_descriptor.format;
    let Some(data) = img.data.as_mut() else { return };
    let f = |a: u8| -> u8 { ((f32::from(a) / 255.0).powf(2.2) * 255.0).round() as u8 };
    match fmt {
        F::Rgba8Unorm | F::Rgba8UnormSrgb => {
            for px in data.chunks_exact_mut(4) {
                px[3] = f(px[3]);
            }
            return;
        }
        // BC2: sixteen 4-bit alphas, then the colour block.
        F::Bc2RgbaUnorm | F::Bc2RgbaUnormSrgb => {
            for blk in data.chunks_exact_mut(16) {
                for b in &mut blk[..8] {
                    let lo = f((*b & 15) * 17) / 17;
                    let hi = f((*b >> 4) * 17) / 17;
                    *b = lo.min(15) | (hi.min(15) << 4);
                }
            }
            return;
        }
        F::Bc3RgbaUnorm | F::Bc3RgbaUnormSrgb => {}
        other => {
            diag::info!(Ui, "bo2zm lui: picture alpha power skipped, format {other:?}");
            return;
        }
    }
    for blk in data.chunks_exact_mut(16) {
        let (a0, a1) = (blk[0], blk[1]);
        let mut pal = [0u8; 8];
        pal[0] = a0;
        pal[1] = a1;
        let (p0, p1) = (u32::from(a0), u32::from(a1));
        if a0 > a1 {
            for k in 1..7u32 {
                pal[(k + 1) as usize] = (((7 - k) * p0 + k * p1) / 7) as u8;
            }
        } else {
            for k in 1..5u32 {
                pal[(k + 1) as usize] = (((5 - k) * p0 + k * p1) / 5) as u8;
            }
            pal[6] = 0;
            pal[7] = 255;
        }
        let mut bits = 0u64;
        for (i, b) in blk[2..8].iter().enumerate() {
            bits |= u64::from(*b) << (8 * i);
        }
        let mut vals = [0u8; 16];
        for (i, v) in vals.iter_mut().enumerate() {
            *v = f(pal[((bits >> (3 * i)) & 7) as usize]);
        }
        let (lo, hi) = (*vals.iter().min().unwrap_or(&0), *vals.iter().max().unwrap_or(&0));
        // 8-value mode: a0 = hi > a1 = lo; a flat block keeps a0 == a1 (index 0).
        let q: [u32; 8] = std::array::from_fn(|k| {
            let k = k as u32;
            match k {
                0 => u32::from(hi),
                1 => u32::from(lo),
                _ => ((8 - k) * u32::from(hi) + (k - 1) * u32::from(lo)) / 7,
            }
        });
        let mut nb = 0u64;
        for (i, v) in vals.iter().enumerate() {
            let best = (0..8).min_by_key(|k| (q[*k] as i32 - i32::from(*v)).abs()).unwrap_or(0);
            nb |= (best as u64) << (3 * i);
        }
        blk[0] = hi;
        blk[1] = lo;
        for i in 0..6 {
            blk[2 + i] = ((nb >> (8 * i)) & 0xff) as u8;
        }
    }
}

fn picture(
    cache: &mut HashMap<String, Handle<Image>>,
    icons: Option<&assets::T6HudIcons>,
    images: &mut Assets<Image>,
    name: &str,
) -> Option<Handle<Image>> {
    // bo2mp: in an in-match menu BO2 blends in the target's gamma space, so
    // a picture's texels show darker than the front end's raw read (the
    // class screen's perk icons: real = ours^2.2). Such a picture keeps its
    // sRGB read, cached under its own key.
    let srgb = IN_MATCH_MENU_PICS.load(std::sync::atomic::Ordering::Relaxed)
        && !matches!(name, "white" | "black")
        && !name.starts_with("menu_mp_popup")
        && !name.starts_with("menu_mp_dots");
    // bo2mp: a card background (a calling card) in the front end is read
    // raw like every other front-end picture; only the match HUD's name
    // card keeps its sRGB read (see below).
    let front_card = (IN_FRONT_END_PICS.load(std::sync::atomic::Ordering::Relaxed) || UI_ENCODED.load(std::sync::atomic::Ordering::Relaxed))
        && name.starts_with("emblem_bg");
    let key: std::borrow::Cow<str> = if srgb {
        format!("{name}|srgb").into()
    } else if front_card {
        format!("{name}|front").into()
    } else {
        name.into()
    };
    if let Some(h) = cache.get(key.as_ref()) {
        return Some(h.clone());
    }
    // The engine draws a team's emblem into a picture; the Solo team's is
    // the green shield with the BO2 mark (made here, no material holds it).
    if name == LEAGUE_EMBLEM_SOLO {
        let h = images.add(league_emblem_solo());
        cache.insert(key.into_owned(), h.clone());
        return Some(h);
    }
    let Some(img) = icons?.0.iter().find(|(k, _)| k == name).map(|(_, i)| i.clone()) else {
        // (Once per name: a picture the scripts name that was not loaded.)
        static MISSING: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());
        if MISSING.lock().is_ok_and(|mut m| m.insert(name.to_owned())) {
            diag::info!(Ui, "bo2zm lui: no picture `{name}` loaded");
        }
        return None;
    };
    // Clamped at its edges: a picture is drawn once over its element, so a
    // repeating sampler only bleeds the far edge in (a dark line along the
    // bottom of the loading screen's `gradient_top`).
    let mut img = (*img).clone();
    // (bo2mp: a card's background keeps its sRGB read, or it shows far too
    // bright over the world.)
    if (!name.starts_with("emblem_bg") || front_card) && !srgb {
        raw_texels(&mut img);
    }
    // bo2mp: BO2 blends in gamma space in an in-match menu, so a picture's
    // see-through texels (a weapon's soft shadow) show as texel * alpha;
    // this linear blend would show them brighter (texel * alpha^(1/2.2)).
    // The alpha takes the same 2.2 power the element colours' alpha takes.
    if srgb {
        gamma_alpha_bc3(&mut img);
    }
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
        ..bevy::image::ImageSamplerDescriptor::linear()
    });
    let h = images.add(img);
    cache.insert(key.into_owned(), h.clone());
    Some(h)
}

/// The Solo team's emblem picture (see `picture`).
pub(crate) const LEAGUE_EMBLEM_SOLO: &str = "\u{1}bo2mp league emblem solo";

/// The Barracks' League Teams card art for the Solo team: a green shield
/// (outline, half disc, the two bars of the BO2 mark) on nothing, centred in
/// a picture as wide as the card (260 x 172).
fn league_emblem_solo() -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    // Shield drawn in a 640 x 680 box (x, y): its outer edge and inner edge.
    const OUTER: [(f32, f32); 11] = [
        (105.0, 65.0), (550.0, 65.0), (570.0, 170.0), (520.0, 222.0), (560.0, 375.0), (540.0, 478.0),
        (325.0, 625.0), (115.0, 480.0), (95.0, 375.0), (135.0, 222.0), (80.0, 160.0),
    ];
    const INNER: [(f32, f32); 11] = [
        (125.0, 90.0), (528.0, 90.0), (545.0, 172.0), (497.0, 226.0), (535.0, 378.0), (518.0, 458.0),
        (325.0, 585.0), (136.0, 458.0), (118.0, 378.0), (158.0, 226.0), (104.0, 170.0),
    ];
    fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
        let mut c = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
        c
    }
    let (w, h) = (256usize, 262usize);
    // The card draws it in a square of about 116 x 119 (the picture is
    // stretched to it): 640 x 680 box -> 288 x 306 px, centred (its drawn part is narrower).
    let scale = 306.0 / 680.0;
    let (ox, oy) = (w as f32 / 2.0 - 320.0 * scale, h as f32 / 2.0 - 340.0 * scale);
    let green = [107u8, 145, 14];
    let ss = 3usize;
    let mut data = vec![0u8; w * h * 4];
    for py in 0..h {
        for px in 0..w {
            let mut hit = 0usize;
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f32 + (sx as f32 + 0.5) / ss as f32 - ox) / scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / ss as f32 - oy) / scale;
                    let in_outer = inside(&OUTER, x, y);
                    let in_inner = inside(&INNER, x, y);
                    let disc = (x - 322.0).powi(2) + (y - 465.0).powi(2) < 180.0f32.powi(2);
                    let bar = (y > 342.0 && y < 502.0) && ((x > 278.0 && x < 320.0) || (x > 326.0 && x < 372.0));
                    if (in_outer && !in_inner) || (in_inner && disc && !bar) {
                        hit += 1;
                    }
                }
            }
            if hit > 0 {
                let i = (py * w + px) * 4;
                data[i..i + 3].copy_from_slice(&green);
                data[i + 3] = (hit * 255 / (ss * ss)) as u8;
            }
        }
    }
    Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        bevy::asset::RenderAssetUsages::default(),
    )
}

/// Set each frame: the front end is the one drawing (see `picture`).
static IN_FRONT_END_PICS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set each frame: the match UI is drawn onto the world camera, whose target
/// holds encoded values (pictures and colours go in as written).
static UI_ENCODED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static UI_ENCODED_LAST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set each frame: a multiplayer in-match menu or the match HUD is up (see `picture`).
static IN_MATCH_MENU_PICS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Read a picture's texels as stored, as the scripts' colours are (BO2's UI
/// writes both as they are): an sRGB format would turn the texels linear
/// first and every picture would show darker than BO2's (the main menu's
/// soldiers at a third of their brightness; the lobby's backdrop black).
pub(crate) fn raw_texels(img: &mut Image) {
    use bevy::render::render_resource::TextureFormat as F;
    let f = &mut img.texture_descriptor.format;
    *f = match *f {
        F::Rgba8UnormSrgb => F::Rgba8Unorm,
        F::Bgra8UnormSrgb => F::Bgra8Unorm,
        F::Bc1RgbaUnormSrgb => F::Bc1RgbaUnorm,
        F::Bc2RgbaUnormSrgb => F::Bc2RgbaUnorm,
        F::Bc3RgbaUnormSrgb => F::Bc3RgbaUnorm,
        F::Bc7RgbaUnormSrgb => F::Bc7RgbaUnorm,
        other => other,
    };
}

pub(crate) fn register_lui_hud_systems(app: &mut App) {
    app.insert_non_send(LuiHudSlot::default());
    crate::lui_additive::register(app);
    app.add_systems(Update, lui_hud.after(crate::bo2_font::load_bo2_fonts).in_set(ClientSet::Ui));
    app.add_systems(Update, crate::lui_frontend::restore_after_match.in_set(ClientSet::Ui));
}
