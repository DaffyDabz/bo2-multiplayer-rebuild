//! bo2zm: Black Ops II weapons into the weapon catalog.
//!
//! `asset_t6::WeaponRef` carries a weapon's variant and shared definition as
//! bytes plus every name they point at; this reads the fields the engine's
//! weapon rules, viewmodel and sounds use, by their T6 offsets. Field
//! meanings follow the IW4 and Black Ops ones of the same name; where T6
//! numbers an enum differently (class, offhand class, type), it is mapped
//! to the IW4 value the engine expects.

use fastfile_t6::layout::{WeaponDef as d, WeaponVariantDef as v};

use super::*;

/// T6 `weapAnimFiles_t` slot -> the catalog's slot (IW4 numbering plus the
/// quick-reload extras). Slots IW4 has no use for are left out.
const T6_ANIM_SLOTS: [(usize, usize); 33] = [
    (0x01, weap_anim::IDLE),
    (0x02, weap_anim::EMPTY_IDLE),
    (0x04, weap_anim::FIRE),
    (0x05, weap_anim::HOLD_FIRE),
    (0x06, weap_anim::LASTSHOT),
    (0x08, weap_anim::RECHAMBER),
    (0x09, weap_anim::MELEE),
    (0x0E, weap_anim::MELEE_CHARGE),
    (0x10, weap_anim::RELOAD),
    (0x12, weap_anim::RELOAD_EMPTY),
    (0x13, weap_anim::RELOAD_START),
    (0x14, weap_anim::RELOAD_END),
    (0x15, weap_anim_extra::RELOAD_QUICK),
    (0x16, weap_anim_extra::RELOAD_QUICK_EMPTY),
    (0x17, weap_anim::RAISE),
    (0x18, weap_anim::FIRST_RAISE),
    (0x19, weap_anim::DROP),
    (0x1A, weap_anim::ALT_RAISE),
    (0x1B, weap_anim::ALT_DROP),
    (0x1C, weap_anim::QUICK_RAISE),
    (0x1D, weap_anim::QUICK_DROP),
    (0x1E, weap_anim::EMPTY_RAISE),
    (0x1F, weap_anim::EMPTY_DROP),
    (0x20, weap_anim::SPRINT_IN),
    (0x21, weap_anim::SPRINT_LOOP),
    (0x22, weap_anim::SPRINT_OUT),
    (0x3A, weap_anim::DETONATE),
    (0x3B, weap_anim::NIGHTVISION_WEAR),
    (0x3C, weap_anim::NIGHTVISION_REMOVE),
    (0x3D, weap_anim::ADS_FIRE),
    (0x3E, weap_anim::ADS_LASTSHOT),
    (0x40, weap_anim::ADS_RECHAMBER),
    (0x55, weap_anim::ADS_UP),
];

/// T6 asset names are case-insensitive (the knife names
/// `viewmodel_M4m203_knife_melee_1`, the zone holds `viewmodel_m4m203_...`);
/// animations are keyed lower case.
fn remap_t6_sz_xanims(t6: &[String]) -> [Option<String>; WEAPON_ANIM_SLOTS] {
    let mut out = [const { None }; WEAPON_ANIM_SLOTS];
    for (src, dst) in T6_ANIM_SLOTS {
        if let Some(name) = t6.get(src).filter(|n| !n.is_empty()) {
            out[dst] = Some(name.to_ascii_lowercase());
        }
    }
    // ADS down (0x56) has no IW4 slot of its own beside ADS up's pair.
    if let Some(name) = t6.get(0x56).filter(|n| !n.is_empty()) {
        out[weap_anim::ADS_DOWN] = Some(name.to_ascii_lowercase());
    }
    out
}

/// bo2zm: a dual-wield pair's left-hand animations (the left weapon's
/// `dw_left_*` slots) on the catalog's slots: its other slots (raise, drop,
/// sprint) as they are, then fire 0x4E, last shot 0x4F, idle 0x51, empty
/// idle 0x52, empty reload 0x53, reload 0x54 (measured on fivesevenlh_zm and
/// m1911lh_upgraded_zm).
fn remap_t6_left_hand_xanims(t6: &[String]) -> [Option<String>; WEAPON_ANIM_SLOTS] {
    let mut out = remap_t6_sz_xanims(t6);
    for (src, dst) in [
        (0x4E, weap_anim::FIRE),
        (0x4F, weap_anim::LASTSHOT),
        (0x51, weap_anim::IDLE),
        (0x52, weap_anim::EMPTY_IDLE),
        (0x53, weap_anim::RELOAD_EMPTY),
        (0x54, weap_anim::RELOAD),
    ] {
        if let Some(name) = t6.get(src).filter(|n| !n.is_empty()) {
            out[dst] = Some(name.to_ascii_lowercase());
        }
    }
    out
}

/// T6 `weapClass_t` -> IW4. T6: rifle, mg, smg, spread, pistol, grenade,
/// rocket launcher, turret, non-player, gas, item, melee, killstreak alt
/// stored weapon, pistol spread. IW4: rifle, sniper, mg, smg, spread,
/// pistol, grenade, rocket launcher, turret, throwing knife, non-player,
/// item.
fn remap_t6_weap_class(raw: i32) -> i32 {
    match raw {
        0 => 0,
        1 => 2,
        2 => 3,
        3 => 4,
        4 => 5,
        5 => 6,
        6 => 7,
        7 => 8,
        8 => 10,
        9 => 10,
        10 => 11,
        11 => 11,
        13 => 4,
        other => other,
    }
}

