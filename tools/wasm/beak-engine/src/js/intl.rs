//! `Intl.PluralRules`, `Intl.RelativeTimeFormat` and `Intl.Segmenter`.
//!
//! Locale data exists for English and German; every other locale resolves to
//! English, which is what a lookup without a match does (ECMA-402 §9.2.3).
//! Segment boundaries follow UAX #29 in simplified form and count code
//! points, like every string index in this engine.
//!
//! Not implemented: `selectRange`, number formatting options beyond grouping
//! and three fraction digits, word dictionaries for scripts without spaces.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

use super::interp::{C, Interp, Realm};
use super::value::*;

const LANG: &str = "\0!intl.lang";
const LOCALE: &str = "\0!intl.locale";
const OPT_A: &str = "\0!intl.a";
const OPT_B: &str = "\0!intl.b";
const SEG_LIST: &str = "\0!intl.segs";

pub fn install(realm: &mut Realm, intl: &Gc) {
    ctor(realm, intl, "PluralRules", "Intl.PluralRules", pr_new, &[
        ("select", pr_select, 1), ("resolvedOptions", pr_options, 0),
    ]);
    ctor(realm, intl, "RelativeTimeFormat", "Intl.RelativeTimeFormat", rtf_new, &[
        ("format", rtf_format, 2), ("formatToParts", rtf_parts, 2),
        ("resolvedOptions", rtf_options, 0),
    ]);
    ctor(realm, intl, "Segmenter", "Intl.Segmenter", seg_new, &[
        ("segment", seg_segment, 1), ("resolvedOptions", seg_options, 0),
    ]);
    // The `Segments` object `segment()` returns.
    let segs = new_obj(Some(realm.object_proto.clone()));
    method(realm, &segs, "containing", segs_containing, 1);
    method(realm, &segs, SYM_ITERATOR, |i, t, _| {
        let list = i.get(&t, SEG_LIST)?;
        i.array_iter(list, 0)
    }, 0);
    realm.intl_protos.insert("Segments", segs);
}

type Methods<'a> = &'a [(&'static str, NativeFn, usize)];

fn method(realm: &Realm, o: &Gc, name: &str, f: NativeFn, len: usize) {
    let shown = if is_sym_key(name) { "[Symbol.iterator]" } else { name };
    let g = native(Some(realm.function_proto.clone()), f, shown, len, false);
    o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
}

fn ctor(realm: &mut Realm, intl: &Gc, name: &'static str, tag: &'static str, f: NativeFn,
        methods: Methods) {
    let proto = new_obj(Some(realm.object_proto.clone()));
    proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str(tag)));
    for (n, m, len) in methods { method(realm, &proto, n, *m, *len); }
    let c = native(Some(realm.function_proto.clone()), f, name, 0, true);
    c.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    method(realm, &c, "supportedLocalesOf", supported_locales, 1);
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(c.clone())));
    intl.borrow_mut().define(name, Prop::builtin(Value::Obj(c)));
    realm.intl_protos.insert(name, proto);
}

/// The requested tags: `undefined`, one string, or a list.
fn requested(i: &mut Interp, v: &Value) -> C<Vec<String>> {
    Ok(match v {
        Value::Undefined => Vec::new(),
        Value::Str(s) => vec![String::from(&**s)],
        other => {
            let mut out = Vec::new();
            for x in i.elems(other)? { out.push(String::from(&*i.to_string(&x)?)); }
            out
        }
    })
}

/// The first requested tag whose language has data, else English.
fn resolve(i: &mut Interp, v: &Value) -> C<(&'static str, String)> {
    for t in requested(i, v)? {
        let lang = t.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
        match lang.as_str() {
            "en" => return Ok(("en", t)),
            "de" => return Ok(("de", t)),
            _ => {}
        }
    }
    Ok(("en", String::from("en-US")))
}

