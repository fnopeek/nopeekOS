//! `fetch`, `Response`, `Headers` — und `AbortController`/`AbortSignal`.
//!
//! **Warum das eine Runde wert ist.** Der Aufrufzensus stellt
//! `AbortController` mit 319 Aufrufen auf Platz 4 der Luecke, und die
//! Fritzbox-Oberflaeche zeigt, was das heisst: ihr `rest-helper.js` baut im
//! Modulkopf ein `new AbortController()`. Das MODUL scheitert daran, nicht
//! erst der Aufruf — die ganze API-Schicht der Seite ist damit weg, ohne
//! dass etwas kaputt aussieht.
//!
//! **NUR GLEICHE HERKUNFT.** `docs/plan/BROWSER_FETCH_ORIGIN.md` hat das
//! Modell entschieden, und §3.5 staffelt es: A Herkunft/Site + `SameSite`,
//! B Reichweitenriegel im Kernel, C `fetch` gleiche Herkunft, D CORS ganz.
//! Gebaut ist hier **C ohne seine fremde Haelfte**. Eine fremde Herkunft wird
//! abgelehnt und sagt warum.
//!
//! Das ist keine Bequemlichkeit, sondern die Regel des Papiers: *„Eine halbe
//! CORS ist gefaehrlicher als keine."* Ohne Antwortpruefung darf es keine
//! fremde Antwort zu lesen geben — sonst liest eine oeffentliche Seite
//! `https://192.168.178.1/` aus, und §1.4 stellt fest, dass genau davor heute
//! nichts schuetzt. `<img src>` und `<script src>` gehen zwar auch fremd, aber
//! sie geben die BYTES nicht an die Seite zurueck; `fetch` taete es.
//!
//! **Die Engine holt nichts.** Sie legt die Anfrage in `pending_fetches`; der
//! Wirt holt sie ab, laedt und meldet mit `fetch_done`/`fetch_failed` zurueck.
//! Genau der Weg, den `pending_sheets`/`sheet_done` schon geht — kein zweiter
//! daneben. Und `abort()` ist deshalb ECHT und nicht nur eine Fahne: die id
//! wandert nach `aborted_fetches`, der Wirt ruft `npk_http_cancel`.
//!
//! **Was hier NICHT gebaut ist, und woran man es merkt:**
//!
//! * `Request`-Objekte. Eingabe ist eine Zeichenkette (oder etwas, das sich
//!   in eine verwandeln laesst). `fetch(new Request(u))` wirft.
//! * Ruempfe ausser Text — kein `FormData`, kein `Blob`, kein
//!   `ArrayBuffer`. `JSON.stringify(...)` als Rumpf ist der gemessene Fall.
//! * `response.body` als Strom, und `arrayBuffer()`/`blob()`. Es gibt
//!   `text()` und `json()`, und die geben die ganze Antwort auf einmal.
//! * `AbortSignal.timeout(ms)`. Die Zeitgeberliste der Engine ist eine
//!   `Vec<Value>` OHNE Verzoegerung — ein `timeout(5000)` feuerte beim
//!   naechsten Ablauf, also sofort. Lieber nicht da als falsch da
//!   ([[feedback_invented_fallback_hides_the_fault]]).
//!
//! `Headers` haelt den **rohen Kopfblock als Text**, nicht eine Liste. Das
//! ist genau, was der Wirt liefert und was er erwartet — an der Grenze wird
//! damit nichts uebersetzt, und `Set-Cookie` darf sich wiederholen.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::interp::*;
use super::promise;
use super::value::*;

/// Wer auf eine Antwort wartet.
pub enum Waiter {
    /// `fetch()` — das Versprechen wird erfuellt oder abgelehnt.
    Promise(Gc),
    /// `XMLHttpRequest` — das Objekt selbst nimmt die Antwort auf und ruft
    /// seine Behandler.
    Xhr(Gc),
}

/// Eine Anfrage, solange der Wirt sie holt.
pub struct PendingFetch {
    pub id: u32,
    pub url: String,
    pub method: String,
    /// Roher Kopfblock, Zeilen mit `\r\n` getrennt.
    pub headers: String,
    pub body: Option<String>,
}

// ── Verborgene Felder ───────────────────────────────────────────────────
// Dasselbe `\0!`-Muster wie in `url.rs`: kein Name, den JS schreiben kann.
const R_STATUS: &str = "\0!res.status";
const R_TEXT: &str = "\0!res.text";
const R_URL: &str = "\0!res.url";
const R_HDRS: &str = "\0!res.hdrs";
const R_USED: &str = "\0!res.used";
const R_NET: &str = "\0!res.neterr";
const H_RAW: &str = "\0!hdr.raw";
const S_ABORTED: &str = "\0!sig.aborted";
const S_REASON: &str = "\0!sig.reason";
const S_LISTEN: &str = "\0!sig.listen";
const S_FETCH: &str = "\0!sig.fetch";
const C_SIGNAL: &str = "\0!ctl.signal";

fn hidden(v: Value) -> Prop {
    Prop { value: Some(v), get: None, set: None,
           writable: true, enumerable: false, configurable: false }
}

fn slot(i: &mut Interp, t: &Value, k: &str) -> Value {
    i.get(t, k).unwrap_or(Value::Undefined)
}

/// Ein Feld, das ein JS-Array haelt, als Rust-Liste. Ein Array legt seine
/// Elemente als Eigenschaften ab, nicht in einem Rust-Vec — gelesen wird es
/// deshalb ueber den gewoehnlichen Weg.
fn list_of(i: &mut Interp, t: &Value, k: &str) -> Vec<Value> {
    let a = slot(i, t, k);
    if !matches!(a, Value::Obj(_)) { return Vec::new() }
    let n = match i.get(&a, "length") { Ok(Value::Num(n)) => n as usize, _ => 0 };
    (0..n).map(|x| i.get(&a, &alloc::format!("{x}")).unwrap_or(Value::Undefined)).collect()
}

fn meth(o: &Gc, name: &str, f: NativeFn, len: usize, fp: &Gc) {
    let g = native(Some(fp.clone()), f, name, len, false);
    o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
}

