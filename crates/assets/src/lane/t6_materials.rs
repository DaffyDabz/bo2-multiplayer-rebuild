//! bo2zm: Black Ops II materials into the engine's material catalog.
//!
//! BO2 materials carry D3D11 techniques the engine cannot run, so a row here
//! is only what the renderer's fallback draw samples: the material's colour
//! map, read from the game's image packs (IWI v27) into a GPU image in its
//! own block format (BC1-3). Rows have no technique set; their surfaces keep
//! drawing through the fallback pass, which samples the colour map.

use std::collections::HashMap;
use std::sync::Arc;

use asset_core::{AssetEdge, AssetNamespace, AssetRef, T6Draw, ZoneOwner};
use asset_material::{
    AuthoredImage, AuthoredMaterial, MaterialCatalog, MaterialTextureBinding, TS_COLOR_MAP,
    TS_NORMAL_MAP, TS_SPECULAR_MAP,
};
use asset_t6::{ImageRef, ImageSource, PackSet, ZoneCapture};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::Image;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};

/// `R_HashString("colorMap")`, the colour map's slot in a texture table.
const COLOR_MAP_HASH: u32 = 0xa0ab_1041;
/// bo2mp: `DiffuseAndGloss`, the sw4 shaders' colour map.
const DIFFUSE_AND_GLOSS_HASH: u32 = 0xd50e_4161;
/// bo2mp: the picture of BO2's laser-scan 2D shader (`sw4_2d_laser_scan`:
/// the big scorestreak icons, `hud_ks_*_big`; its other two images are
/// the scan's noise and wireframe).
const LASER_SCAN_IMAGE_HASH: u32 = 0x079c_6367;

/// bo2mp: the 3-D tile-blend shaders (`sw4_3d_cod7_tile_blend*`, rocks and
/// cliffs on most multiplayer maps) have no `colorMap`: they mix two tiling
/// colour maps (`Micro_1_ColorMap`, `Micro_2_ColorMap`) by a macro map in
/// this slot, whose red, green and blue are blend weights (named `_ao`, or
/// `_c` on the cliffs: Downhill's red and yellow rocks).
const TILE_BLEND_MACRO_HASH: u32 = 0xed0a_4f83;
const TILE_BLEND_MICRO1_HASH: u32 = name_hash("Micro_1_ColorMap");
const TILE_BLEND_MICRO2_HASH: u32 = name_hash("Micro_2_ColorMap");

/// `R_HashString`: case-folded, xor with 33 times the running hash.
const fn name_hash(name: &str) -> u32 {
    let b = name.as_bytes();
    let mut h = 0u32;
    let mut i = 0;
    while i < b.len() {
        h = (b[i] | 0x20) as u32 ^ h.wrapping_mul(33);
        i += 1;
    }
    h
}

/// A layered material's extra colour maps (`colorMap1`, `colorMap2`) ride
/// in the catalog under these private semantics; the fallback draw reads
/// them by semantic. Real T6 semantics stay below 0x20.
pub const T6_LAYER1_SEMANTIC: u8 = 0xf1;
pub const T6_LAYER2_SEMANTIC: u8 = 0xf2;
const LAYER_MAPS: [(u32, u8); 2] = [
    (name_hash("colorMap1"), T6_LAYER1_SEMANTIC),
    (name_hash("colorMap2"), T6_LAYER2_SEMANTIC),
];

/// What linking did: capture material index -> catalog material index.
pub(crate) struct T6Materials {
    pub local: HashMap<usize, usize>,
    pub report: Vec<String>,
}

/// A GPU image over an IWI's own blocks: every mip level, largest first.
/// A cube map (the sky's) is six faces, each with its mips, read linear:
/// the sky shader decodes it as rgb / a in linear space.
pub(crate) fn gpu_image(iwi: &ipak_t6::IwiImage<'_>) -> Result<Image, String> {
    use ipak_t6::IwiFormat as F;
    let cube = iwi.is_cube();
    let format = match (iwi.format, cube) {
        (F::Dxt1, false) => TextureFormat::Bc1RgbaUnormSrgb,
        (F::Dxt3, false) => TextureFormat::Bc2RgbaUnormSrgb,
        (F::Dxt5, false) => TextureFormat::Bc3RgbaUnormSrgb,
        // Normal maps: two channels, read as stored.
        (F::Dxn, false) => TextureFormat::Bc5RgUnorm,
        (F::Dxt1, true) => TextureFormat::Bc1RgbaUnorm,
        (F::Dxt3, true) => TextureFormat::Bc2RgbaUnorm,
        (F::Dxt5, true) => TextureFormat::Bc3RgbaUnorm,
        // Uncompressed: widened to RGBA8 below.
        (F::Rgba8 | F::Rgb8 | F::LuminanceAlpha | F::Luminance | F::Alpha, false) => {
            TextureFormat::Rgba8UnormSrgb
        }
        (other, _) => return Err(format!("{other:?} colour maps are not drawn yet")),
    };
    let widen = |level: &[u8]| -> Vec<u8> {
        match iwi.format {
            F::Rgba8 => level.to_vec(),
            F::Rgb8 => level
                .chunks_exact(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            F::LuminanceAlpha => level
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            F::Luminance => level.iter().flat_map(|&l| [l, l, l, 255]).collect(),
            F::Alpha => level.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
            _ => level.to_vec(),
        }
    };
    if iwi.is_volume() {
        return Err("a volume image".to_owned());
    }
    if iwi.format.is_block_compressed() && (iwi.width % 4 != 0 || iwi.height % 4 != 0) {
        return Err(format!(
            "{}x{} is not whole 4x4 blocks",
            iwi.width, iwi.height
        ));
    }
    // The fast quality setting starts at the second level (half size)
    // where that is still whole blocks; the sky keeps every level.
    let skip = u32::from(
        asset_core::t6_fast() && !cube && iwi.levels > 1 && iwi.width >= 128 && iwi.height >= 128,
    );
    // IWI keeps each level's faces together; the GPU image wants each face
    // with all its levels (layer-major).
    let faces = iwi.faces as usize;
    let mut data = Vec::new();
    for face in 0..faces {
        for level in skip..iwi.levels {
            let all = iwi.level(level);
            let one = all.len() / faces;
            data.extend_from_slice(&widen(&all[face * one..(face + 1) * one]));
        }
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: iwi.width >> skip,
            height: iwi.height >> skip,
            depth_or_array_layers: iwi.faces,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = iwi.levels - skip;
    if cube {
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..Default::default()
        });
    }
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    Ok(image)
}

