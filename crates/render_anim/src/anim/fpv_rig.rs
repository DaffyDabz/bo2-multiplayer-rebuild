//! The prepared first-person rig.
//!

use std::collections::HashMap;
use std::sync::Arc;

use bevy::math::{Mat4, Vec3};
use render_scene::SmodelPassMaterial;

use crate::anim::fpv_pose::{
    FpvBoltFrame, FpvBoltTags, PosedClip, tag_camera_lens_local, tag_view_to_bevy_camera,
};
use crate::anim::xmodel_pose::{FpvSurfOwner, SkinLayout, build_skin_layout, skin_packed_into};
use crate::draw::FpvSurfaceDraw;
use anim_iw4::PartBits;
use anim_iw4::surface_hidden;
use asset_game::{FpvAssembly, FpvClipTracks, FpvPartRole};
use asset_model::FpvMeshCatalog;
use xmodel_runtime::AnimInstance;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FpvSurfaceVerdict {
    Admitted(u32),
    Inapplicable(&'static str),
    Refused {
        material: String,
        cause: &'static str,
    },
}

#[derive(Default)]
pub struct FpvMaterialAdmission {
    pub materials: Vec<SmodelPassMaterial>,
    pub by_authored: HashMap<usize, u32>,
    verdicts: HashMap<usize, Vec<(usize, FpvSurfaceVerdict)>>,
}

impl FpvMaterialAdmission {
    pub fn record(&mut self, catalog_entry: usize, verdicts: Vec<(usize, FpvSurfaceVerdict)>) {
        self.verdicts.insert(catalog_entry, verdicts);
    }

    pub fn verdict(&self, catalog_entry: usize, surface: usize) -> Option<&FpvSurfaceVerdict> {
        self.verdicts
            .get(&catalog_entry)?
            .iter()
            .find(|(index, _)| *index == surface)
            .map(|(_, verdict)| verdict)
    }

    pub fn verdicts_of(&self, catalog_entry: usize) -> &[(usize, FpvSurfaceVerdict)] {
        self.verdicts
            .get(&catalog_entry)
            .map_or(&[], |verdicts| verdicts.as_slice())
    }
}

#[derive(Clone, Copy, Debug)]
struct PreparedFpvSurface {
    index_start: u32,
    index_count: u32,
    material: u32,
    authored: usize,
    lens_named: bool,
}

pub struct PreparedFpvModel {
    catalog_entry: usize,
    posed_surface_n: usize,
    packed_ok: bool,
    layout: SkinLayout,
    surfaces: Vec<PreparedFpvSurface>,
    refusal: Option<FpvMandatoryRefusal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FpvMandatoryRefusal {
    pub model: String,
    pub surface: usize,
    pub material: String,
    pub cause: &'static str,
}

impl core::fmt::Display for FpvMandatoryRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} surface {} `{}`: {}",
            self.model, self.surface, self.material, self.cause
        )
    }
}

#[derive(Debug)]
pub enum FpvRigError {
    Catalog(&'static str),
    Layout(&'static str),
}

impl core::fmt::Display for FpvRigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Catalog(what) => write!(f, "{what} missing from the first-person catalog"),
            Self::Layout(what) => write!(f, "{what}"),
        }
    }
}

pub fn leftover_scope_surf_is_lens(name: &str) -> bool {
    name.contains("lens")
}