fn getter(o: &Gc, name: &str, f: NativeFn, fp: &Gc) {
    let g = native(Some(fp.clone()), f, name, 0, false);
    o.borrow_mut().define(name, Prop {
        value: None, get: Some(Value::Obj(g)), set: None,
        writable: false, enumerable: true, configurable: true });
}

// ── Kopfblock: Text rein, Text raus ─────────────────────────────────────

/// Einen Namen im rohen Block suchen. Kopfnamen sind ohne Ruecksicht auf
/// Gross- und Kleinschreibung gleich — das ist keine Bequemlichkeit, es steht
/// so in RFC 9110, und Server liefern `Content-Type` wie `content-type`.
fn raw_get(raw: &str, name: &str) -> Option<String> {
    let mut hits: Vec<&str> = Vec::new();
    for line in raw.split("\r\n").flat_map(|l| l.split('\n')) {
        let Some((k, v)) = line.split_once(':') else { continue };
        if k.trim().eq_ignore_ascii_case(name.trim()) { hits.push(v.trim()); }
    }
    if hits.is_empty() { return None }
    // Mehrfach gesetzte Koepfe kommen als EINE Zeile mit `, ` zurueck.
    Some(hits.join(", "))
}

fn raw_append(raw: &mut String, name: &str, value: &str) {
    if !raw.is_empty() && !raw.ends_with("\r\n") { raw.push_str("\r\n"); }
    raw.push_str(name.trim());
    raw.push_str(": ");
    raw.push_str(value.trim());
    raw.push_str("\r\n");
}

fn raw_remove(raw: &str, name: &str) -> String {
    let mut out = String::new();
    for line in raw.split("\r\n").flat_map(|l| l.split('\n')) {
        if line.trim().is_empty() { continue }
        let keep = match line.split_once(':') {
            Some((k, _)) => !k.trim().eq_ignore_ascii_case(name.trim()),
            None => true,
        };
        if keep { out.push_str(line); out.push_str("\r\n"); }
    }
    out
}

fn new_headers(i: &Interp, raw: String) -> Gc {
    let h = new_obj(Some(i.realm.headers_proto.clone()));
    h.borrow_mut().define(H_RAW, hidden(Value::string(raw)));
    h
}

fn raw_of(i: &mut Interp, t: &Value) -> String {
    match slot(i, t, H_RAW) { Value::Str(s) => s.to_string(), _ => String::new() }
}

// ── AbortSignal ─────────────────────────────────────────────────────────

/// Der Grund, mit dem ein Abbruch ohne eigenen Grund ablehnt.
///
/// Ein `DOMException` gibt es in dieser Engine nicht; gebaut wird deshalb ein
/// `Error` mit dem NAMEN, auf den Seitencode prueft. Ueber `throw_kind`, damit
/// die Fehlerobjekte hier nicht ein zweites Mal entstehen.
fn abort_error(i: &mut Interp) -> Value {
    let Abrupt::Throw(v) = i.throw_kind("Error", "signal is aborted without reason")
        else { return Value::Undefined };
    if let Value::Obj(o) = &v {
        o.borrow_mut().define("name", Prop::builtin(Value::str("AbortError")));
    }
    v
}

pub(crate) fn new_signal(i: &Interp) -> Gc {
    let s = new_obj(Some(i.realm.abort_signal_proto.clone()));
    {
        let mut b = s.borrow_mut();
        b.define(S_ABORTED, hidden(Value::Bool(false)));
        b.define(S_REASON, hidden(Value::Undefined));
        b.define(S_LISTEN, hidden(Value::Undefined));
        b.define(S_FETCH, hidden(Value::Undefined));
    }
    s
}

fn signal_aborted(i: &mut Interp, sig: &Value) -> bool {
    matches!(slot(i, sig, S_ABORTED), Value::Bool(true))
}

/// Ein Signal auf „abgebrochen" setzen und alles benachrichtigen, was daran
/// haengt: die angemeldeten Behandler, `onabort`, und die laufende Anfrage.
fn do_abort(i: &mut Interp, sig: &Value, reason: Value) -> C<()> {
    if signal_aborted(i, sig) { return Ok(()) }
    let r = if matches!(reason, Value::Undefined) { abort_error(i) } else { reason };
    i.set(sig, S_ABORTED, Value::Bool(true), false)?;
    i.set(sig, S_REASON, r.clone(), false)?;

    // Die laufende Anfrage wirklich abbrechen — nicht nur die Fahne setzen.
    if let Value::Num(id) = slot(i, sig, S_FETCH) {
        let id = id as u32;
        i.aborted_fetches.push(id);
        fetch_failed_with(i, id, r.clone());
    }

    let ev = new_obj(Some(i.realm.object_proto.clone()));
    ev.borrow_mut().define("type", Prop::builtin(Value::str("abort")));
    ev.borrow_mut().define("target", Prop::builtin(sig.clone()));
    let ev = Value::Obj(ev);

    let on = i.get(sig, "onabort")?;
    if i.is_callable(&on) { i.call(&on, sig.clone(), &[ev.clone()])?; }
    {
        let items = list_of(i, sig, S_LISTEN);
        for f in items {
            if i.is_callable(&f) { i.call(&f, sig.clone(), &[ev.clone()])?; }
        }
    }
    Ok(())
}

// ── fetch ───────────────────────────────────────────────────────────────

/// Die Kopfzeilen aus dem `headers`-Feld der Init lesen.
///
/// Zwei Formen kommen vor: ein gewoehnliches Objekt und ein `Headers`. Beide
/// enden im selben rohen Block.
fn init_headers(i: &mut Interp, init: &Value) -> C<String> {
    let h = i.get(init, "headers")?;
    let Value::Obj(o) = &h else { return Ok(String::new()) };
    if Rc::ptr_eq(&o.borrow().proto.clone().unwrap_or_else(|| i.realm.object_proto.clone()),
                  &i.realm.headers_proto) {
        return Ok(raw_of(i, &h));
    }
    let mut raw = String::new();
    let keys = o.borrow().own_keys();
    for k in keys {
        let v = i.get(&h, &k)?;
        let vs = i.to_string(&v)?;
        raw_append(&mut raw, &k, &vs);
    }
    Ok(raw)
}

