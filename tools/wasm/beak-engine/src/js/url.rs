//! `URL` und `URLSearchParams`.
//!
//! **Der gemessene Ausschnitt, nicht die ganze Norm.** Die WHATWG-URL ist ein
//! Zustandsautomat mit vierzig Zustaenden; gebraucht wird auf dem Zielkorpus
//! davon ein Bruchteil, und der ist gezaehlt statt geschaetzt:
//! `href` 909x, `pathname` 401x, `hash` 257x, `origin` 248x,
//! `searchParams` 219x, `protocol` 142x, `hostname` 89x. Alles Uebrige —
//! Zeichenkodierung, IDN, IPv6-Klammern, `file:`-Sonderwege — kommt gar nicht
//! vor und waere Arbeit fuer eine Zeile, die niemand liest.
//!
//! Was hier NICHT geraten wird: eine relative Adresse ohne Grundlage. `new
//! URL("/a")` ohne zweites Argument WIRFT, so wie im Browser. Eine erfundene
//! Grundlage saehe aus wie eine Antwort ([[feedback_invented_fallback_hides_the_fault]]).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

use super::interp::*;
use super::value::*;

/// Die zerlegte Adresse. Alles Text — eine URL IST Text mit Grenzen darin.
#[derive(Clone, Default)]
pub struct Parts {
    pub scheme: String,
    pub host: String,
    pub port: String,
    pub path: String,
    pub query: String,
    pub hash: String,
}

impl Parts {
    pub fn origin(&self) -> String {
        if self.scheme.is_empty() || self.host.is_empty() { return "null".to_string() }
        let mut s = alloc::format!("{}://{}", self.scheme, self.host);
        if !self.port.is_empty() { s.push(':'); s.push_str(&self.port); }
        s
    }
    pub fn host_with_port(&self) -> String {
        if self.port.is_empty() { self.host.clone() }
        else { alloc::format!("{}:{}", self.host, self.port) }
    }
    pub fn href(&self) -> String {
        let mut s = String::new();
        if !self.scheme.is_empty() { s.push_str(&self.scheme); s.push(':'); }
        if !self.host.is_empty() { s.push_str("//"); s.push_str(&self.host_with_port()); }
        s.push_str(&self.path);
        if !self.query.is_empty() { s.push('?'); s.push_str(&self.query); }
        if !self.hash.is_empty() { s.push('#'); s.push_str(&self.hash); }
        s
    }
}

/// Eine absolute Adresse zerlegen. `None`, wenn kein Schema davorsteht —
/// dann ist sie relativ und braucht eine Grundlage.
pub fn parse_abs(input: &str) -> Option<Parts> {
    let t = input.trim();
    let colon = t.find(':')?;
    let scheme = &t[..colon];
    if scheme.is_empty() || !scheme.bytes().next()?.is_ascii_alphabetic() { return None }
    if !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
        return None;
    }
    let mut p = Parts { scheme: scheme.to_ascii_lowercase(), ..Default::default() };
    let mut rest = &t[colon + 1..];
    if let Some(r) = rest.strip_prefix("//") {
        let end = r.find(['/', '?', '#']).unwrap_or(r.len());
        let auth = &r[..end];
        rest = &r[end..];
        // Anmeldedaten in der Adresse werden verworfen, nicht als Host
        // gelesen — `http://user@host/` hat den Host HINTER dem `@`.
        let hostport = auth.rsplit('@').next().unwrap_or(auth);
        match hostport.rfind(':') {
            Some(i) if hostport[i + 1..].bytes().all(|b| b.is_ascii_digit())
                       && !hostport[i + 1..].is_empty() => {
                p.host = hostport[..i].to_ascii_lowercase();
                p.port = hostport[i + 1..].to_string();
            }
            _ => p.host = hostport.to_ascii_lowercase(),
        }
        // Der Vorgabeport steht nicht im `href` — `https://x:443/` und
        // `https://x/` sind dieselbe Adresse.
        if (p.scheme == "https" && p.port == "443") || (p.scheme == "http" && p.port == "80") {
            p.port.clear();
        }
    }
    split_tail(&mut p, rest);
    if p.path.is_empty() && !p.host.is_empty() { p.path = "/".to_string(); }
    Some(p)
}

fn split_tail(p: &mut Parts, rest: &str) {
    let (head, hash) = match rest.find('#') {
        Some(i) => (&rest[..i], rest[i + 1..].to_string()),
        None => (rest, String::new()),
    };
    let (path, query) = match head.find('?') {
        Some(i) => (&head[..i], head[i + 1..].to_string()),
        None => (head, String::new()),
    };
    p.path = path.to_string();
    p.query = query;
    p.hash = hash;
}