/// The normal and specular maps BO2's lit shaders read (the shine), by
/// their material slot hashes (`normalMap`, `specularMap`).
/// (`SpecularAndGloss`, 0x8c297e80: a weapon camo's specular colour and
/// gloss, which BO2's camo shader reads where the lit one reads its
/// specular map: Pack-a-Punch's blue and gold sheen.)
const SHINE_MAPS: [(u32, u8); 3] = [
    (0x59d3_0d0f, TS_NORMAL_MAP),
    (0x34ec_ccb3, TS_SPECULAR_MAP),
    (0x8c29_7e80, TS_SPECULAR_MAP),
];

/// The private semantic the reflection probes' cube array is linked under
/// (a material named `t6/reflection_probes` draws nothing).
pub const T6_PROBES_SEMANTIC: u8 = 0xf3;
pub const T6_PROBES_MATERIAL: &str = "t6/reflection_probes";

/// The world's reflection probes as one cube-array image. BO2 keeps each
/// probe's cube in the zone: BC3, 128x128, six faces each with its whole
/// mip chain, the order a GPU cube array is uploaded in. A probe in any
/// other form (Nuketown's probe 0 is a 4x4 stand-in) is black. Linked as
/// the private semantic of a material that draws nothing.
pub(crate) fn link_reflection_probes(
    catalog: &mut MaterialCatalog,
    capture: &ZoneCapture,
    world: &asset_t6::WorldRef,
    zone: &str,
) -> Option<String> {
    const SIZE: u32 = 128;
    const LEVELS: u32 = 8;
    if world.reflection_probes.is_empty() {
        return None;
    }
    let face_bytes: usize = (0..LEVELS)
        .map(|l| {
            let blocks = ((SIZE >> l).max(1) as usize).div_ceil(4);
            blocks * blocks * 16
        })
        .sum();
    let probe_bytes = face_bytes * 6;
    let mut data = Vec::with_capacity(probe_bytes * world.reflection_probes.len());
    let mut real = 0usize;
    for probe in &world.reflection_probes {
        let embedded = probe
            .image
            .and_then(|k| capture.images.get(k.index))
            .and_then(|img| img.embedded.as_ref())
            .filter(|e| matches!(e.dxgi_format, 77 | 78) && e.data.len() == probe_bytes);
        match embedded {
            Some(e) => {
                data.extend_from_slice(&e.data);
                real += 1;
            }
            None => data.resize(data.len() + probe_bytes, 0),
        }
    }
    let layers = 6 * world.reflection_probes.len() as u32;
    let mut image = Image::new_uninit(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: layers,
        },
        TextureDimension::D2,
        TextureFormat::Bc3RgbaUnorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = LEVELS;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::CubeArray),
        ..Default::default()
    });
    image.data = Some(data);
    let slot = catalog.link_image(AuthoredImage {
        namespace: AssetNamespace::T6,
        name: AssetRef::Real(T6_PROBES_MATERIAL.to_owned()),
        map_type: 5,
        semantic: T6_PROBES_SEMANTIC,
        category: 0,
        use_srgb_reads: false,
        width: SIZE as u16,
        height: SIZE as u16,
        depth: 1,
        level_count: LEVELS as u8,
        format: 0,
        payload: Arc::new(Vec::new()),
        decoded: Some(Arc::new(image)),
        common_owned: false,
        decoded_variant: None,
        decoded_by: None,
        pending_decode: None,
    });
    catalog.link_material(AuthoredMaterial {
        name: AssetRef::Real(T6_PROBES_MATERIAL.to_owned()),
        namespace: AssetNamespace::T6,
        technique_set: AssetRef::default(),
        technique_set_edge: AssetEdge::Absent,
        draw_surf: 0,
        sort_key: 0,
        info_game_flags: 0,
        texture_atlas: None,
        surface_type_bits: None,
        t5_layered_surface_types: None,
        state_flags: 0,
        camera_region: 0,
        state_bits: Vec::new(),
        state_bits_entry: None,
        t5_state_bits_entry: None,
        iw5_state_bits_entry: None,
        technique_table: None,
        route: None,
        textures: vec![MaterialTextureBinding {
            name_hash: 0,
            name_start: 0,
            name_end: 0,
            sampler_state: 0,
            semantic: T6_PROBES_SEMANTIC,
            image: Some(slot),
        }],
        constants: Vec::new(),
        zone: ZoneOwner::intern(zone),
        t6_draw: None,
    });
    Some(format!(
        "t6 reflection probes: {} ({real} cube maps, {} stand-ins black), one {SIZE}x{SIZE} cube array, {LEVELS} mips",
        world.reflection_probes.len(),
        world.reflection_probes.len() - real
    ))
}

/// Decode one captured image from the packs.
pub(crate) fn decode(packs: &PackSet, image: &ImageRef) -> Result<Image, String> {
    match packs.locate(image) {
        Some(ImageSource::Pack(i, entry)) => {
            let bytes = packs.packs[i].read(entry)?;
            let iwi = ipak_t6::parse_iwi(&bytes).map_err(|e| e.to_string())?;
            gpu_image(&iwi)
        }
        Some(ImageSource::Embedded) => Err("in-zone colour maps are not drawn yet".to_owned()),
        Some(ImageSource::Empty) => Err("no pixels".to_owned()),
        None => Err("in no pack".to_owned()),
    }
}

