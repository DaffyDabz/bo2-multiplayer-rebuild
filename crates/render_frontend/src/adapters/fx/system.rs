use std::collections::HashMap;
use std::sync::Arc;

use asset_game::{FxDefinitions, OwnedFxVisual, lookup_fx_color_image};
use assets::PreparedWeapons;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use dpvs_iw4::pack_mark_mesh_draw_surf;
use entity_iw4::{is_left_hand_fire_event, is_weapon_fire_last_shot_event};
use frame::ViewSubject;
use fx::{
    CodeMeshStep, FxGapCause, FxMsec, FxSparkCloudInstance, FxSpriteInstance, FxSystemHost,
    MarkReceiverEnable, PlayResult, axis_from_hit_normal,
};
use fx_iw4::{
    FX_ELEM_VEL_LOCAL, FX_ELEM_VEL_WORLD, FX_GLASS_SHATTER_FX_PER_FRAME, elem_run_mode,
    elem_spawn_offset_mode, glass_shatter_fx_fallback, glass_shatter_fx_name,
    laser_from_tag_orientation, tail_anchor_origin, tail_sprite_axes, tail_sprite_full_extent,
};
use net::{
    AuthorityLoadHold, CEntity, CEntitySlots, ClientPredictionState, ClientSet, GameActive,
    LastAdoptedSnapshot, LocalPresentClient, PendingPelletFx, PlayerDrawGate, PresentedSnapshot,
    WeaponFirePing, WeaponFirePingBus,
};

use render_fx::{
    CombatFxDump, EntityMarks, FxCameraOrigin, FxDumpRequest, FxFrameOutcome, FxGeneratedFrame,
    FxJournalCursor, FxMarkDvars, FxUnavailableCause, FxWorldColorImages, HostFxDlights,
    HostFxPostLights, HostFxSystem, LaserDvars, PreparedFxCatalog, PreparedFxElemInfos,
    PreparedFxModels, PreparedImpactFx, PreparedTracers, PresentedVehicleFx, PresentedVehicleFxRow,
    TracerDrawGate, TracerWorld, clear_fx_owned_plans, publish_empty_fx_owned_plans,
    tick_tracer_beams,
};

use render_fx::combat::{
    IDENTITY_AXIS, explosion_fx_names, log_combat_fx_gaps, missile_bolt_target,
    play_pellet_segment, play_shell_eject, sync_combat_dump, try_play_weapon_fx_at_origin,
    try_play_weapon_fx_bolted,
};
use render_fx::fire_weapon_fx_should_client_trace;
use render_fx::system::stamp_fx_camera_origin;

use crate::{
    adapters::fx::{
        present::{
            FxDrawCull, FxScene, boot_createfx_effects, build_fx_verts,
            play_named_oriented_at_msec, play_named_oriented_in_world,
            restamp_missing_packed_lighting, spawn_named_oriented_in_world, tick_fx_non_dependent,
            tick_fx_remaining,
        },
        tracer::present_tracer_beams,
        world_mark::FrontendFxScene,
    },
    assemble::drawsurf::{
        FxCodeMeshPlan, FxModelDrawPlan, FxParticleCloudPlan, GfxMarkMeshPlan, GfxMarkSubKey,
        MapOutdoor, MaterialGeneration, tan_half_fov_from_clip,
    },
    prepare::scene::{
        camera::FlyCamera,
        model_lighting_atlas::WorldModelLightingAtlas,
        model_lighting_cache::{
            ModelLightingOwner, ModelLightingRequest, ModelLightingRequests,
            WorldModelLightingCache,
        },
        smodel_geom_cache::LodRampSkinnedDvar,
        world::WorldScene,
    },
};
use weapon_iw4::WEAPTYPE_GRENADE;

// Resolve muzzle tags after this frame's weapon poses, before advancing FX.
/// A bullet hit, and whether it is the hit on the local player himself
/// (his own-hit event, small or large: no impact effect).
#[derive(Message)]
struct BulletHitFx(sim::EntityEventPayload, bool);

struct FxFrameTransaction {
    outcome: FxFrameOutcome,
    _present_span: perf::SpanGuard,
}

#[derive(SystemParam)]
struct FxSceneAccess<'w> {
    scene: Option<Res<'w, WorldScene>>,
    marks: Res<'w, EntityMarks>,
}

impl FxSceneAccess<'_> {
    fn scene(&self) -> Option<&WorldScene> {
        self.scene.as_deref()
    }

    fn marks(&self) -> &EntityMarks {
        &self.marks
    }

    fn view(&self) -> Option<FrontendFxScene<'_>> {
        FrontendFxScene::wrap(self.scene(), self.marks())
    }
}

pub(crate) fn register_combat_fx_systems(app: &mut App) {
    app.add_message::<BulletHitFx>()
        .init_resource::<render_fx::FxModelStaging>()
        // bo2zm: Black Ops II effect sprites and marks for the fallback pass.
        .init_resource::<T6FxMesh>()
        .init_resource::<T6DynLights>()
        .init_resource::<T6MarkMesh>()
        .init_resource::<crate::assemble::drawsurf::GfxGlassMeshPlan>()
        .init_resource::<crate::assemble::drawsurf::GlassTable>()
        .add_systems(Update, latch_authority_load_hold.in_set(ClientSet::Load))
        .add_systems(
            Update,
            (
                boot_createfx_oneshots,
                sync_script_fx,
                crate::assemble::drawsurf::tess::glass::apply_glass_host,
                tick_fx_non_dependent_update,
            )
                .chain()
                .after(stamp_fx_camera_origin)
                .in_set(frame::WorkerCmdSet::FxNonDependent),
        )
        .add_systems(
            Update,
            tick_fx_remaining_update
                .after(frame::WorkerCmdSet::SkinModel)
                .in_set(frame::WorkerCmdSet::FxRemaining),
        )
        .add_systems(
            crate::assemble::StaticSunAndFx,
            (
                generate_fx_transaction.pipe(commit_fx_transaction),
                super::super::motion_tracker::draw_motion_tracker,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (drain_bullet_hit_fx, drain_pellet_fx, present_tracker_light)
                .chain()
                .after(render_anim::occupancy::fpv_present::publish_fpv_dobj_pose)
                .after(frame::WorkerCmdSet::SkinModel)
                .before(frame::WorkerCmdSet::FxNonDependent)
                .in_set(ClientSet::Present),
        )
        .add_systems(
            Update,
            tick_missile_present_state.in_set(ClientSet::Effects),
        )
        .add_observer(fire_weapon)
        .add_observer(eject_brass)
        .add_observer(explosion)
        .add_observer(stop_killcam_explosion_fx)
        .add_observer(play_fx)
        .add_observer(play_fx_bullet_hit)
        .add_observer(melee_blood);
}

#[derive(Default)]
struct TrackerLight {
    owner: Option<(
        frame::WorldGeneration,
        sim::ClientId,
        sim::LifeSequence,
        u32,
    )>,
    bolt: Option<(u32, u16)>,
}

#[allow(clippy::too_many_arguments)]
fn present_tracker_light(
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    generation: Res<frame::WorldGeneration>,
    prepared: Res<render_anim::PreparedFpv>,
    bolts: Res<render_anim::FpvBoltTargets>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    fx_world: FxSceneAccess,
    mut light: Local<TrackerLight>,
) {
    const EFFECT: &str = "misc/light_motion_tracker";
    let owner = presented.snapshot().and_then(|snapshot| {
        let meta = snapshot.meta.for_client(local.0)?;
        let ps = presented.player(local.0)?;
        let weapon = weapon_iw4::get_viewmodel_weapon_index(ps);
        (meta.lifecycle == sim::ClientLifecycle::Alive
            && ps.other_flags & 0x400 == 0
            && prepared.table()?.facts_of(weapon).is_some_and(|facts| {
                facts.motion_tracker
                    || (facts.inventory_type == 3
                        && prepared
                            .table()
                            .and_then(|t| t.facts_of(ps.weapon_primary))
                            .is_some_and(|parent| parent.motion_tracker))
            }))
        .then_some((*generation, local.0, meta.life_sequence, weapon))
    });
    if owner != light.owner {
        if light.owner.is_some_and(|old| old.0 == *generation) {
            if let Some((dobj, bone)) = light.bolt {
                host.0.stop_bolted(EFFECT, dobj, bone);
            }
        }
        light.owner = owner;
        light.bolt = None;
    }
    if owner.is_none() || light.bolt.is_some() {
        return;
    }
    let (Some(target), Some(catalog)) = (bolts.tracker_light, catalog) else {
        return;
    };
    elem_infos.0.sync(&catalog.0);
    if render_fx::present::play_named_bolted_in_world(
        &mut host.0,
        &catalog.0,
        &elem_infos.0,
        asset_game::FxName::engine(EFFECT),
        target,
        fx_world.view().as_ref().map(|scene| scene as &dyn FxScene),
    )
    .and_then(PlayResult::handle)
    .is_some()
    {
        light.bolt = Some((target.dobj, target.bone));
    }
}

fn queue_tag_lasers(
    post_lights: &mut HostFxPostLights,
    fpv_bolts: &crate::adapters::anim::fpv_present::FpvBoltTargets,
    remotes: &Query<&crate::adapters::anim::remote_body::RemoteFxBolts>,
    view: [f32; 3],
    dvars: LaserDvars,
) {
    let push = |post_lights: &mut HostFxPostLights, target: fx::FxBoltTarget, range: f32| {
        let Some(light) = laser_from_tag_orientation(
            target.orientation.origin,
            target.orientation.axis[0],
            view,
            range,
            0.0,
            dvars.end_offset,
            dvars.light,
            1,
            dvars.radius,
        ) else {
            return;
        };
        post_lights.add(light);
    };
    for hand in 0..2 {
        if let Some(target) = fpv_bolts.laser[hand] {
            push(post_lights, target, dvars.range_player);
        }
    }
    for bolts in remotes.iter() {
        if let Some(target) = bolts.laser {
            push(post_lights, target, dvars.range);
        }
    }
}

fn latch_authority_load_hold(
    navigation: Option<Res<frame::BotNavigationReady>>,
    scene: Option<Res<WorldScene>>,
    mut hold: Option<ResMut<AuthorityLoadHold>>,
    headless: Option<Res<frame::Headless>>,
) {
    if let Some(hold) = hold.as_mut() {
        let presenting = headless.is_none() && scene.is_some();
        hold.0 = (presenting && !scene.as_ref().is_some_and(|scene| scene.spawned))
            || navigation.is_some_and(|ready| !ready.0);
    }
}

fn boot_createfx_oneshots(
    emitters: Option<Res<asset_audio::CreateFxOneshotEmitters>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    mut cursor: ResMut<FxJournalCursor>,
    fx_world: FxSceneAccess,
    cgame_active: Res<GameActive>,
    adopted: Option<Res<LastAdoptedSnapshot>>,
) {
    if cursor.createfx_booted {
        return;
    }
    if !cgame_active.get() {
        return;
    }
    let Some(catalog) = catalog else {
        return;
    };
    // bo2zm: a Black Ops II map's scripts are compiled and never run here;
    // its placed effects always boot from the lane's read of them.
    let t6 = catalog.0.map_namespace() == asset_core::AssetNamespace::T6;
    if !t6
        && adopted
            .as_ref()
            .and_then(|a| a.next())
            .is_some_and(|snap| snap.meta.objectives.scripted_effects)
    {
        cursor.createfx_booted = true;
        return;
    }
    let Some(emitters) = emitters else {
        return;
    };
    cursor.createfx_booted = true;
    cursor.createfx_boot_msec = Some(host.0.msec_now);
    if emitters.0.is_empty() {
        return;
    }
    let msec = host.0.msec_now;
    elem_infos.0.sync(&catalog.0);
    let census = boot_createfx_effects(
        &mut host.0,
        &catalog.0,
        &elem_infos.0,
        &emitters.0,
        msec,
        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
    );
    cursor.createfx_miss = cursor.createfx_miss.saturating_add(census.miss_def);
    for failure in &census.failed {
        diag::warn!(World, "fx: CreateFX spawn_oriented failed — {failure}");
    }
    diag::info!(
        World,
        "fx: CreateFX oneshots — named={} held={} miss_def={} (Billboard/Oriented sprites; Beam/Trail/… still gaps)",
        census.named,
        census.held,
        census.miss_def
    );
}

fn tick_fx_non_dependent_update(
    mut host: ResMut<HostFxSystem>,
    dvars: Res<FxMarkDvars>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    prediction: Option<Res<ClientPredictionState>>,
    camera_origin: Res<FxCameraOrigin>,
    fx_world: FxSceneAccess,
    mut post_lights: ResMut<HostFxPostLights>,
    mut aliases: MessageWriter<audio::AliasCommand>,
) {
    let msec = host.0.msec_now;
    let clip_world = prediction
        .as_ref()
        .filter(|p| p.0.is_armed() && p.0.world().has_world_clip())
        .map(|p| p.0.world());
    {
        let glass_clip = clip_world.map(PredictionGlassTrace);
        host.0.glass.advance_in_world(
            msec,
            glass_clip.as_ref().map(|g| g as &dyn fx::GlassWorldTrace),
        );
    }
    let mut shatter_fx = 0u32;
    let mut pending_fx = Vec::new();
    for ev in host.0.glass.take_events() {
        if !ev.play_oneshot {
            continue;
        }
        let landing = ev.landing;
        if !landing {
            if shatter_fx >= FX_GLASS_SHATTER_FX_PER_FRAME {
                continue;
            }
            shatter_fx = shatter_fx.saturating_add(1);
        }
        let (alias, fallback) = if landing {
            ("glass_pane_shatter", "glass_pane_blowout")
        } else {
            glass_break_alias(ev.cause)
        };
        aliases.write(audio::AliasCommand::Play(audio::PlayAlias {
            namespace: asset_core::AssetNamespace::Iw4,
            alias: alias.to_owned(),
            fallback: Some(fallback.to_owned()),
            origin_inches: Some(ev.origin),
            snd_ent: Some(fx_iw4::FX_ENTITYNUM_WORLD),
        }));
        pending_fx.push(ev);
    }
    let Some(catalog) = catalog else {
        return;
    };

    post_lights.queued.clear();
    post_lights.cap_full = 0;
    host.0.mark_receivers = MarkReceiverEnable {
        fx_marks: dvars.fx_marks,
        fx_marks_ents: dvars.fx_marks_ents,
        fx_marks_smodels: dvars.fx_marks_smodels,
    };
    let _fx_update = perf::Span::HostFxUpdateCpuMs.enter();

    elem_infos.0.sync(&catalog.0);
    if !pending_fx.is_empty() {
        let world = fx_world.view();
        let scene = world.as_ref().map(|s| s as &dyn FxScene);
        for ev in pending_fx {
            let axis = axis_from_hit_normal(ev.normal);
            let name = glass_shatter_fx_name(ev.landing);
            let spawned = play_named_oriented_in_world(
                &mut host.0,
                &catalog.0,
                &elem_infos.0,
                asset_game::FxName::engine(name),
                ev.origin,
                axis,
                scene,
            );
            if spawned.is_none()
                && let Some(fallback) = glass_shatter_fx_fallback(ev.landing)
            {
                let _ = play_named_oriented_in_world(
                    &mut host.0,
                    &catalog.0,
                    &elem_infos.0,
                    asset_game::FxName::engine(fallback),
                    ev.origin,
                    axis,
                    scene,
                );
            }
        }
    }
    tick_fx_non_dependent(
        &mut host.0,
        &catalog.0,
        &elem_infos.0,
        msec,
        camera_origin.0,
        clip_world,
        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
    );
}

fn glass_break_alias(cause: u8) -> (&'static str, &'static str) {
    match cause {
        1 => ("glass_pane_blowout", "glass_pane_shatter"),
        2 => ("glass_pane_breakout", "glass_pane_shatter"),
        _ => ("glass_pane_shatter", "glass_pane_blowout"),
    }
}

struct PredictionGlassTrace<'a>(&'a sim::SimWorld);

impl fx::GlassWorldTrace for PredictionGlassTrace<'_> {
    fn sweep(&self, start: [f32; 3], end: [f32; 3]) -> Option<fx::GlassWorldContact> {
        let hit = self
            .0
            .trace_static_world(start, end, [0.0; 3], [0.0; 3], sim::MASK_SHOT);
        if hit.startsolid != 0 {
            return Some(fx::GlassWorldContact {
                fraction: 0.0,
                end: start,
                normal: hit.normal,
                startsolid: true,
                pane: sim::glass_piece_from_hit(hit.hit_type, hit.hit_id),
            });
        }
        if hit.fraction >= 1.0 {
            return None;
        }
        Some(fx::GlassWorldContact {
            fraction: hit.fraction,
            end: hit.endpos,
            normal: hit.normal,
            startsolid: false,
            pane: sim::glass_piece_from_hit(hit.hit_type, hit.hit_id),
        })
    }
}

