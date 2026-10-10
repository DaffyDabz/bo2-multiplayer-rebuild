//! bo2zm M4: the engine around Black Ops II's UI scripts, shared by the
//! game and the headless harness (`t6lua`): the scripts by name and
//! `require`, the `Engine` / `UIExpression` / `Dvar` tables the scripts
//! ask (answered from the values the host keeps: dvars, game type
//! settings, visibility bits, localized text, text widths), the LUI root
//! (`LUI.UIRoot`, sized like the engine's: 720 units high, centred), and
//! the frame step that advances animations and sends their
//! `transition_complete_<name>` events.
//!
//! A field the scripts ask that the host has no answer for is a function
//! that returns nothing, counted in `asked` (what to bind next).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::lui::{self, Rect};
use crate::value::{Table, TableRef, UserData, Value};
use crate::vm::{LuaError, Vm};

/// How wide a line of text is: (text, font file, height in units) ->
/// width in units.
pub type MeasureFn = dyn Fn(&str, &str, f32) -> f32;

/// The values the scripts read from the engine. The game writes these as
/// its state changes; the scripts read them when they run.
#[derive(Default)]
pub struct EngineValues {
    /// Dvars by name (`UIExpression.DvarString` / `DvarInt` / `DvarBool` /
    /// `DvarFloat`, `Dvar.<name>:get()`).
    pub dvars: HashMap<String, String>,
    /// `Engine.GetGametypeSetting(name)`.
    pub settings: HashMap<String, f32>,
    /// The visibility bits that are set (`CoD.BIT_*` numbers).
    pub bits: HashSet<i32>,
    /// Localized text by key (`Engine.Localize`), upper-case keys.
    pub localize: HashMap<String, String>,
    /// String tables by name (`mp/zombiemode.csv`): rows of cells
    /// (`UIExpression.TableLookup`).
    pub tables: HashMap<String, Vec<Vec<String>>>,
    /// The local player's team number (`CoD.TEAM_ALLIES`).
    pub team: i32,
    /// The scoreboard's rows: name, then the columns' values.
    pub players: Vec<Vec<String>>,
    /// The scoreboard's column names (`Engine.GetScoreBoardColumnName`).
    pub columns: Vec<String>,
    /// The safe area's size in root units (the root's own size).
    pub safe_area: (f32, f32),
    /// Each bound command's keys, as shown (`+actionslot 4` -> `4`).
    pub binds: HashMap<String, Vec<String>>,
    /// bo2mp vehicle screens: while he rides, the commands BO2 puts on the
    /// keys he has for a vehicle button's command (CG_UpdateVehicleBindings:
    /// `+vehiclemoveup` on the keys of `+frag` for the Dragonfire's
    /// RSHLDR), as `vehicle command -> command it takes its keys from`.
    pub vehicle_binds: HashMap<String, String>,
    /// The session's modes (`CoD.SESSIONMODE_*` numbers): an offline game.
    pub session_modes: Vec<f32>,
    /// An enum dvar's choices (`Dvar.<name>:getDomainEnumStrings()`).
    pub enums: HashMap<String, Vec<String>>,
    /// His profile settings (`Engine.SetProfileVar`, `UIExpression.Profile*`)
    /// and hardware profile (`Engine.*HardwareProfileValue*`).
    pub profile: HashMap<String, String>,
    /// bo2mp: the zones' config files by lower-case name, for the menus'
    /// `exec <file>` (`run_command`).
    pub configs: HashMap<String, String>,
    /// bo2mp: the stats commands those lines ran (`STAT_COMMANDS`), in
    /// order, for the stats' owner to carry out (`hks_t6::mp`).
    pub stat_commands: Vec<String>,
}

/// bo2mp: BO2's console commands that write his stats (mp/prestige_reset.cfg,
/// mp/reset_classes.cfg, mp/reset_classes_offline.cfg and the Prestige
/// menus' `Engine.ExecNow`): `run_command` queues them in
/// `EngineValues::stat_commands`.
pub const STAT_COMMANDS: [&str; 10] = [
    "equipdefaultclass",
    "equipdefaultclasstoprofile",
    "setstatfromlocstring",
    "setprofilelocclass",
    "statwriteddl",
    "prestigestatsreset",
    "prestigerequest",
    "prestigestatsresetall",
    "prestigerespec",
    "prestigeaddcac",
];

/// A config line's value: a number or word as written, or BO2's
/// `( dvarInt( <name> ) )` / `( dvarBool( <name> ) )` read from the dvars
/// (mp/prestige_reset.cfg puts `systemlink` back that way).
fn config_value(values: &RefCell<EngineValues>, text: &str) -> String {
    let t = text.trim().trim_matches('"');
    let squeezed: String = t.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    for f in ["dvarint", "dvarbool"] {
        if let Some(name) = squeezed.strip_prefix(&format!("({f}(")).and_then(|r| r.strip_suffix("))")) {
            let v = values.borrow();
            let hit = v.dvars.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, x)| x.clone());
            let n = hit.and_then(|x| x.trim().parse::<f32>().ok()).unwrap_or(0.0);
            return format!("{}", n as i64);
        }
    }
    t.to_owned()
}

/// bo2mp: carry out a console command line the menus run
/// (`Engine.Exec` / `Engine.ExecNow`) that changes engine values here:
/// `exec <file>` runs that config file's lines, `set` / `seta` / `sets`
/// <dvar> <value> sets a dvar, a stats command (`STAT_COMMANDS`) is queued
/// for the stats' owner. Others (`reset`, `restoreDvars`, binds) are left
/// to the owner. Returns whether it ran one.
pub fn run_command(values: &RefCell<EngineValues>, line: &str) -> bool {
    run_command_depth(values, line, 0)
}

fn run_command_depth(values: &RefCell<EngineValues>, line: &str, depth: usize) -> bool {
    let mut ran = false;
    for cmd in line.split(';') {
        let cmd = cmd.split("//").next().unwrap_or("").trim();
        let mut words = cmd.split_whitespace();
        let Some(verb) = words.next() else { continue };
        match verb.to_ascii_lowercase().as_str() {
            "exec" => {
                let Some(file) = words.next() else { continue };
                let file = file.trim_matches('"').to_ascii_lowercase();
                let text = values.borrow().configs.get(&file).cloned();
                if let Some(text) = text
                    && depth < 8
                {
                    for l in text.lines() {
                        run_command_depth(values, l, depth + 1);
                    }
                    ran = true;
                }
            }
            // A game type's setting (mp/gamesettings_<type>.cfg lines,
            // run when the game type is set).
            "gametype_setting" => {
                let (Some(name), Some(value)) = (words.next(), words.next().and_then(|x| x.parse::<f32>().ok())) else {
                    continue;
                };
                let mut v = values.borrow_mut();
                v.settings.insert(name.to_owned(), value);
                v.settings.insert(name.to_ascii_lowercase(), value);
                ran = true;
            }
            "set" | "seta" | "sets" => {
                let Some(name) = words.next() else { continue };
                let value = config_value(values, &words.collect::<Vec<_>>().join(" "));
                values.borrow_mut().dvars.insert(name.to_owned(), value);
                ran = true;
            }
            v if STAT_COMMANDS.contains(&v) => {
                values.borrow_mut().stat_commands.push(cmd.to_owned());
                ran = true;
            }
            _ => {}
        }
    }
    ran
}

