use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use asset_iw4::size as sz;
use fastfile_iw4::{
    AssetLinkSink, AssetType, Ptr, Result, ScriptStrings, XAnimPartsGeometry, ZoneStream,
};

use crate::asset_graph::ZoneOwner;
use crate::asset_key::AssetNamespace;
use crate::xanim_clip::{AnimClip, ClipNotify, RawDeltaTrans, RawXAnimParts};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct XAnimKey {
    pub namespace: AssetNamespace,
    pub name: String,
}

impl XAnimKey {
    pub fn new(namespace: AssetNamespace, name: &str) -> Self {
        Self {
            namespace,
            name: ascii_lower(name),
        }
    }

    pub fn display(&self) -> String {
        format!("{}:anim/{}", self.namespace.as_str(), self.name)
    }
}

#[derive(Clone, Debug)]
pub struct CapturedXAnim {
    pub namespace: AssetNamespace,
    pub parts: Arc<RawXAnimParts>,
    /// Black Ops II's root motion, kept aside: its clips decode without it
    /// unless asked through [`XAnimCatalog::clip_with_root_motion`].
    pub root_motion: Option<Arc<RawDeltaTrans>>,
}

impl CapturedXAnim {
    pub fn key(&self) -> XAnimKey {
        XAnimKey::new(self.namespace, &self.parts.name)
    }
}

#[derive(Debug)]
pub struct XAnimCatalog {
    entries: Vec<CapturedXAnim>,

    indices: HashMap<XAnimKey, usize>,

    order: Vec<XAnimKey>,

    zones: Vec<ZoneOwner>,

    decoded: Mutex<Vec<Option<Arc<AnimClip>>>>,
}

#[derive(Clone, Debug)]
pub struct XAnimBuild {
    catalog: XAnimCatalog,
    capture_zone: ZoneOwner,
    capture_ns: AssetNamespace,
    pub capture_gaps: usize,
    strings: ScriptStrings,
}

impl Default for XAnimCatalog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            indices: HashMap::new(),
            order: Vec::new(),
            zones: Vec::new(),
            decoded: Mutex::new(Vec::new()),
        }
    }
}

impl Default for XAnimBuild {
    fn default() -> Self {
        Self {
            catalog: XAnimCatalog::default(),
            capture_zone: ZoneOwner::default(),
            capture_ns: AssetNamespace::Iw4,
            capture_gaps: 0,
            strings: ScriptStrings::default(),
        }
    }
}

impl Clone for XAnimCatalog {
    fn clone(&self) -> Self {
        let decoded = self
            .decoded
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        Self {
            entries: self.entries.clone(),
            indices: self.indices.clone(),
            order: self.order.clone(),
            zones: self.zones.clone(),
            decoded: Mutex::new(decoded),
        }
    }
}

impl Deref for XAnimBuild {
    type Target = XAnimCatalog;

    fn deref(&self) -> &Self::Target {
        &self.catalog
    }
}

impl XAnimCatalog {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn get(&self, ns: AssetNamespace, name: &str) -> Option<&CapturedXAnim> {
        self.entries.get(self.index_by_name(ns, name)?)
    }

    pub fn hint_edge(
        &self,
        hint: Option<&str>,
        ns: AssetNamespace,
    ) -> asset_core::AssetEdge<asset_core::XAnimSpace> {
        match hint.filter(|name| !name.is_empty()) {
            None => asset_core::AssetEdge::Absent,
            Some(name) => match self.index_by_name(ns, name) {
                Some(index) => asset_core::AssetEdge::bind_order(index, self.zone_of(index)),
                None => asset_core::AssetEdge::Unresolved(asset_core::AssetEdgeReason::CatalogMiss),
            },
        }
    }

    fn has_key(&self, key: &XAnimKey) -> bool {
        self.indices.contains_key(key)
    }

    pub fn index_by_name(&self, ns: AssetNamespace, name: &str) -> Option<usize> {
        self.indices.get(&XAnimKey::new(ns, name)).copied()
    }

    pub fn zone_of(&self, index: usize) -> ZoneOwner {
        self.zones.get(index).copied().unwrap_or_default()
    }

    pub fn name_at(&self, index: usize) -> Option<&str> {
        self.order.get(index).map(|k| k.name.as_str())
    }