impl PreparedFpvModel {
    pub fn build(
        catalog: &FpvMeshCatalog,
        catalog_entry: usize,
        hide: Option<&[u32; 6]>,
        admission: &FpvMaterialAdmission,
    ) -> Result<Self, FpvRigError> {
        let entry = catalog
            .get_at(catalog_entry)
            .ok_or(FpvRigError::Catalog("model"))?;
        let skel = &entry.skel;
        let lod_range = skel.surfaces_for_lod(0);
        let first_surface = lod_range.start;
        // bo2zm: Black Ops II hides a tag's geometry, not every surface that
        // touches it (the DSR-50's body shares surfaces with the iron sights
        // its scope variant hides): a surface goes only when all its bones
        // are hidden; the rest lose the hidden bones' vertices when posed.
        let t6 = entry.namespace == asset_core::AssetNamespace::T6;
        let hidden = |surface: usize| {
            hide.is_some_and(|words| {
                skel.surface_part_bits.get(surface).is_some_and(|bits| {
                    if t6 {
                        bits.iter().any(|b| *b != 0)
                            && bits.iter().zip(words.iter()).all(|(b, w)| b & !w == 0)
                    } else {
                        surface_hidden(bits, words, 0)
                    }
                })
            })
        };
        let admitted = |surface: usize| match admission.verdict(catalog_entry, surface) {
            Some(FpvSurfaceVerdict::Admitted(row)) => Some(*row),
            _ => None,
        };
        let layout = build_skin_layout(skel, 0, |lod_local| {
            let surface = first_surface + lod_local;
            !hidden(surface) && admitted(surface).is_some()
        })
        .ok_or(FpvRigError::Layout("skin layout refused the model"))?;

        let mut refusal = None;
        for surface in lod_range.clone() {
            if hidden(surface) {
                continue;
            }
            if let Some(FpvSurfaceVerdict::Refused { material, cause }) =
                admission.verdict(catalog_entry, surface)
            {
                refusal = Some(FpvMandatoryRefusal {
                    model: skel.name.clone(),
                    surface,
                    material: material.clone(),
                    cause: *cause,
                });
                break;
            }
        }

        let mut surfaces = Vec::new();
        for surface in &layout.surfaces {
            if !surface.visible || surface.index_count == 0 {
                continue;
            }
            let Some(authored) = skel
                .surface_materials
                .get(surface.surface_index)
                .copied()
                .flatten()
                .map(|index| index.get())
            else {
                continue;
            };
            let Some(material) = admitted(surface.surface_index) else {
                continue;
            };
            surfaces.push(PreparedFpvSurface {
                index_start: surface.index_start,
                index_count: surface.index_count,
                material,
                authored,
                lens_named: entry
                    .material_keys
                    .get(surface.surface_index)
                    .and_then(|key| Some(key.as_ref()?.name.as_str()))
                    .is_some_and(leftover_scope_surf_is_lens),
            });
        }
        Ok(Self {
            catalog_entry,
            posed_surface_n: lod_range.len(),
            packed_ok: skel.packed_vertices.len() == skel.positions.len(),
            layout,
            surfaces,
            refusal,
        })
    }

    pub fn refusal(&self) -> Option<&FpvMandatoryRefusal> {
        self.refusal.as_ref()
    }
}

struct ComposedPart {
    model: Arc<PreparedFpvModel>,
    owner: FpvSurfOwner,
    bone_base: usize,
}

pub struct PreparedFpvComposition {
    assembly: Arc<FpvAssembly>,
    parts: Vec<ComposedPart>,
    refusal: Option<FpvMandatoryRefusal>,
}

fn surf_owner(role: FpvPartRole) -> FpvSurfOwner {
    match role {
        FpvPartRole::Hands => FpvSurfOwner::Hands,
        FpvPartRole::Gun | FpvPartRole::Knife => FpvSurfOwner::Gun,
        FpvPartRole::Attachment => FpvSurfOwner::Scope,
        FpvPartRole::Rocket => FpvSurfOwner::Rocket,
    }
}

impl PreparedFpvComposition {
    pub fn compose(
        assembly: Arc<FpvAssembly>,
        mut model_of: impl FnMut(usize, Option<[u32; 6]>) -> Result<Arc<PreparedFpvModel>, String>,
    ) -> Result<Self, String> {
        let mut parts = Vec::with_capacity(assembly.parts.len());
        let mut refusal = None;
        for part in &assembly.parts {
            let model = model_of(part.model.order(), part.hide)?;
            if refusal.is_none() {
                refusal = model.refusal().cloned();
            }
            parts.push(ComposedPart {
                model,
                owner: surf_owner(part.role),
                bone_base: part.bone_base,
            });
        }
        Ok(Self {
            assembly,
            parts,
            refusal,
        })
    }

    pub fn assembly(&self) -> &Arc<FpvAssembly> {
        &self.assembly
    }

    pub fn refusal(&self) -> Option<&FpvMandatoryRefusal> {
        self.refusal.as_ref()
    }
}

pub fn compose_clip_tracks(
    assembly: &FpvAssembly,
    clip: usize,
    tracks: &FpvClipTracks,
) -> Option<Arc<[u16]>> {
    let composed = assembly.compose_tracks(clip, tracks)?;
    Some(
        composed
            .into_iter()
            .map(|bone| {
                bone.and_then(|bone| u16::try_from(bone).ok())
                    .unwrap_or(FpvClipTracks::NONE)
            })
            .collect(),
    )
}

/// One model drawn for one hand. The same prepared model appears twice in a
/// dual-wield rig — once per hand — with a destination range of its own.
struct PlanSlot {
    hand: usize,
    part: usize,
    dest_base: usize,
}