/// The texture a material is coloured with: its `colorMap`; else, among its
/// colour-map-semantic textures (layered and ember materials name their
/// slots differently), the one whose image is named as a colour map (`_c`,
/// `_col`), not an ember or glow layer (`_e`); else the first of them.
/// A tile-blend material is coloured by its `<name>_cheap` stand-in's
/// colour map (BO2's own mix of the two tiles, `..._c_cmb`), else by its
/// first tile; never by its macro map.
pub(crate) fn colour_texture<'a>(
    material: &'a asset_t6::MaterialRef,
    capture: &'a ZoneCapture,
) -> Option<&'a asset_t6::MaterialTexture> {
    if material
        .textures
        .iter()
        .any(|t| t.name_hash == TILE_BLEND_MACRO_HASH)
    {
        let cheap = format!("{}_cheap", material.name);
        let mixed = capture
            .materials
            .iter()
            .find(|m| m.name == cheap)
            .and_then(|m| {
                m.textures
                    .iter()
                    .find(|t| t.name_hash == COLOR_MAP_HASH && t.image.is_some())
            });
        let tile = || {
            material
                .textures
                .iter()
                .find(|t| t.name_hash == TILE_BLEND_MICRO1_HASH && t.image.is_some())
        };
        if let Some(t) = mixed.or_else(tile) {
            return Some(t);
        }
    }
    // A colour map is named `_c` / `_col`; the burning-ember materials also
    // carry an animated glow noise named `_c` (`glowcycle_random_01_c`),
    // which sorts first in their table, so it never counts as colour.
    // Masks (`c_gen_arm_rim_mask_c`, 32x32 on the viewmodel arms) are named
    // like colour maps too; the real one is the largest.
    let colour_named = |t: &&asset_t6::MaterialTexture| {
        t.image
            .and_then(|k| capture.images.get(k.index))
            .is_some_and(|img| {
                (img.name.ends_with("_c") || img.name.ends_with("_col"))
                    && !img.name.contains("glowcycle")
                    && !img.name.contains("_mask")
            })
    };
    let area = |t: &&asset_t6::MaterialTexture| {
        t.image
            .and_then(|k| capture.images.get(k.index))
            .map_or(0u32, |img| u32::from(img.width) * u32::from(img.height))
    };
    let semantic = || {
        material
            .textures
            .iter()
            .filter(|t| t.semantic == TS_COLOR_MAP && t.image.is_some())
    };
    // bo2mp: the sw4 shaders' `DiffuseAndGloss` (Dig's tarps: their frayed
    // edge texture is named `_c`, the diffuse is not).
    material
        .textures
        .iter()
        .find(|t| t.name_hash == COLOR_MAP_HASH && t.image.is_some())
        .or_else(|| {
            material
                .textures
                .iter()
                .find(|t| t.name_hash == DIFFUSE_AND_GLOSS_HASH && t.image.is_some())
        })
        .or_else(|| semantic().filter(colour_named).max_by_key(area))
        .or_else(|| semantic().next())
        .or_else(|| {
            material
                .textures
                .iter()
                .find(|t| t.name_hash == LASER_SCAN_IMAGE_HASH && t.image.is_some())
        })
        // bo2mp: a 2D picture's one image (BO2's menu backings,
        // `sw4_2d_tile`: semantic 0, no colour-map name).
        .or_else(|| {
            let mut t = material.textures.iter().filter(|t| t.semantic == 0 && t.image.is_some());
            let first = t.next();
            first.filter(|_| t.next().is_none())
        })
}

/// A 1x1 stand-in for a colour map with no pixels here, by its name: white,
/// black, else mid grey ("grey").
fn flat_image(name: &str) -> Image {
    let n = name.to_ascii_lowercase();
    let v = if n.contains("white") {
        255
    } else if n.contains("black") {
        0
    } else {
        128
    };
    let mut image = Image::new_uninit(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(vec![v, v, v, 255]);
    image
}

/// bo2mp: an f32 as an IEEE half float (round to nearest; tiny values to
/// zero, huge ones to infinity).
fn half_bits(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let mant = b & 0x7f_ffff;
    if v.is_nan() {
        return 0x7e00;
    }
    if exp >= 31 {
        return sign | 0x7c00;
    }
    if exp <= 0 {
        return sign;
    }
    let half = sign | ((exp as u16) << 10) | (mant >> 13) as u16;
    // Round to nearest (a carry into the exponent is still right).
    if mant & 0x1000 != 0 { half + 1 } else { half }
}

/// bo2mp: a water material's constants for the fallback draw's water
/// shader, one RGBA16F texel each (BO2's own `cod7water` / `cod7watershore`
/// shaders, fxc disassembly; the constants by their hashes, as the shaders'
/// register arguments name them):
/// 0..3 the four normal-map layers (world xy scale, scroll per second),
/// 4 normal scale and bias (first map, second map), 5 the layers' weights,
/// 6 highlight powers, fresnel bias and scale, 7 highlight strengths, their
/// overall scale and the kind (0 pool, 1 sea, 2 open sea: no vertex
/// alpha), 8 and 9 the water colour and opacity (pool: grazing and facing;
/// sea: scatter and body, deep),
/// 10 the sea's shadow light.
fn water_table(material: &asset_t6::MaterialRef, kind: u8) -> Vec<[f32; 4]> {
    let c = |hash: u32| {
        material
            .constants
            .iter()
            .find(|c| c.0 == hash)
            .map_or([0.0; 4], |c| c.2)
    };
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    if kind == 0 {
        // cod7water: waterNormalPositionScale, waterLoNormalScroll,
        // waterHiNormalScroll, waterNormalScrollSpeed (vertex shader:
        // uv = pos.xy * scale + gameTime * scroll * speed), waterNormalScale,
        // waterControlVar0/1, waterColorN, waterColorS.
        let scale = c(0xc728_b4a7);
        let lo = c(0x6e97_1968);
        let hi = c(0x01a2_0bea);
        let speed = c(0x134c_d3cc);
        let control1 = c(0xa7d6_56a6);
        vec![
            [scale[0], scale[0], lo[0] * speed[0], lo[1] * speed[0]],
            [scale[1], scale[1], lo[2] * speed[1], lo[3] * speed[1]],
            [scale[2], scale[2], hi[0] * speed[2], hi[1] * speed[2]],
            [scale[3], scale[3], hi[2] * speed[3], hi[3] * speed[3]],
            c(0x8632_8e1e),
            [1.0; 4],
            c(0xa7d6_56a7),
            [control1[0], control1[1], control1[2], 0.0],
            c(0xbd74_8e86),
            c(0xbd74_8e9b),
            [0.0; 4],
        ]
    } else {
        // cod7watershore: normalUVscroll_0123, normalUVcontrol0/2/3 (uv =
        // pos.xy * control.xy + gameTime * scroll * control.zw; layer 1 is
        // unused), normalScaleLoHi, controlVar0/1, the deep colours
        // (shallow + deep delta: the control map's depth reads 1 on open
        // water), waterOpacityControl.w, customLightmapShadowColor.
        let scroll = c(0x95ba_cba2);
        let c0 = c(0x9ea1_a767);
        let c2 = c(0x9ea1_a765);
        let c3 = c(0x9ea1_a764);
        let control1 = c(0x483f_c933);
        // The open-sea variants (Carrier, Takeoff, Frostbite's distant sea)
        // have no opacity control: they draw opaque and never read the
        // vertex colour (kind 2; their shader refracts nothing).
        let opacity_control = material.constants.iter().find(|c| c.0 == 0x8898_e1ab);
        let opacity = opacity_control.map_or(1.0, |c| c.2[3]);
        let kind = if opacity_control.is_some() { 1.0 } else { 2.0 };
        let mut scatter = add(c(0xe887_d68c), c(0xeef4_77a2));
        let mut body = add(c(0x7429_46aa), c(0x9092_abc4));
        scatter[3] = opacity;
        body[3] = opacity;
        vec![
            [c0[0], c0[1], scroll[0] * c0[2], scroll[0] * c0[3]],
            [0.0; 4],
            [c2[0], c2[1], scroll[2] * c2[2], scroll[2] * c2[3]],
            [c3[0], c3[1], scroll[3] * c3[2], scroll[3] * c3[3]],
            c(0xb47c_f4c9),
            [1.0, 0.0, 1.0, 1.0],
            c(0x483f_c932),
            [control1[0], control1[1], 0.3184, kind],
            scatter,
            body,
            c(0xc9e7_ae4a),
        ]
    }
}

/// bo2mp: an emissive flow material's (molten lava) extra pictures, from
/// BO2's `cod7emissiveflow` shaders (fxc disassembly; textures and
/// constants by the hashes its register arguments name): its masks packed
/// in one RGBA8 picture (emissiveMapHi alpha, emissiveMapLo alpha,
/// noiseMap green, noiseMap alpha; resampled to the high mask's size), and
/// its colour remap with a last row of constants (RGBA16F): diffuse uv
/// control, emissive uv scale, emissive uv scroll, noise uv control,
/// diffuse control, emissive remap uv, diffuse filter, emissive filter.
fn flow_images(
    material: &asset_t6::MaterialRef,
    decoded: &dyn Fn(u32) -> Option<(usize, usize, Vec<u8>)>,
) -> Option<(Image, Image)> {
    let (w, h, hi) = decoded(0xd1bf_3646)?;
    let lo = decoded(0xd1bf_35c4)?;
    let noise = decoded(0x7350_2222)?;
    let at = |p: &(usize, usize, Vec<u8>), x: usize, y: usize, c: usize| {
        let (pw, ph, px) = p;
        px[((y * ph / h) * pw + x * pw / w) * 4 + c]
    };
    let mut mask = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            mask.extend_from_slice(&[
                hi[(y * w + x) * 4 + 3],
                at(&lo, x, y, 3),
                at(&noise, x, y, 1),
                at(&noise, x, y, 3),
            ]);
        }
    }
    let mask = Image::new(
        Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        mask,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    let (rw, rh, remap) = decoded(0xd0f6_1c30)?;
    if rw < 8 {
        return None;
    }
    let c = |hash: u32| {
        material
            .constants
            .iter()
            .find(|c| c.0 == hash)
            .map_or([0.0; 4], |c| c.2)
    };
    let constants = [
        c(0xb7a9_a24a),
        c(0xe75d_ae00),
        c(0xd31b_b535),
        c(0x2e12_415a),
        c(0x557d_6789),
        c(0xe6ed_ea93),
        c(0xd4f6_818e),
        c(0x1d90_b95b),
    ];
    let mut rows: Vec<[f32; 4]> = remap
        .chunks_exact(4)
        .map(|p| [0, 1, 2, 3].map(|k| f32::from(p[k]) / 255.0))
        .collect();
    let mut last = vec![[0.0f32; 4]; rw];
    last[..constants.len()].copy_from_slice(&constants);
    rows.extend(last);
    let mut table = water_image(&rows);
    table.texture_descriptor.size = Extent3d {
        width: rw as u32,
        height: rh as u32 + 1,
        depth_or_array_layers: 1,
    };
    Some((mask, table))
}