/// **`fetch` LEHNT AB, es wirft nicht.** Ein Netz- oder Herkunftsfehler
/// gehoert ins `catch` des Rufers; wer hier wirft, beendet stattdessen das
/// rufende Skript — und alles danach laeuft nicht mehr. Die eigene Probe ist
/// genau darueber gestolpert, zum zweiten Mal in dieser Datei.
fn do_fetch(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {
    match do_fetch_inner(i, t, a) {
        Ok(v) => Ok(v),
        Err(Abrupt::Throw(e)) => {
            let p = promise::new_promise(i);
            promise::settle(i, &p, e, true);
            Ok(Value::Obj(p))
        }
        Err(e) => Err(e),
    }
}

fn do_fetch_inner(i: &mut Interp, _t: Value, a: &[Value]) -> C<Value> {
    let input = a.first().cloned().unwrap_or(Value::Undefined);
    if let Value::Obj(o) = &input {
        // Ein `Request` gibt es nicht. Das zu sagen ist besser, als seine
        // Felder zu erraten und die Anfrage still falsch zu stellen.
        if o.borrow().get_own("url").is_some() && o.borrow().get_own("method").is_some() {
            return i.type_err("fetch: Request objects are not supported, pass a URL string");
        }
    }
    let raw = i.to_string(&input)?.to_string();
    let init = a.get(1).cloned().unwrap_or(Value::Undefined);

    let Some(url) = same_origin_url(i, &raw)? else {
        let base = document_origin(i).unwrap_or_default();
        return i.type_err(&alloc::format!(
            "fetch: {raw} is a different origin than {base} — cross-origin fetch is not \
             built yet (see docs/plan/BROWSER_FETCH_ORIGIN.md)"));
    };

    let (method, headers, body, signal) = if matches!(init, Value::Obj(_)) {
        let m = match i.get(&init, "method")? {
            Value::Undefined => "GET".to_string(),
            v => i.to_string(&v)?.to_uppercase(),
        };
        let h = init_headers(i, &init)?;
        let b = match i.get(&init, "body")? {
            Value::Undefined | Value::Null => None,
            v => Some(i.to_string(&v)?.to_string()),
        };
        (m, h, b, i.get(&init, "signal")?)
    } else {
        ("GET".to_string(), String::new(), None, Value::Undefined)
    };

    let p = promise::new_promise(i);

    // Schon abgebrochen, bevor es losging: dann geht gar nichts los.
    if matches!(signal, Value::Obj(_)) && signal_aborted(i, &signal) {
        let r = slot(i, &signal, S_REASON);
        promise::settle(i, &p, r, true);
        return Ok(Value::Obj(p));
    }

    let id = i.next_fetch_id;
    i.next_fetch_id += 1;
    i.pending_fetches.push(PendingFetch { id, url, method, headers, body });
    i.fetch_waiting.push((id, Waiter::Promise(p.clone())));
    if matches!(signal, Value::Obj(_)) {
        i.set(&signal, S_FETCH, Value::Num(id as f64), false)?;
    }
    Ok(Value::Obj(p))
}

/// Die Herkunft des Dokuments, als Text.
fn document_origin(i: &mut Interp) -> Option<String> {
    let g = Value::Obj(i.realm.global.clone());
    let loc = i.get(&g, "location").ok()?;
    let href = i.get(&loc, "href").ok()?;
    let Value::Str(h) = href else { return None };
    super::url::parse_abs(&h).map(|p| p.origin())
}

/// Eine Adresse aufloesen und auf gleiche Herkunft pruefen. `None` heisst
/// fremd — was der Rufer daraus macht, ist bei `fetch` eine Ablehnung und
/// bei `XMLHttpRequest` ein Fehlerereignis.
///
/// **Hier wird aufgeloest, und nur hier.** Zwei Aufloeser waeren zwei
/// Meinungen darueber, was `../` bedeutet, und die eine davon entschiede dann
/// ueber die Herkunft ([[feedback_the_probe_must_use_the_targets_resolver]]).
fn same_origin_url(i: &mut Interp, raw: &str) -> C<Option<String>> {
    let g = Value::Obj(i.realm.global.clone());
    let loc = i.get(&g, "location")?;
    let href = i.get(&loc, "href")?;
    let Value::Str(h) = href else {
        return i.type_err("the document has no origin to fetch from");
    };
    let Some(base) = super::url::parse_abs(&h) else {
        return i.type_err("the document has no origin to fetch from");
    };
    let target = super::url::resolve(raw, &base);
    if target.origin() != base.origin() { return Ok(None) }
    Ok(Some(target.href()))
}

// ── Was der Wirt zurueckmeldet ──────────────────────────────────────────

fn take_waiting(i: &mut Interp, id: u32) -> Option<Waiter> {
    let k = i.fetch_waiting.iter().position(|(n, _)| *n == id)?;
    Some(i.fetch_waiting.remove(k).1)
}

/// Eine Antwort ist da. `raw_headers` ist der Kopfblock ohne die Statuszeile.
pub fn fetch_done(i: &mut Interp, id: u32, status: u16, final_url: &str,
                  raw_headers: &str, body: String) {
    let Some(w) = take_waiting(i, id) else { return };
    let p = match w {
        Waiter::Promise(p) => p,
        Waiter::Xhr(x) => { xhr_done(i, &x, status, final_url, raw_headers, body); return }
    };
    let hdrs = new_headers(i, raw_headers.to_string());
    let r = new_obj(Some(i.realm.response_proto.clone()));
    {
        let mut b = r.borrow_mut();
        b.define(R_STATUS, hidden(Value::Num(status as f64)));
        b.define(R_TEXT, hidden(Value::string(body)));
        b.define(R_URL, hidden(Value::str(final_url)));
        b.define(R_HDRS, hidden(Value::Obj(hdrs)));
        b.define(R_USED, hidden(Value::Bool(false)));
        b.define(R_NET, hidden(Value::Bool(false)));
    }
    promise::settle(i, &p, Value::Obj(r), false);
}

/// Die Anfrage ist gescheitert. **Ein `fetch` lehnt mit `TypeError` ab** —
/// nicht mit dem Status. Ein 404 ist eine ANTWORT und wird erfuellt; nur ein
/// Netzfehler ist eine Ablehnung, und Seitencode unterscheidet danach.
pub fn fetch_failed(i: &mut Interp, id: u32, why: &str) {
    let Abrupt::Throw(v) = i.throw_kind("TypeError", &alloc::format!("Failed to fetch: {why}"))
        else { return };
    fetch_failed_with(i, id, v);
}

fn fetch_failed_with(i: &mut Interp, id: u32, reason: Value) {
    let Some(w) = take_waiting(i, id) else { return };
    match w {
        Waiter::Promise(p) => promise::settle(i, &p, reason, true),
        Waiter::Xhr(x) => xhr_failed(i, &x),
    }
}

// ── Einbau ──────────────────────────────────────────────────────────────

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let op = realm.object_proto.clone();

    // ── Headers ─────────────────────────────────────────────────────────
    let h_proto = new_obj(Some(op.clone()));
    realm.headers_proto = h_proto.clone();
    let h_ctor = native(Some(fp.clone()), |i, _, a| {
        let mut raw = String::new();
        if let Some(v @ Value::Obj(_)) = a.first() {
            let init = new_obj(Some(i.realm.object_proto.clone()));
            init.borrow_mut().define("headers", Prop::builtin(v.clone()));
            raw = init_headers(i, &Value::Obj(init))?;
        }
        Ok(Value::Obj(new_headers(i, raw)))
    }, "Headers", 0, true);
    h_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(h_proto.clone())));
    h_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(h_ctor.clone())));
    realm.global.borrow_mut().define("Headers", Prop::builtin(Value::Obj(h_ctor)));

    meth(&h_proto, "get", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let raw = raw_of(i, &t);
        // `null`, nicht `undefined` — daran unterscheidet Seitencode
        // „nicht gesetzt" von „leer gesetzt".
        Ok(raw_get(&raw, &n).map(Value::string).unwrap_or(Value::Null))
    }, 1, &fp);
    meth(&h_proto, "has", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let raw = raw_of(i, &t);
        Ok(Value::Bool(raw_get(&raw, &n).is_some()))
    }, 1, &fp);
    meth(&h_proto, "append", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?.to_string();
        let mut raw = raw_of(i, &t);
        raw_append(&mut raw, &n, &v);
        i.set(&t, H_RAW, Value::string(raw), false)?;
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&h_proto, "set", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?.to_string();
        let mut raw = raw_remove(&raw_of(i, &t), &n);
        raw_append(&mut raw, &n, &v);
        i.set(&t, H_RAW, Value::string(raw), false)?;
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&h_proto, "delete", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let raw = raw_remove(&raw_of(i, &t), &n);
        i.set(&t, H_RAW, Value::string(raw), false)?;
        Ok(Value::Undefined)
    }, 1, &fp);
    meth(&h_proto, "forEach", |i, t, a| {
        let f = a.first().cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return i.type_err("Headers.forEach needs a function") }
        let raw = raw_of(i, &t);
        let pairs: Vec<(String, String)> = raw.split("\r\n").flat_map(|l| l.split('\n'))
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
            .collect();
        let this = a.get(1).cloned().unwrap_or(Value::Undefined);
        for (k, v) in pairs {
            i.call(&f, this.clone(), &[Value::string(v), Value::string(k), t.clone()])?;
        }
        Ok(Value::Undefined)
    }, 1, &fp);

    // ── Response ────────────────────────────────────────────────────────
    let r_proto = new_obj(Some(op.clone()));
    realm.response_proto = r_proto.clone();
    let r_ctor = native(Some(fp.clone()), |i, _, a| {
        let body = match a.first() {
            None | Some(Value::Undefined) | Some(Value::Null) => String::new(),
            Some(v) => i.to_string(v)?.to_string(),
        };
        let status = match a.get(1) {
            Some(o @ Value::Obj(_)) => match i.get(o, "status")? {
                Value::Undefined => 200.0,
                v => i.to_number(&v)?,
            },
            _ => 200.0,
        };
        let hdrs = new_headers(i, String::new());
        let r = new_obj(Some(i.realm.response_proto.clone()));
        {
            let mut b = r.borrow_mut();
            b.define(R_STATUS, hidden(Value::Num(status)));
            b.define(R_TEXT, hidden(Value::string(body)));
            b.define(R_URL, hidden(Value::str("")));
            b.define(R_HDRS, hidden(Value::Obj(hdrs)));
            b.define(R_USED, hidden(Value::Bool(false)));
            b.define(R_NET, hidden(Value::Bool(false)));
        }
        Ok(Value::Obj(r))
    }, "Response", 0, true);
    r_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(r_proto.clone())));
    r_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(r_ctor.clone())));
    realm.global.borrow_mut().define("Response", Prop::builtin(Value::Obj(r_ctor)));

    getter(&r_proto, "status", |i, t, _| Ok(slot(i, &t, R_STATUS)), &fp);
    getter(&r_proto, "ok", |i, t, _| {
        let s = match slot(i, &t, R_STATUS) { Value::Num(n) => n, _ => 0.0 };
        Ok(Value::Bool((200.0..300.0).contains(&s)))
    }, &fp);
    getter(&r_proto, "statusText", |i, t, _| {
        let s = match slot(i, &t, R_STATUS) { Value::Num(n) => n as u16, _ => 0 };
        Ok(Value::str(status_text(s)))
    }, &fp);
    getter(&r_proto, "url", |i, t, _| Ok(slot(i, &t, R_URL)), &fp);
    getter(&r_proto, "headers", |i, t, _| Ok(slot(i, &t, R_HDRS)), &fp);
    getter(&r_proto, "redirected", |_, _, _| Ok(Value::Bool(false)), &fp);
    getter(&r_proto, "type", |_, _, _| Ok(Value::str("basic")), &fp);
    getter(&r_proto, "bodyUsed", |i, t, _| Ok(slot(i, &t, R_USED)), &fp);

    // **Beide LEHNEN AB, sie werfen nicht.** Ein zweites `text()` auf
    // derselben Antwort ist ein Fehler — aber ein Fehler im Versprechen. Wer
    // hier wirft, beendet das rufende Skript, statt in dessen `catch` zu
    // landen; die eigene Probe ist genau darueber gestolpert.
    meth(&r_proto, "text", |i, t, _| {
        let p = promise::new_promise(i);
        match body_once(i, &t) {
            Ok(v) => promise::settle(i, &p, v, false),
            Err(Abrupt::Throw(e)) => promise::settle(i, &p, e, true),
            Err(_) => promise::settle(i, &p, Value::Undefined, true),
        }
        Ok(Value::Obj(p))
    }, 0, &fp);
    meth(&r_proto, "json", |i, t, _| {
        let p = promise::new_promise(i);
        // **Derselbe Leser wie `JSON.parse`.** Ein eigener waere eine zweite
        // Semantik, und die laeuft still auseinander.
        let r = body_once(i, &t).and_then(|v| super::json::parse_value(i, &v));
        match r {
            Ok(x) => promise::settle(i, &p, x, false),
            Err(Abrupt::Throw(e)) => promise::settle(i, &p, e, true),
            Err(_) => promise::settle(i, &p, Value::Undefined, true),
        }
        Ok(Value::Obj(p))
    }, 0, &fp);
    meth(&r_proto, "clone", |i, t, _| {
        let r = new_obj(Some(i.realm.response_proto.clone()));
        for k in [R_STATUS, R_TEXT, R_URL, R_HDRS, R_NET] {
            let v = slot(i, &t, k);
            r.borrow_mut().define(k, hidden(v));
        }
        r.borrow_mut().define(R_USED, hidden(Value::Bool(false)));
        Ok(Value::Obj(r))
    }, 0, &fp);

    // ── AbortSignal ─────────────────────────────────────────────────────
    let s_proto = new_obj(Some(op.clone()));
    realm.abort_signal_proto = s_proto.clone();
    let s_ctor = native(Some(fp.clone()), |i, _, _| {
        // Wie im Browser: ein Signal entsteht am Controller, nicht mit `new`.
        i.type_err("Illegal constructor")
    }, "AbortSignal", 0, true);
    s_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(s_proto.clone())));
    s_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(s_ctor.clone())));
    meth(&s_ctor, "abort", |i, _, a| {
        let s = new_signal(i);
        let sv = Value::Obj(s);
        do_abort(i, &sv, a.first().cloned().unwrap_or(Value::Undefined))?;
        Ok(sv)
    }, 0, &fp);
    realm.global.borrow_mut().define("AbortSignal", Prop::builtin(Value::Obj(s_ctor)));

    getter(&s_proto, "aborted", |i, t, _| Ok(slot(i, &t, S_ABORTED)), &fp);
    getter(&s_proto, "reason", |i, t, _| Ok(slot(i, &t, S_REASON)), &fp);
    meth(&s_proto, "throwIfAborted", |i, t, _| {
        if signal_aborted(i, &t) { return Err(Abrupt::Throw(slot(i, &t, S_REASON))) }
        Ok(Value::Undefined)
    }, 0, &fp);
    // Ein `AbortSignal` ist kein Knoten, also kann es die Anmeldung des
    // Dokuments nicht mitbenutzen — `addEventListener` dort verlangt eine
    // Knoten-id. Es gibt hier genau EINE Art Ereignis, und die Liste dafuer
    // haengt am Signal selbst.
    meth(&s_proto, "addEventListener", |i, t, a| {
        let ev = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if ev != "abort" || !i.is_callable(&f) { return Ok(Value::Undefined) }
        let mut items = list_of(i, &t, S_LISTEN);
        items.push(f);
        let arr = i.new_array(items);
        i.set(&t, S_LISTEN, arr, false)?;
        Ok(Value::Undefined)
    }, 2, &fp);
    meth(&s_proto, "removeEventListener", |i, t, a| {
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        let items = list_of(i, &t, S_LISTEN);
        let keep: Vec<Value> = items.into_iter().filter(|x| !x.strict_eq(&f)).collect();
        let arr = i.new_array(keep);
        i.set(&t, S_LISTEN, arr, false)?;
        Ok(Value::Undefined)
    }, 2, &fp);

    // ── AbortController ─────────────────────────────────────────────────
    let c_proto = new_obj(Some(op.clone()));
    realm.abort_ctrl_proto = c_proto.clone();
    let c_ctor = native(Some(fp.clone()), |i, _, _| {
        let c = new_obj(Some(i.realm.abort_ctrl_proto.clone()));
        let s = new_signal(i);
        c.borrow_mut().define(C_SIGNAL, hidden(Value::Obj(s)));
        Ok(Value::Obj(c))
    }, "AbortController", 0, true);
    c_ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(c_proto.clone())));
    c_proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(c_ctor.clone())));
    realm.global.borrow_mut().define("AbortController", Prop::builtin(Value::Obj(c_ctor)));

    getter(&c_proto, "signal", |i, t, _| Ok(slot(i, &t, C_SIGNAL)), &fp);
    meth(&c_proto, "abort", |i, t, a| {
        let s = slot(i, &t, C_SIGNAL);
        do_abort(i, &s, a.first().cloned().unwrap_or(Value::Undefined))?;
        Ok(Value::Undefined)
    }, 0, &fp);

    // ── fetch ───────────────────────────────────────────────────────────
    let f = native(Some(fp.clone()), do_fetch, "fetch", 1, false);
    realm.global.borrow_mut().define("fetch", Prop::builtin(Value::Obj(f)));

    install_xhr(realm);
}

