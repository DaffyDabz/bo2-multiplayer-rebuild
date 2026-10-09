//! bo2mp: a sight attachment's reticle (the reflex's dot, the EOTech's
//! ring, the ACOG's chevron, the Target Finder's box).
//!
//! Black Ops II draws it on the sight's glass with `sw4_3d_reticle_dynamic`
//! (fxc disassembly): the picture's texture coordinate is
//! `0.5 + Color_Map_Scale * (tangent . d, -binormal . d)`, `d` the eye ray,
//! so the picture hangs at infinity along the sight and spans
//! `1 / Color_Map_Scale` in tangent of the view angle. Aiming down the
//! sights puts the sight on the line of sight, so the HUD draws it there at
//! that size (the asset lane bakes its colours and glow into
//! `reticle:<weapon>` and keeps the glass surface from drawing it flat).
//! The material adds its colour onto the frame (blend ONE / ONE, or ONE /
//! 1 - alpha with the shader's zero alpha), so the HUD draws it with the
//! match HUD's additive material, on the match world's camera.

use assets::PreparedWeapons;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::ui::{Display, FocusPolicy, UiTargetCamera, ZIndex};
use net::{LocalPresentClient, PresentedSnapshot, ViewweaponAim};
use weapon_iw4::get_viewmodel_weapon_index;

use crate::presentation_scale::{PresentationScale, ScaleClass};
use crate::ui_write::adopt_display;

#[derive(Component, Clone, Copy)]
pub(crate) struct SightReticle;

/// How far up the sights must be before the reticle shows (it fades in to
/// full at full ADS, as the glass comes in front of the eye).
const SHOW_FROM_FRAC: f32 = 0.75;

/// The reticle's opacity at this ADS fraction.
pub(crate) fn sight_alpha(frac: f32) -> f32 {
    ((frac - SHOW_FROM_FRAC) / (1.0 - SHOW_FROM_FRAC)).clamp(0.0, 1.0)
}

/// The reticle picture's width in pixels for a screen `height` pixels tall
/// seen through a vertical field of view whose half-angle tangent is
/// `tan_half_y`.
pub(crate) fn sight_size_px(scale: f32, height: f32, tan_half_y: f32) -> f32 {
    (1.0 / scale) * (height * 0.5) / tan_half_y.max(1e-4)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_sight(
    mut commands: Commands,
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    icons: Option<Res<assets::T6HudIcons>>,
    cameras: Query<&Projection, With<Camera3d>>,
    world_cam: Query<Entity, (With<Camera3d>, With<bevy::camera::CompositingSpace>, Without<frame::UiCamera>)>,
    aim: Res<ViewweaponAim>,
    mut images: ResMut<Assets<Image>>,
    mut additive: ResMut<Assets<ui::AdditiveUi>>,
    mut cache: Local<Vec<(String, Handle<Image>)>>,
    mut node: Query<(Entity, &mut Node, &MaterialNode<ui::AdditiveUi>, Option<&UiTargetCamera>), With<SightReticle>>,
) {
    // The reticle is a node of its own, made once its material can be.
    let Ok((entity, mut node, material, target)) = node.single_mut() else {
        commands.spawn((
            SightReticle,
            Node { position_type: PositionType::Absolute, display: Display::None, ..default() },
            MaterialNode(additive.add(ui::AdditiveUi {
                tint: LinearRgba::NONE,
                picture: Handle::default(),
                uv: Vec4::new(0.0, 0.0, 1.0, 1.0),
                encoded: 0.0,
                color_add: 0.0,
            })),
            FocusPolicy::Pass,
            ZIndex(0),
        ));
        return;
    };
    let hide = |node: &mut Node| adopt_display(node, Display::None);
    let (Some(ps), Some(weapons)) = (presented.player(local.0), weapons.as_deref()) else {
        hide(&mut node);
        return;
    };
    let alpha = sight_alpha(ps.f_weapon_pos_frac);
    let weapon = get_viewmodel_weapon_index(ps);
    let scale = weapons
        .0
        .facts_of(weapon)
        .map_or(0.0, |f| f.sight_reticle_scale);
    if !surface.is_ready()
        || alpha <= 0.0
        || scale <= 0.0
        || ps.pm_type >= playerstate_iw4::PM_TYPE_DEAD
    {
        hide(&mut node);
        return;
    }
    let key = format!("reticle:{}", weapons.0.name_of(weapon));
    let handle = match cache.iter().find(|(k, _)| *k == key) {
        Some((_, h)) => h.clone(),
        None => {
            let Some((_, picture)) = icons.as_deref().and_then(|i| i.0.iter().find(|(k, _)| *k == key))
            else {
                hide(&mut node);
                return;
            };
            let mut picture = (**picture).clone();
            picture.asset_usage = RenderAssetUsages::default();
            let h = images.add(picture);
            cache.push((key, h.clone()));
            h
        }
    };
    let Some(tan_half_y) = cameras.iter().find_map(|p| match p {
        Projection::Perspective(persp) => Some((persp.fov * 0.5).tan()),
        _ => None,
    }) else {
        hide(&mut node);
        return;
    };
    // BO2 adds the reticle onto the frame, in the frame's own (encoded)
    // space: on the match world's camera when there is one (its target holds
    // encoded values, as the match HUD's additive pictures are drawn),
    // otherwise on the overlay's straight-alpha target.
    let lens = world_cam.iter().next();
    if lens != target.map(|t| t.0) {
        match lens {
            Some(cam) => commands.entity(entity).insert(UiTargetCamera(cam)),
            None => commands.entity(entity).remove::<UiTargetCamera>(),
        };
    }
    let tint = LinearRgba::new(1.0, 1.0, 1.0, alpha);
    let encoded = if lens.is_some() { 1.0 } else { 0.0 };
    let stale = additive.get(&material.0).is_none_or(|m| m.picture != handle || m.tint != tint || m.encoded != encoded);
    if stale && let Some(mut m) = additive.get_mut(&material.0) {
        m.picture = handle;
        m.tint = tint;
        m.encoded = encoded;
    }
    let view = PresentationScale::from_window(surface.width(), surface.height());
    let factor = view.factor(ScaleClass::ProjectionBound);
    let (dx, dy) = if aim.live {
        (aim.xhair_x * factor, aim.xhair_y * factor)
    } else {
        (0.0, 0.0)
    };
    let size = sight_size_px(scale, view.height(), tan_half_y);
    let cx = view.width() * 0.5 + dx;
    let cy = view.height() * 0.5 + dy;
    adopt_display(&mut node, Display::Flex);
    node.left = Val::Px((cx - size * 0.5).round());
    node.top = Val::Px((cy - size * 0.5).round());
    node.width = Val::Px(size.round().max(1.0));
    node.height = Val::Px(size.round().max(1.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflex_dot_size_on_a_1080p_screen() {
        // The reflex (scale 30) at its 50 degree zoom: tan(25 deg) * 0.75
        // vertical, 1080 tall.
        let tan_half_y = (25f32).to_radians().tan() * 0.75;
        let px = sight_size_px(30.0, 1080.0, tan_half_y);
        assert!((px - 51.5).abs() < 1.0, "{px}");
        assert_eq!(sight_alpha(0.5), 0.0);
        assert_eq!(sight_alpha(1.0), 1.0);
    }
}
