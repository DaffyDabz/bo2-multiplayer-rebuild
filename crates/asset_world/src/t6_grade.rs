//! bo2mp: Black Ops II's final colour, from the map's own vision file
//! (`vision/<map>.vision`) and colour table (`GfxWorld::lutMaterial`).
//!
//! The game builds a 32x32x32 colour table each frame with
//! `hdr_create_lut2dv` (fxc disassembly of its pixel shader) and looks the
//! frame up in it after bloom and the highlight roll-off (`hdr_bloom_apply`):
//!
//! - start from the map's own table (band 0 of the lut material's image,
//!   gamma values; the band picked when no LUT volume holds the eye),
//! - weights by its luminance (Rec. 709, on the gamma values): shadows a
//!   falling ramp `vc_RS.x..vc_RE.x`, highlights a rising ramp `y`,
//!   midtones a rising ramp `z` times a falling ramp `w`; each smoothstep,
//!   the three normalised,
//! - in linear (squared): the shadow, midtone and highlight matrices
//!   (`vc_SM*`, `vc_MM*`, `vc_HM*`, rows with an offset) mixed by those
//!   weights, clamped; then per channel the power `vc_FGM`, the saturation
//!   `vc_FSM` (weights, amount), the blend back toward the input `vc_FBM`,
//!   then the code's colour matrix (identity: the vision's levels are),
//!   clamped and square-rooted.
//!
//! The vision's bloom rows (`vc_RGBH`, `vc_RGBL`, `vc_YH`, `vc_YL`) are the
//! tints `hdr_bloom_combine_hilo` puts the high and low bloom levels'
//! colour and luminance back with.

use std::sync::{Arc, RwLock};

/// The `vc_*` rows of a Black Ops II vision file (four floats each).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct T6Vision {
    pub film_enable: bool,
    pub levels_in_black: [f32; 4],
    pub levels_in_white: [f32; 4],
    pub levels_in_gamma: [f32; 4],
    pub levels_out_black: [f32; 4],
    pub levels_out_white: [f32; 4],
    pub bloom_rgb_hi: [f32; 4],
    pub bloom_rgb_lo: [f32; 4],
    pub bloom_y_hi: [f32; 4],
    pub bloom_y_lo: [f32; 4],
    pub range_start: [f32; 4],
    pub range_end: [f32; 4],
    pub shadow: [[f32; 4]; 3],
    pub highlight: [[f32; 4]; 3],
    pub midtone: [[f32; 4]; 3],
    pub final_gamma: [f32; 4],
    pub final_saturation: [f32; 4],
    pub final_blend: [f32; 4],
}

impl Default for T6Vision {
    /// What changes nothing: identity levels and matrices, full saturation.
    fn default() -> Self {
        let row = |i: usize| {
            let mut r = [0.0; 4];
            r[i] = 1.0;
            r
        };
        let identity = [row(0), row(1), row(2)];
        Self {
            film_enable: false,
            levels_in_black: [0.0; 4],
            levels_in_white: [32.0; 4],
            levels_in_gamma: [1.0; 4],
            levels_out_black: [0.0; 4],
            levels_out_white: [32.0; 4],
            bloom_rgb_hi: [0.0; 4],
            bloom_rgb_lo: [0.0; 4],
            bloom_y_hi: [0.0; 4],
            bloom_y_lo: [0.0; 4],
            range_start: [0.0, 0.0, 0.0, 0.5],
            range_end: [1.0, 1.0, 0.5, 1.0],
            shadow: identity,
            highlight: identity,
            midtone: identity,
            final_gamma: [1.0, 1.0, 1.0, 0.0],
            final_saturation: [T6_LUMA[0], T6_LUMA[1], T6_LUMA[2], 1.0],
            final_blend: [1.0, 1.0, 1.0, 0.0],
        }
    }
}

/// The luminance weights BO2's post shaders use.
pub const T6_LUMA: [f32; 3] = [0.212585, 0.715195, 0.07222];