impl EngineValues {
    /// The keys a command is on: his own binds for it, else (in a vehicle)
    /// the keys of the command its vehicle button stands for.
    pub fn keys_of(&self, command: &str) -> Option<&Vec<String>> {
        self.binds
            .get(command)
            .or_else(|| self.vehicle_binds.get(command).and_then(|from| self.binds.get(from)))
    }
}

/// What the scripts asked the engine to do (the host's owner carries it
/// out): a menu answer for the server, a console command, a sound.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineCall {
    /// `Engine.SendMenuResponse(controller, menu, response)`.
    MenuResponse(String, String),
    /// `Engine.Exec(controller, command)`.
    Exec(String),
    /// `Engine.PlaySound(alias)`.
    PlaySound(String),
    /// `Engine.BindCommand(controller, command, index)`: bind the next key
    /// he presses (then send `key_bound`).
    BindCommand(String, usize),
    /// `Engine.BlurWorld(controller, amount)`: blur the world behind the
    /// menus (0 = none).
    BlurWorld(f32),
    /// `Engine.FetchLeagueTeams(...)`: the engine answers with
    /// `league_team_info_fetched`.
    FetchLeagueTeams,
}

/// One element as drawn: its rectangle in root units (0,0 = the root's top
/// left), alpha including its parents', and what it shows.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub id: usize,
    pub kind: &'static str,
    pub rect: Rect,
    pub alpha: f32,
    pub rgb: [f32; 3],
    pub material: Option<String>,
    pub text: Option<String>,
    pub font: Option<String>,
    /// LUI.Alignment as drawn: 1 left, 2 centre, 3 right. A text that set
    /// none takes its box's anchors: pinned left only reads from the left,
    /// right only from the right, else from the middle (BO2's "+100"
    /// score popup and rank-up text, in boxes as wide as their parent;
    /// the "N POINTS TO WIN" line, a zero-width box pinned left).
    pub alignment: i32,
    pub z_rot: f32,
    /// bo2mp: turned out of the screen's plane (a 3D widget: the front
    /// end's holotable grid).
    pub x_rot: f32,
    pub y_rot: f32,
    /// A dashes bar's (count, lit, change) and units per dash.
    pub dashes: (i32, i32, i32),
    pub dash_pitch: f32,
    /// `setupTiles(n)`: the picture repeats in tiles n units wide.
    pub tiles: f32,
    /// bo2mp: the globe's `setShaderVector(0, ..)` (0 hidden, 1 wireframe, 2 map in).
    pub shader: [f32; 4],
    /// What is drawn under it shows blurred (`setBlur`).
    pub blur: bool,
    /// bo2mp: the main menu covered by Options or the Quit prompt: drawn
    /// blurred and dimmed, without its text.
    pub behind: bool,
    /// bo2mp: the box it is clipped to (an ancestor's `setUseStencil`), if any.
    pub clip: Option<Rect>,
}

pub struct Host {
    pub vm: Vm,
    pub values: Rc<RefCell<EngineValues>>,
    /// Engine fields the scripts asked that nothing answers, with counts.
    pub asked: Rc<RefCell<BTreeMap<String, usize>>>,
    /// Modules `require` found no script for.
    pub missing: Rc<RefCell<Vec<String>>>,
    /// Engine calls since the owner last took them (`take_calls`).
    calls: Rc<RefCell<Vec<EngineCall>>>,
    /// The engine's roots: `UIRoot0` (the player's) and `UIRootFull`
    /// (the whole screen), drawn in that order.
    roots: Vec<Value>,
    /// Root size in units (720 high; the width follows the screen).
    root_size: (f32, f32),
    pub errors: Vec<String>,
    /// Steps one entry into the scripts may take.
    pub step_budget: u64,
}

/// A script name as a lookup key: lower case, no separators, no
/// extension (`ui_mp/t6/hud.lua`, `ui_mp_t6_hud.lua` and
/// `ui_mp__t6__hud.lua` are one key).
fn script_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".lua").unwrap_or(&lower);
    stem.chars().filter(|c| !matches!(c, '/' | '\\' | '_' | '.')).collect()
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Nil)
}

/// The mouse keys BO2 shows as a button picture in the Controls rows, with
/// the FontIcon entry that is its glyph (BO2's own icon set names the
/// picture and its size; the text carries `^B<entry>^`).
pub const MOUSE_GLYPHS: [(&str, &str); 5] = [
    ("MOUSE1", "mouseButtonLeft"),
    ("MOUSE2", "mouseButtonRight"),
    ("MOUSE3", "mouseButtonMiddle"),
    ("MWHEELUP", "mouseWheelUp"),
    ("MWHEELDOWN", "mouseWheelDown"),
];