/// bo2mp: an emissive tile material's (Magma's lava rocks) glow as one
/// RGBA16F picture, from BO2's `sw4_3d_cod7_emissive_tile` shaders (fxc
/// disassembly): (emissive A + emissive B at Micro_Scale) * A's alpha *
/// hdrAmount, before the shader squares it; B averaged over the texels one
/// A texel covers. Then its constants (Micro_Scale, Micro_Height,
/// Macro_Height) as a 1x1 RGBA16F picture.
fn tile_images(
    material: &asset_t6::MaterialRef,
    decoded: &dyn Fn(u32) -> Option<(usize, usize, Vec<u8>)>,
) -> Option<(Image, Image)> {
    let (w, h, a) = decoded(0x7c73_d0d6)?;
    let (bw, bh, b) = decoded(0xa4af_0cde)?;
    let c = |hash: u32| {
        material
            .constants
            .iter()
            .find(|c| c.0 == hash)
            .map_or([0.0; 4], |c| c.2)
    };
    let hdr = c(0x00e2_62b2)[0];
    let micro = c(0x00ef_311d);
    let (sx, sy) = (micro[0].max(0.0), micro[1].max(0.0));
    let mut rows = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            let ta = [0, 1, 2, 3].map(|k| f32::from(a[at + k]) / 255.0);
            // B's texels under this A texel (micro scale times), wrapped.
            let span = |p: usize, s: f32, n: usize, bn: usize| {
                let lo = (p as f32 * s * bn as f32 / n as f32).floor() as i64;
                let hi = (((p + 1) as f32 * s * bn as f32 / n as f32).ceil() as i64).max(lo + 1);
                (lo, hi)
            };
            let (x0, x1) = span(x, sx, w, bw);
            let (y0, y1) = span(y, sy, h, bh);
            let mut sum = [0.0f32; 3];
            let mut n = 0.0f32;
            for by in y0..y1 {
                for bx in x0..x1 {
                    let px = bx.rem_euclid(bw as i64) as usize;
                    let py = by.rem_euclid(bh as i64) as usize;
                    let o = (py * bw + px) * 4;
                    for k in 0..3 {
                        sum[k] += f32::from(b[o + k]) / 255.0;
                    }
                    n += 1.0;
                }
            }
            let glow = [0, 1, 2].map(|k| (ta[k] + sum[k] / n.max(1.0)) * ta[3] * hdr);
            rows.push([glow[0], glow[1], glow[2], 1.0]);
        }
    }
    let mut glow = water_image(&rows);
    glow.texture_descriptor.size = Extent3d {
        width: w as u32,
        height: h as u32,
        depth_or_array_layers: 1,
    };
    let consts = water_image(&[[sx, sy, c(0x0e10_685a)[0], c(0x1f32_ca52)[0]]]);
    Some((glow, consts))
}

