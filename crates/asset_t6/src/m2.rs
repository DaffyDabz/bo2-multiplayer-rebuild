//! bo2zm M2: what a T6 zone walk keeps for guns, effects and sound.
//!
//! Weapons (`WeaponVariantDef` + its shared `WeaponDef`), effects
//! (`FxEffectDef` and its elements), tracers, the bullet impact table, the
//! footstep tables, sound banks (aliases) and the sound driver's falloff
//! curves. Everything is copied while the asset is still in place; asset
//! pointers become names (or keys into the capture's lists), so nothing
//! here points into a zone that has closed.

use fastfile_t6::layout as l;
use fastfile_t6::{AssetType, Ptr, ZoneStream};

use crate::capture::{AssetKey, ZoneCapture, deref, string_field};

type Result<T> = fastfile_t6::Result<T>;

/// `WeaponDef` fields that are `const char*` (sound aliases, names), by
/// offset, from the generated zone walk (`ld_WeaponDef`'s `xstring` calls).
pub const WEAPON_DEF_STRINGS: [usize; 104] = [
    0, 12, 64, 148, 152, 156, 160, 164, 168, 172, 176, 180, 184, 188, 192, 196, 200, 204, 208, 212,
    216, 220, 224, 228, 232, 236, 240, 244, 248, 252, 256, 260, 264, 268, 272, 276, 280, 284, 288,
    292, 296, 300, 304, 308, 312, 316, 320, 324, 328, 332, 336, 340, 344, 348, 352, 356, 360, 364,
    368, 372, 376, 380, 384, 388, 392, 396, 400, 404, 408, 412, 416, 420, 424, 428, 432, 440, 444,
    448, 980, 1100, 1104, 1108, 1112, 1116, 1120, 1388, 1652, 1656, 1820, 1824, 1828, 1832, 1920,
    2124, 2128, 2236, 2240, 2284, 2304, 2308, 2312, 2316, 2356, 2360,
];

/// `WeaponDef` fields that point at an `FxEffectDef`.
pub const WEAPON_DEF_FX: [usize; 20] = [
    108, 112, 116, 464, 468, 472, 476, 1776, 1784, 1792, 1800, 1808, 1816, 1888, 1916, 2372, 2376,
    2404, 2408, 2412,
];

/// `WeaponDef` fields that point at a `Material`.
pub const WEAPON_DEF_MATERIALS: [usize; 7] = [528, 532, 936, 940, 948, 956, 1632];

/// `WeaponDef` fields that point at one `XModel` (`gunXModel` and
/// `worldModel` are arrays of 16, read separately).
pub const WEAPON_DEF_XMODELS: [usize; 6] = [8, 920, 924, 928, 932, 1768];

/// `WeaponDef` fields that point at a `TracerDef`.
pub const WEAPON_DEF_TRACERS: [usize; 2] = [2320, 2324];

/// Number of `szXAnims` slots in T6 (`NUM_WEAP_ANIMS`).
pub const WEAPON_ANIM_COUNT: usize = 88;

/// bo2zm M3: a font (`Font_s`): its glyphs on the sheet its material draws
/// (Black Ops II's game text is `fonts/distFont`, a distance-field sheet).
#[derive(Clone, Debug, Default)]
pub struct FontRef {
    pub name: String,
    pub pixel_height: i32,
    pub material: String,
    pub glyphs: Vec<GlyphRef>,
    /// (first letter, second letter, amount).
    pub kerning: Vec<(u16, u16, i32)>,
}

/// One glyph: its letter, its box against the pen (x0, y0 from the pen and
/// the baseline, dx the advance, pixel size) and its place on the sheet.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphRef {
    pub letter: u16,
    pub x0: i8,
    pub y0: i8,
    pub dx: u8,
    pub pixel_width: u8,
    pub pixel_height: u8,
    pub st: [f32; 4],
}