    pub fn clip_at(&self, index: usize) -> Option<Arc<AnimClip>> {
        let captured = self.entries.get(index)?;
        {
            let decoded = self
                .decoded
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(clip) = decoded.get(index).and_then(Option::as_ref) {
                return Some(Arc::clone(clip));
            }
        }
        let arc = Arc::new(AnimClip::from_parts(&captured.parts).ok()?);
        let mut decoded = self
            .decoded
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let slot = decoded.get_mut(index)?;
        Some(Arc::clone(slot.get_or_insert(arc)))
    }

    pub fn clip(&self, ns: AssetNamespace, name: &str) -> Option<Arc<AnimClip>> {
        self.clip_at(self.index_by_name(ns, name)?)
    }

    pub fn body_clip(
        &self,
        namespace: AssetNamespace,
        name: &str,
        body_bones: &[String],
    ) -> Option<Arc<AnimClip>> {
        if let Some(clip) = self.clip(namespace, name) {
            return Some(clip);
        }
        if namespace == AssetNamespace::Iw4 {
            return None;
        }
        let mut clip = (*self.clip(AssetNamespace::Iw4, name)?).clone();
        clip.tracks.retain(|track| body_bones.contains(&track.name));
        Some(Arc::new(clip))
    }

    /// The clip with its root motion, also where the catalog's own decode goes
    /// without it (Black Ops II's clips: a climb moves the player along it).
    pub fn clip_with_root_motion(&self, ns: AssetNamespace, name: &str) -> Option<AnimClip> {
        let captured = self.get(ns, name)?;
        match captured.root_motion.as_deref() {
            Some(root) if captured.parts.delta_trans.is_none() => {
                let mut parts = (*captured.parts).clone();
                parts.delta_trans = Some(root.clone());
                AnimClip::from_parts(&parts).ok()
            }
            _ => self.clip(ns, name).map(|clip| (*clip).clone()),
        }
    }

    pub fn decode(&self, ns: AssetNamespace, name: &str) -> Option<AnimClip> {
        let captured = self.get(ns, name)?;
        AnimClip::from_parts(&captured.parts).ok()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(|k| k.name.as_str())
    }

    pub fn namespace_count(&self, ns: AssetNamespace) -> usize {
        self.order.iter().filter(|k| k.namespace == ns).count()
    }

    pub fn collide_name_count(&self) -> usize {
        let mut seen: std::collections::HashMap<&str, u8> = std::collections::HashMap::new();
        for k in &self.order {
            *seen.entry(k.name.as_str()).or_insert(0) |= match k.namespace {
                AssetNamespace::Iw4 => 1,
                AssetNamespace::T5 => 2,
                AssetNamespace::Iw5 => 4,
                AssetNamespace::T6 => 8, // bo2zm
            };
        }
        seen.values().filter(|bits| bits.count_ones() >= 2).count()
    }

    pub fn peer(&self, ns: AssetNamespace, name: &str) -> Option<&CapturedXAnim> {
        const PREFER: [AssetNamespace; 3] =
            [AssetNamespace::T5, AssetNamespace::Iw5, AssetNamespace::Iw4];
        for other in PREFER {
            if other == ns {
                continue;
            }
            if let Some(captured) = self.get(other, name) {
                return Some(captured);
            }
        }
        None
    }
}

impl XAnimBuild {
    pub fn publish(self) -> XAnimCatalog {
        self.catalog
    }

    pub fn set_strings(&mut self, strings: ScriptStrings) {
        self.strings = strings;
    }

    pub fn set_capture_ns(&mut self, ns: AssetNamespace) {
        self.capture_ns = ns;
    }

    pub fn set_capture_zone(&mut self, zone: ZoneOwner) {
        self.capture_zone = zone;
    }

    pub fn insert_captured(&mut self, captured: CapturedXAnim) {
        self.insert_in(self.capture_ns, captured);
    }

    pub fn insert_in(&mut self, ns: AssetNamespace, mut captured: CapturedXAnim) {
        captured.namespace = ns;
        let key = captured.key();
        self.retain(key, captured);
    }

