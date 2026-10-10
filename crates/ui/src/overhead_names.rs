//! bo2mp: other players' names over their heads in Black Ops II's own font.
//! The HUD (`hud`'s overhead names) picks who shows, where and how faded,
//! and fills [`OverheadNameRows`] when the match has no MW2 menu font; this
//! draws those rows with the map's BO2 HUD font.

use bevy::prelude::*;
use frame::ClientSet;

use crate::layers::{UiLayer, UiLayerVisibility};

/// The BO2 font the names are drawn in.
const FONT: &str = "Default";

/// One name to draw this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct OverheadNameRow {
    /// The player's entity number (keeps his row between frames).
    pub ent: u16,
    pub text: String,
    /// The name's centre x and the line's middle y, in window pixels.
    pub x: f32,
    pub y: f32,
    /// The font's height in window pixels.
    pub px: f32,
    pub color: [f32; 4],
}

/// The names to draw this frame (empty when none, or in an MW2 match).
#[derive(Resource, Debug, Default, Clone)]
pub struct OverheadNameRows(pub Vec<OverheadNameRow>);

#[derive(Component)]
struct OverheadNamesRoot;

#[derive(Component)]
struct OverheadNameNode {
    ent: u16,
    /// What the row was built from: text, size and colour (positions only
    /// move the node).
    built: String,
    size: Vec2,
}

fn built_key(row: &OverheadNameRow) -> String {
    let q = |v: f32| (v * 32.0).round() as i32;
    format!(
        "{}|{}|{},{},{},{}",
        row.text,
        (row.px * 2.0).round() as i32,
        q(row.color[0]),
        q(row.color[1]),
        q(row.color[2]),
        q(row.color[3])
    )
}

fn update(
    mut commands: Commands,
    rows: Res<OverheadNameRows>,
    fonts: Option<Res<crate::bo2_font::Bo2Fonts>>,
    roots: Query<Entity, With<OverheadNamesRoot>>,
    mut nodes: Query<(Entity, &OverheadNameNode, &mut Node)>,
) {
    let Some(fonts) = fonts.as_deref().filter(|_| !rows.0.is_empty()) else {
        for e in &roots {
            commands.entity(e).try_despawn();
        }
        return;
    };
    let root = match roots.iter().next() {
        Some(root) => root,
        None => commands
            .spawn((
                OverheadNamesRoot,
                UiLayer::Hud,
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
    let mut kept = Vec::new();
    for (e, node, mut layout) in &mut nodes {
        match rows.0.iter().find(|r| r.ent == node.ent) {
            Some(row) if built_key(row) == node.built => {
                layout.left = Val::Px((row.x - node.size.x * 0.5).round());
                layout.top = Val::Px((row.y - node.size.y * 0.5).round());
                kept.push(node.ent);
            }
            _ => commands.entity(e).try_despawn(),
        }
    }
    for row in rows.0.iter().filter(|r| !kept.contains(&r.ent)) {
        let [r, g, b, a] = row.color;
        let runs = [(row.text.clone(), Color::srgba(r, g, b, a))];
        let width = fonts.line_width(FONT, &row.text, row.px);
        let height = row.px * 1.2;
        commands.entity(root).with_children(|parent| {
            parent
                .spawn((
                    OverheadNameNode {
                        ent: row.ent,
                        built: built_key(row),
                        size: Vec2::new(width, height),
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px((row.x - width * 0.5).round()),
                        top: Val::Px((row.y - height * 0.5).round()),
                        ..default()
                    },
                ))
                .with_children(|c| {
                    fonts.spawn_line(c, FONT, &runs, row.px, (row.px / 16.0).max(1.0));
                });
        });
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<OverheadNameRows>().add_systems(
        Update,
        update.after(crate::bo2_font::load_bo2_fonts).in_set(ClientSet::Ui),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moving_name_keeps_its_build_and_a_fading_one_rebuilds() {
        let row = OverheadNameRow {
            ent: 3,
            text: "Bot3".into(),
            x: 10.0,
            y: 20.0,
            px: 16.0,
            color: [0.4, 0.8, 1.0, 1.0],
        };
        let moved = OverheadNameRow { x: 300.0, y: 200.0, ..row.clone() };
        assert_eq!(built_key(&row), built_key(&moved));
        let faded = OverheadNameRow { color: [0.4, 0.8, 1.0, 0.5], ..row.clone() };
        assert_ne!(built_key(&row), built_key(&faded));
    }
}
