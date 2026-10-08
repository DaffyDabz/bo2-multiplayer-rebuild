//! The 60 T6 PC asset pools, in pool-id order. Names are the ones the
//! game's own asset tools print, so a census can be compared against them.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[repr(u32)]
pub enum AssetType {
    XModelPieces = 0x00,
    PhysPreset = 0x01,
    PhysConstraints = 0x02,
    DestructibleDef = 0x03,
    XAnimParts = 0x04,
    XModel = 0x05,
    Material = 0x06,
    TechniqueSet = 0x07,
    Image = 0x08,
    Sound = 0x09,
    SoundPatch = 0x0A,
    ClipMap = 0x0B,
    ClipMapPvs = 0x0C,
    ComWorld = 0x0D,
    GameWorldSp = 0x0E,
    GameWorldMp = 0x0F,
    MapEnts = 0x10,
    GfxWorld = 0x11,
    LightDef = 0x12,
    UiMap = 0x13,
    Font = 0x14,
    FontIcon = 0x15,
    MenuList = 0x16,
    Menu = 0x17,
    Localize = 0x18,
    Weapon = 0x19,
    WeaponDef = 0x1A,
    WeaponVariant = 0x1B,
    WeaponFull = 0x1C,
    Attachment = 0x1D,
    AttachmentUnique = 0x1E,
    WeaponCamo = 0x1F,
    SndDriverGlobals = 0x20,
    Fx = 0x21,
    ImpactFx = 0x22,
    AiType = 0x23,
    MpType = 0x24,
    MpBody = 0x25,
    MpHead = 0x26,
    Character = 0x27,
    XModelAlias = 0x28,
    RawFile = 0x29,
    StringTable = 0x2A,
    Leaderboard = 0x2B,
    XGlobals = 0x2C,
    Ddl = 0x2D,
    Glasses = 0x2E,
    EmblemSet = 0x2F,
    ScriptParseTree = 0x30,
    KeyValuePairs = 0x31,
    VehicleDef = 0x32,
    MemoryBlock = 0x33,
    AddonMapEnts = 0x34,
    Tracer = 0x35,
    SkinnedVerts = 0x36,
    Qdb = 0x37,
    Slug = 0x38,
    FootstepTable = 0x39,
    FootstepFxTable = 0x3A,
    ZBarrier = 0x3B,
}

pub const ASSET_TYPE_COUNT: usize = 0x3C;

impl AssetType {
    pub const ALL: [AssetType; ASSET_TYPE_COUNT] = {
        let mut all = [AssetType::XModelPieces; ASSET_TYPE_COUNT];
        let mut i = 0;
        while i < ASSET_TYPE_COUNT {
            all[i] = match AssetType::from_u32(i as u32) {
                Some(ty) => ty,
                None => panic!("asset pool ids are dense"),
            };
            i += 1;
        }
        all
    };

