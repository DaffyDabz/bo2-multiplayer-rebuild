use bevy::prelude::*;

#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitLevelCalled;

/// bo2mp: since when the end-of-match scoreboard has been on screen (the UI
/// writes it); the match goes back to the front end only after the board has
/// been up for BO2's intermission hold (`wait 5.0` before `exitlevel` in
/// `_globallogic::endgame`).
#[derive(Resource, Default, Debug)]
pub struct EndBoardShown(pub Option<std::time::Instant>);

pub fn register_script_notify(app: &mut App) {
    app.add_message::<ExitLevelCalled>().init_resource::<EndBoardShown>();
}