    fn retain(&mut self, key: XAnimKey, captured: CapturedXAnim) {
        let mut decoded = self
            .catalog
            .decoded
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(&pos) = self.catalog.indices.get(&key) {
            self.catalog.zones[pos] = self.capture_zone;
            self.catalog.entries[pos] = captured;
            decoded[pos] = None;
        } else {
            self.catalog
                .indices
                .insert(key.clone(), self.catalog.entries.len());
            self.catalog.order.push(key.clone());
            self.catalog.zones.push(self.capture_zone);
            self.catalog.entries.push(captured);
            decoded.push(None);
        }
    }

    pub fn absorb(&mut self, mut local: Self) -> usize {
        self.capture_gaps = self.capture_gaps.saturating_add(local.capture_gaps);
        let saved_zone = self.capture_zone;
        let saved_ns = self.capture_ns;
        let mut added = 0;
        let order = std::mem::take(&mut local.catalog.order);
        let entries = std::mem::take(&mut local.catalog.entries);
        for (i, (key, captured)) in order.into_iter().zip(entries).enumerate() {
            let vacant = !self.catalog.has_key(&key);
            self.capture_zone = local
                .catalog
                .zones
                .get(i)
                .copied()
                .unwrap_or(local.capture_zone);
            self.capture_ns = key.namespace;
            self.retain(key, captured);
            if vacant {
                added += 1;
            }
        }
        self.capture_zone = saved_zone;
        self.capture_ns = saved_ns;
        added
    }

    pub fn absorb_local(&mut self, local: Self) {
        let _ = self.absorb(local);
    }

    pub fn capture_xanim_iw5(
        &mut self,
        s: &fastfile_iw5::ZoneStream<'_>,
        strings: &fastfile_iw5::ScriptStrings,
        geometry: fastfile_iw5::XAnimPartsGeometry,
    ) {
        let Some(name_ptr) = geometry.name else {
            self.capture_gaps += 1;
            return;
        };
        let Ok(name) = s.cstr(name_ptr) else {
            self.capture_gaps += 1;
            return;
        };
        if name.is_empty() {
            self.capture_gaps += 1;
            return;
        }

        let track_count = geometry.bone_count[9] as usize;
        let mut names = Vec::with_capacity(track_count);
        if let Some(arr) = geometry.names {
            for i in 0..track_count {
                let sid = s.u16_at(arr, i * 2).unwrap_or(0);
                names.push(strings.get(s, sid).unwrap_or("").to_owned());
            }
        } else if track_count > 0 {
            self.capture_gaps += 1;
            return;
        }

        let mut notifies = Vec::with_capacity(geometry.notify_count);
        if let Some(arr) = geometry.notify {
            for i in 0..geometry.notify_count {
                let b = arr.at(i * fastfile_iw5::size::XANIM_NOTIFY_INFO);
                let sid = s.u16_at(b, 0).unwrap_or(0);
                let time = s.f32_at(b, 4).unwrap_or(0.0);
                notifies.push(ClipNotify {
                    name: strings.get(s, sid).unwrap_or("").to_owned(),
                    time,
                });
            }
        }

        let data_byte = copy_u8_iw5(s, geometry.data_byte, geometry.data_byte_count);
        let data_short = copy_u16_iw5(s, geometry.data_short, geometry.data_short_count);
        let data_int = copy_u32_iw5(s, geometry.data_int, geometry.data_int_count);
        let random_data_short = copy_u16_iw5(
            s,
            geometry.random_data_short,
            geometry.random_data_short_count,
        );
        let random_data_byte = copy_u8_iw5(
            s,
            geometry.random_data_byte,
            geometry.random_data_byte_count,
        );
        let random_data_int =
            copy_u32_iw5(s, geometry.random_data_int, geometry.random_data_int_count);
        let indices = if geometry.indices_are_bytes {
            copy_u8_iw5(s, geometry.indices, geometry.index_count)
                .into_iter()
                .map(u16::from)
                .collect()
        } else {
            copy_u16_iw5(s, geometry.indices, geometry.index_count)
        };

        self.insert_in(
            AssetNamespace::Iw5,
            CapturedXAnim {
                namespace: AssetNamespace::Iw5,
                root_motion: None,
                parts: Arc::new(RawXAnimParts {
                    name: name.to_owned(),
                    data_byte,
                    data_short,
                    data_int,
                    random_data_byte,
                    random_data_short,
                    random_data_int,
                    numframes: geometry.numframes,
                    flags: geometry.flags,
                    bone_count: geometry.bone_count,
                    framerate: geometry.framerate,
                    names,
                    notifies,
                    indices,
                    delta_trans: None,
                }),
            },
        );
    }