impl Host {
    /// A VM with the standard library, LUI's natives and the engine tables,
    /// and `scripts` ((rawfile name, bytes); a later name wins) behind
    /// `require`. Nothing runs yet (`load_base`).
    #[allow(clippy::too_many_lines)]
    pub fn new(scripts: Vec<(String, Vec<u8>)>, measure: Box<MeasureFn>) -> Host {
        let mut vm = Vm::new();
        crate::stdlib::open(&mut vm);
        let asked: Rc<RefCell<BTreeMap<String, usize>>> = Rc::default();
        let values: Rc<RefCell<EngineValues>> = Rc::default();
        let missing: Rc<RefCell<Vec<String>>> = Rc::default();
        for name in ["Engine", "UIExpression"] {
            let v = stub_table(&mut vm, name, asked.clone());
            vm.set_global(name, v);
        }
        lui::install(&mut vm);

        // Dvar.<name>: :get() reads the host's dvar, :set(v) writes it.
        {
            let dvars = Table::new_ref();
            let meta = Table::new_ref();
            let vals = values.clone();
            let index = vm.native("dvar_index", move |vm, a| {
                let name = arg(&a, 1).to_string();
                let d = Table::new_ref();
                let (v1, v2) = (vals.clone(), vals.clone());
                let n1 = name.clone();
                let get = vm.native("dvar_get", move |_, _| {
                    let v = v1.borrow().dvars.get(&n1).cloned();
                    // A colour dvar ("0.6 0.64 0.69") answers its r, g, b (the team colours the
                    // scripts read as `local r, g, b = Dvar.g_TeamColor_MyTeam:get()`).
                    if n1.starts_with("g_TeamColor_")
                        && let Some(rgb) = v.as_deref().map(|s| s.split_whitespace().filter_map(|x| x.parse::<f32>().ok()).collect::<Vec<_>>()).filter(|c| c.len() >= 3)
                    {
                        return Ok(rgb.into_iter().take(3).map(Value::Num).collect());
                    }
                    // (The engine's boolean dvars answer true/false; the
                    // scripts compare them with `== true`.)
                    let boolean = matches!(n1.as_str(), "ui_inGameStoreVisible" | "demo_recordPrivateMatch" | "tu7_scoreboardPingAsNumbers");
                    Ok(v.map(|s| match s.parse::<f32>() {
                        Ok(n) if boolean => Value::Bool(n != 0.0),
                        Ok(n) => Value::Num(n),
                        Err(_) => Value::str(&s),
                    })
                    .into_iter()
                    .collect())
                });
                let set = vm.native("dvar_set", move |_, a| {
                    v2.borrow_mut().dvars.insert(name.clone(), arg(&a, 1).to_string());
                    Ok(vec![])
                });
                // getDomainEnumStrings(): an enum dvar's choices
                // (`enums`, e.g. r_mode's resolutions).
                let v3 = vals.clone();
                let n3 = arg(&a, 1).to_string();
                let domain = vm.native("dvar_domain", move |_, _| {
                    let t = Table::new_ref();
                    for (i, c) in v3.borrow().enums.get(&n3).into_iter().flatten().enumerate() {
                        t.borrow_mut().set(Value::Num((i + 1) as f32), Value::str(c));
                    }
                    Ok(vec![Value::Table(t)])
                });
                d.borrow_mut().set_str("get", get);
                d.borrow_mut().set_str("set", set);
                d.borrow_mut().set_str("getDomainEnumStrings", domain);
                Ok(vec![Value::Table(d)])
            });
            meta.borrow_mut().set_str("__index", index);
            dvars.borrow_mut().meta = Some(meta);
            vm.set_global("Dvar", Value::Table(dvars));
        }

        // UIExpression: dvars and visibility bits.
        let ui = vm.global("UIExpression");
        let dvar = |vals: &Rc<RefCell<EngineValues>>, a: &[Value]| {
            vals.borrow().dvars.get(&arg(a, 1).to_string()).cloned().unwrap_or_default()
        };
        let v = values.clone();
        let f = vm.native("DvarString", move |_, a| Ok(vec![Value::str(&dvar(&v, &a))]));
        set_field(&ui, "DvarString", f);
        for name in ["DvarInt", "DvarFloat"] {
            let v = values.clone();
            let f = vm.native(name, move |_, a| {
                Ok(vec![Value::Num(dvar(&v, &a).trim().parse::<f32>().unwrap_or(0.0))])
            });
            set_field(&ui, name, f);
        }
        let v = values.clone();
        let f = vm.native("DvarBool", move |_, a| {
            let s = dvar(&v, &a);
            let on = matches!(s.trim(), "1" | "true") || s.trim().parse::<f32>().is_ok_and(|n| n != 0.0);
            Ok(vec![Value::Num(if on { 1.0 } else { 0.0 })])
        });
        set_field(&ui, "DvarBool", f);
        let v = values.clone();
        let f = vm.native("IsVisibilityBitSet", move |_, a| {
            let bit = arg(&a, 1).as_num().map_or(-1, |n| n as i32);
            Ok(vec![Value::Num(if v.borrow().bits.contains(&bit) { 1.0 } else { 0.0 })])
        });
        set_field(&ui, "IsVisibilityBitSet", f);
        // ToUpper(controller, text).
        let f = vm.native("ToUpper", |_, a| {
            let text = if a.len() >= 2 { arg(&a, 1) } else { arg(&a, 0) };
            Ok(vec![Value::str(&text.to_string().to_uppercase())])
        });
        set_field(&ui, "ToUpper", f);
        // A zombies game on PC (the multiplayer exe runs zombies).
        let f = vm.native("GetCurrentPlatform", |_, _| Ok(vec![Value::str("pc")]));
        set_field(&ui, "GetCurrentPlatform", f);
        let f = vm.native("GetCurrentExe", |_, _| Ok(vec![Value::str("multiplayer")]));
        set_field(&ui, "GetCurrentExe", f);
        let f = vm.native("SessionMode_IsZombiesGame", |_, _| Ok(vec![Value::Num(1.0)]));
        set_field(&ui, "SessionMode_IsZombiesGame", f);
        // TableLookup(controller, table, column, value, return column).
        let v = values.clone();
        let f = vm.native("TableLookup", move |_, a| {
            let table = arg(&a, 1).to_string();
            let col = arg(&a, 2).as_num().map_or(0, |n| n as usize);
            let want = arg(&a, 3).to_string();
            let ret = arg(&a, 4).as_num().map_or(0, |n| n as usize);
            let v = v.borrow();
            // Table names are case-blind (`mp/rankIconTable.csv`).
            let rows = v.tables.get(&table).or_else(|| {
                v.tables.iter().find(|(k, _)| k.eq_ignore_ascii_case(&table)).map(|(_, r)| r)
            });
            let hit = rows
                .and_then(|rows| rows.iter().find(|r| r.get(col).is_some_and(|c| *c == want)))
                .and_then(|r| r.get(ret).cloned())
                .unwrap_or_default();
            Ok(vec![Value::str(&hit)])
        });
        set_field(&ui, "TableLookup", f);
        // The maps table a zombies game reads (CoD.mapsTable).
        let f = vm.native("GetCurrentMapTableName", |_, _| Ok(vec![Value::str("zm/mapstable.csv")]));
        set_field(&ui, "GetCurrentMapTableName", f);

        // Engine: the safe area, settings, localized text.
        let engine = vm.global("Engine");
        // The safe area: the whole root (a PC screen has no overscan), in
        // root units. ForController: its edges around the centre;
        // GetUserSafeArea: its width and height.
        let v = values.clone();
        let f = vm.native("GetUserSafeAreaForController", move |_, _| {
            let (w, h) = v.borrow().safe_area;
            Ok(vec![Value::Num(-w / 2.0), Value::Num(-h / 2.0), Value::Num(w / 2.0), Value::Num(h / 2.0)])
        });
        set_field(&engine, "GetUserSafeAreaForController", f);
        let v = values.clone();
        let f = vm.native("GetUserSafeArea", move |_, _| {
            let (w, h) = v.borrow().safe_area;
            Ok(vec![Value::Num(w), Value::Num(h)])
        });
        set_field(&engine, "GetUserSafeArea", f);
        let v = values.clone();
        let f = vm.native("GetGametypeSetting", move |_, a| {
            Ok(vec![Value::Num(v.borrow().settings.get(&arg(&a, 0).to_string()).copied().unwrap_or(0.0))])
        });
        set_field(&engine, "GetGametypeSetting", f);
        let v = values.clone();
        let strict = std::env::var_os("BO2MP_STRICT_LOCALIZE").is_some();
        let f = vm.native("Localize", move |_, a| {
            // BO2MP_STRICT_LOCALIZE=1: Localize(nil) is an error with its
            // trace (debugging aid: which engine answer came back nil).
            if strict && matches!(arg(&a, 0), Value::Nil) {
                return Err(LuaError::new("Localize(nil)".to_owned()));
            }
            let key = arg(&a, 0).to_string();
            let mut text = v
                .borrow()
                .localize
                .get(&key.to_ascii_uppercase())
                .cloned()
                .unwrap_or(key);
            // "&&1".. take the arguments; an argument that is a text key is
            // its text (Create-a-Class's camo challenges pass the weapon's
            // name key: "Get 5 Headshot medals with MTAR.").
            for (i, x) in a.iter().enumerate().skip(1) {
                let s = x.to_string();
                let s = match x {
                    Value::Str(_) => v.borrow().localize.get(&s.to_ascii_uppercase()).cloned().unwrap_or(s),
                    _ => s,
                };
                text = text.replace(&format!("&&{i}"), &s);
            }
            // bo2mp vehicle screens: a `[{+command}]` in the text is the key
            // his binds put that command on, by its KEY_* name ("Hold [F] to
            // Exit", the VTOL Warship's MP_CHOPPER_GUNNER_* prompts); a
            // command with no key shows BO2's own KEY_UNBOUND text
            // ("UNBOUND", the word its Controls rows show for one). The
            // sticks are keyboard-and-mouse words: the look stick is the
            // mouse (MENU_MOUSE_LOOK, the Hellstorm's "Steer"), the move
            // stick the four movement keys.
            let key_name = |cmd: &str| {
                let key = v.borrow().keys_of(cmd).and_then(|k| k.first().cloned());
                match key {
                    Some(key) => {
                        let up = key.to_uppercase();
                        v.borrow().localize.get(&format!("KEY_{up}")).cloned().unwrap_or(up)
                    }
                    None => v.borrow().localize.get("KEY_UNBOUND").cloned().unwrap_or_else(|| "UNBOUND".to_owned()),
                }
            };
            let mut from = 0;
            while let Some(start) = text[from..].find("[{").map(|s| s + from) {
                let Some(len) = text[start..].find("}]") else { break };
                let cmd = text[start + 2..start + len].to_owned();
                let name = if cmd.eq_ignore_ascii_case("+lookstick") {
                    v.borrow().localize.get("MENU_MOUSE_LOOK").cloned().unwrap_or_else(|| "Mouse Look".to_owned())
                } else if cmd.eq_ignore_ascii_case("+movestick") {
                    ["+forward", "+back", "+moveleft", "+moveright"].map(key_name).join(",")
                } else {
                    key_name(&cmd)
                };
                text.replace_range(start..start + len + 2, &format!("[{name}]"));
                from = start + name.len() + 2;
            }
            Ok(vec![Value::str(&text)])
        });
        set_field(&engine, "Localize", f);
        let falsy = vm.native("false", |_, _| Ok(vec![Value::Bool(false)]));
        for name in ["GameModeIsMode", "IsSplitscreen", "IsDemoShoutcaster", "IsInGame"] {
            set_field(&engine, name, falsy.clone());
        }
        // SessionModeIsMode(mode): the session's modes (`session_modes`).
        let v = values.clone();
        let f = vm.native("SessionModeIsMode", move |_, a| {
            let m = arg(&a, 0).as_num().unwrap_or(f32::NAN);
            Ok(vec![Value::Bool(v.borrow().session_modes.contains(&m))])
        });
        set_field(&engine, "SessionModeIsMode", f);
        let zero = vm.native("zero", |_, _| Ok(vec![Value::Num(0.0)]));
        for name in ["GetPlayerCount", "GetAspectRatio"] {
            set_field(&engine, name, zero.clone());
        }
        for name in ["LastInput_Gamepad", "PartyConnectingToDedicated"] {
            set_field(&engine, name, falsy.clone());
        }
        let nothing = vm.native("nothing", |_, _| Ok(vec![]));
        for name in ["SetDvar", "PlayMenuMusic", "SetForceMouseRootFull", "StopEditingPresetClass"] {
            set_field(&engine, name, nothing.clone());
        }
        // What the menus ask of the engine, queued for the owner.
        let calls: Rc<RefCell<Vec<EngineCall>>> = Rc::default();
        let c = calls.clone();
        let f = vm.native("SendMenuResponse", move |_, a| {
            c.borrow_mut().push(EngineCall::MenuResponse(arg(&a, 1).to_string(), arg(&a, 2).to_string()));
            Ok(vec![])
        });
        set_field(&engine, "SendMenuResponse", f);
        let c = calls.clone();
        let f = vm.native("Exec", move |_, a| {
            // Exec(controller, command) or Exec(command).
            let cmd = a.iter().rev().find_map(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            c.borrow_mut().push(EngineCall::Exec(cmd));
            Ok(vec![])
        });
        set_field(&engine, "Exec", f);
        // LoadCodtvDWContent(controller, folderIndex, offset, userData): the
        // online file list's request; offline the answer is at once (no
        // files), raised by the UI as `fileshare_search_complete`.
        let c = calls.clone();
        let f = vm.native("LoadCodtvDWContent", move |_, a| {
            let folder = arg(&a, 1).as_num().unwrap_or(0.0) as i64;
            c.borrow_mut().push(EngineCall::Exec(format!("bo2mpDwSearch {folder}")));
            Ok(vec![Value::Bool(true)])
        });
        set_field(&engine, "LoadCodtvDWContent", f);
        let c = calls.clone();
        let f = vm.native("PlaySound", move |_, a| {
            c.borrow_mut().push(EngineCall::PlaySound(arg(&a, 0).to_string()));
            Ok(vec![])
        });
        set_field(&engine, "PlaySound", f);
        // A solo game pauses (one local player, no one else in the game).
        let f = vm.native("CanPauseZombiesGame", |_, _| Ok(vec![Value::Bool(true)]));
        set_field(&engine, "CanPauseZombiesGame", f);
        let c = calls.clone();
        let f = vm.native("BlurWorld", move |_, a| {
            let amount = match a.get(1) {
                Some(Value::Num(n)) => *n,
                _ => 0.0,
            };
            c.borrow_mut().push(EngineCall::BlurWorld(amount));
            Ok(vec![])
        });
        set_field(&engine, "BlurWorld", f);
        let c = calls.clone();
        let f = vm.native("FetchLeagueTeams", move |_, _| {
            c.borrow_mut().push(EngineCall::FetchLeagueTeams);
            Ok(vec![])
        });
        set_field(&engine, "FetchLeagueTeams", f);
        for name in ["IsMigrating", "LockInput", "SetUIActive", "ProbationCheckIfPenalizedForQuit"] {
            set_field(&engine, name, if name == "IsMigrating" || name == "ProbationCheckIfPenalizedForQuit" { falsy.clone() } else { nothing.clone() });
        }
        let one = vm.native("one", |_, _| Ok(vec![Value::Num(1.0)]));
        set_field(&ui, "IsInGame", one.clone());
        // His keyboard and mouse are controller 0 (menus pass on input only
        // from a controller in use).
        set_field(&ui, "IsControllerBeingUsed", one.clone());
        // One local player.
        set_field(&ui, "SplitscreenNum", one.clone());
        set_field(&engine, "PartyGetPlayerCount", one.clone());
        set_field(&engine, "GetClientNum", zero.clone());
        set_field(&ui, "IsGuest", zero.clone());
        // The scripts' debug output (a release build prints nothing).
        vm.set_global("DebugPrint", nothing.clone());
        for name in ["IsDemoPlaying", "SessionMode_IsOnlineGame"] {
            set_field(&ui, name, zero.clone());
        }
        // The scoreboard: one team (CoD.TEAM_ALLIES, set by `load_base`'s
        // caller through `players`), its players.
        let v = values.clone();
        let f = vm.native("GetTeamPositions", move |_, _| {
            let t = Table::new_ref();
            let row = Table::new_ref();
            row.borrow_mut().set_str("team", Value::Num(v.borrow().team as f32));
            t.borrow_mut().set(Value::Num(1.0), Value::Table(row));
            Ok(vec![Value::Table(t)])
        });
        set_field(&engine, "GetTeamPositions", f);
        let v = values.clone();
        let f = vm.native("GetMatchScoreboardClientCount", move |_, _| {
            Ok(vec![Value::Num(v.borrow().players.len().max(1) as f32)])
        });
        set_field(&engine, "GetMatchScoreboardClientCount", f);
        let v = values.clone();
        let f = vm.native("GetScoreBoardColumnName", move |_, a| {
            let i = arg(&a, 1).as_num().map_or(0, |n| n as usize);
            Ok(vec![Value::str(v.borrow().columns.get(i).map_or("", String::as_str))])
        });
        set_field(&engine, "GetScoreBoardColumnName", f);
        // Weapons (the HUD passes the weapon the engine named in
        // `hud_update_weapon`: here its name).
        let f = vm.native("IsWeaponType", |_, a| {
            let w = arg(&a, 0).to_string();
            let hit = match arg(&a, 1).to_string().as_str() {
                "melee" => ["knife", "bowie", "tazer", "fists", "sickle"].iter().any(|k| w.contains(k)),
                "grenade" => ["grenade", "monkey", "claymore", "emp"].iter().any(|k| w.contains(k)),
                _ => false,
            };
            Ok(vec![Value::Bool(hit)])
        });
        set_field(&engine, "IsWeaponType", f);
        set_field(&engine, "IsOverheatWeapon", falsy.clone());
        set_field(&engine, "IsShoutcaster", falsy.clone());
        set_field(&engine, "ForceHUDRefresh", nothing.clone());
        set_field(&engine, "GetActiveLocalClientsCount", one.clone());
        // The key a command is bound to (the string argument naming it).
        // (controller, command[, index]): the index-th key (0 first).
        for (table, name) in [(&engine, "GetKeyBindingLocalizedString"), (&ui, "KeyBinding")] {
            let v = values.clone();
            let f = vm.native(name, move |_, a| {
                let at = a.iter().position(|x| x.as_str().is_some());
                let cmd = at.and_then(|i| a[i].as_str()).unwrap_or("").to_owned();
                let index = at.and_then(|i| a.get(i + 1)).and_then(Value::as_num).map_or(0, |n| n as usize);
                let keys: Vec<String> = v.borrow().keys_of(&cmd).map(|k| k.iter().skip(index).cloned().collect()).unwrap_or_default();
                let key = keys.first().cloned().unwrap_or_default();
                // The Controls rows ask with every flag (nine arguments) and
                // show what comes back: BO2 says UNBOUND (its KEY_UNBOUND
                // text) for a command with no key (the footer prompts ask
                // with three and want "").
                if key.is_empty() && a.len() >= 9 {
                    let unbound = v.borrow().localize.get("KEY_UNBOUND").cloned();
                    return Ok(vec![Value::str(unbound.as_deref().unwrap_or("UNBOUND"))]);
                }
                if a.len() < 9 {
                    return Ok(vec![Value::str(&key.to_uppercase())]);
                }
                // A row shows every key the command is on, joined by BO2's
                // own KEY_OR; each is its KEY_* name, a mouse button its
                // glyph (^B<FontIcon entry>^).
                let or = v.borrow().localize.get("KEY_OR").cloned().unwrap_or_else(|| "OR".to_owned());
                let shown: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        if let Some((_, glyph)) = MOUSE_GLYPHS.iter().find(|(m, _)| k.eq_ignore_ascii_case(m)) {
                            return format!("^B{glyph}^");
                        }
                        let up = k.to_uppercase();
                        v.borrow().localize.get(&format!("KEY_{up}")).cloned().unwrap_or(up)
                    })
                    .collect();
                // BO2's key table has one SHIFT, one CTRL and one ALT
                // (KEY_SHIFT/KEY_CTRL/KEY_ALT, no left/right names), so a
                // command on both physical keys lists that name once.
                let mut once: Vec<String> = Vec::new();
                for s in shown {
                    if !once.contains(&s) {
                        once.push(s);
                    }
                }
                Ok(vec![Value::str(&once.join(&format!(" {or} ")))])
            });
            set_field(table, name, f);
        }
        // Profile and hardware-profile settings: kept as text by name.
        for (name, arg_at) in [("SetProfileVar", 1usize), ("SetHardwareProfileValue", 0)] {
            let v = values.clone();
            let f = vm.native(name, move |_, a| {
                let key = arg(&a, arg_at).to_string();
                v.borrow_mut().profile.insert(key, arg(&a, arg_at + 1).to_string());
                Ok(vec![])
            });
            set_field(&engine, name, f);
        }
        let v = values.clone();
        let f = vm.native("GetHardwareProfileValueAsString", move |_, a| {
            let key = arg(&a, 0).to_string();
            let vb = v.borrow();
            Ok(vec![Value::str(vb.profile.get(&key).or_else(|| vb.dvars.get(&key)).map_or("0", String::as_str))])
        });
        set_field(&engine, "GetHardwareProfileValueAsString", f);
        set_field(&engine, "SyncHardwareProfileWithDvars", nothing.clone());
        for (name, number) in [("ProfileValueAsString", false), ("ProfileInt", true), ("ProfileFloat", true), ("ProfileBool", true)] {
            let v = values.clone();
            let f = vm.native(name, move |_, a| {
                let key = arg(&a, 1).to_string();
                let s = v.borrow().profile.get(&key).cloned().unwrap_or_default();
                Ok(vec![if number { Value::Num(s.trim().parse().unwrap_or(0.0)) } else { Value::str(&s) }])
            });
            set_field(&ui, name, f);
        }
        // BindCommand(controller, command, index): the next key he presses.
        let c = calls.clone();
        let f = vm.native("BindCommand", move |_, a| {
            let cmd = arg(&a, 1).to_string();
            c.borrow_mut().push(EngineCall::BindCommand(cmd, arg(&a, 2).as_num().map_or(0, |n| n as usize)));
            Ok(vec![])
        });
        set_field(&engine, "BindCommand", f);
        // ExecNow(controller, command): config files and dvars here.
        let v = values.clone();
        let f = vm.native("ExecNow", move |_, a| {
            let cmd = a.iter().rev().find_map(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            run_command(&v, &cmd);
            Ok(vec![])
        });
        set_field(&engine, "ExecNow", f);

        // GetTextDimensions(text, font, height) -> left, top, right, bottom.
        let f = vm.native("GetTextDimensions", move |_, a| {
            let text = arg(&a, 0).to_string();
            let font = match arg(&a, 1) {
                Value::User(u) => font_name(&u).unwrap_or_default(),
                other => other.to_string(),
            };
            let h = arg(&a, 2).as_num().unwrap_or(0.0);
            Ok(vec![Value::Num(0.0), Value::Num(0.0), Value::Num(measure(&text, &font, h)), Value::Num(h)])
        });
        vm.set_global("GetTextDimensions", f);

        // require(module): the script named like it under ui/ or ui_mp/.
        let mut by_key: HashMap<String, Rc<[u8]>> = HashMap::new();
        for (name, bytes) in scripts {
            if name.to_ascii_lowercase().ends_with(".lua") {
                by_key.insert(script_key(&name), Rc::from(bytes));
            }
        }
        let loaded = Table::new_ref();
        let miss = missing.clone();
        let require = vm.native("require", move |vm, a| {
            let module = arg(&a, 0).to_string();
            let hit = loaded.borrow().get_str(&module);
            if hit.truthy() {
                return Ok(vec![hit]);
            }
            let stem = script_key(&module);
            let Some(bytes) = ["ui", "uimp"].iter().find_map(|p| by_key.get(&format!("{p}{stem}"))).cloned()
            else {
                // The engine's require skips a script the zones lack (the
                // zombies HUD names multiplayer-only ones).
                miss.borrow_mut().push(module);
                return Ok(vec![]);
            };
            loaded.borrow_mut().set_str(&module, Value::Bool(true));
            let chunk = crate::parse(&bytes).map_err(|e| LuaError::new(format!("{module}: {e}")))?;
            let f = vm.load(chunk.main);
            let r = vm.call(f, vec![])?;
            let v = r.into_iter().next().filter(Value::truthy).unwrap_or(Value::Bool(true));
            loaded.borrow_mut().set_str(&module, v.clone());
            Ok(vec![v])
        });
        vm.set_global("require", require);

        // The fonts' resolution (`fonts/720/...`), read while CoD's base
        // loads.
        values.borrow_mut().dvars.insert("r_fontResolution".to_owned(), "720".to_owned());
        values.borrow_mut().safe_area = (1280.0, 720.0);
        Host {
            vm,
            values,
            asked,
            missing,
            calls,
            roots: Vec::new(),
            root_size: (1280.0, 720.0),
            errors: Vec::new(),
            step_budget: 20_000_000,
        }
    }

