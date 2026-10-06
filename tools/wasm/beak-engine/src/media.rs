//! Media Queries Level 4 and the size features of container queries.
//!
//! One condition grammar serves `@media`, `matchMedia`, `<source media>` and
//! `@container` (css-contain-3 §6.1 reuses it). Evaluation is three-valued
//! (MQ4 §3.2): an unknown feature, an unknown value or general-enclosed
//! syntax is "unknown", `not unknown` stays unknown, and a query that ends
//! unknown does not match. A malformed query is `not all` and does not
//! affect its comma siblings.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Num(f32, String),
    /// A whole function token with its balanced arguments, e.g. `calc(…)`.
    Func(String),
    LParen,
    RParen,
    Colon,
    Comma,
    Slash,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Bad,
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c >= 0x80
}

fn tokenize(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let digit_at = |k: usize| k < b.len() && b[k].is_ascii_digit();
        let num_start = c.is_ascii_digit()
            || (c == b'.' && digit_at(i + 1))
            || ((c == b'+' || c == b'-') && (digit_at(i + 1) || (i + 2 < b.len() && b[i + 1] == b'.' && digit_at(i + 2))));
        if num_start {
            let st = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i + 1 < b.len() && (b[i] | 0x20) == b'e' && (b[i + 1].is_ascii_digit()
                || ((b[i + 1] == b'+' || b[i + 1] == b'-') && digit_at(i + 2))) {
                i += 2;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let n: f32 = s[st..i].parse().unwrap_or(f32::NAN);
            let us = i;
            if i < b.len() && b[i] == b'%' {
                i += 1;
            } else {
                while i < b.len() && is_name(b[i]) {
                    i += 1;
                }
            }
            out.push(Tok::Num(n, s[us..i].to_ascii_lowercase()));
            continue;
        }
        let ident_start = is_name_start(c)
            || (c == b'-' && i + 1 < b.len() && (is_name_start(b[i + 1]) || b[i + 1] == b'-'));
        if ident_start {
            let st = i;
            while i < b.len() && is_name(b[i]) {
                i += 1;
            }
            if i < b.len() && b[i] == b'(' {
                let mut depth = 0;
                while i < b.len() {
                    match b[i] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                out.push(Tok::Func(String::from(&s[st..i])));
            } else {
                out.push(Tok::Ident(String::from(&s[st..i])));
            }
            continue;
        }
        i += 1;
        out.push(match c {
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b':' => Tok::Colon,
            b',' => Tok::Comma,
            b'/' => Tok::Slash,
            b'=' => Tok::Eq,
            b'<' | b'>' => {
                let eq = i < b.len() && b[i] == b'=';
                if eq {
                    i += 1;
                }
                match (c, eq) {
                    (b'<', false) => Tok::Lt,
                    (b'<', true) => Tok::Le,
                    (_, false) => Tok::Gt,
                    _ => Tok::Ge,
                }
            }
            _ => Tok::Bad,
        });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl Op {
    fn of(t: &Tok) -> Option<Op> {
        Some(match t {
            Tok::Lt => Op::Lt,
            Tok::Le => Op::Le,
            Tok::Gt => Op::Gt,
            Tok::Ge => Op::Ge,
            Tok::Eq => Op::Eq,
            _ => return None,
        })
    }

    /// The same comparison with the operands swapped.
    fn flip(self) -> Op {
        match self {
            Op::Lt => Op::Gt,
            Op::Le => Op::Ge,
            Op::Gt => Op::Lt,
            Op::Ge => Op::Le,
            Op::Eq => Op::Eq,
        }
    }

    fn test(self, a: f32, b: f32) -> bool {
        // Inequalities are exact: pages write `calc(1012px - 0.02px)` to
        // make two ranges meet without overlapping.
        match self {
            Op::Lt => a < b,
            Op::Le => a <= b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
            Op::Eq => (a - b).abs() <= 1e-4 * b.abs().max(1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Num(f32, String),
    Ratio(f32, f32),
    Ident(String),
    Calc(String),
}

impl Val {
    fn parse(t: &[Tok]) -> Option<Val> {
        match t {
            [Tok::Num(n, u)] => Some(Val::Num(*n, u.clone())),
            [Tok::Num(a, u1), Tok::Slash, Tok::Num(b, u2)] if u1.is_empty() && u2.is_empty() => {
                Some(Val::Ratio(*a, *b))
            }
            [Tok::Func(f)] => Some(Val::Calc(f.clone())),
            [Tok::Ident(s)] => Some(Val::Ident(s.to_ascii_lowercase())),
            _ => None,
        }
    }

    /// A `<length>` in px. `em`/`rem` are the initial font size (MQ4 §1.3);
    /// a unitless zero is a length.
    pub fn px(&self) -> Option<f32> {
        match self {
            Val::Num(n, u) => length_px(*n, u),
            Val::Calc(f) => crate::css::parse_media_px(f),
            _ => None,
        }
    }

    fn ratio(&self) -> Option<f32> {
        match self {
            Val::Num(n, u) if u.is_empty() => Some(*n),
            Val::Ratio(a, b) => Some(if *b == 0.0 { f32::INFINITY } else { a / b }),
            _ => None,
        }
    }

    /// A `<resolution>` in dppx.
    fn dppx(&self) -> Option<f32> {
        match self {
            Val::Num(n, u) => match u.as_str() {
                "dppx" | "x" => Some(*n),
                "dpi" => Some(n / 96.0),
                "dpcm" => Some(n * 2.54 / 96.0),
                _ => None,
            },
            _ => None,
        }
    }

    fn number(&self) -> Option<f32> {
        match self {
            Val::Num(n, u) if u.is_empty() => Some(*n),
            _ => None,
        }
    }

    fn ident(&self) -> Option<&str> {
        match self {
            Val::Ident(s) => Some(s),
            _ => None,
        }
    }
}

fn length_px(n: f32, unit: &str) -> Option<f32> {
    const FONT: f32 = 16.0;
    Some(match unit {
        "px" => n,
        "" if n == 0.0 => 0.0,
        "em" | "rem" => n * FONT,
        "in" => n * 96.0,
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "q" => n * 96.0 / 101.6,
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 16.0,
        _ => return None,
    })
}

/// What a feature test asks.
#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    /// `(name)` — boolean context.
    Bool,
    /// `(name: value)` on a discrete feature, or on a range feature (equality).
    Plain(Val),
    /// One or two comparisons with the feature on the left.
    Range(Vec<(Op, Val)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub name: String,
    pub test: Test,
}

impl Feature {
    fn parse(t: &[Tok]) -> Option<Feature> {
        match t {
            [Tok::Ident(n)] => return Some(Feature { name: n.to_ascii_lowercase(), test: Test::Bool }),
            [Tok::Ident(n), Tok::Colon, rest @ ..] => {
                let n = n.to_ascii_lowercase();
                let v = Val::parse(rest)?;
                // `min-`/`max-` are range comparisons spelled as a plain
                // feature, `-webkit-min-device-pixel-ratio` included.
                for (pre, op) in [("min-", Op::Ge), ("max-", Op::Le)] {
                    if let Some(base) = n.strip_prefix(pre) {
                        return Some(Feature { name: String::from(base), test: Test::Range(alloc::vec![(op, v)]) });
                    }
                    if let Some(base) = n.strip_prefix("-webkit-").and_then(|r| r.strip_prefix(pre)) {
                        let mut name = String::from("-webkit-");
                        name.push_str(base);
                        return Some(Feature { name, test: Test::Range(alloc::vec![(op, v)]) });
                    }
                }
                return Some(Feature { name: n, test: Test::Plain(v) });
            }
            _ => {}
        }
        let ops: Vec<usize> = t.iter().enumerate().filter(|(_, x)| Op::of(x).is_some()).map(|(i, _)| i).collect();
        match ops.as_slice() {
            &[k] => {
                let op = Op::of(&t[k])?;
                if let [Tok::Ident(n)] = &t[..k] {
                    let v = Val::parse(&t[k + 1..])?;
                    return Some(Feature { name: n.to_ascii_lowercase(), test: Test::Range(alloc::vec![(op, v)]) });
                }
                if let [Tok::Ident(n)] = &t[k + 1..] {
                    let v = Val::parse(&t[..k])?;
                    return Some(Feature { name: n.to_ascii_lowercase(), test: Test::Range(alloc::vec![(op.flip(), v)]) });
                }
                None
            }
            &[k1, k2] => {
                let (o1, o2) = (Op::of(&t[k1])?, Op::of(&t[k2])?);
                let less = |o: Op| matches!(o, Op::Lt | Op::Le);
                let more = |o: Op| matches!(o, Op::Gt | Op::Ge);
                if !((less(o1) && less(o2)) || (more(o1) && more(o2))) {
                    return None;
                }
                let [Tok::Ident(n)] = &t[k1 + 1..k2] else { return None };
                let v1 = Val::parse(&t[..k1])?;
                let v2 = Val::parse(&t[k2 + 1..])?;
                Some(Feature { name: n.to_ascii_lowercase(), test: Test::Range(alloc::vec![(o1.flip(), v1), (o2, v2)]) })
            }
            _ => None,
        }
    }
}

/// A media or container condition.
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
    Feat(Feature),
    /// General-enclosed syntax (`foo(…)`, `(foo bar)`), or a `style()` query.
    Unknown,
}

/// Kleene conjunction/disjunction over `Option<bool>` (`None` = unknown).
fn and3(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

fn or3(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// Answers feature tests for one evaluation context.
pub trait Env {
    fn feature(&self, f: &Feature) -> Option<bool>;
}

impl Cond {
    pub fn eval(&self, env: &dyn Env) -> Option<bool> {
        match self {
            Cond::And(v) => v.iter().fold(Some(true), |acc, c| and3(acc, c.eval(env))),
            Cond::Or(v) => v.iter().fold(Some(false), |acc, c| or3(acc, c.eval(env))),
            Cond::Not(c) => c.eval(env).map(|b| !b),
            Cond::Feat(f) => env.feature(f),
            Cond::Unknown => None,
        }
    }

    /// Does any feature test in here ask about `pred`?
    pub fn any_feature(&self, pred: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Cond::And(v) | Cond::Or(v) => v.iter().any(|c| c.any_feature(pred)),
            Cond::Not(c) => c.any_feature(pred),
            Cond::Feat(f) => pred(&f.name),
            Cond::Unknown => false,
        }
    }
}

struct Parser<'t> {
    t: &'t [Tok],
    i: usize,
}

impl Parser<'_> {
    fn peek_ident(&self, word: &str) -> bool {
        matches!(self.t.get(self.i), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(word))
    }

    fn condition(&mut self, allow_or: bool) -> Option<Cond> {
        if self.peek_ident("not") {
            self.i += 1;
            return Some(Cond::Not(Box::new(self.in_parens()?)));
        }
        let first = self.in_parens()?;
        let mut items = alloc::vec![first];
        let mut joiner: Option<&'static str> = None;
        loop {
            let word = if self.peek_ident("and") {
                "and"
            } else if self.peek_ident("or") {
                "or"
            } else {
                break;
            };
            if word == "or" && !allow_or {
                return None;
            }
            if joiner.is_some_and(|j| j != word) {
                return None;
            }
            joiner = Some(word);
            self.i += 1;
            items.push(self.in_parens()?);
        }
        Some(match joiner {
            None => items.pop().unwrap_or(Cond::Unknown),
            Some("and") => Cond::And(items),
            Some(_) => Cond::Or(items),
        })
    }

    fn in_parens(&mut self) -> Option<Cond> {
        match self.t.get(self.i)? {
            Tok::LParen => {
                let mut depth = 0;
                let mut j = self.i;
                while j < self.t.len() {
                    match self.t[j] {
                        Tok::LParen => depth += 1,
                        Tok::RParen => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if j >= self.t.len() {
                    return None;
                }
                let inner = &self.t[self.i + 1..j];
                self.i = j + 1;
                if matches!(inner.first(), Some(Tok::LParen)) || matches!(inner.first(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("not")) {
                    let mut p = Parser { t: inner, i: 0 };
                    if let Some(c) = p.condition(true) {
                        if p.i == inner.len() {
                            return Some(c);
                        }
                    }
                    return Some(Cond::Unknown);
                }
                Some(Feature::parse(inner).map_or(Cond::Unknown, Cond::Feat))
            }
            Tok::Func(_) => {
                self.i += 1;
                Some(Cond::Unknown)
            }
            _ => None,
        }
    }
}

/// Parse a whole condition (`@container`'s, or the part after a media type).
fn parse_condition(t: &[Tok], allow_or: bool) -> Option<Cond> {
    let mut p = Parser { t, i: 0 };
    let c = p.condition(allow_or)?;
    (p.i == t.len()).then_some(c)
}

/// One query of a media query list.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// `None` for a malformed query, which never matches.
    inner: Option<QueryBody>,
}

#[derive(Clone, Debug, PartialEq)]
struct QueryBody {
    negated: bool,
    /// Whether the media type matches; `true` when none is named.
    type_ok: bool,
    cond: Option<Cond>,
}

impl Query {
    fn parse(t: &[Tok]) -> Query {
        Query { inner: Self::body(t) }
    }

    fn body(t: &[Tok]) -> Option<QueryBody> {
        let mut i = 0;
        let mut negated = false;
        let word = |k: usize| match t.get(k) {
            Some(Tok::Ident(s)) => Some(s.to_ascii_lowercase()),
            _ => None,
        };
        match word(0).as_deref() {
            Some("not") if word(1).is_some() => {
                negated = true;
                i = 1;
            }
            Some("only") => i = 1,
            _ => {}
        }
        let Some(ty) = word(i) else {
            if i > 0 {
                return None;
            }
            return Some(QueryBody { negated: false, type_ok: true, cond: Some(parse_condition(t, true)?) });
        };
        if ty == "not" && i == 0 {
            return Some(QueryBody { negated: false, type_ok: true, cond: Some(parse_condition(t, true)?) });
        }
        let type_ok = match ty.as_str() {
            "all" | "screen" => true,
            "and" | "or" | "not" | "only" | "layer" => return None,
            _ => false,
        };
        i += 1;
        let cond = if i < t.len() {
            if word(i).as_deref() != Some("and") {
                return None;
            }
            Some(parse_condition(&t[i + 1..], false)?)
        } else {
            None
        };
        Some(QueryBody { negated, type_ok, cond })
    }

    fn eval(&self, env: &dyn Env) -> bool {
        let Some(b) = &self.inner else { return false };
        let mut r = Some(b.type_ok);
        if let Some(c) = &b.cond {
            r = and3(r, c.eval(env));
        }
        if b.negated {
            r = r.map(|x| !x);
        }
        r == Some(true)
    }
}

/// A comma-separated media query list. Empty matches everything.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MediaList {
    queries: Vec<Query>,
}

/// Split at top-level commas; a comma inside parentheses belongs to them.
fn split_commas(t: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let (mut depth, mut st) = (0i32, 0usize);
    for (k, x) in t.iter().enumerate() {
        match x {
            Tok::LParen => depth += 1,
            Tok::RParen => depth -= 1,
            Tok::Comma if depth == 0 => {
                out.push(&t[st..k]);
                st = k + 1;
            }
            _ => {}
        }
    }
    out.push(&t[st..]);
    out
}

impl MediaList {
    pub fn parse(prelude: &str) -> MediaList {
        let t = tokenize(prelude);
        if t.is_empty() {
            return MediaList::default();
        }
        MediaList { queries: split_commas(&t).into_iter().map(Query::parse).collect() }
    }

    pub fn matches(&self, env: &dyn Env) -> bool {
        self.queries.is_empty() || self.queries.iter().any(|q| q.eval(env))
    }

    /// Does any query test a feature named by `pred`?
    pub fn any_feature(&self, pred: &dyn Fn(&str) -> bool) -> bool {
        self.queries.iter().any(|q| {
            q.inner.as_ref().and_then(|b| b.cond.as_ref()).is_some_and(|c| c.any_feature(pred))
        })
    }
}

/// Nested `@media` blocks: every level has to match.
#[derive(Debug)]
pub struct MediaChain {
    pub list: MediaList,
    pub parent: Option<Rc<MediaChain>>,
}

impl MediaChain {
    pub fn matches(&self, env: &dyn Env) -> bool {
        self.list.matches(env) && self.parent.as_ref().is_none_or(|p| p.matches(env))
    }

    pub fn any_feature(&self, pred: &dyn Fn(&str) -> bool) -> bool {
        self.list.any_feature(pred) || self.parent.as_ref().is_some_and(|p| p.any_feature(pred))
    }
}

/// Range comparisons against a known value; equality for the plain form.
fn range(actual: Option<f32>, test: &Test, conv: fn(&Val) -> Option<f32>) -> Option<bool> {
    let a = actual?;
    match test {
        Test::Bool => Some(a != 0.0),
        Test::Plain(v) => Some(Op::Eq.test(a, conv(v)?)),
        Test::Range(cmps) => {
            let mut ok = true;
            for (op, v) in cmps {
                ok &= op.test(a, conv(v)?);
            }
            Some(ok)
        }
    }
}

/// A discrete feature: `yes` are the values that hold, `known` every value
/// the feature has, `boolean` its answer in boolean context.
fn discrete(test: &Test, yes: &[&str], known: &[&str], boolean: bool) -> Option<bool> {
    match test {
        Test::Bool => Some(boolean),
        Test::Plain(v) => {
            let s = v.ident()?;
            if yes.contains(&s) {
                Some(true)
            } else if known.contains(&s) {
                Some(false)
            } else {
                None
            }
        }
        Test::Range(_) => None,
    }
}

fn orientation(w: f32, h: Option<f32>, test: &Test) -> Option<bool> {
    let h = h?;
    let portrait = h >= w;
    discrete(test, &[if portrait { "portrait" } else { "landscape" }], &["portrait", "landscape"], true)
}

impl Env for crate::css::Media {
    fn feature(&self, f: &Feature) -> Option<bool> {
        let (w, h) = (self.width, self.height);
        let t = &f.test;
        match f.name.as_str() {
            "width" | "device-width" => range(Some(w), t, Val::px),
            "height" | "device-height" => range(h, t, Val::px),
            "aspect-ratio" | "device-aspect-ratio" => {
                range(h.filter(|h| *h > 0.0).map(|h| w / h), t, Val::ratio)
            }
            "orientation" => orientation(w, h, t),
            "resolution" => range(Some(1.0), t, Val::dppx),
            "-webkit-device-pixel-ratio" => range(Some(1.0), t, Val::number),
            "color" => range(Some(8.0), t, Val::number),
            "color-index" | "monochrome" => range(Some(0.0), t, Val::number),
            "grid" => range(Some(0.0), t, Val::number),
            "hover" | "any-hover" => discrete(t, &["hover"], &["none", "hover"], true),
            "pointer" | "any-pointer" => discrete(t, &["fine"], &["none", "coarse", "fine"], true),
            "prefers-reduced-motion" | "prefers-reduced-transparency" => {
                discrete(t, &["no-preference"], &["no-preference", "reduce"], false)
            }
            "prefers-reduced-data" => discrete(t, &["no-preference"], &["no-preference", "reduce"], false),
            "prefers-contrast" => discrete(t, &["no-preference"], &["no-preference", "more", "less", "custom"], false),
            "forced-colors" => discrete(t, &["none"], &["none", "active"], false),
            "inverted-colors" => discrete(t, &["none"], &["none", "inverted"], false),
            "prefers-color-scheme" => match t {
                Test::Plain(Val::Ident(s)) if s == "dark" => Some(self.dark),
                Test::Plain(Val::Ident(s)) if s == "light" => Some(!self.dark),
                _ => None,
            },
            "color-gamut" => discrete(t, &["srgb"], &["srgb", "p3", "rec2020"], true),
            "dynamic-range" | "video-dynamic-range" => discrete(t, &["standard"], &["standard", "high"], true),
            "display-mode" => discrete(t, &["browser"],
                &["browser", "fullscreen", "standalone", "minimal-ui", "picture-in-picture", "window-controls-overlay"], true),
            "scripting" => discrete(t, &["enabled"], &["enabled", "initial-only", "none"], true),
            "update" => discrete(t, &["fast"], &["fast", "slow", "none"], true),
            "overflow-block" => discrete(t, &["scroll"], &["none", "scroll", "paged"], true),
            "overflow-inline" => discrete(t, &["scroll"], &["none", "scroll"], true),
            _ => None,
        }
    }
}

/// Whether a media feature name reads the viewport height.
pub fn reads_height(name: &str) -> bool {
    matches!(name, "height" | "device-height" | "aspect-ratio" | "device-aspect-ratio" | "orientation")
}

/// `container-type`.
pub const CQ_NONE: u8 = 0;
pub const CQ_INLINE: u8 = 1;
pub const CQ_SIZE: u8 = 2;

/// FNV-1a over a container name; 0 is reserved for "no name".
pub fn name_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h.max(1)
}

/// A query container as the layout sees it while its descendants cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CqBox {
    pub seq: u32,
    pub kind: u8,
    pub names: [u32; 2],
    /// Content-box width.
    pub w: f32,
    /// Content-box height, when the container has size containment and a
    /// definite height.
    pub h: Option<f32>,
}

impl Env for CqBox {
    fn feature(&self, f: &Feature) -> Option<bool> {
        let t = &f.test;
        let h = if self.kind == CQ_SIZE { self.h } else { None };
        match f.name.as_str() {
            "width" | "inline-size" => range(Some(self.w), t, Val::px),
            "height" | "block-size" => range(h, t, Val::px),
            "aspect-ratio" => range(h.filter(|h| *h > 0.0).map(|h| self.w / h), t, Val::ratio),
            "orientation" => orientation(self.w, h, t),
            _ => None,
        }
    }
}

/// One `@container [name] <condition>`.
#[derive(Debug)]
pub struct ContainerQuery {
    name: u32,
    cond: Option<Cond>,
}

impl ContainerQuery {
    fn parse(t: &[Tok]) -> ContainerQuery {
        // Container names are case-sensitive custom identifiers.
        let (name, rest) = match t.first() {
            Some(Tok::Ident(s)) if !["not", "and", "or", "none"].iter().any(|r| s.eq_ignore_ascii_case(r)) => {
                (name_hash(s), &t[1..])
            }
            _ => (0, t),
        };
        ContainerQuery { name, cond: parse_condition(rest, true) }
    }

    /// The nearest container that can answer (css-contain-3 §6.1): it must
    /// carry the name if one is given, and have a type that covers the
    /// features asked about. No such container, or a malformed query: false.
    fn matches(&self, containers: &[CqBox]) -> bool {
        let Some(cond) = &self.cond else { return false };
        let needs_block = cond.any_feature(&|n| matches!(n, "height" | "block-size" | "aspect-ratio" | "orientation"));
        let found = containers.iter().rev().find(|c| {
            c.kind != CQ_NONE
                && (self.name == 0 || c.names.contains(&self.name))
                && (!needs_block || c.kind == CQ_SIZE)
        });
        match found {
            Some(c) => cond.eval(c) == Some(true),
            None => false,
        }
    }
}

/// `@container` blocks, nested ones chained. A comma list matches if any query does.
#[derive(Debug)]
pub struct ContainerChain {
    queries: Vec<ContainerQuery>,
    pub parent: Option<Rc<ContainerChain>>,
}

impl ContainerChain {
    pub fn parse(prelude: &str, parent: Option<Rc<ContainerChain>>) -> ContainerChain {
        let t = tokenize(prelude);
        let queries = split_commas(&t).into_iter().map(ContainerQuery::parse).collect();
        ContainerChain { queries, parent }
    }

    pub fn matches(&self, containers: &[CqBox]) -> bool {
        self.queries.iter().any(|q| q.matches(containers))
            && self.parent.as_ref().is_none_or(|p| p.matches(containers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::Media;

    fn m(q: &str) -> bool {
        MediaList::parse(q).matches(&Media::new(1902.0, false).with_height(993.0))
    }

    #[test]
    fn range_syntax_and_its_min_max_spellings_agree() {
        assert!(m("(width >= 544px)"));
        assert!(m("(width>=544px)"));
        assert!(m("screen and (width>=544px)"));
        assert!(m("(width<=100000px)"));
        assert!(m("(544px <= width <= 100000px)"));
        assert!(m("(400px <= width < 2000px)"));
        assert!(!m("(400px <= width < 700px)"));
        assert!(m("(min-width: 544px) and (max-width: 100000px)"));
        assert!(!m("(width <= calc(1012px - 0.02px))"));
        assert!(m("(width > 1901px)"));
        assert!(!m("(width < 1902px)"));
        assert!(m("(height >= 10px)"));
        assert!(m("(orientation: landscape)"));
        assert!(!m("(orientation: portrait)"));
        assert!(m("(min-aspect-ratio: 16/9)"));
    }

    #[test]
    fn feature_values_of_a_mouse_driven_screen() {
        assert!(m("(hover: hover)"));
        assert!(m("(hover)"));
        assert!(m("(any-pointer: fine)"));
        assert!(!m("(pointer: coarse)"));
        assert!(m("(prefers-reduced-motion: no-preference)"));
        assert!(!m("(prefers-reduced-motion)"));
        assert!(!m("(forced-colors: active)"));
        assert!(m("(forced-colors: none)"));
        assert!(m("(prefers-contrast: no-preference)"));
        assert!(m("(display-mode: browser)"));
        assert!(m("(scripting: enabled)"));
        assert!(m("(min-resolution: 1dppx)"));
        assert!(!m("(min-resolution: 2dppx)"));
        assert!(m("(-webkit-min-device-pixel-ratio: 1)"));
        assert!(m("(color)"));
        assert!(m("(prefers-color-scheme: light)"));
        assert!(!m("(prefers-color-scheme: dark)"));
    }

    #[test]
    fn logic_is_three_valued_and_errors_stay_local() {
        assert!(!m("(frobnicate: 1)"));
        assert!(!m("not (frobnicate: 1)"));
        assert!(m("(frobnicate: 1) or (width > 0)"));
        assert!(m("not (pointer: coarse)"));
        assert!(m("not print"));
        assert!(!m("print"));
        assert!(m("print, (width > 0)"));
        assert!(m("((hover: hover) and (pointer: fine))"));
        assert!(m("only screen and (min-width: 1px)"));
        assert!(!m("screen and (hover) or (width > 0)"));
        assert!(m("all and (orientation: landscape)"));
        assert!(m(""));
        assert!(!m("garbage !"));
        assert!(!m("(width > 0) and (width > 0) or (width > 0)"));
    }

    #[test]
    fn container_queries_ask_the_nearest_fitting_container() {
        let mk = |q: &str| ContainerChain::parse(q, None);
        let outer = CqBox { seq: 1, kind: CQ_INLINE, names: [name_hash("banner"), 0], w: 900.0, h: None };
        let inner = CqBox { seq: 2, kind: CQ_INLINE, names: [0, 0], w: 300.0, h: None };
        let both = [outer, inner];
        assert!(mk("(width <= 500px)").matches(&both));
        assert!(!mk("banner (width <= 500px)").matches(&both));
        assert!(mk("banner (width >= 500px)").matches(&both));
        assert!(!mk("Banner (width >= 500px)").matches(&both));
        assert!(!mk("(height > 0px)").matches(&both));
        assert!(!mk("(min-width: 1px)").matches(&[]));
        assert!(mk("(min-width: 1px)").matches(&both));
    }
}
