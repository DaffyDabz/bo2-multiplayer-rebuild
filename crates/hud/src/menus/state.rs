use bevy::prelude::*;
use frame::AppScreen;

#[derive(Clone, Debug, Default)]
pub(crate) struct ItemState {
    pub(crate) hidden: bool,
    pub(crate) fore: Option<[f32; 4]>,
    pub(crate) back: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenMenu {
    pub(crate) name: String,
    pub(crate) focus: Option<usize>,
    pub(crate) hover: Option<usize>,
    pub(crate) items: Vec<ItemState>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditState {
    pub(crate) menu: String,
    pub(crate) item: usize,
    pub(crate) buffer: Vec<char>,
    pub(crate) cursor: usize,
    pub(crate) max_chars: usize,
}

#[derive(Resource, Default)]
pub struct ScriptMenus {
    pub(crate) stack: Vec<OpenMenu>,
    pub(crate) applied_serial: u32,
    pub(crate) responses: Vec<(String, String)>,
    pub(crate) exec: Vec<String>,
    pub(crate) sounds: Vec<String>,
    pub(crate) reported: std::collections::HashSet<String>,
    pub(crate) screen: Option<AppScreen>,
    pub(crate) cursor: Option<Vec2>,
    pub(crate) editing: Option<EditState>,
    pub(crate) slider_drag: Option<(String, usize)>,
    pub(crate) binding_menu: Option<String>,
    pub(crate) bind_requests: Vec<String>,
    /// bo2zm M4: one of Black Ops II's own menus holds the input
    /// (`frame::LuiMenus`).
    pub(crate) lui_open: bool,
}

impl ScriptMenus {
    pub fn captures_input(&self) -> bool {
        !self.stack.is_empty() || self.lui_open
    }

    pub fn open_names(&self) -> Vec<String> {
        self.stack.iter().map(|m| m.name.clone()).collect()
    }

    pub fn focused_item(&self) -> Option<(&str, usize)> {
        let menu = self.stack.last()?;
        Some((&menu.name, menu.focus?))
    }

    pub(crate) fn position(&self, name: &str) -> Option<usize> {
        self.stack
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
    }

    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut OpenMenu> {
        let pos = self.position(name)?;
        self.stack.get_mut(pos)
    }
}