/// T6 `weapType_t` -> IW4 (bullet, grenade, projectile, riot shield).
/// Melee, gas, bomb and mine weapons are not fired like guns.
fn remap_t6_weap_type(raw: i32) -> i32 {
    match raw {
        0 => 0,
        1 => 1,
        2 => 2,
        8 => 3,
        4 | 5 | 6 => 1,
        _ => 0,
    }
}

/// T6 `OffhandClass`: none, frag, smoke, flash, gear, supply drop marker.
fn remap_t6_offhand_class(raw: i32) -> i32 {
    match raw {
        4 | 5 => 5,
        other => other,
    }
}

/// T6 `weapFireType_t`: full auto, single, burst 2..5, stacked, minigun,
/// charge shot, jet gun. IW4 knows full auto, single and bursts 2..4.
fn remap_t6_fire_type(raw: i32) -> i32 {
    match raw {
        0..=4 => raw,
        5 => 4,
        7 | 9 => 0,
        _ => 1,
    }
}

fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

fn t6_body_facts(w: &asset_t6::WeaponRef) -> WeaponBodyFacts {
    let mut f = WeaponBodyFacts {
        body_resolved: !w.def.is_empty(),
        clip_size: w.var_i32(v::iClipSize),
        reload_time_ms: w.var_i32(v::iReloadTime),
        reload_empty_time_ms: w.var_i32(v::iReloadEmptyTime),
        alternate_raise_time_ms: w.var_i32(v::iAltRaiseTime),
        aim_assist_range_ads: w.var_f32(v::fAimAssistRangeAds),
        ads_zoom_in_frac: w.var_f32(v::fAdsZoomInFrac),
        ads_zoom_out_frac: w.var_f32(v::fAdsZoomOutFrac),
        select_requires_ammo: Some(false),
        inherits_perks: true,
        ..WeaponBodyFacts::default()
    };
    let ads_in = w.var_i32(v::iAdsTransInTime);
    let ads_out = w.var_i32(v::iAdsTransOutTime);
    f.ads_in_rate = if ads_in > 0 { 1.0 / ads_in as f32 } else { 0.0 };
    f.ads_out_rate = if ads_out > 0 { 1.0 / ads_out as f32 } else { 0.0 };
    f.ads_zoom_fov = [v::fAdsZoomFov1, v::fAdsZoomFov2, v::fAdsZoomFov3]
        .into_iter()
        .map(|o| w.var_f32(o))
        .find(|&fov| fov > 0.0)
        .unwrap_or(0.0);
    if w.var_u8(v::bDualMag) != 0 {
        f.dual_mag = Some(weapon_iw4::DualMagTimes {
            reload_ms: w.var_i32(v::iReloadQuickTime),
            reload_empty_ms: w.var_i32(v::iReloadQuickEmptyTime),
            add_ms: w.def_i32(d::iReloadQuickAddTime),
            empty_add_ms: w.def_i32(d::iReloadQuickEmptyAddTime),
        });
    }
    if w.def.is_empty() {
        return f;
    }
    f.fire_time_ms = w.def_i32(d::iFireTime);
    f.impact_type = w.def_i32(d::impactType);
    f.raise_time_ms = w.def_i32(d::iRaiseTime);
    f.drop_time_ms = w.def_i32(d::iDropTime);
    f.alternate_drop_time_ms = w.def_i32(d::iAltDropTime);
    f.fire_delay_ms = w.def_i32(d::iFireDelay);
    f.hold_fire_time_ms = w.def_i32(d::iHoldFireTime);
    f.weap_type = remap_t6_weap_type(w.def_i32(d::weapType));
    f.player_anim_type = w.def_i32(d::playerAnimType);
    f.weap_class = remap_t6_weap_class(w.def_i32(d::weapClass));
    f.offhand_class = remap_t6_offhand_class(w.def_i32(d::offhandClass));
    f.shots_per_fire = w.def_i32(d::shotCount);
    f.ammo_counter_clip = w.def_i32(d::ammoCounterClip);
    f.low_ammo_warning_threshold = w.def_f32(d::lowAmmoWarningThreshold);
    apply_leftover_hip_spread(
        &mut f,
        leftover_hip_spread_block(|off| w.def_f32(off), d::fHipSpreadStandMin),
    );
    f.i_reticle_side_size = w.def_i32(d::iReticleSideSize);
    f.i_reticle_min_ofs = w.def_i32(d::iReticleMinOfs);
    f.hip_reticle_side_pos = w.def_f32(d::fHipReticleSidePos);
    f.ads_aim_pitch = w.def_f32(d::fAdsAimPitch);
    f.ads_crosshair_in_frac = w.def_f32(d::fAdsCrosshairInFrac);
    f.ads_crosshair_out_frac = w.def_f32(d::fAdsCrosshairOutFrac);
    f.ads_spread = w.def_f32(d::fAdsSpread);
    f.aim_down_sight = w.def_u8(d::aimDownSight) != 0;
    f.hold_breath_to_steady = w.def_u8(d::bHoldBreathToSteady) != 0;
    let dof = [w.def_f32(d::adsDofStart), w.def_f32(d::adsDofEnd)];
    f.ads_dof = (dof[1] > 0.0).then_some(dof);
    f.no_ads_when_mag_empty = w.def_u8(d::noAdsWhenMagEmpty) != 0;
    f.rechamber_while_ads = w.def_u8(d::bRechamberWhileAds) != 0;
    f.ads_fire_only = w.def_u8(d::adsFireOnly) != 0;
    f.melee_damage = w.def_i32(d::iMeleeDamage);
    f.overlay_reticle = w.def_i32(d::overlayReticle);
    f.overlay_interface = w.def_i32(d::overlayInterface);
    f.ads_overlay_width = w.def_f32(d::overlayWidth);
    f.ads_overlay_height = w.def_f32(d::overlayHeight);
    f.melee_time_ms = w.def_i32(d::iMeleeTime);
    f.melee_delay_ms = w.def_i32(d::iMeleeDelay);
    f.melee_charge_time_ms = w.def_i32(d::meleeChargeTime);
    f.melee_charge_delay_ms = w.def_i32(d::meleeChargeDelay);
    f.use_as_melee = w.def_u8(d::bUseAsMelee) != 0;
    f.quick_raise_time_ms = w.def_i32(d::quickRaiseTime);
    f.quick_drop_time_ms = w.def_i32(d::quickDropTime);
    f.offhand_hold_is_cancelable = Some(w.def_u8(d::offhandHoldIsCancelable) != 0);
    f.move_speed_scale = w.def_f32(d::moveSpeedScale);
    f.ads_move_speed_scale = w.def_f32(d::adsMoveSpeedScale);
    f.sprint_duration_scale = w.def_f32(d::sprintDurationScale);
    f.ducked_ofs = w.def_vec3(d::vDuckedOfs);
    f.prone_ofs = w.def_vec3(d::vProneOfs);
    f.night_vision_wear_time = w.def_i32(d::nightVisionWearTime);
    f.ads_bob_factor = w.def_f32(d::fAdsBobFactor);
    f.ads_view_bob_mult = w.def_f32(d::fAdsViewBobMult);
    f.movement = WeaponMovementOfsInputs {
        stand_move: w.def_vec3(d::vStandMove),
        stand_rot: w.def_vec3(d::vStandRot),
        strafe_move: w.def_vec3(d::vStrafeMove),
        strafe_rot: w.def_vec3(d::vStrafeRot),
        ducked_move: w.def_vec3(d::vDuckedMove),
        ducked_rot: w.def_vec3(d::vDuckedRot),
        prone_move: w.def_vec3(d::vProneMove),
        prone_rot: w.def_vec3(d::vProneRot),
        pos_move_rate: w.def_f32(d::fPosMoveRate),
        pos_prone_move_rate: w.def_f32(d::fPosProneMoveRate),
        stand_move_min_speed: w.def_f32(d::fStandMoveMinSpeed),
        ducked_move_min_speed: w.def_f32(d::fDuckedMoveMinSpeed),
        prone_move_min_speed: w.def_f32(d::fProneMoveMinSpeed),
        pos_rot_rate: w.def_f32(d::fPosRotRate),
        pos_prone_rot_rate: w.def_f32(d::fPosProneRotRate),
        ..WeaponMovementOfsInputs::default()
    };
    f.idle = WeaponIdleInputs {
        ads_idle_amount: w.def_f32(d::fAdsIdleAmount),
        hip_idle_amount: w.def_f32(d::fHipIdleAmount),
        ads_idle_speed: w.def_f32(d::adsIdleSpeed),
        hip_idle_speed: w.def_f32(d::hipIdleSpeed),
        idle_crouch_factor: w.def_f32(d::fIdleCrouchFactor),
        idle_prone_factor: w.def_f32(d::fIdleProneFactor),
    };
    f.penetrate_type = w.def_i32(d::penetrateTWeaponAttachmentype);
    f.rifle_bullet = w.def_u8(d::bRifleBullet) != 0;
    f.inventory_type = match w.def_i32(d::inventoryType) {
        t @ 0..=3 => t,
        _ => 2,
    };
    f.fire_type = remap_t6_fire_type(w.def_i32(d::fireType));
    f.max_ammo = w.def_i32(d::iMaxAmmo);
    f.start_ammo = w.def_i32(d::iStartAmmo);
    f.ammo_count_clip_relative = w.def_u8(d::ammoCountClipRelative) != 0;
    // T6 damage falls off over up to six ranges; the engine knows a near
    // and a far value.
    let damage: Vec<i32> = (0..6).map(|i| w.def_i32(d::damage + i * 4)).collect();
    let ranges: Vec<f32> = (0..6).map(|i| w.def_f32(d::damageRange + i * 4)).collect();
    f.damage = damage[0];
    f.max_damage_range = ranges[0];
    let last = (1..6).rev().find(|&i| ranges[i] > 0.0 && damage[i] > 0);
    match last {
        Some(i) => {
            f.min_damage = damage[i];
            f.min_damage_range = ranges[i];
        }
        None => {
            f.min_damage = damage[0];
            f.min_damage_range = ranges[0];
        }
    }
    f.min_player_damage = w.def_i32(d::minPlayerDamage);
    f.rechamber_time_ms = w.def_i32(d::iRechamberTime);
    f.rechamber_bolt_time_ms = w.def_i32(d::iRechamberBoltTime);
    f.reload_show_rocket_time_ms = w.def_i32(d::reloadShowRocketTime);
    f.reload_add_time_ms = w.def_i32(d::iReloadAddTime);
    f.reload_empty_add_time_ms = w.def_i32(d::iReloadEmptyAddTime);
    f.reload_start_time_ms = w.def_i32(d::iReloadStartTime);
    f.reload_start_add_time_ms = w.def_i32(d::iReloadStartAddTime);
    f.reload_end_time_ms = w.def_i32(d::iReloadEndTime);
    f.kill_icon_ratio = w.def_i32(d::killIconRatio);
    f.flip_kill_icon = w.def_u8(d::flipKillIcon) != 0;
    f.reload_ammo_add = w.def_i32(d::iReloadAmmoAdd);
    f.reload_start_add = w.def_i32(d::iReloadStartAdd);
    f.no_partial_reload = w.def_u8(d::bNoPartialReload) != 0;
    f.bolt_action = w.def_u8(d::bBoltAction) != 0;
    f.segmented_reload = w.def_u8(d::bSegmentedReload) != 0;
    f.sprint_raise_time_ms = w.def_i32(d::sprintInTime);
    f.sprint_loop_time_ms = w.def_i32(d::sprintLoopTime);
    f.sprint_drop_time_ms = w.def_i32(d::sprintOutTime);
    f.fuse_time_ms = w.def_i32(d::fuseTime);
    f.auto_aim_range = w.def_f32(d::autoAimRange);
    f.aim_assist_range = w.def_f32(d::aimAssistRange);
    f.cook_off_hold = w.def_u8(d::bCookOffHold) != 0;
    f.clip_only = w.def_u8(d::bClipOnly) != 0;
    f.has_detonator = w.def_u8(d::hasDetonator) != 0;
    f.detonate_delay_ms = w.def_i32(d::iDetonateDelay);
    f.detonate_time_ms = w.def_i32(d::iDetonateTime);
    f.projectile_rotates = w.def_u8(d::rotate) != 0;
    // bo2zm fix list 2: BO2's own minimums only matter this way (see
    // `WeaponBodyFacts::spread_before_fire_add`).
    f.spread_before_fire_add = true;
    f.timed_detonation = w.def_u8(d::timedDetonation) != 0;
    f.proj_impact_explode = w.def_u8(d::bProjImpactExplode) != 0;
    f.explosion_radius = w.def_i32(d::iExplosionRadius);
    f.explosion_radius_min = w.def_i32(d::iExplosionRadiusMin);
    f.explosion_inner_damage = w.def_i32(d::iExplosionInnerDamage);
    f.explosion_outer_damage = w.def_i32(d::iExplosionOuterDamage);
    f.damage_cone_angle = w.def_f32(d::damageConeAngle);
    f.stickiness = w.def_i32(d::stickiness);
    f.projectile_speed = w.def_i32(d::iProjectileSpeed);
    f.projectile_speed_up = w.def_i32(d::iProjectileSpeedUp);
    f.projectile_speed_forward = w.def_i32(d::iProjectileSpeedForward);
    f.projectile_speed_relative_up = w.def_i32(d::iProjectileSpeedRelativeUp);
    f.projectile_parent_velocity = Some(w.def_f32(d::fProjectileTakeParentVelocity));
    f.projectile_activate_dist = w.def_i32(d::iProjectileActivateDist);
    let array31 = |v: &Option<Vec<f32>>| -> Option<[f32; 31]> {
        let v = v.as_ref()?;
        let mut out = [0.0; 31];
        for (o, x) in out.iter_mut().zip(v.iter()) {
            *o = *x;
        }
        Some(out)
    };
    f.parallel_bounce = array31(&w.parallel_bounce);
    f.perpendicular_bounce = array31(&w.perpendicular_bounce);
    f.location_damage_mult = w.location_damage.as_ref().map(|v| {
        let mut out = [1.0f32; 20];
        for (o, x) in out.iter_mut().zip(v.iter()) {
            *o = *x;
        }
        out
    });
    f.no_dual_wield = w.def_u8(d::bDualWield) == 0;
    f.kick = WeaponKickFacts {
        f_ads_view_kick_center_speed: w.var_f32(v::fAdsViewKickCenterSpeed),
        f_hip_view_kick_center_speed: w.var_f32(v::fHipViewKickCenterSpeed),
        gun_max_pitch: w.def_f32(d::fGunMaxPitch),
        gun_max_yaw: w.def_f32(d::fGunMaxYaw),
        ads_gun_kick_reduced_kick_bullets: w.def_i32(d::adsGunKickReducedKickBullets),
        ads_gun_kick_reduced_kick_percent: w.def_f32(d::adsGunKickReducedKickPercent),
        ads_gun_kick_pitch_min: w.def_f32(d::fAdsGunKickPitchMin),
        ads_gun_kick_pitch_max: w.def_f32(d::fAdsGunKickPitchMax),
        ads_gun_kick_yaw_min: w.def_f32(d::fAdsGunKickYawMin),
        ads_gun_kick_yaw_max: w.def_f32(d::fAdsGunKickYawMax),
        ads_gun_kick_accel: w.def_f32(d::fAdsGunKickAccel),
        ads_gun_kick_speed_max: w.def_f32(d::fAdsGunKickSpeedMax),
        ads_gun_kick_speed_decay: w.def_f32(d::fAdsGunKickSpeedDecay),
        ads_gun_kick_static_decay: w.def_f32(d::fAdsGunKickStaticDecay),
        ads_view_kick_pitch_min: w.def_f32(d::fAdsViewKickPitchMin),
        ads_view_kick_pitch_max: w.def_f32(d::fAdsViewKickPitchMax),
        ads_view_kick_yaw_min: w.def_f32(d::fAdsViewKickYawMin),
        ads_view_kick_yaw_max: w.def_f32(d::fAdsViewKickYawMax),
        hip_gun_kick_reduced_kick_bullets: w.def_i32(d::hipGunKickReducedKickBullets),
        hip_gun_kick_reduced_kick_percent: w.def_f32(d::hipGunKickReducedKickPercent),
        hip_gun_kick_pitch_min: w.def_f32(d::fHipGunKickPitchMin),
        hip_gun_kick_pitch_max: w.def_f32(d::fHipGunKickPitchMax),
        hip_gun_kick_yaw_min: w.def_f32(d::fHipGunKickYawMin),
        hip_gun_kick_yaw_max: w.def_f32(d::fHipGunKickYawMax),
        hip_gun_kick_accel: w.def_f32(d::fHipGunKickAccel),
        hip_gun_kick_speed_max: w.def_f32(d::fHipGunKickSpeedMax),
        hip_gun_kick_speed_decay: w.def_f32(d::fHipGunKickSpeedDecay),
        hip_gun_kick_static_decay: w.def_f32(d::fHipGunKickStaticDecay),
        hip_view_kick_pitch_min: w.def_f32(d::fHipViewKickPitchMin),
        hip_view_kick_pitch_max: w.def_f32(d::fHipViewKickPitchMax),
        hip_view_kick_yaw_min: w.def_f32(d::fHipViewKickYawMin),
        hip_view_kick_yaw_max: w.def_f32(d::fHipViewKickYawMax),
        hip_view_kick_min_magnitude: w.def_f32(d::fHipViewKickMinMagnitude),
        ads_view_kick_min_magnitude: w.def_f32(d::fAdsViewKickMinMagnitude),
    };
    f.sway = WeaponSwayFacts {
        sway_max_angle: w.def_f32(d::swayMaxAngle),
        sway_lerp_speed: w.def_f32(d::swayLerpSpeed),
        sway_pitch_scale: w.def_f32(d::swayPitchScale),
        sway_yaw_scale: w.def_f32(d::swayYawScale),
        sway_horiz_scale: w.def_f32(d::swayHorizScale),
        sway_vert_scale: w.def_f32(d::swayVertScale),
        sway_shell_shock_scale: w.def_f32(d::swayShellShockScale),
        ads_sway_max_angle: w.def_f32(d::adsSwayMaxAngle),
        ads_sway_lerp_speed: w.def_f32(d::adsSwayLerpSpeed),
        ads_sway_pitch_scale: w.def_f32(d::adsSwayPitchScale),
        ads_sway_yaw_scale: w.def_f32(d::adsSwayYawScale),
        ads_sway_horiz_scale: w.var_f32(v::fAdsSwayHorizScale),
        ads_sway_vert_scale: w.var_f32(v::fAdsSwayVertScale),
    };
    if f.body_resolved {
        let pair = |off: usize| [w.def_f32(off), w.def_f32(off + 4)];
        f.bob = WeaponBobFacts {
            sprint_cycle_scale: w.def_f32(d::fSprintCycleScale),
            ducked_sprint_cycle_scale: w.def_f32(d::fDuckedSprintCycleScale),
            dtp_cycle_scale: w.def_f32(d::fDtpCycleScale),
            sprint_bob: pair(d::vSprintBob),
            ducked_sprint_bob: pair(d::vDuckedSprintBob),
            dtp_bob: pair(d::vDtpBob),
        };
    }
    f
}