    /// Call into the scripts with a fresh step budget; an error is kept in
    /// `errors` (the scripts go on, as the engine's do).
    pub fn call(&mut self, f: Value, args: Vec<Value>) -> Option<Vec<Value>> {
        self.vm.steps = 0;
        self.vm.step_limit = self.step_budget;
        match self.vm.call(f, args) {
            Ok(r) => Some(r),
            Err(e) => {
                if self.errors.len() < 200 {
                    self.errors.push(e.to_string());
                }
                None
            }
        }
    }

    pub fn require(&mut self, module: &str) -> Option<Value> {
        let r = self.vm.global("require");
        self.call(r, vec![Value::str(module)]).map(|v| v.into_iter().next().unwrap_or(Value::Nil))
    }

    /// LUI's core and CoD's base, as the engine loads them first.
    pub fn load_base(&mut self) {
        let metas: Vec<(&str, Option<TableRef>)> = ["Engine", "UIExpression"]
            .into_iter()
            .map(|n| (n, self.vm.global(n).table().and_then(|t| t.borrow().meta.clone())))
            .collect();
        for base in ["LUI.LUI", "T6.CoDBase"] {
            self.require(base);
        }
        // CoD's base answers a missing engine field with CoD.NullFunction;
        // ours does the same and counts it.
        for (n, meta) in metas {
            if let Some(t) = self.vm.global(n).table() {
                t.borrow_mut().meta = meta;
            }
        }
    }

