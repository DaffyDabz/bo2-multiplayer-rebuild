//! The loops a destructible speaks while it sits in a stage: gas hissing out
//! of a punctured tank, a propane fire burning on the cap. The host publishes
//! which alias each prop is speaking and where; this reconciles that set
//! against the positional emitters that carry it. A loop on something that
//! moves (a helicopter's engine, a spy plane) follows it, and its volume and
//! pitch follow the row (BO2's `setloopstate`).

use crate::ambient::{MapAmbient, MapEmitter, SoundBankNamespace};
use crate::clip_store::{ClipStore, clip_keys_for_alias};
use crate::pcm::PcmAudio;
use crate::playback::{MissingAliasGaps, SharedPlayAssets, SoundBank};
use asset_core::AssetNamespace;
use bevy::prelude::*;
use net::PresentedSnapshot;
use sim::DestructibleLoopSound;
use std::sync::Arc;

#[derive(Component)]
pub(crate) struct DestructibleLoop {
    owner: u32,
    alias: String,
    /// The alias's own level (`vol_min`), scaled by the row's volume.
    vol: f32,
    /// What the log last said about its level (IW4L_SOUND_LOG).
    logged: (u8, u8, f64),
    /// Whether the log last saw a voice playing it (among the loudest map
    /// emitters and above the audible floor).
    voiced: bool,
}

fn sound_log() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| std::env::var_os("IW4L_SOUND_LOG").is_some())
}

pub(crate) fn update(
    mut commands: Commands,
    presented: Res<PresentedSnapshot>,
    mut playing: Query<(
        Entity,
        &mut DestructibleLoop,
        &mut MapEmitter,
        &mut Transform,
        Has<crate::backend::Voice>,
    )>,
    time: Res<Time>,
    bank: Option<Res<SoundBank>>,
    namespace: Option<Res<SoundBankNamespace>>,
    mut clips: Option<ResMut<ClipStore>>,
    mut pcm: ResMut<Assets<PcmAudio>>,
    mut shared: ResMut<SharedPlayAssets>,
    mut gaps: ResMut<MissingAliasGaps>,
) {
    let Some(snapshot) = presented.snapshot() else {
        return;
    };
    let speaking: Vec<(&DestructibleLoopSound, &str)> = snapshot
        .meta
        .world_objects
        .destructible_loop_sounds
        .iter()
        .filter_map(|row| {
            let alias = snapshot
                .meta
                .sound_aliases
                .iter()
                .find(|(index, _)| *index == row.alias_index)?;
            Some((row, alias.1.as_str()))
        })
        .collect();

    let now = time.elapsed_secs_f64();
    for (entity, mut loop_sound, mut emitter, mut transform, voiced) in &mut playing {
        let Some((row, _)) = speaking.iter().find(|(row, alias)| {
            row.owner.to_wire() == loop_sound.owner && *alias == loop_sound.alias
        }) else {
            commands.entity(entity).try_despawn();
            diag::info!(
                Audio,
                "audio: destructible loop `{}` on {:#x} stopped at {:?}",
                loop_sound.alias,
                loop_sound.owner,
                emitter.origin_inches.map(|v| v.round())
            );
            continue;
        };
        if sound_log() && voiced != loop_sound.voiced {
            loop_sound.voiced = voiced;
            diag::info!(
                Audio,
                "audio: destructible loop `{}` on {:#x} voice {} at {:?}",
                loop_sound.alias,
                loop_sound.owner,
                if voiced { "playing" } else { "dropped (out of range or not loudest)" },
                emitter.origin_inches.map(|v| v.round())
            );
        }
        if emitter.origin_inches != row.origin {
            emitter.origin_inches = row.origin;
            transform.translation = Vec3::from_array(row.origin);
        }
        let base_gain = loop_sound.vol * f32::from(row.volume) / 100.0;
        if emitter.base_gain != base_gain {
            emitter.base_gain = base_gain;
        }
        let speed = f32::from(row.pitch.max(1)) / 100.0;
        if emitter.speed != speed {
            emitter.speed = speed;
        }
        if sound_log()
            && (row.volume, row.pitch) != (loop_sound.logged.0, loop_sound.logged.1)
            && now - loop_sound.logged.2 >= 1.0
        {
            loop_sound.logged = (row.volume, row.pitch, now);
            diag::info!(
                Audio,
                "audio: destructible loop `{}` on {:#x} level vol={} pitch={} at {:?}",
                loop_sound.alias,
                loop_sound.owner,
                row.volume,
                row.pitch,
                row.origin.map(|v| v.round())
            );
        }
    }

    let (Some(bank), Some(clips)) = (bank, clips.as_mut()) else {
        return;
    };
    let ns = namespace.map_or(AssetNamespace::Iw4, |map| map.namespace);
    for (row, alias) in speaking {
        if playing.iter().any(|(_, playing, _, _, _)| {
            playing.owner == row.owner.to_wire() && playing.alias == alias
        }) {
            continue;
        }
        let Some(key) = clip_keys_for_alias(&bank.0, ns, alias).into_iter().next() else {
            gaps.record(alias);
            continue;
        };
        clips.request(key.clone());
        let Some(Ok(audio)) = clips.ready(&key) else {
            continue;
        };
        let Some(sound) = bank
            .0
            .sound_in(ns, alias)
            .or_else(|| {
                bank.0
                    .index_unique(alias)
                    .and_then(|index| bank.0.sounds.get(index))
            })
            .and_then(|s| s.aliases.first())
        else {
            gaps.record(alias);
            continue;
        };
        let knots = sound
            .volume_falloff
            .as_ref()
            .map(|curve| shared.intern_curve(&curve.name, &curve.knots))
            .unwrap_or_else(|| Arc::from(Vec::<[f32; 2]>::new()));
        if knots.is_empty() {
            diag::warn!(
                Audio,
                "audio: destructible loop `{alias}` has no falloff curve (typed gap)"
            );
        }
        let vol = sound.vol_min.max(0.0);
        commands.spawn((
            DestructibleLoop {
                owner: row.owner.to_wire(),
                alias: alias.to_owned(),
                vol,
                logged: (row.volume, row.pitch, now),
                voiced: false,
            },
            MapAmbient,
            MapEmitter {
                origin_inches: row.origin,
                dist_min: sound.dist_min,
                dist_max: sound.dist_max,
                knots,
                base_gain: vol * f32::from(row.volume) / 100.0,
                pcm: pcm.add(audio),
                speed: f32::from(row.pitch.max(1)) / 100.0,
                live_pan: None,
            },
            Transform::from_translation(Vec3::from_array(row.origin)),
        ));
        diag::info!(
            Audio,
            "audio: destructible loop `{alias}` on {:#x} vol={} pitch={} at {:?}",
            row.owner.to_wire(),
            row.volume,
            row.pitch,
            row.origin.map(|v| v.round())
        );
    }
}
