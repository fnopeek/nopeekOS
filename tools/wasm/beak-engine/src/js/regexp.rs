//! Regulaere Ausdruecke: Muster-Parser und Rueckverfolgung.
//!
//! **Rueckverfolgung, nicht Automat.** Ein DFA waere schneller und koennte
//! nicht katastrophal werden — aber er kann keine Rueckwaertsverweise und
//! keine Umschau, und die Spezifikation ist selbst in Rueckverfolgung
//! formuliert (ES 22.2.2). Ein Motor, der `(a+)+b` in Sekunden statt in
//! Jahrtausenden beantwortet, aber `\1` nicht kennt, waere fuer eine echte
//! Seite der schlechtere Tausch.
//!
//! **Deshalb ein Schrittdeckel, von Anfang an.** Katastrophales
//! Backtracking ist dieselbe Falle wie die vier nativen Schleifen, die in
//! dieser Sitzung ohne Deckel gelaufen sind — nur dass hier die FREMDE SEITE
//! das Muster stellt. `(a+)+$` auf dreissig `a` sind ohne Deckel 2^30 Wege.
//!
//! **Zeichen, nicht UTF-16-Einheiten.** JS zaehlt in UTF-16; hier wird in
//! `char` gezaehlt. Fuer alles ausserhalb der Basisebene (Emoji) weichen die
//! Indizes ab. Bewusst und benannt, statt still falsch.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

#[derive(Clone, Copy, Default)]
pub struct Flags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub sticky: bool,
    pub unicode: bool,
}

impl Flags {
    pub fn parse(s: &str) -> Option<Flags> {
        let mut f = Flags::default();
        for c in s.chars() {
            match c {
                'g' => f.global = true,
                'i' => f.ignore_case = true,
                'm' => f.multiline = true,
                's' => f.dot_all = true,
                'y' => f.sticky = true,
                'u' | 'v' => f.unicode = true,
                'd' => {}
                _ => return None,
            }
        }
        Some(f)
    }
    pub fn as_string(&self) -> String {
        let mut s = String::new();
        if self.dot_all { }
        for (on, c) in [(self.dot_all, 's'), (self.global, 'g'), (self.ignore_case, 'i'),
                        (self.multiline, 'm'), (self.unicode, 'u'), (self.sticky, 'y')] {
            if on { s.push(c); }
        }
        // Die Reihenfolge ist festgelegt: d g i m s u v y.
        let mut out = String::new();
        for c in ['g', 'i', 'm', 's', 'u', 'y'] { if s.contains(c) { out.push(c); } }
        out
    }
}

#[derive(Clone)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Digit(bool), Word(bool), Space(bool),
}

#[derive(Clone)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class { neg: bool, items: Vec<ClassItem> },
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: u32, greedy: bool },
    Group { slot: Option<usize>, node: Box<Node> },
    Look { behind: bool, neg: bool, node: Box<Node> },
    Start,
    End,
    WordBoundary(bool),
    BackRef(usize),
}

pub struct Regex {
    root: Node,
    pub group_count: usize,
    pub names: Vec<(String, usize)>,
    pub flags: Flags,
    pub source: String,
}

struct P<'a> {
    c: &'a [char],
    i: usize,
    groups: usize,
    names: Vec<(String, usize)>,
    unicode: bool,
}

impl<'a> P<'a> {
    fn at(&self) -> Option<char> { self.c.get(self.i).copied() }
    fn eat(&mut self, ch: char) -> bool {
        if self.at() == Some(ch) { self.i += 1; true } else { false }
    }

