pub const INPUT_COMMAND_NAMES: [&str; 81] = [
    "",
    "+attack",
    "-attack",
    "+melee",
    "-melee",
    "+frag",
    "-frag",
    "+smoke",
    "-smoke",
    "+breath_sprint",
    "-breath_sprint",
    "+usereload",
    "-usereload",
    "+speed_throw",
    "-speed_throw",
    "+actionslot 1",
    "-actionslot 1",
    "+actionslot 2",
    "-actionslot 2",
    "+actionslot 3",
    "-actionslot 3",
    "+actionslot 4",
    "-actionslot 4",
    "+stance",
    "-stance",
    "+gostand",
    "-gostand",
    "+forward",
    "-forward",
    "+back",
    "-back",
    "+moveleft",
    "-moveleft",
    "+moveright",
    "-moveright",
    "+movedown",
    "-movedown",
    "+left",
    "-left",
    "+right",
    "-right",
    "+lookup",
    "-lookup",
    "+lookdown",
    "-lookdown",
    "+strafe",
    "-strafe",
    "+holdbreath",
    "-holdbreath",
    "+activate",
    "-activate",
    "+reload",
    "-reload",
    "+prone",
    "-prone",
    "+mlook",
    "-mlook",
    "+toggleads_throw",
    "-toggleads_throw",
    "+sprint",
    "-sprint",
    "+scores",
    "-scores",
    "+talk",
    "-talk",
    "togglemenu",
    "weapnext",
    "pause",
    "chatmodepublic",
    "chatmodeteam",
    "weapprev",
    "centerview",
    "togglecrouch",
    "toggleprone",
    "goprone",
    "gocrouch",
    "toggleads",
    "leaveads",
    // Black Ops II's switch-weapon key (players/bindings_mp.bdg: MWHEELUP and
    // BUTTON_Y "+weapnext_inventory"): a tap is the next weapon on release, a
    // hold is the inventory weapon. `inventory` picks the inventory weapon.
    "+weapnext_inventory",
    "-weapnext_inventory",
    "inventory",
];

pub const HOLD_PAIR_LIMIT: u32 = 0x41;

/// `+weapnext_inventory`; its up half follows it.
pub const WEAPNEXT_INVENTORY_DOWN: u32 = 78;
/// `inventory`: select the inventory weapon.
pub const INVENTORY_COMMAND: u32 = 80;

pub const SCRIPT_KEYNUM: i32 = 0x400;

pub fn command_id_from_name(name: &str) -> Option<u32> {
    INPUT_COMMAND_NAMES
        .iter()
        .position(|&s| s == name)
        .map(|i| i as u32)
        .filter(|&id| id != 0)
}

pub fn command_id_lookup(name: &str) -> Option<u32> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut buf = [0u8; 32];
    let n = trimmed.len().min(buf.len());
    for (i, b) in trimmed.as_bytes()[..n].iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let folded = core::str::from_utf8(&buf[..n]).ok()?;
    if let Some(id) = command_id_from_name(folded) {
        return Some(id);
    }
    if folded.starts_with('+') || folded.starts_with('-') {
        return None;
    }
    if n + 1 > buf.len() {
        return None;
    }
    buf.copy_within(..n, 1);
    buf[0] = b'+';
    let plus = core::str::from_utf8(&buf[..n + 1]).ok()?;
    command_id_from_name(plus)
}

pub fn command_name(id: u32) -> Option<&'static str> {
    INPUT_COMMAND_NAMES
        .get(id as usize)
        .copied()
        .filter(|s| !s.is_empty())
}

pub fn key_up_command_id(binding: u32) -> Option<u32> {
    if binding == 0 {
        None
    } else if binding < HOLD_PAIR_LIMIT && binding % 2 == 1 {
        Some(binding + 1)
    } else if binding == WEAPNEXT_INVENTORY_DOWN {
        Some(WEAPNEXT_INVENTORY_DOWN + 1)
    } else {
        None
    }
}

pub fn command_names() -> impl Iterator<Item = &'static str> {
    INPUT_COMMAND_NAMES
        .iter()
        .copied()
        .filter(|s| !s.is_empty())
}