pub struct PreparedFpvGeometry {
    segments: Vec<(Arc<PreparedFpvModel>, u32)>,
    pub index_n: usize,
    pub surface_ranges: Vec<(u32, u32)>,
    pub materials: Vec<SmodelPassMaterial>,
    pub draws: Vec<FpvSurfaceDraw>,
    pub dest_n: usize,
    pub packed_ok: bool,

    pub hands_plan_n: u32,
    pub gun_plan_n: u32,
    pub scope_plan_n: u32,
    pub scope_house_plan_n: u32,
    pub scope_lens_plan_n: u32,
    pub plan_draw_n: u32,
    pub plan_skip_n: u32,
}

impl PreparedFpvGeometry {
    pub fn write_indices(&self, out: &mut Vec<u32>) {
        out.clear();
        out.reserve(self.index_n);
        for (model, rebase) in &self.segments {
            out.extend(model.layout.indices.iter().map(|index| index + rebase));
        }
    }
}

pub struct FpvHandPose {
    skin: Vec<Mat4>,
    eye_from_world: Mat4,
    offset: Vec3,
    pub lens: Mat4,
    pub bolt: FpvBoltFrame,
}

pub struct PreparedFpvRig {
    composition: Arc<PreparedFpvComposition>,
    dual: bool,
    generation: u64,
    parts: PartBits,
    tags: FpvBoltTags,
    slots: Vec<PlanSlot>,
    tracks: [Vec<Option<Arc<[u16]>>>; 2],
    pub geometry: PreparedFpvGeometry,
}