    fn alternation(&mut self) -> Result<Node, &'static str> {
        let mut alts = vec![self.sequence()?];
        while self.eat('|') { alts.push(self.sequence()?); }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn sequence(&mut self) -> Result<Node, &'static str> {
        let mut items = Vec::new();
        while let Some(ch) = self.at() {
            if ch == '|' || ch == ')' { break; }
            items.push(self.quantified()?);
        }
        Ok(match items.len() { 0 => Node::Empty, 1 => items.pop().unwrap(), _ => Node::Seq(items) })
    }

    fn quantified(&mut self) -> Result<Node, &'static str> {
        let atom = self.atom()?;
        let (min, max) = match self.at() {
            Some('*') => { self.i += 1; (0, u32::MAX) }
            Some('+') => { self.i += 1; (1, u32::MAX) }
            Some('?') => { self.i += 1; (0, 1) }
            Some('{') => {
                // `{` ist nur dann ein Zaehler, wenn es auch einer ist —
                // sonst ein gewoehnliches Zeichen (`a{b}` ist gueltig).
                let save = self.i;
                self.i += 1;
                let lo = self.number();
                match (lo, self.at()) {
                    (Some(l), Some('}')) => { self.i += 1; (l, l) }
                    (Some(l), Some(',')) => {
                        self.i += 1;
                        let hi = self.number();
                        if self.eat('}') { (l, hi.unwrap_or(u32::MAX)) }
                        else { self.i = save; return Ok(atom); }
                    }
                    _ => { self.i = save; return Ok(atom); }
                }
            }
            _ => return Ok(atom),
        };
        if min > max { return Err("numbers out of order in {} quantifier"); }
        let greedy = !self.eat('?');
        Ok(Node::Repeat { node: Box::new(atom), min, max, greedy })
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.i;
        let mut v: u32 = 0;
        while let Some(c) = self.at() {
            let Some(d) = c.to_digit(10) else { break };
            v = v.saturating_mul(10).saturating_add(d);
            self.i += 1;
        }
        if self.i == start { None } else { Some(v) }
    }

    fn atom(&mut self) -> Result<Node, &'static str> {
        let Some(ch) = self.at() else { return Ok(Node::Empty) };
        self.i += 1;
        Ok(match ch {
            '^' => Node::Start,
            '$' => Node::End,
            '.' => Node::Any,
            '(' => {
                let slot;
                let mut look = None;
                if self.eat('?') {
                    match self.at() {
                        Some(':') => { self.i += 1; slot = None; }
                        Some('=') => { self.i += 1; slot = None; look = Some((false, false)); }
                        Some('!') => { self.i += 1; slot = None; look = Some((false, true)); }
                        Some('<') => {
                            self.i += 1;
                            match self.at() {
                                Some('=') => { self.i += 1; slot = None; look = Some((true, false)); }
                                Some('!') => { self.i += 1; slot = None; look = Some((true, true)); }
                                _ => {
                                    // Benannte Gruppe `(?<name>…)`.
                                    let mut name = String::new();
                                    while let Some(c) = self.at() {
                                        if c == '>' { break; }
                                        name.push(c); self.i += 1;
                                    }
                                    if !self.eat('>') { return Err("invalid group name"); }
                                    self.groups += 1;
                                    self.names.push((name, self.groups));
                                    slot = Some(self.groups);
                                }
                            }
                        }
                        _ => return Err("invalid group"),
                    }
                } else {
                    self.groups += 1;
                    slot = Some(self.groups);
                }
                let inner = self.alternation()?;
                if !self.eat(')') { return Err("unterminated group"); }
                match look {
                    Some((behind, neg)) => Node::Look { behind, neg, node: Box::new(inner) },
                    None => Node::Group { slot, node: Box::new(inner) },
                }
            }
            '[' => self.class()?,
            '\\' => self.escape()?,
            ')' => return Err("unmatched )"),
            c => Node::Char(c),
        })
    }

    fn class(&mut self) -> Result<Node, &'static str> {
        let neg = self.eat('^');
        let mut items = Vec::new();
        loop {
            let Some(c) = self.at() else { return Err("unterminated character class") };
            if c == ']' { self.i += 1; break; }
            self.i += 1;
            let lo = if c == '\\' { 
                match self.class_escape()? { Ok(ch) => ch, Err(item) => { items.push(item); continue } }
            } else { c };
            // Ein `-` gefolgt von etwas anderem als `]` macht einen Bereich.
            if self.at() == Some('-') && self.c.get(self.i + 1).copied() != Some(']') && self.c.get(self.i + 1).is_some() {
                self.i += 1;
                let hc = self.at().unwrap();
                self.i += 1;
                let hi = if hc == '\\' {
                    match self.class_escape()? { Ok(ch) => ch, Err(_) => return Err("invalid class range") }
                } else { hc };
                if (lo as u32) > (hi as u32) { return Err("range out of order in character class"); }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Ch(lo));
            }
        }
        Ok(Node::Class { neg, items })
    }

    /// In einer Klasse: entweder ein Zeichen (`Ok`) oder eine ganze Gruppe
    /// wie `\d` (`Err`, was hier kein Fehler ist sondern der andere Fall).
    fn class_escape(&mut self) -> Result<Result<char, ClassItem>, &'static str> {
        let Some(c) = self.at() else { return Err("trailing backslash") };
        self.i += 1;
        Ok(match c {
            'd' => Err(ClassItem::Digit(false)), 'D' => Err(ClassItem::Digit(true)),
            'w' => Err(ClassItem::Word(false)), 'W' => Err(ClassItem::Word(true)),
            's' => Err(ClassItem::Space(false)), 'S' => Err(ClassItem::Space(true)),
            'b' => Ok('\u{8}'),
            _ => Ok(self.simple_escape(c)?),
        })
    }

    fn simple_escape(&mut self, c: char) -> Result<char, &'static str> {
        Ok(match c {
            'n' => '\n', 't' => '\t', 'r' => '\r', 'f' => '\u{C}', 'v' => '\u{B}', '0' => '\0',
            'x' => {
                let mut v = 0u32;
                for _ in 0..2 {
                    let Some(d) = self.at().and_then(|x| x.to_digit(16)) else { return Ok('x') };
                    v = v * 16 + d; self.i += 1;
                }
                char::from_u32(v).unwrap_or('\u{FFFD}')
            }
            'u' => {
                if self.eat('{') {
                    let mut v = 0u32;
                    while let Some(d) = self.at().and_then(|x| x.to_digit(16)) { v = v * 16 + d; self.i += 1; }
                    if !self.eat('}') { return Err("invalid unicode escape"); }
                    char::from_u32(v).unwrap_or('\u{FFFD}')
                } else {
                    let mut v = 0u32;
                    for _ in 0..4 {
                        let Some(d) = self.at().and_then(|x| x.to_digit(16)) else { return Ok('u') };
                        v = v * 16 + d; self.i += 1;
                    }
                    char::from_u32(v).unwrap_or('\u{FFFD}')
                }
            }
            'c' => {
                match self.at() {
                    Some(l) if l.is_ascii_alphabetic() => { self.i += 1; ((l as u8) % 32) as char }
                    _ => 'c',
                }
            }
            other => other,
        })
    }

    fn escape(&mut self) -> Result<Node, &'static str> {
        let Some(c) = self.at() else { return Err("trailing backslash") };
        self.i += 1;
        Ok(match c {
            'd' => Node::Class { neg: false, items: vec![ClassItem::Digit(false)] },
            'D' => Node::Class { neg: false, items: vec![ClassItem::Digit(true)] },
            'w' => Node::Class { neg: false, items: vec![ClassItem::Word(false)] },
            'W' => Node::Class { neg: false, items: vec![ClassItem::Word(true)] },
            's' => Node::Class { neg: false, items: vec![ClassItem::Space(false)] },
            'S' => Node::Class { neg: false, items: vec![ClassItem::Space(true)] },
            'b' => Node::WordBoundary(false),
            'B' => Node::WordBoundary(true),
            'k' => {
                // Benannter Rueckverweis `\k<name>`.
                if !self.eat('<') { return Ok(Node::Char('k')); }
                let mut name = String::new();
                while let Some(x) = self.at() { if x == '>' { break; } name.push(x); self.i += 1; }
                self.eat('>');
                match self.names.iter().find(|(n, _)| *n == name) {
                    Some((_, idx)) => Node::BackRef(*idx),
                    None => Node::Empty,
                }
            }
            '1'..='9' => {
                self.i -= 1;
                let n = self.number().unwrap_or(0) as usize;
                Node::BackRef(n)
            }
            'p' | 'P' if self.unicode => {
                // Unicode-Eigenschaften: die Tabelle dafuer kostet Zehntausende
                // Zeichen und keine Seite des Zielkorpus benutzt sie. Als
                // FEHLER melden, nicht still als Zeichen lesen — ein Muster,
                // das etwas anderes tut als es sagt, ist schlimmer als eins,
                // das gar nicht laeuft.
                return Err("unicode property escapes are not supported");
            }
            other => Node::Char(self.simple_escape(other)?),
        })
    }
}