    pub fn capture_xanim_t5(
        &mut self,
        s: &fastfile_t5::ZoneStream<'_>,
        strings: &fastfile_t5::ScriptStrings,
        geometry: fastfile_t5::XAnimPartsGeometry,
    ) {
        let Some(name_ptr) = geometry.name else {
            self.capture_gaps += 1;
            return;
        };
        let Ok(name) = s.cstr(name_ptr) else {
            self.capture_gaps += 1;
            return;
        };
        if name.is_empty() {
            self.capture_gaps += 1;
            return;
        }

        let track_count = geometry.bone_count[9] as usize;
        let mut names = Vec::with_capacity(track_count);
        if let Some(arr) = geometry.names {
            for i in 0..track_count {
                let sid = s.u16_at(arr, i * 2).unwrap_or(0);
                names.push(strings.get(s, sid).unwrap_or("").to_owned());
            }
        } else if track_count > 0 {
            self.capture_gaps += 1;
            return;
        }

        let mut notifies = Vec::with_capacity(geometry.notify_count);
        if let Some(arr) = geometry.notify {
            for i in 0..geometry.notify_count {
                let b = arr.at(i * fastfile_t5::size::XANIM_NOTIFY_INFO);
                let sid = s.u16_at(b, 0).unwrap_or(0);
                let time = s.f32_at(b, 4).unwrap_or(0.0);
                notifies.push(ClipNotify {
                    name: strings.get(s, sid).unwrap_or("").to_owned(),
                    time,
                });
            }
        }

        let data_byte = copy_u8_t5(s, geometry.data_byte, geometry.data_byte_count);
        let data_short = copy_u16_t5(s, geometry.data_short, geometry.data_short_count);
        let data_int = copy_u32_t5(s, geometry.data_int, geometry.data_int_count);
        let random_data_short = copy_u16_t5(
            s,
            geometry.random_data_short,
            geometry.random_data_short_count,
        );
        let random_data_byte = copy_u8_t5(
            s,
            geometry.random_data_byte,
            geometry.random_data_byte_count,
        );
        let random_data_int =
            copy_u32_t5(s, geometry.random_data_int, geometry.random_data_int_count);
        let indices = if geometry.indices_are_bytes {
            copy_u8_t5(s, geometry.indices, geometry.index_count)
                .into_iter()
                .map(u16::from)
                .collect()
        } else {
            copy_u16_t5(s, geometry.indices, geometry.index_count)
        };

        self.insert_in(
            AssetNamespace::T5,
            CapturedXAnim {
                namespace: AssetNamespace::T5,
                root_motion: None,
                parts: Arc::new(RawXAnimParts {
                    name: name.to_owned(),
                    data_byte,
                    data_short,
                    data_int,
                    random_data_byte,
                    random_data_short,
                    random_data_int,
                    numframes: geometry.numframes,
                    flags: geometry.flags,
                    bone_count: geometry.bone_count,
                    framerate: geometry.framerate,
                    names,
                    notifies,
                    indices,
                    delta_trans: None,
                }),
            },
        );
    }
}

