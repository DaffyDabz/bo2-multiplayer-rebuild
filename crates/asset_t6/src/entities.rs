//! The map entity text (`MapEnts`): `{ "key" "value" ... }` blocks.

#[derive(Clone, Debug, Default)]
pub struct MapEntity {
    pub fields: Vec<(String, String)>,
}

impl MapEntity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn vec3(&self, key: &str) -> Option<[f32; 3]> {
        let mut it = self
            .get(key)?
            .split_whitespace()
            .map(|v| v.parse::<f32>().ok());
        Some([it.next()??, it.next()??, it.next()??])
    }
}

/// Parse the whole entity text. Quoted strings only; no escapes in retail.
pub fn parse_entities(text: &str) -> Vec<MapEntity> {
    let mut out = Vec::new();
    let mut current: Option<MapEntity> = None;
    let mut strings = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        match c {
            '{' => {
                current = Some(MapEntity::default());
                strings.clear();
            }
            '}' => {
                if let Some(e) = current.take() {
                    out.push(e);
                }
                strings.clear();
            }
            '"' => {
                let mut s = String::new();
                for (_, c) in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    s.push(c);
                }
                strings.push(s);
                if strings.len() == 2 {
                    let value = strings.pop().unwrap_or_default();
                    let key = strings.pop().unwrap_or_default();
                    if let Some(e) = current.as_mut() {
                        e.fields.push((key, value));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_fields() {
        let ents = parse_entities(
            "{\n\"classname\" \"worldspawn\"\n}\n{\n\"origin\" \"1 2.5 -3\"\n\"classname\" \"info_player_start\"\n}\n",
        );
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[1].classname(), "info_player_start");
        assert_eq!(ents[1].vec3("origin"), Some([1.0, 2.5, -3.0]));
    }
}

/// bo2zm: a map's volumetric fog as its createart script sets it
/// (`setvolfog( start_dist, half_dist, half_height, base_height, fog_r,
/// fog_g, fog_b, fog_scale, sun_col_r, sun_col_g, sun_col_b, sun_dir_x,
/// sun_dir_y, sun_dir_z, sun_start_ang, sun_stop_ang, time,
/// max_fog_opacity )`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArtFog {
    pub start_dist: f32,
    pub half_dist: f32,
    pub half_height: f32,
    pub base_height: f32,
    pub color: [f32; 3],
    pub scale: f32,
    pub sun_color: [f32; 3],
    pub sun_dir: [f32; 3],
    pub sun_start_ang: f32,
    pub sun_stop_ang: f32,
    pub time: f32,
    pub max_opacity: f32,
}

/// Read `setvolfog`'s arguments from a compiled createart script (T6 GSC).
/// The script assigns its 18 locals in the call's order, highest local
/// index first; each assignment is a float literal (opcode 0x09, the value
/// aligned to 4 bytes) or zero (0x03), then 0x27 and the local's index
/// (measured on Nuketown's `maps/mp/createart/zm_nuked_art.gsc`). `None`
/// when the script has no `setvolfog` or not all 18 locals are found.
pub fn parse_art_fog(script: &[u8]) -> Option<ArtFog> {
    if !script.windows(9).any(|w| w == b"setvolfog") {
        return None;
    }
    let mut values: [Option<f32>; 18] = [None; 18];
    let mut i = 0usize;
    while i + 2 < script.len() {
        let (value, next) = match script[i] {
            0x09 => {
                let at = (i + 1 + 3) & !3;
                let Some(raw) = script.get(at..at + 4) else {
                    break;
                };
                (f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]), at + 4)
            }
            0x03 => (0.0, i + 1),
            _ => {
                i += 1;
                continue;
            }
        };
        if script.get(next) == Some(&0x27)
            && let Some(&index) = script.get(next + 1)
            && usize::from(index) < 18
            && value.is_finite()
        {
            values[usize::from(index)] = Some(value);
            i = next + 2;
            continue;
        }
        i += 1;
    }
    // Local 17 is the first argument.
    let mut args = [0.0f32; 18];
    for (k, arg) in args.iter_mut().enumerate() {
        *arg = values[17 - k]?;
    }
    Some(ArtFog {
        start_dist: args[0],
        half_dist: args[1],
        half_height: args[2],
        base_height: args[3],
        color: [args[4], args[5], args[6]],
        scale: args[7],
        sun_color: [args[8], args[9], args[10]],
        sun_dir: [args[11], args[12], args[13]],
        sun_start_ang: args[14],
        sun_stop_ang: args[15],
        time: args[16],
        max_opacity: args[17],
    })
}
