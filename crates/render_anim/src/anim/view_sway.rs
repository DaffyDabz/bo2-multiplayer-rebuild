use weapon_iw4::{
    SwayContribution, SwaySpringState, WeaponSwayParams, calculate_weapon_movement_sway,
    lerp_sway_params, sway_contribution,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewSwayState {
    springs: SwaySpringState,
    prev_view_angles: Option<[f32; 3]>,
    last: SwayContribution,
}

impl ViewSwayState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn springs(&self) -> SwaySpringState {
        self.springs
    }

    /// bo2zm: Black Ops II keeps following the view while the scope hides
    /// the gun, so coming off the scope does not swing it.
    pub fn follow_view(&mut self, view_angles: [f32; 3]) {
        self.prev_view_angles = Some(view_angles);
    }

    pub fn advance(
        &mut self,
        hip: WeaponSwayParams,
        ads: WeaponSwayParams,
        view_angles: [f32; 3],
        weapon_pos_frac: f32,
        aim_down_sight: bool,
        overlay_active: bool,
        landing_scale: f32,
        dt_secs: f32,
    ) {
        if !dt_secs.is_finite() || dt_secs <= 0.0 {
            return;
        }
        if overlay_active && weapon_pos_frac > 0.0 {
            return;
        }
        let Some(prev) = self.prev_view_angles.replace(view_angles) else {
            self.last = sway_contribution(self.springs);
            return;
        };
        let params = if aim_down_sight {
            lerp_sway_params(hip, ads, weapon_pos_frac)
        } else {
            hip
        };
        calculate_weapon_movement_sway(
            &mut self.springs,
            view_angles,
            prev,
            params,
            landing_scale,
            dt_secs,
        );
        self.last = sway_contribution(self.springs);
    }
}

#[cfg(test)]
mod bo2_tests {
    use super::*;

    const HIP: WeaponSwayParams = WeaponSwayParams {
        max_angle: 4.0,
        lerp_speed: 6.0,
        pitch_scale: 0.1,
        yaw_scale: 0.1,
        horiz_scale: 0.2,
        vert_scale: 0.2,
    };

    /// Turns 90 degrees while scoped, then lets go of the aim.
    fn sway_after_scope(follow: bool) -> f32 {
        let mut sway = ViewSwayState::default();
        sway.advance(HIP, HIP, [0.0; 3], 0.0, true, true, 1.0, 1.0 / 60.0);
        for frame in 1..=30 {
            let view = [0.0, frame as f32 * 3.0, 0.0];
            if follow {
                sway.follow_view(view);
            } else {
                sway.advance(HIP, HIP, view, 1.0, true, true, 1.0, 1.0 / 60.0);
            }
        }
        sway.advance(HIP, HIP, [0.0, 90.0, 0.0], 0.0, true, true, 1.0, 1.0 / 60.0);
        sway.springs().yaw.abs()
    }

    #[test]
    fn coming_off_the_scope_does_not_swing_the_gun_in_bo2() {
        let bo2 = sway_after_scope(true);
        let iw4 = sway_after_scope(false);
        eprintln!("gun turn right after the scope comes off: bo2 {bo2:.3} iw4 {iw4:.3}");
        assert_eq!(bo2, 0.0);
        assert!(iw4 > 0.03);
    }
}
