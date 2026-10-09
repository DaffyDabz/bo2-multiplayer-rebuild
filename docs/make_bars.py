#!/usr/bin/env python3
"""Map completion bars for the README, from docs/maps.json.

    python docs/make_bars.py            # rewrite the table in README.md
    python docs/make_bars.py --print    # only print the table
    python docs/make_bars.py --check    # also fail if a pass/match has no evidence

docs/maps.json holds one row per map:

    {"id": "zm_tomb", "map": "Origins", "modes": ["Origins"],
     "mechanics": {"Mystery Box": "pass", "Panzer Soldat": "part", ...},
     "visuals":   {"spawn": "match", "church": "close", ...},
     "evidence":  {"Mystery Box": "runs/tomb-box-1", "spawn": "ref/zm_tomb/spawn.png", ...}}

States and scores (the bars come only from these):
    mechanics  pass = 1, part = 0.5, todo = 0
    visuals    match = 1, close = 0.5, wrong = 0, todo = 0 (not compared yet)
    na         not on this map; left out of the count
A Visuals cell with nothing compared yet (every spot todo) reads "not compared yet" instead of an empty bar.
A visual spot is "match" only when its geometry, textures, lighting/fog/sky, effects, doors/debris and HUD all
match Black Ops II side by side; "close" when most do. Every pass/match names its proof in "evidence" (a run
folder or a side-by-side shot), as a path relative to the work folder, never a drive letter or a user folder.

The table goes between the two marker lines in README.md:
    <!-- map-bars:start -->
    <!-- map-bars:end -->
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "maps.json")
README = os.path.join(HERE, os.pardir, "README.md")
START, END = "<!-- map-bars:start -->", "<!-- map-bars:end -->"
SCORES = {
    "mechanics": {"pass": 1.0, "part": 0.5, "todo": 0.0},
    "visuals": {"match": 1.0, "close": 0.5, "wrong": 0.0, "todo": 0.0},
}
GOOD = {"pass", "match"}
WIDTH = 10
NOT_COMPARED = "not compared yet"


def compared(items):
    return any(state not in ("todo", "na") for state in items.values())


def score(items, column, where, problems):
    total, n = 0.0, 0
    for name, state in items.items():
        if state == "na":
            continue
        if state not in SCORES[column]:
            problems.append(f"{where}: {column} '{name}' has unknown state '{state}'")
            continue
        total += SCORES[column][state]
        n += 1
    return total, n


def bar(total, n):
    pct = round(100 * total / n) if n else 0
    full = round(pct * WIDTH / 100)
    return f"{'█' * full}{'░' * (WIDTH - full)} {pct:3d}%"


def build(data, problems):
    rows, sums = [], {"mechanics": [0.0, 0], "visuals": [0.0, 0]}
    any_visuals = False
    for m in data["maps"]:
        where = m.get("map", m.get("id", "?"))
        cells = []
        for column in ("mechanics", "visuals"):
            items = m.get(column, {})
            t, n = score(items, column, where, problems)
            sums[column][0] += t
            sums[column][1] += n
            if column == "visuals" and not compared(items):
                cells.append(NOT_COMPARED)
            else:
                any_visuals |= column == "visuals"
                cells.append(bar(t, n))
            evidence = m.get("evidence", {})
            for name, state in items.items():
                if state in GOOD and not evidence.get(name):
                    problems.append(f"{where}: {column} '{name}' is {state} with no evidence")
        rows.append((where, cells[0], cells[1]))
    name_w = max(len(r[0]) for r in rows + [("All maps", "", "")])
    lines = ["```text", f"{'':{name_w}}   {'Mechanics':18}Visuals"]
    for name, mech, vis in rows:
        lines.append(f"{name:{name_w}}   {mech}   {vis}")
    all_visuals = bar(*sums["visuals"]) if any_visuals else NOT_COMPARED
    lines.append(f"{'All maps':{name_w}}   {bar(*sums['mechanics'])}   {all_visuals}")
    lines.append("```")
    lines.append("")
    lines.append(
        f"Mechanics: each map's systems and modes work by Black Ops II's rules. Visuals: set spots compared side by "
        f"side with Black Ops II. Scored in [`docs/maps.json`](docs/maps.json) (pass or match 1, part or close half, "
        f"not yet 0), last updated {data.get('updated', '?')}; drawn by [`docs/make_bars.py`](docs/make_bars.py)."
    )
    return "\n".join(lines)


def main(argv):
    with open(DATA, encoding="utf-8") as f:
        data = json.load(f)
    problems = []
    table = build(data, problems)
    for p in problems:
        print("warning:", p, file=sys.stderr)
    if "--print" in argv:
        sys.stdout.reconfigure(encoding="utf-8")
        print(table)
    else:
        raw = open(README, "rb").read().decode("utf-8")
        nl = "\r\n" if "\r\n" in raw else "\n"
        text = raw.replace("\r\n", "\n")
        if START not in text or END not in text:
            sys.exit(f"README.md has no {START} / {END} markers")
        head, rest = text.split(START, 1)
        _, tail = rest.split(END, 1)
        text = f"{head}{START}\n{table}\n{END}{tail}"
        open(README, "wb").write(text.replace("\n", nl).encode("utf-8"))
        print("README.md map completion table updated")
    if "--check" in argv and problems:
        sys.exit(1)


if __name__ == "__main__":
    main(sys.argv[1:])
