//! bo2zm: Black Ops II lighting a moving thing samples at run time (the
//! first-person gun and arms, effects): the map's light grid, the sky grid
//! volumes outside it, and the scale the props measured (the grid's
//! coefficient scale is not stored with it). Installed once per map load.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// What lights a point: the grid's light there (linear, before exposure,
/// times the measured scale), the primary light most of its corners carry
/// and that light's visibility (0..1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct T6PointLight {
    pub light: [f32; 3],
    pub primary: u8,
    pub visibility: f32,
}

pub struct T6LightSampler {
    pub grid: Option<asset_model::OwnedLightGrid>,
    /// Grid bounds in cells (x, y at 32 units, z at 64), for the in-grid
    /// test the props use.
    pub mins: [u16; 3],
    pub maxs: [u16; 3],
    /// Sky volumes: bounds, base light, primary light and visibility.
    pub sky: Vec<([f32; 3], [f32; 3], [f32; 3], u8, u8)>,
    pub scale: f32,
    pub sun_primary: u8,
    /// bo2zm M3 fix list 1: the map's collision, for the grid's sight
    /// checks: a grid point marked "needs trace" counts only when it can be
    /// seen from the lit point (BO2's R_LightGridLookup); without them a
    /// zombie near the ground took the light of the points under it (black).
    pub clip: Option<Arc<crate::ClipCollision>>,
    /// Sight-checked lights by 8-unit cell (the traces cost; a zombie's
    /// body and head, the gun and effects read the same cells frame after
    /// frame).
    pub cache: Mutex<HashMap<[i32; 3], T6PointLight>>,
}

fn slot() -> &'static RwLock<Option<Arc<T6LightSampler>>> {
    static SLOT: RwLock<Option<Arc<T6LightSampler>>> = RwLock::new(None);
    &SLOT
}

pub fn install_t6_light_sampler(sampler: Option<T6LightSampler>) {
    if let Ok(mut s) = slot().write() {
        *s = sampler.map(Arc::new);
    }
}

pub fn t6_light_sampler() -> Option<Arc<T6LightSampler>> {
    slot().read().ok()?.clone()
}

impl T6LightSampler {
    fn in_grid(&self, p: [f32; 3]) -> bool {
        let cell = [
            (p[0].floor() as i32).wrapping_add(0x20000) >> 5,
            (p[1].floor() as i32).wrapping_add(0x20000) >> 5,
            (p[2].floor() as i32).wrapping_add(0x20000) >> 6,
        ];
        (0..3).all(|a| cell[a] >= i32::from(self.mins[a]) && cell[a] <= i32::from(self.maxs[a]))
    }

    /// The light at `pos` (with sight checks: once per 8-unit cell, read at
    /// its centre).
    pub fn at(&self, pos: [f32; 3]) -> T6PointLight {
        if self.clip.is_none() {
            return self.sample(pos);
        }
        let key = pos.map(|v| (v / 8.0).floor() as i32);
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(&key)
        {
            return *hit;
        }
        let centre = key.map(|k| k as f32 * 8.0 + 4.0);
        let lit = self.sample(centre);
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() > 16_384 {
                cache.clear();
            }
            cache.insert(key, lit);
        }
        lit
    }

    fn sample(&self, pos: [f32; 3]) -> T6PointLight {
        if !self.in_grid(pos) {
            if let Some(v) = self
                .sky
                .iter()
                .find(|v| (0..3).all(|a| pos[a] >= v.0[a] && pos[a] <= v.1[a]))
            {
                return T6PointLight {
                    light: v.2,
                    primary: v.3,
                    visibility: f32::from(v.4) / 255.0,
                };
            }
            return T6PointLight::default();
        }
        let Some(grid) = self.grid.as_ref() else {
            return T6PointLight::default();
        };
        let sampled = match self.clip.as_deref() {
            Some(clip) => {
                let clear = |a: [f32; 3], b: [f32; 3]| {
                    clip.box_sight_clear(a, b, lighting_iw4::LIGHT_GRID_SIGHT_CONTENT_MASK)
                };
                asset_model::sample_light_grid_with_sight(&grid.view(), pos, Some(&clear))
            }
            None => asset_model::sample_light_grid(&grid.view(), pos),
        }
        .ok();
        let light = sampled
            .as_ref()
            .map(|s| crate::t6_sampled_light(s).map(|c| c * self.scale))
            .unwrap_or_default();
        // The primary light most of the surrounding entries carry.
        let primary = sampled
            .as_ref()
            .and_then(|s| {
                let mut best: Option<(u8, f32)> = None;
                let mut totals: Vec<(u8, f32)> = Vec::new();
                for i in 0..8 {
                    if let Some(p) = s.corner_primaries[i] {
                        match totals.iter_mut().find(|t| t.0 == p) {
                            Some(t) => t.1 += s.weights[i],
                            None => totals.push((p, s.weights[i])),
                        }
                    }
                }
                for t in totals {
                    if best.is_none_or(|b| t.1 > b.1) {
                        best = Some(t);
                    }
                }
                best.map(|b| b.0)
            })
            .unwrap_or(self.sun_primary);
        let visibility = sampled
            .as_ref()
            .and_then(|s| crate::t6_sampled_visibility(s, primary))
            .map_or(0.0, |v| f32::from(v) / 255.0);
        T6PointLight {
            light,
            primary,
            visibility,
        }
    }
}