/// Eine relative Adresse gegen eine Grundlage aufloesen.
pub fn resolve(input: &str, base: &Parts) -> Parts {
    let t = input.trim();
    if let Some(p) = parse_abs(t) { return p }
    let mut p = base.clone();
    p.query.clear();
    p.hash.clear();
    if let Some(r) = t.strip_prefix("//") {
        // Schemarelativ: Host neu, Schema von der Grundlage.
        let mut s = String::from(&p.scheme);
        s.push(':'); s.push_str("//"); s.push_str(r);
        return parse_abs(&s).unwrap_or(p);
    }
    if t.is_empty() { p.query = base.query.clone(); return p }
    if let Some(r) = t.strip_prefix('#') { p.query = base.query.clone(); p.hash = r.to_string(); return p }
    if t.starts_with('?') { split_tail(&mut p, t); p.path = base.path.clone(); return p }
    if t.starts_with('/') { split_tail(&mut p, t); p.path = norm(&p.path); return p }
    // Wirklich relativ: ab dem letzten `/` der Grundlage.
    let dir = match base.path.rfind('/') { Some(i) => &base.path[..=i], None => "/" };
    let joined = alloc::format!("{dir}{t}");
    split_tail(&mut p, &joined);
    p.path = norm(&p.path);
    p
}

/// `.` und `..` aufloesen. Ohne das ist `new URL("../x", base)` eine Adresse,
/// die es nicht gibt.
fn norm(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let abs = path.starts_with('/');
    let trailing = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => { out.pop(); }
            s => out.push(s),
        }
    }
    let mut s = String::new();
    if abs { s.push('/'); }
    s.push_str(&out.join("/"));
    if trailing && !s.ends_with('/') { s.push('/'); }
    if s.is_empty() { s.push('/'); }
    s
}

// ── Prozentkodierung ────────────────────────────────────────────────────

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hexval(b[i + 1]), hexval(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn pct_encode_form(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => { out.push('%'); out.push(hexdig(b >> 4)); out.push(hexdig(b & 15)); }
        }
    }
    out
}

fn hexdig(v: u8) -> char { if v < 10 { (b'0' + v) as char } else { (b'A' + v - 10) as char } }

/// `a=1&b=2` in Paare. Ein Feld ohne `=` hat den leeren Wert.
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&').filter(|s| !s.is_empty()).map(|kv| match kv.find('=') {
        Some(i) => (pct_decode(&kv[..i]), pct_decode(&kv[i + 1..])),
        None => (pct_decode(kv), String::new()),
    }).collect()
}

pub fn build_query(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in pairs {
        if !out.is_empty() { out.push('&'); }
        out.push_str(&pct_encode_form(k));
        out.push('=');
        out.push_str(&pct_encode_form(v));
    }
    out
}

// ── Die Anbindung an die Maschine ───────────────────────────────────────

/// Die zerlegte Adresse liegt als Text auf dem Objekt, nicht als Rust-Wert:
/// eine Seite darf `u.hash = "#x"` schreiben, und dann muss `u.href` sich
/// mitaendern. Ein eingefrorener Rust-Wert koennte das nicht.
const U_HREF: &str = "\0!url";
/// Rueckverweis eines `URLSearchParams` auf sein `URL` — `p.set(…)` muss die
/// Adresse aendern, nicht nur die Kopie.
const U_OWNER: &str = "\0!url.owner";
const U_QUERY: &str = "\0!url.q";

fn parts_of(i: &mut Interp, t: &Value) -> C<Parts> {
    let h = i.get(t, U_HREF)?;
    match &h {
        Value::Str(s) => Ok(parse_abs(s).unwrap_or_default()),
        _ => i.type_err("not a URL"),
    }
}

fn store(i: &mut Interp, t: &Value, p: &Parts) -> C<()> {
    i.set(t, U_HREF, Value::string(p.href()))
}