impl PreparedFpvRig {
    /// A plan holding this generation is a plan whose indices, ranges,
    /// materials and draws are this rig's.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_dual(&self) -> bool {
        self.dual
    }

    pub fn build(
        composition: Arc<PreparedFpvComposition>,
        dual: bool,
        admission: &FpvMaterialAdmission,
        tracks: [Vec<Option<Arc<[u16]>>>; 2],
    ) -> Self {
        let parts_n = composition.parts.len();

        // Every hand's view hands first, then every hand's gun and whatever
        // hangs off it.
        let hand_n = if dual { 2 } else { 1 };
        let mut order: Vec<(usize, usize)> = Vec::new();
        for hand in 0..hand_n {
            order.push((hand, 0));
        }
        for hand in 0..hand_n {
            for part in 1..parts_n {
                // The scope and the rocket hang off the right gun only.
                if hand == 1 && part != 1 {
                    continue;
                }
                order.push((hand, part));
            }
        }

        let mut geometry = PreparedFpvGeometry {
            segments: Vec::with_capacity(order.len()),
            index_n: 0,
            surface_ranges: Vec::new(),
            materials: Vec::new(),
            draws: Vec::new(),
            dest_n: 0,
            packed_ok: true,
            hands_plan_n: 0,
            gun_plan_n: 0,
            scope_plan_n: 0,
            scope_house_plan_n: 0,
            scope_lens_plan_n: 0,
            plan_draw_n: 0,
            plan_skip_n: 0,
        };
        let mut slots: Vec<PlanSlot> = Vec::with_capacity(order.len());
        let mut material_key: HashMap<usize, u32> = HashMap::new();
        let mut dest_base = 0usize;
        let mut posed_surface_n = 0usize;
        for (hand, part_index) in order {
            let part = &composition.parts[part_index];
            let model = &part.model;
            posed_surface_n += model.posed_surface_n;
            geometry.packed_ok &= model.packed_ok;
            let index_base = geometry.index_n as u32;
            for surface in &model.surfaces {
                let Some(material) = admission.materials.get(surface.material as usize) else {
                    continue;
                };
                let is_scope = part.owner == FpvSurfOwner::Scope;
                let is_lens = is_scope && surface.lens_named;
                let range_index = geometry.surface_ranges.len() as u32;
                geometry
                    .surface_ranges
                    .push((index_base + surface.index_start, surface.index_count));
                let material_index = *material_key.entry(surface.authored).or_insert_with(|| {
                    let index = geometry.materials.len() as u32;
                    geometry.materials.push(material.clone());
                    index
                });
                geometry.draws.push(FpvSurfaceDraw {
                    surface: range_index,
                    material: material_index,
                    is_scope: is_scope && !is_lens,
                });
                if hand == 0 {
                    match part.owner {
                        FpvSurfOwner::Hands => geometry.hands_plan_n += 1,
                        _ => geometry.gun_plan_n += 1,
                    }
                }
                if is_scope {
                    geometry.scope_plan_n += 1;
                    if is_lens {
                        geometry.scope_lens_plan_n += 1;
                    } else {
                        geometry.scope_house_plan_n += 1;
                    }
                }
            }
            geometry
                .segments
                .push((Arc::clone(model), dest_base as u32));
            geometry.index_n += model.layout.indices.len();
            slots.push(PlanSlot {
                hand,
                part: part_index,
                dest_base,
            });
            dest_base += model.layout.dest_vertex_n;
        }
        geometry.dest_n = dest_base;
        geometry.plan_draw_n = geometry.draws.len() as u32;
        geometry.plan_skip_n = posed_surface_n.saturating_sub(geometry.draws.len()) as u32;

        let assembly = &composition.assembly;
        let tags = FpvBoltTags {
            flash: assembly.tags.flash,
            flash_silenced: assembly.tags.flash_silenced,
            brass: assembly.tags.brass,
            knife: assembly.tags.knife,
            laser: assembly.tags.laser,
            tracker_screen: assembly.tags.tracker_screen,
            tracker_light: assembly.tags.tracker_light,
            flash_left: assembly.tags.flash_left,
            brass_left: assembly.tags.brass_left,
        };
        let parts = assembly.dobj.all_parts();

        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            composition,
            dual,
            generation: GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            parts,
            tags,
            slots,
            tracks,
            geometry,
        }
    }

    /// One hand's bones for this frame. No skeleton is built here.
    /// `camera` (bo2zm) is a camera-only clip that takes the camera tag over.
    pub fn pose_hand(
        &self,
        hand: usize,
        anims: &[PosedClip<'_>],
        offset: Vec3,
        camera: Option<&PosedClip<'_>>,
    ) -> Option<FpvHandPose> {
        let listed: Vec<(usize, &PosedClip<'_>)> = anims.iter().map(|a| (hand, a)).collect();
        self.pose_listed(hand, &listed, offset, camera)
    }

    /// bo2zm: Black Ops II dual wield: one set of arms holding both guns,
    /// the right hand's clips (bound by the right table) and the left
    /// hand's (by the left table) posed together. Their animations key
    /// disjoint bones (`j_gun`, `tag_weapon`, the right arm / `j_gun1`,
    /// `tag_weapon1`, the left arm), so each bone takes its own side's.
    pub fn pose_combined(
        &self,
        right: &[PosedClip<'_>],
        left: &[PosedClip<'_>],
        camera: Option<&PosedClip<'_>>,
    ) -> Option<FpvHandPose> {
        let listed: Vec<(usize, &PosedClip<'_>)> = right
            .iter()
            .map(|a| (0, a))
            .chain(left.iter().map(|a| (1, a)))
            .collect();
        self.pose_listed(0, &listed, Vec3::ZERO, camera)
    }

    /// Clips each bound by the given hand's track table, posed as `hand`.
    fn pose_listed(
        &self,
        hand: usize,
        anims: &[(usize, &PosedClip<'_>)],
        offset: Vec3,
        camera: Option<&PosedClip<'_>>,
    ) -> Option<FpvHandPose> {
        if anims.is_empty() {
            return None;
        }
        let bound: Vec<(&PosedClip<'_>, Vec<Option<usize>>)> = anims
            .iter()
            .filter_map(|&(table, anim)| {
                let tracks = self.tracks[table].get(anim.node)?.as_ref()?;
                let bones = tracks
                    .iter()
                    .map(|&bone| (bone != FpvClipTracks::NONE).then_some(usize::from(bone)))
                    .collect();
                Some((anim, bones))
            })
            .collect();
        if !bound
            .iter()
            .any(|(_, bones)| bones.iter().any(Option::is_some))
        {
            return None;
        }
        let instances: Vec<AnimInstance<'_>> = bound
            .iter()
            .map(|(anim, bones)| AnimInstance {
                clip: anim.clip,
                tracks: bones,
                time: anim.time,
                weight: anim.weight,
                parts: None,
            })
            .collect();
        let assembly = &self.composition.assembly;
        // bo2zm: a camera-only clip (the climb's) takes the camera tag over
        // the other clips by its weight; the lens follows the camera tag.
        let camera = camera.zip(assembly.camera_bone).and_then(|(anim, bone)| {
            let tracks = self.tracks[0].get(anim.node)?.as_ref()?;
            let track = tracks
                .iter()
                .position(|&b| b != FpvClipTracks::NONE && usize::from(b) == bone)?;
            let sample = anim.clip.sample_track(track, anim.time)?;
            Some((bone, sample, anim.weight.clamp(0.0, 1.0)))
        });
        let world = assembly.dobj.pose_with_controller(
            &instances,
            &self.parts,
            Mat4::IDENTITY,
            |_, _, locals| {
                let Some((bone, sample, weight)) = camera else {
                    return;
                };
                let Some(local) = locals.get_mut(bone) else {
                    return;
                };
                if let Some(q) = sample.rotation {
                    let q = if anim_iw4::quat_dot(local.rotation, q) < 0.0 {
                        anim_iw4::quat_neg(q)
                    } else {
                        q
                    };
                    let r = local.rotation;
                    local.rotation = anim_iw4::normalize(core::array::from_fn(|i| {
                        r[i] + (q[i] - r[i]) * weight
                    }));
                }
                if let Some(t) = sample.translation {
                    let o = local.translation;
                    local.translation = core::array::from_fn(|i| o[i] + (t[i] - o[i]) * weight);
                }
            },
        );
        let mut skin = assembly.dobj.skin_matrices(&world);
        // A hidden tag's vertices collapse (its surfaces that also hold
        // other bones still draw the rest).
        for (part, slot) in assembly.parts.iter().zip(&assembly.dobj.models) {
            let Some(words) = part.hide else {
                continue;
            };
            for bone in 0..slot.bone_count {
                if anim_iw4::hide_part_bit(&words, bone)
                    && let Some(m) = skin.get_mut(part.bone_base + bone)
                {
                    *m = Mat4::ZERO;
                }
            }
        }
        let eye_from_world = tag_view_to_bevy_camera() * world[assembly.view_bone].inverse();
        let lens = assembly
            .camera_bone
            .map(|camera| tag_camera_lens_local(world[assembly.view_bone], world[camera]))
            .unwrap_or(Mat4::IDENTITY);
        let published = if hand == 0 {
            world.len()
        } else {
            assembly.paired_bones.min(world.len())
        };
        let shift = (offset != Vec3::ZERO).then(|| Mat4::from_translation(offset));
        let bones = world[..published]
            .iter()
            .map(|bone| {
                let bone = eye_from_world * *bone;
                match shift {
                    Some(shift) => shift * bone,
                    None => bone,
                }
            })
            .collect();
        Some(FpvHandPose {
            skin,
            eye_from_world,
            offset,
            lens,
            bolt: FpvBoltFrame {
                bones,
                tags: self.tags,
            },
        })
    }

    /// Skin every slot into the destination buffer the plan published. A vertex
    /// update only: indices, surface ranges and materials are untouched.
    pub fn skin_into(
        &self,
        catalog: &FpvMeshCatalog,
        poses: &[Option<FpvHandPose>; 2],
        dest: &mut [[u8; asset_iw4::size::GFX_PACKED_VERTEX]],
    ) {
        for slot in &self.slots {
            let Some(pose) = poses[slot.hand].as_ref() else {
                continue;
            };
            let part = &self.composition.parts[slot.part];
            let model = &part.model;
            let Some(skel) = catalog.get_at(model.catalog_entry).map(|entry| &entry.skel) else {
                continue;
            };
            let end = slot.dest_base.saturating_add(model.layout.dest_vertex_n);
            if end > dest.len() {
                continue;
            }
            let rows = &mut dest[slot.dest_base..end];
            skin_packed_into(
                skel,
                |bone| pose.eye_from_world * pose.skin[part.bone_base + bone],
                |_| false,
                0,
                &model.layout,
                rows,
            );
            if pose.offset != Vec3::ZERO {
                translate_packed_rows(rows, pose.offset);
            }
        }
    }
}

fn translate_packed_rows(rows: &mut [[u8; asset_iw4::size::GFX_PACKED_VERTEX]], delta: Vec3) {
    for row in rows {
        let x = f32::from_le_bytes([row[0], row[1], row[2], row[3]]) + delta.x;
        let y = f32::from_le_bytes([row[4], row[5], row[6], row[7]]) + delta.y;
        let z = f32::from_le_bytes([row[8], row[9], row[10], row[11]]) + delta.z;
        row[0..4].copy_from_slice(&x.to_le_bytes());
        row[4..8].copy_from_slice(&y.to_le_bytes());
        row[8..12].copy_from_slice(&z.to_le_bytes());
    }
}
