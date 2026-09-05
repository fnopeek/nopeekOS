//! `Date` — die Zeitrechnung aus ES 21.4, nicht der Stumpf davor.
//!
//! **Eine Zeitzone: UTC.** Es gibt keine Zonendatenbank im Bild und keinen
//! Weg, an die des Wirts zu kommen; „lokal" IST hier UTC, und
//! `getTimezoneOffset()` sagt ehrlich 0. Das ist eine benannte
//! Vereinfachung, keine Luecke im Verborgenen: alle `getX`/`setX` fallen
//! damit mit ihren `getUTCX`/`setUTCX` zusammen.
//!
//! Der Zeitwert liegt in `ObjKind::Date` und nicht als Eigenschaft am
//! Objekt — die Vorfassung trug ihn als `__t`, und der stand damit in
//! `Object.getOwnPropertyNames(d)`.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;

use super::interp::*;
use super::value::*;

const MS_DAY: f64 = 86400000.0;
const MS_HOUR: f64 = 3600000.0;
const MS_MIN: f64 = 60000.0;
const MS_SEC: f64 = 1000.0;
/// Der aeusserste darstellbare Zeitwert (ES 21.4.1.1).
const MAX_TIME: f64 = 8.64e15;

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/// Kumulierte Tage vor jedem Monat, im Gemeinjahr.
const CUM: [f64; 13] = [0.0, 31.0, 59.0, 90.0, 120.0, 151.0,
                        181.0, 212.0, 243.0, 273.0, 304.0, 334.0, 365.0];

fn fdiv(a: f64, b: f64) -> f64 { libm::floor(a / b) }
fn fmod_pos(a: f64, b: f64) -> f64 { let r = libm::fmod(a, b); if r < 0.0 { r + b } else { r } }

pub fn day(t: f64) -> f64 { fdiv(t, MS_DAY) }
fn time_in_day(t: f64) -> f64 { fmod_pos(t, MS_DAY) }

fn leap(y: f64) -> bool {
    libm::fmod(y, 4.0) == 0.0 && (libm::fmod(y, 100.0) != 0.0 || libm::fmod(y, 400.0) == 0.0)
}
fn day_from_year(y: f64) -> f64 {
    365.0 * (y - 1970.0) + fdiv(y - 1969.0, 4.0) - fdiv(y - 1901.0, 100.0) + fdiv(y - 1601.0, 400.0)
}
fn time_from_year(y: f64) -> f64 { MS_DAY * day_from_year(y) }

fn year_from_time(t: f64) -> f64 {
    // Schaetzen und in hoechstens zwei Schritten korrigieren — die Schaetzung
    // ueber die mittlere Jahreslaenge liegt nie weiter daneben.
    let mut y = libm::floor(t / (MS_DAY * 365.2425)) + 1970.0;
    while time_from_year(y) > t { y -= 1.0; }
    while time_from_year(y + 1.0) <= t { y += 1.0; }
    y
}
fn day_in_year(t: f64) -> f64 { day(t) - day_from_year(year_from_time(t)) }

fn month_from_time(t: f64) -> f64 {
    let d = day_in_year(t);
    let l = if leap(year_from_time(t)) { 1.0 } else { 0.0 };
    for m in (0..12).rev() {
        let start = CUM[m] + if m >= 2 { l } else { 0.0 };
        if d >= start { return m as f64; }
    }
    0.0
}
fn date_from_time(t: f64) -> f64 {
    let m = month_from_time(t) as usize;
    let l = if leap(year_from_time(t)) { 1.0 } else { 0.0 };
    day_in_year(t) - CUM[m] - if m >= 2 { l } else { 0.0 } + 1.0
}
fn week_day(t: f64) -> f64 { fmod_pos(day(t) + 4.0, 7.0) }
fn hour(t: f64) -> f64 { fmod_pos(fdiv(t, MS_HOUR), 24.0) }
fn minute(t: f64) -> f64 { fmod_pos(fdiv(t, MS_MIN), 60.0) }
fn second(t: f64) -> f64 { fmod_pos(fdiv(t, MS_SEC), 60.0) }
fn milli(t: f64) -> f64 { fmod_pos(t, MS_SEC) }

fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    if !h.is_finite() || !m.is_finite() || !s.is_finite() || !ms.is_finite() { return f64::NAN; }
    libm::trunc(h) * MS_HOUR + libm::trunc(m) * MS_MIN + libm::trunc(s) * MS_SEC + libm::trunc(ms)
}

