# `tools/wasm/beak-engine/src/js/fetch.rs` @ 5e0102684

## L1-44 · `use alloc::rc::Rc;`

```
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
```

## L54 · `pub enum Waiter {`

```
/// Wer auf eine Antwort wartet.
```

## L56 · `Promise(Gc),`

```
/// `fetch()` — das Versprechen wird erfuellt oder abgelehnt.
```

## L58-59 · `Xhr(Gc),`

```
/// `XMLHttpRequest` — das Objekt selbst nimmt die Antwort auf und ruft
/// seine Behandler.
```

## L63 · `pub struct PendingFetch {`

```
/// Eine Anfrage, solange der Wirt sie holt.
```

## L68 · `pub headers: String,`

```
/// Roher Kopfblock, Zeilen mit `\r\n` getrennt.
```

## L73-74 · `const R_STATUS: &str = "\0!res.status";`

```
// ── Verborgene Felder ───────────────────────────────────────────────────
// Dasselbe `\0!`-Muster wie in `url.rs`: kein Name, den JS schreiben kann.
```

## L97-99 · `fn list_of(i: &mut Interp, t: &Value, k: &str) -> Vec<Value> {`

```
/// Ein Feld, das ein JS-Array haelt, als Rust-Liste. Ein Array legt seine
/// Elemente als Eigenschaften ab, nicht in einem Rust-Vec — gelesen wird es
/// deshalb ueber den gewoehnlichen Weg.
```

## L119 · `fn raw_get(raw: &str, name: &str) -> Option<String> {`

```
// ── Kopfblock: Text rein, Text raus ─────────────────────────────────────
```

## L121-123 · `fn raw_get(raw: &str, name: &str) -> Option<String> {`

```
/// Einen Namen im rohen Block suchen. Kopfnamen sind ohne Ruecksicht auf
/// Gross- und Kleinschreibung gleich — das ist keine Bequemlichkeit, es steht
/// so in RFC 9110, und Server liefern `Content-Type` wie `content-type`.
```

## L131 · `Some(hits.join(", "))`

```
// Mehrfach gesetzte Koepfe kommen als EINE Zeile mit `, ` zurueck.
```

## L166 · `fn abort_error(i: &mut Interp) -> Value {`

```
// ── AbortSignal ─────────────────────────────────────────────────────────
```

## L168-172 · `fn abort_error(i: &mut Interp) -> Value {`

```
/// Der Grund, mit dem ein Abbruch ohne eigenen Grund ablehnt.
///
/// Ein `DOMException` gibt es in dieser Engine nicht; gebaut wird deshalb ein
/// `Error` mit dem NAMEN, auf den Seitencode prueft. Ueber `throw_kind`, damit
/// die Fehlerobjekte hier nicht ein zweites Mal entstehen.
```

## L198-199 · `fn do_abort(i: &mut Interp, sig: &Value, reason: Value) -> C<()> {`

```
/// Ein Signal auf „abgebrochen" setzen und alles benachrichtigen, was daran
/// haengt: die angemeldeten Behandler, `onabort`, und die laufende Anfrage.
```

## L206 · `if let Value::Num(id) = slot(i, sig, S_FETCH) {`

```
// Die laufende Anfrage wirklich abbrechen — nicht nur die Fahne setzen.
```

## L229 · `fn init_headers(i: &mut Interp, init: &Value) -> C<String> {`

```
// ── fetch ───────────────────────────────────────────────────────────────
```

## L231-234 · `fn init_headers(i: &mut Interp, init: &Value) -> C<String> {`

```
/// Die Kopfzeilen aus dem `headers`-Feld der Init lesen.
///
/// Zwei Formen kommen vor: ein gewoehnliches Objekt und ein `Headers`. Beide
/// enden im selben rohen Block.
```

## L252-255 · `fn do_fetch(i: &mut Interp, t: Value, a: &[Value]) -> C<Value> {`

```
/// **`fetch` LEHNT AB, es wirft nicht.** Ein Netz- oder Herkunftsfehler
/// gehoert ins `catch` des Rufers; wer hier wirft, beendet stattdessen das
/// rufende Skript — und alles danach laeuft nicht mehr. Die eigene Probe ist
/// genau darueber gestolpert, zum zweiten Mal in dieser Datei.
```

## L271-272 · `if o.borrow().get_own("url").is_some() && o.borrow().get_own("method").is_some() {`

```
// Ein `Request` gibt es nicht. Das zu sagen ist besser, als seine
// Felder zu erraten und die Anfrage still falsch zu stellen.
```

## L304 · `if matches!(signal, Value::Obj(_)) && signal_aborted(i, &signal) {`