/// Den Rumpf EINMAL hergeben. `bodyUsed` ist kein Schmuck: eine Antwort
/// zweimal zu lesen ist ein Fehler, und Seitencode baut darauf, dass er ihn
/// bekommt statt einer leeren Zeichenkette.
fn body_once(i: &mut Interp, t: &Value) -> C<Value> {
    if matches!(slot(i, t, R_USED), Value::Bool(true)) {
        return i.type_err("body stream already read");
    }
    i.set(t, R_USED, Value::Bool(true), false)?;
    Ok(slot(i, t, R_TEXT))
}

/// Die Statuszeilen, die vorkommen. Kein vollstaendiger Katalog — was fehlt,
/// bekommt eine leere Zeichenkette, und das ist auch, was ein Browser fuer
/// einen unbekannten Code liefert.
fn status_text(s: u16) -> &'static str {
    match s {
        200 => "OK", 201 => "Created", 202 => "Accepted", 204 => "No Content",
        301 => "Moved Permanently", 302 => "Found", 303 => "See Other",
        304 => "Not Modified", 307 => "Temporary Redirect", 308 => "Permanent Redirect",
        400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden",
        404 => "Not Found", 405 => "Method Not Allowed", 409 => "Conflict",
        413 => "Payload Too Large", 415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        500 => "Internal Server Error", 501 => "Not Implemented",
        502 => "Bad Gateway", 503 => "Service Unavailable", 504 => "Gateway Timeout",
        _ => "",
    }
}