impl Regex {
    pub fn new(pattern: &str, flags_str: &str) -> Result<Regex, &'static str> {
        let flags = Flags::parse(flags_str).ok_or("invalid flags")?;
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = P { c: &chars, i: 0, groups: 0, names: Vec::new(), unicode: flags.unicode };
        let root = p.alternation()?;
        if p.i < chars.len() { return Err("unmatched ) in regular expression"); }
        Ok(Regex { root, group_count: p.groups, names: p.names, flags, source:
            if pattern.is_empty() { "(?:)".to_string() } else { pattern.to_string() } })
    }
}

/// Ein Treffer: Zeichenspannen je Gruppe, 0 = das Ganze.
pub struct Match {
    pub caps: Vec<Option<(usize, usize)>>,
}

struct St<'a> {
    s: &'a [char],
    f: Flags,
    caps: Vec<Option<(usize, usize)>>,
    steps: u32,
}

/// Wie viele Rueckverfolgungsschritte ein Treffer kosten darf.
///
/// Kein Zierrat: `(a+)+$` auf dreissig `a` sind 2^30 Wege, und das MUSTER
/// stellt die fremde Seite. Reisst der Deckel, gilt „kein Treffer" — falsch,
/// aber begrenzt falsch, und ein haengender Browser waere schlimmer.
const MAX_STEPS: u32 = 400_000;