```
// Schon abgebrochen, bevor es losging: dann geht gar nichts los.
```

## L321 · `fn document_origin(i: &mut Interp) -> Option<String> {`

```
/// Die Herkunft des Dokuments, als Text.
```

## L330-336 · `fn same_origin_url(i: &mut Interp, raw: &str) -> C<Option<String>> {`

```
/// Eine Adresse aufloesen und auf gleiche Herkunft pruefen. `None` heisst
/// fremd — was der Rufer daraus macht, ist bei `fetch` eine Ablehnung und
/// bei `XMLHttpRequest` ein Fehlerereignis.
///
/// **Hier wird aufgeloest, und nur hier.** Zwei Aufloeser waeren zwei
/// Meinungen darueber, was `../` bedeutet, und die eine davon entschiede dann
/// ueber die Herkunft ([[feedback_the_probe_must_use_the_targets_resolver]]).
```

## L352 · `fn take_waiting(i: &mut Interp, id: u32) -> Option<Waiter> {`

```
// ── Was der Wirt zurueckmeldet ──────────────────────────────────────────
```

## L359 · `pub fn fetch_done(i: &mut Interp, id: u32, status: u16, final_url: &str,`

```
/// Eine Antwort ist da. `raw_headers` ist der Kopfblock ohne die Statuszeile.
```

## L381-383 · `pub fn fetch_failed(i: &mut Interp, id: u32, why: &str) {`

```
/// Die Anfrage ist gescheitert. **Ein `fetch` lehnt mit `TypeError` ab** —
/// nicht mit dem Status. Ein 404 ist eine ANTWORT und wird erfuellt; nur ein
/// Netzfehler ist eine Ablehnung, und Seitencode unterscheidet danach.
```

## L398 · `pub fn install(realm: &mut Realm) {`

```
// ── Einbau ──────────────────────────────────────────────────────────────
```

## L404 · `let h_proto = new_obj(Some(op.clone()));`

```
// ── Headers ─────────────────────────────────────────────────────────
```

## L424-425 · `Ok(raw_get(&raw, &n).map(Value::string).unwrap_or(Value::Null))`

```
// `null`, nicht `undefined` — daran unterscheidet Seitencode
// „nicht gesetzt" von „leer gesetzt".
```

## L470 · `let r_proto = new_obj(Some(op.clone()));`

```
// ── Response ────────────────────────────────────────────────────────
```

## L518-521 · `meth(&r_proto, "text", |i, t, _| {`

```
// **Beide LEHNEN AB, sie werfen nicht.** Ein zweites `text()` auf
// derselben Antwort ist ein Fehler — aber ein Fehler im Versprechen. Wer
// hier wirft, beendet das rufende Skript, statt in dessen `catch` zu
// landen; die eigene Probe ist genau darueber gestolpert.
```

## L533-534 · `let r = body_once(i, &t).and_then(|v| super::json::parse_value(i, &v));`

```
// **Derselbe Leser wie `JSON.parse`.** Ein eigener waere eine zweite
// Semantik, und die laeuft still auseinander.
```

## L553 · `let s_proto = new_obj(Some(op.clone()));`

```
// ── AbortSignal ─────────────────────────────────────────────────────
```

## L558 · `i.type_err("Illegal constructor")`

```
// Wie im Browser: ein Signal entsteht am Controller, nicht mit `new`.
```

## L577-580 · `meth(&s_proto, "addEventListener", |i, t, a| {`

```
// Ein `AbortSignal` ist kein Knoten, also kann es die Anmeldung des
// Dokuments nicht mitbenutzen — `addEventListener` dort verlangt eine
// Knoten-id. Es gibt hier genau EINE Art Ereignis, und die Liste dafuer
// haengt am Signal selbst.
```

## L600 · `let c_proto = new_obj(Some(op.clone()));`

```
// ── AbortController ─────────────────────────────────────────────────
```

## L621 · `let f = native(Some(fp.clone()), do_fetch, "fetch", 1, false);`

```
// ── fetch ───────────────────────────────────────────────────────────
```

## L627-641 · `if let Some(Value::Obj(nav)) = realm.global.borrow().get_own("navigator")`

```
// ── navigator.sendBeacon ────────────────────────────────────────────
//
// **Fire and forget: die Antwort interessiert niemanden.** Deshalb steht
// kein Warter in `fetch_waiting` — `fetch_done` findet keinen und legt
// die Antwort weg. Genau das ist die Semantik.
//
// Gefunden an Googles Startseite: sie meldet die gemessene Fenstergroesse
// mit `navigator.sendBeacon("/client_204?…&biw=…&bih=…")`, und der Aufruf
// steht in einem `try{}catch{}`. Ohne die Funktion scheitert er STILL —
// im Log stand nichts, und die Seite verhielt sich, als haette sie nie
// gemessen.
//
// Gleiche Herkunft, wie bei `fetch` und `XMLHttpRequest`. Ein Beacon an
// eine fremde Herkunft ist der Ausleitungskanal in Reinform; `false`
// sagt dem Rufer, dass nichts abging.
```