// ── XMLHttpRequest ──────────────────────────────────────────────────────
//
// **Warum das gebaut ist, obwohl es `fetch` gibt.** Googles Startseite
// entscheidet mit EINER Zeile, ob sie eine Seite mit oder ohne JavaScript
// ausliefert:
//
//     if (typeof XMLHttpRequest != "undefined") b = "2";
//     …
//     if (a == "2" && …) g.value = a;      // <input id="gbv" value="1">
//
// Ohne `XMLHttpRequest` bleibt `gbv=1`, und Google schickt die Seite „bitte
// aktiviere JavaScript". Host-seitig nachgestellt: `GBV=1 xhr=undefined`
// gegen `GBV=2 xhr=function` — alles andere an beak tat schon, was es soll.
//
// **Und deshalb darf es kein Stummel sein.** Ein `function(){}` haette
// gereicht, damit Google `gbv=2` setzt — und dann BENUTZT die Seite es. Ein
// Merkmal vorzutaeuschen ist schlimmer, als es nicht zu haben
// ([[feedback_a_workaround_is_the_wrong_answer_to_a_missing_capability]]).
//
// Dieselbe Leitung wie `fetch`, dieselbe Herkunftsregel: nur gleiche
// Herkunft, siehe `docs/plan/BROWSER_FETCH_ORIGIN.md`.
//
// NICHT gebaut: der SYNCHRONE Modus (`open(…, false)`). Die Engine kann
// nicht blockieren — sie gibt die Kontrolle an den Wirt zurueck, und der
// holt. Ein synchrones `send` wirft deshalb und sagt warum, statt still
// etwas Leeres zu liefern.