fn supported_locales(i: &mut Interp, _: Value, a: &[Value]) -> C<Value> {
    let tags = requested(i, a.first().unwrap_or(&Value::Undefined))?;
    let out: Vec<Value> = tags.into_iter()
        .filter(|t| matches!(t.split(['-', '_']).next().map(|l| l.to_ascii_lowercase()).as_deref(),
                             Some("en" | "de")))
        .map(Value::string).collect();
    Ok(i.new_array(out))
}

/// A string option out of `allowed`, `default` when absent (ECMA-402 GetOption).
fn option(i: &mut Interp, opts: &Value, key: &str, allowed: &[&'static str],
          default: &'static str) -> C<&'static str> {
    if !matches!(opts, Value::Obj(_)) { return Ok(default) }
    let v = i.get(opts, key)?;
    if matches!(v, Value::Undefined) { return Ok(default) }
    let s = i.to_string(&v)?;
    match allowed.iter().find(|a| ***a == *s) {
        Some(a) => Ok(a),
        None => i.range_err(&alloc::format!("{key}: invalid value {s}")),
    }
}

fn instance(i: &mut Interp, name: &'static str, a: &[Value], opts: [(&str, &str); 2])
    -> C<Value> {
    if !i.native_new { return i.type_err(&alloc::format!("Intl.{name} requires 'new'")) }
    let (lang, tag) = resolve(i, a.first().unwrap_or(&Value::Undefined))?;
    let proto = i.realm.intl_protos.get(name).cloned();
    let o = new_obj(proto);
    let mut b = o.borrow_mut();
    b.define(LANG, Prop::frozen(Value::str(lang)));
    b.define(LOCALE, Prop::frozen(Value::string(tag)));
    b.define(OPT_A, Prop::frozen(Value::str(opts[0].1)));
    b.define(OPT_B, Prop::frozen(Value::str(opts[1].1)));
    drop(b);
    Ok(Value::Obj(o))
}

/// An internal slot of `this`, or a TypeError for a foreign receiver.
fn slot(i: &mut Interp, t: &Value, key: &str, what: &str) -> C<String> {
    let v = match t {
        Value::Obj(o) => o.borrow().get_own(key).and_then(|p| p.value.clone()),
        _ => None,
    };
    match v {
        Some(Value::Str(s)) => Ok(String::from(&*s)),
        _ => i.type_err(&alloc::format!("{what} called on an incompatible receiver")),
    }
}

fn options_object(i: &mut Interp, pairs: Vec<(&str, Value)>) -> Value {
    let o = new_obj(Some(i.realm.object_proto.clone()));
    for (k, v) in pairs { o.borrow_mut().define(k, Prop::data(v)); }
    Value::Obj(o)
}

// ── PluralRules ──────────────────────────────────────────────────────────

fn pr_new(i: &mut Interp, _: Value, a: &[Value]) -> C<Value> {
    let opts = a.get(1).cloned().unwrap_or(Value::Undefined);
    let ty = option(i, &opts, "type", &["cardinal", "ordinal"], "cardinal")?;
    instance(i, "PluralRules", a, [("type", ty), ("", "")])
}

/// CLDR plural rules for `en` and `de` (cardinal: one = integer 1; ordinal
/// only in English: 1st, 2nd, 3rd, but 11th-13th).
fn plural(lang: &str, ordinal: bool, n: f64) -> &'static str {
    if !n.is_finite() { return "other" }
    let n = libm::fabs(n);
    let int = libm::trunc(n) == n;
    if !ordinal { return if int && n == 1.0 { "one" } else { "other" } }
    if lang != "en" || !int { return "other" }
    let (m10, m100) = (libm::fmod(n, 10.0), libm::fmod(n, 100.0));
    if m10 == 1.0 && m100 != 11.0 { "one" }
    else if m10 == 2.0 && m100 != 12.0 { "two" }
    else if m10 == 3.0 && m100 != 13.0 { "few" }
    else { "other" }
}

fn pr_select(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {
    let lang = slot(i, &t, LANG, "PluralRules.prototype.select")?;
    let ty = slot(i, &t, OPT_A, "PluralRules.prototype.select")?;
    let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
    Ok(Value::str(plural(&lang, ty == "ordinal", n)))
}

fn pr_options(i: &mut Interp, t: Value, _: &[Value]) -> C<Value> {
    let lang = slot(i, &t, LANG, "PluralRules.prototype.resolvedOptions")?;
    let loc = slot(i, &t, LOCALE, "PluralRules.prototype.resolvedOptions")?;
    let ty = slot(i, &t, OPT_A, "PluralRules.prototype.resolvedOptions")?;
    let cats: &[&str] = if ty == "ordinal" && lang == "en" { &["few", "one", "two", "other"] }
                        else if ty == "ordinal" { &["other"] } else { &["one", "other"] };
    let cats = cats.iter().map(|c| Value::str(c)).collect();
    let cats = i.new_array(cats);
    Ok(options_object(i, vec![
        ("locale", Value::string(loc)), ("type", Value::string(ty)),
        ("minimumIntegerDigits", Value::Num(1.0)), ("minimumFractionDigits", Value::Num(0.0)),
        ("maximumFractionDigits", Value::Num(3.0)), ("pluralCategories", cats),
    ]))
}

// ── RelativeTimeFormat ───────────────────────────────────────────────────

fn rtf_new(i: &mut Interp, _: Value, a: &[Value]) -> C<Value> {
    let opts = a.get(1).cloned().unwrap_or(Value::Undefined);
    let style = option(i, &opts, "style", &["long", "short", "narrow"], "long")?;
    let numeric = option(i, &opts, "numeric", &["always", "auto"], "always")?;
    instance(i, "RelativeTimeFormat", a, [("style", style), ("numeric", numeric)])
}

const UNITS: &[&str] = &["second", "minute", "hour", "day", "week", "month", "quarter", "year"];

/// `n` with grouping and at most three fraction digits, in the locale's
/// separators.
fn format_number(lang: &str, n: f64) -> String {
    let (group, dec) = if lang == "de" { ('.', ',') } else { (',', '.') };
    let r = libm::round(n * 1000.0) / 1000.0;
    let int = libm::trunc(r);
    let digits = alloc::format!("{}", int as u64);
    let mut out = String::new();
    for (k, c) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k) % 3 == 0 { out.push(group); }
        out.push(c);
    }
    let frac = libm::round((r - int) * 1000.0) as u32;
    if frac > 0 {
        let f = alloc::format!("{frac:03}");
        out.push(dec);
        out.push_str(f.trim_end_matches('0'));
    }
    out
}

/// A phrase for `numeric: "auto"` (yesterday, next year), if the locale has one.
fn auto_phrase(lang: &str, unit: &str, v: f64) -> Option<&'static str> {
    let v = if v == 0.0 { 0 } else if v == 1.0 { 1 } else if v == -1.0 { -1 }
            else if v == 2.0 { 2 } else if v == -2.0 { -2 } else { return None };
    Some(match (lang, unit, v) {
        ("en", "day", -1) => "yesterday", ("en", "day", 0) => "today", ("en", "day", 1) => "tomorrow",
        ("en", "second", 0) => "now", ("en", "minute", 0) => "this minute",
        ("en", "hour", 0) => "this hour",
        ("en", "week", -1) => "last week", ("en", "week", 0) => "this week", ("en", "week", 1) => "next week",
        ("en", "month", -1) => "last month", ("en", "month", 0) => "this month",
        ("en", "month", 1) => "next month",
        ("en", "quarter", -1) => "last quarter", ("en", "quarter", 0) => "this quarter",
        ("en", "quarter", 1) => "next quarter",
        ("en", "year", -1) => "last year", ("en", "year", 0) => "this year", ("en", "year", 1) => "next year",
        ("de", "day", -2) => "vorgestern", ("de", "day", -1) => "gestern", ("de", "day", 0) => "heute",
        ("de", "day", 1) => "morgen", ("de", "day", 2) => "übermorgen",
        ("de", "second", 0) => "jetzt", ("de", "minute", 0) => "in dieser Minute",
        ("de", "hour", 0) => "in dieser Stunde",
        ("de", "week", -1) => "letzte Woche", ("de", "week", 0) => "diese Woche",
        ("de", "week", 1) => "nächste Woche",
        ("de", "month", -1) => "letzten Monat", ("de", "month", 0) => "diesen Monat",
        ("de", "month", 1) => "nächsten Monat",
        ("de", "quarter", -1) => "letztes Quartal", ("de", "quarter", 0) => "dieses Quartal",
        ("de", "quarter", 1) => "nächstes Quartal",
        ("de", "year", -1) => "letztes Jahr", ("de", "year", 0) => "dieses Jahr",
        ("de", "year", 1) => "nächstes Jahr",
        _ => return None,
    })
}

