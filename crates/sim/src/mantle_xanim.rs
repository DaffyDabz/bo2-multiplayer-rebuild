use movement_iw4::{
    CreateAnimsMantleRootDelta, FlatMantleAnimLength, MANTLE_XANIM_NAMES, MANTLE_XANIM_NAMES_FR,
    MANTLE_XANIM_TREE_SIZE, MantleRootDelta, MantleXAnimLength, mantle,
};
use xmodel_runtime::AnimClip;

/// Black Ops II's climb tree, in the order its exe lists it. The climbs up and
/// the high and mid vaults share Modern Warfare 2's names; the low vault is the
/// player's own `player_mantle_over_low` (BO2's `mp_mantle_over_low` carries no
/// root motion). BO2 has no second, fast tree.
pub const MANTLE_XANIM_NAMES_T6: [&str; MANTLE_XANIM_TREE_SIZE] = [
    "mp_mantle_root",
    "mp_mantle_up_57",
    "mp_mantle_up_51",
    "mp_mantle_up_45",
    "mp_mantle_up_39",
    "mp_mantle_up_33",
    "mp_mantle_up_27",
    "mp_mantle_up_21",
    "mp_mantle_over_high",
    "mp_mantle_over_mid",
    "player_mantle_over_low",
];

/// `perk_mantleReduction`: with `specialty_fastmantle` BO2 plays the same climb
/// with its climb-up part scaled by this (the vault over keeps its length).
/// BO2's default, 0.4: the climb up takes 40% of its time.
pub const PERK_MANTLE_REDUCTION_DEFAULT: f32 = 0.4;

#[derive(Clone, Debug)]
struct Leaf {
    length_msec: i32,
    clip: Option<AnimClip>,
}

impl Leaf {
    fn of(clip: Option<AnimClip>) -> Self {
        match clip {
            Some(clip) => Self {
                length_msec: clip.length_msec().max(1),
                clip: Some(clip),
            },
            None => Self {
                length_msec: 0,
                clip: None,
            },
        }
    }
}

fn is_up_anim(index: usize) -> bool {
    (0..7).any(|t| usize::try_from(mantle::trans_up_anim(t)).ok() == Some(index))
}

#[derive(Clone, Debug)]
pub struct MantleXAnimBind {
    slow: [Leaf; MANTLE_XANIM_TREE_SIZE],
    fast: [Leaf; MANTLE_XANIM_TREE_SIZE],
}

impl Default for MantleXAnimBind {
    fn default() -> Self {
        Self {
            slow: core::array::from_fn(|_| Leaf {
                length_msec: 0,
                clip: None,
            }),
            fast: core::array::from_fn(|_| Leaf {
                length_msec: 0,
                clip: None,
            }),
        }
    }
}

impl MantleXAnimBind {
    #[must_use]
    pub fn clip_name(fast: bool, index: usize) -> Option<&'static str> {
        let names = if fast {
            MANTLE_XANIM_NAMES_FR.as_slice()
        } else {
            MANTLE_XANIM_NAMES.as_slice()
        };
        names.get(index).copied()
    }

    #[must_use]
    pub fn up_anim(trans_index: i32) -> i32 {
        mantle::trans_up_anim(trans_index)
    }

    #[must_use]
    pub fn over_anim(trans_index: i32) -> i32 {
        mantle::trans_over_anim(trans_index)
    }

    #[must_use]
    pub fn clip_name_t6(index: usize) -> Option<&'static str> {
        MANTLE_XANIM_NAMES_T6.get(index).copied()
    }

    pub fn from_clips(mut get: impl FnMut(bool, usize) -> Option<AnimClip>) -> Self {
        let fill = |fast: bool, get: &mut dyn FnMut(bool, usize) -> Option<AnimClip>| {
            core::array::from_fn(|i| Leaf::of(get(fast, i)))
        };
        Self {
            slow: fill(false, &mut get),
            fast: fill(true, &mut get),
        }
    }

    /// Black Ops II: one tree for both speeds; the fast climb only shortens
    /// the climb up, by `up_reduction`.
    pub fn from_t6_clips(mut get: impl FnMut(usize) -> Option<AnimClip>, up_reduction: f32) -> Self {
        let slow: [Leaf; MANTLE_XANIM_TREE_SIZE] = core::array::from_fn(|i| Leaf::of(get(i)));
        let fast = core::array::from_fn(|i| {
            let mut leaf = slow[i].clone();
            if is_up_anim(i) && leaf.length_msec > 0 {
                leaf.length_msec = ((leaf.length_msec as f32 * up_reduction) as i32).max(1);
            }
            leaf
        });
        Self { slow, fast }
    }

    fn leaf(&self, fast: bool, anim: i32) -> Option<&Leaf> {
        let i = usize::try_from(anim).ok()?;
        let row = if fast { &self.fast } else { &self.slow };
        row.get(i)
    }
}

impl MantleXAnimLength for MantleXAnimBind {
    fn length_msec(&self, fast_mantle: bool, anim_index: i32) -> i32 {
        if let Some(leaf) = self.leaf(fast_mantle, anim_index) {
            if leaf.length_msec > 0 {
                return leaf.length_msec;
            }
        }
        FlatMantleAnimLength::default().length_msec(fast_mantle, anim_index)
    }
}

impl MantleRootDelta for MantleXAnimBind {
    fn abs_delta(&self, fast_mantle: bool, anim_index: i32, frac: f32) -> [f32; 3] {
        if let Some(leaf) = self.leaf(fast_mantle, anim_index) {
            if let Some(clip) = leaf.clip.as_ref() {
                if clip.has_delta() {
                    return clip.abs_delta_trans(frac);
                }
                return [0.0; 3];
            }
        }
        CreateAnimsMantleRootDelta.abs_delta(fast_mantle, anim_index, frac)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmodel_runtime::Translation;

    fn clip(frames: u16, end: [f32; 3]) -> AnimClip {
        AnimClip {
            name: String::new(),
            framerate: 30.0,
            numframes: frames,
            looping: false,
            tracks: Vec::new(),
            notifies: Vec::new(),
            delta_translation: Translation::Constant(end),
        }
    }

    #[test]
    fn fast_climb_shortens_only_the_climb_up_in_bo2() {
        // BO2's 57 climb up is 26 frames, its high vault 10, at 30 a second.
        let bind = MantleXAnimBind::from_t6_clips(
            |i| match i {
                1 => Some(clip(26, [16.0, 0.0, 57.0])),
                8 => Some(clip(10, [31.2, 0.0, -17.9])),
                _ => None,
            },
            PERK_MANTLE_REDUCTION_DEFAULT,
        );
        assert_eq!(bind.length_msec(false, 1), 866);
        assert_eq!(bind.length_msec(true, 1), 346);
        assert_eq!(bind.length_msec(false, 8), 333);
        assert_eq!(bind.length_msec(true, 8), 333);
        // The quicker climb covers the same ground.
        assert_eq!(bind.abs_delta(true, 1, 1.0), bind.abs_delta(false, 1, 1.0));
    }

    #[test]
    fn bo2_low_vault_is_the_players_own() {
        let low = usize::try_from(MantleXAnimBind::over_anim(6)).unwrap();
        assert_eq!(MantleXAnimBind::clip_name_t6(low), Some("player_mantle_over_low"));
        let top = usize::try_from(MantleXAnimBind::up_anim(0)).unwrap();
        assert_eq!(MantleXAnimBind::clip_name_t6(top), Some("mp_mantle_up_57"));
    }
}