fn is_word(c: char) -> bool { c.is_ascii_alphanumeric() || c == '_' }
fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{feff}' | '\u{a0}')
}
fn fold(c: char, f: Flags) -> char {
    if f.ignore_case { c.to_lowercase().next().unwrap_or(c) } else { c }
}

fn class_hit(items: &[ClassItem], neg: bool, c: char, f: Flags) -> bool {
    let mut hit = false;
    for it in items {
        let m = match it {
            ClassItem::Ch(x) => fold(*x, f) == fold(c, f),
            ClassItem::Range(a, b) => {
                if f.ignore_case {
                    let lc = fold(c, f);
                    let uc = c.to_uppercase().next().unwrap_or(c);
                    (*a..=*b).contains(&c) || (*a..=*b).contains(&lc) || (*a..=*b).contains(&uc)
                } else { (*a..=*b).contains(&c) }
            }
            ClassItem::Digit(n) => c.is_ascii_digit() != *n,
            ClassItem::Word(n) => is_word(c) != *n,
            ClassItem::Space(n) => is_space(c) != *n,
        };
        if m { hit = true; break; }
    }
    hit != neg
}

type K<'k> = &'k dyn Fn(&mut St, usize) -> Option<usize>;

fn m(n: &Node, st: &mut St, pos: usize, k: K) -> Option<usize> {
    st.steps += 1;
    if st.steps > MAX_STEPS { return None; }
    match n {
        Node::Empty => k(st, pos),
        Node::Char(c) => {
            let got = st.s.get(pos).copied()?;
            if fold(got, st.f) == fold(*c, st.f) { k(st, pos + 1) } else { None }
        }
        Node::Any => {
            let got = st.s.get(pos).copied()?;
            if !st.f.dot_all && matches!(got, '\n' | '\r' | '\u{2028}' | '\u{2029}') { return None; }
            k(st, pos + 1)
        }
        Node::Class { neg, items } => {
            let got = st.s.get(pos).copied()?;
            if class_hit(items, *neg, got, st.f) { k(st, pos + 1) } else { None }
        }
        Node::Start => {
            if pos == 0 { return k(st, pos); }
            if st.f.multiline && matches!(st.s.get(pos - 1), Some('\n' | '\r')) { return k(st, pos); }
            None
        }
        Node::End => {
            if pos == st.s.len() { return k(st, pos); }
            if st.f.multiline && matches!(st.s.get(pos), Some('\n' | '\r')) { return k(st, pos); }
            None
        }
        Node::WordBoundary(neg) => {
            let a = pos > 0 && is_word(st.s[pos - 1]);
            let b = pos < st.s.len() && is_word(st.s[pos]);
            if (a != b) != *neg { k(st, pos) } else { None }
        }
        Node::Seq(items) => seq(items, 0, st, pos, k),
        Node::Alt(alts) => {
            for a in alts {
                // Die Erfassungen der gescheiterten Alternative muessen weg,
                // sonst traegt der Treffer Spuren eines Weges, den er nicht
                // gegangen ist.
                let save = st.caps.clone();
                if let Some(e) = m(a, st, pos, k) { return Some(e); }
                st.caps = save;
            }
            None
        }
        Node::Group { slot, node } => {
            match slot {
                None => m(node, st, pos, k),
                Some(idx) => {
                    let idx = *idx;
                    let start = pos;
                    let inner = move |st: &mut St, e: usize| -> Option<usize> {
                        let prev = st.caps[idx];
                        st.caps[idx] = Some((start, e));
                        match k(st, e) { Some(x) => Some(x), None => { st.caps[idx] = prev; None } }
                    };
                    m(node, st, pos, &inner)
                }
            }
        }
        Node::Look { behind, neg, node } => {
            let ok = if !*behind {
                let save = st.caps.clone();
                let hit = m(node, st, pos, &|_, e| Some(e)).is_some();
                if *neg || !hit { st.caps = save; }
                hit
            } else {
                // Rueckschau: jede Startstelle davor probieren, die genau hier
                // endet. Naiv, aber richtig — und Rueckschau ist selten genug,
                // dass die Naivitaet nicht auffaellt.
                let save = st.caps.clone();
                let mut hit = false;
                for start in (0..=pos).rev() {
                    if m(node, st, start, &move |_, e| if e == pos { Some(e) } else { None }).is_some() {
                        hit = true; break;
                    }
                }
                if *neg || !hit { st.caps = save; }
                hit
            };
            if ok != *neg { k(st, pos) } else { None }
        }
        Node::BackRef(idx) => {
            let Some(&Some((a, b))) = st.caps.get(*idx) else { return k(st, pos) };
            let len = b - a;
            if pos + len > st.s.len() { return None; }
            for j in 0..len {
                if fold(st.s[a + j], st.f) != fold(st.s[pos + j], st.f) { return None; }
            }
            k(st, pos + len)
        }
        Node::Repeat { node, min, max, greedy } => {
            repeat(node, *min, *max, *greedy, st, pos, 0, k)
        }
    }
}

