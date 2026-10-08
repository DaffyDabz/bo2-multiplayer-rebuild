//! bo2zm: Black Ops II effects into the effect catalog.
//!
//! A T6 `FxElemDef` keeps the IW4 field order with T6 additions (fade
//! ranges, a rotation axis, wind); `asset_t6` hands it over as bytes plus
//! the names it points at. Velocity and visual samples have the IW4 layout
//! and are copied as they are. Element types follow Black Ops numbering
//! (billboard, oriented, rotated, tail, line, trail, cloud, model, omni,
//! spot, sound, decal, runner) and are mapped to IW4's the way the Black
//! Ops adapter maps them. Flag bits are kept as stored (the engine family
//! shares them).

use fastfile_t6::layout::{FxElemDef as e, FxElemVelStateSample, FxElemVisStateSample};

use super::*;

/// T6 element type -> IW4.
pub fn t6_elem_type(raw: u8) -> u8 {
    match raw {
        0 => elem_type::BILLBOARD,
        1 | 2 => elem_type::ORIENTED,
        3 | 4 => elem_type::TAIL,
        5 => elem_type::TRAIL,
        6 => elem_type::CLOUD,
        other => other,
    }
}

fn t6_elem_view(el: &asset_t6::FxElemRef) -> (FxElemDefView, [u8; 8]) {
    let f = |off: usize| el.f32_at(off);
    let i = |off: usize| el.i32_at(off);
    let range = |off: usize| [f(off), f(off + 4)];
    let b = |off: usize| el.raw.get(off).copied().unwrap_or(0);
    let mut atlas = [0u8; 8];
    if let Some(src) = el.raw.get(e::atlas..e::atlas + 8) {
        atlas.copy_from_slice(src);
    }
    // T6 packs the entry count with an index range (`entryCountAndIndexRange`:
    // count in the low byte, measured 516 = 4 entries on a 2x2 atlas); IW4
    // keeps a plain count there.
    let count = u16::from(atlas[6]).max(1);
    atlas[6..8].copy_from_slice(&count.to_le_bytes());
    let mut view = FxElemDefView {
        flags: i(e::flags),
        spawn_a: i(e::spawn),
        spawn_b: i(e::spawn + 4),
        spawn_range_base: f(e::spawnRange),
        spawn_range_amplitude: f(e::spawnRange + 4),
        fade_in_range: range(e::fadeInRange),
        fade_out_range: range(e::fadeOutRange),
        spawn_frustum_cull_radius: f(e::spawnFrustumCullRadius),
        spawn_delay_msec_base: i(e::spawnDelayMsec),
        spawn_delay_msec_amplitude: i(e::spawnDelayMsec + 4),
        life_span_msec_base: i(e::lifeSpanMsec),
        life_span_msec_amplitude: i(e::lifeSpanMsec + 4),
        spawn_origin: core::array::from_fn(|a| range(e::spawnOrigin + a * 8)),
        spawn_offset_radius_base: f(e::spawnOffsetRadius),
        spawn_offset_radius_amplitude: f(e::spawnOffsetRadius + 4),
        spawn_offset_height_base: f(e::spawnOffsetHeight),
        spawn_offset_height_amplitude: f(e::spawnOffsetHeight + 4),
        spawn_angles: core::array::from_fn(|a| range(e::spawnAngles + a * 8)),
        angular_velocity: core::array::from_fn(|a| range(e::angularVelocity + a * 8)),
        initial_rotation: range(e::initialRotation),
        gravity_base: f(e::gravity),
        gravity_amplitude: f(e::gravity + 4),
        reflection_factor: range(e::reflectionFactor),
        coll_mins: core::array::from_fn(|a| f(e::collMins + a * 4)),
        coll_maxs: core::array::from_fn(|a| f(e::collMaxs + a * 4)),
        elem_type: t6_elem_type(b(e::elemType)),
        visual_count: b(e::visualCount),
        vel_interval_count: b(e::velIntervalCount),
        vis_state_interval_count: b(e::visStateIntervalCount),
        lighting_frac: b(e::lightingFrac),
        use_item_clip: 0,
        sort_order: b(e::sortOrder),
        emit_dist: range(e::emitDist),
        emit_dist_variance: range(e::emitDistVariance),
    };
    // T6's line (type 4) draws as a tail, from the part forward along its
    // velocity (a tail trails behind it).
    if b(e::elemType) == 4 {
        view.flags |= T6_FX_ELEM_LINE;
    }
    (view, atlas)
}

