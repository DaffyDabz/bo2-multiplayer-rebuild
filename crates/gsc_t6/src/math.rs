//! Angle and vector math the scripts use (Quake/IW conventions: angles are
//! pitch, yaw, roll in degrees; forward = +x at yaw 0, left = +y, up = +z).

pub fn angle_vectors(a: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sp, cp) = a[0].to_radians().sin_cos();
    let (sy, cy) = a[1].to_radians().sin_cos();
    let (sr, cr) = a[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    (forward, right, up)
}

pub fn vector_to_angles(d: [f32; 3]) -> [f32; 3] {
    if d[0] == 0.0 && d[1] == 0.0 {
        let pitch = if d[2] > 0.0 { 270.0 } else { 90.0 };
        return [pitch, 0.0, 0.0];
    }
    let mut yaw = d[1].atan2(d[0]).to_degrees();
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let forward = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let mut pitch = (-d[2]).atan2(forward).to_degrees();
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw, 0.0]
}

/// An angle in (-180, 180].
pub fn angle_clamp180(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    } else if a <= -180.0 {
        a += 360.0;
    }
    a
}

/// An angle in [0, 360).
pub fn angle_clamp(a: f32) -> f32 {
    let a = a % 360.0;
    if a < 0.0 { a + 360.0 } else { a }
}

/// `atoi`: leading integer, 0 if none.
pub fn parse_int(s: &str) -> i32 {
    let s = s.trim();
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    s[..end]
        .parse::<i32>()
        .unwrap_or_else(|_| s.parse::<f32>().map(|f| f as i32).unwrap_or(0))
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

pub fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 0.0 { scale(a, 1.0 / l) } else { [0.0; 3] }
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