/// Parse a vision file's text: `name "values"` per line. Rows it does not
/// name keep their identity value; `None` when it names no `vc_` row.
pub fn parse_t6_vision(text: &str) -> Option<T6Vision> {
    let mut v = T6Vision::default();
    let mut found = false;
    for line in text.lines() {
        let line = line.trim();
        let Some((name, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = rest.trim().trim_matches('"');
        let mut four = [0.0f32; 4];
        let mut n = 0;
        for (slot, part) in four.iter_mut().zip(value.split_whitespace()) {
            let Ok(x) = part.parse::<f32>() else {
                break;
            };
            *slot = x;
            n += 1;
        }
        let name = name.to_ascii_lowercase();
        if name == "r_filmenable" {
            v.film_enable = value.trim() != "0";
            continue;
        }
        if n != 4 {
            continue;
        }
        let target = match name.as_str() {
            "vc_lib" => &mut v.levels_in_black,
            "vc_liw" => &mut v.levels_in_white,
            "vc_lig" => &mut v.levels_in_gamma,
            "vc_lob" => &mut v.levels_out_black,
            "vc_low" => &mut v.levels_out_white,
            "vc_rgbh" => &mut v.bloom_rgb_hi,
            "vc_rgbl" => &mut v.bloom_rgb_lo,
            "vc_yh" => &mut v.bloom_y_hi,
            "vc_yl" => &mut v.bloom_y_lo,
            "vc_rs" => &mut v.range_start,
            "vc_re" => &mut v.range_end,
            "vc_smr" => &mut v.shadow[0],
            "vc_smg" => &mut v.shadow[1],
            "vc_smb" => &mut v.shadow[2],
            "vc_hmr" => &mut v.highlight[0],
            "vc_hmg" => &mut v.highlight[1],
            "vc_hmb" => &mut v.highlight[2],
            "vc_mmr" => &mut v.midtone[0],
            "vc_mmg" => &mut v.midtone[1],
            "vc_mmb" => &mut v.midtone[2],
            "vc_fgm" => &mut v.final_gamma,
            "vc_fsm" => &mut v.final_saturation,
            "vc_fbm" => &mut v.final_blend,
            _ => continue,
        };
        *target = four;
        found = true;
    }
    found.then_some(v)
}

/// Side of the colour table.
pub const T6_LUT_SIZE: usize = 32;

/// One colour (gamma values, the table's input) through `hdr_create_lut2dv`
/// with `alpha` the base table's alpha. Returns gamma values.
pub fn t6_grade_colour(v: &T6Vision, c: [f32; 3], alpha: f32) -> [f32; 3] {
    let dot3 = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let l = dot3(c, T6_LUMA);
    // The code passes each ramp as scale and bias (mad_sat): rising from
    // start to end, or falling.
    let ramp = |i: usize, rising: bool| {
        let (s, e) = (v.range_start[i], v.range_end[i]);
        let t = if (e - s).abs() < 1e-6 {
            if l >= s { 1.0 } else { 0.0 }
        } else {
            ((l - s) / (e - s)).clamp(0.0, 1.0)
        };
        if rising { t } else { 1.0 - t }
    };
    let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
    let mut w = [
        smooth(ramp(0, false)),
        smooth(ramp(2, true) * ramp(3, false)),
        smooth(ramp(1, true)),
    ];
    let sum = w[0] + w[1] + w[2];
    if sum > 0.0 {
        w = w.map(|x| x / sum);
    }
    let lin = c.map(|x| x * x);
    let row = |r: [f32; 4]| r[0] * lin[0] + r[1] * lin[1] + r[2] * lin[2] + r[3] * alpha;
    let mut out = [0.0f32; 3];
    for (ch, o) in out.iter_mut().enumerate() {
        let graded =
            w[0] * row(v.shadow[ch]) + w[1] * row(v.midtone[ch]) + w[2] * row(v.highlight[ch]);
        *o = graded.clamp(0.0, 1.0).powf(v.final_gamma[ch]);
    }
    let fs = v.final_saturation;
    let lum = dot3(out, [fs[0], fs[1], fs[2]]);
    for (ch, o) in out.iter_mut().enumerate() {
        let saturated = lum + fs[3] * (*o - lum);
        let blended = lin[ch] + v.final_blend[ch] * (saturated - lin[ch]);
        *o = blended.clamp(0.0, 1.0).sqrt();
    }
    out
}

/// The 32x32x32 table, RGBA8, red fastest then green then blue (a 3D
/// texture's layout). `base` is the map's own table: band 0 of its lut
/// image (1024x32 RGBA8, texel (b * 32 + r, g)); `None` = identity.
pub fn t6_grade_lut(v: &T6Vision, base: Option<&[u8]>) -> Vec<u8> {
    let n = T6_LUT_SIZE;
    let base = base.filter(|b| b.len() >= n * n * n * 4);
    let mut out = Vec::with_capacity(n * n * n * 4);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let (c, a) = match base {
                    Some(px) => {
                        let o = (g * n * n + b * n + r) * 4;
                        (
                            [0, 1, 2].map(|i| f32::from(px[o + i]) / 255.0),
                            f32::from(px[o + 3]) / 255.0,
                        )
                    }
                    None => ([r, g, b].map(|x| x as f32 / (n - 1) as f32), 1.0),
                };
                let graded = t6_grade_colour(v, c, a);
                for x in graded {
                    out.push((x * 255.0).round().clamp(0.0, 255.0) as u8);
                }
                out.push(255);
            }
        }
    }
    out
}

/// What the renderer's final pass draws a Black Ops II map with.
#[derive(Clone, Debug)]
pub struct T6Grade {
    /// `t6_grade_lut`'s table.
    pub lut: Vec<u8>,
    pub vision: T6Vision,
}

fn grade_slot() -> &'static RwLock<(u64, Option<Arc<T6Grade>>)> {
    static SLOT: std::sync::OnceLock<RwLock<(u64, Option<Arc<T6Grade>>)>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(Default::default)
}

