//! bo2zm: the scripts a Black Ops II map starts with, written here. Black Ops
//! II's own scripts are compiled bytecode IW4L cannot run yet, and the IW4
//! match scripts come from MW2's common_mp, which a BO2-only install lacks.
//! These are just enough for the engine's script start-up (its four roots and
//! callbacks) and to put the player at the map's own start point.
//! bo2zm M3: when the map's own scripts run (dvar `bo2zm_t6` = 1), the
//! connect callback steps aside: they spawn and arm the player.

const DELETE: &str = "main()\n{\n}\n";

const STRUCT: &str = "\
initstructs()
{
\tlevel.struct = [];
}

createstruct()
{
\tstruct = spawnstruct();
\tlevel.struct[level.struct.size] = struct;
\treturn struct;
}
";

const GAMETYPE: &str = "main()\n{\n}\n";

/// Black Ops II Zombies' start: the M1911 in hand and two frag grenades
/// (`_zm_weapons.gsc` gives the same), by their script names (`_mp` added).
const ZOMBIES_LOADOUT: &str = "\
\tself giveweapon(\"m1911_zm_mp\");
\tself giveweapon(\"frag_grenade_zm_mp\");
\tself setweaponammoclip(\"frag_grenade_zm_mp\", 2);
\tself setoffhandprimaryclass(\"frag\");
\tself setspawnweapon(\"m1911_zm_mp\");
";

/// The start point and facing written into the connect callback, then the
/// map's starting loadout.
fn callbacks(origin: [f32; 3], yaw: f32, loadout: &str) -> String {
    format!(
        "\
codecallback_startgametype()
{{
\tlevel.gametypestarted = true;
\tlevel notify(\"prematch_over\");
}}

codecallback_playerconnect()
{{
\tif (getdvar(\"bo2zm_t6\") == \"1\")
\t\treturn;
\tself.sessionteam = \"allies\";
\tself.sessionstate = \"playing\";
\tself spawn(({x:.3}, {y:.3}, {z:.3}), (0, {yaw:.3}, 0));
{loadout}}}

codecallback_playerdisconnect()
{{
}}

codecallback_playerdamage(einflictor, eattacker, idamage, idflags, smeansofdeath, sweapon, vpoint, vdir, shitloc, timeoffset)
{{
}}

codecallback_playerkilled(einflictor, eattacker, idamage, smeansofdeath, sweapon, vdir, shitloc, timeoffset, deathanimduration)
{{
}}

codecallback_playerlaststand(einflictor, eattacker, idamage, smeansofdeath, sweapon, vdir, shitloc, timeoffset, deathanimduration)
{{
}}

codecallback_vehicledamage(einflictor, eattacker, idamage, idflags, smeansofdeath, sweapon, vpoint, vdir, shitloc, timeoffset, modelindex, partname)
{{
}}
",
        x = origin[0],
        y = origin[1],
        z = origin[2],
    )
}

/// The script set for a BO2 map: start-up roots, every gametype's empty
/// main, and an entity string holding only what spawning reads. A Zombies
/// map (`zm_*`) starts the player with the Zombies loadout.
pub(crate) fn start_scripts(origin: [f32; 3], yaw: f32, zombies: bool) -> crate::ScriptSources {
    let mut s = crate::ScriptSources::default();
    s.insert_source("codescripts/delete", DELETE.to_owned());
    s.insert_source("codescripts/struct", STRUCT.to_owned());
    let loadout = if zombies { ZOMBIES_LOADOUT } else { "" };
    s.insert_source("maps/mp/gametypes/_callbacksetup", callbacks(origin, yaw, loadout));
    for token in [
        "dm", "dd", "dem", "dom", "war", "tdm", "sd", "ctf", "koth", "sab",
    ] {
        s.insert_source(&format!("maps/mp/gametypes/{token}"), GAMETYPE.to_owned());
    }
    s.set_entities(format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n{{\n\"classname\" \"info_player_start\"\n\"origin\" \"{} {} {}\"\n\"angles\" \"0 {} 0\"\n}}\n",
        origin[0], origin[1], origin[2], yaw
    ));
    s
}
