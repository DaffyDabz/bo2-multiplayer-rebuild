pub use render_scene::{FlyCamera, FpvLens, SimCamera, WorldCameraPose, transform_from_iw_view};

use bevy::prelude::*;

pub(crate) fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    sim_cam: Option<Res<SimCamera>>,
    mut q: Query<(&mut Transform, &mut FlyCamera)>,
) {
    let (sim_driven, freeze_fly) = sim_cam
        .as_ref()
        .map(|c| (c.enabled, c.freeze_fly))
        .unwrap_or((false, false));
    if sim_driven || freeze_fly {
        return;
    }
    let dt = time.delta_secs();
    for (mut transform, mut cam) in &mut q {
        let look = 1.6 * dt;
        let mut yaw_delta = 0.0;
        let mut pitch_delta = 0.0;
        if keys.pressed(KeyCode::ArrowLeft) {
            yaw_delta += look;
        }
        if keys.pressed(KeyCode::ArrowRight) {
            yaw_delta -= look;
        }
        if keys.pressed(KeyCode::ArrowUp) {
            pitch_delta += look;
        }
        if keys.pressed(KeyCode::ArrowDown) {
            pitch_delta -= look;
        }
        let new_pitch = (cam.pitch + pitch_delta).clamp(-1.54, 1.54);
        pitch_delta = new_pitch - cam.pitch;
        cam.yaw += yaw_delta;
        cam.pitch = new_pitch;

        if yaw_delta != 0.0 {
            transform.rotate_z(yaw_delta);
        }
        if pitch_delta != 0.0 {
            transform.rotate_local_x(pitch_delta);
        }

        if sim_driven {
            continue;
        }

        let mut movement = Vec3::ZERO;
        let forward = {
            let f = *transform.forward();
            let flat = Vec3::new(f.x, f.y, 0.0);
            if flat.length_squared() > 1e-6 {
                flat.normalize()
            } else {
                Vec3::Y
            }
        };
        let right = Vec3::new(forward.y, -forward.x, 0.0);
        if keys.pressed(KeyCode::KeyW) {
            movement += forward;
        }
        if keys.pressed(KeyCode::KeyS) {
            movement -= forward;
        }
        if keys.pressed(KeyCode::KeyD) {
            movement += right;
        }
        if keys.pressed(KeyCode::KeyA) {
            movement -= right;
        }
        if keys.pressed(KeyCode::KeyE) {
            movement += Vec3::Z;
        }
        if keys.pressed(KeyCode::KeyQ) {
            movement -= Vec3::Z;
        }

        if movement != Vec3::ZERO {
            let sprint = if keys.pressed(KeyCode::ShiftLeft) {
                6.0
            } else {
                1.0
            };
            transform.translation += movement.normalize() * cam.speed * sprint * dt;
        }
    }
}

/// bo2mp look lane: `IW4L_LOOK_CAMERA="x y z yaw pitch"` holds the world
/// camera at that pose whatever the player does (a test aid: views no
/// player can stand at, above the sea or a lava pool).
pub(crate) fn look_camera_override(mut q: Query<&mut Transform, With<FlyCamera>>) {
    static POSE: std::sync::OnceLock<Option<[f32; 5]>> = std::sync::OnceLock::new();
    let pose = POSE.get_or_init(|| {
        let v: Vec<f32> = std::env::var("IW4L_LOOK_CAMERA")
            .ok()?
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        <[f32; 5]>::try_from(v).ok()
    });
    let Some([x, y, z, yaw, pitch]) = *pose else {
        return;
    };
    let held = transform_from_iw_view(WorldCameraPose {
        origin: [x, y, z],
        angles: [pitch, yaw, 0.0],
    });
    for mut transform in &mut q {
        *transform = held;
    }
}
