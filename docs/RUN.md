# Run and poke the live game

How to touch the live process. Observation is [`PERF.md`](PERF.md).
Two players through the master: [`make duo`](DUO.md).

```bash
set -a; . ./.env; set +a          # IW4L_GAMES; DISPLAY=:0 if the session has none
make map mp_boneyard CMDS='spawn 0; wait 2s; quit'
cargo run --profile play -p launcher -- map iw5:mp_overwatch --cmds '…'
```

`make` passes no foreign flags through: in the `Makefile` it is `CMDS`, on the
binary `--cmds`. The colon is a GNU make pattern, so `make map iw5:…` fails —
write `make map ZONE=iw5:mp_overwatch` or use `cargo run`. Recipes: `make
scenario`, `chaos`, `bench` ([`BENCH.md`](BENCH.md)), `bench-live`, `lifecycle-*`
(`*_CMDS` in the `Makefile`). Live recipes use `[profile.play]`; LTO is `PROFILE=release`.

**GSC controls the start freeze.** `freezecontrols` constrains authority movement;
snapshots carry the same constraints into client prediction and command replay.
Wait for the loaded mode's countdown, or use the debug command:
`wait world; spawn 0; force_match_start; wait 1s; …`.
Firing, hit and decal probes must start after the countdown releases the player.

**Sync by default.** Ritual `wait`s are unnecessary: a command holds the FIFO
until its own fact — `map`/`demo`/`play` until the swap plus GPU-ready
`WorldScene::spawned`, `spawn` until `AppScreen::InGame`, `move`/`look`/`tp`
until a matching presented pose, `force_match_start` until `Playing`,
`disconnect` until the hold is torn down. `wait Ns` remains, for pacing and
observation windows. The `&` suffix is a loud refusal to wait (`map mp_rust &`),
`!` jumps the queue (in bash write `disconnect !`, or history expansion bites).
The 120 s gate timeout aborts the rest with an echo; parsing is
`crates/console/src/`. Hand-typed `dump`/`clip`/`screenshot` pass through any
wait while preserving the queue, which inside `--cmds` takes an explicit `!`.
A hand-typed `quit`/`exit`/`disconnect` goes further: it jumps **every** hold,
`wait 60s` included, and releases it. A `quit` written into `--cmds` does not —
it is part of the script and waits its turn.

## Verbs and traps

`map spawn class give attach name kill damage move tp look nudge press hold
release bind bot wait mark record stoprecord clip demo dump screenshot ui disconnect quit
finish_run`
plus the debug `force_match_start` / `showpos`. `quit` leaves now and abandons
whatever screenshot was queued or half-written; `finish_run` is the scripted
ending that waits for those files first. `disconnect` leaves the session, not
just the world — it also leaves the room, or closes it when hosting, and works
with no map installed. `bot` is
`add | dummy | hold | tp | give | fire`; `dump [name]` writes the current snapshot into
`dumps/`. `clip` writes the last available 45 s into `clips/<ULID>/` (demo + dump)
on host and clients — the host records authority, a client the snapshots it
received plus its own presented state; `demo LATEST` plays it back.

* ADS is `hold +speed_throw`, not `+speed`;
* Aim binding: `bind MOUSE2 +speed_throw` aims only while held;
  `bind MOUSE2 +toggleads_throw` toggles aim on each press. Both are available
  under Options → Controls → Actions, and the chosen bind is saved in settings.
* bolt action (`fire_type=1`): `hold +attack` is **one** shot, and a full
  magazine will not reload itself (`press +attack` ×N, then `press +reload`);
* `look` without `LookState` only writes `ps.viewangles` — no aiming;
* Use (`+activate` or `+usereload`) retrieves your settled C4, claymores and
  deployable gadgets when there is room in their equipment ammo slot.
* `give` takes a namespace: `give t5:weapon/psg1_acog`, `give iw5:weapon/msr`.
* custom classes live in `iw4l-artifacts/profile/classes.txt` (one tab-separated
  row per class); `spawn 0` selects the first slot. Delete the file to generate
  five available classes again.

Video settings include brightness (50–150%, neutral 100%) and FOV (65–120°)
sliders. Drag with the mouse or use Left/Right on the focused slider; the value
appears to its right. Multiplayer settings contain the player-name field
(Enter to edit and accept, Escape to cancel). These settings persist across
launches in `$XDG_CONFIG_HOME/iw4l/settings.cfg` or `~/.config/iw4l/settings.cfg`
on Linux; `IW4L_SETTINGS_PATH` selects a separate profile for probes.
