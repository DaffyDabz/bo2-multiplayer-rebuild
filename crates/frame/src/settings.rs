use bevy::prelude::Resource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayResolution {
    pub width: u32,
    pub height: u32,
}

impl DisplayResolution {
    pub const HD: Self = Self {
        width: 1280,
        height: 720,
    };

    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl core::fmt::Display for DisplayResolution {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GameSettings {
    pub resolution: DisplayResolution,
    pub fullscreen: bool,
    pub vsync: bool,
    pub fov: f32,
    pub master_volume: f32,
    pub brightness: f32,
    pub shadows: bool,
    pub depth_of_field: bool,
    pub bloom: bool,
    pub sensitivity: f32,
    pub invert_mouse: bool,
    pub player_name: String,

    pub pad_layout: u8,
    pub pad_stick_layout: u8,
    pub pad_sensitivity: f32,
    pub pad_ads_sensitivity: f32,
    pub pad_invert: bool,
    pub pad_curve: u8,
    pub pad_aim_assist: u8,
    pub pad_prompts: u8,
    pub pad_vibration: bool,
    pub pad_deadzone_left: f32,
    pub pad_deadzone_right: f32,

    /// bo2zm: `bo2_quality=fast` (Black Ops II maps: half-size textures,
    /// single-layer ground, props hidden sooner); `full` by default.
    pub bo2_fast: bool,

    pub revision: u64,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            resolution: DisplayResolution::HD,
            fullscreen: false,
            vsync: true,
            fov: Self::FOV_DEFAULT,
            master_volume: 1.0,
            brightness: 0.0,
            shadows: true,
            depth_of_field: true,
            bloom: true,
            sensitivity: 5.0,
            invert_mouse: false,
            player_name: "Player".to_owned(),
            pad_layout: 0,
            pad_stick_layout: 0,
            pad_sensitivity: Self::PAD_SENSITIVITY_DEFAULT,
            pad_ads_sensitivity: 1.0,
            pad_invert: false,
            pad_curve: 0,
            pad_aim_assist: 0,
            pad_prompts: 0,
            pad_vibration: true,
            pad_deadzone_left: 0.12,
            pad_deadzone_right: 0.12,
            bo2_fast: false,
            revision: 0,
        }
    }
}

impl GameSettings {
    pub const FOV_DEFAULT: f32 = 65.0;
    pub const FOV_MIN: f32 = 65.0;
    pub const FOV_MAX: f32 = 120.0;
    pub const PAD_SENSITIVITY_DEFAULT: f32 = 3.0;
    pub const PAD_LAYOUT_CUSTOM: u8 = 255;

    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn sanitize(&mut self) {
        self.resolution.width = self.resolution.width.clamp(640, 7680);
        self.resolution.height = self.resolution.height.clamp(480, 4320);
        self.fov = if self.fov.is_finite() {
            self.fov.clamp(Self::FOV_MIN, Self::FOV_MAX)
        } else {
            Self::FOV_DEFAULT
        };
        self.brightness = if self.brightness.is_finite() {
            self.brightness.clamp(-0.2, 0.2)
        } else {
            0.0
        };
        self.master_volume = self.master_volume.clamp(0.0, 1.0);
        self.sensitivity = self.sensitivity.clamp(0.1, 30.0);
        if self.pad_layout != Self::PAD_LAYOUT_CUSTOM {
            self.pad_layout = self.pad_layout.min(4);
        }
        self.pad_stick_layout = self.pad_stick_layout.min(3);
        self.pad_curve = self.pad_curve.min(2);
        self.pad_aim_assist = 0;
        self.pad_prompts = self.pad_prompts.min(3);
        let finite = |v: f32, lo: f32, hi: f32, default: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        self.pad_sensitivity = finite(
            self.pad_sensitivity,
            1.0,
            10.0,
            Self::PAD_SENSITIVITY_DEFAULT,
        );
        self.pad_ads_sensitivity = finite(self.pad_ads_sensitivity, 0.5, 1.5, 1.0);
        self.pad_deadzone_left = finite(self.pad_deadzone_left, 0.0, 0.4, 0.12);
        self.pad_deadzone_right = finite(self.pad_deadzone_right, 0.0, 0.4, 0.12);
        self.player_name = self.player_name.trim().chars().take(16).collect();
        if self.player_name.is_empty() {
            self.player_name = "Player".to_owned();
        }
    }
}