fn tick_fx_remaining_update(
    mut host: ResMut<HostFxSystem>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    prediction: Option<Res<ClientPredictionState>>,
    camera_origin: Res<FxCameraOrigin>,
    fx_world: FxSceneAccess,
    dobj_poses: Option<Res<crate::adapters::anim::dobj_pose::HostDObjPoseFrame>>,
) {
    let msec = host.0.msec_now;
    let Some(catalog) = catalog else {
        return;
    };
    let _fx_update = perf::Span::HostFxUpdateCpuMs.enter();
    let clip_world = prediction
        .as_ref()
        .filter(|p| p.0.is_armed() && p.0.world().has_world_clip())
        .map(|p| p.0.world());
    host.0.refresh_bolt_poses(|dobj, bone| {
        dobj_poses
            .as_deref()
            .and_then(|poses| poses.resolve_live_bolt(dobj, bone))
    });
    elem_infos.0.sync(&catalog.0);
    tick_fx_remaining(
        &mut host.0,
        &catalog.0,
        &elem_infos.0,
        msec,
        camera_origin.0,
        clip_world,
        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
    );
    perf::Counter::CounterFxElemLive.emit(f64::from(host.0.elem_live_count));
    perf::Counter::CounterFxElemAllocFail.emit(f64::from(host.0.elem_alloc_failures));
    // bo2zm: the effect pools every 5 s of game: parts alive (most seen), of
    // the pool; effects; parts refused since the last line.
    static POOL: std::sync::Mutex<(i32, u32, u32)> = std::sync::Mutex::new((0, 0, 0));
    if let Ok(mut p) = POOL.lock() {
        p.1 = p.1.max(host.0.elem_live_count);
        if msec.wrapping_sub(p.0) >= 5000 || msec < p.0 {
            diag::info!(
                World,
                "bo2zm fx pool: {} parts alive (most {}) of {}, {} effects, {} parts refused",
                host.0.elem_live_count,
                p.1,
                fx_iw4::FX_ELEM_POOL_CAPACITY,
                host.0.ring_occupancy(),
                host.0.elem_alloc_failures.wrapping_sub(p.2)
            );
            *p = (msec, 0, host.0.elem_alloc_failures);
        }
    }
}

#[derive(SystemParam)]
struct FxPresentEnv<'w> {
    outdoor: Option<Res<'w, MapOutdoor>>,
    dlights: ResMut<'w, HostFxDlights>,
    post_lights: ResMut<'w, HostFxPostLights>,
    models: Option<Res<'w, PreparedFxModels>>,
    model_geometry: Option<Res<'w, render_fx::PreparedFxModelGeometry>>,
    atlas: Option<Res<'w, WorldModelLightingAtlas>>,
    atpoint: Res<'w, render_scene::DynAtPointLookup>,
    lod_skinned: Res<'w, LodRampSkinnedDvar>,
    staged_models: ResMut<'w, render_fx::FxModelStaging>,
}

#[allow(clippy::too_many_arguments)]
fn generate_fx_transaction(
    _main_thread: bevy::ecs::system::NonSendMarker,
    mut host: ResMut<HostFxSystem>,
    catalog: Option<Res<PreparedFxCatalog>>,
    dvars: Res<FxMarkDvars>,
    laser_dvars: Res<LaserDvars>,
    cam_q: Query<&Transform, With<FlyCamera>>,
    fx_world: FxSceneAccess,
    scene_view: Option<Res<crate::prepare::scene::view_parms::PreparedSceneView>>,
    mut post_lights: ResMut<HostFxPostLights>,
    mut tracers: ResMut<TracerWorld>,
    mark_models: Option<Res<asset_world::MapXModelSceneCatalog>>,
    mark_owners: Query<(
        &crate::prepare::scene::world::WorldScriptModelInstance,
        &Transform,
        &Visibility,
    )>,
    xanims: Option<Res<assets::PreparedXAnims>>,
    fpv_bolts: Res<crate::adapters::anim::fpv_present::FpvBoltTargets>,
    remotes: Query<&crate::adapters::anim::remote_body::RemoteFxBolts>,
) -> FxFrameTransaction {
    let present_span = perf::Span::HostFxPresentCpuMs.enter();
    let Some(catalog) = catalog else {
        return FxFrameTransaction {
            outcome: FxFrameOutcome::Unavailable(FxUnavailableCause::CatalogAbsent),
            _present_span: present_span,
        };
    };
    let cam_tf = cam_q.iter().next().copied().unwrap_or(Transform::IDENTITY);
    queue_tag_lasers(
        &mut post_lights,
        &fpv_bolts,
        &remotes,
        cam_tf.translation.to_array(),
        *laser_dvars,
    );
    host.0.generate_world_mark_verts();
    if let (Some(scene), Some(models)) = (fx_world.scene(), mark_models.as_deref()) {
        super::entity_mark::generate(
            &mut host.0,
            scene,
            fx_world.marks(),
            &mark_owners,
            xanims.as_deref(),
            models,
        );
    }

    restamp_missing_packed_lighting(
        &mut host.0,
        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
    );
    let frustum_planes = scene_view
        .as_ref()
        .map(|view| view.frustum_planes.to_vec())
        .unwrap_or_default();
    let (out, verts_gaps) = build_fx_verts(
        &mut host.0,
        &catalog.0,
        FxDrawCull {
            elem_draw: dvars.fx_cull_elem_draw,
            planes: &frustum_planes,
        },
        cam_tf.translation.to_array(),
    );
    for def_index in &verts_gaps.lighting_frac_def_indices {
        host.0.gaps.raise(FxGapCause::NoLightGridSample {
            def_index: *def_index,
        });
    }
    let mut vis_write = host.0.vis_blocker_write;
    let mut vis_read = host.0.vis_blocker_read;
    fx_iw4::vis_blocker_generate_verts(&mut vis_write, &mut vis_read);
    host.0.vis_blocker_write = vis_write;
    host.0.vis_blocker_read = vis_read;
    tick_tracer_beams(&mut tracers, FxMsec(host.0.msec_now));

    FxFrameTransaction {
        outcome: FxFrameOutcome::Generated(FxGeneratedFrame {
            out,
            verts_gaps,
            mark_mesh: host.0.last_mark_mesh.take(),
            post_lights: std::mem::take(&mut post_lights.queued),
            tracers: std::mem::take(&mut *tracers),
            cam_tf,
            frustum_planes,
            clip_from_world: scene_view
                .as_ref()
                .map(|view| view.clip_from_world.to_cols_array()),
            tan_half_fov: scene_view
                .as_ref()
                .map(|view| tan_half_fov_from_clip(view.clip_from_view)),
            world_present: fx_world.scene().is_some_and(|scene| scene.spawned),
        }),
        _present_span: present_span,
    }
}

/// bo2zm: Black Ops II effect sprites. Their materials have no technique,
/// so the code mesh never draws them; the renderer's fallback pass does,
/// from these world-space quads (float texcoords, sprite colour), per draw
/// `(first index, index count, catalog material)`.
#[derive(Resource, Clone, Debug, Default)]
pub struct T6FxMesh {
    pub vertices: Vec<render_frame::SmodelVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<(u32, u32, u32)>,
    /// The same for elements drawn with the first-person gun.
    pub viewmodel_draws: Vec<(u32, u32, u32)>,
    /// Particle clouds (embers, ash): each draws the shared box of points
    /// (appended once a frame) with its own placement, colour and size.
    pub clouds: Vec<T6CloudDraw>,
    /// The box's first index this frame, once a cloud has appended it.
    cloud_box: Option<u32>,
}

/// bo2zm: one Black Ops II particle cloud. Its vertex shader
/// (`particlecloud_*`, fxc disassembly) takes each point of the cloud's
/// box to the world by the cloud's placement, then out to a quad facing the
/// camera: the corner's texcoord less one half times `particleCloudMatrix`
/// (the particle's width and height, as the engine packs it); its pixel
/// shader is texture times `particleCloudColor`, the colour halved.
#[derive(Clone, Debug)]
pub struct T6CloudDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u32,
    /// World from the box (columns): the cloud's rotation, scale, origin.
    pub world_from_local: [f32; 16],
    /// The cloud's colour (rgba, 0..1), lit.
    pub color: [f32; 4],
    /// The particle's width and height.
    pub size: [f32; 2],
}

/// The particle cloud box: 8 x 8 x 16 cells of the unit cube, each point
/// jittered in its cell by the C runtime's `rand` from its default seed,
/// nearest the centre first (a cloud's shape keeps the innermost ones), as
/// the engine builds its cloud vertex buffer.
fn t6_cloud_box() -> &'static [[f32; 3]] {
    static BOX: std::sync::OnceLock<Vec<[f32; 3]>> = std::sync::OnceLock::new();
    BOX.get_or_init(|| {
        let mut seed = fx_iw4::MSVCRT_HOLDRAND_DEFAULT;
        let mut cells = Vec::with_capacity(fx_iw4::FX_PARTICLE_CLOUD_TEMPLATE_CELLS);
        for x in 0..fx_iw4::FX_PARTICLE_CLOUD_GRID_X {
            for y in 0..fx_iw4::FX_PARTICLE_CLOUD_GRID_Y {
                for z in 0..fx_iw4::FX_PARTICLE_CLOUD_GRID_Z {
                    let jitter = [
                        fx_iw4::msvcrt_rand01(&mut seed),
                        fx_iw4::msvcrt_rand01(&mut seed),
                        fx_iw4::msvcrt_rand01(&mut seed),
                    ];
                    cells.push(fx_iw4::particle_cloud_cell_xyz(x, y, z, jitter));
                }
            }
        }
        cells.sort_by(|a, b| {
            fx_iw4::particle_cloud_cell_radius_sq(*a)
                .total_cmp(&fx_iw4::particle_cloud_cell_radius_sq(*b))
        });
        cells
    })
}

/// T6 element flag: drawn with the viewmodel (the parts of the game's
/// first-person gas flashes carry it; their smoke and the Ray Gun's
/// first-person flash do not).
const T6_FX_ELEM_DRAW_WITH_VIEWMODEL: i32 = 0x1000;
/// `asset_game`'s T6 line element: a tail ahead of its part.
const T6_FX_ELEM_LINE: i32 = 0x2000_0000;

impl T6FxMesh {
    fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.draws.clear();
        self.viewmodel_draws.clear();
        self.clouds.clear();
        self.cloud_box = None;
    }

    /// bo2zm: one effect model (shell casing, debris), rigid: its bind
    /// pose placed by the element (origin, axis rows, scale), per surface
    /// with its material.
    pub(crate) fn push_model(
        &mut self,
        skel: &asset_model::ModelSkel,
        material: impl Fn(usize) -> Option<u32>,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        scale: f32,
    ) {
        let o = Vec3::from_array(origin);
        let [ax, ay, az] = axis.map(Vec3::from_array);
        let s = if scale > 0.0 { scale } else { 1.0 };
        let ranges = skel
            .surface_vertex_ranges
            .iter()
            .zip(&skel.surface_index_ranges);
        for (surface, (&(vbase, vn), &(ibase, icount))) in ranges.enumerate() {
            let Some(mat) = material(surface) else {
                continue;
            };
            if vbase + vn > skel.positions.len() || ibase + icount > skel.indices.len() {
                continue;
            }
            let base = self.vertices.len() as u32;
            for v in vbase..vbase + vn {
                let p = skel.positions[v];
                let n = skel.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]);
                let world = o + (ax * p[0] + ay * p[1] + az * p[2]) * s;
                let normal = (ax * n[0] + ay * n[1] + az * n[2]).normalize_or_zero();
                self.vertices.push(render_frame::SmodelVertex {
                    position: world.to_array(),
                    normal: normal.to_array(),
                    color: skel.colors.get(v).copied().unwrap_or([1.0; 4]),
                    uv0: skel.uvs.get(v).copied().unwrap_or([0.0; 2]),
                });
            }
            let start = self.indices.len() as u32;
            for &i in &skel.indices[ibase..ibase + icount] {
                self.indices.push(base + i.saturating_sub(vbase as u32));
            }
            self.draws.push((start, icount as u32, mat));
        }
    }

    /// bo2zm: one tracer beam (world-space strip, its own colours).
    pub(crate) fn push_beam(
        &mut self,
        verts: &[fx_iw4::FxBeamVert],
        indices: &[u16],
        material: u32,
    ) {
        let base = self.vertices.len() as u32;
        let start = self.indices.len() as u32;
        for v in verts {
            self.vertices.push(render_frame::SmodelVertex {
                position: v.xyz,
                normal: [0.0, 0.0, 1.0],
                color: v.color.map(|c| c.clamp(0.0, 1.0)),
                uv0: v.uv,
            });
        }
        self.indices
            .extend(indices.iter().map(|&i| base + u32::from(i)));
        let count = self.indices.len() as u32 - start;
        if count > 0 {
            self.draws.push((start, count, material));
        }
    }

    /// One particle cloud: its draw over the shared box (appended the first
    /// time a frame), as many of the innermost points as it has particles
    /// (a BO2 cloud's own count, else what its shape keeps).
    fn push_cloud(
        &mut self,
        cloud: &fx_iw4::GfxParticleCloud,
        particles: Option<u32>,
        material: u32,
    ) {
        let first_index = match self.cloud_box {
            Some(first) => first,
            None => {
                let first = self.indices.len() as u32;
                let base = self.vertices.len() as u32;
                for (cell, xyz) in t6_cloud_box().iter().enumerate() {
                    for uv in fx_iw4::FX_PARTICLE_CLOUD_UV {
                        self.vertices.push(render_frame::SmodelVertex {
                            position: *xyz,
                            normal: [0.0, 0.0, 1.0],
                            color: [1.0; 4],
                            uv0: uv,
                        });
                    }
                    let at = base + cell as u32 * 4;
                    for i in fx_iw4::FX_PARTICLE_CLOUD_QUAD_INDICES {
                        self.indices.push(at + u32::from(i));
                    }
                }
                self.cloud_box = Some(first);
                first
            }
        };
        let cells = particles
            .unwrap_or(fx_iw4::particle_cloud_draw_cell_count(cloud.flags) as u32)
            .min(fx_iw4::FX_PARTICLE_CLOUD_TEMPLATE_CELLS as u32);
        let world_from_local = Mat4::from_scale_rotation_translation(
            Vec3::splat(cloud.placement_scale),
            Quat::from_array(cloud.quat).normalize(),
            Vec3::from_array(cloud.pos),
        );
        self.clouds.push(T6CloudDraw {
            first_index,
            index_count: cells * 6,
            material,
            world_from_local: world_from_local.to_cols_array(),
            color: fx_iw4::particle_cloud_color_const(cloud.color),
            size: [cloud.size0, cloud.size1],
        });
    }

    fn push_quad(
        &mut self,
        transform: Transform,
        color_rgba: [u8; 4],
        uv: fx_iw4::FxSpriteAtlasUv,
    ) {
        let base = self.vertices.len() as u32;
        let normal = (transform.rotation * Vec3::Z).normalize_or_zero();
        let corners = uv.corners();
        for (i, xy) in fx_iw4::FX_SPRITE_QUAD_LOCAL_XY.iter().enumerate() {
            let world = transform.transform_point(Vec3::new(xy[0], xy[1], 0.0));
            self.vertices.push(render_frame::SmodelVertex {
                position: world.to_array(),
                normal: normal.to_array(),
                color: color_rgba.map(|c| f32::from(c) / 255.0),
                uv0: corners[i],
            });
        }
        for i in fx_iw4::FX_SPRITE_QUAD_INDICES {
            self.indices.push(base + u32::from(i));
        }
    }
}

/// bo2zm: the lights effects throw this frame (muzzle flashes, explosions,
/// impact flashes), nearest the eye first, at most `T6_DYN_LIGHTS`: per
/// light its origin and radius, then its colour (linear, rgb).
#[derive(Resource, Clone, Debug, Default)]
pub struct T6DynLights {
    pub lights: Vec<[f32; 8]>,
}

/// The most effect lights the fallback pass reads.
pub const T6_DYN_LIGHTS: usize = 16;

/// bo2zm: Black Ops II decal marks (bullet holes, blood): world-space
/// triangles with the mark's own texcoords, per draw `(first index, index
/// count, catalog material, first vertex, lightmap light)`. A mark on a
/// lightmapped wall is lit as the wall is (BO2's `wc_` mark shaders read the
/// wall's lightmap): its vertex colours then hold that light ((c^2) * 32,
/// times the mark's tint), and the last field its primary light and that
/// light's baked visibility (times the tint); `None` = lit by the light grid
/// where it landed.
#[derive(Resource, Clone, Debug, Default)]
pub struct T6MarkMesh {
    pub vertices: Vec<render_frame::SmodelVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<(u32, u32, u32, [f32; 3], Option<(u8, f32)>)>,
}

/// bo2zm: a Black Ops II material (it draws in the fallback pass).
fn t6_fx_material(runtime: &MaterialGeneration, asset_id: usize) -> bool {
    runtime
        .catalog
        .derived(assets::MaterialIndex::from_order(asset_id))
        .is_some_and(|m| m.t6_draw.is_some())
}

