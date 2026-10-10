//! bo2zm: Black Ops II assets for IW4L. Takes what a T6 zone walk loads and
//! what the image packs hold, and turns it into data the engine uses.
//! Not part of upstream IW4L.

pub mod capture;
pub mod entities;
pub mod gsc;
pub mod images;
pub mod m2;

pub use capture::{
    AabbTreeRef, AssetKey, ClipAabbRef, ClipBrushRef, ClipCmodelRef, ClipLeafBrushNodeRef, ClipLeafRef,
    ClipMaterialRef, ClipNodeRef, ClipRef, ClipStaticModelRef, DpvsRanges, DrawState, EmbeddedImage, ImageRef, LightGridRef, MaterialRef,
    MaterialTexture, PassRef, PathNodeRef, PrimaryLightRef, ReflectionProbeRef, ShaderArgRef, SkyGridVolumeRef, StaticModel, StringTableRef, SunRef, TechniqueRef, TechniqueSetRef,
    DestructiblePieceRef, DestructibleRef, DestructibleStageRef, VehicleDrive, VehicleRef,
    WorldFogRef, WorldRef, WorldSurface, XAnimRef, XModelCollSurfRef, XModelRef, XSurfaceRef, ZoneCapture, read_image,
};
pub use entities::{ArtFog, MapEntity, parse_art_fog, parse_entities};
pub use gsc::{GscObject, GscRun, GscValue, MapScriptFacts, PlacedFx};
pub use images::{ImageSource, Pack, PackSet};
pub use m2::{
    AttachmentRef, AttachmentUniqueRef,
    FootstepFxTableRef, FootstepTableRef, FxEffectRef, FxElemRef, FxTrailRef, FxVisualRef, ImpactTableRef,
    SndAliasListRef, SndAliasRef, SndBankRef, SndCurveRef, SndDriverGlobalsRef, TracerRef,
    FontRef, GlyphRef, WeaponCamoMaterialRef, WeaponCamoRef, WeaponCamoSetRef, WeaponRef,
};

use std::path::Path;

use asset_transport::{T6ZoneMemory, ZoneGame, ZoneImage, open_zone};

/// Open, walk and capture one zone file.
pub fn capture_zone(path: &Path) -> Result<ZoneCapture, String> {
    let image = open_zone(path).map_err(|e| e.to_string())?;
    capture_image(path, &image)
}

/// `capture_zone`, keeping every pass's shader bytecode too (probes).
pub fn capture_zone_with_shaders(path: &Path) -> Result<ZoneCapture, String> {
    let image = open_zone(path).map_err(|e| e.to_string())?;
    capture_image_with(path, &image, true)
}

/// Walk and capture an already opened zone image.
pub fn capture_image(path: &Path, image: &ZoneImage) -> Result<ZoneCapture, String> {
    capture_image_with(path, image, false)
}

fn capture_image_with(
    path: &Path,
    image: &ZoneImage,
    keep_shaders: bool,
) -> Result<ZoneCapture, String> {
    if image.game != ZoneGame::T6 {
        return Err(format!("{}: not a Black Ops II zone", path.display()));
    }
    let header = image.t6_header().map_err(|e| e.to_string())?;
    let mut memory = T6ZoneMemory::for_header(&header);
    let mut stream = memory.stream(&image.bytes).map_err(|e| e.to_string())?;
    let table = fastfile_t6::open_asset_table(&mut stream).map_err(|e| e.to_string())?;
    let mut capture = ZoneCapture::default();
    // bo2zm M2: script strings, for bone names and notetracks.
    capture.strings = (0..table.strings.count())
        .map(|i| table.strings.get(&stream, i).unwrap_or("").to_owned())
        .collect();
    capture.keep_shaders = keep_shaders;
    capture.zone = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let result = {
        let mut walker = fastfile_t6::Walker::new(&mut stream, &mut capture);
        walker.walk_assets(&table).map_err(|e| {
            let trail: Vec<&str> = walker.trail().collect();
            format!(
                "{}: walk failed: {e} (in {})",
                path.display(),
                trail.join(" > ")
            )
        })
    };
    result?;
    if stream.cursor() != image.bytes.len() {
        return Err(format!(
            "{}: walk ended at {:#x} of {:#x}",
            path.display(),
            stream.cursor(),
            image.bytes.len()
        ));
    }
    Ok(capture)
}
