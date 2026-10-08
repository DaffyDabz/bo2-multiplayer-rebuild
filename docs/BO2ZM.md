# bo2zm: Black Ops II on IW4L

A local fork of IW4L (upstream `11514ea`, 2026-09-30) that reads Call of Duty:
Black Ops II (T6) PC files. Goal: play BO2 Zombies, starting with Nuketown,
from an owned BO2 install, in this engine.

## Build (native Windows, MSVC)

Builds go to `target/` by default; set `CARGO_TARGET_DIR` to put them on a roomy
drive (a full build is tens of GB). The game binary's `iw4l-artifacts/` (logs,
caches, settings) lands next to it.

```
cargo build -p launcher --profile play      # <target-dir>\play\iw4l.exe, ~12 min cold
cargo build -p bo2zm_tools --profile play   # <target-dir>\play\t6zone.exe
cargo test -p fastfile_t6
```

Upstream cross-compiles Windows builds from Linux (`cargo xwin`); the native
MSVC build above works unchanged.

## Read BO2 zones

```
t6zone [--walk] --nuketown ["C:\Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops II"]
t6zone [--walk] path\to\zone.ff ...
```

Per zone: envelope, XChunk record count, inflated image against the XFile
header's own size, block sizes, script strings, dependencies, asset count by
type. `--walk` then reads every asset in order and checks the result: the
stream must end exactly at the image's end, every block must fill to exactly
its declared size, and no pointer may name data not yet placed. On failure it
prints the asset and the structure path it was in. Game files are only read.

Measured 2026-09-30 on the Steam install: all 196 zone files (`zone/all` and
`zone/english`, every Zombies map) walk to the exact end with every block at
its declared size, in 49 s; the 9 Nuketown zones in 0.25 s.

## The zone walk is generated

`crates/fastfile_t6/src/walk_gen.rs` is written by
`crates/fastfile_t6/gen/t6gen.py`; do not edit it by hand.

```
python crates/fastfile_t6/gen/t6gen.py <OpenAssetTools checkout> crates/fastfile_t6/src/walk_gen.rs [<layout check .cpp>]
```

The generator reads two kinds of facts from an OpenAssetTools checkout on the
developer's machine: the T6 structure
declarations (`src/Common/Game/T6/T6_Assets.h`, enums from `T6.h`) and the
zone-load rules (`src/ZoneCode/Game/T6/T6_Commands.txt` and `XAssets/*.txt`:
counts, conditions, blocks, reuse, reorder, alignment overrides). It computes
32-bit MSVC layouts itself (`gen/cheader.py`) and emits one Rust load
function per structure; the runtime half is `src/walk.rs`. No OpenAssetTools
source is copied into the repository.

`gen/layoutcheck.py` (third argument above) writes a C++ file of
`static_assert`s pinning every computed size, alignment and member offset;
compiling it with the x86 MSVC compiler (`vcvarsall.bat x86`, `cl /c
/std:c++20`) against the same header proves the layout: 4,212 checks pass.

Rules the walk follows, all from the game's own loader as the zone-load rules
describe it: an asset header loads into TEMP, its members into VIRTUAL unless
a rule names another block; pointers are null, follow (-1), insert (-2, TEMP
contexts only, aliased through a VIRTUAL slot) or name earlier data; a
structure with a variable-length tail loads up to the tail first; union
members pick by condition, the one without a condition last; a member's
allocation aligns to its declared type (a typedef's alignment wins), a
struct-level `allocalign` only to that struct's own pointer loads; counts and
conditions read the most recently entered instance of the named structure.

## What exists

| piece | where |
| ----- | ----- |
| `TAff0100` v147 envelope, `PHEEBs71` auth header (signature kept, not verified) | `crates/fastfile_t6/src/envelope.rs` |
| XChunk records: 4 round-robin streams, 0x80000 read window, Salsa20 + SHA-1 IV chain | `crates/fastfile_t6/src/xchunk.rs` |
| Raw-deflate per record into one image | `crates/asset_transport/src/zone.rs` (`parse_t6_zone_image`) |
| XFile header, 8 blocks, pointer fixups | `crates/fastfile_t6/src/zone.rs` |
| XAssetList: script strings, dependencies, asset array; 60 asset types | `crates/fastfile_t6/src/content.rs`, `asset_type.rs` |
| Zone walk over every asset type (generated) + its runtime | `crates/fastfile_t6/src/walk_gen.rs`, `walk.rs` |
| `ZoneGame::T6` / `AssetNamespace::T6` (`t6:` prefix), zone discovery for version 147 | `asset_core`, `asset_transport/src/discover.rs` |
| T6 lane: envelope supported, everything else a typed gap with the zone's census | `crates/assets/src/lane/t6.rs` |

Since then (see the plan for the history): image packs, world, props,
collision, the BO2-only start (`map t6:zm_nuked`), the drawing below, and
M2's sound, weapons and effects (last section). Not yet: zombies and the
game's server scripts (M3), its HUD and menus (M4).

## How Black Ops II surfaces draw (fallback pass)

BO2 techniques are D3D11 shaders the engine cannot run, so
`render_gpu/src/drawsurf/geometry_diagnostic.{rs,wgsl}` draws every BO2
surface with formulas read from the game's own shaders (the `t6shader` probe
dumps a technique's DXBC; the Windows SDK's `fxc /dumpbin` disassembles it).

