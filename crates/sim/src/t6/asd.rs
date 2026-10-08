//! Animation state definitions (`animstatedefs/*.asd`, plain text in the
//! zones): states such as `zm_move_walk` or `zm_death`, each with flags
//! (`aliased` = named substates, `restart`, `missing_legs` = the crawler
//! variant) and the notify name its animations' notetracks are sent under,
//! then its substates: one animation per line, `name anim` when aliased.
//!
//! ```text
//! zm_walk_melee : restart notify melee_anim
//! {
//!     ai_zombie_attack_v2
//!     ai_zombie_attack_v4
//! }
//! ```

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub(crate) struct AnimState {
    pub name: String,
    pub aliased: bool,
    pub restart: bool,
    pub missing_legs: bool,
    /// Other flag words (`move_turn`, `attack_combat`, ...).
    pub flags: Vec<String>,
    /// Notetracks of this state's animations go out under this notify.
    pub notify: String,
    /// (substate name, animation); unnamed substates use their index.
    pub substates: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AnimStateDef {
    pub states: HashMap<String, AnimState>,
}

impl AnimStateDef {
    pub(crate) fn parse(text: &str) -> Self {
        let mut def = AnimStateDef::default();
        let mut cur: Option<AnimState> = None;
        for raw in text.lines() {
            let line = raw.split("//").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line == "{" {
                continue;
            }
            if line == "}" {
                if let Some(s) = cur.take() {
                    def.states.insert(s.name.clone(), s);
                }
                continue;
            }
            if let Some((name, flags)) = line.split_once(':') {
                if let Some(s) = cur.take() {
                    def.states.insert(s.name.clone(), s);
                }
                let mut st = AnimState {
                    name: name.trim().to_ascii_lowercase(),
                    ..Default::default()
                };
                let mut words = flags.split_whitespace();
                while let Some(w) = words.next() {
                    match w {
                        "aliased" => st.aliased = true,
                        "restart" => st.restart = true,
                        "missing_legs" => st.missing_legs = true,
                        "notify" => st.notify = words.next().unwrap_or("").to_owned(),
                        other => st.flags.push(other.to_owned()),
                    }
                }
                cur = Some(st);
                continue;
            }
            if let Some(st) = cur.as_mut() {
                let mut words = line.split_whitespace();
                let a = words.next().unwrap_or("").to_owned();
                match words.next() {
                    Some(anim) if st.aliased => {
                        st.substates.push((a.to_ascii_lowercase(), anim.to_owned()))
                    }
                    Some(anim) => st.substates.push((a.to_ascii_lowercase(), anim.to_owned())),
                    None => {
                        let i = st.substates.len().to_string();
                        st.substates.push((i, a));
                    }
                }
            }
        }
        if let Some(s) = cur.take() {
            def.states.insert(s.name.clone(), s);
        }
        def
    }

    /// The state for `name`, or its crawler twin (`<name>_crawl`) when the
    /// actor has lost its legs and the def has one.
    pub(crate) fn state(&self, name: &str, missing_legs: bool) -> Option<&AnimState> {
        let lname = name.to_ascii_lowercase();
        if missing_legs && let Some(s) = self.states.get(&format!("{lname}_crawl")) {
            return Some(s);
        }
        self.states.get(&lname)
    }
}