impl XAnimBuild {
    /// bo2zm: a Black Ops II animation, captured whole by `asset_t6`. Its
    /// data streams use the same part types and packing as IW4 and Black
    /// Ops, so they go to the decoder as they are.
    pub fn insert_t6(&mut self, anim: &asset_t6::XAnimRef) -> bool {
        let name = anim.name.trim_start_matches(',');
        if name.is_empty() || name.len() != anim.name.len() {
            // A comma-named asset is a reference to another zone's.
            return false;
        }
        if anim.names.len() != usize::from(anim.bone_count[9]) {
            self.capture_gaps += 1;
            return false;
        }
        self.insert_in(
            AssetNamespace::T6,
            CapturedXAnim {
                namespace: AssetNamespace::T6,
                root_motion: if anim.delta {
                    t6_root_motion(&anim.delta_trans).map(Arc::new)
                } else {
                    None
                },
                parts: Arc::new(RawXAnimParts {
                    // T6 names are case-insensitive: keyed lower case.
                    name: name.to_ascii_lowercase(),
                    data_byte: anim.data_byte.clone(),
                    data_short: anim.data_short.clone(),
                    data_int: anim.data_int.clone(),
                    random_data_byte: anim.random_data_byte.clone(),
                    random_data_short: anim.random_data_short.clone(),
                    random_data_int: anim.random_data_int.clone(),
                    numframes: anim.numframes,
                    flags: u8::from(anim.looping) | (u8::from(anim.delta) << 1),
                    bone_count: anim.bone_count,
                    framerate: anim.framerate,
                    names: anim.names.clone(),
                    notifies: anim
                        .notifies
                        .iter()
                        .map(|(name, time)| ClipNotify {
                            name: name.clone(),
                            time: *time,
                        })
                        .collect(),
                    indices: anim.indices.clone(),
                    delta_trans: None,
                }),
            },
        );
        true
    }
}

impl AssetLinkSink for XAnimBuild {
    fn loaded(
        &mut self,
        _s: &ZoneStream<'_>,
        _ty: AssetType,
        _slot: Ptr,
        _insert_slot: Option<Ptr>,
    ) -> Result<()> {
        Ok(())
    }

    fn alias(&mut self, _ty: AssetType, _slot: Ptr, _target: Ptr) -> Result<()> {
        Ok(())
    }

    fn capture_xanim(&mut self, s: &ZoneStream<'_>, geometry: XAnimPartsGeometry) -> Result<()> {
        let Some(name_ptr) = geometry.name else {
            self.capture_gaps += 1;
            return Ok(());
        };
        let Ok(name) = s.cstr(name_ptr) else {
            self.capture_gaps += 1;
            return Ok(());
        };
        if name.is_empty() {
            self.capture_gaps += 1;
            return Ok(());
        }

        let track_count = geometry.bone_count[9] as usize;
        let mut names = Vec::with_capacity(track_count);
        if let Some(arr) = geometry.names {
            for i in 0..track_count {
                let sid = s.u16_at(arr, i * 2).unwrap_or(0);
                names.push(self.strings.get(s, sid).unwrap_or("").to_owned());
            }
        } else if track_count > 0 {
            self.capture_gaps += 1;
            return Ok(());
        }

        let mut notifies = Vec::with_capacity(geometry.notify_count);
        if let Some(arr) = geometry.notify {
            for i in 0..geometry.notify_count {
                let b = arr.at(i * sz::XANIM_NOTIFY_INFO);
                let sid = s.u16_at(b, 0).unwrap_or(0);
                let time = s.f32_at(b, 4).unwrap_or(0.0);
                notifies.push(ClipNotify {
                    name: self.strings.get(s, sid).unwrap_or("").to_owned(),
                    time,
                });
            }
        }

        let data_byte = copy_u8(s, geometry.data_byte, geometry.data_byte_count);
        let data_short = copy_u16(s, geometry.data_short, geometry.data_short_count);
        let data_int = copy_u32(s, geometry.data_int, geometry.data_int_count);
        let random_data_short = copy_u16(
            s,
            geometry.random_data_short,
            geometry.random_data_short_count,
        );
        let random_data_byte = copy_u8(
            s,
            geometry.random_data_byte,
            geometry.random_data_byte_count,
        );
        let random_data_int = copy_u32(s, geometry.random_data_int, geometry.random_data_int_count);
        let indices = if geometry.indices_are_bytes {
            copy_u8(s, geometry.indices, geometry.index_count)
                .into_iter()
                .map(u16::from)
                .collect()
        } else {
            copy_u16(s, geometry.indices, geometry.index_count)
        };
        let delta_trans = copy_delta_trans(s, geometry.delta_trans);

        self.insert_captured(CapturedXAnim {
            namespace: AssetNamespace::Iw4,
            root_motion: None,
            parts: Arc::new(RawXAnimParts {
                name: name.to_owned(),
                data_byte,
                data_short,
                data_int,
                random_data_byte,
                random_data_short,
                random_data_int,
                numframes: geometry.numframes,
                flags: geometry.flags,
                bone_count: geometry.bone_count,
                framerate: geometry.framerate,
                names,
                notifies,
                indices,
                delta_trans,
            }),
        });
        let _ = geometry.frequency;
        Ok(())
    }
}