fn t6_material_visual(name: &str) -> OwnedFxVisual {
    if name.is_empty() {
        return OwnedFxVisual::None;
    }
    OwnedFxVisual::Material {
        material: FxElemMaterial::Unresolved(AssetEdgeReason::CatalogMiss),
        hint: Some(name.to_owned()),
        material_namespace: crate::AssetNamespace::T6,
        authored: AuthoredRef {
            slot: true,
            alias: false,
        },
    }
}

fn t6_visuals(el: &asset_t6::FxElemRef, view: &FxElemDefView) -> Vec<OwnedFxVisual> {
    let t = view.elem_type;
    if matches!(t, elem_type::OMNI_LIGHT | elem_type::SPOT_LIGHT) || el.visuals.is_empty() {
        return vec![OwnedFxVisual::None];
    }
    el.visuals
        .iter()
        .map(|v| match v {
            asset_t6::FxVisualRef::Material(name) => t6_material_visual(name),
            asset_t6::FxVisualRef::Mark([a, b]) => {
                mark_pair(t6_material_visual(a), t6_material_visual(b))
            }
            asset_t6::FxVisualRef::Model(name) => model_visual(Some(name.clone())),
            asset_t6::FxVisualRef::Effect(name) => runner_visual(name.clone()),
            asset_t6::FxVisualRef::Sound(name) => sound_visual(name.clone()),
            asset_t6::FxVisualRef::Light(_) | asset_t6::FxVisualRef::None => OwnedFxVisual::None,
        })
        .collect()
}

fn t6_trail(el: &asset_t6::FxElemRef) -> Option<OwnedFxTrailDef> {
    let trail = el.trail.as_ref()?;
    Some(OwnedFxTrailDef {
        scroll_time_msec: trail.scroll_time_msec,
        repeat_dist: trail.repeat_dist,
        // A split distance of (almost) nothing splits by distance not at
        // all: the Warthog's contrail stores a denormal here, whose inverse
        // is infinite and asked the trail for billions of pieces a frame.
        inv_split_dist: Some(1.0 / trail.split_dist)
            .filter(|inv| trail.split_dist > 0.0 && inv.is_finite())
            .unwrap_or(0.0),
        inv_split_arc_dist: 0.0,
        inv_split_time: 0.0,
        verts: trail
            .verts
            .iter()
            .map(|v| FxTrailVertex {
                pos: [v[0], v[1]],
                normal: [v[2], v[3]],
                tex_coord: v[4],
            })
            .collect(),
        inds: trail.inds.clone(),
    })
}

fn t6_elem(el: &asset_t6::FxElemRef) -> OwnedFxElemDef {
    let (view, atlas) = t6_elem_view(el);
    let vel_count = usize::from(view.vel_interval_count) + 1;
    let vel_samples = el
        .vel_samples
        .get(..vel_count * FxElemVelStateSample::SIZE)
        .unwrap_or(&el.vel_samples)
        .to_vec();
    let vis_count = usize::from(view.vis_state_interval_count) + 1;
    let mut vis_samples = el
        .vis_samples
        .get(..vis_count * FxElemVisStateSample::SIZE)
        .unwrap_or(&el.vis_samples)
        .to_vec();
    // T6 keeps a sample's colour bytes red first; IW4's are blue first
    // (seen: flames drew blue-white and smoke dark red until swapped).
    for sample in vis_samples.chunks_exact_mut(FxElemVisStateSample::SIZE) {
        for half in [0, FxElemVisStateSample::SIZE / 2] {
            sample.swap(half, half + 2);
        }
    }
    let vel_graph_local = parse_vel_graph_channel(&vel_samples, vel_count, false);
    let vel_graph_world = parse_vel_graph_channel(&vel_samples, vel_count, true);
    let visuals = t6_visuals(el, &view);
    let (effect_on_impact, effect_on_impact_hint) = capture_named_child(el.effect_on_impact.clone());
    let (effect_on_death, effect_on_death_hint) = capture_named_child(el.effect_on_death.clone());
    let (effect_emitted, effect_emitted_hint) = capture_named_child(el.effect_emitted.clone());
    let trail_def = if view.elem_type == elem_type::TRAIL {
        t6_trail(el)
    } else {
        None
    };
    OwnedFxElemDef {
        raw: leftover_pack_iw4_elem_raw(&view, atlas),
        view,
        vel_samples,
        vel_graph_local,
        vel_graph_world,
        vis_samples,
        visuals,
        effect_on_impact,
        effect_on_impact_hint,
        effect_on_death,
        effect_on_death_hint,
        effect_emitted,
        effect_emitted_hint,
        has_extended: el.trail.is_some() || el.spot.is_some(),
        trail_def,
        spark_fountain_def: None,
        // T6 replaces IW4's cloud shape bits with a count (measured 512
        // base, 0..512 extra on Nuketown's ash and ember clouds).
        cloud_density: (view.elem_type == elem_type::CLOUD)
            .then(|| [el.i32_at(e::u), el.i32_at(e::u + 4)]),
    }
}

