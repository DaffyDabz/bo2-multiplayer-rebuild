//! bo2mp: the front end's 3D backdrop, drawn on the CPU from BO2's own
//! textures. `ui_mp/t6/main.lua` builds it from widgets the engine draws in
//! 3D, which the 2D layer cannot: the holotable floor (three pictures
//! `ui_holotable_grid`, `_grid3` at half brightness and `_grid2` 32 units
//! lower, each 2560 units square, 400 below the screen's middle, turned
//! -80 degrees about its width so it lies back into the screen) and the
//! globe (`setupGlobe`, material `ui_globe`: the world map `globe_map` over
//! its dotted mesh `globe_map_mesh`, a sphere 720 units across at the
//! left, turning).
//!
//! Ours (no engine numbers for these): the camera's distance for the
//! floor's perspective, the tiles' size on it and its fade with distance;
//! the globe's tilt, start longitude, turning speed, rim light and the
//! mesh's strength. They are set to match the real game's lobby shots.

use bevy::prelude::*;

/// A texture's pixels (RGBA8, as stored).
pub(crate) struct Tex<'a> {
    pub w: usize,
    pub h: usize,
    pub px: &'a [u8],
}

impl<'a> Tex<'a> {
    pub fn of(img: &'a Image) -> Option<Tex<'a>> {
        let w = img.texture_descriptor.size.width as usize;
        let h = img.texture_descriptor.size.height as usize;
        let px = img.data.as_deref()?;
        (px.len() >= w * h * 4 && w > 0 && h > 0).then_some(Tex { w, h, px })
    }

    /// Bilinear sample at (u, v) in 0..1, repeating; 0..1 floats.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u.rem_euclid(1.0) * self.w as f32 - 0.5;
        let y = v.rem_euclid(1.0) * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| {
            let xi = (xi as isize).rem_euclid(self.w as isize) as usize;
            let yi = (yi as isize).rem_euclid(self.h as isize) as usize;
            let i = (yi * self.w + xi) * 4;
            [
                f32::from(self.px[i]) / 255.0,
                f32::from(self.px[i + 1]) / 255.0,
                f32::from(self.px[i + 2]) / 255.0,
                f32::from(self.px[i + 3]) / 255.0,
            ]
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        let mut out = [0.0; 4];
        for k in 0..4 {
            out[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
        }
        out
    }
}

/// The camera's distance from the screen, in root units (ours).
const EYE: f32 = 1000.0;
/// One tile of the finer pictures on the floor, in units (BO2's: they lay
/// 32 tiles over their 2560 units; the larger ones 8).
const TILE: f32 = 80.0;
/// Where the radial light is centred along the floor (the shader adds the
/// globe's position to its UV; ours: the globe's middle, left of centre).
const GLOBE_U: f32 = -490.0;
/// The pictures' size, in units, for the radial terms (BO2's own).
const QUAD: f32 = 2560.0;

fn frac(x: f32) -> f32 {
    x - x.floor()
}

/// C's `x - trunc(x)`, as the shaders' `frc` of a signed value.
fn sfrac(x: f32) -> f32 {
    x - x.trunc()
}

/// The picture's own turn, 25.2 degrees, about its middle.
fn turn(q: [f32; 2]) -> [f32; 2] {
    let (c, s) = (0.904827f32, 0.425779f32);
    [c * (q[0] - 0.5) + s * (q[1] - 0.5) + 0.5, -s * (q[0] - 0.5) + c * (q[1] - 0.5) + 0.5]
}

/// `sw4_2d_holotable_grid`'s pixel shader (`ui_holotable_grid`, the small stars),
/// constants as disassembled: `q` the quad's UV (sized here to the tiles,
/// see `floor`), `t` the game's clock in seconds, `tv` the `tv_lookup`
/// picture, `tex` the tile picture (32 tiles across), `dn` the distance
/// from the quad's middle (1 at its corners). Returns the colour before the
/// vertex colour.
fn grid_ps(q: [f32; 2], t: f32, tv: Option<&Tex>, tex: &Tex, dn: f32) -> [f32; 3] {
    // A ring of light running out from the middle.
    let fy = sfrac(0.4 + 0.175 * t);
    let ring = (dn - 2.0 * fy + 1.0).clamp(0.0, 1.0);
    let g = 1.0 - (12.0 * (ring - 0.5).abs()).min(1.0);
    let off = g * 0.005;
    let bright = g * 0.1 + 0.25;
    let rq = turn(q);
    // The lookup splits the colours apart sideways, over time.
    let l = tv.map_or(0.5, |tv| tv.sample(frac(0.5 + 0.25 * t), rq[1])[0]);
    let split = 2.0 * l * off;
    let (sx, sy) = (frac(t * 0.000625), frac(t * -0.00125));
    let at = |dx: f32| tex.sample((rq[0] + dx + sx) * 32.0, (rq[1] + sy) * 32.0);
    let (rp, gc, bm) = (at(split), at(0.0), at(-split));
    let base = [rp[0] * bright, gc[1] * bright, bm[2] * bright];
    // Toward orange near the middle, pulsing in and out.
    let tri = 4.0 - 2.0 * (2.0 * sfrac(0.175 * t).abs() - 1.0).abs();
    let k = 0.5 * (1.0 - (tri * dn.min(1.0)).clamp(0.0, 1.0));
    let orange = [1.0, 0.5, 0.0];
    let mut out = [0.0; 3];
    for c in 0..3 {
        out[c] = base[c] + k * (orange[c] - base[c]);
    }
    out
}

/// `sw4_2d_holotable_grid2`'s pixel shader (`ui_holotable_grid2` and `_grid3`: the two layers of orange crosses, as the real shot shows them in pairs): the
/// larger crosses (8 tiles across), their colours split by the lookup, the
/// whole pulsing between three quarters and twice as bright.
fn grid2_ps(q: [f32; 2], t: f32, tv: Option<&Tex>, tex: &Tex) -> [f32; 3] {
    let pulse = 0.75 + 1.25 * (2.0 * sfrac(0.175 * t).abs() - 1.0).abs();
    let rq = turn(q);
    let tc = [(rq[0] + frac(t * 0.00125)) * 8.0, (rq[1] + frac(t * -0.0025)) * 8.0];
    let l = tv.map_or(0.5, |tv| tv.sample(frac(0.25 * t + 0.5), tc[1])[0]);
    let split = 0.01 * (l * l) * (l * l) * 8.0;
    let c = tex.sample(tc[0], tc[1]);
    let xp = tex.sample(tc[0] + split, tc[1]);
    let xm = tex.sample(tc[0] - split, tc[1]);
    [xp[0] * pulse, xm[1] * pulse, c[2] * pulse]
}

/// The holotable floor over the whole root (`w` x 720 units) as an RGBA8
/// picture `pw` x `ph` pixels at the game's clock `t`: the three grids
/// added up (they are additive in BO2), their brightness as alpha.
pub(crate) fn floor(w: f32, pw: usize, ph: usize, t: f32, tv: Option<&Tex>, grid: &Tex, grid2: &Tex, grid3: &Tex) -> Vec<u8> {
    let (s, c) = (80f32.to_radians().sin(), 80f32.to_radians().cos());
    let mut out = vec![0u8; pw * ph * 4];
    for py in 0..ph {
        // Screen y about the middle (root units, down).
        let sy = (py as f32 + 0.5) / ph as f32 * 720.0 - 360.0;
        // Where the ray meets each plane (offset `dy` lower): v along the
        // plane (negative = further away), its depth.
        let hit = |dy: f32| {
            let v = EYE * (sy - (400.0 + dy)) / (sy * s + c * EYE);
            let depth = EYE - v * s;
            (v, depth)
        };
        for px in 0..pw {
            let sx = (px as f32 + 0.5) / pw as f32 * w - w * 0.5;
            let mut rgb = [0.0f32; 3];
            // (picture, the vertex colour, the plane's offset, the larger one)
            for (tex, tint, dy, big) in [(grid, 1.0f32, 0.0f32, false), (grid3, 0.5, 0.0, true), (grid2, 1.0, -32.0, true)] {
                let (v, depth) = hit(dy);
                if depth <= 1.0 || v.abs() > 1280.0 {
                    continue;
                }
                let u = sx * depth / EYE;
                if u.abs() > 1280.0 {
                    continue;
                }
                // Far away fades out (the shader's distance fade, ours).
                let near = ((v + 1280.0) / 1280.0).clamp(0.0, 1.0);
                let fade = near * near * (3.0 - 2.0 * near);
                // The picture's UV over its own 2560 units (the radial
                // terms), and over the units the tiles are sized by (ours,
                // matched to the real shot).
                let dn = (((u - GLOBE_U) / QUAD).powi(2) + (v / QUAD).powi(2)).sqrt() * std::f32::consts::SQRT_2;
                let q = [u / (TILE * 32.0) + 0.5, v / (TILE * 32.0) + 0.5];
                let col = if big { grid2_ps(q, t, tv, tex) } else { grid_ps(q, t, tv, tex, dn) };
                for k in 0..3 {
                    rgb[k] += col[k] * tint * fade;
                }
            }
            let a = rgb[0].max(rgb[1]).max(rgb[2]).min(1.0);
            let i = (py * pw + px) * 4;
            if a > 0.0 {
                for k in 0..3 {
                    out[i + k] = ((rgb[k] / a).min(1.0) * 255.0) as u8;
                }
                out[i + 3] = (a * 255.0) as u8;
            }
        }
    }
    out
}

/// The globe as an RGBA8 picture `n` x `n`: the map on a sphere seen from
/// above `tilt` degrees of latitude, `spin` degrees of longitude at its
/// middle, the mesh's dots over it, a light rim.
/// The picture holds the globe's disc plus a soft glow around its limb: the
/// disc spans 1/PAD of the picture's width (BO2's limb glows 30 px out).
pub(crate) const GLOBE_PAD: f32 = 1.11;

/// The glow's strength (0..1) at radius `r` (the disc's radius is 1).
fn glow_alpha(r: f32) -> f32 {
    let g = (1.0 - (r - 1.0) / 0.09).clamp(0.0, 1.0);
    g.powf(1.3) * 0.11
}

pub(crate) fn globe(n: usize, map: &Tex, mesh: Option<&Tex>, spin: f32, tilt: f32) -> Vec<u8> {
    let (ts, tc) = (tilt.to_radians().sin(), tilt.to_radians().cos());
    let mut out = vec![0u8; n * n * 4];
    let px_size = 2.0 * GLOBE_PAD / n as f32;
    for y in 0..n {
        let ny = GLOBE_PAD - (y as f32 + 0.5) * px_size;
        for x in 0..n {
            let nx = (x as f32 + 0.5) * px_size - GLOBE_PAD;
            let r2 = nx * nx + ny * ny;
            let r = r2.sqrt();
            if r > 1.0 + px_size {
                // The limb's glow (real: ~29 of 255 at the limb, a soft grey
                // falling away over ~6% of the radius).
                let a = glow_alpha(r);
                if a > 0.0 {
                    let i = (y * n + x) * 4;
                    out[i] = 255;
                    out[i + 1] = 247;
                    out[i + 2] = 235;
                    out[i + 3] = (a * 255.0) as u8;
                }
                continue;
            }
            let edge = ((1.0 + px_size - r) / (2.0 * px_size)).clamp(0.0, 1.0);
            let nz = (1.0 - r2.min(1.0)).sqrt();
            // Into the globe's frame: tilted toward us by `tilt`.
            let gy = ny * tc + nz * ts;
            let gz = -ny * ts + nz * tc;
            let lat = gy.clamp(-1.0, 1.0).asin();
            let lon = nx.atan2(gz) + spin.to_radians();
            let u = lon / std::f32::consts::TAU + 0.5;
            let v = 0.5 - lat / std::f32::consts::PI;
            let m = map.sample(u, v);
            let d = mesh.map_or([0.0; 4], |t| t.sample(u, v));
            // As the shader does: the land is the day map's own colour (its
            // alpha is the land mask; the coast lines are in its colour), the
            // sea shows the dotted mesh, and the heat map (orange) fills the
            // SEA (measured on the real shot: Black Sea, Red Sea, Caspian
            // and the oceans are orange-brown, the land dark).
            let land = m[3];
            // The mesh's dots fade toward the limb, where they alias.
            let dots = (nz * 4.0).min(1.0);
            // The orange is lit from the lower right: dim toward the
            // upper left (the real globe's unlit side).
            let lit = (0.4 + 0.9 * (0.3 * nx - 0.6 * ny + 0.75 * nz)).clamp(0.0, 1.0);
            // The shader's cell noise scales the heat by 0.72..2.0; the
            // mesh's dots stand in for the cells.
            let cell = 0.75 + 0.8 * (d[0] / 0.16).clamp(0.0, 1.0);
            const HEAT: [f32; 3] = [1.0, 0.62, 0.2];
            // The ring of light at the limb.
            let ring = ((0.32 - nz) / 0.32).clamp(0.0, 1.0);
            let ring = ring.powf(1.1) * 0.14;
            const WARM: [f32; 3] = [1.0, 0.97, 0.92];
            let mut rgb = [0.0f32; 3];
            for k in 0..3 {
                let sea = d[k] * dots * 0.9 + HEAT[k] * 0.09 * lit * cell;
                rgb[k] = (sea * (1.0 - land) + m[k] * land + ring * WARM[k]).min(1.0);
            }
            let i = (y * n + x) * 4;
            for k in 0..3 {
                out[i + k] = (rgb[k] * 255.0) as u8;
            }
            // The disc's edge blends into the glow (never below it).
            let ga = glow_alpha(r);
            let a = ga + (1.0 - ga) * edge;
            if a > 0.0 && edge < 1.0 {
                let gcol = [1.0, 247.0 / 255.0, 235.0 / 255.0];
                for k in 0..3 {
                    let c = (rgb[k] * edge + gcol[k] * ga * (1.0 - edge)) / (edge + ga * (1.0 - edge)).max(1e-4);
                    out[i + k] = (c.min(1.0) * 255.0) as u8;
                }
            }
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

/// An RGBA8 sRGB picture of `w` x `h` from `px`.
pub(crate) fn image(w: usize, h: usize, px: Vec<u8>) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let size = Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 };
    let mut img = Image::new(size, TextureDimension::D2, px, TextureFormat::Rgba8Unorm, bevy::asset::RenderAssetUsages::default());
    img.sampler = bevy::image::ImageSampler::linear();
    img
}

/// Blur an RGBA8 picture in place (three box passes each way, about a
/// Gaussian of `radius` pixels): BO2's front-end menus over another blur
/// what is under them (`setBlur`).
pub(crate) fn blur(px: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let mut buf = vec![0u8; px.len()];
    for _ in 0..3 {
        box_pass(px, &mut buf, w, h, radius, true);
        box_pass(&buf, px, w, h, radius, false);
    }
}

/// One box blur along rows (`horizontal`) or columns, `src` into `dst`.
fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let n = (2 * r + 1) as u32;
    for line in 0..lines {
        let mut sum = [0u32; 4];
        for i in 0..=r.min(len - 1) {
            for k in 0..4 {
                sum[k] += u32::from(src[at(line, i) + k]);
            }
        }
        for i in 0..len {
            let o = at(line, i);
            for k in 0..4 {
                dst[o + k] = (sum[k] / n) as u8;
            }
            if i + r + 1 < len {
                let a = at(line, i + r + 1);
                for k in 0..4 {
                    sum[k] += u32::from(src[a + k]);
                }
            }
            if i >= r {
                let b = at(line, i - r);
                for k in 0..4 {
                    sum[k] -= u32::from(src[b + k]);
                }
            }
        }
    }
}
