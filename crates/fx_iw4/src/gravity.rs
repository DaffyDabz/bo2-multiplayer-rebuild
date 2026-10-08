use crate::origin::sample_float_range;
use crate::random::{FX_RAND_CH_GRAVITY, random_table_f32};

pub const FX_GRAVITY: f64 = 800.0;

#[inline]
pub fn elem_gravity_accel_z(authored: f32) -> f32 {
    authored * (FX_GRAVITY as f32)
}

#[inline]
pub fn sample_gravity_authored(base: f32, amplitude: f32, seed: u32) -> f32 {
    sample_float_range(base, amplitude, random_table_f32(seed, FX_RAND_CH_GRAVITY))
}

#[inline]
pub fn elem_gravity_accel_z_sampled(base: f32, amplitude: f32, seed: u32) -> f32 {
    elem_gravity_accel_z(sample_gravity_authored(base, amplitude, seed))
}