fn make_day(y: f64, m: f64, d: f64) -> f64 {
    if !y.is_finite() || !m.is_finite() || !d.is_finite() { return f64::NAN; }
    let (y, m, d) = (libm::trunc(y), libm::trunc(m), libm::trunc(d));
    let ym = y + fdiv(m, 12.0);
    if !ym.is_finite() { return f64::NAN; }
    let mn = fmod_pos(m, 12.0) as usize;
    let l = if leap(ym) { 1.0 } else { 0.0 };
    let first = day_from_year(ym) + CUM[mn] + if mn >= 2 { l } else { 0.0 };
    first + d - 1.0
}

fn make_date(d: f64, t: f64) -> f64 {
    if !d.is_finite() || !t.is_finite() { return f64::NAN; }
    let r = d * MS_DAY + t;
    if !r.is_finite() { return f64::NAN; }
    r
}

fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || libm::fabs(t) > MAX_TIME { return f64::NAN; }
    libm::trunc(t) + 0.0
}

/// Den Zeitwert eines `Date` holen. Alles andere wirft — `Date.prototype.
/// getTime.call({})` ist ein TypeError, keine 0.
fn this_time(i: &mut Interp, t: &Value) -> C<f64> {
    match t {
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::Date(c) => Ok(c.get()),
            _ => i.type_err("not a Date"),
        },
        _ => i.type_err("not a Date"),
    }
}

fn set_time(i: &mut Interp, t: &Value, v: f64) -> C<f64> {
    match t {
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::Date(c) => { c.set(v); Ok(v) }
            _ => i.type_err("not a Date"),
        },
        _ => i.type_err("not a Date"),
    }
}

fn pad(n: f64, w: usize) -> String {
    let neg = n < 0.0;
    let mut s = num_to_string(libm::fabs(n));
    while s.len() < w { s.insert(0, '0'); }
    if neg { s.insert(0, '-'); }
    s
}

/// Das Jahr, wie `toString` es schreibt: vierstellig, mit Vorzeichen wenn
/// negativ.
fn year_str(y: f64) -> String {
    if y < 0.0 { alloc::format!("-{}", pad(-y, 6)) } else { pad(y, 4) }
}

fn date_string(t: f64) -> String {
    alloc::format!("{} {} {} {}", DAYS[week_day(t) as usize],
        MONTHS[month_from_time(t) as usize], pad(date_from_time(t), 2),
        year_str(year_from_time(t)))
}

fn time_string(t: f64) -> String {
    alloc::format!("{}:{}:{} GMT+0000 (Coordinated Universal Time)",
        pad(hour(t), 2), pad(minute(t), 2), pad(second(t), 2))
}

fn full_string(t: f64) -> String {
    if t.is_nan() { return "Invalid Date".to_string(); }
    alloc::format!("{} {}", date_string(t), time_string(t))
}

fn utc_string(t: f64) -> String {
    if t.is_nan() { return "Invalid Date".to_string(); }
    alloc::format!("{}, {} {} {} {}:{}:{} GMT", DAYS[week_day(t) as usize],
        pad(date_from_time(t), 2), MONTHS[month_from_time(t) as usize],
        year_str(year_from_time(t)), pad(hour(t), 2), pad(minute(t), 2), pad(second(t), 2))
}

fn iso_string(t: f64) -> String {
    let y = year_from_time(t);
    let ys = if (0.0..=9999.0).contains(&y) { pad(y, 4) }
             else if y < 0.0 { alloc::format!("-{}", pad(-y, 6)) }
             else { alloc::format!("+{}", pad(y, 6)) };
    alloc::format!("{ys}-{}-{}T{}:{}:{}.{}Z", pad(month_from_time(t) + 1.0, 2),
        pad(date_from_time(t), 2), pad(hour(t), 2), pad(minute(t), 2),
        pad(second(t), 2), pad(milli(t), 3))
}

/// `Date.parse`. Zwei Formate, und beide muessen sein: das ISO-Format der
/// Spezifikation und die eigene Ausgabe von `toString`/`toUTCString` — die
/// Spezifikation verlangt ausdruecklich, dass der Rueckweg klappt.
pub fn parse_date(s: &str) -> f64 {
    let t = s.trim();
    if let Some(v) = parse_iso(t) { return v; }
    parse_legacy(t).unwrap_or(f64::NAN)
}

fn num(s: &str) -> Option<f64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
    s.parse::<f64>().ok()
}

fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    // Jahr: vierstellig, oder mit Vorzeichen sechsstellig.
    let (year, neg, mut k) = if b.first() == Some(&b'+') || b.first() == Some(&b'-') {
        if b.len() < 7 { return None }
        (num(&s[1..7])?, b[0] == b'-', 7usize)
    } else {
        if b.len() < 4 { return None }
        (num(&s[0..4])?, false, 4usize)
    };
    // `-000000` ist ausdruecklich ungueltig.
    if neg && year == 0.0 { return None; }
    let year = if neg { -year } else { year };
    let mut month = 1.0;
    let mut dayv = 1.0;
    if k < b.len() && b[k] == b'-' {
        if k + 3 > b.len() { return None }
        month = num(&s[k + 1..k + 3])?;
        k += 3;
        if k < b.len() && b[k] == b'-' {
            if k + 3 > b.len() { return None }
            dayv = num(&s[k + 1..k + 3])?;
            k += 3;
        }
    }
    if !(1.0..=12.0).contains(&month) || !(1.0..=31.0).contains(&dayv) { return None; }
    let (mut h, mut mi, mut sec, mut ms) = (0.0, 0.0, 0.0, 0.0);
    // Ohne Zeitteil ist ein reines Datum UTC; MIT Zeitteil und ohne Zone
    // waere es ortszeitlich — und die IST hier UTC.
    let mut off = 0.0;
    if k < b.len() && (b[k] == b'T' || b[k] == b't') {
        if k + 6 > b.len() { return None }
        h = num(&s[k + 1..k + 3])?;
        if b[k + 3] != b':' { return None }
        mi = num(&s[k + 4..k + 6])?;
        k += 6;
        if k < b.len() && b[k] == b':' {
            if k + 3 > b.len() { return None }
            sec = num(&s[k + 1..k + 3])?;
            k += 3;
            if k < b.len() && b[k] == b'.' {
                let start = k + 1;
                let mut e = start;
                while e < b.len() && b[e].is_ascii_digit() { e += 1; }
                if e == start { return None }
                // Nur die ersten drei Stellen zaehlen, der Rest faellt weg.
                let frac = &s[start..e.min(start + 3)];
                let mut v = num(frac)?;
                for _ in frac.len()..3 { v *= 10.0; }
                ms = v;
                k = e;
            }
        }
        if k < b.len() {
            match b[k] {
                b'Z' | b'z' => { k += 1; }
                b'+' | b'-' => {
                    let sign = if b[k] == b'-' { -1.0 } else { 1.0 };
                    if k + 3 > b.len() { return None }
                    let oh = num(&s[k + 1..k + 3])?;
                    k += 3;
                    let om = if k < b.len() && b[k] == b':' {
                        let v = num(&s[k + 1..k + 3.min(b.len() - k + k)])?;
                        k += 3; v
                    } else if k + 2 <= b.len() && b[k].is_ascii_digit() {
                        let v = num(&s[k..k + 2])?; k += 2; v
                    } else { 0.0 };
                    off = sign * (oh * MS_HOUR + om * MS_MIN);
                }
                _ => return None,
            }
        }
    }
    if k != b.len() { return None; }
    if h > 24.0 || mi > 59.0 || sec > 59.0 { return None; }
    let d = make_day(year, month - 1.0, dayv);
    Some(time_clip(make_date(d, make_time(h, mi, sec, ms)) - off))
}