    /// Set a function on an engine table (`Engine`, `UIExpression`, ...).
    pub fn bind(
        &mut self,
        table: &str,
        name: &'static str,
        f: impl Fn(&mut Vm, Vec<Value>) -> Result<Vec<Value>, LuaError> + 'static,
    ) {
        let t = self.vm.global(table);
        let v = self.vm.native(name, f);
        set_field(&t, name, v);
    }

    /// A material, as the scripts' `RegisterMaterial(name)` makes it.
    pub fn material(&mut self, name: &str) -> Value {
        let f = self.vm.global("RegisterMaterial");
        self.call(f, vec![Value::str(name)]).and_then(|r| r.into_iter().next()).unwrap_or(Value::Nil)
    }

    /// A field of a global table (`CoD.BIT_HUD_VISIBLE`).
    pub fn field(&mut self, global: &str, name: &str) -> Value {
        let g = self.vm.global(global);
        self.vm.index(&g, &Value::str(name)).unwrap_or(Value::Nil)
    }

    /// The player's root (`LUI.UIRoot.new("UIRoot0")`, with `UIRootFull`
    /// beside it), made on first use and sized for a screen of this aspect
    /// (width / height).
    pub fn root(&mut self, aspect: f32) -> Option<Value> {
        let w = 720.0 * aspect.max(0.5);
        if self.roots.is_empty() {
            let ui_root = self.field("LUI", "UIRoot");
            let new = self.vm.index(&ui_root, &Value::str("new")).ok()?;
            for name in ["UIRoot0", "UIRootFull"] {
                let root = self.call(new.clone(), vec![Value::str(name)])?.into_iter().next()?;
                lui::add_root(lui::element(&root)?);
                self.roots.push(root);
            }
            self.root_size = (0.0, 0.0);
        }
        if (self.root_size.0 - w).abs() > 0.5 {
            self.root_size = (w, 720.0);
            self.values.borrow_mut().safe_area = (w, 720.0);
            lui::set_root_rect([0.0, 0.0, w, 720.0]);
            for root in self.roots.clone() {
                self.event(
                    &root,
                    "resize",
                    &[("width", Value::Num(w)), ("height", Value::Num(720.0)), ("unitsToPixels", Value::Num(1.0))],
                );
            }
        }
        self.roots.first().cloned()
    }