## L653 · `let method = if body.is_some() { "POST" } else { "GET" };`

```
// POST, wenn etwas mitfaehrt — sonst GET, so wie ein Zaehlpixel.
```

## L663-665 · `fn body_once(i: &mut Interp, t: &Value) -> C<Value> {`

```
/// Den Rumpf EINMAL hergeben. `bodyUsed` ist kein Schmuck: eine Antwort
/// zweimal zu lesen ist ein Fehler, und Seitencode baut darauf, dass er ihn
/// bekommt statt einer leeren Zeichenkette.
```

## L674-676 · `fn status_text(s: u16) -> &'static str {`

```
/// Die Statuszeilen, die vorkommen. Kein vollstaendiger Katalog — was fehlt,
/// bekommt eine leere Zeichenkette, und das ist auch, was ein Browser fuer
/// einen unbekannten Code liefert.
```

## L692-717 · `const X_METHOD: &str = "\0!xhr.method";`

```
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
```

## L734 · `fn xhr_fire(i: &mut Interp, x: &Value, kind: &str) -> C<()> {`

```
/// Einen Behandler rufen: `onX` und die ueber `addEventListener` angemeldeten.
```

## L760-761 · `let _ = xhr_state(i, &xv, 2.0);`

```
// 2, 3, 4 der Reihe nach — Seitencode prueft beides, `readyState` UND
// die Ereignisse, und manche warten auf die Zwischenstufen.
```

## L771-772 · `let _ = i.set(&xv, X_STATUS, Value::Num(0.0), false);`

```
// **Status 0, nicht ein erfundener Fehlercode.** So unterscheidet
// Seitencode einen Netzfehler von einer Antwort mit 500.
```

## L857-858 · `if let Value::Obj(x) = &t { xhr_failed(i, x); }`

```
// Fremde Herkunft: kein Wurf, ein FEHLEREREIGNIS — so wie im
// Browser ohne CORS. Siehe Kopf dieser Datei.
```

## L941-947 · `#[test]`

```
/// **Der ganze Weg eines `XMLHttpRequest`, ohne Wirt.**
///
/// Google entscheidet mit `typeof XMLHttpRequest != "undefined"`, ob es
/// eine Seite mit JavaScript ausliefert. Ein Stummel haette dafuer
/// gereicht — und die Seite haette ihn dann BENUTZT. Der Test faehrt
/// deshalb die Anfrage bis zur Antwort durch: anlegen, oeffnen,
/// abschicken, den Wirt antworten lassen, Zustaende und Text pruefen.
```

## L960 · `assert_eq!(ausdruck(&mut i, "x.open('GET','../b/d.json'); String(x.readyState)"), "1");`

```
// `open` relativ — die Adresse muss gegen das Dokument aufgeloest werden.
```

## L967 · `super::fetch_done(&mut i, offen[0].id, 200, &offen[0].url,`

```
// Jetzt antwortet der Wirt.
```

## L977-978 · `#[test]`

```
/// Fremde Herkunft ist ein FEHLEREREIGNIS, kein Wurf — so wie im Browser
/// ohne CORS. Und sie darf gar nicht erst beim Wirt landen.
```

## L992-997 · `#[test]`

```
/// **`sendBeacon` ist fire-and-forget** — es meldet nur, ob etwas abging.
///
/// Googles Startseite meldet damit die gemessene Fenstergroesse, und der
/// Aufruf steht in einem `try{}catch{}`: fehlt die Funktion, scheitert er
/// STILL. Fremde Herkunft gibt `false` statt eines Wurfs — ein Beacon
/// dorthin waere der Ausleitungskanal in Reinform.
```

## L1008 · `assert_eq!(ausdruck(&mut i, "String(navigator.sendBeacon('/p', 'daten'))"), "true");`

```
// Mit Rumpf ein POST.
```

## L1013 · `assert_eq!(ausdruck(&mut i, "String(navigator.sendBeacon('https://fremd.test/x'))"), "false");`

```
// Fremd: false, und nichts geht ab.
```

## L1018-1019 · `#[test]`

```
/// Synchron kann die Engine nicht — sie gibt die Kontrolle an den Wirt
/// zurueck, und der holt. Das gehoert gesagt, nicht still umgangen.
```