/// bo2zm test aid: `IW4L_T6_FX_ONLY=a,b` draws only effect materials whose
/// name contains one of them; `IW4L_T6_FX_SKIP=a,b` hides those.
fn t6_fx_filtered_out(runtime: &MaterialGeneration, asset_id: usize) -> bool {
    static FILTER: std::sync::OnceLock<(Vec<String>, Vec<String>)> = std::sync::OnceLock::new();
    let (only, skip) = FILTER.get_or_init(|| {
        let list = |var: &str| -> Vec<String> {
            std::env::var(var)
                .map(|v| {
                    v.split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        (list("IW4L_T6_FX_ONLY"), list("IW4L_T6_FX_SKIP"))
    });
    if only.is_empty() && skip.is_empty() {
        return false;
    }
    let Some(m) = runtime
        .catalog
        .derived(assets::MaterialIndex::from_order(asset_id))
    else {
        return false;
    };
    (!only.is_empty() && !only.iter().any(|w| m.name.contains(w.as_str())))
        || skip.iter().any(|w| m.name.contains(w.as_str()))
}

fn commit_fx_transaction(
    In(transaction): In<FxFrameTransaction>,
    mut host: ResMut<HostFxSystem>,
    catalog: Option<Res<PreparedFxCatalog>>,
    color_images: Res<FxWorldColorImages>,
    runtime: Res<MaterialGeneration>,
    scene: Option<Res<WorldScene>>,
    mut cursor: ResMut<FxJournalCursor>,
    (mut plan, mut t6_fx, mut t6_marks, mut t6_lights): (
        ResMut<FxCodeMeshPlan>,
        ResMut<T6FxMesh>,
        ResMut<T6MarkMesh>,
        ResMut<T6DynLights>,
    ),
    mut spark_plan: ResMut<FxParticleCloudPlan>,
    mut mark_plan: ResMut<GfxMarkMeshPlan>,
    mut model_plan: ResMut<FxModelDrawPlan>,
    mut dump_req: ResMut<FxDumpRequest>,
    mut tracer_world: ResMut<TracerWorld>,
    mut combat: ResMut<CombatFxDump>,
    mut env: FxPresentEnv,
    lighting_cache: Option<Res<WorldModelLightingCache>>,
    mut lighting_requests: ResMut<ModelLightingRequests>,
) {
    let FxFrameTransaction {
        outcome,
        _present_span,
    } = transaction;
    clear_fx_owned_plans(&mut plan, &mut spark_plan, &mut mark_plan);
    t6_fx.clear();
    env.staged_models.0.clear();
    if let Some(prepared) = &env.model_geometry {
        env.staged_models.0.use_prepared(&prepared.0);
    } else {
        env.staged_models.0 = FxModelDrawPlan::default();
    }
    env.dlights.scene.clear();
    env.dlights.cap_full = 0;
    env.post_lights.queued.clear();
    env.post_lights.cap_full = 0;
    env.post_lights.drawn = 0;
    env.post_lights.skipped_short = 0;
    env.post_lights.miss_material = 0;

    let generated = match outcome {
        FxFrameOutcome::Generated(generated) => generated,
        FxFrameOutcome::Unavailable(FxUnavailableCause::CatalogAbsent) => {
            host.0.last_mark_mesh = None;
            fill_mark_mesh_plan(&mut host.0, None, &mut mark_plan, &runtime);
            publish_empty_fx_owned_plans(&mut plan, &mut spark_plan);
            return;
        }
    };
    let FxGeneratedFrame {
        out,
        verts_gaps,
        mark_mesh,
        post_lights,
        mut tracers,
        cam_tf,
        frustum_planes,
        clip_from_world,
        tan_half_fov,
        world_present,
    } = generated;
    let Some(catalog) = catalog else {
        *tracer_world = tracers;
        fill_mark_mesh_plan(&mut host.0, None, &mut mark_plan, &runtime);
        publish_empty_fx_owned_plans(&mut plan, &mut spark_plan);
        return;
    };
    let mut zero_half = 0u32;

    take_t6_marks(&host.0, mark_mesh.as_ref(), &runtime, &mut t6_marks);
    fill_mark_mesh_plan(&mut host.0, mark_mesh, &mut mark_plan, &runtime);
    let planes = frustum_planes.as_slice();
    // bo2zm: Black Ops II effect models (shell casings, debris) go to the
    // fallback pass, posed here; the rest to the model plan.
    let fx_model_catalog = env.models.as_deref().map(|prepared| &prepared.0);
    let mut iw_models: Vec<fx::FxModelInstance> = Vec::with_capacity(out.models.len());
    for m in &out.models {
        let entry = fx_model_catalog.and_then(|catalog| catalog.get_at(m.model_index));
        // bo2mp: Black Ops II's `fx_decal_*` models are decal volumes (a
        // box BO2 projects blood onto the body inside it); drawn as a model
        // the box shows as a black block on the body. Not drawn.
        if entry.is_some_and(|e| e.skel.name.starts_with("fx_decal")) {
            continue;
        }
        let t6 = entry.is_some_and(|e| {
            (0..e.skel.surface_materials.len()).any(|s| {
                e.material_index(s)
                    .is_some_and(|mi| t6_fx_material(&runtime, mi.order()))
            })
        });
        match entry {
            Some(e) if t6 => t6_fx.push_model(
                &e.skel,
                |s| {
                    e.material_index(s)
                        .map(|mi| mi.order())
                        .filter(|&mi| t6_fx_material(&runtime, mi))
                        .map(|mi| mi as u32)
                },
                m.origin,
                m.axis,
                m.scale,
            ),
            _ => iw_models.push(m.clone()),
        }
    }
    fill_fx_model_plan(
        &iw_models,
        env.models.as_deref().map(|prepared| &prepared.0),
        env.atlas.as_deref(),
        scene.as_deref(),
        &env.atpoint,
        cam_tf.translation,
        planes,
        env.lod_skinned.args(),
        lighting_cache.as_deref(),
        &mut lighting_requests,
        &mut env.staged_models.0,
    );
    model_plan.publish_rebuild(&mut env.staged_models.0);
    let spot_cone = lighting_iw4::SpotLightConeDvars::register_defaults();
    for light in &out.spot_lights {
        match lighting_iw4::add_omni_light_to_scene_allows(
            world_present,
            light.radius,
            env.dlights.scene.len() as u32,
        ) {
            Ok(()) => env.dlights.scene.push(lighting_iw4::spot_light_pack(
                light.origin,
                light.axis[0],
                light.radius,
                light.color_bgr,
                spot_cone,
            )),
            Err(lighting_iw4::AddOmniLightRefuse::Cap) => {
                env.dlights.cap_full = env.dlights.cap_full.saturating_add(1);
            }
            Err(_) => {}
        }
    }
    for light in &out.omni_lights {
        match lighting_iw4::add_omni_light_to_scene_allows(
            world_present,
            light.radius,
            env.dlights.scene.len() as u32,
        ) {
            Ok(()) => env.dlights.scene.push(lighting_iw4::omni_light_pack(
                light.origin,
                light.radius,
                light.color_bgr,
            )),
            Err(lighting_iw4::AddOmniLightRefuse::Cap) => {
                env.dlights.cap_full = env.dlights.cap_full.saturating_add(1);
            }
            Err(_) => {}
        }
    }
    // bo2zm: the omni lights for the fallback pass, nearest the eye first.
    t6_lights.lights.clear();
    {
        let mut near: Vec<(f32, [f32; 8])> = out
            .omni_lights
            .iter()
            .filter(|l| l.radius > 0.0 && l.color_bgr.iter().any(|c| *c > 0.0))
            .map(|l| {
                let d = Vec3::from_array(l.origin).distance(cam_tf.translation);
                let [b, g, r] = l.color_bgr;
                (
                    d - l.radius,
                    [
                        l.origin[0],
                        l.origin[1],
                        l.origin[2],
                        l.radius,
                        r,
                        g,
                        b,
                        0.0,
                    ],
                )
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        t6_lights
            .lights
            .extend(near.into_iter().take(T6_DYN_LIGHTS).map(|(_, l)| l));
    }
    let eye = cam_tf.translation.to_array();
    for light in &post_lights {
        let Some(tess) = fx_iw4::post_light_generate_verts(light, eye) else {
            env.post_lights.skipped_short = env.post_lights.skipped_short.saturating_add(1);
            continue;
        };
        match code_mesh_bind(light.material_name, &color_images, &runtime) {
            FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap) => {
                env.post_lights.miss_material = env.post_lights.miss_material.saturating_add(1);
                plan.miss_material = plan.miss_material.saturating_add(1);
                continue;
            }
            FxCodeMeshBind::Skip(cause) => {
                count_fx_present_skip(&mut cursor, cause);
                env.post_lights.miss_material = env.post_lights.miss_material.saturating_add(1);
                continue;
            }
            FxCodeMeshBind::Ready {
                color,
                sort_key,
                ordinal,
            } => {
                let slot = plan.begin_material_draw(color, sort_key, Some(ordinal));
                if !plan.push_post_light(&tess) {
                    plan.end_material_draw(slot);
                    continue;
                }
                plan.end_material_draw(slot);
                env.post_lights.drawn = env.post_lights.drawn.saturating_add(1);
            }
        }
    }

    // bo2zm test aid: IW4L_FX_SPRITE_BIG=<min size>: every 10th frame, each
    // sprite in front of the eye at least that big (effect, part, distance,
    // size, place, colour).
    {
        static BIG: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
        static FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if let Some(min) = *BIG.get_or_init(|| {
            std::env::var("IW4L_FX_SPRITE_BIG")
                .ok()
                .and_then(|v| v.parse().ok())
        }) && FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 10 == 0
        {
            let eye = cam_tf.translation;
            let fwd = cam_tf.forward().as_vec3();
            let rows: Vec<String> = out
                .sprites
                .iter()
                .filter(|s| s.size0.max(s.size1) >= min)
                .filter(|s| (Vec3::from_array(s.origin) - eye).dot(fwd) > 0.0)
                .map(|s| {
                    let rel = Vec3::from_array(s.origin) - eye;
                    format!(
                        "{} e{} t{} d{:.0} size {:.0}/{:.0} rgba {:?} at ({:.0} {:.0} {:.0})",
                        s.def_name,
                        s.def_index,
                        s.elem_type,
                        rel.length(),
                        s.size0,
                        s.size1,
                        s.color_rgba,
                        s.origin[0],
                        s.origin[1],
                        s.origin[2]
                    )
                })
                .collect();
            if !rows.is_empty() {
                diag::info!(World, "fx big sprites: {}", rows.join(" | "));
            }
        }
    }
    // bo2zm test aid: IW4L_FX_SPRITE_LOG=<effect name part> logs that
    // effect's sprites each frame (element, place against the eye, size,
    // colour).
    {
        static WANT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        if let Some(want) = WANT.get_or_init(|| std::env::var("IW4L_FX_SPRITE_LOG").ok()) {
            let eye = cam_tf.translation;
            let rows: Vec<String> = out
                .sprites
                .iter()
                .filter(|s| s.def_name.contains(want.as_str()))
                .map(|s| {
                    let rel = Vec3::from_array(s.origin) - eye;
                    format!(
                        "e{} t{} d{:.0} fwd{:.0} size {:.1}/{:.1} rgba {:?} flags {:#x} at ({:.1} {:.1} {:.1}) dir ({:.2} {:.2} {:.2})",
                        s.def_index,
                        s.elem_type,
                        rel.length(),
                        rel.dot(cam_tf.forward().as_vec3()),
                        s.size0,
                        s.size1,
                        s.color_rgba,
                        s.flags,
                        s.origin[0],
                        s.origin[1],
                        s.origin[2],
                        s.vel_dir[0],
                        s.vel_dir[1],
                        s.vel_dir[2]
                    )
                })
                .collect();
            if !rows.is_empty() {
                diag::info!(World, "fx sprites {want}: {}", rows.join(" | "));
            }
            // IW4L_FX_SPRITE_LOG_STATE=1: every frame, the clock and each
            // matching effect's state and live parts (begin, life).
            if std::env::var_os("IW4L_FX_SPRITE_LOG_STATE").is_some() {
                let now = host.0.msec_now;
                let mut effects = Vec::new();
                for slot in 0..fx_iw4::FX_EFFECT_POOL_CAPACITY as usize {
                    let Some(e) = host.0.effect_at(slot) else {
                        break;
                    };
                    if !e.ring_resident || !e.def_name.contains(want.as_str()) {
                        continue;
                    }
                    let parts: Vec<String> = host
                        .0
                        .live_elems()
                        .filter(|el| usize::from(el.owner_effect_slot) == slot)
                        .map(|el| {
                            format!(
                                "e{}@{}+{}",
                                el.def_index,
                                el.msec_begin - now,
                                el.life_span_msec
                            )
                        })
                        .collect();
                    effects.push(format!(
                        "slot {slot} status {:#x} begun {} updated {} parts [{}]",
                        e.status,
                        e.msec_begin - now,
                        e.msec_last_update - now,
                        parts.join(" ")
                    ));
                }
                // Every 2 s: the effects holding the most live parts.
                if now % 2000 < 8 {
                    let mut by: HashMap<usize, usize> = HashMap::new();
                    for el in host.0.live_elems() {
                        *by.entry(usize::from(el.owner_effect_slot)).or_default() += 1;
                    }
                    let mut named: HashMap<String, (usize, usize)> = HashMap::new();
                    for (slot, n) in by {
                        let name = host
                            .0
                            .effect_at(slot)
                            .map_or("?".to_owned(), |e| e.def_name.clone());
                        let row = named.entry(name).or_default();
                        row.0 += n;
                        row.1 += 1;
                    }
                    let mut top: Vec<(String, (usize, usize))> = named.into_iter().collect();
                    top.sort_by(|a, b| b.1.0.cmp(&a.1.0));
                    let rows: Vec<String> = top
                        .iter()
                        .take(12)
                        .map(|(n, (p, e))| format!("{n} {p} parts/{e} effects"))
                        .collect();
                    diag::info!(World, "fx pool at {now}: {}", rows.join(", "));
                }
                diag::info!(
                    World,
                    "fx state {want} at {now} (parts live {} failed {}): {} sprites; {}",
                    host.0.elem_live_count,
                    host.0.elem_alloc_failures,
                    rows.len(),
                    effects.join(" | ")
                );
            }
        }
    }
    // bo2zm M3 fix list 2: Black Ops II's parts draw in the effects
    // system's own order (its sorted lists: each effect's parts by their
    // authored order, then distance), one draw per run of parts sharing a
    // material; the renderer then orders draws by the material's sort key
    // alone. Before, they were gathered per material in a hash map whose
    // order changes every frame, so overlapping layers (a fire's flames and
    // smoke, the nuke cloud's glow and soot, the box light) swapped places
    // frame to frame: the flicker. Other games' materials still gather per
    // material, in first-seen order.
    let mut batches: Vec<(usize, Vec<&FxSpriteInstance>)> = Vec::new();
    let mut gathered: HashMap<usize, usize> = HashMap::new();
    let sprites_total = out.sprites.len();
    for sprite in &out.sprites {
        let Some(asset_id) = sprite.material_index else {
            cursor.draw_miss_material = cursor.draw_miss_material.saturating_add(1);
            plan.miss_material = plan.miss_material.saturating_add(1);
            continue;
        };
        if t6_fx_material(&runtime, asset_id) {
            match batches.last_mut() {
                Some((id, run)) if *id == asset_id => run.push(sprite),
                _ => batches.push((asset_id, vec![sprite])),
            }
        } else if let Some(&i) = gathered.get(&asset_id) {
            batches[i].1.push(sprite);
        } else {
            gathered.insert(asset_id, batches.len());
            batches.push((asset_id, vec![sprite]));
        }
    }

    let mut drawn = 0usize;
    let mut alpha_min = u8::MAX;
    let mut alpha_max = 0u8;
    let mut size_min = f32::MAX;
    let mut size_max = 0.0f32;
    let mut dist_min = f32::MAX;
    let mut dist_max = 0.0f32;
    let mut in_front = 0u32;
    let mut behind = 0u32;
    let cam_fwd = cam_tf.forward();
    // bo2zm test aid: IW4L_FX_RED=<effect name part> draws that effect's
    // sprites opaque red.
    static RED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let red = RED.get_or_init(|| std::env::var("IW4L_FX_RED").ok());
    for (asset_id, sprites) in &batches {
        // bo2zm: Black Ops II sprites go to the fallback pass.
        if t6_fx_material(&runtime, *asset_id) {
            if t6_fx_filtered_out(&runtime, *asset_id) {
                continue;
            }
            for viewmodel in [false, true] {
                let start = t6_fx.indices.len() as u32;
                for sprite in sprites
                    .iter()
                    .filter(|s| (s.flags & T6_FX_ELEM_DRAW_WITH_VIEWMODEL != 0) == viewmodel)
                {
                    let colour = match red {
                        Some(want) if sprite.def_name.contains(want.as_str()) => [255, 0, 0, 255],
                        _ => sprite.color_rgba,
                    };
                    t6_fx.push_quad(sprite_transform(sprite, &cam_tf), colour, sprite.atlas);
                    drawn = drawn.saturating_add(1);
                }
                let count = t6_fx.indices.len() as u32 - start;
                if count > 0 {
                    let draw = (start, count, *asset_id as u32);
                    if viewmodel {
                        t6_fx.viewmodel_draws.push(draw);
                    } else {
                        t6_fx.draws.push(draw);
                    }
                }
            }
            continue;
        }
        match code_mesh_bind_asset(*asset_id, &color_images, &runtime) {
            FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap) => {
                let n = sprites.len() as u32;
                cursor.draw_miss_material = cursor.draw_miss_material.saturating_add(n);
                plan.miss_material = plan.miss_material.saturating_add(n);
                if cursor.draw_miss_material == n {
                    let name = sprites
                        .first()
                        .map(|s| s.material_name.as_ref())
                        .unwrap_or("?");
                    diag::warn!(
                        World,
                        "fx: sprite Bound `{asset_id}` (`{name}`) not in colors_by_asset — further misses counted"
                    );
                }
            }
            FxCodeMeshBind::Skip(cause) => count_fx_present_skip(&mut cursor, cause),
            FxCodeMeshBind::Ready {
                color,
                sort_key,
                ordinal,
            } => {
                if sort_key == 0 && ordinal == 0 {
                    zero_half = zero_half.saturating_add(1);
                }
                let slot = plan.begin_material_draw(color, sort_key, Some(ordinal));
                for sprite in sprites {
                    if !plan.push_quad(
                        sprite_transform(sprite, &cam_tf),
                        sprite.color_rgba,
                        sprite.atlas,
                    ) {
                        continue;
                    }
                    drawn = drawn.saturating_add(1);
                    let alpha = sprite.color_rgba[3];
                    alpha_min = alpha_min.min(alpha);
                    alpha_max = alpha_max.max(alpha);
                    size_min = size_min.min(sprite.size0);
                    size_max = size_max.max(sprite.size0);
                    let delta = Vec3::from_array(sprite.origin) - cam_tf.translation;
                    let dist = delta.length();
                    dist_min = dist_min.min(dist);
                    dist_max = dist_max.max(dist);
                    if delta.dot(*cam_fwd) > 0.0 {
                        in_front = in_front.saturating_add(1);
                    } else {
                        behind = behind.saturating_add(1);
                    }
                }
                plan.end_material_draw(slot);
            }
        }
    }

    let mut trails_drawn = 0usize;
    for mesh in &out.trail_meshes {
        let Some(asset_id) = mesh.material_index else {
            host.0.gaps.raise(FxGapCause::TrailCodeMeshRefused {
                step: CodeMeshStep::Bind,
            });
            cursor.draw_miss_material = cursor.draw_miss_material.saturating_add(1);
            continue;
        };
        match code_mesh_bind_asset(asset_id, &color_images, &runtime) {
            FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap) => {
                host.0.gaps.raise(FxGapCause::TrailCodeMeshRefused {
                    step: CodeMeshStep::Bind,
                });
                cursor.draw_miss_material = cursor.draw_miss_material.saturating_add(1);
            }
            FxCodeMeshBind::Skip(cause) => count_fx_present_skip(&mut cursor, cause),
            FxCodeMeshBind::Ready {
                color,
                sort_key,
                ordinal,
            } => {
                if sort_key == 0 && ordinal == 0 {
                    zero_half = zero_half.saturating_add(1);
                }
                let slot = plan.begin_material_draw(color, sort_key, Some(ordinal));
                let vert_used = plan.mesh.vert_used;
                let index_used = plan.mesh.index_used;
                let mut verts_ok = true;
                for v in &mesh.verts {
                    if !plan.push_trail_vert(
                        v.xyz,
                        v.color_rgba,
                        v.texcoord_packed,
                        v.normal_packed,
                        v.tangent_packed,
                    ) {
                        verts_ok = false;
                        break;
                    }
                }
                if !verts_ok {
                    plan.shrink_verts_to(vert_used);
                    host.0.gaps.raise(FxGapCause::TrailCodeMeshRefused {
                        step: CodeMeshStep::VertReserve,
                    });
                    plan.end_material_draw(slot);
                    continue;
                }
                let base = vert_used;

                let mut index_ok = true;
                for chunk in mesh.index_pairs.chunks_exact(3) {
                    let pairs = [chunk[0], chunk[1], chunk[2]];
                    let tris = fx_iw4::trail_index_quad_tris(pairs);
                    let mut six = [0u32; 6];
                    let mut i = 0;
                    for tri in tris {
                        six[i] = base.saturating_add(u32::from(tri[0]));
                        six[i + 1] = base.saturating_add(u32::from(tri[1]));
                        six[i + 2] = base.saturating_add(u32::from(tri[2]));
                        i += 3;
                    }
                    if !plan.extend_indices(&six) {
                        index_ok = false;
                        break;
                    }
                }
                if !index_ok {
                    plan.shrink_verts_to(vert_used);
                    plan.shrink_indices_to(index_used);
                    host.0.gaps.raise(FxGapCause::TrailCodeMeshRefused {
                        step: CodeMeshStep::IndexReserve,
                    });
                    plan.end_material_draw(slot);
                    continue;
                }
                if mesh.index_pairs.len() % 3 != 0 {
                    host.0.gaps.raise(FxGapCause::TrailCodeMeshRefused {
                        step: CodeMeshStep::IndexReserve,
                    });
                }
                plan.end_material_draw(slot);
                trails_drawn = trails_drawn.saturating_add(1);
            }
        }
    }

    present_tracer_beams(
        &mut tracers,
        &cam_tf,
        clip_from_world,
        tan_half_fov,
        &color_images,
        &runtime,
        &mut plan,
        &mut combat,
        code_mesh_bind_asset,
        &mut t6_fx,
        |asset_id| t6_fx_material(&runtime, asset_id) && !t6_fx_filtered_out(&runtime, asset_id),
    );
    *tracer_world = tracers;

    let mut spark_drawn = 0usize;
    for spark in &out.spark_clouds {
        let Some(asset_id) = spark_elem_asset_id(&catalog.0, spark.catalog_index, spark.def_index)
        else {
            spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
            continue;
        };
        match code_mesh_bind_asset(asset_id, &color_images, &runtime) {
            FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap) => {
                spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
            }
            FxCodeMeshBind::Skip(cause) => count_fx_present_skip(&mut cursor, cause),
            FxCodeMeshBind::Ready {
                color,
                sort_key,
                ordinal,
            } => {
                spark_plan.begin_material_draw(color, sort_key, Some(ordinal), spark.clouds);
                spark_drawn = spark_drawn.saturating_add(1);
            }
        }
    }
    for cloud in &out.clouds {
        let Some(asset_id) = spark_elem_asset_id(&catalog.0, cloud.catalog_index, cloud.def_index)
        else {
            spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
            continue;
        };
        // bo2zm: Black Ops II particle clouds go to the fallback pass.
        if t6_fx_material(&runtime, asset_id) {
            if !t6_fx_filtered_out(&runtime, asset_id) {
                t6_fx.push_cloud(&cloud.cloud, cloud.particles, asset_id as u32);
            }
            continue;
        }
        match code_mesh_bind_asset(asset_id, &color_images, &runtime) {
            FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap) => {
                spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
            }
            FxCodeMeshBind::Skip(cause) => count_fx_present_skip(&mut cursor, cause),
            FxCodeMeshBind::Ready {
                color,
                sort_key,
                ordinal,
            } => {
                spark_plan.begin_material_draw(
                    color,
                    sort_key,
                    Some(ordinal),
                    [cloud.cloud, cloud.cloud, cloud.cloud],
                );
            }
        }
    }
    for fountain in &out.fountains {
        let Some(asset_id) =
            spark_elem_asset_id(&catalog.0, fountain.catalog_index, fountain.def_index)
        else {
            spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
            continue;
        };
        match code_mesh_bind_asset(asset_id, &color_images, &runtime) {
            FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap) => {
                spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
            }
            FxCodeMeshBind::Skip(cause) => count_fx_present_skip(&mut cursor, cause),
            FxCodeMeshBind::Ready {
                color,
                sort_key,
                ordinal,
            } => {
                if spark_plan
                    .begin_custom_draw(
                        color,
                        sort_key,
                        Some(ordinal),
                        fountain.cloud,
                        &fountain.cells,
                    )
                    .is_none()
                {
                    spark_plan.miss_material = spark_plan.miss_material.saturating_add(1);
                }
            }
        }
    }
    spark_plan.bump();
    spark_plan.publish_share();

    plan.bump();
    plan.publish_share();

    let cam_origin = cam_tf.translation.to_array();
    let first_census = !cursor.draw_logged
        && (sprites_total > 0
            || !out.trail_meshes.is_empty()
            || out.skipped_unsupported_type > 0
            || out.skipped_no_trail_def > 0
            || !out.omni_lights.is_empty()
            || !out.spot_lights.is_empty()
            || drawn > 0
            || trails_drawn > 0
            || combat.beam_queued > 0
            || combat.tracer_live > 0);
    let dump_now = dump_req.pending || first_census;
    if dump_now {
        let (alpha_min, alpha_max, size_min, size_max, dist_min, dist_max) = if drawn == 0 {
            (0, 0, 0.0, 0.0, 0.0, 0.0)
        } else {
            (alpha_min, alpha_max, size_min, size_max, dist_min, dist_max)
        };
        diag::info!(
            World,
            "fx: generate_verts sprites={sprites_total} drawn={drawn} miss_material={} \
             trails={} trails_drawn={trails_drawn} miss_lookup={} unsupported={} \
             cloud={} spark_cloud={} spark_cpu={} spark_hist_empty={} spark_no_size1={} \
             spark_vis_size1={:?} fountain={} skipped_fountain={} model_cpu={} model_draws={} \
             model_skip_lookup={} model_skip_catalog={} model_skip_pose={} model_skip_lod={} model_skip_culled={} model_skip_material={} model_skip_lighting={} model_skip_flags={} \
             omni_cpu={} scene_dlights={} \
             omni_skip={} omni_cap={} spot_cpu={} spot_skip={} postlight={} skipped_postlight={} postlight_miss={} \
             vis_blocker_w={} vis_blocker_r={} other_type={} \
             null_handler={} no_trail_def={} dormant={} \
             trail_code_mesh_gap={} zero_material_half={} codemesh_draws={} \
             spark_drawn={spark_drawn} sparkcloud_draws={} spark_miss_material={} \
             skip_no_ordinal={} skip_not_emissive={} \
             lookup_split=[no_def={} no_elem={} no_material_visual={} no_size0={} \
             size0<=0={} no_size1={} size1<=0={}] \
             exact_sprites alpha={alpha_min}..{alpha_max} size0={size_min:.2}..{size_max:.2} \
             dist={dist_min:.1}..{dist_max:.1} in_front={in_front} behind={behind}",
            cursor.draw_miss_material,
            out.trail_meshes.len(),
            out.skipped_no_lookup,
            out.skipped_unsupported_type,
            out.skipped_cloud,
            out.skipped_spark_cloud,
            out.spark_clouds.len(),
            out.spark_cloud_history_empty,
            out.spark_cloud_no_size1,
            out.spark_clouds.first().map(|s| s.vis_size1),
            out.fountains.len(),
            out.skipped_spark_fountain,
            out.models.len(),
            model_plan.draws().len(),
            out.skipped_model,
            model_plan.skipped_no_catalog,
            model_plan.skipped_no_pose,
            model_plan.skipped_no_lod,
            model_plan.skipped_culled,
            model_plan.skipped_no_material,
            model_plan.skipped_no_lighting,
            model_plan.skipped_render_fx_flags,
            out.omni_lights.len(),
            env.dlights.scene.len(),
            out.skipped_omni_light,
            env.dlights.cap_full,
            out.spot_lights.len(),
            out.skipped_spot_light,
            env.post_lights.drawn,
            env.post_lights.skipped_short,
            env.post_lights.miss_material,
            host.0.vis_blocker_write.count,
            host.0.vis_blocker_read.count,
            out.skipped_other_type,
            out.skipped_null_handler,
            out.skipped_no_trail_def,
            out.skipped_dormant,
            host.0.gaps.hits(fx::FxGap::TrailCodeMesh),
            zero_half,
            plan.draws.len(),
            spark_plan.draws.len(),
            spark_plan.miss_material,
            cursor.skipped_no_ordinal,
            cursor.skipped_not_emissive,
            verts_gaps.no_def,
            verts_gaps.no_elem,
            verts_gaps.no_material_visual,
            verts_gaps.no_size0_sample,
            verts_gaps.size0_not_positive,
            verts_gaps.no_size1_sample,
            verts_gaps.size1_not_positive
        );
        diag::info!(
            World,
            "fx: tracer live={} queued={} drawn={} miss_mat={} miss_color={} miss_unprep={} miss_ord={} miss_emis={} spawned={} skip_interval={} skip_short={} skip_no_def={} name={:?} mat={:?} bind={:?} has_color={:?} speed={:?} beam_len={:?}",
            combat.tracer_live,
            combat.beam_queued,
            combat.beam_drawn,
            combat.beam_miss_material,
            combat.beam_miss_color,
            combat.beam_miss_unprepared,
            combat.beam_miss_ordinal,
            combat.beam_miss_emissive,
            combat.tracer_spawned,
            combat.tracer_skip_interval,
            combat.tracer_skip_short,
            combat.tracer_skip_no_def,
            combat.last_tracer_name,
            combat.last_tracer_material,
            combat.last_tracer_bind,
            combat.last_tracer_has_color,
            combat.last_tracer_speed,
            combat.last_tracer_beam_length
        );
        log_fx_near_camera(
            &host.0,
            &catalog.0,
            &out.sprites,
            &out.spark_clouds,
            &batches,
            cam_origin,
            *cam_fwd,
            dump_req.radius,
            out.skipped_spark_cloud,
            out.skipped_cloud,
            out.spark_cloud_history_empty,
            env.outdoor.as_ref().and_then(|o| o.image).map(|id| id.0),
            spark_plan.tmpl_first_xyz,
            spark_plan.tmpl_first_r2,
            spark_plan.tmpl_holdrand,
            combat.last_impact_def.as_deref(),
        );
        cursor.draw_logged = true;
        dump_req.pending = false;
    }
}

