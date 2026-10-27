//! Time zones: a named zone (`Europe/Zurich`) or a POSIX TZ string
//! (`CET-1CEST,M3.5.0,M10.5.0/3`), with daylight saving, as glibc reads
//! the `TZ` variable (POSIX.1-2017 §8.3). The names map to the TZ string
//! at the end of their tzdata file, which holds the rule for current years.
//! A bare offset (`+1`, `-5`, `+5:30`) is a fixed zone east of UTC.
//!
//! The zone is the config key `timezone`. A copy lives on the ESP
//! (`/EFI/NOPEEK/TZ`) so the login screen, which runs before the encrypted
//! config can be read, shows the same time.

use alloc::string::String;
use spin::Mutex;

/// Zone name to the POSIX TZ string at the end of its tzdata file.
const ZONES: &[(&str, &str)] = &[
    ("UTC", "UTC0"),
    ("Europe/Zurich", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Berlin", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Vienna", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Paris", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Rome", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Madrid", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Amsterdam", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Brussels", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Luxembourg", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Copenhagen", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Oslo", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Stockholm", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Prague", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Warsaw", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/Budapest", "CET-1CEST,M3.5.0,M10.5.0/3"),
    ("Europe/London", "GMT0BST,M3.5.0/1,M10.5.0"),
    ("Europe/Dublin", "IST-1GMT0,M10.5.0,M3.5.0/1"),
    ("Europe/Lisbon", "WET0WEST,M3.5.0/1,M10.5.0"),
    ("Europe/Helsinki", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Athens", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Bucharest", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Kyiv", "EET-2EEST,M3.5.0/3,M10.5.0/4"),
    ("Europe/Istanbul", "<+03>-3"),
    ("Europe/Moscow", "MSK-3"),
    ("America/New_York", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Toronto", "EST5EDT,M3.2.0,M11.1.0"),
    ("America/Chicago", "CST6CDT,M3.2.0,M11.1.0"),
    ("America/Denver", "MST7MDT,M3.2.0,M11.1.0"),
    ("America/Phoenix", "MST7"),
    ("America/Los_Angeles", "PST8PDT,M3.2.0,M11.1.0"),
    ("America/Anchorage", "AKST9AKDT,M3.2.0,M11.1.0"),
    ("America/Mexico_City", "CST6"),
    ("America/Sao_Paulo", "<-03>3"),
    ("America/Argentina/Buenos_Aires", "<-03>3"),
    ("Pacific/Honolulu", "HST10"),
    ("Africa/Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24"),
    ("Africa/Johannesburg", "SAST-2"),
    ("Africa/Lagos", "WAT-1"),
    ("Africa/Nairobi", "EAT-3"),
    ("Asia/Dubai", "<+04>-4"),
    ("Asia/Kolkata", "IST-5:30"),
    ("Asia/Kathmandu", "<+0545>-5:45"),
    ("Asia/Bangkok", "<+07>-7"),
    ("Asia/Singapore", "<+08>-8"),
    ("Asia/Shanghai", "CST-8"),
    ("Asia/Hong_Kong", "HKT-8"),
    ("Asia/Tokyo", "JST-9"),
    ("Asia/Seoul", "KST-9"),
    ("Australia/Perth", "AWST-8"),
    ("Australia/Brisbane", "AEST-10"),
    ("Australia/Sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Australia/Melbourne", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
    ("Pacific/Auckland", "NZST-12NZDT,M9.5.0,M4.1.0/3"),
];

/// When a transition happens: `Mm.w.d` (day `d` of week `w` of month `m`,
/// week 5 = the last), `Jn` (day 1..365, February 29 never counted) or `n`
/// (day 0..365, counting February 29), at `secs` local time.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Date {
    MonthWeekDay(u32, u32, u32),
    Julian1(u32),
    Julian0(u32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Rule {
    date: Date,
    /// Local time of day of the transition, seconds; may be negative or
    /// past 24 h (POSIX.1-2024 allows -167..167 hours).
    secs: i32,
}

/// A parsed zone. Offsets are seconds east of UTC.
#[derive(Clone, PartialEq, Debug)]
pub struct Zone {
    std_name: String,
    std_off: i32,
    dst: Option<(String, i32, Rule, Rule)>,
}

impl Zone {
    const fn utc() -> Self {
        Zone { std_name: String::new(), std_off: 0, dst: None }
    }

    /// The offset east of UTC and the abbreviation in effect at `unix`.
    pub fn at(&self, unix: u64) -> (i32, &str) {
        let std = (self.std_off, self.std_abbr());
        let Some((name, dst_off, start, end)) = &self.dst else { return std };
        let t = unix as i64;
        let year = super::civil_from_days((t + self.std_off as i64).div_euclid(86_400)).0;
        // A start rule is in standard time, an end rule in daylight time.
        let on = transition_utc(year, start, self.std_off);
        let off = transition_utc(year, end, *dst_off);
        let in_dst = if on < off { t >= on && t < off } else { !(t >= off && t < on) };
        if in_dst { (*dst_off, name.as_str()) } else { std }
    }

    fn std_abbr(&self) -> &str {
        if self.std_name.is_empty() { "UTC" } else { &self.std_name }
    }
}

/// The UTC second at which `rule` fires in `year`, for a rule given in
/// local time `off` seconds east of UTC.
fn transition_utc(year: i64, rule: &Rule, off: i32) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let jan1 = super::days_from_civil(year, 1, 1);
    let day = match rule.date {
        Date::Julian1(n) => {
            let n = n as i64;
            jan1 + n - 1 + if leap && n >= 60 { 1 } else { 0 }
        }
        Date::Julian0(n) => jan1 + n as i64,
        Date::MonthWeekDay(m, w, d) => {
            let first = super::days_from_civil(year, m, 1);
            let wd = super::weekday(first);
            let mut day = first + ((d + 7 - wd) % 7) as i64 + 7 * (w as i64 - 1);
            let next_month = if m == 12 {
                super::days_from_civil(year + 1, 1, 1)
            } else {
                super::days_from_civil(year, m + 1, 1)
            };
            while day >= next_month {
                day -= 7;
            }
            day
        }
    };
    day * 86_400 + rule.secs as i64 - off as i64
}

/// Parse a `timezone` value: a known zone name, a POSIX TZ string, or a
/// bare offset east of UTC.
pub fn parse(s: &str) -> Option<Zone> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((_, tz)) = ZONES.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
        return parse_posix(tz);
    }
    if let Some(off) = parse_fixed(s) {
        return Some(Zone { std_name: fixed_name(off), std_off: off, dst: None });
    }
    parse_posix(s)
}

/// `+1`, `-5`, `+5:30`, `0`: hours (and minutes) east of UTC.
fn parse_fixed(s: &str) -> Option<i32> {
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => (1, s),
    };
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return None;
    }
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?),
        None => (rest.parse::<i32>().ok()?, 0),
    };
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * 3600 + m * 60))
}