fn seq(items: &[Node], i: usize, st: &mut St, pos: usize, k: K) -> Option<usize> {
    if i == items.len() { return k(st, pos); }
    let rest = move |st: &mut St, p: usize| seq(items, i + 1, st, p, k);
    m(&items[i], st, pos, &rest)
}

fn repeat(node: &Node, min: u32, max: u32, greedy: bool, st: &mut St,
          pos: usize, done: u32, k: K) -> Option<usize> {
    st.steps += 1;
    if st.steps > MAX_STEPS { return None; }
    let can_more = done < max;
    let more = |st: &mut St| -> Option<usize> {
        if !can_more { return None; }
        let again = move |st: &mut St, p: usize| -> Option<usize> {
            // Ein Durchgang, der NICHTS verbraucht hat, wuerde ewig laufen —
            // `(a?)*` ist gueltiges JavaScript. Abbrechen, sobald die
            // Mindestzahl erreicht ist.
            if p == pos { return if done + 1 >= min { k(st, p) } else { None }; }
            repeat(node, min, max, greedy, st, p, done + 1, k)
        };
        m(node, st, pos, &again)
    };
    if done < min { return more(st); }
    if greedy {
        let save = st.caps.clone();
        if let Some(e) = more(st) { return Some(e); }
        st.caps = save;
        k(st, pos)
    } else {
        if let Some(e) = k(st, pos) { return Some(e); }
        more(st)
    }
}

impl Regex {
    /// Sucht ab `start`. `sticky` erzwingt einen Treffer GENAU dort.
    pub fn exec(&self, s: &[char], start: usize) -> Option<Match> {
        let last = if self.flags.sticky { start } else { s.len() };
        for at in start..=last {
            let mut st = St { s, f: self.flags, caps: vec![None; self.group_count + 1], steps: 0 };
            if let Some(end) = m(&self.root, &mut st, at, &|_, e| Some(e)) {
                st.caps[0] = Some((at, end));
                return Some(Match { caps: st.caps });
            }
            if self.flags.sticky { break; }
        }
        None
    }
}

// ── Die JS-Seite ────────────────────────────────────────────────────────────

use super::interp::{C, Interp, Realm};
use super::value::*;
use alloc::rc::Rc;

pub fn compiled(v: &Value) -> Option<Rc<Regex>> {
    match v { Value::Obj(o) => match &o.borrow().kind {
        ObjKind::Regex(r) => Some(r.clone()), _ => None }, _ => None }
}

/// Ein RegExp-Objekt aus Muster und Flaggen.
pub fn make(i: &mut Interp, pattern: &str, flags: &str) -> C<Value> {
    let re = match Regex::new(pattern, flags) {
        Ok(r) => Rc::new(r),
        Err(e) => return Err(i.throw_kind("SyntaxError",
            &alloc::format!("invalid regular expression: {e}"))),
    };
    let g = new_kind(Some(i.realm.regexp_proto.clone()), ObjKind::Regex(re.clone()));
    {
        let mut o = g.borrow_mut();
        // `lastIndex` ist SCHREIBBAR und gehoert dem Objekt, nicht dem
        // Prototyp — daran haengt, dass `g`-Suchen weiterlaufen.
        o.define("lastIndex", Prop { value: Some(Value::Num(0.0)), get: None, set: None,
            writable: true, enumerable: false, configurable: false });
        o.define("source", Prop::frozen(Value::str(&re.source)));
        o.define("flags", Prop::frozen(Value::string(re.flags.as_string())));
        o.define("global", Prop::frozen(Value::Bool(re.flags.global)));
        o.define("ignoreCase", Prop::frozen(Value::Bool(re.flags.ignore_case)));
        o.define("multiline", Prop::frozen(Value::Bool(re.flags.multiline)));
        o.define("sticky", Prop::frozen(Value::Bool(re.flags.sticky)));
        // `dotAll` fehlte, und genau daran ist Wikipedias
        // Vertraeglichkeitspruefung gescheitert (`/./.dotAll === false`) —
        // die Flagge gab es intern laengst, nur herausgereicht war sie nicht.
        o.define("dotAll", Prop::frozen(Value::Bool(re.flags.dot_all)));
        o.define("unicode", Prop::frozen(Value::Bool(re.flags.unicode)));
        o.define("hasIndices", Prop::frozen(Value::Bool(false)));
    }
    Ok(Value::Obj(g))
}