fn fill_fx_model_plan(
    instances: &[fx::FxModelInstance],
    models: Option<&asset_game::FxModelCatalog>,
    atlas: Option<&WorldModelLightingAtlas>,
    scene: Option<&WorldScene>,
    atpoint: &render_scene::DynAtPointLookup,
    camera_origin: Vec3,
    frustum_planes: &[[f32; 4]],
    lod_ramp: crate::assemble::drawsurf::tess::smodel::LodRampArgs,
    cache: Option<&WorldModelLightingCache>,
    lighting_requests: &mut ModelLightingRequests,
    plan: &mut FxModelDrawPlan,
) {
    plan.generated = u32::try_from(instances.len()).unwrap_or(u32::MAX);
    if instances.is_empty() {
        return;
    }
    let Some(models) = models.filter(|catalog| !catalog.is_empty()) else {
        plan.skipped_no_catalog = plan.generated;
        return;
    };
    let scene_atlas = scene.and_then(|world| {
        Some(WorldModelLightingAtlas {
            image: world.model_lighting_image.clone()?,
            dims: world.model_lighting_dims?,
        })
    });
    let Some(_) = atlas.or(scene_atlas.as_ref()) else {
        plan.skipped_no_lighting = plan.generated;
        return;
    };
    let Some(_) = scene else {
        plan.skipped_no_lighting = plan.generated;
        return;
    };
    if cache.is_none() {
        plan.skipped_no_lighting = plan.generated;
        return;
    }

    for instance in instances {
        let Some(entry) = models.get_at(instance.model_index) else {
            plan.skipped_no_catalog = plan.skipped_no_catalog.saturating_add(1);
            continue;
        };
        if instance.flags & 0x800 != 0 {
            plan.skipped_render_fx_flags = plan.skipped_render_fx_flags.saturating_add(1);
            continue;
        }
        let Some(lod) = crate::assemble::drawsurf::tess::smodel::smodel_camera_lod(
            entry.skel.lod,
            instance.origin,
            instance.scale.abs(),
            Some(camera_origin),
            lod_ramp,
        ) else {
            plan.skipped_no_lod = plan.skipped_no_lod.saturating_add(1);
            continue;
        };
        if entry.skel.radius.is_some_and(|radius| {
            dpvs_iw4::scene_ent_sphere_hides(
                instance.origin,
                radius * instance.scale.abs(),
                frustum_planes,
            )
        }) {
            plan.skipped_culled = plan.skipped_culled.saturating_add(1);
            continue;
        }
        let Some(asset) = plan.asset(instance.model_index, lod) else {
            plan.skipped_no_material = plan.skipped_no_material.saturating_add(1);
            continue;
        };
        let asset_surfaces = asset.surfaces.clone();

        let box_half = entry.skel.radius.and_then(|radius| {
            crate::prepare::scene::model_lighting_cache::lighting_box_half(
                &[radius * instance.scale.abs()],
                &[anim_iw4::DOBJ_RADIUS_PARENT_ROOT],
            )
        });
        let lookup_fallback = atpoint.fallback(instance.origin, box_half);
        let lighting = lighting_requests.request(ModelLightingRequest {
            owner: ModelLightingOwner::FxModel(instance.elem_handle),
            origin: instance.origin,
            lookup_fallback,
        });
        let quat = Quat::from_array(fx_iw4::axis_to_quat(instance.axis));
        let world_from_local = Mat4::from_scale_rotation_translation(
            Vec3::splat(instance.scale),
            quat,
            Vec3::from_array(instance.origin),
        );
        let caster_bound = entry
            .skel
            .radius
            .map(|radius| render_scene::XModelCasterBound {
                origin: instance.origin,
                radius: (radius * instance.scale.abs()).max(1.0),
            });
        for (surface, material) in asset_surfaces {
            plan.push_draw(crate::assemble::drawsurf::tess::xmodel::XModelSurfaceDraw {
                surface,
                material,
                world_from_local,
                lighting_handle: 0,
                pending_lighting: Some(lighting),
                colour_refusal: None,
                object_id: instance.elem_handle,
                scene_light_index: 0,
                reflection_probe_index: 0,
                packed_lighting: None,
                is_scope: false,
                scene_entnum: None,
                caster_bound,
            });
        }
    }
}

