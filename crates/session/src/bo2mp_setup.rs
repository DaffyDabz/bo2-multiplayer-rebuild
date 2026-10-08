//! bo2mp lane C: the match Black Ops II's front end set up. Its private
//! match lobby (CUSTOM GAMES) writes it when START MATCH is pressed, then
//! asks for the swap to `t6:<map>`; the match reads it as it starts.

use bevy::prelude::*;

/// The lobby's choices, as BO2's own menus hold them: the map's load name
/// (`ui_mapname`, e.g. `mp_nuketown_2020`), the game type (`ui_gametype`,
/// e.g. `tdm`) and SETUP BOTS (game type settings `bot_friends`,
/// `bot_enemies`: how many on his team / the other; `bot_difficulty`:
/// 0 recruit, 1 regular, 2 hardened, 3 veteran, as the menu lists them).
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct Bo2mpMatchSetup {
    pub map: String,
    pub gametype: String,
    pub bot_friends: u32,
    pub bot_enemies: u32,
    pub bot_difficulty: u32,
}

impl Bo2mpMatchSetup {
    /// The zone the match loads (`t6:mp_nuketown_2020`).
    pub fn zone(&self) -> String {
        format!("t6:{}", self.map)
    }
}