const X_METHOD: &str = "\0!xhr.method";
const X_URL: &str = "\0!xhr.url";
const X_HDRS: &str = "\0!xhr.hdrs";
const X_STATE: &str = "\0!xhr.state";
const X_STATUS: &str = "\0!xhr.status";
const X_TEXT: &str = "\0!xhr.text";
const X_RESHDRS: &str = "\0!xhr.reshdrs";
const X_FETCH: &str = "\0!xhr.fetch";
const X_LISTEN: &str = "\0!xhr.listen";

fn xhr_state(i: &mut Interp, x: &Value, n: f64) -> C<()> {
    i.set(x, X_STATE, Value::Num(n), false)?;
    xhr_fire(i, x, "readystatechange")
}

/// Einen Behandler rufen: `onX` und die ueber `addEventListener` angemeldeten.
fn xhr_fire(i: &mut Interp, x: &Value, kind: &str) -> C<()> {
    let ev = new_obj(Some(i.realm.object_proto.clone()));
    ev.borrow_mut().define("type", Prop::builtin(Value::str(kind)));
    ev.borrow_mut().define("target", Prop::builtin(x.clone()));
    let ev = Value::Obj(ev);
    let on = i.get(x, &alloc::format!("on{kind}"))?;
    if i.is_callable(&on) { i.call(&on, x.clone(), &[ev.clone()])?; }
    for f in list_of(i, x, X_LISTEN) {
        if !matches!(f, Value::Obj(_)) { continue }
        let k = i.get(&f, "0")?;
        let g = i.get(&f, "1")?;
        if i.to_string(&k)?.as_ref() == kind && i.is_callable(&g) {
            i.call(&g, x.clone(), &[ev.clone()])?;
        }
    }
    Ok(())
}

fn xhr_done(i: &mut Interp, x: &Gc, status: u16, url: &str, raw: &str, body: String) {
    let xv = Value::Obj(x.clone());
    let _ = i.set(&xv, X_STATUS, Value::Num(status as f64), false);
    let _ = i.set(&xv, X_TEXT, Value::string(body), false);
    let _ = i.set(&xv, X_RESHDRS, Value::str(raw), false);
    let _ = i.set(&xv, X_URL, Value::str(url), false);
    let _ = i.set(&xv, X_FETCH, Value::Undefined, false);
    // 2, 3, 4 der Reihe nach — Seitencode prueft beides, `readyState` UND
    // die Ereignisse, und manche warten auf die Zwischenstufen.
    let _ = xhr_state(i, &xv, 2.0);
    let _ = xhr_state(i, &xv, 3.0);
    let _ = xhr_state(i, &xv, 4.0);
    let _ = xhr_fire(i, &xv, "load");
    let _ = xhr_fire(i, &xv, "loadend");
}

fn xhr_failed(i: &mut Interp, x: &Gc) {
    let xv = Value::Obj(x.clone());
    // **Status 0, nicht ein erfundener Fehlercode.** So unterscheidet
    // Seitencode einen Netzfehler von einer Antwort mit 500.
    let _ = i.set(&xv, X_STATUS, Value::Num(0.0), false);
    let _ = i.set(&xv, X_TEXT, Value::str(""), false);
    let _ = i.set(&xv, X_FETCH, Value::Undefined, false);
    let _ = xhr_state(i, &xv, 4.0);
    let _ = xhr_fire(i, &xv, "error");
    let _ = xhr_fire(i, &xv, "loadend");
}

