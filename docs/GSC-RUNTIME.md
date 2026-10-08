# GSC → IR → Bevy

Rust language, IR and host integration live under `sim::script`.
Gameplay policy belongs to loaded GSC; Rust supplies language and engine primitives.
The scripts own the match flow,
player connect/spawn/damage/killed, the in-game menus, and the sounds they play.

## Sources and installation

Matches support IW4 modes only (dm, war, dom, sd, sab, ctf, koth, dd).
T5 and IW5 maps supply entities and map declarations; their gameplay/UI scripts
are excluded. Menu/HUD layout and match flow stay IW4, with weapon and team icons
from the corresponding game. Objective models, materials, shaders and effects
come from the map's game, including the plant/defuse weapon viewmodels and animations.
`ObjectiveVisuals` binds IW4 names to those assets at
install; missing bindings or foreign objective assets refuse the transaction.
Common scene models are selected from retained IW4 source strings, table cells,
map entities and objective bindings, without a per-model capture allowlist.

`Program::load` compiles a `SourceResolver`. `FileSources` reads explicit files.
IW4 asset walks retain GSC RawFiles from common_mp and the map; map files override
common files. `ScriptSources` also keeps every string table (code_post_gfx_mp and
patch_mp, then common_mp, then the map, later zones winning) and the map's entity
string. Sources travel through the common/resident caches into PreparedMatch.
Session preflight compiles the mode and map modules and builds `LevelData` (parsed
entities, tables). The install transaction installs the program, runs
`codescripts/struct::initstructs` synchronously (it must not wait), spawns the map
entities, sets the `mapname` and `g_gametype` dvars and schedules the `Iw4Startup`
entries before authority ticks: the gametype `main`, the map `main` (if present), then
`_callbacksetup`'s `codecallback_startgametype`. Missing dependencies, unsupported
syntax, unresolved names and unbound natives fail the transaction.

Map entities: `worldspawn` is kept as level settings (`getnorthyaw`),
`script_struct` blocks become plain objects appended to `level.struct`, and every other
block, known classname or not, becomes a script entity with typed keys (`origin`,
`angles`, `angle` as yaw, integer `spawnflags`/`count`/`health`/`dmg`/`maxhealth`,
float `speed`/`radius`/`height`, strings otherwise). Every entity except a hud element
carries `classname`, `code_classname` (`script_vehicle` for any `script_vehicle*`) and a
zero `origin` and `angles` until something sets them. Entity numbers start at the first
non-client slot.
Names are normalized; traversal is rejected. The transaction announces its script
outcome on stdout for a controller: `gsc: installed …` with the program fingerprint
and module/function/native/entry counts once the world is published, `gsc: refused …
stage=compile|install|entry fault="…"` when the load stopped at the script.
`crates/approved_tests` records and asserts both.

Natives are linked against a `Catalog` of the game's builtins (generated into
`script/profile/iw4_catalog.rs`, split into function and method namespaces), not against the
registry. An unqualified call resolves to a function in the same file, then a builtin
in the call's namespace, then a unique include. Qualified names and `thread` calls
never resolve to builtins. A developer builtin used as a statement compiles to nothing
(arguments are not evaluated); using its value or referencing it fails the load.
`prof_begin`/`prof_end` statements compile to nothing. SHA-256 covers decompressed source bytes
after terminator removal. Fingerprints hash IR version 3 and each module's
`(site, realm)`; server modules of the IW4 and T5 realms are instantiated. Calls, `::f` references (script
`Function` or native `Builtin`), locals (per-function slots) and field names (symbol ids)
are resolved at load, and `install` binds native slots once.
UTF-8 and single-byte sources are supported; developer blocks are excluded.

## Language and scheduling

