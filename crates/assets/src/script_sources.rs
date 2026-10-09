use std::collections::{BTreeMap, BTreeSet};

/// bo2zm M3: a Black Ops II map's own compiled scripts, string tables and
/// entity strings, for the BO2 script runtime.
#[derive(Clone, Debug, Default)]
pub struct T6ScriptSet {
    /// Compiled script objects in zone load order (later replace earlier).
    pub objects: Vec<Vec<u8>>,
    /// (name, columns, rows, cells row-major), later zones last.
    pub tables: Vec<(String, usize, usize, Vec<String>)>,
    pub entities: Vec<String>,
    /// The map's AI path nodes (GameWorldMp), in node order.
    pub path_nodes: Vec<asset_t6::PathNodeRef>,
    /// Every animation's timing, root motion and notetracks.
    pub anims: Vec<T6AnimFacts>,
    /// Animation state definitions (`animstatedefs/*.asd`): (name, text).
    pub animstatedefs: Vec<(String, String)>,
    /// The language zones' strings (`ZOMBIE_WEAPON_M14` -> its English
    /// text), later zones winning a name.
    pub strings: Vec<(String, String)>,
    /// Every sound alias the map's banks hold (lower case).
    pub sound_aliases: Vec<String>,
    /// bo2mp: the game type settings files (`mp/gamesettings_default.cfg`,
    /// `mp/gamesettings_tdm.cfg`, ...): (name, text), later zones last.
    pub gamesettings: Vec<(String, String)>,
    /// bo2mp: each sound alias's length in ms (lower-case name).
    pub sound_lengths: Vec<(String, u32)>,
    /// bo2mp: weapons whose weapon file marks them retrievable (bRetrievable).
    pub retrievable_weapons: Vec<String>,
    /// bo2mp: each vehicle's turret weapon and gunner weapons (the
    /// scorestreak helicopters' guns) and how it drives, later zones
    /// replacing earlier.
    pub vehicles: Vec<asset_t6::VehicleRef>,
    /// bo2mp: BO2's shellshock files (`shock/<name>.shock`): (name, text),
    /// later zones replacing earlier.
    pub shocks: Vec<(String, String)>,
    /// bo2mp: the third-person player animation script and its anim type
    /// list (`mp/playeranim.script`, `mp/playeranimtypes.txt`).
    pub playeranim: Option<(String, String)>,
}

/// bo2zm M3: what the server needs of an animation.
#[derive(Clone, Debug, Default)]
pub struct T6AnimFacts {
    pub name: String,
    pub numframes: u16,
    pub framerate: f32,
    pub looping: bool,
    /// Root translation keys: (frame, position).
    pub delta_trans: Vec<(u16, [f32; 3])>,
    /// Notetracks: (name, time 0..1).
    pub notifies: Vec<(String, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptSources {
    /// bo2zm M3: set for a Black Ops II map.
    pub t6: Option<T6ScriptSet>,
    sources: BTreeMap<String, Result<Vec<u8>, String>>,
    tables: BTreeMap<String, ScriptTable>,
    entities: Option<String>,
    configs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptTable {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<String>,
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

impl ScriptSources {
    pub fn read(&self, module: &str) -> Result<Vec<u8>, String> {
        self.sources
            .get(module)
            .cloned()
            .unwrap_or_else(|| Err(format!("missing script asset {module}.gsc")))
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub(crate) fn asset_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for source in self.sources.values().filter_map(|s| s.as_ref().ok()) {
            quoted_names(source, &mut names);
        }
        if let Some(entities) = &self.entities {
            quoted_names(entities.as_bytes(), &mut names);
        }
        for table in self.tables.values() {
            names.extend(table.cells.iter().cloned());
        }
        names
    }

    pub fn tables(&self) -> &BTreeMap<String, ScriptTable> {
        &self.tables
    }

    pub fn entities(&self) -> Option<&str> {
        self.entities.as_deref()
    }

    pub fn config(&self, name: &str) -> Option<&str> {
        self.configs.get(&normalize(name)).map(String::as_str)
    }

    pub fn shocks(&self) -> impl Iterator<Item = (&str, &str)> {
        self.configs.iter().filter_map(|(path, text)| {
            let name = path.strip_prefix("shock/")?.strip_suffix(".shock")?;
            Some((name, text.as_str()))
        })
    }

    pub(crate) fn capture(&mut self, name: &str, data: &[u8], compressed: bool) {
        let name = normalize(name);
        if name.ends_with(".cfg") || name.ends_with(".shock") || name == "radiant/keys.txt" {
            if let Some(text) = asset_world::decode_rawfile_text(data, compressed) {
                self.configs.insert(name, text);
            }
            return;
        }
        let Some(module) = name.strip_suffix(".gsc") else {
            return;
        };
        let source = if compressed {
            asset_transport::inflate_zlib(data).map_err(|e| e.to_string())
        } else {
            Ok(asset_world::decode_packed_rawfile(data).unwrap_or_else(|| data.to_vec()))
        }
        .map(|mut bytes| {
            while bytes.last() == Some(&0) {
                bytes.pop();
            }
            bytes
        });
        self.sources.insert(module.to_owned(), source);
    }

    pub(crate) fn capture_table(&mut self, table: &asset_game::CapturedStringTable) {
        self.tables.insert(
            normalize(&table.name),
            ScriptTable {
                columns: table.columns,
                rows: table.rows,
                cells: table.cells.clone(),
            },
        );
    }

    pub(crate) fn set_table_cells(
        &mut self,
        table: &str,
        key: &str,
        cells: &[(usize, String)],
    ) -> bool {
        let Some(table) = self.tables.get_mut(&normalize(table)) else {
            return false;
        };
        let Some(row) = table
            .cells
            .chunks_exact_mut(table.columns.max(1))
            .take(table.rows)
            .find(|row| row[0].eq_ignore_ascii_case(key))
        else {
            return false;
        };
        for (column, value) in cells {
            if let Some(cell) = row.get_mut(*column) {
                cell.clone_from(value);
            }
        }
        true
    }

    pub(crate) fn insert_source(&mut self, module: &str, source: String) {
        self.sources
            .insert(normalize(module), Ok(source.into_bytes()));
    }

    pub(crate) fn set_entities(&mut self, entities: String) {
        self.entities = Some(entities);
    }

    pub(crate) fn overlay(&mut self, other: Self) {
        self.sources.extend(other.sources);
        self.tables.extend(other.tables);
        self.configs.extend(other.configs);
        if other.entities.is_some() {
            self.entities = other.entities;
        }
        if other.t6.is_some() {
            self.t6 = other.t6;
        }
    }
}

fn quoted_names(bytes: &[u8], names: &mut BTreeSet<String>) {
    let mut at = 0;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1).copied()) {
            (b'/', Some(b'/')) => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            (b'/', Some(b'*')) => {
                at += 2;
                while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" {
                    at += 1;
                }
                at = (at + 2).min(bytes.len());
            }
            (b'"', _) => {
                at += 1;
                let start = at;
                while at < bytes.len() && bytes[at] != b'"' {
                    if bytes[at] == b'\\' && at + 1 < bytes.len() {
                        at += 1;
                    }
                    at += 1;
                }
                if let Ok(name) = std::str::from_utf8(&bytes[start..at]) {
                    names.insert(name.to_owned());
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
}