pub(crate) fn install_xhr(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let op = realm.object_proto.clone();
    let proto = new_obj(Some(op.clone()));
    realm.xhr_proto = proto.clone();

    let ctor = native(Some(fp.clone()), |i, _, _| {
        let x = new_obj(Some(i.realm.xhr_proto.clone()));
        {
            let mut b = x.borrow_mut();
            b.define(X_METHOD, hidden(Value::str("GET")));
            b.define(X_URL, hidden(Value::str("")));
            b.define(X_HDRS, hidden(Value::str("")));
            b.define(X_STATE, hidden(Value::Num(0.0)));
            b.define(X_STATUS, hidden(Value::Num(0.0)));
            b.define(X_TEXT, hidden(Value::str("")));
            b.define(X_RESHDRS, hidden(Value::str("")));
            b.define(X_FETCH, hidden(Value::Undefined));
            b.define(X_LISTEN, hidden(Value::Undefined));
            b.define("onreadystatechange", hidden(Value::Undefined));
            b.define("onload", hidden(Value::Undefined));
            b.define("onerror", hidden(Value::Undefined));
            b.define("onabort", hidden(Value::Undefined));
            b.define("onloadend", hidden(Value::Undefined));
            b.define("withCredentials", hidden(Value::Bool(false)));
            b.define("responseType", hidden(Value::str("")));
            b.define("timeout", hidden(Value::Num(0.0)));
        }
        Ok(Value::Obj(x))
    }, "XMLHttpRequest", 0, true);
    ctor.borrow_mut().define("prototype", Prop::frozen(Value::Obj(proto.clone())));
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    for (n, v) in [("UNSENT", 0.0), ("OPENED", 1.0), ("HEADERS_RECEIVED", 2.0),
                   ("LOADING", 3.0), ("DONE", 4.0)] {
        ctor.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
        proto.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    realm.global.borrow_mut().define("XMLHttpRequest", Prop::builtin(Value::Obj(ctor)));

    meth(&proto, "open", |i, t, a| {
        let m = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_uppercase();
        let u = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?.to_string();
        if let Some(v) = a.get(2) {
            if !v.truthy() {
                return i.type_err("XMLHttpRequest: synchronous send is not supported \
                                   (the engine cannot block; use the async form)");
            }
        }
        i.set(&t, X_METHOD, Value::string(m), false)?;
        i.set(&t, X_URL, Value::string(u), false)?;
        i.set(&t, X_HDRS, Value::str(""), false)?;
        xhr_state(i, &t, 1.0)?;
        Ok(Value::Undefined)
    }, 2, &fp);

    meth(&proto, "setRequestHeader", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let v = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?.to_string();
        let mut raw = match slot(i, &t, X_HDRS) { Value::Str(s) => s.to_string(), _ => String::new() };
        raw_append(&mut raw, &n, &v);
        i.set(&t, X_HDRS, Value::string(raw), false)?;
        Ok(Value::Undefined)
    }, 2, &fp);

    meth(&proto, "send", |i, t, a| {
        let uv = slot(i, &t, X_URL);
        let raw = i.to_string(&uv)?.to_string();
        let mv = slot(i, &t, X_METHOD);
        let method = i.to_string(&mv)?.to_string();
        let hdrs = match slot(i, &t, X_HDRS) { Value::Str(s) => s.to_string(), _ => String::new() };
        let body = match a.first() {
            None | Some(Value::Undefined) | Some(Value::Null) => None,
            Some(v) => Some(i.to_string(v)?.to_string()),
        };
        let Some(url) = same_origin_url(i, &raw)? else {
            // Fremde Herkunft: kein Wurf, ein FEHLEREREIGNIS — so wie im
            // Browser ohne CORS. Siehe Kopf dieser Datei.
            if let Value::Obj(x) = &t { xhr_failed(i, x); }
            return Ok(Value::Undefined);
        };
        let id = i.next_fetch_id;
        i.next_fetch_id += 1;
        i.pending_fetches.push(PendingFetch { id, url, method, headers: hdrs, body });
        if let Value::Obj(x) = &t { i.fetch_waiting.push((id, Waiter::Xhr(x.clone()))); }
        i.set(&t, X_FETCH, Value::Num(id as f64), false)?;
        Ok(Value::Undefined)
    }, 1, &fp);

    meth(&proto, "abort", |i, t, _| {
        if let Value::Num(id) = slot(i, &t, X_FETCH) {
            i.aborted_fetches.push(id as u32);
            let k = i.fetch_waiting.iter().position(|(n, _)| *n == id as u32);
            if let Some(k) = k { i.fetch_waiting.remove(k); }
            i.set(&t, X_FETCH, Value::Undefined, false)?;
            xhr_state(i, &t, 4.0)?;
            xhr_fire(i, &t, "abort")?;
            xhr_fire(i, &t, "loadend")?;
        }
        Ok(Value::Undefined)
    }, 0, &fp);

    meth(&proto, "addEventListener", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !i.is_callable(&f) { return Ok(Value::Undefined) }
        let pair = i.new_array(alloc::vec![Value::string(k), f]);
        let mut items = list_of(i, &t, X_LISTEN);
        items.push(pair);
        let arr = i.new_array(items);
        i.set(&t, X_LISTEN, arr, false)?;
        Ok(Value::Undefined)
    }, 2, &fp);

    meth(&proto, "getAllResponseHeaders", |i, t, _| Ok(slot(i, &t, X_RESHDRS)), 0, &fp);
    meth(&proto, "getResponseHeader", |i, t, a| {
        let n = i.to_string(a.first().unwrap_or(&Value::Undefined))?.to_string();
        let raw = match slot(i, &t, X_RESHDRS) { Value::Str(s) => s.to_string(), _ => String::new() };
        Ok(raw_get(&raw, &n).map(Value::string).unwrap_or(Value::Null))
    }, 1, &fp);
    meth(&proto, "overrideMimeType", |_, _, _| Ok(Value::Undefined), 1, &fp);

    getter(&proto, "readyState", |i, t, _| Ok(slot(i, &t, X_STATE)), &fp);
    getter(&proto, "status", |i, t, _| Ok(slot(i, &t, X_STATUS)), &fp);
    getter(&proto, "statusText", |i, t, _| {
        let s = match slot(i, &t, X_STATUS) { Value::Num(n) => n as u16, _ => 0 };
        Ok(Value::str(status_text(s)))
    }, &fp);
    getter(&proto, "responseText", |i, t, _| Ok(slot(i, &t, X_TEXT)), &fp);
    getter(&proto, "responseURL", |i, t, _| Ok(slot(i, &t, X_URL)), &fp);
    getter(&proto, "response", |i, t, _| {
        let rtv = slot(i, &t, "responseType");
        let rt = i.to_string(&rtv)?.to_string();
        let text = slot(i, &t, X_TEXT);
        if rt == "json" {
            return Ok(super::json::parse_value(i, &text).unwrap_or(Value::Null));
        }
        Ok(text)
    }, &fp);
}

