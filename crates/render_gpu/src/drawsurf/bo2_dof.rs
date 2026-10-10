//! bo2mp: depth of field on a Black Ops II map. The engine's own depth of
//! field (`postfx.rs`) needs the MW2 post-effect materials, which a BO2 map
//! does not load, so a BO2 match had none: no blur on near things while
//! aiming down the sights (real 53: the near wall and car are soft, the gun
//! and the distance sharp) and none in the death view (real 54: the body in
//! the near corner is blurred, the killer sharp). The ranges are the same
//! frame values the engine's pass reads (`DofFrame`: the sights' autofocus,
//! the dead view's, script and dvar overrides); this pass draws them: the
//! frame at quarter size with each block's blur amount, blurred, then mixed
//! back by each pixel's distance from the scene depth.

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::RenderStartup;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, uniform_buffer_sized,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferDescriptor,
    BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode,
    FragmentState, LoadOp, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, StoreOp, Texture, TextureDescriptor, TextureDimension, TextureFormat,
    TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::shader::Shader;
use std::collections::HashMap;
use std::num::NonZeroU64;

use super::scene_depth::SceneDepthTexture;

const SHADER_PATH: &str = "embedded://render_gpu/drawsurf/bo2_dof.wgsl";
const PARAMS_BYTES: u64 = 80;
const QUARTER_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

#[derive(Resource)]
struct Bo2Dof {
    blur_layout: BindGroupLayoutDescriptor,
    shrink_layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    sampler: Option<Sampler>,
    /// Two quarter-size ping-pong targets, for the frame size they were made at.
    quarter: Option<(UVec2, [(Texture, TextureView); 2])>,
    /// shrink, blur x, blur y, composite.
    params: Option<[Buffer; 4]>,
    shrink: Option<CachedRenderPipelineId>,
    blur: Option<CachedRenderPipelineId>,
    composite: HashMap<TextureFormat, CachedRenderPipelineId>,
}

fn init(mut commands: Commands, asset_server: Res<AssetServer>) {
    let float = || texture_2d(TextureSampleType::Float { filterable: true });
    let params = || uniform_buffer_sized(false, NonZeroU64::new(PARAMS_BYTES));
    commands.insert_resource(Bo2Dof {
        blur_layout: BindGroupLayoutDescriptor::new(
            "bo2mp_dof_blur",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, float()),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (2, params()),
                ),
            ),
        ),
        shrink_layout: BindGroupLayoutDescriptor::new(
            "bo2mp_dof_shrink",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, float()),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (2, params()),
                    (4, texture_depth_2d()),
                ),
            ),
        ),
        composite_layout: BindGroupLayoutDescriptor::new(
            "bo2mp_dof_composite",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, float()),
                    (1, sampler(SamplerBindingType::Filtering)),
                    (2, params()),
                    (3, float()),
                    (4, texture_depth_2d()),
                ),
            ),
        ),
        shader: asset_server.load(SHADER_PATH),
        sampler: None,
        quarter: None,
        params: None,
        shrink: None,
        blur: None,
        composite: HashMap::new(),
    });
}