    /// Send `name` with `fields` to an element (`processEvent`).
    pub fn event(&mut self, target: &Value, name: &str, fields: &[(&str, Value)]) {
        let t = Table::new_ref();
        t.borrow_mut().set_str("name", Value::str(name));
        t.borrow_mut().set_str("controller", Value::Num(0.0));
        for (k, v) in fields {
            t.borrow_mut().set_str(k, v.clone());
        }
        let pe = self.vm.index(target, &Value::str("processEvent")).unwrap_or(Value::Nil);
        self.call(pe, vec![target.clone(), Value::Table(t)]);
    }

    /// Send an event to the root (and so to every menu on it).
    pub fn root_event(&mut self, name: &str, fields: &[(&str, Value)]) {
        if let Some(root) = self.roots.first().cloned() {
            self.event(&root, name, fields);
        }
    }

    /// An event whose fields are a table already made (a name card the
    /// engine sends as the event itself: `player_obituary_callout`).
    pub fn root_event_table(&mut self, name: &str, t: TableRef) {
        let Some(root) = self.roots.first().cloned() else { return };
        t.borrow_mut().set_str("name", Value::str(name));
        t.borrow_mut().set_str("controller", Value::Num(0.0));
        let pe = self.vm.index(&root, &Value::str("processEvent")).unwrap_or(Value::Nil);
        self.call(pe, vec![root, Value::Table(t)]);
    }

