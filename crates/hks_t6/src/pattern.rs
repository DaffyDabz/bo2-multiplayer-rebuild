//! Lua 5.1 string patterns, for `string.find` / `match` / `gmatch` /
//! `gsub`: character classes (`%a %c %d %l %p %s %u %w %x %z`, upper case
//! negates), `.`, sets (`[a-z%d]`, `[^...]`), the quantifiers `* + - ?`,
//! anchors `^` and `$`, captures (`(...)`, position `()`), `%b()`, `%f[set]`
//! and back references `%1`..`%9`. Matching is on bytes.

const MAX_CAPTURES: usize = 32;
const CAP_UNFINISHED: isize = -1;
const CAP_POSITION: isize = -2;
/// Recursion bound (a pattern like `(.-)*` on a long subject).
const MAX_DEPTH: usize = 200;

/// A capture: a span of the subject, or a position (1-based, as Lua gives).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    Span(usize, usize),
    Position(usize),
}

pub struct Matcher<'a> {
    s: &'a [u8],
    p: &'a [u8],
    level: usize,
    caps: [(usize, isize); MAX_CAPTURES],
    depth: usize,
    pub error: Option<String>,
}

fn class_matches(c: u8, class: u8) -> bool {
    let hit = match class.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => c.is_ascii_whitespace() || c == 0x0b,
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'z' => c == 0,
        _ => return class == c,
    };
    if class.is_ascii_uppercase() { !hit } else { hit }
}

impl<'a> Matcher<'a> {
    pub fn new(s: &'a [u8], p: &'a [u8]) -> Self {
        Matcher {
            s,
            p,
            level: 0,
            caps: [(0, 0); MAX_CAPTURES],
            depth: 0,
            error: None,
        }
    }

    fn fail(&mut self, msg: &str) -> Option<usize> {
        if self.error.is_none() {
            self.error = Some(msg.to_owned());
        }
        None
    }

    /// The end of the single-character class at `p`.
    fn class_end(&mut self, mut p: usize) -> Option<usize> {
        let c = self.p[p];
        p += 1;
        if c == b'%' {
            if p >= self.p.len() {
                return self.fail("malformed pattern (ends with '%')");
            }
            return Some(p + 1);
        }
        if c == b'[' {
            if p < self.p.len() && self.p[p] == b'^' {
                p += 1;
            }
            // The first character is always part of the set (so `[]]`).
            loop {
                if p >= self.p.len() {
                    return self.fail("malformed pattern (missing ']')");
                }
                let cc = self.p[p];
                p += 1;
                if cc == b'%' && p < self.p.len() {
                    p += 1;
                }
                if p < self.p.len() && self.p[p] == b']' {
                    return Some(p + 1);
                }
                if p >= self.p.len() {
                    return self.fail("malformed pattern (missing ']')");
                }
            }
        }
        Some(p)
    }

    /// `c` against the set from `[` at `p` to its `]` at `ec`.
    fn bracket_matches(&self, c: u8, mut p: usize, ec: usize) -> bool {
        let mut sig = true;
        if self.p.get(p + 1) == Some(&b'^') {
            sig = false;
            p += 1;
        }
        p += 1;
        while p < ec {
            if self.p[p] == b'%' {
                p += 1;
                if p < ec && class_matches(c, self.p[p]) {
                    return sig;
                }
            } else if self.p.get(p + 1) == Some(&b'-') && p + 2 < ec {
                if self.p[p] <= c && c <= self.p[p + 2] {
                    return sig;
                }
                p += 2;
            } else if self.p[p] == c {
                return sig;
            }
            p += 1;
        }
        !sig
    }

    fn single_matches(&self, c: u8, p: usize, ep: usize) -> bool {
        match self.p[p] {
            b'.' => true,
            b'%' => class_matches(c, self.p[p + 1]),
            b'[' => self.bracket_matches(c, p, ep - 1),
            other => other == c,
        }
    }