/// bo2zm fix list 2: the map's lightmaps on the CPU, for what is lit like
/// the world but drawn later: a decal on a wall (BO2's `wc_` mark
/// materials) takes the wall's own lightmap, not the light grid in front of
/// it. Each image holds three pages stacked (base light, directional light,
/// direction + the primary light's baked visibility), as the world shader
/// reads them.
pub struct T6LightmapPage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

fn lightmap_slot() -> &'static RwLock<Vec<Option<Arc<T6LightmapPage>>>> {
    static SLOT: RwLock<Vec<Option<Arc<T6LightmapPage>>>> = RwLock::new(Vec::new());
    &SLOT
}

pub fn install_t6_lightmaps(pages: Vec<Option<T6LightmapPage>>) {
    if let Ok(mut s) = lightmap_slot().write() {
        *s = pages.into_iter().map(|p| p.map(Arc::new)).collect();
    }
}

impl T6LightmapPage {
    /// One texel channel set, filtered as the GPU's linear sampler with
    /// clamped edges does.
    fn bilinear(&self, u: f32, v: f32) -> [f32; 4] {
        let (w, h) = (self.width as i64, self.height as i64);
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let texel = |xi: i64, yi: i64| -> [f32; 4] {
            let xi = xi.clamp(0, w - 1) as usize;
            let yi = yi.clamp(0, h - 1) as usize;
            let at = (yi * self.width as usize + xi) * 4;
            self.rgba
                .get(at..at + 4)
                .map_or([0.0; 4], |p| [0, 1, 2, 3].map(|c| f32::from(p[c]) / 255.0))
        };
        let (xi, yi) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (texel(xi, yi), texel(xi + 1, yi), texel(xi, yi + 1), texel(xi + 1, yi + 1));
        [0, 1, 2, 3].map(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// The world shader's lightmap light at lightmap `index`, coordinate `uv`
/// (one page's u, v) for normal `n`: base + directional * saturate(N.dir),
/// and the baked visibility of the surface's primary light. `None` when the
/// map has no such lightmap.
pub fn t6_lightmap_light(index: u8, uv: [f32; 2], n: [f32; 3]) -> Option<([f32; 3], f32)> {
    let (base, directional, ndl, vis) = t6_lightmap_parts(index, uv, n)?;
    let light = [0, 1, 2].map(|c| base[c] + directional[c] * ndl);
    Some((light, vis))
}

/// The lightmap's parts at a point: base light, directional light, the
/// directional term's N.dir, and the primary light's baked visibility.
pub fn t6_lightmap_parts(
    index: u8,
    uv: [f32; 2],
    n: [f32; 3],
) -> Option<([f32; 3], [f32; 3], f32, f32)> {
    let pages = lightmap_slot().read().ok()?;
    let page = pages.get(usize::from(index))?.as_ref()?;
    let v = uv[1] / 3.0;
    let p0 = page.bilinear(uv[0], v);
    let p1 = page.bilinear(uv[0], v + 1.0 / 3.0);
    let p2 = page.bilinear(uv[0], v + 2.0 / 3.0);
    let dir = [p2[0] * 2.0 - 1.0, p2[1] * 2.0 - 1.0, p2[2] * 2.0 - 1.0];
    let ndl = (dir[0] * n[0] + dir[1] * n[1] + dir[2] * n[2]).clamp(0.0, 1.0);
    let base = [0, 1, 2].map(|c| p0[c] / (p0[3] + 0.000001));
    let directional = [0, 1, 2].map(|c| p1[c] / (p1[3] + 0.000001));
    Some((base, directional, ndl, p2[3]))
}

/// bo2zm: the world's reflection probe origins, for what moves (the
/// first-person gun reflects the probe nearest the eye).
fn probe_slot() -> &'static std::sync::RwLock<Vec<[f32; 3]>> {
    static SLOT: std::sync::OnceLock<std::sync::RwLock<Vec<[f32; 3]>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(Default::default)
}

pub fn install_t6_probe_origins(origins: Vec<[f32; 3]>) {
    if let Ok(mut s) = probe_slot().write() {
        *s = origins;
    }
}

/// The reflection probe nearest `p` (probe 0 is a stand-in and never picked
/// while another exists).
pub fn t6_nearest_probe(p: [f32; 3]) -> Option<u32> {
    let origins = probe_slot().read().ok()?;
    origins
        .iter()
        .enumerate()
        .skip(usize::from(origins.len() > 1))
        .min_by(|(_, a), (_, b)| {
            let d = |o: &[f32; 3]| (0..3).map(|i| (o[i] - p[i]) * (o[i] - p[i])).sum::<f32>();
            d(a).total_cmp(&d(b))
        })
        .map(|(i, _)| i as u32)
}