    /// An event whose data is a list (`hud_update_rewards`: the event is
    /// indexed 1, 2, 3 ... as the scripts walk it).
    pub fn root_event_list(&mut self, name: &str, items: Vec<Value>) {
        let Some(root) = self.roots.first().cloned() else { return };
        let t = Table::new_ref();
        t.borrow_mut().set_str("name", Value::str(name));
        t.borrow_mut().set_str("controller", Value::Num(0.0));
        for (i, v) in items.into_iter().enumerate() {
            t.borrow_mut().set(Value::Num((i + 1) as f32), v);
        }
        let pe = self.vm.index(&root, &Value::str("processEvent")).unwrap_or(Value::Nil);
        self.call(pe, vec![root, Value::Table(t)]);
    }

    /// The mouse, as the engine sends it to the root: `name` is
    /// mousemove / mousedown / mouseup, (x, y) in root units, `button`
    /// left / right.
    pub fn mouse(&mut self, name: &str, x: f32, y: f32, button: &str) {
        self.root_event(
            name,
            &[
                ("rootName", Value::str("UIRoot0")),
                ("x", Value::Num(x)),
                ("y", Value::Num(y)),
                ("button", Value::str(button)),
            ],
        );
    }

    /// A pad button (or the key that stands for it): primary, secondary,
    /// start, up, down, left, right, ...
    pub fn button(&mut self, button: &str, down: bool) {
        self.root_event(
            "gamepad_button",
            &[("button", Value::str(button)), ("down", Value::Bool(down)), ("qualifier", Value::str("keyboard"))],
        );
    }

    /// The engine calls the scripts made since the last time.
    pub fn take_calls(&mut self) -> Vec<EngineCall> {
        std::mem::take(&mut *self.calls.borrow_mut())
    }

    /// The menus open over the HUD (the root's children other than the HUD
    /// menu: `Menu.class`, popups).
    pub fn open_menus(&self) -> Vec<String> {
        let Some(root) = self.roots.first().and_then(lui::element) else {
            return Vec::new();
        };
        lui::child_ids(&root)
            .into_iter()
            .filter(|id| id.starts_with("Menu.") && id != "Menu.HUD")
            .collect()
    }

    /// bo2mp: close the open menu whose id is `id` (e.g. a confirm
    /// prompt's `Menu.SetDefaultPopup`; the newest if several) as its own NO
    /// does (`CoD.PopupMenus.GoBack(menu, { controller = 0 })`); false if
    /// none is open.
    pub fn close_popup(&mut self, id: &str) -> bool {
        fn find(u: &Rc<UserData>, id: &str, out: &mut Option<Rc<UserData>>) {
            if lui::with(u, |e| e.fields.borrow().get_str("id").as_str().is_some_and(|i| i == id)) {
                *out = Some(u.clone());
            }
            for k in lui::with(u, |e| e.children.clone()) {
                find(&k, id, out);
            }
        }
        let mut found = None;
        for root in self.roots.clone() {
            if let Some(r) = lui::element(&root) {
                find(&r, id, &mut found);
            }
        }
        let Some(menu) = found else {
            return false;
        };
        let popups = self.field("CoD", "PopupMenus");
        let Ok(go_back) = self.vm.index(&popups, &Value::str("GoBack")) else {
            return false;
        };
        let event = crate::value::Table::new_ref();
        event.borrow_mut().set_str("controller", Value::Num(0.0));
        self.call(go_back, vec![Value::User(menu), Value::Table(event)]).is_some()
    }