/// Black Ops II root keys (frame, position) packed the way the clip decoder
/// reads them: full 16-bit steps over the keys' own range.
fn t6_root_motion(keys: &[(u16, [f32; 3])]) -> Option<RawDeltaTrans> {
    let (&(_, first), rest) = keys.split_first()?;
    if rest.is_empty() {
        return Some(RawDeltaTrans {
            size: 0,
            small: false,
            mins: first,
            step: [0.0; 3],
            indices: Vec::new(),
            packed: Vec::new(),
        });
    }
    let mut mins = first;
    let mut maxs = first;
    for (_, v) in rest {
        for a in 0..3 {
            mins[a] = mins[a].min(v[a]);
            maxs[a] = maxs[a].max(v[a]);
        }
    }
    let step: [f32; 3] = core::array::from_fn(|a| (maxs[a] - mins[a]) / f32::from(u16::MAX));
    let mut packed = Vec::with_capacity(keys.len() * 6);
    for (_, v) in keys {
        for a in 0..3 {
            let q = if step[a] > 0.0 {
                ((v[a] - mins[a]) / step[a]).round() as u16
            } else {
                0
            };
            packed.extend_from_slice(&q.to_le_bytes());
        }
    }
    Some(RawDeltaTrans {
        size: u16::try_from(keys.len() - 1).ok()?,
        small: false,
        mins,
        step,
        indices: keys.iter().map(|&(frame, _)| frame).collect(),
        packed,
    })
}

fn ascii_lower(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn copy_u8(s: &ZoneStream<'_>, ptr: Option<Ptr>, count: usize) -> Vec<u8> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    s.slice_at(p, 0, count)
        .map(|b| b.to_vec())
        .unwrap_or_default()
}

fn copy_u16(s: &ZoneStream<'_>, ptr: Option<Ptr>, count: usize) -> Vec<u16> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    let Ok(bytes) = s.slice_at(p, 0, count * 2) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn copy_u32(s: &ZoneStream<'_>, ptr: Option<Ptr>, count: usize) -> Vec<u32> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    let Ok(bytes) = s.slice_at(p, 0, count * 4) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn copy_f32_3(s: &ZoneStream<'_>, ptr: Ptr, off: usize) -> Option<[f32; 3]> {
    let bytes = s.slice_at(ptr, off, 12).ok()?;
    Some([
        f32::from_le_bytes(bytes[0..4].try_into().ok()?),
        f32::from_le_bytes(bytes[4..8].try_into().ok()?),
        f32::from_le_bytes(bytes[8..12].try_into().ok()?),
    ])
}

fn copy_delta_trans(
    s: &ZoneStream<'_>,
    geo: fastfile_iw4::XAnimDeltaTransGeometry,
) -> Option<RawDeltaTrans> {
    if let Some(constant) = geo.constant {
        let mins = copy_f32_3(s, constant, 0)?;
        return Some(RawDeltaTrans {
            size: 0,
            small: geo.small != 0,
            mins,
            step: [0.0; 3],
            indices: Vec::new(),
            packed: Vec::new(),
        });
    }
    let mins_step = geo.mins_step?;
    let mins = copy_f32_3(s, mins_step, 0)?;
    let step = copy_f32_3(s, mins_step, 12)?;
    let n = geo.size as usize + 1;
    let indices = if geo.indices_are_bytes {
        copy_u8(s, geo.indices, n)
            .into_iter()
            .map(u16::from)
            .collect()
    } else {
        copy_u16(s, geo.indices, n)
    };
    let packed_n = if geo.small != 0 { 3 * n } else { 6 * n };
    let packed = copy_u8(s, geo.frames, packed_n);
    Some(RawDeltaTrans {
        size: geo.size,
        small: geo.small != 0,
        mins,
        step,
        indices,
        packed,
    })
}

fn copy_u8_iw5(
    s: &fastfile_iw5::ZoneStream<'_>,
    ptr: Option<fastfile_iw5::Ptr>,
    count: usize,
) -> Vec<u8> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    s.slice_at(p, 0, count)
        .map(|b| b.to_vec())
        .unwrap_or_default()
}