/// T6 element flag: drawn with the first-person gun (the parts of the
/// game's own first-person gas flashes, muzzle breaks and shell casings
/// carry it; the Ray Gun's first-person flash does without).
pub const T6_FX_ELEM_DRAW_WITH_VIEWMODEL: i32 = 0x1000;

/// bo2zm M3 fix list 1: a T6 line element (type 4), drawn as a tail that
/// runs from the part forward along its velocity: the zombies' eye beams
/// (they start behind the eye and move out of the face), the fires' flame
/// sheets (they start low and rise). A bit no T6 element sets.
pub const T6_FX_ELEM_LINE: i32 = 0x2000_0000;

/// bo2zm fix list 3: how much smaller a far-view muzzle flash is drawn on
/// the player's own gun. OUR CHOICE, not the files': the Python, M14 and
/// Olympia (and the other guns listed in the load report) name the flash
/// other players see as their first-person flash too; its fireballs grow
/// to 23-48 units around a barrel 25 units from the eye and fill the
/// screen. At a fifth the Python's peaks on screen about as big as the
/// Pack-a-Punched Python's own first-person flash (measured side by side).
pub const T6_FIRST_PERSON_FLASH_SCALE: f32 = 0.2;

/// The name of a far-view flash's first-person copy.
pub fn t6_first_person_flash_name(name: &str) -> String {
    format!("{name}#1p")
}

/// The first-person copy of one element: sizes, offsets, speeds and reach
/// times `s`, drawn with the gun. Lights keep their reach, and parts the
/// effect already keeps off the eye (a distance fade: its muzzle smoke) are
/// left as they are.
fn t6_first_person_elem(el: &OwnedFxElemDef, s: f32) -> OwnedFxElemDef {
    let mut out = el.clone();
    let t = el.view.elem_type;
    if matches!(
        t,
        elem_type::OMNI_LIGHT
            | elem_type::SPOT_LIGHT
            | elem_type::SOUND
            | elem_type::DECAL
            | elem_type::RUNNER
    ) {
        return out;
    }
    let v = &el.view;
    if v.fade_in_range[1] != 0.0 || v.fade_out_range[1] != 0.0 {
        return out;
    }
    let view = &mut out.view;
    view.flags |= T6_FX_ELEM_DRAW_WITH_VIEWMODEL;
    for axis in &mut view.spawn_origin {
        axis[0] *= s;
        axis[1] *= s;
    }
    view.spawn_offset_radius_base *= s;
    view.spawn_offset_radius_amplitude *= s;
    view.spawn_offset_height_base *= s;
    view.spawn_offset_height_amplitude *= s;
    view.gravity_base *= s;
    view.gravity_amplitude *= s;
    view.spawn_frustum_cull_radius *= s;
    for c in view.coll_mins.iter_mut().chain(view.coll_maxs.iter_mut()) {
        *c *= s;
    }
    for d in view
        .emit_dist
        .iter_mut()
        .chain(view.emit_dist_variance.iter_mut())
    {
        *d *= s;
    }
    // Each visual sample: base then amplitude, each colour (4 bytes),
    // rotation delta and total, size[2], scale.
    let half = FxElemVisStateSample::SIZE / 2;
    for sample in out.vis_samples.chunks_exact_mut(FxElemVisStateSample::SIZE) {
        for at in [12, 16, 20].into_iter().flat_map(|o| [o, half + o]) {
            let f =
                f32::from_le_bytes([sample[at], sample[at + 1], sample[at + 2], sample[at + 3]])
                    * s;
            sample[at..at + 4].copy_from_slice(&f.to_le_bytes());
        }
    }
    // Velocity samples are all distances (local and world velocity, total
    // delta).
    for f in out.vel_samples.chunks_exact_mut(4) {
        let x = f32::from_le_bytes([f[0], f[1], f[2], f[3]]) * s;
        f.copy_from_slice(&x.to_le_bytes());
    }
    let vel_count = usize::from(out.view.vel_interval_count) + 1;
    out.vel_graph_local = parse_vel_graph_channel(&out.vel_samples, vel_count, false);
    out.vel_graph_world = parse_vel_graph_channel(&out.vel_samples, vel_count, true);
    let mut atlas = [0u8; 8];
    if let Some(src) = el.raw.get(0xa8..0xb0) {
        atlas.copy_from_slice(src);
    }
    out.raw = leftover_pack_iw4_elem_raw(&out.view, atlas);
    out
}