fn t6_sounds(w: &asset_t6::WeaponRef) -> WeaponSoundAliases {
    let s = |off: usize| opt(w.def_string(off));
    WeaponSoundAliases {
        notetrack_convention: NotetrackConvention::InlinePrefix,
        fire: s(d::fireSound),
        fire_player: s(d::fireSoundPlayer),
        empty_fire: s(d::emptyFireSound),
        empty_fire_player: s(d::emptyFireSoundPlayer),
        melee_swipe: s(d::meleeSwipeSound),
        melee_swipe_player: s(d::meleeSwipeSoundPlayer),
        melee_hit: s(d::meleeHitSound),
        melee_miss: s(d::meleeMissSound),
        pickup: s(d::pickupSound),
        pickup_player: s(d::pickupSoundPlayer),
        ammo_pickup: s(d::ammoPickupSound),
        ammo_pickup_player: s(d::ammoPickupSoundPlayer),
        pullback: s(d::pullbackSound),
        pullback_player: s(d::pullbackSoundPlayer),
        reload: s(d::reloadSound),
        reload_player: s(d::reloadSoundPlayer),
        reload_empty: s(d::reloadEmptySound),
        reload_empty_player: s(d::reloadEmptySoundPlayer),
        reload_start: s(d::reloadStartSound),
        reload_start_player: s(d::reloadStartSoundPlayer),
        reload_end: s(d::reloadEndSound),
        reload_end_player: s(d::reloadEndSoundPlayer),
        rechamber: s(d::rechamberSound),
        rechamber_player: s(d::rechamberSoundPlayer),
        alt_switch: s(d::altSwitchSound),
        alt_switch_player: s(d::altSwitchSoundPlayer),
        raise: s(d::raiseSound),
        raise_player: s(d::raiseSoundPlayer),
        first_raise: s(d::firstRaiseSound),
        first_raise_player: s(d::firstRaiseSoundPlayer),
        putaway: s(d::putawaySound),
        putaway_player: s(d::putawaySoundPlayer),
        proj_explosion: s(d::projExplosionSound),
        projectile: s(d::projectileSound),
        proj_ignition_sound: s(d::projIgnitionSound),
        fire_loop: s(d::fireLoopSound),
        fire_loop_player: s(d::fireLoopSoundPlayer),
        fire_stop: s(d::fireStopSound),
        fire_stop_player: s(d::fireStopSoundPlayer),
        fire_last: s(d::fireLastSound),
        fire_last_player: s(d::fireLastSoundPlayer),
        ads_raise_player: s(d::adsRaiseSoundPlayer),
        ads_lower_player: s(d::adsLowerSoundPlayer),
        ads_zoom: s(d::adsZoomSound),
        notetrack_sound_map: w.notetrack_sounds.clone(),
        bounce: std::array::from_fn(|surf| w.bounce_sounds.get(surf).and_then(|n| opt(n))),
        ..WeaponSoundAliases::default()
    }
}

