//! bo2mp: a destructible's broken-off pieces (a mannequin's head). Each one
//! the server sends spawns as loose clutter where its bone was, turned the
//! same way, and leaves at the speed the hit gave it; the client's own
//! physics flies it from there, as Black Ops II's clients do. Black Ops II
//! keeps 50 at most, the oldest going first.

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::adapters::anim::dyn_ent_phys::{DynEntPhysImpulse, DynEntPhysWorld};
use crate::adapters::anim::dyn_ent_wake::{DYNENT_BULLET_FORCE, DYNENT_EXPLODE_FORCE};
use crate::prepare::scene::cull::DynEntModelEntity;
use crate::prepare::scene::world::{WorldDynEntInstance, transform_from_gfx_placement};
use net::PendingDebris;

const MAX_DEBRIS: usize = 50;

/// Black Ops II's push on a loose piece per unit of hit
/// (`dynEnt_bulletForce`, `dynEnt_explodeForce`). The shared code that
/// pushes loose things when shot or blasted uses the older games' numbers,
/// so a piece's own scales are cut to land on these.
const BO2_BULLET_FORCE: f32 = 0.8;
const BO2_EXPLODE_FORCE: f32 = 12.0;

/// A piece that came loose; `launch` is its first push, given once the
/// piece exists for the physics.
#[derive(Component)]
pub struct Debris {
    launch: Option<Vec3>,
}

#[derive(Resource, Default)]
struct DebrisPool {
    live: VecDeque<(Entity, u16)>,
}

fn spawn_debris(
    mut commands: Commands,
    mut pending: ResMut<PendingDebris>,
    mut pool: ResMut<DebrisPool>,
    mut phys: ResMut<DynEntPhysWorld>,
    facts: Res<render_scene::WorldPresentFacts>,
    map: Query<&WorldDynEntInstance, (With<DynEntModelEntity>, Without<Debris>)>,
    live: Query<(), With<Debris>>,
) {
    pool.live.retain(|(entity, _)| live.contains(*entity));
    if pending.0.is_empty() {
        return;
    }
    if !facts.spawned {
        pending.0.clear();
        return;
    }
    // The map's own loose things keep their numbers; pieces take the ones
    // after them.
    let first = map.iter().map(|i| i.index).max().map_or(0, |i| i.saturating_add(1));
    for record in pending.0.drain(..) {
        let index = if pool.live.len() >= MAX_DEBRIS {
            let (oldest, index) = pool.live.pop_front().expect("pool is full");
            phys.destroy_body(oldest);
            commands.entity(oldest).despawn();
            index
        } else {
            (0..MAX_DEBRIS as u16)
                .map(|k| first.saturating_add(k))
                .find(|i| !pool.live.iter().any(|(_, used)| used == i))
                .unwrap_or(first)
        };
        if std::env::var_os("IW4L_T6_HITLOG").is_some() {
            diag::info!(
                World,
                "bo2mp debris {} spawns as loose piece {index} at {:?} flying {:?}",
                record.model,
                record.origin,
                record.velocity
            );
        }
        let transform = transform_from_gfx_placement(record.origin, record.rotation);
        let instance = WorldDynEntInstance {
            index,
            ty: asset_world::DynEntType::Clutter,
            current_model: asset_world::MapXModelAssetKey(record.model.clone()),
            transform,
            lighting_origin: record.origin,
            phys_preset: Some(asset_world::OwnedPhysPreset {
                name: record.model,
                preset_type: 0,
                mass: record.mass,
                bounce: record.bounce,
                friction: record.friction,
                bullet_force_scale: record.bullet_force_scale * BO2_BULLET_FORCE
                    / DYNENT_BULLET_FORCE,
                explosive_force_scale: record.explosive_force_scale * BO2_EXPLODE_FORCE
                    / DYNENT_EXPLODE_FORCE,
                snd_alias_prefix: String::new(),
                pieces_spread_fraction: 0.0,
                pieces_upward_velocity: 0.0,
                temp_default_to_cylinder: false,
                per_surface_snd_alias: false,
            }),
            health: 0,
            destroy_fx: None,
            dead: false,
        };
        let entity = commands
            .spawn((
                transform,
                Visibility::Inherited,
                DynEntModelEntity,
                instance,
                Debris {
                    launch: Some(Vec3::from_array(record.velocity) * record.mass),
                },
            ))
            .id();
        pool.live.push_back((entity, index));
    }
}

fn launch_debris(
    mut pieces: Query<(Entity, &mut Debris)>,
    mut impulses: MessageWriter<DynEntPhysImpulse>,
) {
    for (entity, mut piece) in &mut pieces {
        if let Some(impulse) = piece.launch.take() {
            // A piece with no push still falls: wake it.
            let impulse = if impulse.length_squared() > 0.0 {
                impulse
            } else {
                Vec3::new(0.0, 0.0, -1e-6)
            };
            impulses.write(DynEntPhysImpulse { entity, impulse });
        }
    }
}

pub fn register_debris(app: &mut App) {
    app.init_resource::<DebrisPool>()
        .add_systems(
            Update,
            spawn_debris.in_set(net::ClientSet::Present),
        )
        .add_systems(
            Update,
            launch_debris
                .in_set(frame::WorkerCmdSet::Physics)
                .before(render_anim::occupancy::dyn_ent_phys::step_phys_world0),
        );
}