/// The unit word: (singular, plural); German in the dative after `in`/`vor`.
fn unit_word(lang: &str, short: bool, unit: &str) -> (&'static str, &'static str) {
    match (lang, short, unit) {
        ("de", false, "second") => ("Sekunde", "Sekunden"),
        ("de", false, "minute") => ("Minute", "Minuten"),
        ("de", false, "hour") => ("Stunde", "Stunden"),
        ("de", false, "day") => ("Tag", "Tagen"),
        ("de", false, "week") => ("Woche", "Wochen"),
        ("de", false, "month") => ("Monat", "Monaten"),
        ("de", false, "quarter") => ("Quartal", "Quartalen"),
        ("de", false, _) => ("Jahr", "Jahren"),
        ("de", true, "second") => ("Sek.", "Sek."),
        ("de", true, "minute") => ("Min.", "Min."),
        ("de", true, "hour") => ("Std.", "Std."),
        ("de", true, "day") => ("Tag", "Tagen"),
        ("de", true, "week") => ("Woche", "Wochen"),
        ("de", true, "month") => ("Monat", "Monaten"),
        ("de", true, "quarter") => ("Quart.", "Quart."),
        ("de", true, _) => ("Jahr", "Jahren"),
        (_, false, "second") => ("second", "seconds"),
        (_, false, "minute") => ("minute", "minutes"),
        (_, false, "hour") => ("hour", "hours"),
        (_, false, "day") => ("day", "days"),
        (_, false, "week") => ("week", "weeks"),
        (_, false, "month") => ("month", "months"),
        (_, false, "quarter") => ("quarter", "quarters"),
        (_, false, _) => ("year", "years"),
        (_, true, "second") => ("sec.", "sec."),
        (_, true, "minute") => ("min.", "min."),
        (_, true, "hour") => ("hr.", "hr."),
        (_, true, "day") => ("day", "days"),
        (_, true, "week") => ("wk.", "wk."),
        (_, true, "month") => ("mo.", "mo."),
        (_, true, "quarter") => ("qtr.", "qtrs."),
        (_, true, _) => ("yr.", "yr."),
    }
}

