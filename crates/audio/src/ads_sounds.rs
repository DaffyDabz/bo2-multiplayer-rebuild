//! bo2mp: Black Ops II's aim-down-sight sounds for his own gun: the gear
//! rustle as the sights come up (`adsRaiseSoundPlayer`) and go down
//! (`adsLowerSoundPlayer`), and a scope's zoom click (`adsZoomSound`,
//! `fly_scope_zoom`) the moment its overlay snaps on.

use asset_game::WeaponSoundSlot;
use assets::PreparedWeapons;
use bevy::prelude::*;
use net::{LocalPresentClient, PresentedSnapshot};

use crate::{SND_ENT_LOCAL, SoundBank, WeaponSound};

#[derive(Default)]
pub(crate) struct AdsSoundState {
    weapon: u32,
    frac: f32,
    /// Which way the sights last moved (true: up).
    raising: bool,
}

/// What the sights did this frame: (raise started, lower started, reached
/// full ADS).
fn ads_edges(state: &mut AdsSoundState, weapon: u32, frac: f32) -> (bool, bool, bool) {
    if weapon != state.weapon {
        *state = AdsSoundState {
            weapon,
            frac,
            raising: frac > 0.0,
        };
        return (false, false, false);
    }
    let (mut up, mut down) = (false, false);
    if frac > state.frac + 1e-4 && !state.raising {
        state.raising = true;
        up = true;
    } else if frac + 1e-4 < state.frac && state.raising {
        state.raising = false;
        down = true;
    }
    let full = state.frac < 1.0 && frac >= 1.0;
    state.frac = frac;
    (up, down, full)
}

pub(crate) fn play_ads_sounds(
    mut state: Local<AdsSoundState>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    bank: Option<Res<SoundBank>>,
    mut out: MessageWriter<WeaponSound>,
) {
    let Some(ps) = presented.player(local.0) else {
        *state = AdsSoundState::default();
        return;
    };
    let (up, down, full) = ads_edges(&mut state, ps.weapon, ps.f_weapon_pos_frac);
    let (Some(weapons), Some(bank)) = (weapons.as_deref(), bank.as_deref()) else {
        return;
    };
    // Only Black Ops II's guns name these.
    if weapons.0.namespace_of(ps.weapon) != Some(asset_core::AssetNamespace::T6) {
        return;
    }
    let scoped = weapons.0.overlay_is_hud_iris(ps.weapon);
    for (fire, slot) in [
        (up, WeaponSoundSlot::AdsRaisePlayer),
        (down, WeaponSoundSlot::AdsLowerPlayer),
        (full && scoped, WeaponSoundSlot::AdsZoom),
    ] {
        if !fire {
            continue;
        }
        if let Some((namespace, alias)) = weapons.0.weapon_sound_key(ps.weapon, slot, &bank.0) {
            out.write(WeaponSound {
                namespace,
                alias: alias.to_owned(),
                origin_inches: None,
                snd_ent: Some(SND_ENT_LOCAL),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_lower_and_full_fire_once_each() {
        let mut s = AdsSoundState::default();
        assert_eq!(ads_edges(&mut s, 5, 0.0), (false, false, false));
        assert_eq!(ads_edges(&mut s, 5, 0.2), (true, false, false));
        assert_eq!(ads_edges(&mut s, 5, 0.6), (false, false, false));
        assert_eq!(ads_edges(&mut s, 5, 1.0), (false, false, true));
        assert_eq!(ads_edges(&mut s, 5, 1.0), (false, false, false));
        assert_eq!(ads_edges(&mut s, 5, 0.7), (false, true, false));
        assert_eq!(ads_edges(&mut s, 5, 0.0), (false, false, false));
        // A new gun is never a raise.
        assert_eq!(ads_edges(&mut s, 6, 0.5), (false, false, false));
    }
}
