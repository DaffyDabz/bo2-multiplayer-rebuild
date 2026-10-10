//! bo2mp: Black Ops II's sun shadows: the things that move (players,
//! bodies, thrown things, script models) cast shadows from the sun onto the
//! world and onto each other. As the game: two sun partitions, the near one
//! `1024 * sm_sunSampleSizeNear` units across and the far one four times
//! that, fitted around the camera in the sun's view and snapped to whole
//! texels so they do not crawl; the lit shaders read them with a compare
//! sampler (`sun_shadow_at` in the world shader). The world itself keeps its
//! baked sun shadows.

use super::*;
use bevy::render::render_resource::{
    BufferDescriptor, Extent3d, LoadOp, Operations, RenderPassDepthStencilAttachment,
    RenderPassDescriptor, TextureDescriptor, TextureDimension, TextureUsages, TextureView,
    TextureViewDescriptor,
};

pub(super) const SUN_SHADOW_SHADER: &str = "embedded://render_gpu/drawsurf/t6_sun_shadow.wgsl";
/// Texels per partition side (`1024 << sm_sunQuality`, quality 1).
const SIZE: u32 = 2048;
/// The lit side's constants: both partitions' clip-from-world, then (on,
/// near texel, far texel, depth range).
pub(super) const PARAMS_BYTES: u64 = 144;
/// `sm_sunSampleSizeNear` where the map's own is not known (`_load.csc`'s).
const SAMPLE_SIZE_NEAR: f32 = 0.5;
/// How far casters reach toward the sun and receivers away from it.
const DEPTH_HALF: f32 = 4096.0;

#[derive(Resource)]
pub(super) struct SunShadow {
    layers: [TextureView; 2],
    pub(super) array: TextureView,
    pub(super) sampler: Sampler,
    pub(super) params: Buffer,
    casters: [Buffer; 2],
    caster_layout: BindGroupLayoutDescriptor,
    caster_groups: Option<[BindGroup; 2]>,
    shader: Handle<Shader>,
    pipeline: Option<CachedRenderPipelineId>,
}

pub(super) fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("bo2mp_sun_shadow"),
        size: Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let layer = |base_array_layer| {
        texture.create_view(&TextureViewDescriptor {
            label: Some("bo2mp_sun_shadow_partition"),
            dimension: Some(TextureViewDimension::D2),
            base_array_layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    };
    let layers = [layer(0), layer(1)];
    let array = texture.create_view(&TextureViewDescriptor {
        label: Some("bo2mp_sun_shadow_partitions"),
        dimension: Some(TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("bo2mp_sun_shadow_compare"),
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        address_mode_w: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        compare: Some(CompareFunction::LessEqual),
        ..Default::default()
    });
    let uniform = |label: &'static str, size: u64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    commands.insert_resource(SunShadow {
        layers,
        array,
        sampler,
        params: uniform("bo2mp_sun_shadow_params", PARAMS_BYTES),
        casters: [
            uniform("bo2mp_sun_shadow_near", 64),
            uniform("bo2mp_sun_shadow_far", 64),
        ],
        caster_layout: BindGroupLayoutDescriptor::new(
            "bo2mp_sun_shadow_casters",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX,
                (uniform_buffer_sized(false, std::num::NonZeroU64::new(64)),),
            ),
        ),
        caster_groups: None,
        shader: asset_server.load(SUN_SHADOW_SHADER),
        pipeline: None,
    });
}

/// The casters' pipeline: depth only from the sun, both faces (a body's
/// back faces the sun as often as its front), with a slope bias against
/// a lit side shadowing itself.
fn caster_pipeline(shadow: &SunShadow) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some("bo2mp_sun_shadow_casters".into()),
        layout: vec![shadow.caster_layout.clone()],
        immediate_size: 0,
        vertex: VertexState {
            shader: shadow.shader.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vertex_caster".into()),
            buffers: vec![
                VertexBufferLayout {
                    array_stride: size_of::<SmodelVertex>() as u64,
                    step_mode: VertexStepMode::Vertex,
                    attributes: vec![VertexAttribute {
                        format: VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                },
                VertexBufferLayout {
                    array_stride: 96,
                    step_mode: VertexStepMode::Instance,
                    attributes: (0..4)
                        .map(|column| VertexAttribute {
                            format: VertexFormat::Float32x4,
                            offset: u64::from(column) * 16,
                            shader_location: 6 + column,
                        })
                        .collect(),
                },
            ],
        },
        fragment: None,
        primitive: PrimitiveState {
            front_face: FrontFace::Cw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        }),
        multisample: MultisampleState::default(),
        zero_initialize_workgroup_memory: false,
    }
}