    /// bo2mp: set the text of an engine-fed label of an open menu (the
    /// Loading screen's `statusLabel`: the engine writes its connect
    /// status there). False if the menu or label is not there.
    pub fn set_menu_label_text(&mut self, menu_id: &str, field: &str, text: &str) -> bool {
        fn find(u: &Rc<UserData>, id: &str, out: &mut Option<Rc<UserData>>) {
            if lui::with(u, |e| e.fields.borrow().get_str("id").as_str().is_some_and(|i| i == id)) {
                *out = Some(u.clone());
            }
            for k in lui::with(u, |e| e.children.clone()) {
                find(&k, id, out);
            }
        }
        let mut found = None;
        for root in self.roots.clone() {
            if let Some(r) = lui::element(&root) {
                find(&r, menu_id, &mut found);
            }
        }
        let Some(menu) = found else { return false };
        let Ok(label) = self.vm.index(&Value::User(menu), &Value::str(field)) else { return false };
        if matches!(label, Value::Nil) {
            return false;
        }
        let Ok(set) = self.vm.index(&label, &Value::str("setText")) else { return false };
        self.call(set, vec![label, Value::str(text)]).is_some()
    }

    /// Open a menu on the root (the root's `addmenu`), as the engine does.
    pub fn open_menu(&mut self, menu: &str) {
        self.root_event("addmenu", &[("menu", Value::str(menu))]);
    }

    /// One frame at `now_ms`: start the animations the scripts wrote, move
    /// every animation on, and send each one that ended its
    /// `transition_complete_<name>` (the handlers may start more, so this
    /// repeats a few times).
    pub fn frame(&mut self, now_ms: f64) {
        for (el, ev) in lui::stream_events(now_ms) {
            self.event(&Value::User(el), ev, &[]);
        }
        for _ in 0..4 {
            lui::commit_pending();
            let done = lui::tick(now_ms);
            if done.is_empty() {
                break;
            }
            for (el, anim, interrupted, late) in done {
                let target = Value::User(el);
                self.event(
                    &target,
                    &format!("transition_complete_{anim}"),
                    &[("interrupted", Value::Bool(interrupted)), ("lateness", Value::Num(late as f32))],
                );
            }
        }
    }

    /// LUI's clock (the last frame's time).
    pub fn now_ms(&self) -> f64 {
        lui::now_ms()
    }

    /// The element with this id, as a value (debugging aid).
    pub fn element_by_id(&self, id: usize) -> Option<Value> {
        self.roots.iter().filter_map(lui::element).find_map(|r| lui::find(&r, id)).map(Value::User)
    }

    /// The element trees under the roots (debugging aid).
    pub fn tree(&self) -> Vec<String> {
        let mut out = Vec::new();
        for r in self.roots.iter().filter_map(lui::element) {
            lui::tree(&r, 0, &mut out);
        }
        out
    }

    /// Every element in drawing order, in root units.
    pub fn drawn(&self) -> Vec<Drawn> {
        let (w, h) = self.root_size;
        self.roots
            .iter()
            .filter_map(lui::element)
            .flat_map(|root| lui::layout_clipped(&root, [0.0, 0.0, w, h]))
            .map(|(u, rect, alpha, clip)| drawn_of(&u, rect, alpha, clip))
            .collect()
    }
}

fn drawn_of(u: &Rc<UserData>, rect: Rect, alpha: f32, clip: Option<Rect>) -> Drawn {
    let d = u.data.borrow();
    let e = d.downcast_ref::<lui::Element>().expect("element");
    let s = &e.state;
    Drawn {
        id: e.id,
        kind: e.kind,
        rect,
        alpha,
        rgb: [s.red, s.green, s.blue],
        material: s.material.clone(),
        text: s.text.clone(),
        font: s.font.clone(),
        alignment: match (s.alignment, s.left_anchor, s.right_anchor) {
            (0, true, false) => 1,
            (0, false, true) => 3,
            // Both anchors pinned: a box with width reads from its middle
            // (the rank-up text); a zero-width one is a place the text
            // starts at, as BO2's Setup Bots values (a selector's value
            // text in a list slot of no width) do.
            (0, true, true) if rect[2] - rect[0] <= 0.5 => 1,
            (0, ..) => 2,
            (a, ..) => a,
        },
        z_rot: s.z_rot,
        x_rot: s.x_rot,
        y_rot: s.y_rot,
        dashes: e.dashes,
        dash_pitch: e.dash_pitch,
        tiles: e.tiles,
        shader: e.shader,
        blur: e.blur,
        behind: e.behind,
        clip,
    }
}

fn font_name(u: &UserData) -> Option<String> {
    if u.kind != "font" {
        return None;
    }
    u.data.borrow().downcast_ref::<String>().cloned()
}

pub(crate) fn set_field(t: &Value, name: &str, v: Value) {
    if let Value::Table(t) = t {
        t.borrow_mut().set_str(name, v);
    }
}

/// A table whose missing fields are do-nothing functions, counted.
fn stub_table(vm: &mut Vm, name: &'static str, asked: Rc<RefCell<BTreeMap<String, usize>>>) -> Value {
    let t: TableRef = Table::new_ref();
    let meta = Table::new_ref();
    let index = vm.native("stub_index", move |vm, a| {
        let key = arg(&a, 1).to_string();
        *asked.borrow_mut().entry(format!("{name}.{key}")).or_insert(0) += 1;
        Ok(vec![vm.native("stub_fn", |_, _| Ok(vec![]))])
    });
    meta.borrow_mut().set_str("__index", index);
    t.borrow_mut().meta = Some(meta);
    Value::Table(t)
}

#[cfg(test)]
mod tests {
    use super::script_key;

    #[test]
    fn script_names_meet_modules() {
        assert_eq!(script_key("ui_mp/t6/hud.lua"), script_key("ui_mp_t6_hud.lua"));
        assert_eq!(script_key("ui_mp__t6__zombie__basezombie.lua"), format!("uimp{}", script_key("T6.Zombie.BaseZombie")));
    }
}

#[cfg(test)]
mod host_tests {
    use super::Host;
    use crate::value::Value;

    #[test]
    fn unanswered_engine_fields_are_counted() {
        let mut h = Host::new(Vec::new(), Box::new(|_, _, _| 0.0));
        let e = h.vm.global("Engine");
        let f = h.vm.index(&e, &Value::str("SomeUnboundField")).unwrap();
        assert!(matches!(f, Value::Native(_)));
        assert_eq!(h.asked.borrow().get("Engine.SomeUnboundField"), Some(&1));
    }
}