/// Ein Treffer als JS-Array — mit `index`, `input` und `groups` daran, so wie
/// `exec` es liefert.
fn match_result(i: &mut Interp, re: &Regex, chars: &[char], m: &Match, input: &str) -> Value {
    let mut items = Vec::new();
    for c in &m.caps {
        items.push(match c {
            Some((a, b)) => Value::string(chars[*a..*b].iter().collect::<String>()),
            None => Value::Undefined,
        });
    }
    let arr = i.new_array(items);
    if let Value::Obj(o) = &arr {
        let idx = m.caps[0].map(|(a, _)| a).unwrap_or(0);
        o.borrow_mut().define("index", Prop::data(Value::Num(idx as f64)));
        o.borrow_mut().define("input", Prop::data(Value::str(input)));
        let groups = if re.names.is_empty() { Value::Undefined } else {
            let g = new_obj(Some(i.realm.object_proto.clone()));
            for (name, slot) in &re.names {
                let v = match m.caps.get(*slot).and_then(|c| *c) {
                    Some((a, b)) => Value::string(chars[a..b].iter().collect::<String>()),
                    None => Value::Undefined,
                };
                g.borrow_mut().define(name, Prop::data(v));
            }
            Value::Obj(g)
        };
        o.borrow_mut().define("groups", Prop::data(groups));
    }
    arr
}

/// `exec` mit der `lastIndex`-Buchhaltung, die `g`/`y` verlangen.
fn do_exec(i: &mut Interp, this: &Value, s: &str) -> C<Value> {
    let Some(re) = compiled(this) else { return i.type_err("not a RegExp") };
    let chars: Vec<char> = s.chars().collect();
    let use_last = re.flags.global || re.flags.sticky;
    let start = if use_last {
        let li = i.get(this, "lastIndex")?;
        let n = i.to_number(&li)?;
        if n < 0.0 || n as usize > chars.len() {
            i.set(this, "lastIndex", Value::Num(0.0))?;
            return Ok(Value::Null);
        }
        n as usize
    } else { 0 };
    match re.exec(&chars, start) {
        Some(m) => {
            if use_last {
                let end = m.caps[0].map(|(_, b)| b).unwrap_or(start);
                i.set(this, "lastIndex", Value::Num(end as f64))?;
            }
            Ok(match_result(i, &re, &chars, &m, s))
        }
        None => {
            if use_last { i.set(this, "lastIndex", Value::Num(0.0))?; }
            Ok(Value::Null)
        }
    }
}

/// `$1`, `$&`, `$\`` und `$'` in einer Ersetzung auffuellen.
fn expand(rep: &str, chars: &[char], m: &Match) -> String {
    let mut out = String::new();
    let b: Vec<char> = rep.chars().collect();
    let mut k = 0;
    while k < b.len() {
        if b[k] != '$' || k + 1 >= b.len() { out.push(b[k]); k += 1; continue; }
        match b[k + 1] {
            '$' => { out.push('$'); k += 2; }
            '&' => { if let Some((a, e)) = m.caps[0] { out.extend(&chars[a..e]); } k += 2; }
            '`' => { if let Some((a, _)) = m.caps[0] { out.extend(&chars[..a]); } k += 2; }
            '\'' => { if let Some((_, e)) = m.caps[0] { out.extend(&chars[e..]); } k += 2; }
            d if d.is_ascii_digit() => {
                let mut n = d.to_digit(10).unwrap() as usize;
                let mut used = 2;
                if k + 2 < b.len() && b[k + 2].is_ascii_digit() {
                    let two = n * 10 + b[k + 2].to_digit(10).unwrap() as usize;
                    if two < m.caps.len() { n = two; used = 3; }
                }
                if n > 0 && n < m.caps.len() {
                    if let Some((a, e)) = m.caps[n] { out.extend(&chars[a..e]); }
                    k += used;
                } else { out.push('$'); k += 1; }
            }
            _ => { out.push('$'); k += 1; }
        }
    }
    out
}