IR supports calls/references/indirect calls, receiver threads, locals, object fields,
arrays/index lvalues, constants, for/foreach/while, switch, conditional expressions,
waits and receiver-bound events. Localized strings and animation literals retain
separate value types. Values are `Int(i32)` and `Float(f32)`. The operator, cast, truth and
string-conversion rules are:
- int/int `/` yields a float. Division or `%` by zero yields zero and does not fault.
- `% & | ^ << >>` accept ints only.
- A type mismatch in `==` or a comparison is a runtime error.
- Only ints and floats have truth.
- Floats print with MSVC `%g`.
- Overlong int literals wrap.

Arrays copy on assignment and argument/event transfer; objects preserve aliases.
Foreach snapshots sorted keys.

Scheduling uses time buckets:
- The run loop pops the head of the current bucket.
- `wait` pushes to the head of its target bucket.
- `waittillframeend` pushes to the tail of the current bucket.
- `wait` counts server frames: an int is 20 frames per second, a float rounds
  `f * 20 + 0.5` down in f32, and a nonzero wait is at least one frame. A negative or
  too-long wait faults. `wait 0` resumes in the same frame, after the current thread yields.
- `thread f()` runs `f` inline until its first yield, then the spawner continues.
  Calls and thread calls share a limit of 31 frames across the running thread and the
  spawners suspended under it. An endon that fires on a suspended spawner unwinds it
  when control returns to it.

Events use one waiter list per program:
- Notify walks the waiters oldest-registration first and never runs script.
- Woken threads go to the head of the current bucket, so they resume newest first.
- `waittillmatch` skips non-matching payloads silently.
- `endon` is scoped to the registering frame. Firing it kills that frame and the frames above
  it. The caller resumes with `undefined`, or the thread ends if the root frame was killed.
- An endon registered before a waittill on the same event wins.

Deleting an entity ends the threads that `waittill` or `endon` on it, checked at the
start of each tick and before a thread resumes. A thread whose `self` is deleted keeps
running. The `delete` native sends `death` before retiring the entity. A deleted entity
keeps its fields until the frame ends, so `death` handlers can still read them; after that, references point at a
fieldless dead entity: field reads give `undefined`, writes land, `isdefined` is false
and the id is reclaimed by heap collection.

Runtime errors recover: the failed
operation leaves `undefined` in place of its results and the thread carries on. A failed
condition falls into the body, a failed switch takes the default, and a failed `foreach`
operand iterates nothing. Each site is logged once (`gsc: runtime_error at=… fault="…"`)
and counted. A thread that runs 16M instructions in one resumption is killed as a runaway
loop (level startup runs in one frame and needs over a million on the larger maps). A
panicking builtin is a runtime error of its caller. Array copies have bounds.
Heap collection follows globals and live threads, including cycles; natives must
keep persistent script values in script-owned roots rather than retain raw handles.

## Engine boundary and restoration

Natives are grouped by what they touch; the core three:
- `script/host/natives/iw4.rs`: dvars (`getdvar*` with an optional default for a missing dvar,
  `setdvar` storing a localized value as its reference, `setdvarifuninitialized`,
  `makedvarserverinfo`), precache and `loadfx` (allowed only while the first tick is
  loading), string helpers, presented client state (fog, vision, ambient).
- `script/host/natives/math.rs`: math in degrees, vectors, deterministic randomness (a runtime-owned
  xorshift seeded from the program fingerprint, so clones and replays draw the same
  numbers), substrings, and string tables (case-insensitive lookups, `""` or `-1` when
  missing).
- `script/host/natives/engine.rs`: entity queries and `spawn`, entity methods (origin, model,
  visibility, contents, link offsets, attachments), timed moves and rotations that
  notify `movedone`/`rotatedone`, radius-trigger `istouching`, `placespawnpoint`,
  hud elements, world traces against the map's clip, team scores and radar, match data,
  level settings, and weapon facts from the captured weapon table.

Argument reads live in `script/host/args.rs`, array helpers in `script/host/arrays.rs`,
and table lookup in `script/host/tables.rs`. All three use the one `Runtime`
(`arrays`, `next_object`, `tables`, `rng`). The compiler is `sim::script::compiler`:
`Token`/`lex` and the call, expression, assignment, and statement methods sit in child
modules, and `Parser` stays in the parent.

