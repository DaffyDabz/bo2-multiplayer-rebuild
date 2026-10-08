//! bo2mp scope lane: BO2's additive HUD pictures (blend ONE/ONE or
//! SRC_ALPHA/ONE: the bottom-left score panel's lines and glows, the
//! faction emblems' glow) drawn the way the game draws them: the picture's
//! colour is added onto what is under it. Bevy's UI only alpha-blends, which
//! darkens the scene under the lines instead of lighting it.

use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
};
use bevy::shader::ShaderRef;

const SHADER_PATH: &str = "embedded://ui/lui_additive.wgsl";

/// One additive picture and its tint (linear, alpha = fade).
#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct AdditiveUi {
    #[uniform(0)]
    pub tint: LinearRgba,
    #[texture(1)]
    #[sampler(2)]
    pub picture: Handle<Image>,
    /// The part of the picture the node shows: offset xy, size zw (0..1).
    #[uniform(3)]
    pub uv: Vec4,
    /// 1 when drawn into an encoded (gamma-space) main texture: the colour is
    /// added as it is (BO2's ONE/ONE); 0 for the straight-alpha overlay target.
    #[uniform(4)]
    pub encoded: f32,
}

impl UiMaterial for AdditiveUi {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn specialize(descriptor: &mut RenderPipelineDescriptor, _key: UiMaterialKey<Self>) {
        let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        if let Some(target) = descriptor.fragment.as_mut().and_then(|f| f.targets.first_mut()).and_then(|t| t.as_mut()) {
            target.blend = Some(BlendState { color: add, alpha: add });
        }
    }
}

pub(crate) fn register(app: &mut App) {
    bevy::asset::embedded_asset!(app, "lui_additive.wgsl");
    app.add_plugins(UiMaterialPlugin::<AdditiveUi>::default());
}