- Draw state per material from its main technique's `GfxStateBits`
  (`asset_core::T6Draw`): blend, alpha test, cull, polygon offset, sort key.
  Names alone misread materials (`cod7_tile_blend` rocks are opaque).
- Output: every BO2 shader ends `sqrt(colour * exposure)`; the colour target
  holds gamma-encoded values, highlights rolled off above 0.75. Then the
  final pass (`bo2_bloom.{rs,wgsl}`), as BO2's post shaders do: bloom (a
  high and a low level, tinted by the vision's `vc_RGBH/RGBL/YH/YL`) added
  in linear, the roll-off, then the map's colour table: band 0 of its
  `GfxWorld::lutMaterial` image graded by its vision file
  (`vision/<map>.vision`: shadow/midtone/highlight matrices by luminance
  ramps `vc_RS/RE`, `vc_FGM` power, `vc_FSM` saturation, `vc_FBM` blend),
  the `hdr_create_lut2dv` shader ported in `asset_world::t6_grade`.
- Sky: fogged by fog^2 * opacity (its pixel shader multiplies the fog by
  the vertex shader's fog times opacity).
- Water (`cod7water` pools, `cod7watershore` seas and rivers): its
  constants go in an RGBA16F picture in the colour map's place
  (`t6_materials::water_table`), its two normal maps in the normal and
  specular maps' places; the shader (`WATER`) scrolls four normal samples
  over world xy by game time, mixes the water colour and the reflection
  probe by fresnel and adds the sun's two highlight lobes. The game
  refracts the scene behind; here the water blends over it (no depth fade,
  no shore foam, the probe instead of a sea's own sky cube).
  Water writes depth (the sky, drawn last, must not cover an open sea).
  The open-sea variant (Carrier, Takeoff: no `waterOpacityControl`) is
  opaque and reflects the probe's sky as is; seas without a lightmap draw
  over the first page.
- Emissive flow (`cod7emissiveflow`, Magma's molten lava, world and props):
  `t6_materials::flow_images` packs its masks (emissiveMapHi.a,
  emissiveMapLo.a, noise g and a) into the specular map's place and its
  colour remap with a last row of constants into layer 1's (shine binding
  10); `bo2_flow` in the shader: heat = masks + noise * vertex blue, remap
  row = smoothstep of noise + vertex red, rock lit by the sun alone (Burley,
  GGX), glow = remap^2 * emissiveFilter + remap^2 * diffuseFilter * n.v; no
  lightmap (it draws over the first page), no fog. `IW4L_T6_NO_FLOW` turns
  it off for comparing.
- Emissive tile (`sw4_3d_cod7_emissive_tile`, Magma's lava rocks, props):
  the colour map tiled by Micro_Scale, the macro normal map, lit; plus the
  glow (emissive A + emissive B) * A's alpha * hdrAmount, squared, combined
  by `t6_materials::tile_images` into the specular map's place (B averaged
  over the texels one A texel covers); constants in layer 1's place. The
  micro normal map is not drawn.
- Tattered flags (`sw4_3d_phong_simple_flag_tatters`, Dig's tarps, props):
  colour = mix(frayed edge at uv * EdgeScale, DiffuseAndGloss, vertex
  red), alpha = mix(edge alpha, 1, saturate(2 * vertex red)), tested at
  128/255; the edge in the specular map's place, EdgeScale in layer 1's.
  `DiffuseAndGloss` (0xd50e4161) is a colour map wherever a material has
  no `colorMap`.
- Exposure: the first exposure volume's, else the world sun's own
  (`GfxWorld::sunParse.initWorldSun.exposure`: Cove 2.43, Plaza 4.65, Pod
  2.6, Uplink 2.7 have no volumes).
- Test aid: `IW4L_LOOK_CAMERA="x y z yaw pitch"` holds the world camera at
  a pose (above the sea, over a lava pool) whatever the player does.
- Foliage (`treecanopy`): the vertex colour is wind data (rgb) and a
  grid-light scale (alpha), never a tint.
- `sw4_3d_unlit` (screens, backlit glass): (texel * hdrAmount)^2.
- Prop grid light: the scale against baked vertex light is the median
  ratio, unless it is over five times the median of the half of props with
  the brightest grid samples (Cargo: 773 against 38; dark samples inside
  walls), then that.
- World light: lightmap base + directional, plus the surface's primary
  light (sun, spot or omni from `ComWorld`) times the lightmap's baked
  visibility (page 2 alpha). Props: light grid (or the sky grid volume
  outside it) plus their primary light times the grid's visibility.
- Layer kinds (bo2mp look 4, from BO2's `lit_sm_*_a1c1` / `_t1c1n1` pixel
  shaders): `aK` adds layer * layer alpha * weight before squaring; `t1`
  replaces the colour where layer alpha * weight >= 0.5 (style bit 2 with
  layer 1's kind blend; `t2`/`t3` draw as blend). Unparsed, these drew the
  vertex colour (layer weights, 255/0/255) as a tint: magenta trim on 27
  maps.
- Materials with surface flag 0x80000 (`onlycastshadow`: `wpc/shadowcaster`,
  trees' shadow cards) never draw in colour.
- Layered materials (`*_b1c1*`, `*_m2c2*`): layer texcoords from `vd1`
  (half2, plus 4 bytes when the layer and the base both have normal maps);
  blend layers mix by layer alpha times vertex green / blue, multiply
  layers scale by 1 + weight * (layer - 1).
- Fog: the map's `initWorldFog` (`GfxWorld::sunParse`), applied as the
  vertex shaders do (height term, sun fog), the sky at 2e7 units.
- Only the static visibility list's surfaces draw (Nuketown's orange light
  dome sits outside it). Props hide beyond their own `cullDist`.

## Quality setting

`iw4l-artifacts/settings.cfg`: `bo2_quality=full` (default, as close to the
game as the fallback draws) or `bo2_quality=fast` (half-size textures,
single-layer ground, props hidden at 60% of their distance, 4x anisotropy).
Every 5 s the log has a `bo2zm perf:` line (fps, frame time, props drawn and
hidden).

## Dive to prone

`crates/movement_iw4/src/dive.rs`: sprint, then prone (or a pad's stance
button) launches a dive, a slide on landing, then prone until a stance key
or jump. Timing from BO2's dive animations (`pb_dive_prone` 0.733 s,
`pb_dive_prone_land` 0.767 s); the launch numbers are fitted to that.

## Sound, weapons and effects (M2)

Read from every zone the map loads (`assets/src/lane/t6_m2.rs`); the load
report ends with three census lines (`t6 census:`): missing weapons
(models, first person, magazines and scopes, animations), missing effects
and missing sounds of everything the weapons, their animations, the impact
table and the map's scripts name. All three read 0 for Nuketown.

- Sound: the install's `sound/*.sabl|sabs` banks (`sab_t6`), aliases by
  `SND_HashName`; footsteps and landings by surface from the map's
  footstep tables; ambience (room tone, looping and random point sounds)
  and placed effects from the map's compiled client scripts
  (`asset_t6::gsc`, a straight-line walk of `createfx` and `_amb`).
- Weapons: `WeaponVariantDef`/`WeaponDef` into the weapon catalog; first
  person = the player's arms (`c_zom_suit_viewhands`) + gun + its
  attachment view models (every gun's magazine, the snipers' scopes) at
  `attachViewModelOffsets` on the gun's root. A variant's `hideTags` drop
  only the hidden bones' geometry (the DSR-50 hides its iron sights).
  Knives root on `tag_knife_attach`. Tracers by name (the DSR-50's is a
  smoke trail that fades), projectiles drawn in flight.
- Effects: `FxEffectDef` into the effect catalog (colour bytes red first,
  T6 cloud particle counts); sprites, particle clouds (BO2's
  `particlecloud` vertex shader), tracer beams, effect models (shell
  casings) and marks all draw in the fallback pass. Additive sprites take
  no fog; blended ones fog by the effect keep factor; the engine's
  distance fade (`fadeInRange`/`fadeOutRange`) keeps muzzle smoke and
  explosion parts off the eye; `zfeather` materials fade near the camera
  and, in a pass of their own over a read-only depth, where they meet the
  scene. Effect omni lights (muzzle flashes, explosions) light lit
  surfaces as BO2's glights: colour * (1 - d / radius)^2 * N.L.
- Shine: normal and specular maps, gloss highlights and reflection probes
  (`GfxWorldDraw.reflectionProbes`, SH-scaled) on the world, props, guns
  and glass, from the `wpc_`/`mc_lit_sm_r0c0n0s0` disassembly. Bloom from
  `hdr_bloom_remap`/`hdr_bloom_apply`.

Gotchas measured on the way: a BO2 model's local bone offsets (`trans`)
are three packed floats per bone (reading four put every untouched tag,
`tag_flash` included, in the wrong place); a light element's brightness is
its colour (its scale is 0); a background window is capped at 60 fps by
the driver, so frame-rate tests must keep the game window in front.

The first playtest's fix list (10-01), what BO2 does that Modern Warfare 2
did not:

- Recoil moves the aim. A BO2 gun's view kick is the player's aim, not a
  camera-only offset: the kick's change each frame is queued on
  `net::LookState::kick_pending` and folded into the next command's
  angles, so bullets follow the sights and the kick's return to centre
  brings the aim back (`render_anim::occupancy::view_kick`). Kicks are at
  least `fHip/AdsViewKickMinMagnitude`. Spread is the data's own
  (`fAdsSpread` 0 for most guns, hip 3-6 degrees for the M1911, 6-12 for
  the HAMR); there is no crosshair until the HUD (M4).
- Throws: velocity = view forward * `iProjectileSpeed` + world up *
  `iProjectileSpeedUp` + view up * `iProjectileSpeedRelativeUp` (the frag:
  920, 0, 120) + the thrower's velocity * `fProjectileTakeParentVelocity`
  (the frag: 0). A level throw lands about 515 units out. Bounce sounds are
  `WeaponDef.bounceSound`, one alias per surface type.
- Knife: the player's swing alias `wpn_melee_whoosh_plr` ships silent (its
  one variant names no file); the world swing `wpn_melee_whoosh_npc` plays
  instead. Melee hit names are Black Ops' (`wpn_melee_hit_other`).
- Impacts: the table's 21 rows are Black Ops' (the campaign's table names
  them: row 2 underwater, 7/8 armour-piercing, 9/10 extreme). Zombies'
  armour-piercing row holds the 20 mm cannon hits (a fireball, a light,
  smoke to 175 units); the HAMR, LSAT, DSR-50 and SMR hit as large bullets
  instead, as the campaign's own `fx_ap_*` are sized.
- Tracers draw as the data says (the LMG's: a 2.5-unit-radius beam, 100
  long, 5000 units/s, its bright core a fifth of its width at the front):
  from first person a small streak for a frame or two.

The second fix list (10-01):

- Props are solid to bullets and thrown things. The clip map's
  `staticModelList` (1,349 placed models in Nuketown: cars, the bus,
  fences, trees, porches, rocks) joins the collision as IW4's static
  models do, each with its model's `collSurfs` (T6 stores their bounds as
  mins/maxs) and the placement's own contents; player movement keeps to
  the clip brushes around them. Leaves are foliage (contents 2) and let
  bullets through, trunks and pots stop them.
- Their bullet holes: a T6 mark against models clips every opaque first-LOD
  triangle of the props its sphere touches (`smodel_mark_cpu`, placed by
  the drawn transform) and draws the effect's `mc_` material, lit by the
  light grid as BO2's model marks are.
- A hole on a wall is lit by that wall's lightmap, as BO2's `wc_` mark
  shaders do: each mark corner samples the lightmap pages on the CPU
  (`t6_lightmap_light`: base + directional * N.dir, and the primary
  light's baked visibility) and carries the light in its vertex colour
  (the prop shader's baked-light form). The light grid in front of a wall
  can be far darker than the wall (the fieldstone wall by the start), which
  drew every hole's light plaster ring as a black blotch.
- The frag is a rolling grenade (`isRollingGrenade`): it rests and rolls on
  its body (1.37 units, the nearest face of its projectile model's bounds)
  instead of on its centre. Landing on a floor without bouncing back up
  more than 60 units/s it rolls: a straight-line trajectory each tick
  (`TR_LINEAR`, so no new snapshot field), snapped to the ground under it,
  sped down slopes by gravity, slowed by 150 units/s^2 (our choice: BO2's
  rolling resistance is in its exe), spinning at speed / radius; it stops
  below 4 units/s, or falls when it rolls off an edge.
- Hip fire aims each shot with the spread before that shot widens it
  (`spread_before_fire_add`): a spaced M1911 shot is 3 degrees, not the 6
  its fire add gave every shot; BO2's per-gun minimums only matter this way.
- A hip fire animation plays out: the trigger-up settle waits for the fire
  clip to end (`fire_plays_out`), as the aimed one always did; the Python's
  0.4 s recoil was cut to idle wherever the trigger-up arrived.
- The Python, M14 and Olympia use one world flash for both views
  (`fx_rifle1_flash_base`, `fx_shotgun_flash_base`; their Pack-a-Punched
  versions have first-person ones). Its flames grow from 3-6 to 23-48
  units in 70 ms at the barrel 25 units from the eye: one frame at the
  barrel, then the whole screen. Drawn as the files say until the real
  game is compared.

The third fix list (10-01):

- A gun that names one flash for both views gets a first-person copy of
  it (`<name>#1p`, made at load; the load report lists them): the Python,
  M14 and Olympia, and the Pack-a-Punched M14, FAL, SMR and Barrett. The
  files do not say how Black Ops II draws such a flash on the player's own
  gun: the "drawn with the gun" element flag (`0x1000`) is not what shows
  a part in first person (the game's own first-person muzzle smoke and the
  Ray Gun's first-person flash do without it), and the same kind of flash
  is the first-person flash of popular guns on other maps (Origins'
  MP40 and STG-44, Mob's AK-47 and Uzi). OUR CHOICE: in the copy every
  part but the lights and the parts the effect already keeps off the eye
  (a distance fade: its muzzle smoke) is a fifth the size (sizes, offsets,
  speeds) and drawn with the gun. At a fifth the Python's flash peaks on
  screen about as big as the Pack-a-Punched Python's own first-person
  flash (measured side by side). Bullet guns only: the launchers' shared
  flashes are launch smoke or already small; a flash made for the first
  person keeps its size.
- Holes on the fieldstone wall by the start were measured, not changed:
  they are the files' own. The wall (`wpc/jun_art_rock_wall01`, its clip
  the same) is rock; Zombies' and the campaign's tables give rock hits
  `fx_small_rock` / `fx_large_rock`, whose hole is the chipped-plaster
  crater (`gfx_impact_plaster01/02`, half the picture solid where wood's
  splinters are a quarter) at radius 2.6-3.75 (pistols, SMGs) or 3.4-5
  (rifles, the Python), tinted 0.75. On this light wall (texel 0.59) the
  tinted crater (0.32) draws as a dark patch the size of the whole picture:
  an M1911 hole measured 5.7 units across from 62 away. Black Ops II's own
  world-mark shader (`wc_lit_sm_b0c0n0`) would draw it heavier, not
  lighter: it blends by the square root of the picture's alpha (darker soft
  edges) and lights the crater through its normal map (the wall's light is
  80% directional, nearly head-on, so every tilted facet darkens). Neither
  is drawn yet.

Test aids (environment): `IW4L_T6_FX_ONLY=a,b` / `IW4L_T6_FX_SKIP=a,b`
(effect materials by name), `IW4L_T6_NO_SHINE`, `IW4L_T6_NO_BLOOM`,
`IW4L_T6_NO_SOFT`, `IW4L_SOUND_LOG` (every sound start),
`IW4L_PERF_WINDOW=secs` (the `bo2zm perf` window), `IW4L_SHOT_LOG` (every
shot's aim, spread before -> after its own fire add and camera kick, each
bullet's tracer and landing, each throw and every tick of a grenade's
flight), `IW4L_TRACER_DEBUG=w,s` (tracers w times wider, s times slower),
`IW4L_FX_SPRITE_LOG=<effect name part>` (that effect's sprites every
frame), `IW4L_ANIM_LOG` (the fire animation's settle),
`IW4L_MARK_LIGHT_LOG` (a world mark's lightmap light in parts).
Probes in `crates/bo2zm_tools`: `t6m2` (weapons, effects, sounds;
`T6M2_WEAPON`, `T6M2_AIM`, `T6M2_FX`, `T6M2_IMPACT`, `T6M2_ALIAS`,
`T6M2_VIS_RAW`, `T6M2_FLAGS` (which element flag bits occur on which
element types), `T6M2_ZONES=a,b` for other zones such as the campaign's
`common`), `t6ipak` (`T6IPAK_FIND=<hash>`, `T6IPAK_OUT=<file>`), `t6gsc`
(scripts; `T6GSC_PLACED`), `t6fxmat`, `t6bones`, `t6shader`, `t6mat`,
`t6props` (placed models and their collision; `T6PROPS_NEAR=x,y,z,r`,
`T6PROPS_MAT=<name part>` for world surfaces and clip materials,
`T6PROPS_AT=x,y,z,r` for the world surfaces near a point,
`T6PROPS_MATFLAGS=<name part>` for any zone's materials' game flags and
surface type bits).

## Zombies: third fix list (M3, 10-03)

- A zombie's pose takes its animation times from the same snapshot its
  frame time is measured from (the owner's copy is a frame late after each
  new snapshot, which stepped the walk back 20 times a second).
- Text keys are looked up without case (`ZOMBIE_WEAPON_BERETTA93r`).
- A bullet or pellet that hits nothing still sends its tracer to the end of
  its range (no impact). The player's own tracers leave the barrel on the
  first frame 25 ms after the shot, past the first-person muzzle flash
  (our choice; Black Ops II's pistol tracer is 30 units long and was spent
  under the flash).
- Black Ops II guns draw their hip crosshair (`reticle_side_small` and the
  other reticle materials come from the zone's HUD icons).
- Mouse wheel and `1` switch weapons by default.
- Zombies pass a path point within 32 units of it (a crowd rising at one
  crater spot otherwise froze on its first point until the scripts'
  30-second failsafe), and one that has not moved 16 units in 3 s plans
  again from where it stands; stalling twice short of the same path node,
  it plans round that node for 15 s.
- The hip crosshair hides with the rest of the HUD (the scripts'
  `setclientuivisibilityflag("hud_visible", 0)`).
- An actor's goal counts as reached only within 80 units in height too
  (the engine's `goalheight`).
- Solid collision models (`collision_*`, `zm_collision_perks1`) and the
  Mystery Box (any shown `magic_box` piece) block player movement as turned
  boxes: `SnapshotMeta::oriented_blockers`, six-plane brushes in the
  movement trace on the server and in the client's prediction. The
  `collision_wall/player_AxBxC` models have no collision surfaces and only
  a cube for bounds; their size comes from the name (width x height x
  thickness on local x, z, y, centred), which Nuketown's six-wall box in
  `nuked_collision_patch` confirms. Model capability bounds are (middle,
  half-size). Test aid: `IW4L_T6_BLOCKERLOG`.
- A placed model without baked vertex light takes the brighter of its own
  light grid set (`GfxStaticModelDrawInst.colorsIndex`) and the grid at its
  origin; grid points buried in a model or the ground are near black (the
  bunker behind the teal house).

Test aids: `IW4L_T6_POSE_LOG=<clip part>`, `IW4L_T6_OPENALL=buy[:flags]`
(doors and debris bought by their own scripts), `IW4L_T6_ROAM="x y z;..."`
with `IW4L_T6_ROAM_SECS`, `IW4L_T6_GLIDE="x y;..."`,
`IW4L_T6_ONLY_SPAWN="x y"`, `IW4L_T6_CENSUS` (now with walk/run/sprint and
each move state's animation speeds), `IW4L_T6_SMLIGHT_LOG=<model part>`,
`IW4L_T6_SMLIGHT_ALL`. Probes: `T6M2_TRACERS`, `T6RUN_ENTS=<text>`,
`T6MAT_SMODEL=<model part>`. In a scripted run `press weapnext 0.2`
switches weapons.