/// Alle Treffer einer Suche — die Grundlage von `match`, `replace` und
/// `split`. Ein LEERER Treffer muss die Stelle weiterschieben, sonst laeuft
/// die Schleife ewig (`"abc".replace(/x*/g, "-")`).
fn all_matches(re: &Regex, chars: &[char]) -> Vec<Match> {
    let mut out = Vec::new();
    let mut at = 0;
    while at <= chars.len() {
        let Some(m) = re.exec(chars, at) else { break };
        let (a, b) = m.caps[0].unwrap_or((at, at));
        out.push(m);
        at = if b > a { b } else { b + 1 };
        if !re.flags.global { break; }
    }
    out
}

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let proto = new_obj(Some(realm.object_proto.clone()));

    let def = |o: &Gc, name: &str, f: NativeFn, len: usize| {
        let g = native(Some(fp.clone()), f, name, len, false);
        o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
    };

    def(&proto, "exec", |i, t, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        do_exec(i, &t, &s)
    }, 1);
    def(&proto, "test", |i, t, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Bool(!matches!(do_exec(i, &t, &s)?, Value::Null)))
    }, 1);
    def(&proto, "toString", |i, t, _| {
        let src = i.get(&t, "source")?;
        let fl = i.get(&t, "flags")?;
        Ok(Value::string(alloc::format!("/{}/{}", i.to_string(&src)?, i.to_string(&fl)?)))
    }, 0);

    let ctor = native(Some(fp.clone()), |i, _, a| {
        let (pat, fl) = match a.first() {
            Some(v) if compiled(v).is_some() => {
                let r = compiled(v).unwrap();
                (r.source.clone(), match a.get(1) {
                    Some(Value::Undefined) | None => r.flags.as_string(),
                    Some(x) => i.to_string(x)?.to_string(),
                })
            }
            Some(v) => (i.to_string(v)?.to_string(), match a.get(1) {
                Some(Value::Undefined) | None => String::new(),
                Some(x) => i.to_string(x)?.to_string(),
            }),
            None => (String::new(), String::new()),
        };
        make(i, &pat, &fl)
    }, "RegExp", 2, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    realm.global.borrow_mut().define("RegExp", Prop::builtin(Value::Obj(ctor)));
    realm.regexp_proto = proto;

    // ── Die String-Methoden, die ein Muster nehmen ───────────────────────
    let sp = realm.string_proto.clone();
    def(&sp, "match", |i, t, a| {
        let s = i.to_string(&t)?;
        let chars: Vec<char> = s.chars().collect();
        let rev = as_regex(i, a.first())?;
        let Some(re) = compiled(&rev) else { return Ok(Value::Null) };
        if !re.flags.global {
            return do_exec(i, &rev, &s);
        }
        let ms = all_matches(&re, &chars);
        if ms.is_empty() { return Ok(Value::Null); }
        let items: Vec<Value> = ms.iter().map(|m| {
            let (a2, b) = m.caps[0].unwrap_or((0, 0));
            Value::string(chars[a2..b].iter().collect::<String>())
        }).collect();
        Ok(i.new_array(items))
    }, 1);
    def(&sp, "search", |i, t, a| {
        let s = i.to_string(&t)?;
        let chars: Vec<char> = s.chars().collect();
        let rev = as_regex(i, a.first())?;
        let Some(re) = compiled(&rev) else { return Ok(Value::Num(-1.0)) };
        Ok(match re.exec(&chars, 0) {
            Some(m) => Value::Num(m.caps[0].map(|(x, _)| x).unwrap_or(0) as f64),
            None => Value::Num(-1.0),
        })
    }, 1);
    def(&sp, "replace", |i, t, a| do_replace(i, t, a, false), 2);
    def(&sp, "replaceAll", |i, t, a| do_replace(i, t, a, true), 2);
    def(&sp, "split", |i, t, a| {
        let s = i.to_string(&t)?;
        let chars: Vec<char> = s.chars().collect();
        let sep = a.first().cloned().unwrap_or(Value::Undefined);
        if compiled(&sep).is_none() {
            // Zeichenkettenteilung — wie bisher.
            let parts: Vec<Value> = match &sep {
                Value::Undefined => alloc::vec![Value::Str(s.clone())],
                v => {
                    let p = i.to_string(v)?;
                    if p.is_empty() {
                        s.chars().map(|c| { let mut x = String::new(); x.push(c); Value::string(x) }).collect()
                    } else { s.split(&*p).map(Value::str).collect() }
                }
            };
            return Ok(i.new_array(parts));
        }
        let re = compiled(&sep).unwrap();
        let mut out = Vec::new();
        let mut last = 0usize;
        let mut at = 0usize;
        while at <= chars.len() {
            let Some(m) = re.exec(&chars, at) else { break };
            let (a2, b) = m.caps[0].unwrap_or((at, at));
            if b == a2 && a2 >= chars.len() { break; }
            if b == 0 && a2 == 0 { at = 1; continue; }
            out.push(Value::string(chars[last..a2].iter().collect::<String>()));
            // Erfasste Gruppen landen MIT in der Liste — das ist die Regel,
            // an der `"a1b".split(/(\d)/)` haengt.
            for c in m.caps.iter().skip(1) {
                out.push(match c { Some((x, y)) => Value::string(chars[*x..*y].iter().collect::<String>()),
                                   None => Value::Undefined });
            }
            last = b;
            at = if b > a2 { b } else { b + 1 };
        }
        out.push(Value::string(chars[last.min(chars.len())..].iter().collect::<String>()));
        Ok(i.new_array(out))
    }, 2);
}

