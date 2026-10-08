//! bo2zm: the Zombies music bed. Black Ops II's scripts set a music state
//! (`setmusicstate("WAVE")`) that its client scripts turn into a looping
//! track; the server sends the track's alias as the client dvar
//! `bo2zm_music` (empty for silence) and this keeps that one loop playing
//! (2D, the alias's own volume) for the match.

use asset_core::AssetNamespace;
use bevy::audio::Volume;
use bevy::prelude::*;
use net::{LocalPresentClient, PresentedSnapshot};

use crate::backend::{AudioScope, MatchEpoch};
use crate::clip_store::{ClipStore, clip_keys_for_alias};
use crate::pcm::LoopingPcmAudio;
use crate::playback::{MissingAliasGaps, SoundBank};

#[derive(Component)]
pub(crate) struct ZmMusicBed {
    alias: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    mut commands: Commands,
    presented: Option<Res<PresentedSnapshot>>,
    local: Option<Res<LocalPresentClient>>,
    bank: Option<Res<SoundBank>>,
    mut clips: Option<ResMut<ClipStore>>,
    mut looping_assets: ResMut<Assets<LoopingPcmAudio>>,
    mut gaps: ResMut<MissingAliasGaps>,
    epoch: Res<MatchEpoch>,
    playing: Query<(Entity, &ZmMusicBed)>,
) {
    let (Some(presented), Some(local)) = (presented, local) else {
        return;
    };
    let Some(snap) = presented.snapshot() else {
        return;
    };
    let wanted = snap
        .meta
        .script_dvars(local.0)
        .string("bo2zm_music")
        .unwrap_or_default()
        .to_owned();
    for (entity, bed) in &playing {
        if bed.alias != wanted {
            crate::backend::stop(&mut commands, entity);
            diag::info!(Audio, "audio: zombies music `{}` stopped", bed.alias);
        }
    }
    if wanted.is_empty() || playing.iter().any(|(_, bed)| bed.alias == wanted) {
        return;
    }
    let (Some(bank), Some(clips)) = (bank, clips.as_mut()) else {
        return;
    };
    let Some(key) = clip_keys_for_alias(&bank.0, AssetNamespace::T6, &wanted)
        .into_iter()
        .next()
    else {
        gaps.record(&wanted);
        return;
    };
    clips.request(key.clone());
    let Some(Ok(pcm)) = clips.ready(&key) else {
        return;
    };
    let volume = bank
        .0
        .sound_in(AssetNamespace::T6, &wanted)
        .and_then(|s| s.aliases.first())
        .map_or(0.8, |a| a.vol_min.max(0.0));
    let handle = looping_assets.add(pcm.into_looping());
    let entity = crate::backend::spawn_loop(
        &mut commands,
        handle,
        Volume::Linear(volume),
        epoch.0,
        AudioScope::Match,
    );
    commands.entity(entity).insert(ZmMusicBed {
        alias: wanted.clone(),
    });
    diag::info!(Audio, "audio: zombies music `{wanted}` at {volume:.2}");
}