impl WeaponCatalog {
    /// bo2zm: one Black Ops II weapon (a variant with its definition).
    /// Returns false for a reference stub (a comma-named asset another zone
    /// defines) or a weapon without a name.
    /// bo2zm fix list 2: a rolling grenade's rest radius (measured on its
    /// projectile model by the lane, which holds the models).
    pub fn set_t6_rolling_radius(&mut self, name: &str, radius: f32) {
        for e in self.entries.iter_mut().filter(|e| e.name == name) {
            e.facts.rolling_radius = Some(radius);
        }
    }

    /// `left`: a dual-wield weapon's left-hand weapon (its
    /// `szDualWieldWeaponName`), whose gun hangs on the arms' `tag_weapon1`
    /// and whose `dw_left_*` animations drive the left arm.
    ///
    /// `camo_models`: an upgraded weapon's gun model with Pack-a-Punch's
    /// camo swapped in (by weapon name), for it and its left-hand gun.
    pub fn capture_t6(
        &mut self,
        w: &asset_t6::WeaponRef,
        left: Option<&asset_t6::WeaponRef>,
        camo_models: &std::collections::HashMap<String, String>,
    ) -> bool {
        if w.name.is_empty() || w.name.starts_with(',') {
            return false;
        }
        let left = left.filter(|_| w.def_u8(d::bDualWield) != 0);
        let left_gun = left.and_then(|l| {
            camo_models
                .get(&l.name)
                .cloned()
                .or_else(|| l.gun_models.first().and_then(|n| opt(n)))
        });
        let gun_xmodel = camo_models
            .get(&w.name)
            .cloned()
            .or_else(|| w.gun_models.first().and_then(|n| opt(n)));
        let combat_fx = WeaponCombatFx {
            view_flash_hint: opt(w.def_fx(d::viewFlashEffect)),
            world_flash_hint: opt(w.def_fx(d::worldFlashEffect)),
            view_shell_eject_hint: opt(w.def_fx(d::viewShellEjectEffect)),
            world_shell_eject_hint: opt(w.def_fx(d::worldShellEjectEffect)),
            view_last_shot_eject_hint: opt(w.def_fx(d::viewLastShotEjectEffect)),
            world_last_shot_eject_hint: opt(w.def_fx(d::worldLastShotEjectEffect)),
            explosion_hint: opt(w.def_fx(d::projExplosionEffect)),
            tracer_hint: opt(w.def_tracer(d::tracerType)),
            ..WeaponCombatFx::default()
        };
        let reticle = WeaponReticleAssets {
            center_material: opt(w.def_material(d::reticleCenter)),
            side_material: opt(w.def_material(d::reticleSide)),
            center_authored: !w.def_material(d::reticleCenter).is_empty(),
            side_authored: !w.def_material(d::reticleSide).is_empty(),
            center_size: w.def_i32(d::iReticleCenterSize),
            side_size: w.def_i32(d::iReticleSideSize),
            ..WeaponReticleAssets::default()
        };
        self.entries.push(CatalogWeapon {
            namespace: self.capture_ns,
            name: w.name.clone(),
            alternate_weapon: opt(&w.alt_weapon_name),
            weap_def: None,
            display_name_key: opt(&w.display_name),
            reticle,
            hud_material_edges: WeaponHudMaterialEdges::default(),
            overlay_material: opt(&w.overlay_material).or_else(|| opt(&w.overlay_material_low)),
            overlay_image: None,
            reticle_center_slot: None,
            reticle_side_slot: None,
            overlay_material_slot: None,
            scope_name: None,
            scope_rows: Default::default(),
            iw5_attachment_slots: std::array::from_fn(|_| None),
            iw5_reload_overrides: Vec::new(),
            iw5_anim_overrides: Vec::new(),
            iw5_fx_overrides: Vec::new(),
            iw5_notetrack_overrides: Vec::new(),
            hud_icon: opt(w.def_material(d::hudIcon)),
            hud_icon_slot: None,
            pickup_icon: None,
            pickup_icon_slot: None,
            pickup_icon_image: None,
            pickup_icon_ratio: 0,
            hud_icon_ratio: w.def_i32(d::hudIconRatio),
            hud_icon_image: None,
            dpad_icon: opt(&w.dpad_icon),
            dpad_icon_image: None,
            dpad_icon_atlas: None,
            dpad_icon_ratio: w.var_i32(v::dpadIconRatio),
            kill_icon: opt(w.def_material(d::killIcon)),
            kill_icon_slot: None,
            kill_icon_image: None,
            proj_trail: opt(w.def_fx(d::projTrailEffect)),
            proj_trail_slot: None,
            proj_beacon: None,
            proj_beacon_slot: None,
            proj_ignition: opt(w.def_fx(d::projIgnitionEffect)),
            proj_ignition_slot: None,
            projectile_fx: WeaponProjectileFx::default(),
            gun_xmodel,
            // BO2 draws the player character's arms with every weapon;
            // `handXModel` (`viewmodel_hands_no_model`, ...) is a rig, not
            // the arms the player sees.
            hand_xmodel: None,
            world_model: w.world_models.first().and_then(|n| opt(n)),
            projectile_model: opt(w.def_model(d::projectileModel)),
            rocket_model: opt(w.def_model(d::rocketModel)),
            knife_xmodel: opt(w.def_model(d::additionalMeleeModel)),
            sz_xanims: remap_t6_sz_xanims(&w.xanims),
            sz_xanims_right: match left {
                Some(_) => remap_t6_sz_xanims(&w.xanims),
                None => [const { None }; WEAPON_ANIM_SLOTS],
            },
            sz_xanims_left: match left {
                Some(l) => remap_t6_left_hand_xanims(&l.xanims),
                None => [const { None }; WEAPON_ANIM_SLOTS],
            },
            dual_wield_weapon: left.map(|l| l.name.clone()),
            hide_tags: w.hide_tags.clone(),
            ads_view_attachments: Vec::new(),
            view_attachments: w
                .attach_view_models
                .iter()
                .map(|(_, model, tag, offset, rotation)| {
                    (
                        model.clone(),
                        asset_model::FpvMountOffset {
                            tag: tag.clone(),
                            offset: *offset,
                            rotation: *rotation,
                            on_arms: false,
                        },
                    )
                })
                .chain(left_gun.map(|model| {
                    (
                        model,
                        asset_model::FpvMountOffset {
                            tag: "tag_weapon1".to_owned(),
                            offset: [0.0; 3],
                            rotation: [0.0; 3],
                            on_arms: true,
                        },
                    )
                }))
                .collect(),
            sounds: t6_sounds(w),
            combat_fx,
            combat_slots: CombatFxSlots::default(),
            facts: t6_body_facts(w),
        });
        true
    }
}

