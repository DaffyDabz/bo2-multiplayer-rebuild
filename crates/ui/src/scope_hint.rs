//! bo2mp: Black Ops II's scope hints, in the game's own font. Looking
//! through a sniper's scope the game prints "Press SHIFT and hold to
//! steady" (`PLATFORM_HOLD_BREATH`, the key bound to Sprint / Hold Breath)
//! and, on a Variable Zoom scope, "Press V to zoom"
//! (`PLATFORM_HOLD_BREATH_ZOOM`, the melee key) at the top of the scope,
//! centred, the key in yellow (the real game's shots: the lines at 110 and
//! 140 of 720). The steady line goes while he holds the key.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use frame::{ClientSet, HudInputView};
use net::{ClientActionInput, LocalPresentClient, PresentedSnapshot};

use crate::layers::{UiLayer, UiLayerVisibility};

/// The lines' centres in the 720-high root, from the real game.
const STEADY_Y: f32 = 110.0;
const ZOOM_Y: f32 = 140.0;
/// The font's height in the 720-high root (the real lines are about 200
/// px wide for "Press SHIFT and hold to steady").
const FONT_PX: f32 = 22.0;

#[derive(Component)]
struct ScopeHintRoot {
    shows: String,
}

/// The lines to show: (localization key, command whose key fills `&&1`,
/// centre height).
pub(crate) fn hint_lines(
    hold_breath: bool,
    variable_zoom: bool,
    holding: bool,
) -> Vec<(&'static str, &'static str, f32)> {
    let mut out = Vec::new();
    if hold_breath && !holding {
        out.push(("PLATFORM_HOLD_BREATH", "+breath_sprint", STEADY_Y));
    }
    if variable_zoom {
        out.push(("PLATFORM_HOLD_BREATH_ZOOM", "+melee", ZOOM_Y));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    roots: Query<(Entity, &ScopeHintRoot)>,
    window: Query<&Window, With<PrimaryWindow>>,
    presented: Option<Res<PresentedSnapshot>>,
    local: Option<Res<LocalPresentClient>>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    strings: Option<Res<assets::T6Ui>>,
    keys: Option<Res<HudInputView>>,
    actions: Option<Res<ClientActionInput>>,
    fonts: Option<Res<crate::bo2_font::Bo2Fonts>>,
) {
    let wanted = (|| {
        let (p, l, w, s) = (
            presented.as_deref()?,
            local.as_deref()?,
            weapons.as_deref()?,
            strings.as_deref()?,
        );
        let ps = p.alive_player(l.0)?;
        if ps.f_weapon_pos_frac < 1.0 || !w.0.overlay_is_hud_iris(ps.weapon) {
            return None;
        }
        let facts = w.0.facts_of(ps.weapon as u32)?;
        let holding = actions.as_deref().is_some_and(|a| a.client.kb.holdbreath.active);
        let keys = keys.as_deref().cloned().unwrap_or_default();
        let unbound = "UNBOUND".to_owned();
        let lines: Vec<(String, f32)> = hint_lines(
            facts.hold_breath_to_steady,
            facts.variable_zoom_fovs[0] > 0.0,
            holding,
        )
        .into_iter()
        .filter_map(|(key, command, y)| {
            let template = s.strings.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())?;
            let bind = keys.binding_keys.get(command).cloned().unwrap_or_else(|| unbound.clone());
            Some((template.replace("&&1", &bind.to_uppercase()), y))
        })
        .collect();
        (!lines.is_empty()).then_some((lines, keys))
    })();
    let (Some((lines, keys)), Ok(win), Some(fonts)) = (wanted, window.single(), fonts.as_deref()) else {
        for (e, _) in &roots {
            commands.entity(e).try_despawn();
        }
        return;
    };
    let scale = win.height() / 720.0;
    let shows = format!(
        "{}|{:.2}|{}",
        lines.iter().map(|(t, y)| format!("{t}@{y}")).collect::<Vec<_>>().join("/"),
        scale,
        win.width()
    );
    if roots.iter().any(|(_, r)| r.shows == shows) {
        return;
    }
    for (e, _) in &roots {
        commands.entity(e).try_despawn();
    }
    let px = FONT_PX * scale;
    commands
        .spawn((
            ScopeHintRoot { shows },
            UiLayer::Hud,
            UiLayerVisibility,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ))
        .with_children(|root| {
            for (text, y) in &lines {
                let runs = crate::zm_hud::hint_runs(text, &keys);
                let plain: String = runs.iter().map(|(t, _)| t.as_str()).collect();
                let width = fonts.line_width("Default", &plain, px);
                root.spawn(Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(win.width() * 0.5 - width * 0.5),
                    top: Val::Px(y * scale - px * 0.6),
                    ..default()
                })
                .with_children(|c| {
                    fonts.spawn_line(c, "Default", &runs, px, 1.0 * scale);
                });
            }
        });
}

pub(crate) fn register(app: &mut App) {
    app.add_systems(Update, update.after(crate::bo2_font::load_bo2_fonts).in_set(ClientSet::Ui));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_goes_while_held_and_zoom_only_on_variable_zoom() {
        assert_eq!(hint_lines(true, false, false).len(), 1);
        assert!(hint_lines(true, false, true).is_empty());
        let held = hint_lines(true, true, true);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].0, "PLATFORM_HOLD_BREATH_ZOOM");
        assert!(hint_lines(false, false, false).is_empty());
    }
}
