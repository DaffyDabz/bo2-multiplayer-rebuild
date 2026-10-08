use std::collections::HashMap;
use std::sync::Arc;

use anim_iw4::set_hide_part_bit;
use asset_model::{FpvMeshCatalog, FpvMountPlan};
use xmodel_runtime::{AnimClip, Attach, DObj, DObjError, ModelPoseSrc};

use crate::FpvMeshIndex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpvPartRole {
    Hands,
    Gun,
    Attachment,
    Rocket,
    Knife,
}

#[derive(Clone, Debug)]
pub struct FpvAssemblyPart {
    pub model: FpvMeshIndex,
    pub role: FpvPartRole,
    pub bone_base: usize,
    pub hide: Option<[u32; 6]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FpvAssemblyTags {
    pub flash: Option<u16>,
    pub flash_silenced: Option<u16>,
    pub brass: Option<u16>,
    pub knife: Option<u16>,
    pub laser: Option<u16>,
    pub tracker_screen: [Option<u16>; 3],
    pub tracker_light: Option<u16>,
    /// bo2zm: a dual-wield weapon's left gun's flash and brass tags
    /// (`tag_flash1`, `tag_brass1`) when it hangs on the arms.
    pub flash_left: Option<u16>,
    pub brass_left: Option<u16>,
}

#[derive(Debug)]
pub struct FpvAssembly {
    pub dobj: DObj,
    pub parts: Vec<FpvAssemblyPart>,
    pub view_bone: usize,
    pub camera_bone: Option<usize>,
    pub paired_bones: usize,
    pub tags: FpvAssemblyTags,
    /// bo2zm: a second gun hangs on the arms (Black Ops II dual wield: one
    /// set of arms, both guns, the left and right animations together).
    pub arms_gun: bool,
}

#[derive(Debug)]
pub enum FpvAssemblyError {
    Catalog(&'static str),
    Skeleton(DObjError),
    NoTagView,
}

impl core::fmt::Display for FpvAssemblyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Catalog(what) => write!(f, "{what} missing from the first-person catalog"),
            Self::Skeleton(error) => write!(f, "combined skeleton: {error}"),
            Self::NoTagView => write!(f, "combined skeleton has no tag_view"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FpvAssemblyKey {
    pub hands: FpvMeshIndex,
    pub gun: FpvMeshIndex,
    pub attachments: Vec<FpvMeshIndex>,
    /// bo2zm: each attachment's root placement, as bits.
    pub attachment_binds: Vec<Option<([u32; 4], [u32; 3])>>,
    pub rocket: Option<FpvMeshIndex>,
    pub knife: Option<FpvMeshIndex>,
    pub hide_tags: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FpvSideAssemblies {
    pub bare: Arc<FpvAssembly>,
    pub rocket: Option<Arc<FpvAssembly>>,
    pub melee: Option<Arc<FpvAssembly>>,
    /// bo2mp: the gun while aimed, when its sight swaps to a stripped model.
    pub ads: Option<Arc<FpvAssembly>>,
}

impl FpvSideAssemblies {
    pub fn pick(&self, rocket: bool) -> &Arc<FpvAssembly> {
        match (&self.rocket, rocket) {
            (Some(with), true) => with,
            _ => &self.bare,
        }
    }
}

fn hide_words(
    catalog: &FpvMeshCatalog,
    model: FpvMeshIndex,
    hide_tags: &[String],
) -> Option<[u32; 6]> {
    let skel = &catalog.get_at(model.order())?.skel;
    if hide_tags.is_empty() || skel.surface_part_bits.len() != skel.surface_vertex_ranges.len() {
        return None;
    }
    let mut words = [0u32; 6];
    for bone in 0..skel.bone_names.len() {
        if crate::bone_has_hidden_ancestor(&skel.bone_names, |b| skel.parent_of(b), bone, hide_tags)
        {
            set_hide_part_bit(&mut words, bone);
        }
    }
    Some(words)
}

impl FpvAssembly {
    pub fn build(
        catalog: &FpvMeshCatalog,
        hands: FpvMeshIndex,
        mounts: &FpvMountPlan,
        rocket: bool,
        knife: Option<FpvMeshIndex>,
        hide_tags: &[String],
    ) -> Result<Self, FpvAssemblyError> {
        let pose_of = |model: FpvMeshIndex| -> Result<&ModelPoseSrc, FpvAssemblyError> {
            catalog
                .get_at(model.order())
                .and_then(|entry| entry.skel.pose.as_ref())
                .ok_or(FpvAssemblyError::Catalog("model pose"))
        };
        // bo2zm: a melee weapon's model rooted at `tag_knife` (Black Ops II's
        // `viewmodel_knife`) mounts on the arms' `tag_knife_attach`, the bone
        // its swing animates; every other weapon on `tag_weapon`.
        let knife_rooted = catalog
            .get_at(mounts.gun.order())
            .and_then(|entry| entry.skel.bone_names.first())
            .is_some_and(|root| root.eq_ignore_ascii_case("tag_knife"));
        let mut parts = vec![
            (hands, FpvPartRole::Hands, None),
            (
                mounts.gun,
                FpvPartRole::Gun,
                Some(Attach {
                    parent_model: 0,
                    tag: if knife_rooted { "tag_knife_attach" } else { "tag_weapon" }.into(),
                }),
            ),
        ];
        let mut root_binds: Vec<Option<(bevy::math::Quat, bevy::math::Vec3)>> = vec![None, None];
        for mount in &mounts.attachments {
            root_binds.push(mount.root_bind.map(|(q, t)| {
                (bevy::math::Quat::from_array(q).normalize(), bevy::math::Vec3::from_array(t))
            }));
            parts.push((
                mount.model,
                // A gun on the arms (the left one of a dual-wield pair) is
                // a gun, not a scope.
                if mount.parent_model == 0 {
                    FpvPartRole::Gun
                } else {
                    FpvPartRole::Attachment
                },
                Some(Attach {
                    parent_model: mount.parent_model,
                    tag: mount.tag.clone(),
                }),
            ));
        }
        if rocket {
            let mount = mounts
                .rocket
                .as_ref()
                .ok_or(FpvAssemblyError::Catalog("rocket mount"))?;
            parts.push((
                mount.model,
                FpvPartRole::Rocket,
                Some(Attach {
                    parent_model: mount.parent_model,
                    tag: mount.tag.clone(),
                }),
            ));
        }
        if let Some(model) = knife {
            parts.push((
                model,
                FpvPartRole::Knife,
                Some(Attach {
                    parent_model: 0,
                    tag: "tag_knife_attach".into(),
                }),
            ));
        }
        let mut specs = Vec::with_capacity(parts.len());
        for (model, _, attach) in &parts {
            specs.push((pose_of(*model)?, attach.clone()));
        }
        let dobj =
            DObj::build_with_root_binds(&specs, &root_binds).map_err(FpvAssemblyError::Skeleton)?;

        let view_bone = dobj.find("tag_view").ok_or(FpvAssemblyError::NoTagView)?;
        let camera_bone = dobj.find("tag_camera");
        let tag = |name: &str| dobj.find(name).and_then(|index| u16::try_from(index).ok());
        let tags = FpvAssemblyTags {
            flash: tag("tag_flash"),
            flash_silenced: tag("tag_flash_silenced"),
            brass: tag("tag_brass"),
            knife: tag("tag_knife_fx"),
            laser: tag(fx_iw4::FX_LASER_TAG),
            tracker_screen: [
                tag("tag_screen_tl"),
                tag("tag_screen_bl"),
                tag("tag_screen_br"),
            ],
            tracker_light: tag("tag_motion_tracker_fx"),
            flash_left: tag("tag_flash1"),
            brass_left: tag("tag_brass1"),
        };
        let arms_gun = mounts.attachments.iter().any(|mount| mount.parent_model == 0);
        let paired_bones = specs[0].0.num_bones + specs[1].0.num_bones;
        let parts = parts
            .into_iter()
            .zip(&dobj.models)
            .map(|((model, role, _), slot)| FpvAssemblyPart {
                model,
                role,
                bone_base: slot.base,
                hide: if role == FpvPartRole::Gun {
                    hide_words(catalog, model, hide_tags)
                } else {
                    None
                },
            })
            .collect();
        Ok(Self {
            dobj,
            parts,
            view_bone,
            camera_bone,
            paired_bones,
            tags,
            arms_gun,
        })
    }

    pub fn compose_tracks(
        &self,
        clip: usize,
        tracks: &FpvClipTracks,
    ) -> Option<Vec<Option<usize>>> {
        let tables: Vec<Option<&[u16]>> = self
            .parts
            .iter()
            .map(|part| tracks.table(clip, part.model))
            .collect();
        let track_n = tables.iter().flatten().next()?.len();
        Some(
            (0..track_n)
                .map(|track| {
                    self.parts.iter().zip(&tables).find_map(|(part, table)| {
                        let local = *table.as_ref()?.get(track)?;
                        (local != FpvClipTracks::NONE).then(|| part.bone_base + usize::from(local))
                    })
                })
                .collect(),
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct FpvClipTracks {
    tables: HashMap<(usize, FpvMeshIndex), Arc<[u16]>>,
}

impl FpvClipTracks {
    pub const NONE: u16 = u16::MAX;

    pub fn table(&self, clip: usize, model: FpvMeshIndex) -> Option<&[u16]> {
        self.tables.get(&(clip, model)).map(|table| &table[..])
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    pub fn bind(
        &mut self,
        catalog: &FpvMeshCatalog,
        clip_index: usize,
        clip: &AnimClip,
        model: FpvMeshIndex,
    ) {
        if self.tables.contains_key(&(clip_index, model)) {
            return;
        }
        let Some(pose) = catalog
            .get_at(model.order())
            .and_then(|entry| entry.skel.pose.as_ref())
        else {
            return;
        };
        let mut first_by_name: HashMap<&str, u16> = HashMap::with_capacity(pose.num_bones);
        for (index, name) in pose.bone_names.iter().take(pose.num_bones).enumerate() {
            if let Ok(index) = u16::try_from(index) {
                first_by_name.entry(name.as_str()).or_insert(index);
            }
        }
        let table: Arc<[u16]> = clip
            .tracks
            .iter()
            .map(|track| {
                first_by_name
                    .get(track.name.as_str())
                    .copied()
                    .unwrap_or(Self::NONE)
            })
            .collect();
        self.tables.insert((clip_index, model), table);
    }
}