fn fixed_name(off: i32) -> String {
    if off == 0 {
        return String::from("UTC");
    }
    let a = off.unsigned_abs();
    let sign = if off < 0 { '-' } else { '+' };
    if a % 3600 == 0 {
        alloc::format!("UTC{}{}", sign, a / 3600)
    } else {
        alloc::format!("UTC{}{}:{:02}", sign, a / 3600, (a % 3600) / 60)
    }
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) { self.i += 1; true } else { false }
    }

    fn done(&self) -> bool {
        self.i == self.s.len()
    }

    /// `std` / `dst`: three or more letters, or `<...>` with letters,
    /// digits, `+` and `-`.
    fn name(&mut self) -> Option<String> {
        let start = self.i;
        if self.eat(b'<') {
            let from = self.i;
            while matches!(self.peek(), Some(b) if b.is_ascii_alphanumeric() || b == b'+' || b == b'-') {
                self.i += 1;
            }
            let n = core::str::from_utf8(&self.s[from..self.i]).ok()?;
            if !self.eat(b'>') || n.len() < 3 {
                self.i = start;
                return None;
            }
            return Some(String::from(n));
        }
        while matches!(self.peek(), Some(b) if b.is_ascii_alphabetic()) {
            self.i += 1;
        }
        if self.i - start < 3 {
            self.i = start;
            return None;
        }
        Some(String::from(core::str::from_utf8(&self.s[start..self.i]).ok()?))
    }

    fn number(&mut self, max: u32) -> Option<u32> {
        let start = self.i;
        let mut n: u32 = 0;
        while let Some(b) = self.peek().filter(u8::is_ascii_digit) {
            n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
            self.i += 1;
        }
        (self.i > start && n <= max).then_some(n)
    }

    /// `[+-]hh[:mm[:ss]]` in seconds, hours up to `max_h`.
    fn hms(&mut self, max_h: u32) -> Option<i32> {
        let sign = if self.eat(b'-') { -1 } else { self.eat(b'+'); 1 };
        let h = self.number(max_h)?;
        let m = if self.eat(b':') { self.number(59)? } else { 0 };
        let s = if self.eat(b':') { self.number(59)? } else { 0 };
        Some(sign * (h * 3600 + m * 60 + s) as i32)
    }

    fn rule(&mut self) -> Option<Rule> {
        let date = if self.eat(b'M') {
            let m = self.number(12)?;
            if !self.eat(b'.') { return None; }
            let w = self.number(5)?;
            if !self.eat(b'.') { return None; }
            let d = self.number(6)?;
            if m == 0 || w == 0 { return None; }
            Date::MonthWeekDay(m, w, d)
        } else if self.eat(b'J') {
            let n = self.number(365)?;
            if n == 0 { return None; }
            Date::Julian1(n)
        } else {
            Date::Julian0(self.number(365)?)
        };
        let secs = if self.eat(b'/') { self.hms(167)? } else { 2 * 3600 };
        Some(Rule { date, secs })
    }
}