/// bo2mp: the water table as an RGBA16F picture (read by texel).
fn water_image(rows: &[[f32; 4]]) -> Image {
    let data: Vec<u8> = rows
        .iter()
        .flatten()
        .flat_map(|&v| half_bits(v).to_le_bytes())
        .collect();
    let mut image = Image::new_uninit(
        Extent3d {
            width: rows.len() as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image
}

/// An objective material's two colours as a 2x1 sRGB picture (linear
/// colours in, so a sampled texel gives them back).
fn objective_image(lo: [f32; 4], hi: [f32; 4]) -> Image {
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    let data = vec![
        enc(lo[0]),
        enc(lo[1]),
        enc(lo[2]),
        255,
        enc(hi[0]),
        enc(hi[1]),
        enc(hi[2]),
        255,
    ];
    let mut image = Image::new_uninit(
        Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image
}

/// Link the wanted capture materials into `catalog`, each with its colour
/// map decoded.
pub(crate) fn link_materials(
    catalog: &mut MaterialCatalog,
    capture: &ZoneCapture,
    packs: Option<&PackSet>,
    wanted: impl IntoIterator<Item = usize>,
    zone: &str,
) -> T6Materials {
    link_materials_with(catalog, capture, packs, wanted, zone, None)
}

/// `link_materials`, with the real images of other zones by name: a
/// comma-named image is a reference to one another zone defines.
pub(crate) fn link_materials_with(
    catalog: &mut MaterialCatalog,
    capture: &ZoneCapture,
    packs: Option<&PackSet>,
    wanted: impl IntoIterator<Item = usize>,
    zone: &str,
    foreign_images: Option<&HashMap<String, ImageRef>>,
) -> T6Materials {
    let owner = ZoneOwner::intern(zone);
    let mut images: HashMap<usize, Option<usize>> = HashMap::new();
    let mut local = HashMap::new();
    let (mut coloured, mut uncoloured, mut decoded) = (0usize, 0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();
    let mut draws: HashMap<T6Draw, usize> = HashMap::new();
    for mi in wanted {
        if local.contains_key(&mi) {
            continue;
        }
        let Some(material) = capture.materials.get(mi) else {
            continue;
        };
        let technique_set = material
            .technique_set
            .and_then(|k| capture.technique_sets.get(k.index))
            .map_or("", |t| t.name.as_str());
        // The main technique's draw state: lit with sun shadow, lit with
        // sun, lit, emissive, then unlit (the first the material draws in).
        let state = [6usize, 5, 4, 3, 2].iter().find_map(|&t| {
            let entry = *material.state_bits_entry.get(t)?;
            material.state_bits.get(usize::from(entry)).copied()
        });
        let mut draw = T6Draw::from_technique_set(technique_set, material.sort_key, state);
        // bo2mp test aid: `IW4L_T6_HIDE_MAT=a,b` leaves out every material
        // whose name contains one of them (bisecting a stray surface).
        if let Ok(hide) = std::env::var("IW4L_T6_HIDE_MAT")
            && hide
                .split(',')
                .any(|h| !h.is_empty() && material.name.contains(h))
        {
            draw.blend = asset_core::T6Blend::ShadowOnly;
        }
        // bo2mp: a material whose surface flags say "only cast shadow"
        // (0x80000, the `onlycastshadow` infoParm: `wpc/shadowcaster` and
        // trees' shadow cards like Raid's `mtl_ctl_tree_shadow_caster`)
        // never draws in colour.
        if material.surface_flags & 0x8_0000 != 0 {
            draw.blend = asset_core::T6Blend::ShadowOnly;
        }
        // Unlit shaders scale their colour by the material's `scaleRGB`
        // (the light dome's is 32); powers of two up to 128 are kept.
        // bo2mp: the `sw4_3d_unlit` shaders (screens, backlit glass) scale
        // the texel by `hdrAmount` before squaring it into linear (fxc
        // disassembly: (rgb * a * hdrAmount)^2), so by its square.
        if draw.unlit {
            let sw4 = technique_set.split('_').any(|t| t == "sw4")
                && technique_set.split('_').any(|t| t == "unlit");
            let hdr_amount = material
                .constants
                .iter()
                .find(|c| c.1 == "hdrAmount")
                .map(|c| c.2[0] * c.2[0])
                .filter(|_| sw4);
            let scale = material
                .constants
                .iter()
                .find(|c| c.1.starts_with("scaleRGB"))
                .map(|c| c.2[0])
                .or(hdr_amount)
                .unwrap_or(1.0);
            draw.unlit_scale_exp = scale.max(1.0).log2().round().clamp(0.0, 7.0) as u8;
        }
        *draws
            .entry(T6Draw {
                sort_key: 0,
                polygon_offset: 0,
                unlit_scale_exp: 0,
                layers: [0, 0],
                cull: asset_core::T6Cull::None,
                ..draw
            })
            .or_default() += 1;
        let mut textures = Vec::new();
        let mut link = |key: asset_t6::AssetKey,
                        catalog: &mut MaterialCatalog,
                        failures: &mut Vec<String>,
                        decoded: &mut usize|
         -> Option<_> {
            *images.entry(key.index).or_insert_with(|| {
                let mut image = capture.images.get(key.index)?;
                if let Some(real) = image
                    .name
                    .strip_prefix(',')
                    .and_then(|name| foreign_images?.get(name))
                {
                    image = real;
                }
                let decoded_image = match packs.map(|p| decode(p, image)) {
                    Some(Ok(gpu)) => {
                        *decoded += 1;
                        Some(Arc::new(gpu))
                    }
                    Some(Err(e)) => {
                        failures.push(format!("{}: {e}", image.name));
                        None
                    }
                    None => None,
                };
                let decoded_image = decoded_image?;
                Some(catalog.link_image(AuthoredImage {
                    namespace: AssetNamespace::T6,
                    name: AssetRef::Real(image.name.clone()),
                    map_type: image.map_type,
                    semantic: TS_COLOR_MAP,
                    category: 0,
                    use_srgb_reads: true,
                    width: image.width,
                    height: image.height,
                    depth: image.depth,
                    level_count: image.level_count,
                    format: 0,
                    payload: Arc::new(Vec::new()),
                    decoded: Some(decoded_image),
                    common_owned: false,
                    decoded_variant: None,
                    decoded_by: None,
                    pending_decode: None,
                }))
            })
        };
        // bo2mp: a water material draws from its constants (a picture of
        // them in the colour map's place) and its two normal maps (in the
        // normal and specular maps' places).
        let water = asset_core::t6_water_kind(technique_set).filter(|_| draw.water);
        if let Some(kind) = water {
            let slot = catalog.link_image(AuthoredImage {
                namespace: AssetNamespace::T6,
                name: AssetRef::Real(format!("{}#water", material.name)),
                map_type: 0,
                semantic: TS_COLOR_MAP,
                category: 0,
                use_srgb_reads: false,
                width: 11,
                height: 1,
                depth: 1,
                level_count: 1,
                format: 0,
                payload: Arc::new(Vec::new()),
                decoded: Some(Arc::new(water_image(&water_table(material, kind)))),
                common_owned: false,
                decoded_variant: None,
                decoded_by: None,
                pending_decode: None,
            });
            textures.push(MaterialTextureBinding {
                name_hash: COLOR_MAP_HASH,
                name_start: 0,
                name_end: 0,
                sampler_state: 0x14,
                semantic: TS_COLOR_MAP,
                image: Some(slot),
            });
            // Pool: normalMap00, normalMap01; sea: normalMapLo, normalMapHi.
            let maps = if kind == 0 {
                [0x1aca_8a8f, 0x1aca_8a8e]
            } else {
                [0x1aca_7e8c, 0x1aca_7f0e]
            };
            for (hash, semantic) in maps.into_iter().zip([TS_NORMAL_MAP, TS_SPECULAR_MAP]) {
                if let Some(texture) = material.textures.iter().find(|t| t.name_hash == hash)
                    && let Some(key) = texture.image
                    && let Some(slot) = link(key, catalog, &mut failures, &mut decoded)
                {
                    textures.push(MaterialTextureBinding {
                        name_hash: texture.name_hash,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: texture.sampler_state,
                        semantic,
                        image: Some(slot),
                    });
                }
            }
        } else if draw.objective {
            // bo2zm M3: an objective material (the box's question marks)
            // draws its two pulse colours, not its (black) colour map: they
            // go in a 2x1 picture its shader reads (texel 0 colorObjMin, 1
            // colorObjMax).
            let colour = |name: &str| {
                material
                    .constants
                    .iter()
                    .find(|c| c.1 == name)
                    .map_or([0.0; 4], |c| c.2)
            };
            let image = objective_image(colour("colorObjMin"), colour("colorObjMax"));
            let slot = catalog.link_image(AuthoredImage {
                namespace: AssetNamespace::T6,
                name: AssetRef::Real(format!("{}#objective", material.name)),
                map_type: 0,
                semantic: TS_COLOR_MAP,
                category: 0,
                use_srgb_reads: true,
                width: 2,
                height: 1,
                depth: 1,
                level_count: 1,
                format: 0,
                payload: Arc::new(Vec::new()),
                decoded: Some(Arc::new(image)),
                common_owned: false,
                decoded_variant: None,
                decoded_by: None,
                pending_decode: None,
            });
            textures.push(MaterialTextureBinding {
                name_hash: 0xa0ab_1041,
                name_start: 0,
                name_end: 0,
                sampler_state: 0x14,
                semantic: TS_COLOR_MAP,
                image: Some(slot),
            });
        } else if let Some(texture) = colour_texture(material, capture)
            && let Some(key) = texture.image
        {
            // bo2zm M3 fix list 1: a colour map whose pixels are in no pack
            // here (a reference into another zone, e.g. the leftover Maya
            // `lambert1` material's "grey") draws flat, as its name says,
            // not in the untextured diagnostic colours (his "weird
            // splotches ... of random colors").
            let slot = link(key, catalog, &mut failures, &mut decoded).unwrap_or_else(|| {
                let name = capture
                    .images
                    .get(key.index)
                    .map_or("", |i| i.name.as_str());
                catalog.link_image(AuthoredImage {
                    namespace: AssetNamespace::T6,
                    name: AssetRef::Real(format!("{name}#flat")),
                    map_type: 0,
                    semantic: TS_COLOR_MAP,
                    category: 0,
                    use_srgb_reads: true,
                    width: 1,
                    height: 1,
                    depth: 1,
                    level_count: 1,
                    format: 0,
                    payload: Arc::new(Vec::new()),
                    decoded: Some(Arc::new(flat_image(name))),
                    common_owned: false,
                    decoded_variant: None,
                    decoded_by: None,
                    pending_decode: None,
                })
            });
            textures.push(MaterialTextureBinding {
                name_hash: texture.name_hash,
                name_start: 0,
                name_end: 0,
                sampler_state: texture.sampler_state,
                semantic: TS_COLOR_MAP,
                image: Some(slot),
            });
        }
        // A layered material's extra colour maps (the fast quality setting
        // draws the base layer alone).
        if asset_core::t6_fast() {
            draw.layers = [0, 0];
        }
        if draw.layers.iter().any(|&k| k != 0) {
            for (hash, semantic) in LAYER_MAPS {
                if let Some(texture) = material.textures.iter().find(|t| t.name_hash == hash)
                    && let Some(key) = texture.image
                    && let Some(slot) = link(key, catalog, &mut failures, &mut decoded)
                {
                    textures.push(MaterialTextureBinding {
                        name_hash: texture.name_hash,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: texture.sampler_state,
                        semantic,
                        image: Some(slot),
                    });
                }
            }
        }
        // The normal and specular maps BO2's lit shaders read (the shine);
        // the fast quality setting draws without them.
        // (`IW4L_T6_NO_SHINE` leaves them out: a test aid for comparing.)
        if !asset_core::t6_fast()
            && !draw.unlit
            && water.is_none()
            && std::env::var_os("IW4L_T6_NO_SHINE").is_none()
        {
            for (hash, semantic) in SHINE_MAPS {
                if let Some(texture) = material.textures.iter().find(|t| t.name_hash == hash)
                    && let Some(key) = texture.image
                    && let Some(slot) = link(key, catalog, &mut failures, &mut decoded)
                {
                    textures.push(MaterialTextureBinding {
                        name_hash: texture.name_hash,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: texture.sampler_state,
                        semantic,
                        image: Some(slot),
                    });
                }
            }
        }
        // bo2mp: an emissive flow material's masks (in the specular map's
        // place) and colour remap with constants (in layer 1's).
        // (`IW4L_T6_NO_FLOW` leaves them out: a test aid for comparing.)
        // bo2mp: an emissive tile model material's glow (in the specular
        // map's place), macro normal map and constants (in layer 1's).
        if draw.tile && !technique_set.starts_with("wpc_") {
            let decoded_rgba = |hash: u32| {
                let key = material
                    .textures
                    .iter()
                    .find(|t| t.name_hash == hash)?
                    .image?;
                let image = capture.images.get(key.index)?;
                let gpu = decode(packs?, image).ok()?;
                top_level_rgba8(&gpu)
            };
            if let Some((glow, consts)) = tile_images(material, &decoded_rgba) {
                for (image, suffix, semantic) in
                    [(glow, "glow", TS_SPECULAR_MAP), (consts, "tile", 0xf1u8)]
                {
                    let size = image.texture_descriptor.size;
                    let slot = catalog.link_image(AuthoredImage {
                        namespace: AssetNamespace::T6,
                        name: AssetRef::Real(format!("{}#tile_{suffix}", material.name)),
                        map_type: 0,
                        semantic,
                        category: 0,
                        use_srgb_reads: false,
                        width: size.width as u16,
                        height: size.height as u16,
                        depth: 1,
                        level_count: 1,
                        format: 0,
                        payload: Arc::new(Vec::new()),
                        decoded: Some(Arc::new(image)),
                        common_owned: false,
                        decoded_variant: None,
                        decoded_by: None,
                        pending_decode: None,
                    });
                    textures.push(MaterialTextureBinding {
                        name_hash: 0,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: 0x14,
                        semantic,
                        image: Some(slot),
                    });
                }
                if let Some(texture) = material
                    .textures
                    .iter()
                    .find(|t| t.name_hash == 0x13a7_8442)
                    && let Some(key) = texture.image
                    && let Some(slot) = link(key, catalog, &mut failures, &mut decoded)
                {
                    textures.push(MaterialTextureBinding {
                        name_hash: texture.name_hash,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: texture.sampler_state,
                        semantic: TS_NORMAL_MAP,
                        image: Some(slot),
                    });
                }
            } else {
                draw.tile = false;
            }
        } else {
            draw.tile = false;
        }
        // bo2mp: a tile blend model material (cliffs, rocks, vista mountains):
        // Micro_1 as its colour map, Micro_2 in the specular map's place, the
        // macro AO/mix map in the normal map's, and Micro_1_Scale,
        // Micro_2_Scale, AO_Diffuse_Adj, EdgeHighlight (2x1 RGBA16F) in layer
        // 1's. World tile blends keep the stand-in colour map.
        if draw.tile_blend && !technique_set.starts_with("wpc_") {
            let mut slots = [None; 3];
            for (slot, hash) in slots.iter_mut().zip([
                TILE_BLEND_MICRO1_HASH,
                TILE_BLEND_MICRO2_HASH,
                TILE_BLEND_MACRO_HASH,
            ]) {
                *slot = material
                    .textures
                    .iter()
                    .find(|t| t.name_hash == hash)
                    .and_then(|t| t.image)
                    .and_then(|key| link(key, catalog, &mut failures, &mut decoded));
            }
            let [micro1, micro2, macro_map] = slots;
            if let (Some(micro1), Some(micro2), Some(macro_map)) = (micro1, micro2, macro_map) {
                let c = |hash: u32, default: [f32; 4]| {
                    material
                        .constants
                        .iter()
                        .find(|c| c.0 == hash)
                        .map_or(default, |c| c.2)
                };
                let s1 = c(0x1818_f373, [1.0; 4]);
                let s2 = c(0x7d8c_def0, [1.0; 4]);
                let ao = c(0x9923_36af, [0.0; 4])[0];
                let edge = c(0x259b_0793, [1.0; 4])[0];
                let consts = water_image(&[[s1[0], s1[1], s2[0], s2[1]], [ao, edge, 0.0, 0.0]]);
                let consts = catalog.link_image(AuthoredImage {
                    namespace: AssetNamespace::T6,
                    name: AssetRef::Real(format!("{}#tile_blend", material.name)),
                    map_type: 0,
                    semantic: 0xf1,
                    category: 0,
                    use_srgb_reads: false,
                    width: 2,
                    height: 1,
                    depth: 1,
                    level_count: 1,
                    format: 0,
                    payload: Arc::new(Vec::new()),
                    decoded: Some(Arc::new(consts)),
                    common_owned: false,
                    decoded_variant: None,
                    decoded_by: None,
                    pending_decode: None,
                });
                textures.retain(|t| t.semantic != TS_COLOR_MAP);
                for (image, semantic) in [
                    (micro1, TS_COLOR_MAP),
                    (micro2, TS_SPECULAR_MAP),
                    (macro_map, TS_NORMAL_MAP),
                    (consts, 0xf1u8),
                ] {
                    textures.push(MaterialTextureBinding {
                        name_hash: if semantic == TS_COLOR_MAP {
                            COLOR_MAP_HASH
                        } else {
                            0
                        },
                        name_start: 0,
                        name_end: 0,
                        sampler_state: 0x14,
                        semantic,
                        image: Some(image),
                    });
                }
            } else {
                draw.tile_blend = false;
            }
        } else {
            draw.tile_blend = false;
        }
        // bo2mp: a tattered flag's frayed edge texture (in the specular map's
        // place) and its EdgeScale (a 1x1 picture in layer 1's place).
        if draw.flag {
            let edge = material
                .textures
                .iter()
                .find(|t| t.name_hash == 0xcdab_26c0)
                .and_then(|t| t.image)
                .and_then(|key| link(key, catalog, &mut failures, &mut decoded));
            if let Some(slot) = edge {
                let scale = material
                    .constants
                    .iter()
                    .find(|c| c.0 == 0x8240_aa7b)
                    .map_or([1.0; 4], |c| c.2);
                let consts = water_image(&[[scale[0], scale[1], 0.0, 0.0]]);
                let consts = catalog.link_image(AuthoredImage {
                    namespace: AssetNamespace::T6,
                    name: AssetRef::Real(format!("{}#flag", material.name)),
                    map_type: 0,
                    semantic: 0xf1,
                    category: 0,
                    use_srgb_reads: false,
                    width: 1,
                    height: 1,
                    depth: 1,
                    level_count: 1,
                    format: 0,
                    payload: Arc::new(Vec::new()),
                    decoded: Some(Arc::new(consts)),
                    common_owned: false,
                    decoded_variant: None,
                    decoded_by: None,
                    pending_decode: None,
                });
                for (image, semantic) in [(slot, TS_SPECULAR_MAP), (consts, 0xf1u8)] {
                    textures.push(MaterialTextureBinding {
                        name_hash: 0,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: 0x14,
                        semantic,
                        image: Some(image),
                    });
                }
            } else {
                draw.flag = false;
            }
        }
        if draw.flow && std::env::var_os("IW4L_T6_NO_FLOW").is_none() {
            let decoded_rgba = |hash: u32| {
                let key = material
                    .textures
                    .iter()
                    .find(|t| t.name_hash == hash)?
                    .image?;
                let image = capture.images.get(key.index)?;
                let gpu = decode(packs?, image).ok()?;
                top_level_rgba8(&gpu)
            };
            if let Some((mask, table)) = flow_images(material, &decoded_rgba) {
                for (image, suffix, semantic) in
                    [(mask, "mask", TS_SPECULAR_MAP), (table, "remap", 0xf1u8)]
                {
                    let size = image.texture_descriptor.size;
                    let slot = catalog.link_image(AuthoredImage {
                        namespace: AssetNamespace::T6,
                        name: AssetRef::Real(format!("{}#flow_{suffix}", material.name)),
                        map_type: 0,
                        semantic,
                        category: 0,
                        use_srgb_reads: false,
                        width: size.width as u16,
                        height: size.height as u16,
                        depth: 1,
                        level_count: 1,
                        format: 0,
                        payload: Arc::new(Vec::new()),
                        decoded: Some(Arc::new(image)),
                        common_owned: false,
                        decoded_variant: None,
                        decoded_by: None,
                        pending_decode: None,
                    });
                    textures.push(MaterialTextureBinding {
                        name_hash: 0,
                        name_start: 0,
                        name_end: 0,
                        sampler_state: 0x14,
                        semantic,
                        image: Some(slot),
                    });
                }
            } else {
                draw.flow = false;
            }
        } else {
            draw.flow = false;
        }
        if textures.is_empty() {
            uncoloured += 1;
        } else {
            coloured += 1;
        }
        let index = catalog.link_material(AuthoredMaterial {
            name: AssetRef::Real(material.name.clone()),
            namespace: AssetNamespace::T6,
            technique_set: AssetRef::default(),
            technique_set_edge: AssetEdge::Absent,
            draw_surf: 0,
            sort_key: material.sort_key,
            info_game_flags: 0,
            texture_atlas: None,
            surface_type_bits: None,
            t5_layered_surface_types: None,
            state_flags: 0,
            camera_region: 0,
            state_bits: Vec::new(),
            state_bits_entry: None,
            t5_state_bits_entry: None,
            iw5_state_bits_entry: None,
            technique_table: None,
            route: None,
            textures,
            constants: material
                .constants
                .iter()
                .map(|(hash, name, literal)| {
                    let mut short = [0u8; 12];
                    for (d, b) in short.iter_mut().zip(name.bytes()) {
                        *d = b;
                    }
                    asset_material::MaterialConstant {
                        name_hash: *hash,
                        name: short,
                        literal: *literal,
                    }
                })
                .collect(),
            zone: owner,
            t6_draw: Some(draw),
        });
        local.insert(mi, index);
    }
    let mut report = vec![format!(
        "t6 materials: {} linked ({coloured} with a colour map, {uncoloured} without), {decoded} colour maps decoded from packs, {} failed; draws {:?}",
        local.len(),
        failures.len(),
        draws
    )];
    for f in failures.iter().take(8) {
        report.push(format!("t6 colour map gap: {f}"));
    }
    T6Materials { local, report }
}

/// bo2zm M4: whether a material adds its colour onto what is under it
/// (blend ONE/ONE, or SRC_ALPHA/ONE) - BO2's menu brackets and glows,
/// black where nothing shows.
pub(crate) fn is_additive(material: &asset_t6::MaterialRef) -> bool {
    (0..36).filter_map(|t| material.draw_state(t)).any(|d| d.dst_blend == 2 && matches!(d.src_blend, 2 | 5))
}

/// bo2mp vehicle screens: a 2D material's AddMap picture (texture name
/// hash 0xe98a255d, the `sw4_2d_color_add` pixel shader's `AddMap`, t0),
/// when it has one: the VTOL Warship's reticles (`mp_hud_sentry_outer_white`
/// = `mp_hud_sentry_outer_blend` colour map + `mp_hud_sentry_outer_add`).
pub(crate) fn add_map<'a>(material: &asset_t6::MaterialRef, capture: &'a ZoneCapture) -> Option<&'a ImageRef> {
    let t = material.textures.iter().find(|t| t.name_hash == ADD_MAP_HASH)?;
    capture.images.get(t.image?.index)
}

const ADD_MAP_HASH: u32 = 0xe98a_255d;

/// bo2mp vehicle screens: BO2's `sw4_2d_color_add` pixel shader reads only
/// the colour map's alpha (`rgb = (ColorMap.a * colour.rgb + AddMap.rgb) *
/// colour.a`, blend ONE/ONE). Packed: (AddMap colour, colour map alpha) for
/// the additive pass, and a white-lit flat copy (colour map alpha plus the
/// AddMap's colour as brightness-alpha) for layers that only alpha-blend.
pub(crate) fn pack_color_add(color: &Image, added: &Image) -> Option<(Image, Image)> {
    let (w, h, mut rgba) = top_level_rgba8(color)?;
    let (aw, ah, add) = top_level_rgba8(added)?;
    let mut flat = rgba.clone();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            // (The AddMap read at the same place, nearest texel.)
            let j = ((y * ah / h) * aw + x * aw / w) * 4;
            let a = rgba[i + 3];
            for c in 0..3 {
                rgba[i + c] = add[j + c];
                flat[i + c] = a.saturating_add(add[j + c]);
            }
            flat[i + 3] = 255;
        }
    }
    let packed = rgba8_image(color, w, h, rgba);
    let flat = additive_to_alpha(&rgba8_image(color, w, h, flat))?;
    Some((packed, flat))
}

