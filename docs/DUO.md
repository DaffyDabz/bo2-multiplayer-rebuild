# Two local players through the master

`make duo` builds the game and opens a host and a client in separate 960×540
windows. It uses `IW4L_GAMES` and the `IW4L_MASTER_*` connection settings from
`.env`, like `make menu`. The configured master can be local or remote.

```bash
make duo
make duo ZONE=iw4:mp_afghan MODE=dm
make duo HOST_CMDS='spawn 0; wait 20s; dump host' CLIENT_CMDS='spawn 0'
```

Every run creates a fresh public lobby on the master. The host's actual lobby
ID is printed and saved in `iw4l-artifacts/duo/<run>/lobby.id`; the client joins
that ID directly. The host starts the match only after both sides report lobby
membership. There is no browser-row selection or fixed lobby startup delay.
GSC still owns classes, countdown, scoring, death and respawn.

`HOST_CMDS` and `CLIENT_CMDS` run after `wait world`; both default to
`spawn 0`. During the run, type into the terminal:

```text
host dump host
client dump client
both screenshot
host hold +attack
host release +attack
quit
```

Each line is forwarded to the regular console queue. Semicolons separate
commands; `!` retains its normal meaning of bypassing a wait. `quit` stops the
pair. Closing either game window also ends the pair. Logs, settings, dumps and
screenshots are isolated under the run's `host/` and `client/` directories.
On Unix, the asset cache is shared through symlinks.

For other launch tools, `IW4L_CONSOLE_STDIN=1` enables line-based console input.
`IW4L_MASTER_STATUS_FILE=<absolute path>` writes the current master state as
`key=value` lines (`state`, `room`, `members`, `in_match`) using atomic replacement.
`ui_join_lobby_id <id>` submits the normal master join request; set `ui_mapname`
and `ui_gametype` to the lobby's selection first. Admission remains the master's
responsibility. These interfaces do not bypass networking or GSC.
