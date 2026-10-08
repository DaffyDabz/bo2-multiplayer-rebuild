use bevy::prelude::*;

#[derive(Resource, Default, Debug)]
pub struct UiPartyState {
    pub active: bool,
    pub in_lobby: bool,
    pub is_host: bool,
}

#[derive(Resource, Default, Debug)]
pub struct UiMenuDvars {
    values: std::collections::HashMap<String, String>,
}

impl UiMenuDvars {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.values.insert(name.to_ascii_lowercase(), value.into());
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

#[derive(Resource, Clone, Default, Debug)]
pub struct HostMatchRules(pub Vec<(String, String)>);

#[derive(Message, Clone, Debug)]
pub struct UiExecCommand {
    pub text: String,
}

#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub enum UiMenuRequest {
    Toggle,
    Open(String),
    Close(String),
    Key(UiMenuKey),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMenuKey {
    Escape,
    Enter,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
}

/// bo2zm M4: Black Ops II's own UI (its scripts, `ui::lui_hud`): whether it
/// runs (`active`: Esc is its pause menu, not the classic menu) and whether
/// one of its menus holds the keyboard and mouse (`open`).
#[derive(Resource, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct LuiMenus {
    pub active: bool,
    pub open: bool,
}

/// bo2zm M4: how much Black Ops II's menus blur the world behind them
/// (`Engine.BlurWorld`: 2 while a menu is up in game, 0 when it closes).
#[derive(Resource, Clone, Copy, Default, Debug, PartialEq)]
pub struct WorldBlur(pub f32);

/// bo2mp: how many full-screen black dims an in-match menu has up over the
/// world (the pause menu's, then a popup's on top). BO2 blends them in the
/// target's gamma space; ours blend in linear light, which crushes the dark
/// world under a second dim, so the world pass lifts it to compensate.
#[derive(Resource, Clone, Copy, Default, Debug, PartialEq)]
pub struct WorldDim(pub f32);

/// bo2mp: how many full-screen black dims the in-match menus have up (the
/// pause menu's, then a popup's), whichever way they are drawn (as UI on the
/// world camera, or as the world pass's own darkening, `WorldDim`). Two or
/// more mean a popup is over the pause menu: BO2's HUD is dimmed out by both,
/// so what the HUD draws above the menus goes with them.
#[derive(Resource, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct MenuDims(pub u32);

/// bo2zm M4: a solo game paused by its pause menu: the server does not
/// tick (as Black Ops II pauses a solo Zombies game).
#[derive(Resource, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct GamePaused(pub bool);
