extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;

pub const SCREEN_BLEND_BLURRED: i32 = 0;

pub const SCREEN_BLEND_FLASHED: i32 = 1;

pub const SCREEN_BLEND_NONE: i32 = 2;

#[must_use]
pub fn is_flashbanged(cg_time: i32, start_time: i32, duration: i32, screen_type: i32) -> i32 {
    let remaining = start_time.wrapping_sub(cg_time).wrapping_add(duration);
    if remaining < 1 {
        0
    } else if screen_type != SCREEN_BLEND_FLASHED {
        0
    } else {
        remaining
    }
}

const FLASH_FADE_HALF: f32 = 0.5;

const FLASH_FADE_PI: f32 = 3.141592741012573;

#[must_use]
pub fn shellshock_flash_fade_sin_cos(percent: f32) -> f32 {
    let s = libm::sinf((percent - FLASH_FADE_HALF) * FLASH_FADE_PI);
    (s + 1.0) * FLASH_FADE_HALF
}

#[must_use]
pub fn shellshock_flash_blend(
    remaining_ms: i32,
    white_fade_ms: i32,
    shot_fade_ms: i32,
) -> Option<(f32, f32)> {
    if remaining_ms < 1 {
        return None;
    }
    let dt = remaining_ms as f32;
    let white_lin = if white_fade_ms <= 0 || (white_fade_ms as f32) <= dt {
        1.0
    } else {
        dt / white_fade_ms as f32
    };
    let shot_lin = if shot_fade_ms <= 0 || (shot_fade_ms as f32) <= dt {
        1.0
    } else {
        dt / shot_fade_ms as f32
    };
    Some((
        shellshock_flash_fade_sin_cos(white_lin),
        shellshock_flash_fade_sin_cos(shot_lin),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellshockLookParms {
    pub affect: bool,

    pub fade_ms: i32,

    pub mouse_sensitivity: f32,

    pub max_pitch_speed: f32,

    pub max_yaw_speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellshockLookState {
    pub sensitivity: f32,

    pub max_pitch_speed: f32,

    pub max_yaw_speed: f32,
}

const LOOK_ENDED: ShellshockLookState = ShellshockLookState {
    sensitivity: 1.0,
    max_pitch_speed: 0.0,
    max_yaw_speed: 0.0,
};

#[must_use]
pub fn shellshock_remaining_ms(cg_time: i32, start_time: i32, duration: i32) -> i32 {
    if start_time == 0 {
        return 0;
    }
    let elapsed = cg_time.wrapping_sub(start_time);
    if elapsed < 0 {
        return 0;
    }
    let remaining = duration.wrapping_sub(elapsed);
    if remaining < 1 { 0 } else { remaining }
}

#[must_use]
pub fn update_shellshock_look_control(
    cg_time: i32,
    start_time: i32,
    duration: i32,
    parms: ShellshockLookParms,
) -> ShellshockLookState {
    let elapsed = cg_time.wrapping_sub(start_time);
    if start_time == 0 || elapsed < 0 || !parms.affect {
        return LOOK_ENDED;
    }
    let remaining = duration.wrapping_sub(elapsed);
    if remaining < parms.fade_ms {
        if remaining < 1 {
            return LOOK_ENDED;
        }
        let fade = remaining as f32 / parms.fade_ms as f32;
        if fade == 1.0 {
            return ShellshockLookState {
                sensitivity: parms.mouse_sensitivity,
                max_pitch_speed: parms.max_pitch_speed,
                max_yaw_speed: parms.max_yaw_speed,
            };
        }
        ShellshockLookState {
            sensitivity: fade * (parms.mouse_sensitivity - 1.0) + 1.0,
            max_pitch_speed: parms.max_pitch_speed / fade,
            max_yaw_speed: parms.max_yaw_speed / fade,
        }
    } else {
        ShellshockLookState {
            sensitivity: parms.mouse_sensitivity,
            max_pitch_speed: parms.max_pitch_speed,
            max_yaw_speed: parms.max_yaw_speed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellshockSoundParms {
    pub affect: bool,

    pub loop_alias: String,

    pub end_alias: String,

    pub abort_alias: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShockParams {
    pub screen_type: i32,

    pub white_fade_ms: i32,

    pub shot_fade_ms: i32,

    pub look: ShellshockLookParms,

    pub sound: ShellshockSoundParms,

    pub movement: bool,

    /// BO2: `bg_shock_movement` is a speed scale while shocked (flashbang
    /// 0.8, concussion 0.32), not a switch. 0 when the file has none.
    pub movement_scale: f32,

    /// BO2: `bg_shock_viewKickPeriod` / `Radius` / `FadeTime`, in seconds
    /// (the shock's view shake; read by the view-kick code). 0 when absent.
    pub view_kick_period: f32,
    pub view_kick_radius: f32,
    pub view_kick_fade: f32,

    /// BO2: `bg_shock_visionset_name` and its in / out times in seconds.
    pub visionset: String,
    pub visionset_in: f32,
    pub visionset_out: f32,
}

impl ShockParams {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            let Some((key, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            values.insert(key.to_ascii_lowercase(), value.to_owned());
        }
        let text = |key: &str| {
            values
                .get(&format!("bg_shock_{}", key.to_ascii_lowercase()))
                .cloned()
                .ok_or_else(|| format!("bg_shock_{key} is missing"))
        };
        let number = |key: &str| -> Result<f32, String> {
            let value = text(key)?;
            value
                .parse::<f32>()
                .map_err(|_| format!("bg_shock_{key} \"{value}\" is not a number"))
        };
        let ms = |key: &str| number(key).map(|seconds| libm::roundf(seconds * 1000.0) as i32);
        let flag = |key: &str| number(key).map(|value| value != 0.0);
        let optional = |key: &str| -> f32 {
            text(key)
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(0.0)
        };
        let screen_type = match text("screenType")?.to_ascii_lowercase().as_str() {
            "blurred" => SCREEN_BLEND_BLURRED,
            "flashed" => SCREEN_BLEND_FLASHED,
            "none" => SCREEN_BLEND_NONE,
            other => {
                return Err(format!(
                    "bg_shock_screenType \"{other}\" is not blurred, flashed or none"
                ));
            }
        };
        Ok(Self {
            screen_type,
            white_fade_ms: ms("screenFlashWhiteFadeTime")?,
            shot_fade_ms: ms("screenFlashShotFadeTime")?,
            look: ShellshockLookParms {
                affect: flag("lookControl")?,
                fade_ms: ms("lookControl_fadeTime")?,
                mouse_sensitivity: number("lookControl_mousesensitivityscale")?,
                max_pitch_speed: number("lookControl_maxpitchspeed")?,
                max_yaw_speed: number("lookControl_maxyawspeed")?,
            },
            sound: ShellshockSoundParms {
                affect: flag("sound")?,
                loop_alias: text("soundLoop")?,
                end_alias: text("soundEnd")?,
                abort_alias: text("soundEndAbort")?,
            },
            movement: flag("movement")?,
            movement_scale: optional("movement"),
            view_kick_period: optional("viewKickPeriod"),
            view_kick_radius: optional("viewKickRadius"),
            view_kick_fade: optional("viewKickFadeTime"),
            visionset: text("visionset_name").unwrap_or_default(),
            visionset_in: optional("visionset_inTime"),
            visionset_out: optional("visionset_outTime"),
        })
    }
}

#[cfg(test)]
mod bo2_shock_tests {
    use super::ShockParams;

    // BO2's common_mp shock/flashbang.shock and shock/concussion_grenade_mp.shock
    // (the values the lines that matter are copied from).
    const FLASHBANG: &str = r#"
bg_shock_screenType "flashed"
bg_shock_screenFlashWhiteFadeTime "3.5"
bg_shock_screenFlashShotFadeTime "1.0"
bg_shock_viewKickPeriod ".75"
bg_shock_viewKickRadius ".05"
bg_shock_viewKickFadeTime "3"
bg_shock_sound "1"
bg_shock_soundLoop "chr_flashbang_tinnitus_loop"
bg_shock_soundEnd ""
bg_shock_soundEndAbort ""
bg_shock_lookControl "0"
bg_shock_lookControl_maxpitchspeed "90"
bg_shock_lookControl_maxyawspeed "90"
bg_shock_lookControl_mousesensitivityscale "0.5"
bg_shock_lookControl_fadeTime "2"
bg_shock_movement "0.8"
bg_shock_visionset_name ""
bg_shock_visionset_inTime "0"
bg_shock_visionset_outTime "0"
"#;

    const CONCUSSION: &str = r#"
bg_shock_screenType "flashed"
bg_shock_screenFlashWhiteFadeTime "7.5"
bg_shock_screenFlashShotFadeTime "15"
bg_shock_viewKickPeriod ".75"
bg_shock_viewKickRadius ".1"
bg_shock_viewKickFadeTime "3"
bg_shock_sound "1"
bg_shock_soundLoop "chr_tinitus_loop"
bg_shock_soundEnd ""
bg_shock_soundEndAbort ""
bg_shock_lookControl "1"
bg_shock_lookControl_maxpitchspeed "22"
bg_shock_lookControl_maxyawspeed "22"
bg_shock_lookControl_mousesensitivityscale "0.1"
bg_shock_lookControl_fadeTime "2"
bg_shock_movement "0.32"
bg_shock_visionset_name "concussion_grenade"
bg_shock_visionset_inTime "0"
bg_shock_visionset_outTime "3.5"
"#;

    #[test]
    fn flashbang_movement_is_the_files_scale() {
        let p = ShockParams::parse(FLASHBANG).unwrap();
        assert_eq!(p.movement_scale, 0.8);
        assert_eq!(p.white_fade_ms, 3500);
        assert_eq!(p.view_kick_radius, 0.05);
        assert_eq!(p.visionset, "");
    }

    #[test]
    fn concussion_has_look_limits_scale_and_vision() {
        let p = ShockParams::parse(CONCUSSION).unwrap();
        assert_eq!(p.movement_scale, 0.32);
        assert!(p.look.affect);
        assert_eq!(p.look.max_yaw_speed, 22.0);
        assert_eq!(p.visionset, "concussion_grenade");
        assert_eq!(p.visionset_out, 3.5);
    }
}
