//! bo2zm: Black Ops II's bloom and final colour, after the fallback pass on
//! a Black Ops II map. BO2 (`hdr_bloom_downsample`, `hdr_bloom_combine_hilo`,
//! `hdr_bloom_apply`, `hdr_apply_lut2d`, fxc disassembly) takes the scene's
//! bright parts by smooth ramps on linear luminance (colour and luminance
//! apart), blurs them into a high and a low level, tints those by the map's
//! vision (`vc_RGBH`, `vc_RGBL`, `vc_YH`, `vc_YL`), adds them to the scene in
//! linear, rolls highlights off above 0.75 and looks the result up in the
//! map's colour table (`asset_world::t6_grade`). The ramps' ends are the
//! vision rows' w, low to high (the code's mapping is not in the shaders).
//! The fallback pass already rolls highlights off (8-bit frame); this pass
//! undoes it before adding bloom.
//!
//! bo2zm M4: the same pass blurs the whole world while BO2's menus ask for
//! it (`Engine.BlurWorld`, `frame::WorldBlur`): the frame at a quarter size,
//! blurred twice as wide, put back in its place. The menus draw after it.

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::RenderStartup;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_3d, uniform_buffer_sized,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode,
    FragmentState, LoadOp, Operations, Origin3d, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, StoreOp, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::ViewTarget;
use bevy::shader::Shader;
use std::collections::HashMap;
use std::num::NonZeroU64;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/bo2_bloom.wgsl";
const PARAMS_BYTES: u64 = 112;
const QUARTER_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const LUT_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
/// The low bloom level: the high one blurred again this many texels apart.
const LOW_STEP: f32 = 3.0;

static NO_BLOOM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// BO2 maps only (set by the fallback pass's geometry: the map has T6
/// lighting).
#[derive(Resource, Default)]
pub(super) struct Bo2BloomEnabled(pub bool);

/// How much BO2's menus blur the world this frame (0 = none; 2 = full).
#[derive(Resource, Default)]
struct Bo2MenuBlur(f32, f32, f32, f32);

fn extract_menu_blur(
    blur: bevy::render::Extract<Option<Res<frame::WorldBlur>>>,
    screen: bevy::render::Extract<Option<Res<frame::AppScreen>>>,
    dim: bevy::render::Extract<Option<Res<frame::WorldDim>>>,
    mut out: ResMut<Bo2MenuBlur>,
) {
    out.0 = blur.as_ref().map_or(0.0, |b| b.0);
    // bo2mp: the class select before the first spawn (and the front end) shows the
    // world grey and dim (real 28); the pause menu in a live match keeps its colour (real 58).
    // bo2mp: the black dims the in-match menus stack over the world.
    out.2 = dim.as_ref().map_or(0.0, |d| d.0);
    out.1 = if screen.as_ref().is_some_and(|s| **s == frame::AppScreen::InGame) { 0.0 } else { 1.0 };
    // bo2mp: real 28 (the class select at the first spawn) shows the world sharp, only grey and
    // dim (pebbles and the parking sign's digits resolve); the blur belongs to the pause menu
    // and the front end. (out.3: 1 = no blur, the grey and dim stay.)
    out.3 = if screen.as_ref().is_some_and(|s| **s == frame::AppScreen::ClassSelect) { 1.0 } else { 0.0 };
}

#[derive(Resource)]
struct Bo2Bloom {
    layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    sampler: Option<Sampler>,
    /// Quarter-size targets (two ping-pong, one for the low bloom level),
    /// for the frame size they were made at.
    quarter: Option<(UVec2, [(Texture, TextureView); 3])>,
    /// bright, blur four times, composite: one parameter buffer each; then
    /// the menu blur's: shrink, blur four times, put back.
    params: Option<[Buffer; 12]>,
    /// bo2mp: the map's colour table (3D, 32^3) and the grade generation it
    /// was made from.
    lut: Option<(u64, Texture, TextureView)>,
    extract: Option<CachedRenderPipelineId>,
    bright: Option<CachedRenderPipelineId>,
    blur: Option<CachedRenderPipelineId>,
    composite: HashMap<TextureFormat, CachedRenderPipelineId>,
    blurred: HashMap<TextureFormat, CachedRenderPipelineId>,
}