/// (text before the number, number, text after) or the whole phrase alone.
fn rtf_pieces(i: &mut Interp, t: &Value, a: &[Value])
    -> C<(String, Option<(String, String)>, &'static str)> {
    let lang = slot(i, t, LANG, "RelativeTimeFormat.prototype.format")?;
    let style = slot(i, t, OPT_A, "RelativeTimeFormat.prototype.format")?;
    let numeric = slot(i, t, OPT_B, "RelativeTimeFormat.prototype.format")?;
    let v = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
    let unit = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;
    let unit = unit.strip_suffix('s').unwrap_or(&unit);
    let Some(unit) = UNITS.iter().find(|u| **u == unit).copied() else {
        return i.range_err(&alloc::format!("invalid unit argument '{unit}'"));
    };
    if !v.is_finite() { return i.range_err("value must be finite") }
    if numeric == "auto" {
        if let Some(p) = auto_phrase(&lang, unit, v) {
            if !(v == 0.0 && v.is_sign_negative()) { return Ok((String::from(p), None, unit)) }
        }
    }
    let past = v < 0.0 || (v == 0.0 && v.is_sign_negative());
    let num = format_number(&lang, libm::fabs(v));
    let (one, many) = unit_word(&lang, style != "long", unit);
    let word = if plural(&lang, false, v) == "one" { one } else { many };
    let (pre, post) = match (lang.as_str(), past) {
        ("de", false) => (String::from("in "), alloc::format!(" {word}")),
        ("de", true) => (String::from("vor "), alloc::format!(" {word}")),
        (_, false) => (String::from("in "), alloc::format!(" {word}")),
        (_, true) => (String::new(), alloc::format!(" {word} ago")),
    };
    Ok((pre, Some((num, post)), unit))
}