fn log_fx_near_camera(
    host: &FxSystemHost,
    catalog: &FxDefinitions,
    sprites: &[FxSpriteInstance],
    spark_clouds: &[FxSparkCloudInstance],
    batches: &[(usize, Vec<&FxSpriteInstance>)],
    cam: [f32; 3],
    cam_fwd: Vec3,
    radius: f32,
    spark_cloud: u32,
    cloud: u32,
    spark_hist_empty: u32,
    outdoor_image: Option<u32>,
    spark_tmpl: [f32; 3],
    spark_tmpl_r2: f32,
    spark_tmpl_holdrand: u32,
    last_impact_def: Option<&str>,
) {
    let radius = radius.max(1.0);
    let mut batch_rows: Vec<(f32, String)> = batches
        .iter()
        .map(|(mat, list)| {
            let nearest = list
                .iter()
                .map(|s| sprite_dist(s.origin, cam))
                .fold(f32::MAX, f32::min);
            let amax = list.iter().map(|s| s.color_rgba[3]).max().unwrap_or(0);
            (
                nearest,
                format!(
                    "#{mat} `{}` n={} near={nearest:.0} amax={amax}",
                    list.first()
                        .map(|s| s.material_name.as_ref())
                        .unwrap_or("?"),
                    list.len()
                ),
            )
        })
        .collect();
    batch_rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let batch_summary = batch_rows
        .iter()
        .take(12)
        .map(|(_, row)| row.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    diag::info!(
        World,
        "fx_dump cam={:.0},{:.0},{:.0} fwd={:.2},{:.2},{:.2} radius={radius:.0} spark_cloud={spark_cloud} \
         spark_cpu={} spark_hist_empty={spark_hist_empty} cloud={cloud} outdoor_image={} \
         spark_tmpl={:.5},{:.5},{:.5} r2={:.5} holdrand=0x{:08x} batches={}",
        cam[0],
        cam[1],
        cam[2],
        cam_fwd.x,
        cam_fwd.y,
        cam_fwd.z,
        spark_clouds.len(),
        outdoor_image
            .map(|id| id.to_string())
            .unwrap_or_else(|| "NONE".into()),
        spark_tmpl[0],
        spark_tmpl[1],
        spark_tmpl[2],
        spark_tmpl_r2,
        spark_tmpl_holdrand,
        batch_summary
    );

    let mut near: Vec<&FxSpriteInstance> = sprites.iter().collect();
    near.sort_by(|a, b| {
        sprite_dist(a.origin, cam)
            .partial_cmp(&sprite_dist(b.origin, cam))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let inside = near
        .iter()
        .filter(|s| sprite_dist(s.origin, cam) <= radius)
        .count();
    diag::info!(
        World,
        "fx_dump near_total={} inside_radius={inside}",
        near.len()
    );
    log_fx_run_mode_census(host, sprites, catalog, cam, radius);
    log_fx_impact_census(host, sprites, cam, cam_fwd, last_impact_def);
    for sprite in near.iter().take(12) {
        log_fx_near_sprite("fx_near", sprite, cam, cam_fwd);
    }

    for sprite in near.iter().filter(|s| s.atlas.entry_count > 1).take(8) {
        log_fx_near_sprite("fx_near_cell", sprite, cam, cam_fwd);
    }

    for sprite in near
        .iter()
        .filter(|s| s.atlas.entry_count > 1)
        .filter(|s| {
            let delta = Vec3::from_array(s.origin) - Vec3::from_array(cam);
            delta.dot(cam_fwd) > 0.0
        })
        .take(8)
    {
        log_fx_near_sprite("fx_near_front_cell", sprite, cam, cam_fwd);
    }
    if let Some(sprite) = sprites
        .iter()
        .filter(|s| s.atlas.entry_count > 1)
        .max_by(|a, b| {
            a.size0
                .partial_cmp(&b.size0)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        log_fx_near_sprite("fx_near_cell_max", sprite, cam, cam_fwd);
    }
    if let Some(sprite) = sprites.iter().max_by(|a, b| {
        a.size0
            .partial_cmp(&b.size0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        log_fx_near_sprite("fx_near_size_max", sprite, cam, cam_fwd);
    }

    let small: Vec<&FxSpriteInstance> = sprites
        .iter()
        .filter(|s| s.def_name.contains("firelp_small"))
        .collect();
    let small_dist = small
        .iter()
        .map(|s| sprite_dist(s.origin, cam))
        .fold(f32::MAX, f32::min);
    let (sy_min, sy_max) = small.iter().fold((f32::MAX, f32::MIN), |(mn, mx), s| {
        (mn.min(s.origin[1]), mx.max(s.origin[1]))
    });
    diag::info!(
        World,
        "fx_firelp_small n={} dist_min={} y={:.0}..{:.0}",
        small.len(),
        if small.is_empty() {
            "NULL".into()
        } else {
            format!("{small_dist:.0}")
        },
        if small.is_empty() { 0.0 } else { sy_min },
        if small.is_empty() { 0.0 } else { sy_max }
    );
    for effect in host
        .live_effects()
        .filter(|e| e.def_name.contains("firelp_small"))
    {
        let pending = (effect.status & fx_iw4::FX_STATUS_HAS_PENDING_LOOP_ELEMS) != 0;
        let dist = sprite_dist(effect.origin, cam);
        diag::info!(
            World,
            "fx_held_small origin={:.0},{:.0},{:.0} dist={dist:.0} msec_begin={} pending={pending} def={}",
            effect.origin[0],
            effect.origin[1],
            effect.origin[2],
            effect.msec_begin,
            effect.def_name
        );
    }
    for sprite in small.iter().take(8) {
        log_fx_near_sprite("fx_near_small", sprite, cam, cam_fwd);
    }
    for spark in spark_clouds.iter().take(8) {
        let dist = sprite_dist(spark.origin, cam);
        let mat = spark_elem_material(catalog, spark.catalog_index, spark.def_index)
            .unwrap_or_else(|| "-".into());
        let c0 = &spark.clouds[0];
        let c1 = &spark.clouds[1];
        let c2 = &spark.clouds[2];
        diag::info!(
            World,
            "fx_spark cpu writeIdx={} size0={:.1} size1={:.1} scale={:.1}/{:.1}/{:.1} flags=0x{:x} \
             def={} mat={} origin={:.0},{:.0},{:.0} pos1={:.0},{:.0},{:.0} pos2={:.0},{:.0},{:.0} \
             dist={dist:.0}",
            spark.write_idx,
            spark.size0,
            c0.size1,
            c0.placement_scale,
            c1.placement_scale,
            c2.placement_scale,
            c0.flags,
            spark.def_name,
            mat,
            spark.origin[0],
            spark.origin[1],
            spark.origin[2],
            c1.pos[0],
            c1.pos[1],
            c1.pos[2],
            c2.pos[0],
            c2.pos[1],
            c2.pos[2]
        );
    }
}

fn is_impact_def(name: &str) -> bool {
    name.contains("impacts/")
}

struct ImpactPresentCensus {
    sprite_origin: Option<[f32; 3]>,
    sprite_flags: Option<i32>,
    decal_dist: Option<f32>,
    far_origin: Option<[f32; 3]>,
    far_flags: Option<i32>,
    far_def: Option<String>,
    vel_local_n: u32,
    vel_world_n: u32,
}

fn impact_present_census(host: &FxSystemHost, sprites: &[FxSpriteInstance]) -> ImpactPresentCensus {
    let impacts: Vec<&FxSpriteInstance> = sprites
        .iter()
        .filter(|s| is_impact_def(&s.def_name))
        .collect();
    let anchor = host.last_decal_origin;
    let nearest = impacts.iter().min_by(|a, b| {
        let da = anchor.map_or(0.0, |p| sprite_dist(a.origin, p));
        let db = anchor.map_or(0.0, |p| sprite_dist(b.origin, p));
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    let farthest = impacts.iter().max_by(|a, b| {
        let da = anchor.map_or(0.0, |p| sprite_dist(a.origin, p));
        let db = anchor.map_or(0.0, |p| sprite_dist(b.origin, p));
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    let sprite_origin = nearest.map(|s| s.origin);
    let decal_dist = match (anchor, sprite_origin) {
        (Some(d), Some(o)) => Some(sprite_dist(o, d)),
        _ => None,
    };
    ImpactPresentCensus {
        sprite_origin,
        sprite_flags: nearest.map(|s| s.flags),
        decal_dist,
        far_origin: farthest.map(|s| s.origin),
        far_flags: farthest.map(|s| s.flags),
        far_def: farthest.map(|s| s.def_name.to_string()),
        vel_local_n: impacts
            .iter()
            .filter(|s| (s.flags & FX_ELEM_VEL_LOCAL) != 0)
            .count() as u32,
        vel_world_n: impacts
            .iter()
            .filter(|s| (s.flags & FX_ELEM_VEL_WORLD) != 0)
            .count() as u32,
    }
}

fn log_fx_impact_census(
    host: &FxSystemHost,
    sprites: &[FxSpriteInstance],
    cam: [f32; 3],
    cam_fwd: Vec3,
    last_impact_def: Option<&str>,
) {
    let mut impact_sprites: Vec<&FxSpriteInstance> = sprites
        .iter()
        .filter(|s| is_impact_def(&s.def_name))
        .collect();
    impact_sprites.sort_by(|a, b| {
        sprite_dist(a.origin, cam)
            .partial_cmp(&sprite_dist(b.origin, cam))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let n = impact_sprites.len();
    let world0 = impact_sprites
        .iter()
        .filter(|s| s.origin[0].abs() < 1.0 && s.origin[1].abs() < 1.0 && s.origin[2].abs() < 1.0)
        .count();
    let (spread, nearest) = if n == 0 {
        ("none".into(), "none".into())
    } else {
        let (mut mn, mut mx) = (impact_sprites[0].origin, impact_sprites[0].origin);
        for s in &impact_sprites {
            for i in 0..3 {
                mn[i] = mn[i].min(s.origin[i]);
                mx[i] = mx[i].max(s.origin[i]);
            }
        }
        (
            format!(
                "{:.1}..{:.1},{:.1}..{:.1},{:.1}..{:.1}",
                mn[0], mx[0], mn[1], mx[1], mn[2], mx[2]
            ),
            format!(
                "{:.1},{:.1},{:.1}",
                impact_sprites[0].origin[0],
                impact_sprites[0].origin[1],
                impact_sprites[0].origin[2]
            ),
        )
    };
    let decal = host.last_decal_origin.map_or_else(
        || "NULL".into(),
        |o| format!("{:.1},{:.1},{:.1}", o[0], o[1], o[2]),
    );
    let census = impact_present_census(host, sprites);
    let near_decal = census.sprite_origin.map_or_else(
        || "none".into(),
        |o| format!("{:.1},{:.1},{:.1}", o[0], o[1], o[2]),
    );
    let up = host.last_decal_axis.map_or_else(
        || "NULL".into(),
        |a| format!("{:.3},{:.3},{:.3}", a[2][0], a[2][1], a[2][2]),
    );
    let far = census.far_origin.map_or_else(
        || "none".into(),
        |o| format!("{:.1},{:.1},{:.1}", o[0], o[1], o[2]),
    );
    diag::info!(
        World,
        "fx_dump impact n={n} world0={world0} spread={spread} nearest={nearest} \
         near_decal={near_decal} near_flags=0x{:x} far={far} far_flags=0x{:x} far_def={} \
         vel_local_n={} vel_world_n={} \
         decal_dist={} last_decal={decal} last_decal_up={up} \
         last_impact_def={} last_decal_parent={}",
        census.sprite_flags.unwrap_or(0),
        census.far_flags.unwrap_or(0),
        census.far_def.as_deref().unwrap_or("NULL"),
        census.vel_local_n,
        census.vel_world_n,
        census
            .decal_dist
            .map(|d| format!("{d:.1}"))
            .unwrap_or_else(|| "NULL".into()),
        last_impact_def.unwrap_or("NULL"),
        host.last_decal_parent.as_deref().unwrap_or("NULL")
    );
    for sprite in impact_sprites.iter().take(16) {
        log_fx_near_sprite("fx_near_impact", sprite, cam, cam_fwd);
    }

    let mut held = 0u32;
    for effect in host.live_effects().filter(|e| is_impact_def(&e.def_name)) {
        held = held.saturating_add(1);
        let dist = sprite_dist(effect.origin, cam);
        diag::info!(
            World,
            "fx_held_impact origin={:.1},{:.1},{:.1} dist={dist:.0} msec_begin={} \
             status=0x{:x} def={}",
            effect.origin[0],
            effect.origin[1],
            effect.origin[2],
            effect.msec_begin,
            effect.status,
            effect.def_name
        );
    }
    if held == 0 {
        diag::info!(World, "fx_held_impact n=0");
    }

    let mut stored = 0u32;
    let mut stored0 = 0u32;
    for elem in host.live_elems() {
        let Some(effect) = host.effect_at(elem.owner_effect_slot as usize) else {
            continue;
        };
        if !is_impact_def(&effect.def_name) {
            continue;
        }
        stored = stored.saturating_add(1);
        let zero =
            elem.origin[0].abs() < 1.0 && elem.origin[1].abs() < 1.0 && elem.origin[2].abs() < 1.0;
        if zero {
            stored0 = stored0.saturating_add(1);
        }
        if stored <= 16 {
            diag::info!(
                World,
                "fx_elem_impact stored={:.1},{:.1},{:.1} run=0x{:x} flags=0x{:x} type={} def={} zero={}",
                elem.origin[0],
                elem.origin[1],
                elem.origin[2],
                elem_run_mode(elem.flags),
                elem.flags,
                elem.elem_type,
                effect.def_name,
                u8::from(zero)
            );
        }
    }
    diag::info!(
        World,
        "fx_dump impact_elems stored={stored} stored0={stored0}"
    );
}

fn log_fx_run_mode_census(
    host: &FxSystemHost,
    sprites: &[FxSpriteInstance],
    catalog: &FxDefinitions,
    cam: [f32; 3],
    radius: f32,
) {
    let mut run0 = 0u32;
    let mut run40 = 0u32;
    let mut run80 = 0u32;
    let mut run_c0 = 0u32;
    let mut stored0 = 0u32;
    for elem in host.live_elems() {
        match elem_run_mode(elem.flags) {
            0 => run0 += 1,
            0x40 => run40 += 1,
            0x80 => run80 += 1,
            0xc0 => run_c0 += 1,
            _ => {}
        }
        if elem.origin[0].abs() < 1.0 && elem.origin[1].abs() < 1.0 && elem.origin[2].abs() < 1.0 {
            stored0 += 1;
        }
    }
    let world0 = sprites
        .iter()
        .filter(|s| s.origin[0].abs() < 1.0 && s.origin[1].abs() < 1.0 && s.origin[2].abs() < 1.0)
        .count();
    diag::info!(
        World,
        "fx_dump run=0:{run0} 0x40:{run40} 0x80:{run80} 0xc0:{run_c0} stored0={stored0} world0={world0}"
    );

    let dust: Vec<&FxSpriteInstance> = sprites
        .iter()
        .filter(|s| s.def_name.contains("dust"))
        .collect();
    if dust.is_empty() {
        return;
    }
    let (mut mn, mut mx) = (dust[0].origin, dust[0].origin);
    for s in &dust {
        for i in 0..3 {
            mn[i] = mn[i].min(s.origin[i]);
            mx[i] = mx[i].max(s.origin[i]);
        }
    }
    let inside = dust
        .iter()
        .filter(|s| sprite_dist(s.origin, cam) <= radius)
        .count();
    let nearest = dust.iter().min_by(|a, b| {
        sprite_dist(a.origin, cam)
            .partial_cmp(&sprite_dist(b.origin, cam))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let (offset_r, offset_h, flags) = nearest
        .and_then(|s| {
            let effect = render_fx::present::catalog_lookup(catalog, s.catalog_index)?;
            let elem = effect.elems.get(s.def_index as usize)?;
            Some((
                elem.view.spawn_offset_radius_base,
                elem.view.spawn_offset_height_base,
                elem.view.flags,
            ))
        })
        .unwrap_or((0.0, 0.0, 0));
    let mode = elem_spawn_offset_mode(flags);
    diag::info!(
        World,
        "fx_dump dust n={} inside={inside} spread={:.0}..{:.0},{:.0}..{:.0},{:.0}..{:.0} \
         nearest_run=0x{:x} offset_r={offset_r:.0} offset_h={offset_h:.0} spawn_off={mode:?}",
        dust.len(),
        mn[0],
        mx[0],
        mn[1],
        mx[1],
        mn[2],
        mx[2],
        nearest.map(|s| elem_run_mode(s.flags)).unwrap_or(0),
    );
}

fn log_fx_near_sprite(tag: &str, sprite: &FxSpriteInstance, cam: [f32; 3], cam_fwd: Vec3) {
    let dist = sprite_dist(sprite.origin, cam);
    let delta = Vec3::from_array(sprite.origin) - Vec3::from_array(cam);
    let front = delta.dot(cam_fwd) > 0.0;
    diag::info!(
        World,
        "{tag} dist={dist:.0} front={front} rgb={},{},{} alpha={} size0={:.1} type={} def={} mat={} \
         origin={:.0},{:.0},{:.0} run=0x{:x} flags=0x{:x} atlas={:.3},{:.3},{:.3},{:.3} entry={} idx={}",
        sprite.color_rgba[0],
        sprite.color_rgba[1],
        sprite.color_rgba[2],
        sprite.color_rgba[3],
        sprite.size0,
        sprite.elem_type,
        sprite.def_name,
        sprite.material_name,
        sprite.origin[0],
        sprite.origin[1],
        sprite.origin[2],
        elem_run_mode(sprite.flags),
        sprite.flags,
        sprite.atlas.s0,
        sprite.atlas.ds,
        sprite.atlas.t0,
        sprite.atlas.dt,
        sprite.atlas.entry_count,
        sprite.atlas.atlas_index
    );
}

fn sprite_dist(origin: [f32; 3], cam: [f32; 3]) -> f32 {
    let dx = origin[0] - cam[0];
    let dy = origin[1] - cam[1];
    let dz = origin[2] - cam[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FxPresentSkip {
    NoColorMap,
    NoOrdinal,
    NotEmissive,
}

pub(crate) enum FxCodeMeshBind {
    Ready {
        color: Option<Handle<Image>>,
        sort_key: u8,
        ordinal: u32,
    },
    Skip(FxPresentSkip),
}

fn count_fx_present_skip(cursor: &mut FxJournalCursor, cause: FxPresentSkip) {
    match cause {
        FxPresentSkip::NoOrdinal => {
            cursor.skipped_no_ordinal = cursor.skipped_no_ordinal.saturating_add(1);
        }
        FxPresentSkip::NotEmissive => {
            cursor.skipped_not_emissive = cursor.skipped_not_emissive.saturating_add(1);
        }
        FxPresentSkip::NoColorMap => {}
    }
}

fn spark_elem_material(
    catalog: &FxDefinitions,
    catalog_index: u16,
    def_index: u8,
) -> Option<String> {
    let effect = render_fx::present::catalog_lookup(catalog, catalog_index)?;
    let elem = effect.elems.get(def_index as usize)?;
    elem.visuals
        .iter()
        .find_map(OwnedFxVisual::present_name)
        .map(str::to_owned)
}

fn spark_elem_asset_id(
    catalog: &FxDefinitions,
    catalog_index: u16,
    def_index: u8,
) -> Option<usize> {
    let effect = render_fx::present::catalog_lookup(catalog, catalog_index)?;
    let elem = effect.elems.get(def_index as usize)?;
    elem.visuals.iter().find_map(OwnedFxVisual::bound_index)
}

/// bo2zm: the Black Ops II marks of this frame's mark mesh, taken out
/// before the engine's own mark plan sees it (their materials have no
/// technique, so it would refuse them).
fn take_t6_marks(
    host: &FxSystemHost,
    mesh: Option<&fx::GfxMarkMeshCensus>,
    runtime: &MaterialGeneration,
    out: &mut T6MarkMesh,
) {
    out.vertices.clear();
    out.indices.clear();
    out.draws.clear();
    {
        use std::sync::Mutex;
        static LAST: Mutex<Option<String>> = Mutex::new(None);
        let line = match mesh {
            None => "no mark mesh".to_owned(),
            Some(m) => format!(
                "mark mesh surfs={} verts={} materials={:?}",
                m.surfs.len(),
                m.packed.len(),
                m.surfs
                    .iter()
                    .take(4)
                    .map(|s| host
                        .marks
                        .material_name(s.mark_slot)
                        .unwrap_or("?")
                        .to_owned())
                    .collect::<Vec<_>>()
            ),
        };
        if let Ok(mut last) = LAST.lock()
            && last.as_deref() != Some(line.as_str())
        {
            diag::info!(World, "bo2zm marks: {line}");
            *last = Some(line);
        }
    }
    let Some(mesh) = mesh else { return };
    let rd = |row: &[u8], at: usize| {
        f32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]])
    };
    let mut converted = false;
    let mut lit_ms = 0.0f64;
    for surf in &mesh.surfs {
        if surf.index_count == 0 {
            continue;
        }
        let Some(name) = host.marks.material_name(surf.mark_slot) else {
            continue;
        };
        let bind = asset_game::material_bind_name(name);
        let Some(material) = runtime.catalog.material_for_name(bind) else {
            continue;
        };
        if material.t6_draw.is_none() {
            continue;
        }
        if !converted {
            converted = true;
            out.vertices = mesh
                .packed
                .iter()
                .zip(mesh.verts.iter())
                .map(|(row, pt)| {
                    let color =
                        u32::from_le_bytes([row[16], row[17], row[18], row[19]]).to_le_bytes();
                    render_frame::SmodelVertex {
                        position: pt.xyz,
                        normal: pt.normal,
                        // The mark's colour is stored B, G, R, A.
                        color: [color[2], color[1], color[0], color[3]]
                            .map(|c| f32::from(c) / 255.0),
                        uv0: [rd(row, 0x14), rd(row, 0x18)],
                    }
                })
                .collect();
        }
        let first = surf.index_start as usize;
        let Some(indices) = mesh
            .indices
            .get(first..first.saturating_add(surf.index_count as usize))
        else {
            continue;
        };
        let start = out.indices.len() as u32;
        out.indices.extend(indices.iter().map(|&i| u32::from(i)));
        let at = indices
            .first()
            .and_then(|&i| mesh.verts.get(usize::from(i)))
            .map_or([0.0; 3], |pt| pt.xyz);
        let began = std::time::Instant::now();
        let lit = light_t6_mark_from_lightmap(surf, indices, &mesh.verts, &mut out.vertices);
        lit_ms += began.elapsed().as_secs_f64() * 1000.0;
        out.draws.push((
            start,
            surf.index_count,
            u32::from(material.asset_id.0),
            at,
            lit,
        ));
    }
    if lit_ms > 2.0 {
        diag::warn!(
            World,
            "bo2zm marks: lighting {} marks from the lightmap took {lit_ms:.1} ms",
            out.draws.len()
        );
    }
}

/// bo2zm fix list 2: a mark on a lightmapped world surface takes that
/// surface's lightmap light at each of its corners, as BO2's world mark
/// shaders do (the light grid in front of a wall can be far darker than the
/// wall, which turned holes into black blotches). The corner colours become
/// the tint times sqrt(light / 32) (the prop shader's baked-light form); the
/// result is the surface's primary light and its mean baked visibility
/// (times the tint), or `None` to keep the light grid.
fn light_t6_mark_from_lightmap(
    surf: &fx::GfxMarkMeshSurf,
    indices: &[u16],
    verts: &[marks_iw4::FxMarkStagingPoint],
    out: &mut [render_frame::SmodelVertex],
) -> Option<(u8, f32)> {
    let lmap = marks_iw4::mark_context_lmap(&surf.context);
    if lmap == marks_iw4::GFX_SURFACE_LIGHTMAP_NONE {
        return None;
    }
    let mut seen: Vec<u16> = indices.to_vec();
    seen.sort_unstable();
    seen.dedup();
    let mut vis_sum = 0.0;
    let mut lit_n = 0usize;
    let mut tint = None;
    for &i in &seen {
        let (Some(pt), Some(v)) = (verts.get(usize::from(i)), out.get_mut(usize::from(i))) else {
            continue;
        };
        let Some((light, vis)) = asset_world::t6_lightmap_light(lmap, pt.lmap_coord, pt.normal)
        else {
            return None;
        };
        // bo2zm test aid: IW4L_MARK_LIGHT_LOG=1 logs a mark's first corner
        // light in parts (base, directional, its N.dir, primary visibility).
        if lit_n == 0
            && std::env::var_os("IW4L_MARK_LIGHT_LOG").is_some()
            && let Some((base, dir, ndl, pvis)) =
                asset_world::t6_lightmap_parts(lmap, pt.lmap_coord, pt.normal)
        {
            diag::info!(
                World,
                "bo2zm mark light at {:?}: base {:?} directional {:?} N.dir {:.2} primary {} vis {:.2}",
                pt.xyz.map(f32::round),
                base.map(|c| (c * 100.0).round() / 100.0),
                dir.map(|c| (c * 100.0).round() / 100.0),
                ndl,
                surf.context[5],
                pvis
            );
        }
        let t = *tint.get_or_insert([v.color[0], v.color[1], v.color[2]]);
        for c in 0..3 {
            v.color[c] = t[c] * (light[c] / 32.0).clamp(0.0, 1.0).sqrt();
        }
        vis_sum += vis;
        lit_n += 1;
    }
    let t = tint?;
    let tint_sq = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]) / 3.0;
    Some((surf.context[5], vis_sum / lit_n.max(1) as f32 * tint_sq))
}

fn fill_mark_mesh_plan(
    host: &mut FxSystemHost,
    mesh: Option<fx::GfxMarkMeshCensus>,
    plan: &mut GfxMarkMeshPlan,
    runtime: &MaterialGeneration,
) {
    let Some(mesh) = mesh else {
        host.gfx_mark_draw_n = None;
        host.gfx_mark_draw_tri = None;
        host.gfx_mark_draw_skip_why = None;
        host.last_mark_lmap = None;
        host.last_mark_primary_light = None;
        host.last_mark_probe = None;
        host.last_mark_lmap_none_n = None;
        host.last_mark_lmap_page_n = None;
        plan.bump();
        plan.publish_share();
        return;
    };
    plan.vertices = Arc::new(mesh.packed);
    Arc::make_mut(&mut plan.indices).reserve(mesh.indices.len());
    let mut skip_why = None;

    let mut admitted: Vec<(u64, u8, u32, GfxMarkSubKey, u32, u32)> =
        Vec::with_capacity(mesh.surfs.len());
    for surf in &mesh.surfs {
        if surf.index_count == 0 {
            continue;
        }
        let Some(name) = host.marks.material_name(surf.mark_slot) else {
            skip_why = Some("no_material");
            continue;
        };
        let Some((sort_key, ordinal)) = mark_mesh_bind(name, runtime) else {
            skip_why = Some("no_ordinal");
            continue;
        };
        let sub_key = GfxMarkSubKey::from_context(&surf.context);
        let packed = pack_mark_mesh_draw_surf(
            sort_key,
            render_material::sort_band(ordinal),
            0,
            sub_key.lmap,
            sub_key.primary_light,
            sub_key.probe,
        )
        .packed;
        admitted.push((
            packed,
            sort_key,
            ordinal,
            sub_key,
            surf.index_start,
            surf.index_count,
        ));
    }
    admitted.sort_by_key(|row| row.0);
    for (_, sort_key, ordinal, sub_key, index_start, index_count) in admitted {
        let first = index_start as usize;
        let Some(indices) = mesh
            .indices
            .get(first..first.saturating_add(index_count as usize))
        else {
            skip_why = Some("range_out_of_mesh");
            continue;
        };
        plan.append_run_indices(sort_key, Some(ordinal), sub_key, indices);
    }
    if mesh.budget.surf_n > 0 && plan.draws.is_empty() && skip_why.is_none() {
        skip_why = Some("empty_range");
    }
    plan.skip_why = skip_why;
    plan.bump();
    host.gfx_mark_draw_n = Some(plan.draws.len() as u32);
    host.gfx_mark_draw_tri = Some(
        plan.draws
            .iter()
            .map(|d| d.index_count / 3)
            .fold(0u32, u32::saturating_add),
    );
    host.gfx_mark_draw_skip_why = skip_why.map(str::to_owned);
    if let Some(first) = plan.draws.first() {
        host.last_mark_lmap = Some(first.sub_key.lmap);
        host.last_mark_primary_light = Some(first.sub_key.primary_light);
        host.last_mark_probe = Some(first.sub_key.probe);
    } else {
        host.last_mark_lmap = None;
        host.last_mark_primary_light = None;
        host.last_mark_probe = None;
    }
    let mut none_n = 0u32;
    let mut page_n = 0u32;
    for draw in &plan.draws {
        if draw.sub_key.lmap == marks_iw4::GFX_SURFACE_LIGHTMAP_NONE {
            none_n = none_n.saturating_add(1);
        } else {
            page_n = page_n.saturating_add(1);
        }
    }
    host.last_mark_lmap_none_n = Some(none_n);
    host.last_mark_lmap_page_n = Some(page_n);
    plan.publish_share();
}

fn mark_mesh_bind(name: &str, runtime: &MaterialGeneration) -> Option<(u8, u32)> {
    let bind = asset_game::material_bind_name(name);
    let ordinal = runtime.catalog.ordinal_for_material_name(bind)?;
    let material = runtime_material_for_bind(&runtime.catalog, bind)?;
    let sort_key = material
        .baked_draw_surf
        .map(|packed| dpvs_iw4::GfxDrawSurf { packed }.primary_sort_key())
        .unwrap_or(0);
    Some((sort_key, ordinal.get()))
}

pub(crate) fn code_mesh_bind(
    material_name: &str,
    color_images: &FxWorldColorImages,
    runtime: &MaterialGeneration,
) -> FxCodeMeshBind {
    let bind = asset_game::material_bind_name(material_name);
    let color = lookup_fx_color_image(&color_images.colors, material_name).cloned();
    let Some(ordinal) = runtime.catalog.ordinal_for_material_name(bind) else {
        return FxCodeMeshBind::Skip(FxPresentSkip::NoOrdinal);
    };
    let Some(material) = runtime_material_for_bind(&runtime.catalog, bind) else {
        return FxCodeMeshBind::Skip(FxPresentSkip::NoOrdinal);
    };

    if color.is_none()
        && !material.baked_draw_surf.is_some_and(|key| {
            crate::assemble::drawsurf::material_runtime::resolve_material_technique(
                &runtime.catalog,
                render_material::MaterialDrawKey::new(key, ordinal.get()),
                crate::assemble::drawsurf::TechType(5),
            )
            .is_ok_and(|(_, technique)| technique.flags & 1 != 0)
        })
    {
        return FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap);
    }
    if material.camera_region != asset_iw4::CAMERA_REGION_EMISSIVE {
        return FxCodeMeshBind::Skip(FxPresentSkip::NotEmissive);
    }
    let sort_key = color_images
        .keys
        .get(material_name)
        .or_else(|| color_images.keys.get(bind))
        .map(|(sort_key, _)| *sort_key)
        .or_else(|| {
            material
                .baked_draw_surf
                .map(|packed| dpvs_iw4::GfxDrawSurf { packed }.primary_sort_key())
        })
        .unwrap_or(0);
    FxCodeMeshBind::Ready {
        color,
        sort_key,
        ordinal: ordinal.get(),
    }
}

pub(crate) fn code_mesh_bind_asset(
    asset_id: usize,
    color_images: &FxWorldColorImages,
    runtime: &MaterialGeneration,
) -> FxCodeMeshBind {
    let Some(material) = runtime
        .catalog
        .derived(assets::MaterialIndex::from_order(asset_id))
    else {
        return FxCodeMeshBind::Skip(FxPresentSkip::NoOrdinal);
    };
    let color = color_images.colors_by_asset.get(&asset_id).cloned();
    let Some(ordinal) = runtime
        .catalog
        .sorted_materials
        .ordinal_for_asset_id(asset_id)
    else {
        return FxCodeMeshBind::Skip(FxPresentSkip::NoOrdinal);
    };

    if color.is_none()
        && !material.baked_draw_surf.is_some_and(|key| {
            crate::assemble::drawsurf::material_runtime::resolve_material_technique(
                &runtime.catalog,
                render_material::MaterialDrawKey::new(key, ordinal.get()),
                crate::assemble::drawsurf::TechType(5),
            )
            .is_ok_and(|(_, technique)| technique.flags & 1 != 0)
        })
    {
        return FxCodeMeshBind::Skip(FxPresentSkip::NoColorMap);
    }
    if material.camera_region != asset_iw4::CAMERA_REGION_EMISSIVE {
        return FxCodeMeshBind::Skip(FxPresentSkip::NotEmissive);
    }
    let sort_key = color_images
        .keys_by_asset
        .get(&asset_id)
        .map(|(sort_key, _)| *sort_key)
        .or_else(|| {
            material
                .baked_draw_surf
                .map(|packed| dpvs_iw4::GfxDrawSurf { packed }.primary_sort_key())
        })
        .unwrap_or(0);
    FxCodeMeshBind::Ready {
        color,
        sort_key,
        ordinal: ordinal.get(),
    }
}

fn runtime_material_for_bind<'a>(
    catalog: &'a crate::assemble::drawsurf::RuntimeMaterialCatalog,
    bind: &str,
) -> Option<&'a crate::assemble::drawsurf::RuntimeMaterial> {
    catalog.material_for_name(bind)
}

fn sprite_transform(sprite: &FxSpriteInstance, cam: &Transform) -> Transform {
    let spin = Quat::from_rotation_z(sprite.rotation_rad);
    // bo2zm: a non-uniform sprite (size1 set) is size1 tall.
    let height = if sprite.size1 > 0.0 {
        sprite.size1
    } else {
        sprite.size0
    };
    if sprite.elem_type == 0 {
        Transform {
            translation: Vec3::from_array(sprite.origin),
            rotation: cam.rotation * spin,

            scale: Vec3::new(sprite.size0 * 2.0, height * 2.0, 1.0),
        }
    } else if sprite.elem_type == 2 {
        // A tail trails behind its part; a T6 line runs ahead of it.
        let along = if sprite.flags & T6_FX_ELEM_LINE != 0 {
            sprite.vel_dir.map(|v| -v)
        } else {
            sprite.vel_dir
        };
        let origin = tail_anchor_origin(sprite.origin, along, sprite.size1);
        let cam_o = cam.translation.to_array();
        let Some(axes) = tail_sprite_axes(sprite.vel_dir, cam_o, origin) else {
            return Transform {
                translation: Vec3::from_array(origin),
                rotation: cam.rotation,
                scale: Vec3::ZERO,
            };
        };

        let right = Vec3::from_array(axes[0]).normalize_or_zero();
        let up = Vec3::from_array(axes[1]).normalize_or_zero();
        let forward = Vec3::from_array(axes[2]).normalize_or_zero();
        let mat = Mat3::from_cols(right, up, forward);
        Transform {
            translation: Vec3::from_array(origin),
            rotation: Quat::from_mat3(&mat) * spin,
            scale: Vec3::new(
                tail_sprite_full_extent(sprite.size0),
                tail_sprite_full_extent(sprite.size1),
                1.0,
            ),
        }
    } else {
        let right = Vec3::from_array(sprite.axis[1]).normalize_or_zero();
        let up = Vec3::from_array(sprite.axis[2]).normalize_or_zero();
        let forward = right.cross(up).normalize_or_zero();
        let mat = Mat3::from_cols(right, up, forward);
        Transform {
            translation: Vec3::from_array(sprite.origin),
            rotation: Quat::from_mat3(&mat) * spin,

            scale: Vec3::new(sprite.size0 * 2.0, height * 2.0, 1.0),
        }
    }
}

fn fire_weapon(
    fire: On<net::EntityWeaponFire>,
    identities: Query<&CEntity>,
    world_bolts: Query<(
        &net::CEntityRuntime,
        &crate::adapters::anim::remote_body::RemoteFxBolts,
    )>,
    fpv_bolts: Res<crate::adapters::anim::fpv_present::FpvBoltTargets>,
    local: Res<LocalPresentClient>,
    presented: Res<PresentedSnapshot>,
    view: Res<ViewSubject>,
    weapons: Option<Res<PreparedWeapons>>,
    sound_bank: Option<Res<audio::SoundBank>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    mut sounds: Option<ResMut<Messages<audio::WeaponSound>>>,
    mut cursor: ResMut<FxJournalCursor>,
    mut combat: ResMut<CombatFxDump>,
    mut ping_bus: ResMut<WeaponFirePingBus>,
    fx_world: FxSceneAccess,
) {
    let msec = host.0.msec_now;
    let combat_fx = weapons
        .as_deref()
        .and_then(|weapons| weapons.0.combat_fx_of(fire.event.payload.weapon));

    let eyes = match *view {
        ViewSubject::Seat {
            focus: Some(focus), ..
        } => i32::try_from(focus).unwrap_or(0),
        _ => i32::try_from(local.0.0).unwrap_or(0),
    };
    let gate = PlayerDrawGate {
        eyes_entity_num: eyes,
        other_flags: presented
            .player(local.0)
            .map(|ps| ps.other_flags)
            .unwrap_or(0),
        rendering_third_person: false,
    };
    let player_view = identities
        .get(fire.entity)
        .ok()
        .is_some_and(|identity| gate.is_player_view(identity.number()));
    combat.last_fire_player_view = Some(i64::from(player_view));
    // bo2mp: the gun of the scorestreak he rides (the VTOL Warship's, the
    // Dragonfire's) is his own: its first-person flash and sound.
    let riding = render_anim::occupancy::third_person::vehicle_view(&presented, local.0)
        .is_some_and(|v| v.number == fire.event.payload.number);
    let own_gun = player_view || riding;
    let last_shot = is_weapon_fire_last_shot_event(fire.event.event);
    combat.last_fire_lastshot = Some(i64::from(last_shot));
    combat.last_tracer_name = combat_fx.and_then(|fx| fx.tracer_hint.clone());
    combat.last_weapon_tracer_edge = combat_fx.map(|fx| fx.tracer.edge_kind().to_owned());
    combat.last_weapon_flash_edge =
        combat_fx.map(|fx| fx.flash_edge(own_gun).edge_kind().to_owned());
    let muzzle_name = combat_fx.and_then(|fx| fx.flash_present(own_gun));
    // bo2zm fix list 3: on the player's own gun, a far-view flash plays its
    // first-person copy (`asset_game::T6_FIRST_PERSON_FLASH_SCALE`).
    let first_person_name = muzzle_name
        .filter(|_| own_gun)
        .map(|n| (n.namespace, asset_game::t6_first_person_flash_name(n.name)))
        .filter(|(ns, name)| {
            catalog
                .as_deref()
                .is_some_and(|c| c.0.index_in(*ns, name).is_some())
        });
    let muzzle_name = match &first_person_name {
        Some((ns, name)) => Some(asset_game::FxName::new(*ns, name)),
        None => muzzle_name,
    };
    combat.last_muzzle_name = muzzle_name.map(|n| n.name.to_owned());

    let hand = usize::from(is_left_hand_fire_event(fire.event.event));
    let remote_bolts = world_bolts
        .get(fire.entity)
        .ok()
        .filter(|(runtime, _)| runtime.in_next_snap())
        .map(|(_, bolts)| bolts);
    let flash_target = if player_view {
        fpv_bolts.flash[hand]
    } else {
        remote_bolts.and_then(|bolts| bolts.flash)
    };
    let brass_target = if player_view {
        fpv_bolts.brass[hand]
    } else {
        remote_bolts.and_then(|bolts| bolts.brass)
    };
    if let Some(catalog) = catalog.as_deref() {
        elem_infos.0.sync(&catalog.0);
        let mut muzzle_played = cursor.muzzle_played;
        let mut muzzle_gap = cursor.muzzle_gap;

        // bo2mp: a gun on no player (a scorestreak's helicopter or turret)
        // has no flash bolt here: its flash plays where the server says its
        // muzzle is, turned along the shot.
        let gun_on_no_player = flash_target.is_none()
            && identities
                .get(fire.entity)
                .is_ok_and(|identity| identity.client().is_none());
        if try_play_weapon_fx_bolted(
            &mut host.0,
            &catalog.0,
            &mut elem_infos.0,
            muzzle_name,
            flash_target,
            &mut muzzle_played,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        ) {
            cursor.muzzle_bolted = cursor.muzzle_bolted.saturating_add(1);
        } else if !(gun_on_no_player
            && try_play_weapon_fx_at_origin(
                &mut host.0,
                &catalog.0,
                &mut elem_infos.0,
                muzzle_name,
                fire.event.payload.origin,
                math_iw4::angles_to_axis(fire.event.payload.direction),
                &mut muzzle_played,
                fx_world.view().as_ref().map(|s| s as &dyn FxScene),
            ))
        {
            muzzle_gap = muzzle_gap.saturating_add(1);
        }
        if muzzle_played > cursor.muzzle_played {
            combat.muzzle_msec = Some(msec);
        }
        cursor.muzzle_played = muzzle_played;
        cursor.muzzle_gap = muzzle_gap;
        play_shell_eject(
            &mut host.0,
            &catalog.0,
            &mut elem_infos.0,
            combat_fx,
            player_view,
            last_shot,
            brass_target,
            &mut cursor,
            &mut combat,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        );
    } else {
        cursor.muzzle_gap = cursor.muzzle_gap.saturating_add(1);
        cursor.brass_gap = cursor.brass_gap.saturating_add(1);
    }
    if let Some(facts) = weapons
        .as_deref()
        .and_then(|weapons| weapons.0.facts_of(fire.event.payload.weapon))
        && fire_weapon_fx_should_client_trace(facts.impact_type)
    {
        combat.last_impact_miss_why = Some("authority_segments".into());
    }
    let alias = identities.get(fire.entity).ok().and_then(|_| {
        let weapons = weapons.as_deref()?;
        let bank = sound_bank.as_deref()?;
        let weapon = fire.event.payload.weapon;
        audio::select_cg_fire_alias(
            last_shot,
            own_gun,
            weapons
                .0
                .weapon_sound_alias(weapon, asset_game::WeaponSoundSlot::Fire, &bank.0),
            weapons
                .0
                .weapon_sound_alias(weapon, asset_game::WeaponSoundSlot::FirePlayer, &bank.0),
            weapons
                .0
                .weapon_sound_alias(weapon, asset_game::WeaponSoundSlot::FireLast, &bank.0),
            weapons.0.weapon_sound_alias(
                weapon,
                asset_game::WeaponSoundSlot::FireLastPlayer,
                &bank.0,
            ),
        )
        .map(|alias| (alias, own_gun))
    });
    if let (Some((alias, player_view)), Some(sounds)) = (alias, sounds.as_deref_mut()) {
        combat.last_fire_alias = Some(alias.to_owned());

        let sound_origin = flash_target
            .map(|target| target.orientation.origin)
            .unwrap_or(fire.event.payload.origin);
        sounds.write(audio::WeaponSound {
            namespace: weapons
                .as_deref()
                .and_then(|w| w.0.namespace_of(fire.event.payload.weapon))
                .unwrap_or(asset_core::AssetNamespace::Iw4),
            alias: alias.to_owned(),
            origin_inches: (!player_view).then_some(sound_origin),
            snd_ent: audio::ent_from_number(fire.event.payload.number),
        });
    } else {
        combat.last_fire_alias = None;
        cursor.fire_sound_gap = cursor.fire_sound_gap.saturating_add(1);
    }

    let local_number = i32::try_from(local.0.0).unwrap_or(-1);
    let is_grenade = weapons.as_deref().is_some_and(|weapons| {
        weapons
            .0
            .facts_of(fire.event.payload.weapon)
            .is_some_and(|facts| facts.weap_type == WEAPTYPE_GRENADE)
    });
    if fire.event.payload.number != local_number && !player_view && !is_grenade {
        ping_bus.pings.push(WeaponFirePing {
            number: fire.event.payload.number,
            origin_xy: [fire.event.payload.origin[0], fire.event.payload.origin[1]],
        });
    }
    log_combat_fx_gaps(&mut cursor, &combat);
    sync_combat_dump(&cursor, &mut combat);
}

fn eject_brass(
    brass: On<net::EntityEjectBrass>,
    identities: Query<&CEntity>,
    world_bolts: Query<(
        &net::CEntityRuntime,
        &crate::adapters::anim::remote_body::RemoteFxBolts,
    )>,
    fpv_bolts: Res<crate::adapters::anim::fpv_present::FpvBoltTargets>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    mut cursor: ResMut<FxJournalCursor>,
    mut combat: ResMut<CombatFxDump>,
    fx_world: FxSceneAccess,
) {
    let combat_fx = weapons
        .as_deref()
        .and_then(|weapons| weapons.0.combat_fx_of(brass.event.payload.weapon));
    let player_view = identities
        .get(brass.entity)
        .ok()
        .is_some_and(|identity| identity.client() == Some(local.0));
    let last_shot = is_weapon_fire_last_shot_event(brass.event.event);
    let hand = usize::from(is_left_hand_fire_event(brass.event.event));
    let target = if player_view {
        fpv_bolts.brass[hand]
    } else {
        world_bolts
            .get(brass.entity)
            .ok()
            .filter(|(runtime, _)| runtime.in_next_snap())
            .and_then(|(_, bolts)| bolts.brass)
    };
    if let Some(catalog) = catalog.as_deref() {
        elem_infos.0.sync(&catalog.0);
        play_shell_eject(
            &mut host.0,
            &catalog.0,
            &mut elem_infos.0,
            combat_fx,
            player_view,
            last_shot,
            target,
            &mut cursor,
            &mut combat,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        );
    } else {
        cursor.brass_gap = cursor.brass_gap.saturating_add(1);
    }
    log_combat_fx_gaps(&mut cursor, &combat);
    sync_combat_dump(&cursor, &mut combat);
}

fn tick_missile_present_state(
    occupancy: Option<Res<crate::adapters::anim::missile::MissileOccupancy>>,
    weapons: Option<Res<assets::PreparedWeapons>>,
    projectile_meshes: Option<Res<assets::PreparedProjectileMeshes>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    mut bolts: ResMut<crate::adapters::anim::missile::MissileBoltState>,
    poses: Option<Res<crate::adapters::anim::dobj_pose::HostDObjPoseFrame>>,
    sound_bank: Option<Res<audio::SoundBank>>,
    mut sounds: Option<ResMut<Messages<audio::WeaponSound>>>,
    fx_world: FxSceneAccess,
) {
    let Some(occupancy) = occupancy.as_deref() else {
        return;
    };
    let Some(weapons) = weapons
        .as_deref()
        .map(|prepared| &prepared.0)
        .filter(|registry| !registry.is_empty())
    else {
        return;
    };
    let Some(meshes) = projectile_meshes
        .as_deref()
        .map(|prepared| &prepared.0)
        .filter(|catalog| !catalog.is_empty())
    else {
        return;
    };
    let Some(catalog) = catalog.as_deref().map(|catalog| &catalog.0) else {
        return;
    };
    elem_infos.0.sync(catalog);
    let mut live = Vec::with_capacity(occupancy.rows.len());
    for row in &occupancy.rows {
        let Some(entnum) = row.entnum else {
            bolts.predicted_rows_skipped = bolts.predicted_rows_skipped.saturating_add(1);
            continue;
        };
        live.push(entnum);
        let (want_trail, want_beacon, want_ignition) = {
            let state = bolts.rows.entry(entnum).or_insert(
                crate::adapters::anim::missile::MissileBoltRow {
                    projectile: row.id,
                    weapon: row.weapon,
                    trail_played: false,
                    beacon_played: false,
                    ignition_played: false,
                    ignition_fx_played: false,
                },
            );
            if state.weapon != row.weapon || state.projectile != row.id {
                *state = crate::adapters::anim::missile::MissileBoltRow {
                    projectile: row.id,
                    weapon: row.weapon,
                    trail_played: false,
                    beacon_played: false,
                    ignition_played: false,
                    ignition_fx_played: false,
                };
            }
            (
                row.ignited && !state.trail_played,
                !state.beacon_played,
                row.ignited && !state.ignition_played,
            )
        };
        let mark = |bolts: &mut crate::adapters::anim::missile::MissileBoltState,
                    entnum: u32,
                    f: fn(&mut crate::adapters::anim::missile::MissileBoltRow)| {
            if let Some(state) = bolts.rows.get_mut(&entnum) {
                f(state);
            }
        };

        if want_trail && let Some(name) = weapons.proj_trail_of(row.weapon) {
            match missile_bolt_target(poses.as_deref(), meshes, row.namespace, &row.name, entnum) {
                Some(target) => {
                    let mut played = 0;
                    if try_play_weapon_fx_bolted(
                        &mut host.0,
                        catalog,
                        &mut elem_infos.0,
                        Some(name),
                        Some(target),
                        &mut played,
                        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
                    ) {
                        mark(&mut bolts, entnum, |state| state.trail_played = true);
                    } else {
                        bolts.play_gaps = bolts.play_gaps.saturating_add(1);
                    }
                }
                None => bolts.pose_gaps = bolts.pose_gaps.saturating_add(1),
            }
        }

        if want_beacon && let Some(name) = weapons.proj_beacon_of(row.weapon) {
            match missile_bolt_target(poses.as_deref(), meshes, row.namespace, &row.name, entnum) {
                Some(target) => {
                    let mut played = 0;
                    if try_play_weapon_fx_bolted(
                        &mut host.0,
                        catalog,
                        &mut elem_infos.0,
                        Some(name),
                        Some(target),
                        &mut played,
                        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
                    ) {
                        mark(&mut bolts, entnum, |state| state.beacon_played = true);
                    } else {
                        bolts.play_gaps = bolts.play_gaps.saturating_add(1);
                    }
                }
                None => bolts.pose_gaps = bolts.pose_gaps.saturating_add(1),
            }
        }

        if row.ignited
            && !bolts.rows[&entnum].ignition_fx_played
            && let Some(name) = weapons.proj_ignition_of(row.weapon)
            && let Some(target) =
                missile_bolt_target(poses.as_deref(), meshes, row.namespace, &row.name, entnum)
        {
            let mut played = 0;
            if try_play_weapon_fx_bolted(
                &mut host.0,
                catalog,
                &mut elem_infos.0,
                Some(name),
                Some(target),
                &mut played,
                fx_world.view().as_ref().map(|s| s as &dyn FxScene),
            ) {
                mark(&mut bolts, entnum, |state| state.ignition_fx_played = true);
            } else {
                bolts.play_gaps = bolts.play_gaps.saturating_add(1);
            }
        }

        if want_ignition
            && weapons
                .authored_weapon_sound(row.weapon, asset_game::WeaponSoundSlot::ProjIgnition)
                .is_some()
        {
            match sound_bank.as_deref().and_then(|bank| {
                weapons.weapon_sound_alias(
                    row.weapon,
                    asset_game::WeaponSoundSlot::ProjIgnition,
                    &bank.0,
                )
            }) {
                Some(alias) => {
                    if let Some(sounds) = sounds.as_deref_mut() {
                        sounds.write(audio::WeaponSound {
                            namespace: weapons
                                .namespace_of(row.weapon)
                                .unwrap_or(asset_core::AssetNamespace::Iw4),
                            alias: alias.to_owned(),
                            origin_inches: Some(row.origin),
                            snd_ent: Some(entnum),
                        });
                        mark(&mut bolts, entnum, |state| state.ignition_played = true);
                    }
                }
                None => bolts.ignition_gaps = bolts.ignition_gaps.saturating_add(1),
            }
        }
    }
    bolts.rows.retain(|entnum, _| live.contains(entnum));
}

fn explosion(
    explosion: On<net::EntityExplosion>,
    weapons: Option<Res<PreparedWeapons>>,
    sound_bank: Option<Res<audio::SoundBank>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    impact_fx: Option<Res<PreparedImpactFx>>,
    mut host: ResMut<HostFxSystem>,
    mut sounds: Option<ResMut<Messages<audio::WeaponSound>>>,
    mut cursor: ResMut<FxJournalCursor>,
    mut combat: ResMut<CombatFxDump>,
    fx_world: FxSceneAccess,
) {
    let msec = host.0.msec_now;
    let payload = explosion.event.payload;
    let impact_type = weapons
        .as_deref()
        .and_then(|weapons| weapons.0.facts_of(payload.weapon))
        .map(|facts| facts.impact_type);
    let combat_fx = weapons
        .as_deref()
        .and_then(|weapons| weapons.0.combat_fx_of(payload.weapon));
    combat.last_weapon_explosion_edge = combat_fx.map(|fx| fx.explosion.edge_kind().to_owned());
    let slot = combat_fx.and_then(|fx| fx.explosion_present());
    let names = explosion_fx_names(
        impact_type,
        payload.surf_type,
        impact_fx.as_ref().and_then(|fx| fx.0.as_ref()),
        slot,
    );
    combat.last_explosion_table = names.table.map(|n| n.name.to_owned());
    combat.last_explosion_slot = names.slot.map(|n| n.name.to_owned());
    combat.last_explosion_name = names.table.or(names.slot).map(|n| n.name.to_owned());
    if let Some(row) = names.row {
        combat.last_row = Some(row as i64);
        combat.last_surf = Some(i64::from(payload.surf_type));
        combat.last_impact_def = names.table.map(|n| n.name.to_owned());
    }
    let axis = if payload.direction == [0.0, 0.0, 0.0] {
        IDENTITY_AXIS
    } else {
        axis_from_hit_normal(payload.direction)
    };
    if let Some(catalog) = catalog.as_deref() {
        elem_infos.0.sync(&catalog.0);
        let mut boom_played = cursor.boom_played;
        let table_ok = try_play_weapon_fx_at_origin(
            &mut host.0,
            &catalog.0,
            &mut elem_infos.0,
            names.table,
            payload.origin,
            axis,
            &mut boom_played,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        );
        let slot_ok = try_play_weapon_fx_at_origin(
            &mut host.0,
            &catalog.0,
            &mut elem_infos.0,
            names.slot,
            payload.origin,
            axis,
            &mut boom_played,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        );
        cursor.boom_played = boom_played;
        if table_ok || slot_ok {
            combat.explosion_msec = Some(msec);
        }
        if !table_ok && !slot_ok {
            cursor.explosion_gap = cursor.explosion_gap.saturating_add(1);
        }
    } else {
        cursor.explosion_gap = cursor.explosion_gap.saturating_add(1);
    }
    let alias = weapons
        .as_deref()
        .zip(sound_bank.as_deref())
        .and_then(|(weapons, bank)| {
            weapons.0.weapon_sound_alias(
                payload.weapon,
                asset_game::WeaponSoundSlot::ProjectileExplosion,
                &bank.0,
            )
        });
    if let (Some(alias), Some(sounds)) = (alias, sounds.as_deref_mut()) {
        sounds.write(audio::WeaponSound {
            namespace: weapons
                .as_deref()
                .and_then(|w| w.0.namespace_of(payload.weapon))
                .unwrap_or(asset_core::AssetNamespace::Iw4),
            alias: alias.to_owned(),
            origin_inches: Some(payload.origin),
            snd_ent: audio::ent_from_number(payload.number),
        });
    } else {
        cursor.explosion_sound_gap = cursor.explosion_sound_gap.saturating_add(1);
    }
    log_combat_fx_gaps(&mut cursor, &combat);
    sync_combat_dump(&cursor, &mut combat);
}

const KILLCAM_FX_REMOVAL_WEAPONS: [&str; 1] = ["remotemissile_projectile_mp"];

fn stop_killcam_explosion_fx(
    _transition: On<net::KillcamFxTransition>,
    weapons: Option<Res<PreparedWeapons>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    prediction: Res<ClientPredictionState>,
    mut host: ResMut<HostFxSystem>,
) {
    let (Some(weapons), Some(catalog)) = (weapons, catalog) else {
        return;
    };
    let delta_time = prediction.0.predicted_local().map_or(0, |ps| ps.delta_time);
    let newer_than = host.0.msec_now.wrapping_sub(delta_time);
    for name in KILLCAM_FX_REMOVAL_WEAPONS {
        let Ok(Some(weapon)) = weapons.0.resolve_index(name) else {
            continue;
        };
        let Some(effect) = weapons
            .0
            .combat_fx_of(weapon)
            .and_then(|fx| fx.explosion_present())
            .and_then(|name| name.resolve(&catalog.0))
        else {
            continue;
        };
        host.0.kill_def_newer_than(&effect.name, newer_than);
    }
}

#[derive(Default)]
struct ScriptFxRow {
    effect: u8,
    start: Option<i32>,
    held: Option<u16>,
    next_ms: i32,
}

fn axis(forward: [f32; 3], up: [f32; 3]) -> [[f32; 3]; 3] {
    let forward = Vec3::from_array(forward);
    let up = Vec3::from_array(up);
    [
        forward.to_array(),
        up.cross(forward).to_array(),
        up.to_array(),
    ]
}

fn sync_script_fx(
    adopted: Option<Res<LastAdoptedSnapshot>>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    camera: Option<Res<FxCameraOrigin>>,
    fx_world: FxSceneAccess,
    mut rows: Local<HashMap<u32, ScriptFxRow>>,
) {
    let (Some(adopted), Some(catalog)) = (adopted, catalog) else {
        return;
    };
    let Some(snap) = adopted.next() else {
        return;
    };
    let effects = &snap.meta.objectives.effects;
    rows.retain(|id, row| {
        let keep = effects
            .iter()
            .any(|fx| fx.id == *id && fx.effect == row.effect);
        if !keep && let Some(handle) = row.held {
            host.0.stop_owned(handle);
        }
        keep
    });
    if effects.is_empty() {
        return;
    }
    let server_now = sim::level_time_ms(snap.tick);
    let offset = host.0.msec_now.wrapping_sub(server_now);
    let eye = camera.map(|c| Vec3::from_array(c.0));
    elem_infos.0.sync(&catalog.0);
    let scene = fx_world.view();
    let scene = scene.as_ref().map(|s| s as &dyn FxScene);
    for fx in effects {
        let row = rows.entry(fx.id).or_insert_with(|| ScriptFxRow {
            effect: fx.effect,
            ..Default::default()
        });
        let Some(name) = adopted.effect_name(fx.effect) else {
            continue;
        };
        let name = catalog.0.map_fx_name(name);
        let axis = axis(fx.forward, fx.up);
        let retriggered = row.start != fx.start_ms;
        row.start = fx.start_ms;
        let Some(start) = fx.start_ms else {
            continue;
        };
        if fx.repeat_ms <= 0 {
            if retriggered {
                if let Some(handle) = row.held.take() {
                    host.0.stop_owned(handle);
                }
                row.held = spawn_named_oriented_in_world(
                    &mut host.0,
                    &catalog.0,
                    &elem_infos.0,
                    name,
                    fx.origin,
                    axis,
                    start.wrapping_add(offset),
                    scene,
                )
                .and_then(|result| match result {
                    PlayResult::Held { handle } => Some(handle),
                    _ => None,
                });
            }
            continue;
        }
        if retriggered {
            row.next_ms = start;
        }
        let behind = server_now.saturating_sub(row.next_ms);
        if behind > fx.repeat_ms.saturating_mul(4) {
            row.next_ms = server_now - behind % fx.repeat_ms;
        }
        while row.next_ms <= server_now {
            let culled = fx.cull_distance > 0.0
                && eye.is_some_and(|eye| {
                    eye.distance(Vec3::from_array(fx.origin)) > fx.cull_distance
                });
            if !culled {
                play_named_oriented_at_msec(
                    &mut host.0,
                    &catalog.0,
                    &elem_infos.0,
                    name,
                    fx.origin,
                    axis,
                    row.next_ms.wrapping_add(offset),
                    scene,
                );
            }
            row.next_ms = row.next_ms.saturating_add(fx.repeat_ms);
        }
    }
}

fn play_fx(
    play: On<net::EntityPlayFx>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    adopted: Option<Res<LastAdoptedSnapshot>>,
    mut host: ResMut<HostFxSystem>,
    mut presented: ResMut<PresentedVehicleFx>,
    fx_world: FxSceneAccess,
) {
    let Some(catalog) = catalog else {
        return;
    };
    let payload = play.event.payload;
    let index = u8::try_from(payload.event_parm).unwrap_or(0);
    let owner = (payload.correlation != 0).then_some(payload.correlation);
    let Some(def_name) = adopted
        .as_ref()
        .and_then(|snap| snap.effect_name(index).map(str::to_owned))
    else {
        if let Some(owner) = owner {
            presented.by_id.insert(
                owner,
                PresentedVehicleFxRow {
                    fx_spawn: Some("miss_cs"),
                    fx_spawn_def: None,
                },
            );
        }
        return;
    };
    let normal = if payload.direction == [0.0, 0.0, 0.0] {
        [0.0, 0.0, 1.0]
    } else {
        payload.direction
    };
    elem_infos.0.sync(&catalog.0);
    // bo2zm M3: an effect played with its up as well (BO2's
    // playfx(fx, origin, forward, up): the wall buys' chalk) carries the up
    // in origin2.
    let axis = if payload.origin2 == [0.0, 0.0, 0.0] {
        axis_from_hit_normal(normal)
    } else {
        let f = fx_iw4::vec3_normalize(normal);
        let u0 = payload.origin2;
        let d = u0[0] * f[0] + u0[1] * f[1] + u0[2] * f[2];
        let up = fx_iw4::vec3_normalize([u0[0] - f[0] * d, u0[1] - f[1] * d, u0[2] - f[2] * d]);
        let left = [
            up[1] * f[2] - up[2] * f[1],
            up[2] * f[0] - up[0] * f[2],
            up[0] * f[1] - up[1] * f[0],
        ];
        [f, left, up]
    };
    // bo2zm M3: an effect taken off entity `number`'s bone (a zombie's eye
    // glow going out).
    if payload.simulation_flags & 0x80 != 0 {
        if let Some(def) = catalog.0.map_fx_name(&def_name).resolve(&catalog.0) {
            host.0.stop_bolted(
                &def.name,
                payload.number as u32,
                u16::from(payload.surf_type),
            );
        }
        return;
    }
    // bo2zm M3: BO2's playfxontag: bolted to entity `number`'s bone
    // `surf_type` (its pose published by its model), gone when it goes.
    let played = if payload.simulation_flags & 0x40 != 0 {
        render_fx::present::play_named_bolted_in_world(
            &mut host.0,
            &catalog.0,
            &elem_infos.0,
            catalog.0.map_fx_name(&def_name),
            fx::FxBoltTarget {
                dobj: payload.number as u32,
                bone: u16::from(payload.surf_type),
                centity_teleport: false,
                orientation: fx::FxBoltOrientation {
                    origin: payload.origin,
                    axis,
                },
            },
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        )
    // BO2's playfx keeps a looping effect going (the chalk stays on the
    // wall), and so does spawnfx (0x20): those are spawned held, not played
    // and released.
    } else if payload.origin2 == [0.0, 0.0, 0.0] && payload.simulation_flags & 0x20 == 0 {
        play_named_oriented_in_world(
            &mut host.0,
            &catalog.0,
            &elem_infos.0,
            catalog.0.map_fx_name(&def_name),
            payload.origin,
            axis,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        )
    } else {
        let msec = host.0.msec_now;
        spawn_named_oriented_in_world(
            &mut host.0,
            &catalog.0,
            &elem_infos.0,
            catalog.0.map_fx_name(&def_name),
            payload.origin,
            axis,
            msec,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        )
    };
    let spawn = match played {
        Some(PlayResult::PlayedReleased { .. } | PlayResult::Held { .. }) => "spawned",
        Some(PlayResult::Failed(_)) | None => "miss_def",
    };
    let log = std::env::var("IW4L_FX_AXIS_LOG").ok();
    if log.as_deref() == Some("all")
        || (log.is_some()
            && (payload.origin2 != [0.0, 0.0, 0.0] || payload.simulation_flags & 0x40 != 0))
    {
        diag::info!(
            World,
            "bo2zm fx axis: {def_name} at {:?} axis {:?} bolt {}:{} -> {spawn}",
            payload.origin,
            axis,
            payload.number,
            payload.surf_type
        );
    }
    if let Some(owner) = owner {
        presented.by_id.insert(
            owner,
            PresentedVehicleFxRow {
                fx_spawn: Some(spawn),
                fx_spawn_def: Some(def_name),
            },
        );
    }
}

fn play_fx_bullet_hit(hit: On<net::EntityBulletHit>, mut hits: MessageWriter<BulletHitFx>) {
    let own = matches!(
        hit.event.event,
        entity_iw4::EntityEventKind::BULLET_HIT_CLIENT_SMALL
            | entity_iw4::EntityEventKind::BULLET_HIT_CLIENT_LARGE
    );
    hits.write(BulletHitFx(hit.event.payload, own));
}

fn drain_bullet_hit_fx(
    mut hits: MessageReader<BulletHitFx>,
    world_bolts: Query<&crate::adapters::anim::remote_body::RemoteFxBolts>,
    fpv_bolts: Res<crate::adapters::anim::fpv_present::FpvBoltTargets>,
    slots: Res<CEntitySlots>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    impact_fx: Option<Res<PreparedImpactFx>>,
    weapons: Option<Res<PreparedWeapons>>,
    tracers: Option<Res<PreparedTracers>>,
    mut tracer_world: ResMut<TracerWorld>,
    mut gate: ResMut<TracerDrawGate>,
    local: Res<LocalPresentClient>,
    mut host: ResMut<HostFxSystem>,
    mut cursor: ResMut<FxJournalCursor>,
    mut combat: ResMut<CombatFxDump>,
    fx_world: FxSceneAccess,
) {
    for hit in hits.read() {
        let payload = hit.0;
        let impact = !hit.1;

        let previous_mark_entity = host.0.spawn_mark_entity;
        host.0.spawn_mark_entity = u16::try_from(payload.other_entity_num)
            .ok()
            .filter(|&n| u32::from(n) < fx_iw4::FX_ENTITYNUM_WORLD);
        play_pellet_segment(
            payload.attacker_entity_num,
            payload.weapon,
            payload.correlation,
            payload.pellet,
            payload.hand,
            payload.origin2,
            payload.origin,
            payload.direction,
            payload.surf_type,
            payload.surface_flags,
            payload.event_parm as u32,
            impact,
            &world_bolts,
            &fpv_bolts,
            &slots,
            catalog.as_deref(),
            &mut elem_infos.0,
            impact_fx.as_deref(),
            weapons.as_deref(),
            tracers.as_deref(),
            &mut tracer_world,
            &mut gate,
            local.0.0 as i32,
            &mut host,
            &mut cursor,
            &mut combat,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        );
        host.0.spawn_mark_entity = previous_mark_entity;
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_pellet_fx(
    mut pending: ResMut<PendingPelletFx>,
    world_bolts: Query<&crate::adapters::anim::remote_body::RemoteFxBolts>,
    fpv_bolts: Res<crate::adapters::anim::fpv_present::FpvBoltTargets>,
    slots: Res<CEntitySlots>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    impact_fx: Option<Res<PreparedImpactFx>>,
    weapons: Option<Res<PreparedWeapons>>,
    tracers: Option<Res<PreparedTracers>>,
    mut tracer_world: ResMut<TracerWorld>,
    mut gate: ResMut<TracerDrawGate>,
    local: Res<LocalPresentClient>,
    mut host: ResMut<HostFxSystem>,
    mut cursor: ResMut<FxJournalCursor>,
    mut combat: ResMut<CombatFxDump>,
    fx_world: FxSceneAccess,
) {
    if pending.0.is_empty() {
        return;
    }
    for record in core::mem::take(&mut pending.0) {
        cursor.pellet_played = cursor.pellet_played.saturating_add(1);
        play_pellet_segment(
            record.attacker,
            record.weapon,
            record.correlation,
            record.pellet,
            record.hand,
            record.start,
            record.end,
            record.normal,
            record.surf_type,
            record.surface_flags,
            u32::from(record.flesh_flags),
            true,
            &world_bolts,
            &fpv_bolts,
            &slots,
            catalog.as_deref(),
            &mut elem_infos.0,
            impact_fx.as_deref(),
            weapons.as_deref(),
            tracers.as_deref(),
            &mut tracer_world,
            &mut gate,
            local.0.0 as i32,
            &mut host,
            &mut cursor,
            &mut combat,
            fx_world.view().as_ref().map(|s| s as &dyn FxScene),
        );
    }
}

fn melee_blood(
    hit: On<net::EntityMeleeBlood>,
    identities: Query<&CEntity>,
    world_bolts: Query<&crate::adapters::anim::remote_body::RemoteFxBolts>,
    fpv_bolts: Res<crate::adapters::anim::fpv_present::FpvBoltTargets>,
    local: Res<LocalPresentClient>,
    presented: Res<PresentedSnapshot>,
    view: Res<ViewSubject>,
    slots: Res<CEntitySlots>,
    catalog: Option<Res<PreparedFxCatalog>>,
    mut elem_infos: ResMut<PreparedFxElemInfos>,
    mut host: ResMut<HostFxSystem>,
    mut cursor: ResMut<FxJournalCursor>,
    fx_world: FxSceneAccess,
) {
    let payload = hit.event.payload;
    let Some(catalog) = catalog else {
        return;
    };
    let eyes = match *view {
        ViewSubject::Seat {
            focus: Some(focus), ..
        } => i32::try_from(focus).unwrap_or(0),
        _ => i32::try_from(local.0.0).unwrap_or(0),
    };
    let gate = PlayerDrawGate {
        eyes_entity_num: eyes,
        other_flags: presented
            .player(local.0)
            .map(|ps| ps.other_flags)
            .unwrap_or(0),
        rendering_third_person: false,
    };
    let player_view = identities
        .get(hit.entity)
        .ok()
        .is_some_and(|identity| gate.is_player_view(identity.number()));
    let target = if player_view {
        fpv_bolts.knife[0].or(fpv_bolts.knife[1])
    } else {
        u16::try_from(payload.number)
            .ok()
            .and_then(|number| slots.entity_for_number(number))
            .and_then(|entity| world_bolts.get(entity).ok())
            .and_then(|bolts| bolts.knife)
    };
    let mut played = 0u32;
    try_play_weapon_fx_bolted(
        &mut host.0,
        &catalog.0,
        &mut elem_infos.0,
        Some(asset_game::FxName::engine("impacts/flesh_hit_knife")),
        target,
        &mut played,
        fx_world.view().as_ref().map(|s| s as &dyn FxScene),
    );
    cursor.impact_played = cursor.impact_played.saturating_add(played);
}