fn pipeline(
    dof: &Bo2Dof,
    entry: &'static str,
    layout: &BindGroupLayoutDescriptor,
    format: TextureFormat,
) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some(format!("bo2mp_dof_{entry}").into()),
        layout: vec![layout.clone()],
        immediate_size: 0,
        vertex: VertexState {
            shader: dof.shader.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vertex".into()),
            buffers: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: dof.shader.clone(),
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

/// The blur's step in quarter texels for a frame `height` high: the engine's
/// near blur radius (`near_blur` at a 480-high screen, a quarter of it in
/// quarter texels), over the 9-tap blur's own spread.
fn blur_step(height: u32, near_blur: f32) -> f32 {
    (height as f32 * near_blur * 0.25 / 480.0 / 1.5).clamp(1.0, 4.0)
}

fn draw_bo2_dof(
    view: ViewQuery<(&ViewTarget, &SceneDepthTexture, &ExtractedView)>,
    enabled: Res<super::bo2_bloom::Bo2BloomEnabled>,
    extracted: Res<super::postfx::ExtractedPostFx>,
    dof: Option<ResMut<Bo2Dof>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut logged: Local<Option<super::postfx_dof::DepthOfField>>,
    mut context: RenderContext,
) {
    // The engine's own chain (when the MW2 materials are there) draws it.
    if !enabled.0 || !crate::drawsurf::geometry_diagnostic_enabled() || !extracted.films.is_empty()
    {
        return;
    }
    let d = extracted.frame.dof;
    // The engine's own chain draws nothing under a near blur of 4 either
    // (`postfx_dof`'s plan).
    let on = d.active() && d.near_blur >= 4.0;
    // Once per change: what the pass draws (the sights' and the dead view's
    // focus come from the frame values, so this shows they reach a BO2 map).
    let shown = on.then_some(d);
    if *logged != shown {
        match shown {
            Some(d) => info!(
                "bo2mp dof: near {:.0}-{:.0} far {:.0}-{:.0} gun {:.1}-{:.1} blur {:.2}/{:.2}",
                d.near_start,
                d.near_end,
                d.far_start,
                d.far_end,
                d.view_model_start,
                d.view_model_end,
                d.near_blur,
                d.far_blur
            ),
            None => info!("bo2mp dof: off"),
        }
        *logged = shown;
    }
    if !on {
        return;
    }
    let Some(mut dof) = dof else {
        return;
    };
    let (target, depth, extracted_view) = view.into_inner();
    let Some(znear) = super::floatz::znear_from_clip_from_view(extracted_view.clip_from_view)
    else {
        return;
    };
    let format = target.main_texture_format();
    if dof.shrink.is_none() {
        let p = pipeline(&dof, "shrink", &dof.shrink_layout, QUARTER_FORMAT);
        dof.shrink = Some(cache.queue_render_pipeline(p));
        let p = pipeline(&dof, "blur", &dof.blur_layout, QUARTER_FORMAT);
        dof.blur = Some(cache.queue_render_pipeline(p));
    }
    if !dof.composite.contains_key(&format) {
        let p = pipeline(&dof, "composite", &dof.composite_layout, format);
        let id = cache.queue_render_pipeline(p);
        dof.composite.insert(format, id);
    }
    let (Some(shrink), Some(blur), Some(composite)) = (
        dof.shrink.and_then(|id| cache.get_render_pipeline(id)),
        dof.blur.and_then(|id| cache.get_render_pipeline(id)),
        dof.composite.get(&format).and_then(|&id| cache.get_render_pipeline(id)),
    ) else {
        return;
    };
    let size = target.main_texture().size();
    let full = UVec2::new(size.width, size.height);
    let quarter = (full / 4).max(UVec2::ONE);
    if dof.quarter.as_ref().is_none_or(|(have, _)| *have != full) {
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
        dof.quarter = Some((full, [make("bo2mp_dof_a"), make("bo2mp_dof_b")]));
    }
    let sampler = dof
        .sampler
        .get_or_insert_with(|| {
            device.create_sampler(&SamplerDescriptor {
                label: Some("bo2mp_dof_sampler"),
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                ..Default::default()
            })
        })
        .clone();
    let params = dof
        .params
        .get_or_insert_with(|| {
            std::array::from_fn(|_| {
                device.create_buffer(&BufferDescriptor {
                    label: Some("bo2mp_dof_params"),
                    size: PARAMS_BYTES,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
        })
        .clone();
    let band = render_backend::DEPTH_RANGE_BAND;
    // The gun is drawn with its own near plane (the depth resolve's too).
    let gun_near = Some(extracted.frame.view_model_near).filter(|n| *n > 0.0).unwrap_or(znear);
    let write = |buffer: &Buffer, texel: Vec2, dir: Vec2| {
        let data: [f32; 20] = [
            texel.x,
            texel.y,
            dir.x,
            dir.y,
            d.near_start,
            d.near_end,
            d.far_start,
            d.far_end,
            (d.far_blur / d.near_blur).clamp(0.0, 1.0),
            0.0,
            0.0,
            0.0,
            znear,
            1.0 - band,
            band,
            gun_near,
            d.view_model_start,
            d.view_model_end,
            0.0,
            0.0,
        ];
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&data));
    };
    let full_texel = Vec2::new(1.0 / full.x as f32, 1.0 / full.y as f32);
    let quarter_texel = Vec2::new(1.0 / quarter.x as f32, 1.0 / quarter.y as f32);
    let step = blur_step(full.y, d.near_blur);
    write(&params[0], full_texel, Vec2::ZERO);
    write(&params[1], quarter_texel, Vec2::new(step, 0.0));
    write(&params[2], quarter_texel, Vec2::new(0.0, step));
    write(&params[3], full_texel, Vec2::ZERO);
    let Some((_, [(_, a), (_, b)])) = dof.quarter.as_ref() else {
        return;
    };
    let blur_layout = cache.get_bind_group_layout(&dof.blur_layout);
    let shrink_layout = cache.get_bind_group_layout(&dof.shrink_layout);
    let composite_layout = cache.get_bind_group_layout(&dof.composite_layout);
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
    let shrink_bind = device.create_bind_group(
        "bo2mp_dof_shrink",
        &shrink_layout,
        &BindGroupEntries::with_indices((
            (0, post.source),
            (1, &sampler),
            (2, params[0].as_entire_binding()),
            (4, depth.view()),
        )),
    );
    let blur_bind = |source: &TextureView, buffer: &Buffer| {
        device.create_bind_group(
            "bo2mp_dof_blur",
            &blur_layout,
            &BindGroupEntries::with_indices((
                (0, source),
                (1, &sampler),
                (2, buffer.as_entire_binding()),
            )),
        )
    };
    run("bo2mp_dof_shrink", shrink, &shrink_bind, a);
    run("bo2mp_dof_blur_h", blur, &blur_bind(a, &params[1]), b);
    run("bo2mp_dof_blur_v", blur, &blur_bind(b, &params[2]), a);
    let composite_bind = device.create_bind_group(
        "bo2mp_dof_composite",
        &composite_layout,
        &BindGroupEntries::with_indices((
            (0, post.source),
            (1, &sampler),
            (2, params[3].as_entire_binding()),
            (3, a),
            (4, depth.view()),
        )),
    );
    run("bo2mp_dof_composite", composite, &composite_bind, post.destination);
}

pub(super) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "bo2_dof.wgsl");
    let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
        return;
    };
    render_app.add_systems(RenderStartup, init).add_systems(
        Core3d,
        draw_bo2_dof
            .in_set(Core3dSystems::PostProcess)
            .before(super::bo2_bloom::draw_bo2_bloom),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blur_step_grows_with_the_screen_and_the_near_blur() {
        // 720 high, the engine's near blur 6: 2.25 quarter texels of radius.
        assert!((blur_step(720, 6.0) - 1.5).abs() < 1e-4);
        assert!(blur_step(1440, 6.0) > blur_step(720, 6.0));
        assert_eq!(blur_step(240, 1.0), 1.0);
        assert_eq!(blur_step(4320, 10.0), 4.0);
    }
}