    pub const fn from_u32(v: u32) -> Option<AssetType> {
        use AssetType::*;
        Some(match v {
            0x00 => XModelPieces,
            0x01 => PhysPreset,
            0x02 => PhysConstraints,
            0x03 => DestructibleDef,
            0x04 => XAnimParts,
            0x05 => XModel,
            0x06 => Material,
            0x07 => TechniqueSet,
            0x08 => Image,
            0x09 => Sound,
            0x0A => SoundPatch,
            0x0B => ClipMap,
            0x0C => ClipMapPvs,
            0x0D => ComWorld,
            0x0E => GameWorldSp,
            0x0F => GameWorldMp,
            0x10 => MapEnts,
            0x11 => GfxWorld,
            0x12 => LightDef,
            0x13 => UiMap,
            0x14 => Font,
            0x15 => FontIcon,
            0x16 => MenuList,
            0x17 => Menu,
            0x18 => Localize,
            0x19 => Weapon,
            0x1A => WeaponDef,
            0x1B => WeaponVariant,
            0x1C => WeaponFull,
            0x1D => Attachment,
            0x1E => AttachmentUnique,
            0x1F => WeaponCamo,
            0x20 => SndDriverGlobals,
            0x21 => Fx,
            0x22 => ImpactFx,
            0x23 => AiType,
            0x24 => MpType,
            0x25 => MpBody,
            0x26 => MpHead,
            0x27 => Character,
            0x28 => XModelAlias,
            0x29 => RawFile,
            0x2A => StringTable,
            0x2B => Leaderboard,
            0x2C => XGlobals,
            0x2D => Ddl,
            0x2E => Glasses,
            0x2F => EmblemSet,
            0x30 => ScriptParseTree,
            0x31 => KeyValuePairs,
            0x32 => VehicleDef,
            0x33 => MemoryBlock,
            0x34 => AddonMapEnts,
            0x35 => Tracer,
            0x36 => SkinnedVerts,
            0x37 => Qdb,
            0x38 => Slug,
            0x39 => FootstepTable,
            0x3A => FootstepFxTable,
            0x3B => ZBarrier,
            _ => return None,
        })
    }

    pub const fn index(self) -> usize {
        self as u32 as usize
    }

    pub const fn name(self) -> &'static str {
        use AssetType::*;
        match self {
            XModelPieces => "xmodelpieces",
            PhysPreset => "physpreset",
            PhysConstraints => "physconstraints",
            DestructibleDef => "destructibledef",
            XAnimParts => "xanim",
            XModel => "xmodel",
            Material => "material",
            TechniqueSet => "techniqueset",
            Image => "image",
            Sound => "soundbank",
            SoundPatch => "soundpatch",
            ClipMap => "clipmap",
            ClipMapPvs => "clipmap_pvs",
            ComWorld => "comworld",
            GameWorldSp => "gameworldsp",
            GameWorldMp => "gameworldmp",
            MapEnts => "mapents",
            GfxWorld => "gfxworld",
            LightDef => "gfxlightdef",
            UiMap => "uimap",
            Font => "font",
            FontIcon => "fonticon",
            MenuList => "menulist",
            Menu => "menu",
            Localize => "localize",
            Weapon => "weapon",
            WeaponDef => "weapondef",
            WeaponVariant => "weaponvariant",
            WeaponFull => "weaponfull",
            Attachment => "attachment",
            AttachmentUnique => "attachmentunique",
            WeaponCamo => "weaponcamo",
            SndDriverGlobals => "snddriverglobals",
            Fx => "fx",
            ImpactFx => "fximpacttable",
            AiType => "aitype",
            MpType => "mptype",
            MpBody => "mpbody",
            MpHead => "mphead",
            Character => "character",
            XModelAlias => "xmodelalias",
            RawFile => "rawfile",
            StringTable => "stringtable",
            Leaderboard => "leaderboard",
            XGlobals => "xglobals",
            Ddl => "ddl",
            Glasses => "glasses",
            EmblemSet => "emblemset",
            ScriptParseTree => "script",
            KeyValuePairs => "keyvaluepairs",
            VehicleDef => "vehicle",
            MemoryBlock => "memoryblock",
            AddonMapEnts => "addonmapents",
            Tracer => "tracer",
            SkinnedVerts => "skinnedverts",
            Qdb => "qdb",
            Slug => "slug",
            FootstepTable => "footsteptable",
            FootstepFxTable => "footstepfxtable",
            ZBarrier => "zbarrier",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_stop_at_the_pool_count() {
        for (i, ty) in AssetType::ALL.iter().enumerate() {
            assert_eq!(ty.index(), i);
            assert_eq!(AssetType::from_u32(i as u32), Some(*ty));
        }
        assert_eq!(AssetType::from_u32(ASSET_TYPE_COUNT as u32), None);
        assert_eq!(AssetType::ZBarrier.index(), 0x3B);
        assert_eq!(AssetType::ScriptParseTree.index(), 0x30);
    }
}