fn rtf_format(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {
    let (pre, rest, _) = rtf_pieces(i, &t, a)?;
    Ok(Value::string(match rest {
        Some((num, post)) => alloc::format!("{pre}{num}{post}"),
        None => pre,
    }))
}

fn rtf_parts(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {
    let (pre, rest, unit) = rtf_pieces(i, &t, a)?;
    let part = |i: &mut Interp, ty: &str, v: String, unit: Option<&str>| {
        let mut p = vec![("type", Value::str(ty)), ("value", Value::string(v))];
        if let Some(u) = unit { p.push(("unit", Value::str(u))); }
        options_object(i, p)
    };
    let mut out = Vec::new();
    match rest {
        None => out.push(part(i, "literal", pre, None)),
        Some((num, post)) => {
            if !pre.is_empty() { out.push(part(i, "literal", pre, None)); }
            out.push(part(i, "integer", num, Some(unit)));
            out.push(part(i, "literal", post, None));
        }
    }
    Ok(i.new_array(out))
}

fn rtf_options(i: &mut Interp, t: Value, _: &[Value]) -> C<Value> {
    let loc = slot(i, &t, LOCALE, "RelativeTimeFormat.prototype.resolvedOptions")?;
    let style = slot(i, &t, OPT_A, "RelativeTimeFormat.prototype.resolvedOptions")?;
    let numeric = slot(i, &t, OPT_B, "RelativeTimeFormat.prototype.resolvedOptions")?;
    Ok(options_object(i, vec![
        ("locale", Value::string(loc)), ("style", Value::string(style)),
        ("numeric", Value::string(numeric)), ("numberingSystem", Value::str("latn")),
    ]))
}

// ── Segmenter ────────────────────────────────────────────────────────────

fn seg_new(i: &mut Interp, _: Value, a: &[Value]) -> C<Value> {
    let opts = a.get(1).cloned().unwrap_or(Value::Undefined);
    let g = option(i, &opts, "granularity", &["grapheme", "word", "sentence"], "grapheme")?;
    instance(i, "Segmenter", a, [("granularity", g), ("", "")])
}

fn seg_options(i: &mut Interp, t: Value, _: &[Value]) -> C<Value> {
    let loc = slot(i, &t, LOCALE, "Segmenter.prototype.resolvedOptions")?;
    let g = slot(i, &t, OPT_A, "Segmenter.prototype.resolvedOptions")?;
    Ok(options_object(i, vec![("locale", Value::string(loc)), ("granularity", Value::string(g))]))
}

/// The property sets the boundary rules need, decoded once per `segment()`.
struct Props {
    extend: Vec<(u32, u32)>,
    spacing: Vec<(u32, u32)>,
    pict: Vec<(u32, u32)>,
    alpha: Vec<(u32, u32)>,
    ideo: Vec<(u32, u32)>,
}

impl Props {
    fn load() -> Props {
        let p = |n: &str, v: Option<&str>| super::regexp::unicode_property(n, v).unwrap_or_default();
        let mut extend = p("Grapheme_Extend", None);
        extend.extend(p("Emoji_Modifier", None));
        extend.sort_unstable();
        let mut ideo = p("Ideographic", None);
        for s in ["Hiragana", "Katakana", "Thai", "Lao", "Khmer", "Myanmar"] {
            ideo.extend(p("Script", Some(s)));
        }
        ideo.sort_unstable();
        Props { extend, spacing: p("gc", Some("Mc")), pict: p("Extended_Pictographic", None),
                alpha: p("Alphabetic", None), ideo }
    }
}

fn has(set: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    let k = set.partition_point(|&(_, b)| b < c);
    set.get(k).is_some_and(|&(a, _)| a <= c)
}

#[derive(PartialEq, Clone, Copy)]
enum Jamo { L, V, T, LV, LVT, No }

fn jamo(c: char) -> Jamo {
    match c as u32 {
        0x1100..=0x115F | 0xA960..=0xA97C => Jamo::L,
        0x1160..=0x11A7 | 0xD7B0..=0xD7C6 => Jamo::V,
        0x11A8..=0x11FF | 0xD7CB..=0xD7FB => Jamo::T,
        x @ 0xAC00..=0xD7A3 => if (x - 0xAC00) % 28 == 0 { Jamo::LV } else { Jamo::LVT },
        _ => Jamo::No,
    }
}

fn is_ri(c: char) -> bool { (0x1F1E6..=0x1F1FF).contains(&(c as u32)) }
fn is_control(c: char) -> bool {
    matches!(c as u32, 0..=0x1F | 0x7F..=0x9F | 0x2028 | 0x2029) && c != '\u{200D}'
}

/// Extended grapheme cluster boundaries (UAX #29 §3.1.1) as char ranges.
fn graphemes(s: &[char], p: &Props) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    // Regional indicators ending at the current char, and the GB11 state:
    // 1 after `ExtPict Extend*`, 2 after `ExtPict Extend* ZWJ`.
    let mut ri = 0usize;
    let mut emoji = 0u8;
    for k in 0..s.len() {
        let prev = s[k];
        ri = if is_ri(prev) { ri + 1 } else { 0 };
        emoji = if has(&p.pict, prev) { 1 }
                else if emoji == 1 && has(&p.extend, prev) { 1 }
                else if emoji == 1 && prev == '\u{200D}' { 2 }
                else { 0 };
        let brk = match s.get(k + 1) {
            None => true,
            Some(&c) => {
                let (jp, jc) = (jamo(prev), jamo(c));
                if prev == '\r' && c == '\n' { false }
                else if is_control(prev) || is_control(c) { true }
                else if jp == Jamo::L && matches!(jc, Jamo::L | Jamo::V | Jamo::LV | Jamo::LVT) { false }
                else if matches!(jp, Jamo::LV | Jamo::V) && matches!(jc, Jamo::V | Jamo::T) { false }
                else if matches!(jp, Jamo::LVT | Jamo::T) && jc == Jamo::T { false }
                else if c == '\u{200D}' || has(&p.extend, c) || has(&p.spacing, c) { false }
                else if emoji == 2 && has(&p.pict, c) { false }
                else if is_ri(prev) && is_ri(c) { ri % 2 == 0 }
                else { true }
            }
        };
        if brk {
            out.push((start, k + 1));
            start = k + 1;
        }
    }
    out
}

#[derive(PartialEq, Clone, Copy)]
enum WordKind { Letter, Digit, Ideo, Space, Other }

fn word_kind(c: char, p: &Props) -> WordKind {
    if c.is_ascii_digit() || (c.is_numeric() && !has(&p.alpha, c) && c.is_alphanumeric()) {
        WordKind::Digit
    } else if has(&p.ideo, c) { WordKind::Ideo }
    else if has(&p.alpha, c) || c == '_' { WordKind::Letter }
    else if c != '\n' && c != '\r' && c.is_whitespace() { WordKind::Space }
    else { WordKind::Other }
}

/// Word boundaries (UAX #29 §4.1, simplified) over grapheme clusters; the
/// flag says whether the segment is word-like.
fn words(s: &[char], p: &Props) -> Vec<(usize, usize, bool)> {
    let g = graphemes(s, p);
    let kinds: Vec<WordKind> = g.iter().map(|&(a, _)| word_kind(s[a], p)).collect();
    let mut out: Vec<(usize, usize, bool)> = Vec::new();
    let mut k = 0;
    while k < g.len() {
        let kind = kinds[k];
        let mut e = k + 1;
        match kind {
            WordKind::Letter | WordKind::Digit => {
                while e < g.len() {
                    let n = kinds[e];
                    if matches!(n, WordKind::Letter | WordKind::Digit) { e += 1; continue }
                    // `can't`, `e.g`, `3.14`, `1,000`: a joiner between two word characters.
                    let mid = s[g[e].0];
                    let joins = e + 1 < g.len() && matches!(kinds[e + 1], WordKind::Letter | WordKind::Digit)
                        && (matches!(mid, '\'' | '\u{2019}' | '.' | ':' | '\u{B7}')
                            || (matches!(mid, ',' | ';') && kinds[e - 1] == WordKind::Digit
                                && kinds[e + 1] == WordKind::Digit));
                    if joins { e += 2 } else { break }
                }
            }
            WordKind::Space => while e < g.len() && kinds[e] == WordKind::Space { e += 1 },
            _ => {}
        }
        let like = matches!(kind, WordKind::Letter | WordKind::Digit | WordKind::Ideo);
        out.push((g[k].0, g[e - 1].1, like));
        k = e;
    }
    out
}

/// Sentence boundaries (UAX #29 §5.1, simplified): after `!`, `?` or a
/// full stop that is followed by space and not by a lowercase letter, with
/// trailing closers and spaces kept in the sentence.
fn sentences(s: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut k = 0;
    while k < s.len() {
        let c = s[k];
        k += 1;
        if matches!(c, '\n' | '\u{2029}') {
            out.push((start, k));
            start = k;
            continue;
        }
        if !matches!(c, '.' | '!' | '?' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}') { continue }
        while k < s.len() && matches!(s[k], '.' | '!' | '?' | '"' | '\'' | ')' | ']'
                                        | '\u{201D}' | '\u{2019}') { k += 1 }
        let sp = k;
        while k < s.len() && s[k] != '\n' && s[k].is_whitespace() { k += 1 }
        let next = s.get(k).copied();
        let ends = match next {
            None => true,
            Some(n) => c != '.' || (k > sp && !n.is_lowercase()),
        };
        if c == '.' && sp == k && next.is_some() { k = sp; continue }
        if ends { out.push((start, k)); start = k; } else { k = sp; }
    }
    if start < s.len() { out.push((start, s.len())); }
    out
}

fn seg_segment(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {
    let g = slot(i, &t, OPT_A, "Segmenter.prototype.segment")?;
    let input = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
    let chars: Vec<char> = input.chars().collect();
    let props = Props::load();
    let spans: Vec<(usize, usize, Option<bool>)> = match g.as_str() {
        "word" => words(&chars, &props).into_iter().map(|(a, b, w)| (a, b, Some(w))).collect(),
        "sentence" => sentences(&chars).into_iter().map(|(a, b)| (a, b, None)).collect(),
        _ => graphemes(&chars, &props).into_iter().map(|(a, b)| (a, b, None)).collect(),
    };
    let mut list = Vec::with_capacity(spans.len());
    for (a, b, w) in spans {
        i.tick()?;
        let text: String = chars[a..b].iter().collect();
        let mut p = vec![("segment", Value::string(text)), ("index", Value::Num(a as f64)),
                         ("input", Value::Str(input.clone()))];
        if let Some(w) = w { p.push(("isWordLike", Value::Bool(w))); }
        list.push(options_object(i, p));
    }
    let list = i.new_array(list);
    let proto = i.realm.intl_protos.get("Segments").cloned();
    let o = new_obj(proto);
    o.borrow_mut().define(SEG_LIST, Prop::frozen(list));
    Ok(Value::Obj(o))
}

fn segs_containing(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {
    let list = i.get(&t, SEG_LIST)?;
    let n = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
    let n = if n.is_nan() { 0.0 } else { libm::trunc(n) };
    for seg in i.elems(&list)? {
        let at = i.get(&seg, "index")?;
        let at = i.to_number(&at)?;
        let text = i.get(&seg, "segment")?;
        let len = i.to_string(&text)?.chars().count() as f64;
        if n >= at && n < at + len { return Ok(seg) }
    }
    Ok(Value::Undefined)
}