/// bo2mp: a weapon attachment (`WeaponAttachment`, 284 bytes): what it
/// changes on any gun (zoom, ADS time scales, sway).
#[derive(Clone, Debug, Default)]
pub struct AttachmentRef {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl AttachmentRef {
    pub fn i32_at(&self, off: usize) -> i32 {
        self.bytes.get(off..off + 4).map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32_at(&self, off: usize) -> f32 {
        self.bytes.get(off..off + 4).map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u8_at(&self, off: usize) -> u8 {
        self.bytes.get(off).copied().unwrap_or(0)
    }
}

/// bo2mp: one gun's take on an attachment (`WeaponAttachmentUnique`, 424
/// bytes, `au_<gun>_<attachment>`): its models, sight, overlay, anims.
#[derive(Clone, Debug, Default)]
pub struct AttachmentUniqueRef {
    pub name: String,
    pub bytes: Vec<u8>,
    pub view_model: String,
    pub view_model_additional: String,
    pub view_model_ads: String,
    /// `szAltWeaponName`: the weapon this attachment switches to (Hybrid, Dual Band).
    pub alt_weapon_name: String,
    pub world_model: String,
    pub view_model_tag: String,
    pub world_model_tag: String,
    pub overlay_material: String,
    pub overlay_material_low: String,
    /// `szXAnims` (88 slots, empty = the gun's own).
    pub xanims: Vec<String>,
    pub hide_tags: Vec<String>,
}

impl AttachmentUniqueRef {
    pub fn i32_at(&self, off: usize) -> i32 {
        self.bytes.get(off..off + 4).map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32_at(&self, off: usize) -> f32 {
        self.bytes.get(off..off + 4).map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn vec3_at(&self, off: usize) -> [f32; 3] {
        [self.f32_at(off), self.f32_at(off + 4), self.f32_at(off + 8)]
    }
}

/// bo2zm M3: a weapon camo (`WeaponCamo`): per camo index its images
/// (`WeaponCamoSet`: solid, pattern, the pattern's offset and scale) and
/// its material swaps (`WeaponCamoMaterialSet`: each base material, the
/// camo material drawn instead, the shader constants).
#[derive(Clone, Debug, Default)]
pub struct WeaponCamoRef {
    pub name: String,
    pub solid_base_image: String,
    pub pattern_base_image: String,
    pub sets: Vec<WeaponCamoSetRef>,
    pub material_sets: Vec<Vec<WeaponCamoMaterialRef>>,
}

#[derive(Clone, Debug, Default)]
pub struct WeaponCamoSetRef {
    pub solid: String,
    pub pattern: String,
    pub offset: [f32; 2],
    pub scale: f32,
}

#[derive(Clone, Debug, Default)]
pub struct WeaponCamoMaterialRef {
    pub replace_flags: u16,
    pub base: Vec<String>,
    pub camo: Vec<String>,
    pub consts: [f32; 8],
}

/// One weapon: its variant and its shared definition, as bytes plus every
/// name they point at.
#[derive(Clone, Debug, Default)]
pub struct WeaponRef {
    pub name: String,
    /// `WeaponVariantDef`, 716 bytes.
    pub variant: Vec<u8>,
    /// `WeaponDef`, 2448 bytes (empty when the variant has none).
    pub def: Vec<u8>,
    pub display_name: String,
    pub alt_weapon_name: String,
    pub ammo_name: String,
    pub clip_name: String,
    /// The variant's `szXAnims` (88 slots).
    pub xanims: Vec<String>,
    /// The variant's `hideTags` (bone names).
    pub hide_tags: Vec<String>,
    /// The variant's `attachViewModel` slots that hold a model: (slot,
    /// model, `attachViewModelTag`, offset, rotation).
    pub attach_view_models: Vec<(usize, String, String, [f32; 3], [f32; 3])>,
    /// `gunXModel[16]` and `worldModel[16]`.
    pub gun_models: Vec<String>,
    pub world_models: Vec<String>,
    /// `WeaponDef` string fields by offset (`WEAPON_DEF_STRINGS`).
    pub strings: Vec<(usize, String)>,
    /// `WeaponDef` asset pointers by offset: effects, materials, models,
    /// tracers (`WEAPON_DEF_FX` etc.).
    pub fx: Vec<(usize, String)>,
    pub materials: Vec<(usize, String)>,
    pub models: Vec<(usize, String)>,
    pub tracers: Vec<(usize, String)>,
    /// The variant's materials: overlay (scope), low-res overlay, d-pad icon.
    pub overlay_material: String,
    pub overlay_material_low: String,
    pub dpad_icon: String,
    /// `notetrackSoundMapKeys` / `Values` (20 pairs, script strings).
    pub notetrack_sounds: Vec<(String, String)>,
    /// `bounceSound`: a projectile's bounce alias per surface type (32).
    pub bounce_sounds: Vec<String>,
    pub parallel_bounce: Option<Vec<f32>>,
    pub perpendicular_bounce: Option<Vec<f32>>,
    pub location_damage: Option<Vec<f32>>,
    /// bo2zm M3: its camo (`weaponCamo`), by name.
    pub camo: String,
}

impl WeaponRef {
    fn at<const N: usize>(bytes: &[u8], off: usize) -> Option<[u8; N]> {
        bytes.get(off..off + N)?.try_into().ok()
    }

    pub fn def_i32(&self, off: usize) -> i32 {
        Self::at::<4>(&self.def, off).map_or(0, i32::from_le_bytes)
    }

    pub fn def_f32(&self, off: usize) -> f32 {
        Self::at::<4>(&self.def, off).map_or(0.0, f32::from_le_bytes)
    }

    pub fn def_u8(&self, off: usize) -> u8 {
        self.def.get(off).copied().unwrap_or(0)
    }

    pub fn def_vec3(&self, off: usize) -> [f32; 3] {
        [
            self.def_f32(off),
            self.def_f32(off + 4),
            self.def_f32(off + 8),
        ]
    }

    pub fn var_i32(&self, off: usize) -> i32 {
        Self::at::<4>(&self.variant, off).map_or(0, i32::from_le_bytes)
    }

    pub fn var_f32(&self, off: usize) -> f32 {
        Self::at::<4>(&self.variant, off).map_or(0.0, f32::from_le_bytes)
    }

    pub fn var_u8(&self, off: usize) -> u8 {
        self.variant.get(off).copied().unwrap_or(0)
    }

    /// A `WeaponDef` string field by offset ("" when unset).
    pub fn def_string(&self, off: usize) -> &str {
        self.strings
            .iter()
            .find(|(o, _)| *o == off)
            .map_or("", |(_, s)| s.as_str())
    }

    pub fn def_fx(&self, off: usize) -> &str {
        self.fx
            .iter()
            .find(|(o, _)| *o == off)
            .map_or("", |(_, s)| s.as_str())
    }

    pub fn def_material(&self, off: usize) -> &str {
        self.materials
            .iter()
            .find(|(o, _)| *o == off)
            .map_or("", |(_, s)| s.as_str())
    }

    pub fn def_model(&self, off: usize) -> &str {
        self.models
            .iter()
            .find(|(o, _)| *o == off)
            .map_or("", |(_, s)| s.as_str())
    }

    pub fn def_tracer(&self, off: usize) -> &str {
        self.tracers
            .iter()
            .find(|(o, _)| *o == off)
            .map_or("", |(_, s)| s.as_str())
    }
}

/// One effect visual.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FxVisualRef {
    Material(String),
    /// A decal's pair (world, model).
    Mark([String; 2]),
    Model(String),
    /// A runner's effect, by name.
    Effect(String),
    Sound(String),
    Light(String),
    None,
}

/// One `FxElemDef`: its 292 bytes, samples and what it points at.
#[derive(Clone, Debug, Default)]
pub struct FxElemRef {
    pub raw: Vec<u8>,
    /// `velSamples`: (velIntervalCount + 1) x 96 bytes.
    pub vel_samples: Vec<u8>,
    /// `visSamples`: (visStateIntervalCount + 1) x 48 bytes.
    pub vis_samples: Vec<u8>,
    pub visuals: Vec<FxVisualRef>,
    pub effect_on_impact: String,
    pub effect_on_death: String,
    pub effect_emitted: String,
    pub effect_attached: String,
    /// A trail's definition: (scroll time, repeat distance, split
    /// distance, vertices as (pos xy, tex coord), indices).
    pub trail: Option<FxTrailRef>,
    /// A spot light's (fov inner fraction, start radius, end radius).
    pub spot: Option<[f32; 3]>,
    pub spawn_sound: String,
}

#[derive(Clone, Debug, Default)]
pub struct FxTrailRef {
    pub scroll_time_msec: i32,
    pub repeat_dist: i32,
    pub split_dist: f32,
    /// (pos x, pos y, normal x, normal y, tex coord) per vertex.
    pub verts: Vec<[f32; 5]>,
    pub inds: Vec<u16>,
}

impl FxElemRef {
    pub fn elem_type(&self) -> u8 {
        self.raw.get(l::FxElemDef::elemType).copied().unwrap_or(0)
    }

    pub fn i32_at(&self, off: usize) -> i32 {
        self.raw
            .get(off..off + 4)
            .map_or(0, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32_at(&self, off: usize) -> f32 {
        self.raw
            .get(off..off + 4)
            .map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// One `FxEffectDef`.
#[derive(Clone, Debug, Default)]
pub struct FxEffectRef {
    pub name: String,
    pub flags: u16,
    pub priority: u8,
    pub looping_count: i16,
    pub one_shot_count: i16,
    pub emission_count: i16,
    pub total_size: i32,
    pub msec_looping_life: i32,
    pub msec_non_looping_life: i32,
    /// Looping elements, then one-shot, then emitted.
    pub elems: Vec<FxElemRef>,
    pub bounds_dim: [f32; 3],
    pub bounds_centre: [f32; 3],
}

/// One `TracerDef`.
#[derive(Clone, Debug, Default)]
pub struct TracerRef {
    pub name: String,
    pub ty: i32,
    pub material: String,
    pub draw_interval: i32,
    pub speed: f32,
    pub beam_length: f32,
    pub beam_width: f32,
    pub screw_radius: f32,
    pub screw_dist: f32,
    pub fade_time: f32,
    pub fade_scale: f32,
    pub tex_repeat_rate: f32,
    pub colors: [[f32; 4]; 5],
}

/// `FxImpactTable`: 21 impact types x (32 non-flesh surfaces + 4 flesh).
#[derive(Clone, Debug, Default)]
pub struct ImpactTableRef {
    pub name: String,
    pub rows: Vec<([String; 32], [String; 4])>,
}

/// `FootstepTableDef`: per surface (32) seven alias ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FootstepTableRef {
    pub name: String,
    pub aliases: Vec<[u32; 7]>,
}

/// `FootstepFXTableDef`: per surface the effect a step kicks up.
#[derive(Clone, Debug, Default)]
pub struct FootstepFxTableRef {
    pub name: String,
    pub fx: Vec<String>,
}

/// One `SndAlias` (a variant of an alias name).
#[derive(Clone, Debug, Default)]
pub struct SndAliasRef {
    pub name: String,
    pub id: u32,
    pub subtitle: String,
    pub secondary: String,
    /// `SND_HashName` of the file name: the bank entry's id.
    pub asset_id: u32,
    pub asset_file: String,
    /// `SndAliasFlags`: flags0, flags1.
    pub flags: [u32; 2],
    pub duck: u32,
    pub context_type: u32,
    pub context_value: u32,
    pub stop_on_play: u32,
    pub futz_patch: u32,
    pub flux_time: u16,
    pub start_delay: u16,
    pub reverb_send: u16,
    pub center_send: u16,
    pub vol_min: u16,
    pub vol_max: u16,
    pub pitch_min: u16,
    pub pitch_max: u16,
    pub dist_min: u16,
    pub dist_max: u16,
    pub dist_reverb_max: u16,
    pub envelop_min: u16,
    pub envelop_max: u16,
    pub envelop_percentage: u16,
    pub fade_in: u16,
    pub fade_out: u16,
    pub doppler_scale: i16,
    pub min_priority_threshold: u8,
    pub max_priority_threshold: u8,
    pub probability: u8,
    pub occlusion_level: u8,
    pub min_priority: u8,
    pub max_priority: u8,
    pub pan: u8,
    pub limit_count: u8,
    pub entity_limit_count: u8,
    pub duck_group: u8,
}

impl SndAliasRef {
    pub fn looping(&self) -> bool {
        self.flags[0] & 1 != 0
    }

    /// `panType`: 0 = 2D, 1 = 3D.
    pub fn is_3d(&self) -> bool {
        self.flags[0] & 2 != 0
    }

    /// `loadType`: 1 loaded, 2 streamed, 3 primed.
    pub fn load_type(&self) -> u32 {
        (self.flags[0] >> 15) & 3
    }

    pub fn bus(&self) -> u32 {
        (self.flags[0] >> 11) & 0xf
    }

    pub fn volume_group(&self) -> u32 {
        (self.flags[0] >> 17) & 0x1f
    }

    /// `randomizeType` bits: 1 volume, 2 pitch, 4 variant.
    pub fn randomize(&self) -> u32 {
        (self.flags[0] >> 29) & 7
    }

    pub fn is_music(&self) -> bool {
        self.flags[0] & (1 << 6) != 0
    }

    pub fn volume_falloff_curve(&self) -> u32 {
        (self.flags[1] >> 2) & 0x3f
    }

    pub fn volume_min_falloff_curve(&self) -> u32 {
        (self.flags[1] >> 14) & 0x3f
    }

    pub fn volume(&self) -> (f32, f32) {
        (
            f32::from(self.vol_min) / 65535.0,
            f32::from(self.vol_max) / 65535.0,
        )
    }

    pub fn pitch(&self) -> (f32, f32) {
        (
            f32::from(self.pitch_min) / 32767.0,
            f32::from(self.pitch_max) / 32767.0,
        )
    }
}

/// One `SndAliasList`: an alias name and its variants.
#[derive(Clone, Debug, Default)]
pub struct SndAliasListRef {
    pub name: String,
    pub id: u32,
    pub sequence: i32,
    pub entries: Vec<SndAliasRef>,
}

/// One `SndBank`.
#[derive(Clone, Debug, Default)]
pub struct SndBankRef {
    pub name: String,
    pub aliases: Vec<SndAliasListRef>,
}

/// `SndCurve`: a falloff curve, 8 points.
#[derive(Clone, Debug, Default)]
pub struct SndCurveRef {
    pub name: String,
    pub id: u32,
    pub points: [[f32; 2]; 8],
}

/// `SndDriverGlobals` (the parts the engine uses: curves, volume groups).
#[derive(Clone, Debug, Default)]
pub struct SndDriverGlobalsRef {
    pub name: String,
    pub curves: Vec<SndCurveRef>,
    /// (name, parent name, attenuation sp, attenuation mp).
    pub groups: Vec<(String, String, u16, u16)>,
}

fn fixed_string(s: &ZoneStream<'_>, p: Ptr, off: usize, len: usize) -> Result<String> {
    let raw = s.slice_at(p, off, len)?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
}

impl ZoneCapture {
    /// A script string by its zone index.
    pub fn script_string(&self, id: u16) -> &str {
        self.strings
            .get(usize::from(id))
            .map_or("", String::as_str)
    }

    /// The name of the asset an asset pointer slot names: a kept asset's
    /// name, else the name at the body's start (assets whose first member
    /// is their name).
    pub(crate) fn asset_name_at(
        &mut self,
        s: &ZoneStream<'_>,
        slot: Ptr,
        ty: AssetType,
    ) -> Result<String> {
        if s.u32_at(slot, 0)? == 0 {
            return Ok(String::new());
        }
        // A name with a leading comma is a reference to an asset another
        // zone defines; the name after it is the asset's own.
        if let Some(key) = self.asset_at(s, slot)? {
            return Ok(self.kept_name(key).trim_start_matches(',').to_owned());
        }
        // Not kept (or not resolvable through the capture's slot map):
        // read the name the body starts with, when the body is still there.
        let _ = ty;
        Ok(match deref(s, slot, 0)? {
            Some(body) => string_field(s, body, 0)
                .unwrap_or_default()
                .trim_start_matches(',')
                .to_owned(),
            None => String::new(),
        })
    }

    pub(crate) fn kept_name(&self, key: AssetKey) -> String {
        match key.ty {
            AssetType::Material => self.materials.get(key.index).map(|m| m.name.clone()),
            AssetType::XModel => self.xmodels.get(key.index).map(|m| m.name.clone()),
            AssetType::Image => self.images.get(key.index).map(|m| m.name.clone()),
            AssetType::TechniqueSet => self.technique_sets.get(key.index).map(|m| m.name.clone()),
            AssetType::Fx => self.fx.get(key.index).map(|m| m.name.clone()),
            AssetType::Tracer => self.tracers.get(key.index).map(|m| m.name.clone()),
            AssetType::WeaponCamo => self.weapon_camos.get(key.index).map(|m| m.name.clone()),
            _ => None,
        }
        .unwrap_or_default()
    }

    pub(crate) fn read_font(&mut self, s: &ZoneStream<'_>, f: Ptr) -> Result<FontRef> {
        let mut out = FontRef {
            name: string_field(s, f, l::Font_s::fontName)?,
            pixel_height: s.u32_at(f, l::Font_s::pixelHeight)? as i32,
            material: self.asset_name_at(s, f.at(l::Font_s::material), AssetType::Material)?,
            ..Default::default()
        };
        let glyph_n = s.u32_at(f, l::Font_s::glyphCount)? as usize;
        if let Some(glyphs) = deref(s, f, l::Font_s::glyphs)? {
            for i in 0..glyph_n.min(4096) {
                let g = glyphs.at(i * l::Glyph::SIZE);
                out.glyphs.push(GlyphRef {
                    letter: s.u16_at(g, l::Glyph::letter)?,
                    x0: s.u8_at(g, l::Glyph::x0)? as i8,
                    y0: s.u8_at(g, l::Glyph::y0)? as i8,
                    dx: s.u8_at(g, l::Glyph::dx)?,
                    pixel_width: s.u8_at(g, l::Glyph::pixelWidth)?,
                    pixel_height: s.u8_at(g, l::Glyph::pixelHeight)?,
                    st: [
                        s.f32_at(g, l::Glyph::s0)?,
                        s.f32_at(g, l::Glyph::t0)?,
                        s.f32_at(g, l::Glyph::s1)?,
                        s.f32_at(g, l::Glyph::t1)?,
                    ],
                });
            }
        }
        let kern_n = s.u32_at(f, l::Font_s::kerningPairsCount)? as usize;
        if let Some(pairs) = deref(s, f, l::Font_s::kerningPairs)? {
            for i in 0..kern_n.min(8192) {
                let k = pairs.at(i * l::KerningPairs::SIZE);
                out.kerning.push((
                    s.u16_at(k, l::KerningPairs::wFirst)?,
                    s.u16_at(k, l::KerningPairs::wSecond)?,
                    s.u32_at(k, l::KerningPairs::iKernAmount)? as i32,
                ));
            }
        }
        Ok(out)
    }

    pub(crate) fn read_weapon_camo(&mut self, s: &ZoneStream<'_>, c: Ptr) -> Result<WeaponCamoRef> {
        let mut out = WeaponCamoRef {
            name: string_field(s, c, l::WeaponCamo::name)?,
            solid_base_image: self.asset_name_at(s, c.at(l::WeaponCamo::solidBaseImage), AssetType::Image)?,
            pattern_base_image: self.asset_name_at(s, c.at(l::WeaponCamo::patternBaseImage), AssetType::Image)?,
            ..Default::default()
        };
        let set_n = s.u32_at(c, l::WeaponCamo::numCamoSets)? as usize;
        if let Some(sets) = deref(s, c, l::WeaponCamo::camoSets)? {
            for i in 0..set_n.min(256) {
                let e = sets.at(i * l::WeaponCamoSet::SIZE);
                out.sets.push(WeaponCamoSetRef {
                    solid: self.asset_name_at(s, e.at(l::WeaponCamoSet::solidCamoImage), AssetType::Image)?,
                    pattern: self.asset_name_at(s, e.at(l::WeaponCamoSet::patternCamoImage), AssetType::Image)?,
                    offset: [
                        s.f32_at(e, l::WeaponCamoSet::patternOffset)?,
                        s.f32_at(e, l::WeaponCamoSet::patternOffset + 4)?,
                    ],
                    scale: s.f32_at(e, l::WeaponCamoSet::patternScale)?,
                });
            }
        }
        let mat_n = s.u32_at(c, l::WeaponCamo::numCamoMaterials)? as usize;
        if let Some(sets) = deref(s, c, l::WeaponCamo::camoMaterials)? {
            for i in 0..mat_n.min(64) {
                let set = sets.at(i * l::WeaponCamoMaterialSet::SIZE);
                let n = s.u32_at(set, l::WeaponCamoMaterialSet::numMaterials)? as usize;
                let mut list = Vec::new();
                if let Some(mats) = deref(s, set, l::WeaponCamoMaterialSet::materials)? {
                    for j in 0..n.min(256) {
                        let m = mats.at(j * l::WeaponCamoMaterial::SIZE);
                        let base_n = usize::from(s.u16_at(m, l::WeaponCamoMaterial::numBaseMaterials)?);
                        let mut base = Vec::new();
                        let mut camo = Vec::new();
                        if let Some(b) = deref(s, m, l::WeaponCamoMaterial::baseMaterials)? {
                            for k in 0..base_n.min(64) {
                                base.push(self.asset_name_at(s, b.at(k * 4), AssetType::Material)?);
                            }
                        }
                        if let Some(b) = deref(s, m, l::WeaponCamoMaterial::camoMaterials)? {
                            for k in 0..base_n.min(64) {
                                camo.push(self.asset_name_at(s, b.at(k * 4), AssetType::Material)?);
                            }
                        }
                        let mut consts = [0.0f32; 8];
                        for (k, c) in consts.iter_mut().enumerate() {
                            *c = s.f32_at(m, l::WeaponCamoMaterial::shaderConsts + k * 4)?;
                        }
                        list.push(WeaponCamoMaterialRef {
                            replace_flags: s.u16_at(m, l::WeaponCamoMaterial::replaceFlags)?,
                            base,
                            camo,
                            consts,
                        });
                    }
                }
                out.material_sets.push(list);
            }
        }
        Ok(out)
    }

    pub(crate) fn read_attachment(&mut self, s: &ZoneStream<'_>, a: Ptr) -> Result<AttachmentRef> {
        Ok(AttachmentRef {
            name: string_field(s, a, l::WeaponAttachment::szInternalName)?,
            bytes: s.slice_at(a, 0, l::WeaponAttachment::SIZE)?.to_vec(),
        })
    }

    pub(crate) fn read_attachment_unique(
        &mut self,
        s: &ZoneStream<'_>,
        a: Ptr,
    ) -> Result<AttachmentUniqueRef> {
        use l::WeaponAttachmentUnique as u;
        let model = |this: &mut Self, off: usize| this.asset_name_at(s, a.at(off), AssetType::XModel);
        let mut out = AttachmentUniqueRef {
            name: string_field(s, a, u::szInternalName)?,
            bytes: s.slice_at(a, 0, u::SIZE)?.to_vec(),
            view_model: model(self, u::viewModel)?,
            view_model_additional: model(self, u::viewModelAdditional)?,
            view_model_ads: model(self, u::viewModelADS)?,
            alt_weapon_name: string_field(s, a, u::szAltWeaponName)?,
            world_model: model(self, u::worldModel)?,
            view_model_tag: string_field(s, a, u::viewModelTag)?,
            world_model_tag: string_field(s, a, u::worldModelTag)?,
            overlay_material: self.asset_name_at(s, a.at(u::overlayMaterial), AssetType::Material)?,
            overlay_material_low: self.asset_name_at(s, a.at(u::overlayMaterialLowRes), AssetType::Material)?,
            ..Default::default()
        };
        if let Some(arr) = deref(s, a, u::szXAnims)? {
            for i in 0..WEAPON_ANIM_COUNT {
                out.xanims.push(string_field(s, arr, i * 4)?);
            }
        }
        if let Some(arr) = deref(s, a, u::hideTags)? {
            for i in 0..32 {
                let id = s.u16_at(arr, i * 2)?;
                if id != 0 {
                    out.hide_tags.push(self.script_string(id).to_owned());
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn read_weapon(&mut self, s: &ZoneStream<'_>, v: Ptr) -> Result<WeaponRef> {
        let variant = s.slice_at(v, 0, l::WeaponVariantDef::SIZE)?.to_vec();
        let mut w = WeaponRef {
            name: string_field(s, v, l::WeaponVariantDef::szInternalName)?,
            variant,
            display_name: string_field(s, v, l::WeaponVariantDef::szDisplayName)?,
            alt_weapon_name: string_field(s, v, l::WeaponVariantDef::szAltWeaponName)?,
            ammo_name: string_field(s, v, l::WeaponVariantDef::szAmmoName)?,
            clip_name: string_field(s, v, l::WeaponVariantDef::szClipName)?,
            ..Default::default()
        };
        if let Some(arr) = deref(s, v, l::WeaponVariantDef::szXAnims)? {
            for i in 0..WEAPON_ANIM_COUNT {
                w.xanims.push(string_field(s, arr, i * 4)?);
            }
        }
        if let Some(models) = deref(s, v, l::WeaponVariantDef::attachViewModel)? {
            let tags = deref(s, v, l::WeaponVariantDef::attachViewModelTag)?;
            for i in 0..8 {
                let model = self.asset_name_at(s, models.at(i * 4), AssetType::XModel)?;
                if model.is_empty() {
                    continue;
                }
                let tag = match tags {
                    Some(t) => string_field(s, t, i * 4)?,
                    None => String::new(),
                };
                let f = |base: usize| -> Result<[f32; 3]> {
                    Ok([
                        s.f32_at(v, base + i * 12)?,
                        s.f32_at(v, base + i * 12 + 4)?,
                        s.f32_at(v, base + i * 12 + 8)?,
                    ])
                };
                w.attach_view_models.push((
                    i,
                    model,
                    tag,
                    f(l::WeaponVariantDef::attachViewModelOffsets)?,
                    f(l::WeaponVariantDef::attachViewModelRotations)?,
                ));
            }
        }
        if let Some(arr) = deref(s, v, l::WeaponVariantDef::hideTags)? {
            for i in 0..32 {
                let id = s.u16_at(arr, i * 2)?;
                if id != 0 {
                    w.hide_tags.push(self.script_string(id).to_owned());
                }
            }
        }
        w.overlay_material =
            self.asset_name_at(s, v.at(l::WeaponVariantDef::overlayMaterial), AssetType::Material)?;
        w.overlay_material_low = self.asset_name_at(
            s,
            v.at(l::WeaponVariantDef::overlayMaterialLowRes),
            AssetType::Material,
        )?;
        w.dpad_icon = self.asset_name_at(s, v.at(l::WeaponVariantDef::dpadIcon), AssetType::Material)?;
        let Some(d) = deref(s, v, l::WeaponVariantDef::weapDef)? else {
            return Ok(w);
        };
        w.def = s.slice_at(d, 0, l::WeaponDef::SIZE)?.to_vec();
        w.camo = self.asset_name_at(s, d.at(l::WeaponDef::weaponCamo), AssetType::WeaponCamo)?;
        for off in WEAPON_DEF_STRINGS {
            let text = string_field(s, d, off)?;
            if !text.is_empty() {
                w.strings.push((off, text));
            }
        }
        for off in WEAPON_DEF_FX {
            let name = self.asset_name_at(s, d.at(off), AssetType::Fx)?;
            if !name.is_empty() {
                w.fx.push((off, name));
            }
        }
        for off in WEAPON_DEF_MATERIALS {
            let name = self.asset_name_at(s, d.at(off), AssetType::Material)?;
            if !name.is_empty() {
                w.materials.push((off, name));
            }
        }
        for off in WEAPON_DEF_XMODELS {
            let name = self.asset_name_at(s, d.at(off), AssetType::XModel)?;
            if !name.is_empty() {
                w.models.push((off, name));
            }
        }
        for off in WEAPON_DEF_TRACERS {
            let name = self.asset_name_at(s, d.at(off), AssetType::Tracer)?;
            if !name.is_empty() {
                w.tracers.push((off, name));
            }
        }
        for (field, out) in [
            (l::WeaponDef::gunXModel, &mut w.gun_models),
            (l::WeaponDef::worldModel, &mut w.world_models),
        ] {
            if let Some(arr) = deref(s, d, field)? {
                for i in 0..16 {
                    let name = self.asset_name_at(s, arr.at(i * 4), AssetType::XModel)?;
                    out.push(name);
                }
            }
        }
        if let (Some(keys), Some(values)) = (
            deref(s, d, l::WeaponDef::notetrackSoundMapKeys)?,
            deref(s, d, l::WeaponDef::notetrackSoundMapValues)?,
        ) {
            for i in 0..20 {
                let k = s.u16_at(keys, i * 2)?;
                let val = s.u16_at(values, i * 2)?;
                if k != 0 {
                    w.notetrack_sounds.push((
                        self.script_string(k).to_owned(),
                        self.script_string(val).to_owned(),
                    ));
                }
            }
        }
        if let Some(arr) = deref(s, d, l::WeaponDef::bounceSound)? {
            for i in 0..32 {
                w.bounce_sounds.push(string_field(s, arr, i * 4)?);
            }
        }
        let floats = |p: Option<Ptr>, n: usize| -> Result<Option<Vec<f32>>> {
            let Some(p) = p else { return Ok(None) };
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push(s.f32_at(p, i * 4)?);
            }
            Ok(Some(out))
        };
        w.parallel_bounce = floats(deref(s, d, l::WeaponDef::parallelBounce)?, 32)?;
        w.perpendicular_bounce = floats(deref(s, d, l::WeaponDef::perpendicularBounce)?, 32)?;
        w.location_damage = floats(deref(s, d, l::WeaponDef::locationDamageMultipliers)?, 21)?;
        Ok(w)
    }

    pub(crate) fn read_fx(&mut self, s: &ZoneStream<'_>, e: Ptr) -> Result<FxEffectRef> {
        let looping = s.i16_at(e, l::FxEffectDef::elemDefCountLooping)?;
        let one_shot = s.i16_at(e, l::FxEffectDef::elemDefCountOneShot)?;
        let emission = s.i16_at(e, l::FxEffectDef::elemDefCountEmission)?;
        let count = (i32::from(looping) + i32::from(one_shot) + i32::from(emission)).max(0) as usize;
        let mut elems = Vec::with_capacity(count);
        if let Some(arr) = deref(s, e, l::FxEffectDef::elemDefs)? {
            for i in 0..count {
                elems.push(self.read_fx_elem(s, arr.at(i * l::FxElemDef::SIZE))?);
            }
        }
        let v3 = |off: usize| -> Result<[f32; 3]> {
            Ok([s.f32_at(e, off)?, s.f32_at(e, off + 4)?, s.f32_at(e, off + 8)?])
        };
        Ok(FxEffectRef {
            name: string_field(s, e, l::FxEffectDef::name)?,
            flags: s.u16_at(e, l::FxEffectDef::flags)?,
            priority: s.u8_at(e, l::FxEffectDef::efPriority)?,
            looping_count: looping,
            one_shot_count: one_shot,
            emission_count: emission,
            total_size: s.i32_at(e, l::FxEffectDef::totalSize)?,
            msec_looping_life: s.i32_at(e, l::FxEffectDef::msecLoopingLife)?,
            msec_non_looping_life: s.i32_at(e, l::FxEffectDef::msecNonLoopingLife)?,
            elems,
            bounds_dim: v3(l::FxEffectDef::boundingBoxDim)?,
            bounds_centre: v3(l::FxEffectDef::boundingBoxCentre)?,
        })
    }

    fn read_fx_elem(&mut self, s: &ZoneStream<'_>, p: Ptr) -> Result<FxElemRef> {
        let raw = s.slice_at(p, 0, l::FxElemDef::SIZE)?.to_vec();
        let elem_type = raw[l::FxElemDef::elemType];
        let visual_count = raw[l::FxElemDef::visualCount] as i8;
        let vel_count = (raw[l::FxElemDef::velIntervalCount] as i8 as i32 + 1).max(0) as usize;
        let vis_count = (raw[l::FxElemDef::visStateIntervalCount] as i8 as i32 + 1).max(0) as usize;
        let vel_samples = match deref(s, p, l::FxElemDef::velSamples)? {
            Some(arr) => s
                .slice_at(arr, 0, vel_count * l::FxElemVelStateSample::SIZE)?
                .to_vec(),
            None => Vec::new(),
        };
        let vis_samples = match deref(s, p, l::FxElemDef::visSamples)? {
            Some(arr) => s
                .slice_at(arr, 0, vis_count * l::FxElemVisStateSample::SIZE)?
                .to_vec(),
            None => Vec::new(),
        };
        // Visuals: a decal's mark pairs, an array when more than one, else
        // the single visual stored in place.
        let mut visuals = Vec::new();
        let vslot = p.at(l::FxElemDef::visuals);
        if elem_type == 11 {
            if let Some(arr) = deref(s, vslot, 0)? {
                for i in 0..visual_count.max(0) as usize {
                    let pair = arr.at(i * l::FxElemMarkVisuals::SIZE);
                    visuals.push(FxVisualRef::Mark([
                        self.asset_name_at(s, pair, AssetType::Material)?,
                        self.asset_name_at(s, pair.at(4), AssetType::Material)?,
                    ]));
                }
            }
        } else if visual_count > 1 {
            if let Some(arr) = deref(s, vslot, 0)? {
                for i in 0..visual_count as usize {
                    visuals.push(self.read_fx_visual(s, arr.at(i * 4), elem_type)?);
                }
            }
        } else if visual_count == 1 {
            visuals.push(self.read_fx_visual(s, vslot, elem_type)?);
        }
        let mut trail = None;
        let mut spot = None;
        if let Some(ext) = deref(s, p, l::FxElemDef::extended)? {
            if elem_type == 5 {
                let vert_count = s.i32_at(ext, l::FxTrailDef::vertCount)?.max(0) as usize;
                let ind_count = s.i32_at(ext, l::FxTrailDef::indCount)?.max(0) as usize;
                let mut verts = Vec::with_capacity(vert_count);
                if let Some(vp) = deref(s, ext, l::FxTrailDef::verts)? {
                    for i in 0..vert_count {
                        let q = vp.at(i * 20);
                        verts.push([
                            s.f32_at(q, 0)?,
                            s.f32_at(q, 4)?,
                            s.f32_at(q, 8)?,
                            s.f32_at(q, 12)?,
                            s.f32_at(q, 16)?,
                        ]);
                    }
                }
                let mut inds = Vec::with_capacity(ind_count);
                if let Some(ip) = deref(s, ext, l::FxTrailDef::inds)? {
                    for i in 0..ind_count {
                        inds.push(s.u16_at(ip, i * 2)?);
                    }
                }
                trail = Some(FxTrailRef {
                    scroll_time_msec: s.i32_at(ext, l::FxTrailDef::scrollTimeMsec)?,
                    repeat_dist: s.i32_at(ext, l::FxTrailDef::repeatDist)?,
                    split_dist: s.f32_at(ext, l::FxTrailDef::splitDist)?,
                    verts,
                    inds,
                });
            } else if elem_type == 9 {
                spot = Some([
                    s.f32_at(ext, l::FxSpotLightDef::fovInnerFraction)?,
                    s.f32_at(ext, l::FxSpotLightDef::startRadius)?,
                    s.f32_at(ext, l::FxSpotLightDef::endRadius)?,
                ]);
            }
        }
        Ok(FxElemRef {
            vel_samples,
            vis_samples,
            visuals,
            effect_on_impact: string_field(s, p, l::FxElemDef::effectOnImpact)?,
            effect_on_death: string_field(s, p, l::FxElemDef::effectOnDeath)?,
            effect_emitted: string_field(s, p, l::FxElemDef::effectEmitted)?,
            effect_attached: string_field(s, p, l::FxElemDef::effectAttached)?,
            trail,
            spot,
            spawn_sound: string_field(s, p, l::FxElemDef::spawnSound)?,
            raw,
        })
    }

    fn read_fx_visual(&mut self, s: &ZoneStream<'_>, slot: Ptr, elem_type: u8) -> Result<FxVisualRef> {
        Ok(match elem_type {
            0..=6 => {
                let name = self.asset_name_at(s, slot, AssetType::Material)?;
                if name.is_empty() {
                    FxVisualRef::None
                } else {
                    FxVisualRef::Material(name)
                }
            }
            7 => {
                let name = self.asset_name_at(s, slot, AssetType::XModel)?;
                if name.is_empty() {
                    FxVisualRef::None
                } else {
                    FxVisualRef::Model(name)
                }
            }
            12 => {
                let name = string_field(s, slot, 0)?;
                if name.is_empty() {
                    FxVisualRef::None
                } else {
                    FxVisualRef::Effect(name)
                }
            }
            10 => {
                let name = string_field(s, slot, 0)?;
                if name.is_empty() {
                    FxVisualRef::None
                } else {
                    FxVisualRef::Sound(name)
                }
            }
            9 => FxVisualRef::Light(self.asset_name_at(s, slot, AssetType::LightDef)?),
            _ => FxVisualRef::None,
        })
    }

    pub(crate) fn read_tracer(&mut self, s: &ZoneStream<'_>, t: Ptr) -> Result<TracerRef> {
        let mut colors = [[0.0f32; 4]; 5];
        for (i, c) in colors.iter_mut().enumerate() {
            for (j, v) in c.iter_mut().enumerate() {
                *v = s.f32_at(t, l::TracerDef::colors + (i * 4 + j) * 4)?;
            }
        }
        Ok(TracerRef {
            name: string_field(s, t, l::TracerDef::name)?,
            ty: s.i32_at(t, l::TracerDef::r#type)?,
            material: self.asset_name_at(s, t.at(l::TracerDef::material), AssetType::Material)?,
            draw_interval: s.i32_at(t, l::TracerDef::drawInterval)?,
            speed: s.f32_at(t, l::TracerDef::speed)?,
            beam_length: s.f32_at(t, l::TracerDef::beamLength)?,
            beam_width: s.f32_at(t, l::TracerDef::beamWidth)?,
            screw_radius: s.f32_at(t, l::TracerDef::screwRadius)?,
            screw_dist: s.f32_at(t, l::TracerDef::screwDist)?,
            fade_time: s.f32_at(t, l::TracerDef::fadeTime)?,
            fade_scale: s.f32_at(t, l::TracerDef::fadeScale)?,
            tex_repeat_rate: s.f32_at(t, l::TracerDef::texRepeatRate)?,
            colors,
        })
    }

    pub(crate) fn read_impact_table(&mut self, s: &ZoneStream<'_>, t: Ptr) -> Result<ImpactTableRef> {
        let mut rows = Vec::new();
        if let Some(arr) = deref(s, t, l::FxImpactTable::table)? {
            for i in 0..21 {
                let e = arr.at(i * l::FxImpactEntry::SIZE);
                let mut nonflesh: [String; 32] = Default::default();
                for (j, n) in nonflesh.iter_mut().enumerate() {
                    *n = self.asset_name_at(s, e.at(l::FxImpactEntry::nonflesh + j * 4), AssetType::Fx)?;
                }
                let mut flesh: [String; 4] = Default::default();
                for (j, n) in flesh.iter_mut().enumerate() {
                    *n = self.asset_name_at(s, e.at(l::FxImpactEntry::flesh + j * 4), AssetType::Fx)?;
                }
                rows.push((nonflesh, flesh));
            }
        }
        Ok(ImpactTableRef {
            name: string_field(s, t, l::FxImpactTable::name)?,
            rows,
        })
    }

    pub(crate) fn read_footstep_table(&mut self, s: &ZoneStream<'_>, t: Ptr) -> Result<FootstepTableRef> {
        let mut aliases = Vec::with_capacity(32);
        for surf in 0..32 {
            let mut row = [0u32; 7];
            for (k, a) in row.iter_mut().enumerate() {
                *a = s.u32_at(t, l::FootstepTableDef::sndAliasTable + (surf * 7 + k) * 4)?;
            }
            aliases.push(row);
        }
        Ok(FootstepTableRef {
            name: string_field(s, t, l::FootstepTableDef::name)?,
            aliases,
        })
    }

    pub(crate) fn read_footstep_fx_table(&mut self, s: &ZoneStream<'_>, t: Ptr) -> Result<FootstepFxTableRef> {
        let mut fx = Vec::with_capacity(32);
        for surf in 0..32 {
            fx.push(self.asset_name_at(s, t.at(l::FootstepFXTableDef::footstepFX + surf * 4), AssetType::Fx)?);
        }
        Ok(FootstepFxTableRef {
            name: string_field(s, t, l::FootstepFXTableDef::name)?,
            fx,
        })
    }

    pub(crate) fn read_sound_bank(&mut self, s: &ZoneStream<'_>, b: Ptr) -> Result<SndBankRef> {
        let count = s.u32_at(b, l::SndBank::aliasCount)? as usize;
        let mut aliases = Vec::with_capacity(count);
        if let Some(arr) = deref(s, b, l::SndBank::alias)? {
            for i in 0..count {
                let list = arr.at(i * l::SndAliasList::SIZE);
                let n = s.i32_at(list, l::SndAliasList::count)?.max(0) as usize;
                let mut entries = Vec::with_capacity(n);
                if let Some(head) = deref(s, list, l::SndAliasList::head)? {
                    for j in 0..n {
                        let a = head.at(j * l::SndAlias::SIZE);
                        let u16a = |off: usize| s.u16_at(a, off);
                        let u8a = |off: usize| s.u8_at(a, off);
                        entries.push(SndAliasRef {
                            name: string_field(s, a, l::SndAlias::name)?,
                            id: s.u32_at(a, l::SndAlias::id)?,
                            subtitle: string_field(s, a, l::SndAlias::subtitle)?,
                            secondary: string_field(s, a, l::SndAlias::secondaryName)?,
                            asset_id: s.u32_at(a, l::SndAlias::assetId)?,
                            asset_file: string_field(s, a, l::SndAlias::assetFileName)?,
                            flags: [
                                s.u32_at(a, l::SndAlias::flags)?,
                                s.u32_at(a, l::SndAlias::flags + 4)?,
                            ],
                            duck: s.u32_at(a, l::SndAlias::duck)?,
                            context_type: s.u32_at(a, l::SndAlias::contextType)?,
                            context_value: s.u32_at(a, l::SndAlias::contextValue)?,
                            stop_on_play: s.u32_at(a, l::SndAlias::stopOnPlay)?,
                            futz_patch: s.u32_at(a, l::SndAlias::futzPatch)?,
                            flux_time: u16a(l::SndAlias::fluxTime)?,
                            start_delay: u16a(l::SndAlias::startDelay)?,
                            reverb_send: u16a(l::SndAlias::reverbSend)?,
                            center_send: u16a(l::SndAlias::centerSend)?,
                            vol_min: u16a(l::SndAlias::volMin)?,
                            vol_max: u16a(l::SndAlias::volMax)?,
                            pitch_min: u16a(l::SndAlias::pitchMin)?,
                            pitch_max: u16a(l::SndAlias::pitchMax)?,
                            dist_min: u16a(l::SndAlias::distMin)?,
                            dist_max: u16a(l::SndAlias::distMax)?,
                            dist_reverb_max: u16a(l::SndAlias::distReverbMax)?,
                            envelop_min: u16a(l::SndAlias::envelopMin)?,
                            envelop_max: u16a(l::SndAlias::envelopMax)?,
                            envelop_percentage: u16a(l::SndAlias::envelopPercentage)?,
                            fade_in: u16a(l::SndAlias::fadeIn)?,
                            fade_out: u16a(l::SndAlias::fadeOut)?,
                            doppler_scale: s.i16_at(a, l::SndAlias::dopplerScale)?,
                            min_priority_threshold: u8a(l::SndAlias::minPriorityThreshold)?,
                            max_priority_threshold: u8a(l::SndAlias::maxPriorityThreshold)?,
                            probability: u8a(l::SndAlias::probability)?,
                            occlusion_level: u8a(l::SndAlias::occlusionLevel)?,
                            min_priority: u8a(l::SndAlias::minPriority)?,
                            max_priority: u8a(l::SndAlias::maxPriority)?,
                            pan: u8a(l::SndAlias::pan)?,
                            limit_count: u8a(l::SndAlias::limitCount)?,
                            entity_limit_count: u8a(l::SndAlias::entityLimitCount)?,
                            duck_group: u8a(l::SndAlias::duckGroup)?,
                        });
                    }
                }
                aliases.push(SndAliasListRef {
                    name: string_field(s, list, l::SndAliasList::name)?,
                    id: s.u32_at(list, l::SndAliasList::id)?,
                    sequence: s.i32_at(list, l::SndAliasList::sequence)?,
                    entries,
                });
            }
        }
        Ok(SndBankRef {
            name: string_field(s, b, l::SndBank::name)?,
            aliases,
        })
    }

    pub(crate) fn read_snd_driver_globals(&mut self, s: &ZoneStream<'_>, g: Ptr) -> Result<SndDriverGlobalsRef> {
        let mut out = SndDriverGlobalsRef {
            name: string_field(s, g, l::SndDriverGlobals::name)?,
            ..Default::default()
        };
        let curve_count = s.u32_at(g, l::SndDriverGlobals::curveCount)? as usize;
        if let Some(arr) = deref(s, g, l::SndDriverGlobals::curves)? {
            for i in 0..curve_count {
                let c = arr.at(i * l::SndCurve::SIZE);
                let mut points = [[0.0f32; 2]; 8];
                for (k, pt) in points.iter_mut().enumerate() {
                    pt[0] = s.f32_at(c, l::SndCurve::points + k * 8)?;
                    pt[1] = s.f32_at(c, l::SndCurve::points + k * 8 + 4)?;
                }
                out.curves.push(SndCurveRef {
                    name: fixed_string(s, c, l::SndCurve::name, 32)?,
                    id: s.u32_at(c, l::SndCurve::id)?,
                    points,
                });
            }
        }
        let group_count = s.u32_at(g, l::SndDriverGlobals::groupCount)? as usize;
        if let Some(arr) = deref(s, g, l::SndDriverGlobals::groups)? {
            for i in 0..group_count {
                let c = arr.at(i * l::SndVolumeGroup::SIZE);
                out.groups.push((
                    fixed_string(s, c, l::SndVolumeGroup::name, 32)?,
                    fixed_string(s, c, l::SndVolumeGroup::parentName, 32)?,
                    s.u16_at(c, l::SndVolumeGroup::attenuationSp)?,
                    s.u16_at(c, l::SndVolumeGroup::attenuationMp)?,
                ));
            }
        }
        Ok(out)
    }
}