#[cfg(test)]
mod tests {
    use super::super::interp::{Interp, Value};

    fn ausdruck(i: &mut Interp, src: &str) -> alloc::string::String {
        let prog = crate::js::parse(src, false).expect("parst");
        match i.run_program(&prog) {
            Ok(v) => i.to_string(&v).map(|s| s.to_string())
                      .unwrap_or_else(|_| alloc::string::String::from("?")),
            Err(super::Abrupt::Throw(v)) => {
                let m = i.get(&v, "message").ok()
                    .and_then(|m| i.to_string(&m).ok())
                    .unwrap_or_else(|| alloc::rc::Rc::from("?"));
                alloc::format!("THROW {m}")
            }
            Err(_) => alloc::string::String::from("ABRUPT"),
        }
    }

    /// **Der ganze Weg eines `XMLHttpRequest`, ohne Wirt.**
    ///
    /// Google entscheidet mit `typeof XMLHttpRequest != "undefined"`, ob es
    /// eine Seite mit JavaScript ausliefert. Ein Stummel haette dafuer
    /// gereicht — und die Seite haette ihn dann BENUTZT. Der Test faehrt
    /// deshalb die Anfrage bis zur Antwort durch: anlegen, oeffnen,
    /// abschicken, den Wirt antworten lassen, Zustaende und Text pruefen.
    #[test]
    fn xhr_faehrt_eine_anfrage_zu_ende() {
        let mut i = Interp::new();
        i.set_location("http://beispiel.test/a/seite.html");
        assert_eq!(ausdruck(&mut i, "typeof XMLHttpRequest"), "function");
        assert_eq!(ausdruck(&mut i, "XMLHttpRequest.DONE + ',' + XMLHttpRequest.OPENED"), "4,1");
        assert_eq!(ausdruck(&mut i, "\
            globalThis.x = new XMLHttpRequest(); \
            globalThis.log = []; \
            x.onreadystatechange = function(){ log.push(x.readyState) }; \
            x.onload = function(){ log.push('load:' + x.status) }; \
            String(x.readyState)"), "0");
        // `open` relativ — die Adresse muss gegen das Dokument aufgeloest werden.
        assert_eq!(ausdruck(&mut i, "x.open('GET','../b/d.json'); String(x.readyState)"), "1");
        let _ = ausdruck(&mut i, "x.setRequestHeader('X-A','1'); x.send()");
        let offen = i.take_pending_fetches();
        assert_eq!(offen.len(), 1, "die Anfrage muss beim Wirt liegen");
        assert_eq!(&offen[0].url, "http://beispiel.test/b/d.json", "gegen das Dokument aufgeloest");
        assert!(offen[0].headers.contains("X-A"), "gesetzte Kopfzeile faehrt mit");
        // Jetzt antwortet der Wirt.
        super::fetch_done(&mut i, offen[0].id, 200, &offen[0].url,
                          "content-type: application/json\r\n",
                          alloc::string::String::from("{\"a\":1}"));
        assert_eq!(ausdruck(&mut i, "log.join(',')"), "1,2,3,4,load:200");
        assert_eq!(ausdruck(&mut i, "x.responseText"), "{\"a\":1}");
        assert_eq!(ausdruck(&mut i, "x.getResponseHeader('CONTENT-TYPE')"), "application/json");
        assert_eq!(ausdruck(&mut i, "x.responseType='json'; String(x.response.a)"), "1");
    }

    /// Fremde Herkunft ist ein FEHLEREREIGNIS, kein Wurf — so wie im Browser
    /// ohne CORS. Und sie darf gar nicht erst beim Wirt landen.
    #[test]
    fn xhr_fremde_herkunft_meldet_fehler_und_faehrt_nicht() {
        let mut i = Interp::new();
        i.set_location("http://beispiel.test/");
        let r = ausdruck(&mut i, "\
            var y = new XMLHttpRequest(); var s = 'nichts'; \
            y.onerror = function(){ s = 'error:' + y.status + ':' + y.readyState }; \
            y.onload = function(){ s = 'FALSCH erfuellt' }; \
            y.open('GET','https://fremd.test/x'); y.send(); s");
        assert_eq!(r, "error:0:4");
        assert!(i.take_pending_fetches().is_empty(), "nichts darf beim Wirt liegen");
    }

    /// Synchron kann die Engine nicht — sie gibt die Kontrolle an den Wirt
    /// zurueck, und der holt. Das gehoert gesagt, nicht still umgangen.
    #[test]
    fn xhr_synchron_sagt_dass_es_nicht_geht() {
        let mut i = Interp::new();
        i.set_location("http://beispiel.test/");
        let r = ausdruck(&mut i, "new XMLHttpRequest().open('GET','/a',false)");
        assert!(r.starts_with("THROW XMLHttpRequest: synchronous"), "war: {r}");
    }
}