/// bo2zm M4: an additive picture for a 2D layer that only alpha-blends: its
/// top level as RGBA with the brightness as alpha (black = see-through),
/// colour divided back out.
pub(crate) fn additive_to_alpha(image: &Image) -> Option<Image> {
    let (w, h, mut rgba) = top_level_rgba8(image)?;
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[0].max(px[1]).max(px[2]));
        let alpha = a * u32::from(px[3]) / 255;
        if a > 0 {
            for c in &mut px[..3] {
                *c = (u32::from(*c) * 255 / a).min(255) as u8;
            }
        }
        px[3] = alpha as u8;
    }
    Some(rgba8_image(image, w, h, rgba))
}

/// bo2mp: a picture's top level as plain RGBA8 (the minimap reads its
/// pixels to turn it).
pub(crate) fn to_rgba8(image: &Image) -> Option<Image> {
    let (w, h, rgba) = top_level_rgba8(image)?;
    Some(rgba8_image(image, w, h, rgba))
}

fn rgba8_image(like: &Image, w: usize, h: usize, rgba: Vec<u8>) -> Image {
    let mut out = Image::new(
        Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.sampler = like.sampler.clone();
    out
}

/// A decoded picture's top level as RGBA8 bytes (block formats decoded, the
/// bytes as stored: no sRGB conversion): width, height, pixels.
fn top_level_rgba8(image: &Image) -> Option<(usize, usize, Vec<u8>)> {
    let w = image.texture_descriptor.size.width as usize;
    let h = image.texture_descriptor.size.height as usize;
    let data = image.data.as_ref()?;
    let (block, decode): (usize, fn(&[u8], &mut [u8], usize)) = match image.texture_descriptor.format {
        TextureFormat::Bc1RgbaUnormSrgb | TextureFormat::Bc1RgbaUnorm => (8, bcdec_rs::bc1),
        TextureFormat::Bc2RgbaUnormSrgb | TextureFormat::Bc2RgbaUnorm => (16, bcdec_rs::bc2),
        TextureFormat::Bc3RgbaUnormSrgb | TextureFormat::Bc3RgbaUnorm => (16, bcdec_rs::bc3),
        TextureFormat::Rgba8UnormSrgb => (0, |_, _, _| {}),
        _ => return None,
    };
    let mut rgba = vec![0u8; w * h * 4];
    if block == 0 {
        rgba.copy_from_slice(data.get(..w * h * 4)?);
    } else {
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let mut tile = [0u8; 64];
        for by in 0..bh {
            for bx in 0..bw {
                let at = (by * bw + bx) * block;
                decode(data.get(at..at + block)?, &mut tile, 16);
                for y in 0..4 {
                    for x in 0..4 {
                        let (px, py) = (bx * 4 + x, by * 4 + y);
                        if px < w && py < h {
                            let d = (py * w + px) * 4;
                            let s = (y * 4 + x) * 4;
                            rgba[d..d + 4].copy_from_slice(&tile[s..s + 4]);
                        }
                    }
                }
            }
        }
    }
    Some((w, h, rgba))
}
