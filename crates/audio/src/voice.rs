use std::collections::HashMap;

use asset_core::AssetNamespace;
use asset_iw4::{SND_ENTCHANNEL_MAX, entity_channel_matches};
use bevy::prelude::*;

#[derive(Component, Clone, Copy, Debug)]
pub struct VoiceLease {
    pub channel: u32,
    pub snd_ent: Option<u32>,
}

#[derive(Component, Clone, Debug)]
pub(crate) struct AliasVoiceLease {
    namespace: AssetNamespace,
    alias: String,
    snd_ent: Option<u32>,
    priority: f32,
    sequence: u64,
}

#[derive(Resource, Debug)]
pub struct VoiceOccupancy {
    counts: [i32; SND_ENTCHANNEL_MAX],
    live: HashMap<Entity, VoiceLease>,
    aliases: HashMap<Entity, AliasVoiceLease>,
    sequence: u64,
}

impl Default for VoiceOccupancy {
    fn default() -> Self {
        Self {
            counts: [0; SND_ENTCHANNEL_MAX],
            live: HashMap::new(),
            aliases: HashMap::new(),
            sequence: 0,
        }
    }
}

impl VoiceOccupancy {
    pub(crate) fn limit_alias(
        &mut self,
        namespace: AssetNamespace,
        alias: &str,
        snd_ent: Option<u32>,
        limit: (u32, u8),
        per_entity: bool,
        priority: f32,
    ) -> Result<Option<Entity>, crate::start::SuppressReason> {
        let (mode, count) = limit;
        if mode == 0 {
            return Ok(None);
        }
        let candidates: Vec<_> = self
            .aliases
            .iter()
            .filter(|(_, lease)| {
                lease.namespace == namespace
                    && lease.alias == alias
                    && (!per_entity || lease.snd_ent == snd_ent)
            })
            .collect();
        if candidates.len() < usize::from(count) {
            return Ok(None);
        }
        let victim = match mode {
            1 => candidates
                .iter()
                .min_by_key(|(_, lease)| lease.sequence)
                .map(|(entity, _)| **entity),
            3 => candidates
                .iter()
                .min_by(|(_, a), (_, b)| a.priority.total_cmp(&b.priority))
                .filter(|(_, lease)| priority > lease.priority + 3.0)
                .map(|(entity, _)| **entity),
            _ => return Err(crate::start::SuppressReason::VoiceLimit),
        };
        if mode == 3 && victim.is_none() {
            return Err(crate::start::SuppressReason::VoiceLimit);
        }
        if let Some(entity) = victim {
            self.aliases.remove(&entity);
        }
        Ok(victim)
    }

    pub(crate) fn update_alias_priority(&mut self, entity: Entity, priority: f32) {
        if let Some(lease) = self.aliases.get_mut(&entity) {
            lease.priority = priority;
        }
    }

    pub(crate) fn track_alias(
        &mut self,
        entity: Entity,
        namespace: AssetNamespace,
        alias: &str,
        snd_ent: Option<u32>,
        priority: f32,
    ) -> AliasVoiceLease {
        self.sequence += 1;
        let lease = AliasVoiceLease {
            namespace,
            alias: alias.to_owned(),
            snd_ent,
            priority,
            sequence: self.sequence,
        };
        self.aliases.insert(entity, lease.clone());
        lease
    }

    pub fn voice_count(&self, channel: u32) -> i32 {
        self.counts.get(channel as usize).copied().unwrap_or(0)
    }

    pub fn track(&mut self, entity: Entity, lease: VoiceLease) {
        if let Some(count) = self.counts.get_mut(lease.channel as usize) {
            *count = count.saturating_add(1);
        }
        self.live.insert(entity, lease);
    }

    pub fn reclaim(&mut self, entity: Entity) {
        let Some(lease) = self.live.remove(&entity) else {
            return;
        };
        if let Some(count) = self.counts.get_mut(lease.channel as usize) {
            *count = (*count - 1).max(0);
        }
    }

    pub fn take_entity_channel(&mut self, snd_ent: u32, channel: u32) -> Vec<Entity> {
        let mut out = Vec::new();
        self.live.retain(|&entity, lease| {
            let Some(occupant_ent) = lease.snd_ent else {
                return true;
            };
            if entity_channel_matches(occupant_ent, lease.channel, snd_ent, channel) {
                if let Some(count) = self.counts.get_mut(lease.channel as usize) {
                    *count = (*count - 1).max(0);
                }
                out.push(entity);
                false
            } else {
                true
            }
        });
        out
    }
}

pub(crate) fn reclaim_finished_voices(
    mut occupancy: ResMut<VoiceOccupancy>,
    mut removed: RemovedComponents<VoiceLease>,
    mut removed_aliases: RemovedComponents<AliasVoiceLease>,
) {
    for entity in removed_aliases.read() {
        occupancy.aliases.remove(&entity);
    }
    for entity in removed.read() {
        occupancy.reclaim(entity);
    }
}