    /// Match the pattern from `p` against the subject from `s`; the end of
    /// the match.
    pub fn do_match(&mut self, s: usize, p: usize) -> Option<usize> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return self.fail("pattern too complex");
        }
        let r = self.match_here(s, p);
        self.depth -= 1;
        r
    }

    fn match_here(&mut self, mut s: usize, mut p: usize) -> Option<usize> {
        let (slen, plen) = (self.s.len(), self.p.len());
        loop {
            if self.error.is_some() {
                return None;
            }
            if p == plen {
                return Some(s);
            }
            match self.p[p] {
                b'(' => {
                    return if self.p.get(p + 1) == Some(&b')') {
                        self.start_capture(s, p + 2, CAP_POSITION)
                    } else {
                        self.start_capture(s, p + 1, CAP_UNFINISHED)
                    };
                }
                b')' => return self.end_capture(s, p + 1),
                b'$' if p + 1 == plen => return (s == slen).then_some(s),
                b'%' if p + 1 < plen => match self.p[p + 1] {
                    b'b' => {
                        s = self.match_balance(s, p + 2)?;
                        p += 4;
                        continue;
                    }
                    b'f' => {
                        p += 2;
                        if self.p.get(p) != Some(&b'[') {
                            return self.fail("missing '[' after '%f' in pattern");
                        }
                        let ep = self.class_end(p)?;
                        let prev = if s == 0 { 0 } else { self.s[s - 1] };
                        let cur = self.s.get(s).copied().unwrap_or(0);
                        if !self.bracket_matches(prev, p, ep - 1) && self.bracket_matches(cur, p, ep - 1) {
                            p = ep;
                            continue;
                        }
                        return None;
                    }
                    d if d.is_ascii_digit() => {
                        s = self.match_capture(s, d)?;
                        p += 2;
                        continue;
                    }
                    _ => {}
                },
                _ => {}
            }
            let ep = self.class_end(p)?;
            let m = s < slen && self.single_matches(self.s[s], p, ep);
            if ep < plen {
                match self.p[ep] {
                    b'?' => {
                        if m && let Some(r) = self.do_match(s + 1, ep + 1) {
                            return Some(r);
                        }
                        p = ep + 1;
                        continue;
                    }
                    b'*' => return self.max_expand(s, p, ep),
                    b'+' => return if m { self.max_expand(s + 1, p, ep) } else { None },
                    b'-' => return self.min_expand(s, p, ep),
                    _ => {}
                }
            }
            if !m {
                return None;
            }
            s += 1;
            p = ep;
        }
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> Option<usize> {
        let mut i = 0;
        while s + i < self.s.len() && self.single_matches(self.s[s + i], p, ep) {
            i += 1;
        }
        loop {
            if let Some(r) = self.do_match(s + i, ep + 1) {
                return Some(r);
            }
            if i == 0 || self.error.is_some() {
                return None;
            }
            i -= 1;
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> Option<usize> {
        loop {
            if let Some(r) = self.do_match(s, ep + 1) {
                return Some(r);
            }
            if self.error.is_none() && s < self.s.len() && self.single_matches(self.s[s], p, ep) {
                s += 1;
            } else {
                return None;
            }
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, what: isize) -> Option<usize> {
        if self.level >= MAX_CAPTURES {
            return self.fail("too many captures");
        }
        self.caps[self.level] = (s, what);
        self.level += 1;
        let r = self.do_match(s, p);
        if r.is_none() {
            self.level -= 1;
        }
        r
    }

    fn end_capture(&mut self, s: usize, p: usize) -> Option<usize> {
        let Some(l) = (0..self.level).rev().find(|&i| self.caps[i].1 == CAP_UNFINISHED) else {
            return self.fail("invalid pattern capture");
        };
        self.caps[l].1 = (s - self.caps[l].0) as isize;
        let r = self.do_match(s, p);
        if r.is_none() {
            self.caps[l].1 = CAP_UNFINISHED;
        }
        r
    }

    fn match_balance(&mut self, s: usize, p: usize) -> Option<usize> {
        if p + 1 >= self.p.len() {
            return self.fail("unbalanced pattern");
        }
        if s >= self.s.len() || self.s[s] != self.p[p] {
            return None;
        }
        let (b, e) = (self.p[p], self.p[p + 1]);
        let mut depth = 1;
        for i in s + 1..self.s.len() {
            let c = self.s[i];
            if c == e {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            } else if c == b {
                depth += 1;
            }
        }
        None
    }

    fn match_capture(&mut self, s: usize, d: u8) -> Option<usize> {
        if d < b'1' {
            return self.fail("invalid capture index");
        }
        let l = usize::from(d - b'1');
        if l >= self.level || self.caps[l].1 == CAP_UNFINISHED {
            return self.fail("invalid capture index");
        }
        let (start, len) = self.caps[l];
        let len = len.max(0) as usize;
        if self.s.len() - s >= len && self.s[start..start + len] == self.s[s..s + len] {
            Some(s + len)
        } else {
            None
        }
    }

    /// Capture `i` of a match from `s` to `e` (no captures: the whole
    /// match is capture 0).
    pub fn capture(&self, i: usize, s: usize, e: usize) -> Capture {
        if i >= self.level {
            return Capture::Span(s, e);
        }
        let (start, len) = self.caps[i];
        if len == CAP_POSITION {
            Capture::Position(start + 1)
        } else {
            Capture::Span(start, start + len.max(0) as usize)
        }
    }

    /// Every capture of a match (the whole match if it has none).
    pub fn captures(&self, s: usize, e: usize) -> Vec<Capture> {
        let n = self.level.max(1);
        (0..n).map(|i| self.capture(i, s, e)).collect()
    }

    /// Only the pattern's own captures (`string.find`'s extra results).
    pub fn own_captures(&self, s: usize, e: usize) -> Vec<Capture> {
        (0..self.level).map(|i| self.capture(i, s, e)).collect()
    }

    pub fn reset(&mut self) {
        self.level = 0;
        self.depth = 0;
    }
}

/// The first match at or after `init` (`^` anchors it): (start, end,
/// matcher holding the captures).
pub fn find<'a>(s: &'a [u8], p: &'a [u8], init: usize) -> Result<Option<(usize, usize, Matcher<'a>)>, String> {
    let (anchor, p0) = if p.first() == Some(&b'^') { (true, 1) } else { (false, 0) };
    let mut m = Matcher::new(s, p);
    let mut s1 = init;
    loop {
        m.reset();
        if let Some(e) = m.do_match(s1, p0) {
            return Ok(Some((s1, e, m)));
        }
        if let Some(err) = m.error.take() {
            return Err(err);
        }
        s1 += 1;
        if anchor || s1 > s.len() {
            return Ok(None);
        }
    }
}

/// Whether a pattern uses any special character (else `find` is plain).
pub fn has_specials(p: &[u8]) -> bool {
    p.iter().any(|c| b"^$*+?.([%-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str, p: &str) -> Option<Vec<String>> {
        let (st, e, mm) = find(s.as_bytes(), p.as_bytes(), 0).unwrap()?;
        Some(
            mm.captures(st, e)
                .into_iter()
                .map(|c| match c {
                    Capture::Span(a, b) => s[a..b].to_owned(),
                    Capture::Position(n) => n.to_string(),
                })
                .collect(),
        )
    }

    #[test]
    fn lua_patterns() {
        assert_eq!(m("hello world", "o w"), Some(vec!["o w".into()]));
        assert_eq!(m("key = value", "(%w+)%s*=%s*(%w+)"), Some(vec!["key".into(), "value".into()]));
        assert_eq!(m("  trim  ", "^%s*(.-)%s*$"), Some(vec!["trim".into()]));
        assert_eq!(m("abc", "^b"), None);
        assert_eq!(m("f(a(b)c)d", "%b()"), Some(vec!["(a(b)c)".into()]));
        assert_eq!(m("THE (quick) fox", "%f[%a]%a+"), Some(vec!["THE".into()]));
        assert_eq!(m("hello", "()ll()"), Some(vec!["3".into(), "5".into()]));
        assert_eq!(m("abcabc", "(abc)%1"), Some(vec!["abc".into()]));
        assert_eq!(m("x]y", "[]]"), Some(vec!["]".into()]));
        assert_eq!(m("a-b", "[%-]"), Some(vec!["-".into()]));
        assert_eq!(m("price: 12.50", "%d+%.?%d*"), Some(vec!["12.50".into()]));
        assert_eq!(m("map_name", "[^_]+$"), Some(vec!["name".into()]));
    }
}