fn part_accessor(o: &Gc, name: &str, get: NativeFn, set: NativeFn, fp: &Gc) {
    let g = native(Some(fp.clone()), get, name, 0, false);
    let s = native(Some(fp.clone()), set, name, 1, false);
    o.borrow_mut().define(name, Prop {
        value: None, get: Some(Value::Obj(g)), set: Some(Value::Obj(s)),
        writable: false, enumerable: true, configurable: true });
}

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let proto = new_obj(Some(realm.object_proto.clone()));
    let sp_proto = new_obj(Some(realm.object_proto.clone()));
    realm.url_proto = proto.clone();
    realm.url_params_proto = sp_proto.clone();

    let ctor = native(Some(fp.clone()), |i, _, a| {
        let raw = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let parts = match a.get(1) {
            None | Some(Value::Undefined) => match parse_abs(&raw) {
                Some(p) => p,
                // Kein Schema und keine Grundlage: das ist keine Adresse.
                None => return i.type_err(&alloc::format!("invalid URL: {raw}")),
            },
            Some(b) => {
                let bs = i.to_string(b)?;
                let Some(base) = parse_abs(&bs) else {
                    return i.type_err(&alloc::format!("invalid base URL: {bs}"));
                };
                resolve(&raw, &base)
            }
        };
        let g = new_obj(Some(i.realm.url_proto.clone()));
        g.borrow_mut().define(U_HREF, Prop {
            value: Some(Value::string(parts.href())), get: None, set: None,
            writable: true, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, "URL", 1, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("URL")));
    realm.global.borrow_mut().define("URL", Prop::builtin(Value::Obj(ctor)));

    macro_rules! part {
        ($name:literal, $get:expr, $set:expr) => {
            part_accessor(&proto, $name,
                |i, t, _| { let p = parts_of(i, &t)?; let f: fn(&Parts) -> String = $get; Ok(Value::string(f(&p))) },
                |i, t, a| {
                    let v = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
                    let mut p = parts_of(i, &t)?;
                    let f: fn(&mut Parts, &str) = $set;
                    f(&mut p, &v);
                    store(i, &t, &p)?;
                    Ok(Value::Undefined)
                }, &fp);
        };
    }
    part!("href", |p| p.href(), |p, v| { if let Some(n) = parse_abs(v) { *p = n } });
    part!("protocol", |p| alloc::format!("{}:", p.scheme),
          |p, v| p.scheme = v.trim_end_matches(':').to_ascii_lowercase());
    part!("host", |p| p.host_with_port(), |p, v| {
        match v.rfind(':') {
            Some(i) => { p.host = v[..i].to_ascii_lowercase(); p.port = v[i + 1..].to_string() }
            None => p.host = v.to_ascii_lowercase(),
        }
    });
    part!("hostname", |p| p.host.clone(), |p, v| p.host = v.to_ascii_lowercase());
    part!("port", |p| p.port.clone(), |p, v| p.port = v.to_string());
    part!("pathname", |p| p.path.clone(),
          |p, v| p.path = if v.starts_with('/') { v.to_string() } else { alloc::format!("/{v}") });
    part!("search", |p| if p.query.is_empty() { String::new() } else { alloc::format!("?{}", p.query) },
          |p, v| p.query = v.trim_start_matches('?').to_string());
    part!("hash", |p| if p.hash.is_empty() { String::new() } else { alloc::format!("#{}", p.hash) },
          |p, v| p.hash = v.trim_start_matches('#').to_string());

    let og = native(Some(fp.clone()), |i, t, _| {
        let p = parts_of(i, &t)?; Ok(Value::string(p.origin()))
    }, "origin", 0, false);
    proto.borrow_mut().define("origin", Prop {
        value: None, get: Some(Value::Obj(og)), set: None,
        writable: false, enumerable: true, configurable: true });

    let spg = native(Some(fp.clone()), |i, t, _| {
        // Das Objekt haelt seinen Eigentuemer, damit `set`/`append` in die
        // Adresse zurueckschreiben. Ohne den Rueckverweis waere
        // `u.searchParams.set(…)` eine stille Nulloperation — der haeufigste
        // Weg, `URLSearchParams` falsch zu bauen.
        let g = new_obj(Some(i.realm.url_params_proto.clone()));
        g.borrow_mut().define(U_OWNER, Prop {
            value: Some(t.clone()), get: None, set: None,
            writable: false, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, "searchParams", 0, false);
    proto.borrow_mut().define("searchParams", Prop {
        value: None, get: Some(Value::Obj(spg)), set: None,
        writable: false, enumerable: true, configurable: true });

    let d = |o: &Gc, n: &str, f: NativeFn, l: usize, fp: &Gc| {
        let g = native(Some(fp.clone()), f, n, l, false);
        o.borrow_mut().define(n, Prop::builtin(Value::Obj(g)));
    };
    d(&proto, "toString", |i, t, _| { let p = parts_of(i, &t)?; Ok(Value::string(p.href())) }, 0, &fp);
    d(&proto, "toJSON", |i, t, _| { let p = parts_of(i, &t)?; Ok(Value::string(p.href())) }, 0, &fp);

    // ── URLSearchParams ──────────────────────────────────────────────────
    let sp_ctor = native(Some(fp.clone()), |i, _, a| {
        let q = match a.first() {
            None | Some(Value::Undefined) => String::new(),
            Some(v) => i.to_string(v)?.trim_start_matches('?').to_string(),
        };
        let g = new_obj(Some(i.realm.url_params_proto.clone()));
        g.borrow_mut().define(U_QUERY, Prop {
            value: Some(Value::string(q)), get: None, set: None,
            writable: true, enumerable: false, configurable: false });
        Ok(Value::Obj(g))
    }, "URLSearchParams", 0, true);
    sp_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(sp_proto.clone())));
    sp_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(sp_ctor.clone())));
    sp_proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("URLSearchParams")));
    realm.global.borrow_mut().define("URLSearchParams", Prop::builtin(Value::Obj(sp_ctor)));

    d(&sp_proto, "get", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let pairs = sp_read(i, &t)?;
        Ok(match pairs.iter().find(|(n, _)| *n == *k) {
            Some((_, v)) => Value::string(v.clone()), None => Value::Null })
    }, 1, &fp);
    d(&sp_proto, "getAll", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let pairs = sp_read(i, &t)?;
        let out: Vec<Value> = pairs.iter().filter(|(n, _)| *n == *k)
            .map(|(_, v)| Value::string(v.clone())).collect();
        Ok(i.new_array(out))
    }, 1, &fp);
    d(&sp_proto, "has", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let pairs = sp_read(i, &t)?;
        Ok(Value::Bool(pairs.iter().any(|(n, _)| *n == *k)))
    }, 1, &fp);
    d(&sp_proto, "set", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?.to_string();
        // `set` ersetzt das ERSTE Vorkommen und wirft alle weiteren weg —
        // `append` ist das, was mehrfach anhaengt.
        let mut pairs = sp_read(i, &t)?;
        match pairs.iter().position(|(n, _)| *n == k) {
            Some(at) => {
                pairs[at].1 = v;
                let mut n = 0;
                pairs.retain(|(name, _)| { if *name != k { return true }
                                           n += 1; n == 1 });
            }
            None => pairs.push((k, v)),
        }
        sp_write(i, &t, &pairs)?;
        Ok(Value::Undefined)
    }, 2, &fp);
    d(&sp_proto, "append", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?.to_string();
        let mut pairs = sp_read(i, &t)?;
        pairs.push((k, v));
        sp_write(i, &t, &pairs)?;
        Ok(Value::Undefined)
    }, 2, &fp);
    d(&sp_proto, "delete", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let mut pairs = sp_read(i, &t)?;
        pairs.retain(|(n, _)| *n != *k);
        sp_write(i, &t, &pairs)?;
        Ok(Value::Undefined)
    }, 1, &fp);
    d(&sp_proto, "toString", |i, t, _| {
        let pairs = sp_read(i, &t)?; Ok(Value::string(build_query(&pairs)))
    }, 0, &fp);
    d(&sp_proto, "forEach", |i, t, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("callback is not a function") }
        for (k, v) in sp_read(i, &t)? {
            i.tick()?;
            i.call(&f, Value::Undefined, &[Value::string(v), Value::string(k), t.clone()])?;
        }
        Ok(Value::Undefined)
    }, 1, &fp);
    d(&sp_proto, "keys", |i, t, _| {
        let ps = sp_read(i, &t)?;
        let a = i.new_array(ps.into_iter().map(|(k, _)| Value::string(k)).collect());
        i.array_iter(a, 0)
    }, 0, &fp);
    d(&sp_proto, "values", |i, t, _| {
        let ps = sp_read(i, &t)?;
        let a = i.new_array(ps.into_iter().map(|(_, v)| Value::string(v)).collect());
        i.array_iter(a, 0)
    }, 0, &fp);
    d(&sp_proto, "entries", |i, t, _| { let a = sp_entries(i, &t)?; i.array_iter(a, 0) }, 0, &fp);
    let ent = sp_proto.borrow().get_own("entries").and_then(|p| p.value.clone());
    if let Some(e) = ent { sp_proto.borrow_mut().define(SYM_ITERATOR, Prop::builtin(e)); }
}