Sounds the scripts play (`playLocalSound`, `playSoundToPlayer`, `playSoundToTeam`)
reach the client. Effects, rumble and earthquakes validate their receiver and are
kept in `Runtime.presented` or dropped. In-game menus (team, class, escape, leave-game,
scoreboard header) and the IW4L frontend run from menuDefs through the same ordered
GPU menu pass. `crates/ui/menus/frontend.json` defines IW4L navigation and native
menu styling; frontend service commands connect it to session and master APIs.

Killstreak hardware:
- `spawnHelicopter` arms the vehicle with its VehicleDef `turretWeaponName` (captured
  from the zone, including weapon bodies loaded inline through the vehicle), so the
  gunner's `turret_fire` → `fireWeapon` loop needs no `setVehWeapon`. Turret and vehicle
  weapons with clip 0 or zero fire/raise times are valid combat rows.
- Sentry placement traces a ray to 42 + 5 units ahead,
  a ±30 box drop, normal ≥ 0.7, then four feet (17, 20, 10) traced 20 down that tilt the
  seat, failing when two miss. A failed placement keeps the fallback pose.
  `setContents(0)` makes the carried turret non-solid, so the traces skip it;
  `setContents` returns the previous contents for the restore pattern.
- `notifyOnPlayerCommand` notifies go out for every bound command, including
  `+actionslot N`; `+breath_sprint`, `+stance` (tap crouch, 300 ms hold prone) and
  `centerview` act on the client.
- The laptop map (`setminimap` frame, location selection with direction and yaw) draws
  from the map's compass corners.
- Entity allocation is shared by movers, grenades, missiles and dropped items. A full
  pool drops the new entity with a warning instead of ending the match. `radiusDamage`
  with an invalid area and `glassRadiusDamage` are no-ops.
- A dying player gets `death` (with the attacker) before `CodeCallback_PlayerKilled`;
  the stock scripts send it themselves only for faux death, so every
  player `endon( "death" )` thread depends on it.

The client draws a carried sentry at the authority's 20 Hz pose, not re-placed per
frame from the predicted player state.

The scripts drive the match phase. Every `level` notify is recorded as a signal, and
the simulation reads them after the scheduler each tick: `prematch_over` finishes the
prematch (movement unlocks), `exitlevel` moves to intermission. `map_restart` frees
the level's entities and hud elements, reloads the scripts and keeps the primitive
part of `game` (and `pers` when asked to persist).
Disconnect requests from transport are queued for an authority frame. The player
callback runs before Rust removes the player state and metadata; it can still read
the name, score and team while the scripts remove the player from their lists.
Scripts run and are checked only on authority frames (`try_step`/`step` with
`StepReason::AuthorityFrame`); prediction of new and replayed commands steps a
script-less world. Per-client script control constraints travel in snapshots and are
restored before prediction and command replay. Button-query natives sample raw commands
from the current authority frame, including presses between its first and last commands.
Clones preserve program, heap, threads and waits. Shutdown resets them. Restoring
snapshots into an installed authority VM rejects absent script state. Persistent
replay pinning/restoration and client-role lifecycle evidence are still missing.
Only terminal faults (exhausted identifiers, inconsistent IR) are sticky; they end the
match and return the host to the lobby. Earlier
writes are not rolled back.

## Remaining

A live bomb plant/defuse check. Native IW4L menus can create a private lobby,
select a map/mode, start a match and return to the frontend. Browser/public-lobby
actions are connected but still need multiplayer validation. Display mode/resolution, brightness, volume, VSync, shadows, depth of field and
bloom are bound to runtime settings and persisted locally. Surround output, voice/chat
settings, third-person camera and spectator restrictions remain incomplete. A true
dedicated server still runs on the listen runtime.
