use crate::origin::sample_float_range;
use crate::random::{
    FX_RAND_CH_ANG_VEL_PITCH, FX_RAND_CH_ANG_VEL_ROLL, FX_RAND_CH_ANG_VEL_YAW,
    FX_RAND_CH_SPAWN_ANGLES_PITCH, FX_RAND_CH_SPAWN_ANGLES_ROLL, FX_RAND_CH_SPAWN_ANGLES_YAW,
    random_table_f32,
};

#[inline]
pub fn sample_elem_angles(
    spawn_angles: [[f32; 2]; 3],
    angular_velocity: [[f32; 2]; 3],
    seed: u32,
    age_msec: f32,
) -> [f32; 3] {
    let ch_spawn = [
        FX_RAND_CH_SPAWN_ANGLES_PITCH,
        FX_RAND_CH_SPAWN_ANGLES_YAW,
        FX_RAND_CH_SPAWN_ANGLES_ROLL,
    ];
    let ch_vel = [
        FX_RAND_CH_ANG_VEL_PITCH,
        FX_RAND_CH_ANG_VEL_YAW,
        FX_RAND_CH_ANG_VEL_ROLL,
    ];
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let spawn = sample_float_range(
            spawn_angles[i][0],
            spawn_angles[i][1],
            random_table_f32(seed, ch_spawn[i]),
        );
        let vel = sample_float_range(
            angular_velocity[i][0],
            angular_velocity[i][1],
            random_table_f32(seed, ch_vel[i]),
        );
        out[i] = spawn + age_msec * vel;
    }
    out
}

#[inline]
pub fn angles_to_axis_radians(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let pitch = angles[0];
    let yaw = angles[1];
    let roll = angles[2];
    let cy = libm::cosf(yaw);
    let sy = libm::sinf(yaw);
    let cp = libm::cosf(pitch);
    let sp = libm::sinf(pitch);
    let cr = libm::cosf(roll);
    let sr = libm::sinf(roll);
    [
        [cy * cp, sy * cp, -sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp],
        [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
    ]
}

#[inline]
pub fn mat3_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

#[inline]
pub fn get_elem_angles_axis(
    spawn_angles: [[f32; 2]; 3],
    angular_velocity: [[f32; 2]; 3],
    seed: u32,
    age_msec: f32,
    effect_axis: [[f32; 3]; 3],
) -> [[f32; 3]; 3] {
    let angles = sample_elem_angles(spawn_angles, angular_velocity, seed, age_msec);
    let local = angles_to_axis_radians(angles);
    mat3_mul(local, effect_axis)
}