fn sp_entries(i: &mut Interp, t: &Value) -> C<Value> {
    let ps = sp_read(i, t)?;
    let out: Vec<Value> = ps.into_iter()
        .map(|(k, v)| i.new_array(vec![Value::string(k), Value::string(v)])).collect();
    Ok(i.new_array(out))
}

/// Die Paare lesen — entweder aus der eigenen Zeichenkette oder, wenn das
/// Objekt zu einem `URL` gehoert, aus DESSEN Suchteil.
fn sp_read(i: &mut Interp, t: &Value) -> C<Vec<(String, String)>> {
    let owner = i.get(t, U_OWNER)?;
    if !matches!(owner, Value::Undefined) {
        let p = parts_of(i, &owner)?;
        return Ok(parse_query(&p.query));
    }
    let q = i.get(t, U_QUERY)?;
    Ok(match &q { Value::Str(s) => parse_query(s), _ => Vec::new() })
}

fn sp_write(i: &mut Interp, t: &Value, pairs: &[(String, String)]) -> C<()> {
    let q = build_query(pairs);
    let owner = i.get(t, U_OWNER)?;
    if !matches!(owner, Value::Undefined) {
        let mut p = parts_of(i, &owner)?;
        p.query = q;
        return store(i, &owner, &p);
    }
    i.set(t, U_QUERY, Value::string(q))
}