/// A partition's clip-from-world: the sun's view, `extent` units across,
/// centred on `centre` snapped to whole texels.
fn partition(sun_view: Mat4, centre: Vec3, extent: f32) -> Mat4 {
    let texel = extent / SIZE as f32;
    let c = sun_view.transform_point3(centre);
    let (x, y) = ((c.x / texel).floor() * texel, (c.y / texel).floor() * texel);
    let half = extent * 0.5;
    let depth = -c.z;
    Mat4::orthographic_rh(
        x - half,
        x + half,
        y - half,
        y + half,
        depth - DEPTH_HALF,
        depth + DEPTH_HALF,
    ) * sun_view
}

/// A draw that casts a sun shadow: an opaque, untested surface, or a
/// shadow-only one (effects and clouds never do).
fn casts(code: u32) -> bool {
    if code == T6_NONE || code & (T6_EFFECT | T6_CLOUD) != 0 {
        return false;
    }
    match code & T6_BLEND_MASK {
        0 => code & T6_ALPHA_TEST == 0,
        T6_SHADOW_ONLY => true,
        _ => false,
    }
}

/// Draws both partitions before the world pass, or switches the lit side's
/// shadow off (no sun, nothing to cast).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_sun_shadow(
    view: ViewQuery<(&ExtractedView, &DiagnosticViewBindGroup)>,
    shadow: Option<ResMut<SunShadow>>,
    dynamic: Res<DiagnosticDynamic>,
    geometry: Res<DiagnosticGeometry>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut context: RenderContext,
) {
    if !geometry_diagnostic_enabled() {
        return;
    }
    let Some(mut shadow) = shadow else {
        return;
    };
    let shadow = &mut *shadow;
    if shadow.pipeline.is_none() {
        shadow.pipeline = Some(cache.queue_render_pipeline(caster_pipeline(shadow)));
    }
    if shadow.caster_groups.is_none() {
        let layout = cache.get_bind_group_layout(&shadow.caster_layout);
        let group = |buffer: &Buffer| {
            device.create_bind_group(
                "bo2mp_sun_shadow_casters",
                &layout,
                &BindGroupEntries::sequential((buffer.as_entire_binding(),)),
            )
        };
        shadow.caster_groups = Some([group(&shadow.casters[0]), group(&shadow.casters[1])]);
    }
    let lists = [dynamic.missiles.as_ref(), dynamic.script_static.as_ref()];
    let any = lists
        .iter()
        .flatten()
        .any(|(_, _, _, draws)| draws.iter().any(|d| d[1] != 0 && casts(d[3])));
    let sun = geometry
        .t6_lighting
        .map(|l| Vec3::new(l[0][0], l[0][1], l[0][2]))
        .filter(|d| d.length_squared() > 1e-6)
        .map(Vec3::normalize);
    let pipeline = shadow.pipeline.and_then(|id| cache.get_render_pipeline(id));
    let mut params = [0.0f32; (PARAMS_BYTES / 4) as usize];
    let (Some(sun), Some(pipeline), true) = (sun, pipeline, any) else {
        queue.write_buffer(&shadow.params, 0, bytemuck::cast_slice(&params));
        return;
    };
    let world_from_view = view.into_inner().0.world_from_view.to_matrix();
    let eye = world_from_view.w_axis.truncate();
    let forward = -world_from_view.z_axis.truncate();
    let up = if sun.z.abs() > 0.99 { Vec3::X } else { Vec3::Z };
    let sun_view = Mat4::look_to_rh(Vec3::ZERO, -sun, up);
    let sample_size = match geometry.t6_sun_sample_size_near {
        size if size > 0.0 => size,
        _ => SAMPLE_SIZE_NEAR,
    };
    let near_extent = 1024.0 * sample_size;
    let extents = [near_extent, near_extent * 4.0];
    let mats = extents.map(|extent| partition(sun_view, eye + forward * (extent * 0.25), extent));
    params[..16].copy_from_slice(&mats[0].to_cols_array());
    params[16..32].copy_from_slice(&mats[1].to_cols_array());
    params[32] = 1.0;
    params[33] = extents[0] / SIZE as f32;
    params[34] = extents[1] / SIZE as f32;
    params[35] = DEPTH_HALF * 2.0;
    queue.write_buffer(&shadow.params, 0, bytemuck::cast_slice(&params));
    for (buffer, mat) in shadow.casters.iter().zip(mats.iter()) {
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&mat.to_cols_array()));
    }
    let Some(groups) = shadow.caster_groups.as_ref() else {
        return;
    };
    for (layer, group) in shadow.layers.iter().zip(groups.iter()) {
        let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("bo2mp_sun_shadow_pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: layer,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(1.0),
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        for (vb, ib, inst, draws) in lists.iter().flatten() {
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_vertex_buffer(1, inst.slice(..));
            pass.set_index_buffer(ib.slice(..), IndexFormat::Uint32);
            for &[start, count, _, code, instance, _, _] in draws {
                if count != 0 && casts(code) {
                    pass.draw_indexed(start..start.saturating_add(count), 0, instance..instance + 1);
                }
            }
        }
    }
}