/// Die eigene Ausgabe wieder einlesen: `Www Mmm DD YYYY HH:MM:SS GMT+0000 …`
/// und `Www, DD Mmm YYYY HH:MM:SS GMT`.
fn parse_legacy(s: &str) -> Option<f64> {
    let cleaned = s.replace(',', " ");
    let toks: Vec<&str> = cleaned.split_whitespace().collect();
    if toks.is_empty() { return None }
    let (mut year, mut month, mut dayv) = (None, None, None);
    let (mut h, mut mi, mut sec) = (0.0, 0.0, 0.0);
    let mut off = 0.0;
    for tk in &toks {
        if tk.starts_with('(') { break; }
        if let Some(m) = MONTHS.iter().position(|m| tk.len() >= 3 && tk[..3].eq_ignore_ascii_case(m)) {
            month = Some(m as f64);
            continue;
        }
        if DAYS.iter().any(|d| tk.len() >= 3 && tk[..3].eq_ignore_ascii_case(d)) { continue; }
        if tk.contains(':') {
            let ps: Vec<&str> = tk.split(':').collect();
            if ps.len() < 2 { return None }
            h = num(ps[0])?; mi = num(ps[1])?;
            if ps.len() > 2 { sec = num(ps[2])?; }
            continue;
        }
        if let Some(rest) = tk.strip_prefix("GMT").or_else(|| tk.strip_prefix("UTC")) {
            if rest.is_empty() { continue }
            let sign = if rest.starts_with('-') { -1.0 } else { 1.0 };
            let d = rest.trim_start_matches(['+', '-']);
            if d.len() != 4 { return None }
            off = sign * (num(&d[..2])? * MS_HOUR + num(&d[2..])? * MS_MIN);
            continue;
        }
        if tk.eq_ignore_ascii_case("GMT") || tk.eq_ignore_ascii_case("UTC") { continue }
        let (sign, digits) = match tk.strip_prefix('-') { Some(r) => (-1.0, r), None => (1.0, *tk) };
        let v = num(digits)?;
        if digits.len() <= 2 && dayv.is_none() { dayv = Some(v); }
        else if year.is_none() { year = Some(sign * v); }
        else { return None }
    }
    Some(time_clip(make_date(make_day(year?, month?, dayv?), make_time(h, mi, sec, 0.0)) - off))
}

fn new_date(i: &mut Interp, t: f64) -> Value {
    let proto = i.realm.date_proto.clone();
    Value::Obj(new_kind(Some(proto), ObjKind::Date(Rc::new(Cell::new(t)))))
}