impl FxCatalog {
    /// bo2zm fix list 3: for a flash a gun names for both views (the one
    /// other players see), add its first-person copy
    /// (`t6_first_person_flash_name`). Returns its largest flame size
    /// before and after; `None` when the effect is not here, already has
    /// parts drawn with the gun, or plays other effects.
    pub fn add_t6_first_person_flash(&mut self, name: &str) -> Option<(f32, f32)> {
        let def = self.get_in(crate::AssetNamespace::T6, name)?;
        let drawn = |e: &OwnedFxElemDef| e.view.elem_type <= elem_type::MODEL;
        if def.elems.iter().any(|e| {
            e.view.elem_type == elem_type::RUNNER
                || (drawn(e) && e.view.flags & T6_FX_ELEM_DRAW_WITH_VIEWMODEL != 0)
        }) {
            return None;
        }
        let s = T6_FIRST_PERSON_FLASH_SCALE;
        let mut copy = def.clone();
        copy.name = t6_first_person_flash_name(name);
        copy.elems = def
            .elems
            .iter()
            .map(|e| t6_first_person_elem(e, s))
            .collect();
        // The largest flame (base size of any visual sample) of the parts
        // the copy draws with the gun, before and after.
        let scaled: Vec<usize> = (0..copy.elems.len())
            .filter(|&i| {
                drawn(&copy.elems[i])
                    && copy.elems[i].view.flags & T6_FX_ELEM_DRAW_WITH_VIEWMODEL != 0
            })
            .collect();
        let largest = |elems: &[OwnedFxElemDef]| {
            scaled
                .iter()
                .flat_map(|&i| {
                    elems[i]
                        .vis_samples
                        .chunks_exact(FxElemVisStateSample::SIZE)
                })
                .map(|x| f32::from_le_bytes([x[12], x[13], x[14], x[15]]).abs())
                .fold(0.0f32, f32::max)
        };
        let before = largest(&def.elems);
        let after = largest(&copy.elems);
        self.insert_owned(copy);
        Some((before, after))
    }

    /// bo2zm: one Black Ops II effect. False for a reference stub (a
    /// comma-named asset another zone defines) or an unnamed one.
    pub fn capture_t6(&mut self, fx: &asset_t6::FxEffectRef) -> bool {
        if fx.name.is_empty() || fx.name.starts_with(',') {
            return false;
        }
        let view = FxEffectDefView {
            flags: i32::from(fx.flags),
            msec_looping_life: fx.msec_looping_life,
            looping_count: i32::from(fx.looping_count),
            one_shot_count: i32::from(fx.one_shot_count),
            emission_count: i32::from(fx.emission_count),
        };
        let elems: Vec<OwnedFxElemDef> = fx.elems.iter().map(t6_elem).collect();
        self.last_captured = Some(fx.name.clone());
        self.insert_owned(OwnedFxEffectDef {
            namespace: crate::AssetNamespace::T6,
            name: fx.name.clone(),
            view,
            header_raw: leftover_pack_iw4_effect_header(&view),
            elems,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t6_types_map_like_black_ops() {
        assert_eq!(t6_elem_type(0), elem_type::BILLBOARD);
        assert_eq!(t6_elem_type(2), elem_type::ORIENTED);
        assert_eq!(t6_elem_type(4), elem_type::TAIL);
        assert_eq!(t6_elem_type(5), elem_type::TRAIL);
        assert_eq!(t6_elem_type(6), elem_type::CLOUD);
        assert_eq!(t6_elem_type(11), elem_type::DECAL);
        assert_eq!(t6_elem_type(12), elem_type::RUNNER);
    }
}