/// POSIX TZ: `std offset [dst [offset] [,start[/time],end[/time]]]`. The
/// offset is west of UTC (`CET-1` is UTC+1). A `dst` without rules takes
/// the US rules, as glibc does.
fn parse_posix(s: &str) -> Option<Zone> {
    let mut c = Cursor { s: s.as_bytes(), i: 0 };
    let std_name = c.name()?;
    let std_off = -c.hms(24)?;
    if c.done() {
        return Some(Zone { std_name, std_off, dst: None });
    }
    let dst_name = c.name()?;
    let dst_off = match c.peek() {
        Some(b',') | None => std_off + 3600,
        _ => -c.hms(24)?,
    };
    let (start, end) = if c.eat(b',') {
        let start = c.rule()?;
        if !c.eat(b',') { return None; }
        (start, c.rule()?)
    } else {
        (Rule { date: Date::MonthWeekDay(3, 2, 0), secs: 7200 },
         Rule { date: Date::MonthWeekDay(11, 1, 0), secs: 7200 })
    };
    if !c.done() {
        return None;
    }
    Some(Zone { std_name, std_off, dst: Some((dst_name, dst_off, start, end)) })
}

/// The zone in effect; UTC until one is loaded.
static CURRENT: Mutex<Zone> = Mutex::new(Zone::utc());

/// Offset east of UTC in seconds at `unix`.
pub fn offset_at(unix: u64) -> i32 {
    CURRENT.lock().at(unix).0
}

/// Offset and abbreviation at `unix`.
pub fn at(unix: u64) -> (i32, String) {
    let z = CURRENT.lock();
    let (off, abbr) = z.at(unix);
    (off, String::from(abbr))
}

/// Make `value` the zone in effect. False (and the zone unchanged) when it
/// does not parse.
pub fn apply(value: &str) -> bool {
    match parse(value) {
        Some(z) => { *CURRENT.lock() = z; true }
        None => false,
    }
}

/// Load the zone from the ESP copy. Called at boot, before login.
pub fn load_from_esp() {
    let mut buf = [0u8; 128];
    let Some(esp) = crate::fat32::esp_start() else { return };
    let Some(n) = crate::fat32::read_small_file(esp, "TZ", &mut buf) else { return };
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        apply(s.trim());
    }
}

/// Take the zone from the config (after login) and keep the ESP copy equal
/// to it, so the login screen agrees with the desktop.
pub fn load_from_config() {
    let value = crate::config::get("timezone").unwrap_or_default();
    if !value.is_empty() && !apply(&value) {
        crate::kprintln!("[npk] timezone '{}' unknown, using UTC", value);
        *CURRENT.lock() = Zone::utc();
    }
    if value.is_empty() {
        *CURRENT.lock() = Zone::utc();
    }
    write_esp(&value);
}

/// Store `value` on the ESP unless it is already there.
fn write_esp(value: &str) {
    let Some(esp) = crate::fat32::esp_start() else { return };
    let mut buf = [0u8; 128];
    let have = crate::fat32::read_small_file(esp, "TZ", &mut buf);
    if have.is_some_and(|n| &buf[..n] == value.as_bytes()) {
        return;
    }
    if let Err(e) = crate::fat32::write_small_file(esp, "TZ", value.as_bytes()) {
        crate::kprintln!("[npk] timezone: ESP copy not written: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(y: i64, m: u32, d: u32, h: i64, mi: i64) -> u64 {
        (crate::time::days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60) as u64
    }

    #[test]
    fn zurich_switches_on_the_last_sundays() {
        let z = parse("Europe/Zurich").unwrap();
        // 2026: DST from 29 March 01:00 UTC to 25 October 01:00 UTC.
        assert_eq!(z.at(unix(2026, 3, 29, 0, 59)).0, 3600);
        assert_eq!(z.at(unix(2026, 3, 29, 1, 0)).0, 7200);
        assert_eq!(z.at(unix(2026, 10, 25, 0, 59)).0, 7200);
        assert_eq!(z.at(unix(2026, 10, 25, 1, 0)).0, 3600);
        assert_eq!(z.at(unix(2026, 10, 27, 12, 0)).1, "CET");
    }

    #[test]
    fn southern_and_fixed_zones() {
        let z = parse("Australia/Sydney").unwrap();
        assert_eq!(z.at(unix(2026, 1, 15, 0, 0)).0, 11 * 3600);
        assert_eq!(z.at(unix(2026, 7, 15, 0, 0)).0, 10 * 3600);
        assert_eq!(parse("+5:30").unwrap().at(0).0, 5 * 3600 + 1800);
        assert_eq!(parse("-5").unwrap().at(0).0, -5 * 3600);
        assert_eq!(parse("Asia/Kathmandu").unwrap().at(0).0, 5 * 3600 + 2700);
        assert!(parse("Mars/Olympus").is_none());
    }
}