/// Installed once per Black Ops II map load (`None`: the map has no vision).
pub fn install_t6_grade(grade: Option<T6Grade>) {
    if let Ok(mut s) = grade_slot().write() {
        s.0 += 1;
        s.1 = grade.map(Arc::new);
    }
}

/// bo2mp: every named vision the zones hold (`vision/<name>.vision`, lower
/// case) and the map's colour table band, so a script's `visionsetnaked`
/// (`mpOutro` at a match's end, back to the map's own after) can switch the
/// frame's grade.
fn vision_registry() -> &'static RwLock<(std::collections::HashMap<String, T6Vision>, Option<Vec<u8>>)> {
    static REG: std::sync::OnceLock<
        RwLock<(std::collections::HashMap<String, T6Vision>, Option<Vec<u8>>)>,
    > = std::sync::OnceLock::new();
    REG.get_or_init(Default::default)
}

/// Add named visions (a later one of a name wins).
pub fn register_t6_visions(visions: Vec<(String, T6Vision)>) {
    if let Ok(mut r) = vision_registry().write() {
        for (name, v) in visions {
            r.0.insert(name.to_ascii_lowercase(), v);
        }
    }
}

/// The installed map's colour table band 0 (what every vision's table is
/// built over), `None` when the map has none.
pub fn set_t6_vision_base(base: Option<Vec<u8>>) {
    if let Ok(mut r) = vision_registry().write() {
        r.1 = base;
    }
}

/// `visionsetnaked <name>`: install that vision's grade. False when no
/// zone holds the name (the grade stays).
pub fn set_t6_vision(name: &str) -> bool {
    let Some((vision, base)) = vision_registry()
        .read()
        .ok()
        .and_then(|r| r.0.get(&name.to_ascii_lowercase()).map(|v| (*v, r.1.clone())))
    else {
        return false;
    };
    install_t6_grade(Some(T6Grade { lut: t6_grade_lut(&vision, base.as_deref()), vision }));
    true
}

/// The installed grade and its generation (bumped by every install).
pub fn t6_grade() -> (u64, Option<Arc<T6Grade>>) {
    grade_slot()
        .read()
        .map(|s| (s.0, s.1.clone()))
        .unwrap_or((0, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUKETOWN_MP: &str = "r_filmEnable       \"1\"\n\
vc_LIB \"0.000000 0.000000 0.000000 0.000000\"\n\
vc_RGBH \"0.090000 0.090000 0.090000 0.650000\"\n\
vc_RS \"0.000000 0.000000 0.000000 0.500000\"\n\
vc_RE \"1.000000 1.000000 0.500000 1.000000\"\n\
vc_SMR \"1.164402 0.000000 0.000000 0.002557\"\n\
vc_FSM \"0.212585 0.715195 0.072220 0.880859\"\n";

    #[test]
    fn parses_rows() {
        let v = parse_t6_vision(NUKETOWN_MP).unwrap();
        assert!(v.film_enable);
        assert_eq!(v.bloom_rgb_hi, [0.09, 0.09, 0.09, 0.65]);
        assert_eq!(v.shadow[0], [1.164402, 0.0, 0.0, 0.002557]);
        assert_eq!(v.shadow[1], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(v.final_saturation[3], 0.880859);
        assert!(parse_t6_vision("r_filmEnable \"1\"\n").is_none());
    }

    #[test]
    fn default_vision_is_identity() {
        let v = T6Vision::default();
        for c in [
            [0.0, 0.0, 0.0],
            [0.25, 0.5, 0.75],
            [1.0, 1.0, 1.0],
            [0.9, 0.1, 0.3],
        ] {
            let o = t6_grade_colour(&v, c, 1.0);
            for i in 0..3 {
                assert!((o[i] - c[i]).abs() < 1e-5, "{c:?} -> {o:?}");
            }
        }
        let lut = t6_grade_lut(&v, None);
        assert_eq!(lut.len(), 32 * 32 * 32 * 4);
        // red fastest, then green, then blue
        assert_eq!(&lut[31 * 4..31 * 4 + 4], &[255, 0, 0, 255]);
        assert_eq!(&lut[32 * 31 * 4..32 * 31 * 4 + 4], &[0, 255, 0, 255]);
        assert_eq!(
            &lut[32 * 32 * 31 * 4..32 * 32 * 31 * 4 + 4],
            &[0, 0, 255, 255]
        );
    }

    #[test]
    fn saturation_pulls_toward_luminance() {
        let mut v = T6Vision::default();
        v.final_saturation[3] = 0.5;
        let o = t6_grade_colour(&v, [1.0, 0.0, 0.0], 1.0);
        let lum = T6_LUMA[0];
        assert!((o[0] - (lum + 0.5 * (1.0 - lum)).sqrt()).abs() < 1e-5);
        assert!((o[1] - (lum * 0.5).sqrt()).abs() < 1e-5);
    }
}