/// Ein Argument als RegExp — eine Zeichenkette wird zu einem Muster.
fn as_regex(i: &mut Interp, v: Option<&Value>) -> C<Value> {
    match v {
        Some(x) if compiled(x).is_some() => Ok(x.clone()),
        Some(x) => { let s = i.to_string(x)?; make(i, &escape_literal(&s), "") }
        None => make(i, "(?:)", ""),
    }
}

/// Eine Zeichenkette, die als Muster genau sich selbst treffen soll.
fn escape_literal(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if "\\^$.|?*+()[]{}".contains(c) { o.push('\\'); }
        o.push(c);
    }
    o
}

fn do_replace(i: &mut Interp, t: Value, a: &[Value], all: bool) -> C<Value> {
    let s = i.to_string(&t)?;
    let chars: Vec<char> = s.chars().collect();
    let pat = a.first().cloned().unwrap_or(Value::Undefined);
    let rep = a.get(1).cloned().unwrap_or(Value::Undefined);

    // Zeichenkette als Muster: der einfache Fall, ohne Motor.
    if compiled(&pat).is_none() {
        let p = i.to_string(&pat)?;
        let mut out = String::new();
        let mut rest: &str = &s;
        loop {
            let Some(k) = rest.find(&*p) else { break };
            out.push_str(&rest[..k]);
            if i.is_callable(&rep) {
                let idx = s.len() - rest.len() + k;
                let r = i.call(&rep, Value::Undefined,
                    &[Value::Str(p.clone()), Value::Num(idx as f64), Value::Str(s.clone())])?;
                out.push_str(&i.to_string(&r)?);
            } else {
                out.push_str(&i.to_string(&rep)?);
            }
            rest = &rest[k + p.len()..];
            if !all || p.is_empty() { break; }
        }
        out.push_str(rest);
        return Ok(Value::string(out));
    }

    let re = compiled(&pat).unwrap();
    let global = all || re.flags.global;
    let mut out = String::new();
    let mut last = 0usize;
    let mut at = 0usize;
    while at <= chars.len() {
        let Some(m) = re.exec(&chars, at) else { break };
        let (a2, b) = m.caps[0].unwrap_or((at, at));
        out.extend(&chars[last..a2]);
        if i.is_callable(&rep) {
            // Der Ersetzer bekommt Treffer, Gruppen, Stelle und Text.
            let mut args: Vec<Value> = m.caps.iter().map(|c| match c {
                Some((x, y)) => Value::string(chars[*x..*y].iter().collect::<String>()),
                None => Value::Undefined,
            }).collect();
            args.push(Value::Num(a2 as f64));
            args.push(Value::Str(s.clone()));
            let r = i.call(&rep, Value::Undefined, &args)?;
            out.push_str(&i.to_string(&r)?);
        } else {
            let rs = i.to_string(&rep)?;
            out.push_str(&expand(&rs, &chars, &m));
        }
        last = b;
        at = if b > a2 { b } else { b + 1 };
        if !global { break; }
        if b == a2 && a2 >= chars.len() { break; }
    }
    out.extend(&chars[last.min(chars.len())..]);
    Ok(Value::string(out))
}