fn copy_u16_iw5(
    s: &fastfile_iw5::ZoneStream<'_>,
    ptr: Option<fastfile_iw5::Ptr>,
    count: usize,
) -> Vec<u16> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    let Ok(bytes) = s.slice_at(p, 0, count * 2) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn copy_u32_iw5(
    s: &fastfile_iw5::ZoneStream<'_>,
    ptr: Option<fastfile_iw5::Ptr>,
    count: usize,
) -> Vec<u32> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    let Ok(bytes) = s.slice_at(p, 0, count * 4) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn copy_u8_t5(
    s: &fastfile_t5::ZoneStream<'_>,
    ptr: Option<fastfile_t5::Ptr>,
    count: usize,
) -> Vec<u8> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    s.slice_at(p, 0, count)
        .map(|b| b.to_vec())
        .unwrap_or_default()
}

fn copy_u16_t5(
    s: &fastfile_t5::ZoneStream<'_>,
    ptr: Option<fastfile_t5::Ptr>,
    count: usize,
) -> Vec<u16> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    let Ok(bytes) = s.slice_at(p, 0, count * 2) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn copy_u32_t5(
    s: &fastfile_t5::ZoneStream<'_>,
    ptr: Option<fastfile_t5::Ptr>,
    count: usize,
) -> Vec<u32> {
    let Some(p) = ptr else {
        return Vec::new();
    };
    let Ok(bytes) = s.slice_at(p, 0, count * 4) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bo2_root_motion_is_kept_aside_for_who_asks() {
        let keys = [(0, [0.0, 0.0, 0.0]), (13, [8.0, 0.0, 30.0]), (26, [16.0, 0.0, 57.0])];
        let mut build = XAnimBuild::default();
        build.insert_in(
            AssetNamespace::T6,
            CapturedXAnim {
                namespace: AssetNamespace::T6,
                parts: Arc::new(RawXAnimParts {
                    name: "mp_mantle_up_57".to_owned(),
                    numframes: 26,
                    framerate: 30.0,
                    flags: 2,
                    ..RawXAnimParts::default()
                }),
                root_motion: t6_root_motion(&keys).map(Arc::new),
            },
        );
        let catalog = build.publish();
        let plain = catalog.clip(AssetNamespace::T6, "mp_mantle_up_57").unwrap();
        assert!(!plain.has_delta());
        let clip = catalog
            .clip_with_root_motion(AssetNamespace::T6, "mp_mantle_up_57")
            .unwrap();
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 0.01);
        assert!(close(clip.abs_delta_trans(0.5), [8.0, 0.0, 30.0]));
        assert!(close(clip.abs_delta_trans(1.0), [16.0, 0.0, 57.0]));
    }

    #[test]
    #[ignore = "reads Black Ops II's common_zm.ff named by T6_COMMON_ZM"]
    fn bo2_climbs_carry_their_root_motion() {
        let path = std::env::var("T6_COMMON_ZM").expect("T6_COMMON_ZM");
        let capture = asset_t6::capture_zone(std::path::Path::new(&path)).expect("zone");
        let mut build = XAnimBuild::default();
        for anim in &capture.xanims {
            build.insert_t6(anim);
        }
        let catalog = build.publish();
        for name in [
            "mp_mantle_up_57",
            "mp_mantle_up_51",
            "mp_mantle_up_45",
            "mp_mantle_up_39",
            "mp_mantle_up_33",
            "mp_mantle_up_27",
            "mp_mantle_up_21",
            "mp_mantle_over_high",
            "mp_mantle_over_mid",
            "player_mantle_over_low",
        ] {
            let clip = catalog
                .clip_with_root_motion(AssetNamespace::T6, name)
                .expect(name);
            let end = clip.abs_delta_trans(1.0);
            println!("{name}: {} ms, ends {end:?}", clip.length_msec());
            assert!(clip.has_delta(), "{name} has no root motion");
            if let Some(height) = name.strip_prefix("mp_mantle_up_") {
                let height: f32 = height.parse().unwrap();
                assert!((end[2] - height).abs() < 1.0, "{name} ends {end:?}");
            } else {
                assert!(end[0] > 20.0, "{name} ends {end:?}");
            }
        }
    }
}
