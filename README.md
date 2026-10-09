# BO2 Multiplayer Rebuild

A fan rebuild that plays Call of Duty: Black Ops II multiplayer from your own PC copy of the game, in an engine
written in Rust. It is a fork of [IW4L](https://github.com/vladtrc/iw4L), an open-source Call of Duty runtime built
on [Bevy](https://bevyengine.org/) and [wgpu](https://wgpu.rs/). The same code that runs Black Ops II's Nuketown
Zombies in this fork runs Black Ops II's own multiplayer scripts, menus and bots, so a match follows the game's real
rules without ever starting the original executable.

**No game files are included.** This repository holds no Black Ops II (or any other Call of Duty) files: no maps,
models, textures, sounds, scripts or executables. You need your own legally owned copy of Black Ops II for PC with
Multiplayer installed. The engine reads that install read-only at run time and never patches, copies or
redistributes it. Unofficial fan project, not affiliated with or endorsed by Activision, Treyarch or Infinity Ward.

## Contents

- [Map completion](#map-completion)
- [Features](#features)
- [Requirements](#requirements)
- [Build and run](#build-and-run)
- [Configuration](#configuration)
- [How it works](#how-it-works)
- [Status](#status)
- [Recent changes](#recent-changes)
- [Coming soon](#coming-soon)
- [Credits and license](#credits-and-license)

## Map completion

How far along each multiplayer map is, mechanically (loads, spawns, each mode, bots, scorestreaks, killcam, match
end) and visually (fixed spots shot in the original game and here from the same place, side by side). Every number
comes from [docs/maps.json](docs/maps.json), where each passing item names the test run or screenshot that proves
it; [docs/make_bars.py](docs/make_bars.py) draws this table. A visual spot only counts once it has a same-spot
comparison shot from the original, so visuals read 0% until those shots are taken.

<!-- map-bars:start -->
```text
                Mechanics         Visuals
Aftermath       ████░░░░░░  38%   not compared yet
Cargo           ██░░░░░░░░  25%   not compared yet
Carrier         ██░░░░░░░░  25%   not compared yet
Drone           ██░░░░░░░░  25%   not compared yet
Express         ██░░░░░░░░  25%   not compared yet
Hijacked        ██░░░░░░░░  25%   not compared yet
Meltdown        ██░░░░░░░░  25%   not compared yet
Overflow        ██░░░░░░░░  25%   not compared yet
Plaza           ██░░░░░░░░  25%   not compared yet
Raid            ██░░░░░░░░  25%   not compared yet
Slums           ██░░░░░░░░  25%   not compared yet
Standoff        ██░░░░░░░░  25%   not compared yet
Turbine         ██░░░░░░░░  25%   not compared yet
Yemen           ██░░░░░░░░  25%   not compared yet
Nuketown 2025   ███████░░░  72%   not compared yet
Downhill        ██░░░░░░░░  25%   not compared yet
Mirage          ██░░░░░░░░  25%   not compared yet
Hydro           ██░░░░░░░░  25%   not compared yet
Grind           ██░░░░░░░░  25%   not compared yet
Encore          ██░░░░░░░░  25%   not compared yet
Magma           ██░░░░░░░░  25%   not compared yet
Vertigo         ██░░░░░░░░  25%   not compared yet
Studio          ██░░░░░░░░  25%   not compared yet
Uplink          ██░░░░░░░░  25%   not compared yet
Detour          ██░░░░░░░░  25%   not compared yet
Cove            ██░░░░░░░░  25%   not compared yet
Rush            ██░░░░░░░░  25%   not compared yet
Dig             ██░░░░░░░░  25%   not compared yet
Frost           ██░░░░░░░░  25%   not compared yet
Pod             ██░░░░░░░░  25%   not compared yet
Takeoff         ██░░░░░░░░  25%   not compared yet
All maps        ███░░░░░░░  27%   not compared yet
```

Mechanics: each map's systems and modes work by Black Ops II's rules. Visuals: set spots compared side by side with Black Ops II. Scored in [`docs/maps.json`](docs/maps.json) (pass or match 1, part or close half, not yet 0), last updated 2026-10-08; drawn by [`docs/make_bars.py`](docs/make_bars.py).
<!-- map-bars:end -->

## Features

- **Black Ops II's own front end.** `frontend t6mp` opens the multiplayer main menu built from the game's UI
  scripts: custom games lobby, Setup Game, Setup Bots, Create-a-Class, Scorestreaks, then Start Match. Classes are
  saved next to the binary.
- **Every multiplayer map.** All maps on your disc and in the DLC packs you own load and can be played, each with
  its own factions, lighting, colour grading, sky, water and foliage.
- **The game's own match rules.** Black Ops II's compiled multiplayer scripts run the match: spawns, scoring,
  score and time limits, rounds, the end of the match. The one script no PC file carries (`_globallogic`, the match
  flow) is written for the engine from what the other scripts call, and compiled by the engine's own script compiler
  (`crates/sim/src/t6/scripts/mp_globallogic.gsc`).
- **Bots.** The game's bot scripts choose goals, classes and targets; the engine walks them over the map's path
  nodes, aims, jumps, climbs ladders and fires. Bots play Team Deathmatch, Free-for-All, Domination, Kill
  Confirmed, Hardpoint, Capture the Flag, Demolition and Headquarters.
- **In the match.** The class menu at spawn, the HUD with the turning minimap, compass, clock, score bars, kill feed
  and scoreboard, the pause menu, menu sounds, the killcam, dropped guns and Scavenger bags, and the end-of-match
  screen back to the main menu.
- **Scorestreaks.** Score earns streaks through the game's own momentum code; bots call in Orbital VSAT,
  Counter-UAV, EMP, Death Machine, War Machine, Swarm, Escort Drone and K9 Unit.

## Requirements

- Your own Call of Duty: Black Ops II for PC (Steam) with Multiplayer installed.
- Windows x64 (built and played on Windows 11 only so far).
- Rust (stable, via [rustup](https://rustup.rs/); the code uses the 2024 edition) and Visual Studio 2022 Build Tools
  with the C++ workload (MSVC).
- A GPU with DirectX 12 or Vulkan.
- Tens of GB of free disk space for the Rust build output.

## Build and run

There are no release downloads: you build it from this source tree. These steps have been done on the author's PC
only, not yet on a fresh one by someone else.

1. Install Rust and the Visual Studio 2022 Build Tools (C++ workload).
2. Clone or download this repository.
3. Optional: set `CARGO_TARGET_DIR` to a folder on a drive with plenty of room (the default is `target\` inside the
   repository).
4. Build the game:

   ```powershell
   cargo build -p launcher --profile play
   ```

   The binary is `<target-dir>\play\iw4l.exe`.
5. Make a play folder anywhere, copy `iw4l.exe` into it, and create a `.env` file next to it that points at the
   folder holding your Black Ops II install (quoted, forward slashes), for example:

   ```text
   IW4L_GAMES="C:/Program Files (x86)/Steam/steamapps/common/Call of Duty Black Ops II"
   ```

6. From the play folder, start the multiplayer front end:

   ```powershell
   .\iw4l.exe frontend t6mp
   ```

   Pick a mode, map and bots in the custom games lobby and press Start Match. A Windows shortcut with that target
   and "Start in" set to the play folder works the same way. Keys follow Black Ops II's PC defaults; Esc opens the
   pause menu.

Settings, saved classes, caches and logs go to `iw4l-artifacts\` next to `iw4l.exe`.

Other tools: `cargo build -p bo2zm_tools --profile play` builds the zone readers described in
[`docs/BO2ZM.md`](docs/BO2ZM.md). Tests: `cargo test -p gsc_t6` (and `cargo test -p <crate>` for the others).
More documentation is indexed in [`docs/INDEX.md`](docs/INDEX.md).

## Configuration

- `.env` next to the binary: `IW4L_GAMES` (required).
- `iw4l-artifacts\settings.cfg`: saved settings; `iw4l-artifacts\bo2mp_stats.txt`: saved classes.
- Test switches for scripted test matches (`BO2MP_SCRIPTS`, `BO2MP_BOTS`, `BO2MP_GAMETYPE`, `BO2MP_AUTOCLASS`, ...)
  are listed in [`docs/BO2ZM.md`](docs/BO2ZM.md).

## How it works

On top of IW4L's engine, this fork adds the crates that read Black Ops II (`fastfile_t6`, `asset_t6`, `gsc_t6`,
`hks_t6`, `bo2zm_tools`) and the multiplayer modules in `sim`:

| Part | What it does |
|---|---|
| `fastfile_t6`, `asset_t6` | read Black Ops II's zone files from your install |
| `gsc_t6::compile` | a compiler for Black Ops II script source, for the scripts the engine must provide (`_globallogic`) |
| `sim` `t6::mp`, `natives_mp` | the multiplayer runtime: game types, team scores, the match clock, spawn choice, objectives |
| `sim` `t6::bots` | the engine half of the game's bot scripts: test clients, goals, paths, view, buttons, threats |
| `sim` `t6::loadout`, `t6::items` | classes from the game's stats table; guns and Scavenger bags on the ground |
| `hks_t6::mp`, `bo2_profile` | the game's multiplayer menus, classes and stats |

## Status

Work in progress, first playable build. Matches against Black Ops II's own bots play on every multiplayer map, started
from the game's own menus. Many menus and HUD pieces are close to the original but not one-for-one, and some
scorestreaks and effects are still missing. It has been run on one Windows 11 PC (RTX 4070 Ti class GPU, 1920x1080)
only. Like upstream IW4L, much of this code was written by an LLM (Claude), directed and play-tested by the author.

## Recent changes

- 2026-10-08: movement, footsteps and head bob follow Black Ops II's own rules (set BO2_FEEL=mw2 for the old
  feel); sprinting bobs the gun the way each gun does, the dive has its own rhythm, and hard landings slow you and
  take health. Prestige at level 55 (Barracks > Prestige Mode). Flashbangs and concussions last and sway your gun as
  much as the game's own files say. The Hellstorm, VTOL Warship (with its thermal view), Dragonfire and AGR show
  their own screens; helicopters and drones play their engine sounds; dual-wielded guns both kick.
- 2026-10-08: other players go prone, dive and climb ladders at Black Ops II's own animation speeds, the
  VICTORY / DEFEAT banner at match end sits where the original puts it, and the README shows per-map completion
  bars.
- 2026-10-08: Frostbite's open sea no longer shows a dark wedge beside the rooftops, and cliffs, rocks and
  mountains on 13+ maps (Hydro, Cove, Drone, Castaway...) draw Black Ops II's own tiled rock detail instead of one
  flat colour.

## Coming soon

- Every menu and HUD piece one-for-one with the original (end scoreboard).
- Movement and gun-feel numbers checked side by side against the original game.
- The Lodestar's vehicle screen.
- Live sun shadows; ragdoll deaths.
- More per-map test runs to fill in the mechanics bars; visual comparison shots for each map.
- Playing together over LAN, and one build with Black Ops II's main menu for both Multiplayer and Zombies.

## Credits and license

- [IW4L](https://github.com/vladtrc/iw4L) by vladtrc and its contributors: the engine this fork is built on,
  Apache License 2.0.
- For Black Ops II, [OpenAssetTools](https://github.com/Laupetin/OpenAssetTools) and
  [gsc-tool](https://github.com/xensik/gsc-tool) (both GPL-3.0) were read for format facts only; none of their code
  is copied. Other projects read during development are listed in [`NOTICE`](NOTICE).
- Black Ops II multiplayer rebuild and the Black Ops II zone, script and menu work by DaffyDabz.
- Call of Duty, Black Ops and their assets, trademarks and intellectual property belong to their owners
  (Activision). No game files are included.

Licensed under the [Apache License 2.0](LICENSE), like upstream; [`NOTICE`](NOTICE) holds the copyright notices and
the licenses of the two bundled fonts. The license covers this source code only.