/// Der gemeinsame Rumpf aller `setX`: die sieben Felder holen, die
/// genannten ersetzen, wieder zusammensetzen. `first` ist das erste Feld,
/// das das Argument ersetzt (0 = Jahr … 6 = Millisekunden).
fn set_fields(i: &mut Interp, t: Value, a: &[Value], first: usize, count: usize) -> C<Value> {
    let cur = this_time(i, &t)?;
    // Die Argumente werden IMMER umgewandelt, auch wenn der Zeitwert NaN ist
    // — ihre Nebenwirkungen sind beobachtbar.
    let mut args = Vec::with_capacity(count);
    for k in 0..count {
        match a.get(k) {
            Some(v) => args.push(i.to_number(v)?),
            None => break,
        }
    }
    // `setFullYear` auf einem ungueltigen Datum faengt bei der Epoche an;
    // jedes andere `setX` bleibt ungueltig.
    let base = if cur.is_nan() { if first == 0 { 0.0 } else { return set_time(i, &t, f64::NAN).map(Value::Num) } } else { cur };
    let mut f = [year_from_time(base), month_from_time(base), date_from_time(base),
                 hour(base), minute(base), second(base), milli(base)];
    for (k, v) in args.iter().enumerate() {
        if first + k < 7 { f[first + k] = *v; }
    }
    let v = time_clip(make_date(make_day(f[0], f[1], f[2]), make_time(f[3], f[4], f[5], f[6])));
    set_time(i, &t, v).map(Value::Num)
}

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let proto = new_obj(Some(realm.object_proto.clone()));
    realm.date_proto = proto.clone();

    let def = |o: &Gc, name: &str, f: NativeFn, len: usize| {
        let g = native(Some(fp.clone()), f, name, len, false);
        o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
    };

    // ── Der Konstruktor ──────────────────────────────────────────────────
    //
    // Drei Formen, und die dritte ist die einzige, die rechnet: kein
    // Argument = jetzt, ein Argument = Zahl oder Text, ab zwei = Felder.
    let ctor = native(Some(fp.clone()), |i, _, a| {
        // `Date()` OHNE `new` gibt Text, nicht ein Objekt.
        if !i.native_new {
            let now = { i.fake_now += 1.0; i.epoch_ms + i.fake_now };
            return Ok(Value::string(full_string(libm::trunc(now))));
        }
        let t = match a.len() {
            0 => { i.fake_now += 1.0; libm::trunc(i.epoch_ms + i.fake_now) }
            1 => {
                // Ein `Date` als Argument gibt seinen Zeitwert direkt weiter,
                // ohne den Umweg ueber den Text.
                if let Value::Obj(o) = &a[0] {
                    if let ObjKind::Date(c) = &o.borrow().kind { let v = c.get(); return Ok(new_date(i, v)); }
                }
                let p = i.to_primitive_hint(&a[0], "default")?;
                match p {
                    Value::Str(s) => parse_date(&s),
                    v => time_clip(i.to_number(&v)?),
                }
            }
            _ => {
                let mut f = [f64::NAN; 7];
                let defaults = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
                for k in 0..7 {
                    f[k] = match a.get(k) { Some(v) => i.to_number(v)?, None => defaults[k] };
                }
                // Zwei Ziffern heissen 19xx — die annexB-Regel, und sie gilt
                // auch hier, nicht nur in `setYear`.
                if !f[0].is_nan() {
                    let y = libm::trunc(f[0]);
                    if (0.0..=99.0).contains(&y) { f[0] = 1900.0 + y; }
                }
                time_clip(make_date(make_day(f[0], f[1], f[2]), make_time(f[3], f[4], f[5], f[6])))
            }
        };
        Ok(new_date(i, t))
    }, "Date", 7, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));

    def(&ctor, "now", |i, _, _| {
        // Eine steigende Uhr auf dem Zeitstempel des Wirts: zwei Aufrufe
        // duerfen nicht denselben Wert geben, und der Wert muss heute sein.
        i.fake_now += 1.0;
        Ok(Value::Num(libm::trunc(i.epoch_ms + i.fake_now)))
    }, 0);
    def(&ctor, "parse", |i, _, a| {
        let s = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        Ok(Value::Num(parse_date(&s)))
    }, 1);
    def(&ctor, "UTC", |i, _, a| {
        let defaults = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let mut f = [f64::NAN; 7];
        for k in 0..7 {
            f[k] = match a.get(k) { Some(v) => i.to_number(v)?, None => defaults[k] };
        }
        if f[0].is_nan() && a.is_empty() { return Ok(Value::Num(f64::NAN)); }
        if !f[0].is_nan() {
            let y = libm::trunc(f[0]);
            if (0.0..=99.0).contains(&y) { f[0] = 1900.0 + y; }
        }
        Ok(Value::Num(time_clip(make_date(make_day(f[0], f[1], f[2]),
                                          make_time(f[3], f[4], f[5], f[6])))))
    }, 7);

    // ── Die Leser ────────────────────────────────────────────────────────
    //
    // Weil die Ortszeit UTC ist, ist jedes `getX` sein eigenes `getUTCX` —
    // dieselbe Funktion, kein zweiter Rumpf.
    macro_rules! getter {
        ($($n:literal => $f:expr),* $(,)?) => { $(
            def(&proto, $n, |i, t, _| {
                let v = this_time(i, &t)?;
                if v.is_nan() { return Ok(Value::Num(f64::NAN)); }
                let f: fn(f64) -> f64 = $f;
                Ok(Value::Num(f(v)))
            }, 0);
        )* };
    }
    getter! {
        "getFullYear" => year_from_time, "getUTCFullYear" => year_from_time,
        "getMonth" => month_from_time, "getUTCMonth" => month_from_time,
        "getDate" => date_from_time, "getUTCDate" => date_from_time,
        "getDay" => week_day, "getUTCDay" => week_day,
        "getHours" => hour, "getUTCHours" => hour,
        "getMinutes" => minute, "getUTCMinutes" => minute,
        "getSeconds" => second, "getUTCSeconds" => second,
        "getMilliseconds" => milli, "getUTCMilliseconds" => milli,
        // annexB: das Jahr minus 1900, mit allen Folgen.
        "getYear" => |t| year_from_time(t) - 1900.0,
    }
    def(&proto, "getTime", |i, t, _| Ok(Value::Num(this_time(i, &t)?)), 0);
    def(&proto, "valueOf", |i, t, _| Ok(Value::Num(this_time(i, &t)?)), 0);
    def(&proto, "getTimezoneOffset", |i, t, _| {
        let v = this_time(i, &t)?;
        Ok(Value::Num(if v.is_nan() { f64::NAN } else { 0.0 }))
    }, 0);

    // ── Die Schreiber ────────────────────────────────────────────────────
    macro_rules! setter {
        ($($n:literal => $first:literal, $cnt:literal),* $(,)?) => { $(
            def(&proto, $n, |i, t, a| set_fields(i, t, a, $first, $cnt), $cnt);
        )* };
    }
    setter! {
        "setFullYear" => 0, 3, "setUTCFullYear" => 0, 3,
        "setMonth" => 1, 2, "setUTCMonth" => 1, 2,
        "setDate" => 2, 1, "setUTCDate" => 2, 1,
        "setHours" => 3, 4, "setUTCHours" => 3, 4,
        "setMinutes" => 4, 3, "setUTCMinutes" => 4, 3,
        "setSeconds" => 5, 2, "setUTCSeconds" => 5, 2,
        "setMilliseconds" => 6, 1, "setUTCMilliseconds" => 6, 1,
    }
    def(&proto, "setTime", |i, t, a| {
        this_time(i, &t)?;
        let v = time_clip(i.to_number(a.first().unwrap_or(&Value::Undefined))?);
        set_time(i, &t, v).map(Value::Num)
    }, 1);
    // annexB: zweistellige Jahre heissen 19xx.
    def(&proto, "setYear", |i, t, a| {
        let cur = this_time(i, &t)?;
        let y = i.to_number(a.first().unwrap_or(&Value::Undefined))?;
        let base = if cur.is_nan() { 0.0 } else { cur };
        if y.is_nan() { return set_time(i, &t, f64::NAN).map(Value::Num); }
        let yi = libm::trunc(y);
        let full = if (0.0..=99.0).contains(&yi) { yi + 1900.0 } else { y };
        let v = time_clip(make_date(make_day(full, month_from_time(base), date_from_time(base)),
                                    time_in_day(base)));
        set_time(i, &t, v).map(Value::Num)
    }, 1);

    // ── Die Texte ────────────────────────────────────────────────────────
    macro_rules! stringer {
        ($($n:literal => $f:expr),* $(,)?) => { $(
            def(&proto, $n, |i, t, _| {
                let v = this_time(i, &t)?;
                let f: fn(f64) -> String = $f;
                Ok(Value::string(f(v)))
            }, 0);
        )* };
    }
    stringer! {
        "toString" => full_string,
        "toDateString" => |t| if t.is_nan() { "Invalid Date".to_string() } else { date_string(t) },
        "toTimeString" => |t| if t.is_nan() { "Invalid Date".to_string() } else { time_string(t) },
        "toUTCString" => utc_string,
        // Ohne Landeseinstellungen sind die drei ihre gewoehnlichen
        // Geschwister. Eine erfundene Ortsschreibweise waere die falsche.
        "toLocaleString" => full_string,
        "toLocaleDateString" => |t| if t.is_nan() { "Invalid Date".to_string() } else { date_string(t) },
        "toLocaleTimeString" => |t| if t.is_nan() { "Invalid Date".to_string() } else { time_string(t) },
    }
    def(&proto, "toISOString", |i, t, _| {
        let v = this_time(i, &t)?;
        if !v.is_finite() { return i.range_err("Invalid time value"); }
        Ok(Value::string(iso_string(v)))
    }, 0);
    // `toJSON` ist GENERISCH: es fragt `toISOString` am Objekt, nicht die
    // eigene Rechnung. Ein Ersatz dort schlaegt durch.
    def(&proto, "toJSON", |i, t, _| {
        let o = i.to_object(&t)?;
        let ov = Value::Obj(o);
        let p = i.to_primitive_hint(&ov, "number")?;
        if let Value::Num(n) = &p { if !n.is_finite() { return Ok(Value::Null); } }
        let f = i.get(&ov, "toISOString")?;
        if !i.is_callable(&f) { return i.type_err("toISOString is not a function"); }
        i.call(&f, ov, &[])
    }, 1);
    // annexB: dieselbe Funktion wie `toUTCString`, nicht eine zweite.
    {
        let v = proto.borrow().get_own("toUTCString").and_then(|p| p.value.clone());
        if let Some(v) = v { proto.borrow_mut().define("toGMTString", Prop::builtin(v)); }
    }
    // Der Wunsch „default" wird hier zu Text — daran haengt, dass
    // `date + ""` das Datum schreibt statt die Millisekunden.
    {
        let g = native(Some(fp.clone()), |i, t, a| {
            if !matches!(t, Value::Obj(_)) { return i.type_err("Symbol.toPrimitive on a non-object"); }
            let hint = match a.first() {
                Some(Value::Str(s)) => s.to_string(),
                _ => return i.type_err("invalid hint"),
            };
            match hint.as_str() {
                "string" | "default" => i.ordinary_to_primitive(&t, true),
                "number" => i.ordinary_to_primitive(&t, false),
                _ => i.type_err("invalid hint"),
            }
        }, "[Symbol.toPrimitive]", 1, false);
        proto.borrow_mut().define(SYM_TO_PRIMITIVE, Prop {
            value: Some(Value::Obj(g)), get: None, set: None,
            writable: false, enumerable: false, configurable: true });
    }

    realm.global.borrow_mut().define("Date", Prop::builtin(Value::Obj(ctor)));
}