/// bo2mp: Black Ops II's sight attachments, by attachment name: the ones a
/// gun is given as its own catalog weapon (`mp7_mp+reflex`), so its sight
/// model, hidden iron sights, ADS animations, zoom and scope overlay are the
/// attachment's. Reflex, EOTech (`holo`), ACOG, Hybrid (`dualoptic`), Dual
/// Band (`ir`), Target Finder (`rangefinder`), Millimeter Scanner (`mms`),
/// Variable Zoom (`vzoom`), Ballistics CPU (`swayreduc`) and the Ballista's
/// iron sights (`is`).
pub const T6_SIGHT_ATTACHMENTS: [&str; 10] = [
    "reflex",
    "holo",
    "acog",
    "dualoptic",
    "ir",
    "rangefinder",
    "mms",
    "vzoom",
    "swayreduc",
    "is",
];

impl WeaponCatalog {
    /// bo2mp: `<gun>+<attachment>` for one sight attachment: the gun's own
    /// entry (already captured) with the attachment unique's models, hidden
    /// tags, animations and overlay, and the attachment's zoom.
    pub fn capture_t6_attachment(
        &mut self,
        w: &asset_t6::WeaponRef,
        attachment: &asset_t6::AttachmentRef,
        unique: &asset_t6::AttachmentUniqueRef,
        reticle_scale: f32,
        alt_of: Option<&str>,
        anim_ms: &dyn Fn(&str) -> Option<i32>,
    ) -> bool {
        use fastfile_t6::layout::WeaponAttachment as a;
        use fastfile_t6::layout::WeaponAttachmentUnique as u;
        let ns = self.capture_ns;
        let Some(mut e) = self
            .entries
            .iter()
            .rev()
            .find(|e| e.name == w.name && e.namespace == ns)
            .cloned()
        else {
            return false;
        };
        e.name = format!("{}+{}", w.name, attachment.name);
        // Hybrid Optic: the unique names the gun's other mode (an alternate
        // weapon with its own ADS animations); each is the other's alternate.
        let hybrid = attachment.name == "dualoptic" && !unique.alt_weapon_name.is_empty();
        if let Some(primary) = alt_of {
            e.alternate_weapon = Some(primary.to_owned());
            e.facts.optic_switch = true;
        } else if hybrid {
            e.alternate_weapon = Some(format!("{}+{}", unique.alt_weapon_name, attachment.name));
            e.facts.optic_switch = true;
        }
        // The scope overlay is the unique's (none: an ACOG on a sniper).
        e.overlay_material =
            opt(&unique.overlay_material).or_else(|| opt(&unique.overlay_material_low));
        e.facts.overlay_reticle = unique.i32_at(u::overlayReticle);
        // Models: the gun's own attachment models, less the one in the
        // attachment's point when the unique replaces it (a sniper's scope
        // under an ACOG), moved when it says so; then the unique's.
        let point = attachment.i32_at(a::attachmentPoint);
        let replace = unique.bytes.get(u::disableBaseWeaponAttachment).is_some_and(|&b| b != 0);
        let moved = unique
            .bytes
            .get(u::overrideBaseWeaponAttachmentOffsets)
            .is_some_and(|&b| b != 0);
        let mut views: Vec<(String, asset_model::FpvMountOffset)> = Vec::new();
        for (slot, model, tag, offset, rotation) in &w.attach_view_models {
            // Attachment points count from 1 (1: the sight on top); the
            // gun's own attachment models from 0 (0: a sniper's scope).
            let here = usize::try_from(point - 1).is_ok_and(|p| p == *slot);
            if here && replace {
                continue;
            }
            views.push((
                model.clone(),
                asset_model::FpvMountOffset {
                    tag: tag.clone(),
                    offset: if here && moved {
                        unique.vec3_at(u::viewModelOffsetBaseAttachment)
                    } else {
                        *offset
                    },
                    rotation: *rotation,
                    on_arms: false,
                },
            ));
        }
        for (model, offset, rotation) in [
            (&unique.view_model, u::viewModelOffsets, u::viewModelRotations),
            (
                &unique.view_model_additional,
                u::viewModelAddOffsets,
                u::viewModelAddRotations,
            ),
        ] {
            if let Some(model) = opt(model) {
                views.push((
                    model,
                    asset_model::FpvMountOffset {
                        tag: unique.view_model_tag.clone(),
                        offset: unique.vec3_at(offset),
                        rotation: unique.vec3_at(rotation),
                        on_arms: false,
                    },
                ));
            }
        }
        // A dual-wield left gun stays on the arms.
        views.extend(e.view_attachments.iter().filter(|(_, m)| m.on_arms).cloned());
        // A sight that names a stripped model for aiming (a Rangefinder's
        // `viewModelADS`: no housing or lens, so the eyeline is clear)
        // swaps it for its main model while aimed, at the same placement.
        e.ads_view_attachments = match (opt(&unique.view_model_ads), opt(&unique.view_model)) {
            (Some(ads), Some(main)) => views
                .iter()
                .map(|(model, at)| {
                    if *model == main {
                        (ads.clone(), at.clone())
                    } else {
                        (model.clone(), at.clone())
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        e.view_attachments = views;
        // The unique's hidden tags (the iron sights under a reflex).
        if !unique.hide_tags.is_empty() {
            e.hide_tags = unique.hide_tags.clone();
        }
        // Its animations (ADS up and down to the sight's height).
        // (The alternate keeps its own: they are the other mode's.)
        for (slot, name) in remap_t6_sz_xanims(&unique.xanims).into_iter().enumerate() {
            if alt_of.is_some() {
                break;
            }
            if name.is_some() {
                e.sz_xanims[slot] = name;
            }
        }
        // The Hybrid's other mode lines the gun up on the other window: its
        // own slot 0x57 (`rmr_bottom_ads_up`) is the way up to it; its slot
        // 0x55 (`rmr_top_ads_up`) is the way back to the first window, and
        // each mode's slot 0x56 is the way down from its own.
        if alt_of.is_some()
            && let Some(up) = w.xanims.get(0x57).filter(|n| !n.is_empty())
        {
            e.sz_xanims[weap_anim::ADS_UP] = Some(up.to_ascii_lowercase());
        }
        // Zoom: the attachment's field of view (1: the gun's own) and its
        // zoom-in and -out windows when it names them.
        // (The Hybrid has two: the attachment's first (30) is the tube's, its
        // second (50, the gun's own) the reflex window's. The gun's first mode
        // aims through the window (its ADS slots 0x55/0x56 are the `top` ones,
        // its zoom-in/out windows and idle scale are a reflex's, not an
        // ACOG's); the alternate looks through the tube.)
        let fov = attachment.f32_at(a::fAdsZoomFov1);
        if hybrid {
            let mode_fov = if alt_of.is_some() { fov } else { attachment.f32_at(a::fAdsZoomFov2) };
            if mode_fov > 1.0 {
                e.facts.ads_zoom_fov = mode_fov;
            }
        } else if fov > 1.0 {
            e.facts.ads_zoom_fov = fov;
        }
        // Variable Zoom names no first zoom (1) and two others: it opens at
        // the wider and zooms in to the narrower.
        let (fov2, fov3) = (attachment.f32_at(a::fAdsZoomFov2), attachment.f32_at(a::fAdsZoomFov3));
        if fov <= 1.0 && fov2 > 1.0 && fov3 > 1.0 {
            let (wide, narrow) = (fov2.max(fov3), fov2.min(fov3));
            e.facts.ads_zoom_fov = wide;
            e.facts.variable_zoom_fovs = [wide, narrow];
        }
        let zin = attachment.f32_at(a::fAdsZoomInFrac);
        if zin > 0.0 {
            e.facts.ads_zoom_in_frac = zin;
        }
        let zout = attachment.f32_at(a::fAdsZoomOutFrac);
        if zout > 0.0 {
            e.facts.ads_zoom_out_frac = zout;
        }
        for (rate, scale) in [
            (&mut e.facts.ads_in_rate, attachment.f32_at(a::fAdsTransInTimeScale)),
            (&mut e.facts.ads_out_rate, attachment.f32_at(a::fAdsTransOutTimeScale)),
        ] {
            if scale > 0.0 {
                *rate /= scale;
            }
        }
        let idle = attachment.f32_at(a::fAdsIdleAmountScale);
        if idle > 0.0 {
            e.facts.idle.ads_idle_amount *= idle;
        }
        e.facts.sight_reticle_scale = reticle_scale;
        // Hybrid Optic: the swap takes as long as the gun's own swap
        // animation (`rmr_alt_raise_down`, slot 0x1A: 10 frames at 30 fps =
        // 333 ms on every Hybrid gun), not the stat's 700 ms (16 on the
        // HK416), which no swap in the game honours.
        if e.facts.optic_switch
            && let Some(ms) = e.sz_xanims[weap_anim::ALT_RAISE].as_deref().and_then(anim_ms)
            && ms > 0
        {
            e.facts.alternate_raise_time_ms = ms;
        }
        self.entries.push(e);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t6_anim_slots_land_on_their_iw4_names() {
        let mut t6 = vec![String::new(); 88];
        t6[0x01] = "idle".into();
        t6[0x04] = "fire".into();
        t6[0x10] = "reload".into();
        t6[0x55] = "ads_up".into();
        t6[0x56] = "ads_down".into();
        let out = remap_t6_sz_xanims(&t6);
        assert_eq!(out[weap_anim::IDLE].as_deref(), Some("idle"));
        assert_eq!(out[weap_anim::FIRE].as_deref(), Some("fire"));
        assert_eq!(out[weap_anim::RELOAD].as_deref(), Some("reload"));
        assert_eq!(out[weap_anim::ADS_UP].as_deref(), Some("ads_up"));
        assert_eq!(out[weap_anim::ADS_DOWN].as_deref(), Some("ads_down"));
    }

    #[test]
    fn t6_classes_map_to_iw4() {
        assert_eq!(remap_t6_weap_class(4), 5); // pistol
        assert_eq!(remap_t6_weap_class(2), 3); // smg
        assert_eq!(remap_t6_weap_class(13), 4); // pistol spread -> spread
    }
}