fn init(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(Bo2Bloom {
        layout: BindGroupLayoutDescriptor::new(
            "bo2zm_bloom",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, texture_2d(TextureSampleType::Float { filterable: true })),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (2, uniform_buffer_sized(false, NonZeroU64::new(PARAMS_BYTES))),
                ),
            ),
        ),
        composite_layout: BindGroupLayoutDescriptor::new(
            "bo2zm_bloom_composite",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, texture_2d(TextureSampleType::Float { filterable: true })),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (2, uniform_buffer_sized(false, NonZeroU64::new(PARAMS_BYTES))),
                    (3, texture_2d(TextureSampleType::Float { filterable: true })),
                    (4, texture_2d(TextureSampleType::Float { filterable: true })),
                    (5, texture_3d(TextureSampleType::Float { filterable: true })),
                ),
            ),
        ),
        shader: asset_server.load(SHADER_PATH),
        sampler: None,
        quarter: None,
        params: None,
        lut: None,
        extract: None,
        bright: None,
        blur: None,
        composite: HashMap::new(),
        blurred: HashMap::new(),
    });
}

fn pipeline(
    bloom: &Bo2Bloom,
    entry: &'static str,
    format: TextureFormat,
    composite: bool,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some(format!("bo2zm_bloom_{entry}").into()),
        layout: vec![if composite {
            bloom.composite_layout.clone()
        } else {
            bloom.layout.clone()
        }],
        immediate_size: 0,
        vertex: VertexState {
            shader: bloom.shader.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vertex".into()),
            buffers: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: bloom.shader.clone(),
            shader_defs: Vec::new(),
            entry_point: Some(entry.into()),
            targets: vec![Some(ColorTargetState {
                format,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        zero_initialize_workgroup_memory: false,
    }
}

fn draw_bo2_bloom(
    view: ViewQuery<&ViewTarget>,
    enabled: Res<Bo2BloomEnabled>,
    menu_blur: Res<Bo2MenuBlur>,
    bloom: Option<ResMut<Bo2Bloom>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut context: RenderContext,
) {
    if !enabled.0 || !crate::drawsurf::geometry_diagnostic_enabled() {
        return;
    }
    let Some(mut bloom) = bloom else {
        return;
    };
    let target = view.into_inner();
    let format = target.main_texture_format();
    // Pipelines, once (the composite per target format).
    if bloom.extract.is_none() {
        let d = pipeline(&bloom, "extract", QUARTER_FORMAT, false);
        bloom.extract = Some(cache.queue_render_pipeline(d));
        let d = pipeline(&bloom, "bright", QUARTER_FORMAT, false);
        bloom.bright = Some(cache.queue_render_pipeline(d));
        let d = pipeline(&bloom, "blur", QUARTER_FORMAT, false);
        bloom.blur = Some(cache.queue_render_pipeline(d));
    }
    if !bloom.composite.contains_key(&format) {
        let d = pipeline(&bloom, "composite", format, true);
        let id = cache.queue_render_pipeline(d);
        bloom.composite.insert(format, id);
        let d = pipeline(&bloom, "blurred", format, true);
        let id = cache.queue_render_pipeline(d);
        bloom.blurred.insert(format, id);
    }
    let (Some(extract), Some(bright), Some(blur), Some(composite)) = (
        bloom.extract.and_then(|id| cache.get_render_pipeline(id)),
        bloom.bright.and_then(|id| cache.get_render_pipeline(id)),
        bloom.blur.and_then(|id| cache.get_render_pipeline(id)),
        bloom.composite.get(&format).and_then(|&id| cache.get_render_pipeline(id)),
    ) else {
        return;
    };
    let size = target.main_texture().size();
    let full = UVec2::new(size.width, size.height);
    let quarter = (full / 4).max(UVec2::ONE);
    if bloom.quarter.as_ref().is_none_or(|(have, _)| *have != full) {
        let make = |label: &'static str| {
            let texture = device.create_texture(&TextureDescriptor {
                label: Some(label),
                size: Extent3d {
                    width: quarter.x,
                    height: quarter.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: QUARTER_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&TextureViewDescriptor::default());
            (texture, view)
        };
        bloom.quarter = Some((
            full,
            [
                make("bo2zm_bloom_a"),
                make("bo2zm_bloom_b"),
                make("bo2zm_bloom_low"),
            ],
        ));
    }
    // bo2mp: the map's colour table, remade when a map installs a new one
    // (identity when the map has no vision).
    let (generation, grade) = asset_world::t6_grade();
    if bloom
        .lut
        .as_ref()
        .is_none_or(|(have, ..)| *have != generation)
    {
        let n = asset_world::T6_LUT_SIZE as u32;
        let texels = grade.as_ref().map_or_else(
            || asset_world::t6_grade_lut(&asset_world::T6Vision::default(), None),
            |g| g.lut.clone(),
        );
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("bo2mp_grade_lut"),
            size: Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D3,
            format: LUT_FORMAT,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &texels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 4),
                rows_per_image: Some(n),
            },
            Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        let view = texture.create_view(&TextureViewDescriptor::default());
        bloom.lut = Some((generation, texture, view));
    }
    // The bloom: ramps and tints from the map's vision (none without one;
    // `IW4L_T6_NO_BLOOM` turns it off, a test aid for comparing).
    let vision = grade.as_ref().map(|g| g.vision).unwrap_or_default();
    let no_bloom = *NO_BLOOM.get_or_init(|| std::env::var_os("IW4L_T6_NO_BLOOM").is_some());
    let (rgb_h, rgb_l, y_h, y_l) = (
        vision.bloom_rgb_hi,
        vision.bloom_rgb_lo,
        vision.bloom_y_hi,
        vision.bloom_y_lo,
    );
    let ramp = [
        rgb_h[3].min(rgb_l[3]),
        y_h[3].min(y_l[3]),
        rgb_h[3].max(rgb_l[3]),
        y_h[3].max(y_l[3]),
    ];
    let tint = |row: [f32; 4]| {
        if no_bloom {
            [0.0; 4]
        } else {
            [row[0], row[1], row[2], 0.0]
        }
    };
    let bloom_rows = [ramp, tint(rgb_h), tint(rgb_l), tint(y_h), tint(y_l)];
    let sampler = bloom
        .sampler
        .get_or_insert_with(|| {
            device.create_sampler(&SamplerDescriptor {
                label: Some("bo2zm_bloom_sampler"),
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                ..Default::default()
            })
        })
        .clone();
    let params = bloom
        .params
        .get_or_insert_with(|| {
            std::array::from_fn(|_| {
                device.create_buffer(&BufferDescriptor {
                    label: Some("bo2zm_bloom_params"),
                    size: PARAMS_BYTES,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
        })
        .clone();
    let write_with = |buffer: &Buffer, texel: Vec2, dir: Vec2, threshold: [f32; 4]| {
        let mut data = [0.0f32; 28];
        data[..8].copy_from_slice(&[
            texel.x, texel.y, dir.x, dir.y, threshold[0], threshold[1], threshold[2], threshold[3],
        ]);
        for (i, row) in bloom_rows.iter().enumerate() {
            data[8 + i * 4..12 + i * 4].copy_from_slice(row);
        }
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&data));
    };
    let write = |buffer: &Buffer, texel: Vec2, dir: Vec2| write_with(buffer, texel, dir, [0.0; 4]);
    let full_texel = Vec2::new(1.0 / full.x as f32, 1.0 / full.y as f32);
    let quarter_texel = Vec2::new(1.0 / quarter.x as f32, 1.0 / quarter.y as f32);
    write(&params[0], full_texel, Vec2::ZERO);
    write(&params[1], quarter_texel, Vec2::new(1.0, 0.0));
    write(&params[2], quarter_texel, Vec2::new(0.0, 1.0));
    write(&params[3], quarter_texel, Vec2::new(LOW_STEP, 0.0));
    write(&params[4], quarter_texel, Vec2::new(0.0, LOW_STEP));
    write(&params[5], full_texel, Vec2::ZERO);
    let (Some((_, [(_, a), (_, b), (_, low)])), Some((_, _, lut))) =
        (bloom.quarter.as_ref(), bloom.lut.as_ref())
    else {
        return;
    };
    let layout = cache.get_bind_group_layout(&bloom.layout);
    let composite_layout = cache.get_bind_group_layout(&bloom.composite_layout);
    let post = target.post_process_write();
    let encoder = context.command_encoder();
    let mut run = |label: &'static str,
                   pipe: &bevy::render::render_resource::RenderPipeline,
                   bind: &bevy::render::render_resource::BindGroup,
                   out: &TextureView| {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: out,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(LinearRgba::BLACK.into()),
                    store: StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    };
    let bind = |source: &TextureView, buffer: &Buffer| {
        device.create_bind_group(
            "bo2zm_bloom",
            &layout,
            &BindGroupEntries::with_indices(((0, source), (1, &sampler), (2, buffer.as_entire_binding()))),
        )
    };
    // The high level (quarter size, blurred), then the low (the high blurred
    // again, wider).
    run("bo2zm_bloom_bright", bright, &bind(post.source, &params[0]), a);
    run("bo2zm_bloom_blur_h", blur, &bind(a, &params[1]), b);
    run("bo2zm_bloom_blur_v", blur, &bind(b, &params[2]), a);
    run("bo2mp_bloom_low_h", blur, &bind(a, &params[3]), b);
    run("bo2mp_bloom_low_v", blur, &bind(b, &params[4]), low);
    let composite_bind = device.create_bind_group(
        "bo2zm_bloom_composite",
        &composite_layout,
        &BindGroupEntries::with_indices((
            (0, post.source),
            (1, &sampler),
            (2, params[5].as_entire_binding()),
            (3, a),
            (4, low),
            (5, lut),
        )),
    );
    run("bo2zm_bloom_composite", composite, &composite_bind, post.destination);
    // The menus' world blur: the whole frame (no threshold), shrunk,
    // blurred along x and y at one and then two texels, put back.
    let amount = (menu_blur.0 / 2.0).clamp(0.0, 1.0);
    let Some(blurred) = bloom.blurred.get(&format).and_then(|&id| cache.get_render_pipeline(id))
    else {
        return;
    };
    // (bo2mp: with two or more dims over the match world the pass still runs, with no blur, to lift the dark world.)
    if amount <= 0.0 && !(menu_blur.1 < 0.5 && menu_blur.2 >= 1.5) {
        return;
    }
    let all = [-1.0, 0.0, menu_blur.1, amount];
    write_with(&params[6], full_texel, Vec2::ZERO, all);
    write_with(&params[7], quarter_texel, Vec2::new(1.0, 0.0), all);
    write_with(&params[8], quarter_texel, Vec2::new(0.0, 1.0), all);
    write_with(&params[9], quarter_texel, Vec2::new(2.0, 0.0), all);
    write_with(&params[10], quarter_texel, Vec2::new(0.0, 2.0), all);
    // (y: how many gamma-space dims the UI stacks over this: the shader
    // lifts the dark world so the second one does not crush it.)
    write_with(&params[11], full_texel, Vec2::ZERO, [if menu_blur.3 > 0.5 { 1.0 } else { -1.0 }, menu_blur.2, menu_blur.1, amount]);
    let post = target.post_process_write();
    run("bo2zm_menu_blur_shrink", extract, &bind(post.source, &params[6]), a);
    run("bo2zm_menu_blur_h1", blur, &bind(a, &params[7]), b);
    run("bo2zm_menu_blur_v1", blur, &bind(b, &params[8]), a);
    run("bo2zm_menu_blur_h2", blur, &bind(a, &params[9]), b);
    run("bo2zm_menu_blur_v2", blur, &bind(b, &params[10]), a);
    let blurred_bind = device.create_bind_group(
        "bo2zm_menu_blur",
        &composite_layout,
        &BindGroupEntries::with_indices((
            (0, post.source),
            (1, &sampler),
            (2, params[11].as_entire_binding()),
            (3, a),
            (4, low),
            (5, lut),
        )),
    );
    run("bo2zm_menu_blur", blurred, &blurred_bind, post.destination);
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "bo2_bloom.wgsl");
    let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
        return;
    };
    render_app
        .init_resource::<Bo2BloomEnabled>()
        .init_resource::<Bo2MenuBlur>()
        .add_systems(RenderStartup, init)
        .add_systems(bevy::render::ExtractSchedule, extract_menu_blur)
        .add_systems(
            Core3d,
            draw_bo2_bloom
                .in_set(Core3dSystems::PostProcess)
                .before(super::postfx::PostFxSet),
        );
}
