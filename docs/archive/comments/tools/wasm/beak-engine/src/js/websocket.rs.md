# `tools/wasm/beak-engine/src/js/websocket.rs` @ 5e0102684

## L1-14 · `use alloc::string::String;`

```
//! `WebSocket` (RFC 6455) — Handschlag, Rahmen und die JS-Flaeche.
//!
//! **Die Engine oeffnet keine Verbindung.** Wie bei `fetch` legt sie einen
//! Auftrag hin (`pending_sockets`), der Wirt fuehrt ihn aus und reicht die
//! Bytes zurueck — hier ueber `npk_tls_*`. Alles in dieser Datei rechnet auf
//! Byte-Puffern und laesst sich damit host-seitig pruefen.
//!
//! **Gleiche Herkunft, und das ist keine Bequemlichkeit** (S3 aus
//! `docs/plan/WEB_PLATFORM_GAPS.md`): einen WebSocket schuetzt KEINE
//! CORS-Antwortpruefung. Im Web entscheidet allein der Server im Handschlag,
//! ob er eine fremde Herkunft annimmt — wer ihm den `Origin` schickt und die
//! Antwort trotzdem durchlaesst, baut ein Loch. Also bis auf Weiteres: nur
//! dieselbe Herkunft, und der `Origin` faehrt mit, damit ein Server, der
//! spaeter zustimmen darf, es auch kann.
```

## L22 · `const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";`

```
/// RFC 6455 §4.2.2 — die feste Zeichenkette, die der Server anhaengt.
```

## L25-26 · `const MAX_MESSAGE: usize = 8 * 1024 * 1024;`

```
/// Deckel je Nachricht. Ohne ihn haelt eine schwatzhafte Gegenstelle den
/// Halde des Moduls, und ein Browser hat dafuer keinen zweiten Speicher.
```

## L32 · `#[derive(Debug, PartialEq)]`

```
/// Was aus dem Strom herauskam, fuer den Rufer.
```

## L38-40 · `Closed(u16, String),`

```
/// Code und Grund. `1006` heisst „ohne Close-Rahmen abgerissen" — den
/// darf NIE jemand senden, er ist die Auskunft der Gegenseite ueber ein
/// Ende, das keiner angesagt hat (§7.1.5).
```

## L45 · `pub struct Socket {`

```
/// Ein Steckplatz: der halbe Zustand einer Verbindung, ohne den Wirt.
```

## L54 · `key: String,`

```
/// Der geschickte Schluessel, base64. Gegen ihn wird die Antwort geprueft.
```

## L56 · `out: Vec<u8>,`

```
/// Noch nicht abgeholte Bytes fuer die Leitung.
```

## L58 · `inbox: Vec<u8>,`

```
/// Angekommene Bytes, noch nicht zerlegt.
```

## L60 · `handshake_done: bool,`

```
/// Der Kopfblock der Antwort, solange er noch nicht vollstaendig ist.
```

## L62 · `frag: Vec<u8>,`

```
/// Halbfertige Nachricht aus Fortsetzungsrahmen (§5.4).
```

## L65 · `close_sent: bool,`

```
/// Haben WIR den Close-Rahmen geschickt?
```

## L70-73 · `pub fn new(id: u32, url: &str, page_origin: &str, page_secure: bool,`

```
/// **`wss://` oder `ws://` zerlegen.** Ein fehlender Port ist 443 bzw. 80,
/// wie bei HTTP — und `ws://` ist nur erlaubt, wenn die Seite selbst
/// unverschluesselt kam: ein `ws://` aus einer `https`-Seite ist
/// gemischter Inhalt, und den laesst kein Browser durch.
```

## L112-114 · `pub fn handshake(&self) -> Vec<u8> {`

```
/// Der Aufrueststoss (§4.1). Der `Host`-Kopf traegt den Port mit, wenn er
/// nicht der vorgegebene ist — sonst weist ein Server mit mehreren Namen
/// die Verbindung ab.
```

## L129 · `fn expected_accept(&self) -> String {`

```
/// Was der Server antworten MUSS (§4.1, Punkt 4).
```

## L136 · `pub fn feed(&mut self, bytes: &[u8], rnd: &mut dyn FnMut() -> [u8; 4]) -> Vec<Event> {`

```
/// Bytes von der Leitung hereingeben. Gibt zurueck, was daraus wurde.
```

## L143 · `Ok(false) => return out,          // Kopf noch nicht vollstaendig`

```
// Kopf noch nicht vollstaendig
```

## L171 · `fn try_handshake(&mut self) -> Result<bool, String> {`

```
/// Den Antwortkopf lesen. `Ok(false)` = noch nicht ganz da.
```

## L174 · `if self.inbox.len() > 16 * 1024 {`

```
// Ein Kopf, der nicht enden will, ist kein Kopf.
```

## L184-185 · `if !status.contains(" 101") {`

```
// „HTTP/1.1 101 …" — alles andere ist eine Absage, und der Grund
// gehoert in die Meldung: eine 403 sagt etwas anderes als eine 404.
```

## L203-206 · `if accept != self.expected_accept() {`

```
// **Die Pruefung ist der ganze Sinn des Schluessels.** Sie sagt nicht,
// dass die Gegenstelle vertrauenswuerdig ist — das sagt TLS. Sie sagt,
// dass wirklich ein WebSocket-Server geantwortet hat und nicht ein
// Zwischenspeicher, der eine alte Antwort wiederholt (§1.3).
```

## L214 · `#[allow(clippy::type_complexity)]`

```
/// Einen Rahmen abheben (§5.2). `Ok(None)` = noch nicht vollstaendig.
```

## L223-225 · `if masked { return Err(String::from("WebSocket: the server masked a frame")) }`

```
// **Ein Server maskiert NIE** (§5.1). Tut er es doch, ist die
// Verbindung nach der Spezifikation zu beenden — und nicht etwa
// freundlich zu entmaskieren.
```

## L254 · `0x0 => {`

```
// Fortsetzung
```

## L271 · `0x1 | 0x2 => {`

```
// Text / binaer
```

## L278 · `0x8 => {`

```
// Close (§5.5.1)
```

## L286 · `if !self.close_sent {`

```
// Die Antwort ist derselbe Rahmen zurueck — einmal.
```

## L295 · `0x9 => { self.push_frame(0xA, &payload, rnd); None }`

```
// Ping -> Pong mit DERSELBEN Nutzlast (§5.5.2)
```

## L297 · `0xA => None,`

```
// Pong: nichts zu tun, aber kein Fehler.
```

## L306-308 · `fn push_frame(&mut self, op: u8, payload: &[u8], rnd: &mut dyn FnMut() -> [u8; 4]) {`

```
/// **Jeder Rahmen des Clients wird maskiert** (§5.3) — mit vier Bytes aus
/// dem echten Zufall des Wirts. Das schuetzt nicht den Inhalt (TLS tut
/// das), sondern Zwischenstellen davor, den Strom als HTTP zu lesen.
```

## L328 · `pub fn send_text(&mut self, s: &str, rnd: &mut dyn FnMut() -> [u8; 4]) -> bool {`

```
/// `ws.send(text)` — `false`, wenn die Verbindung nicht offen ist.
```

## L341-343 · `pub fn close(&mut self, code: u16, reason: &str, rnd: &mut dyn FnMut() -> [u8; 4]) {`

```
/// `ws.close(code, reason)`. Der Close-Rahmen geht raus, die Verbindung
/// bleibt bis zur Antwort der Gegenseite in `Closing` — erst dann ist sie
/// sauber zu (§7.1.2).
```

## L354-355 · `pub fn take_out(&mut self) -> Vec<u8> {`

```
/// Was auf die Leitung soll. Der Rufer nimmt es MIT — zweimal senden
/// waere ein zweiter Rahmen.
```

## L362 · `pub fn hung_up(&mut self) -> Option<Event> {`

```
/// Die Gegenstelle ist weg, ohne Close-Rahmen. `1006` ist genau dafuer da.
```

## L370-372 · `pub struct PendingSocket {`

```
// ── Die Auftraege an den Wirt ────────────────────────────────────────────
//
// Dasselbe Muster wie `fetch`: die Engine legt hin, der Wirt fuehrt aus.
```

## L374 · `pub struct PendingSocket {`

```
/// Eine Verbindung, die der Wirt noch aufbauen soll.
```

## L379 · `pub hello: Vec<u8>,`

```
/// Der Aufrueststoss, fertig — der Wirt schickt ihn, sobald TLS steht.
```

## L381-383 · `pub secure: bool,`

```
/// `false` bei `ws://`: dann ohne TLS. Heute lehnt der Wirt das ab, weil
/// es ihn nur fuer eine unverschluesselte Seite gaebe; die Zeile steht
/// hier, damit der Auftrag vollstaendig ist und nicht der Wirt raet.
```

## L390-391 · `const W_ID: &str = "\0!ws.id";`

```
/// Verborgene Felder am JS-Gegenstand, wie bei XHR (`\0!` — kein Name, den
/// ein Skript schreiben kann).
```

## L404-406 · `fn mask_source() -> impl FnMut() -> [u8; 4] {`

```
/// Vier Bytes aus der Quelle des Wirts. **Ohne echten Zufall keine Maske** —
/// und ohne Maske kein Rahmen: RFC 6455 §5.3 laesst dem Client keine Wahl,
/// und eine vorhersagbare Maske waere schlechter als keine Verbindung.
```

## L411-414 · `b = [0; 4];`

```
// Die Maske ist kein Geheimnis, sie ist ein Streuwert gegen
// Zwischenspeicher, die den Strom als HTTP lesen. Ohne Quelle
// bleibt sie null — und der Aufruf schlaegt eine Zeile hoeher
// ohnehin fehl, weil `WebSocket` dann gar nicht erscheint.
```

## L421-429 · `fn run_handler(i: &mut Interp, f: &Value, obj: &Value, ev: &Value, kind: &str) {`

```
/// Ein Ereignis an den JS-Gegenstand zustellen — `onX` und `addEventListener`.
/// Einen Behandler der Seite rufen — und einen Fehler daraus MELDEN.
///
/// **Stille war hier der eigentliche Fehler.** Die Antwort auf ein
/// CDP-Kommando kam an, `onmessage` warf unterwegs, und die Seite sah nichts
/// als einen Timeout ohne Grund — die Zustellung hatte funktioniert, die
/// Auskunft darueber fehlte. Ein Browser schreibt so etwas in die Konsole,
/// also steht es jetzt auch hier. Und die Zustellung laeuft weiter: jeder
/// Behandler steht fuer sich, einer, der wirft, nimmt den naechsten nicht mit.
```

## L469 · `pub fn host_bytes(i: &mut Interp, id: u32, bytes: Option<&[u8]>) {`

```
/// Der Wirt reicht an, was auf der Leitung ankam. `None` heisst „abgerissen".
```

## L492-494 · `let len = d.len();`

```
// **Ohne `ArrayBuffer` waere es eine halbe Schnittstelle.**
// Seitencode prueft `typeof e.data`, und eine Zeichenkette
// dort ist eine falsche Antwort, keine fehlende.
```

## L512-513 · `e.borrow_mut().define("wasClean", Prop::builtin(Value::Bool(code != 1006)));`

```
// `wasClean` ist falsch bei 1006 — genau dafuer ist der
// Code da.
```

## L523-524 · `if let Some(idx) = find_sock(i, id) {`

```
// Fertig heisst weg — sonst haelt die Tabelle jede je geoeffnete
// Verbindung bis zur Navigation fest.
```

## L533 · `pub fn take_out_for(i: &mut Interp, id: u32) -> Option<Vec<u8>> {`

```
/// Was fuer diese Verbindung auf die Leitung soll.
```

## L540-546 · `pub fn install(i: &mut Interp) {`

```
/// `WebSocket` im globalen Objekt.
///
/// **Es erscheint nur, wenn es echten Zufall gibt** — dieselbe Regel wie bei
/// `crypto`. Ohne ihn gibt es keine Maske, und RFC 6455 §5.3 laesst dem
/// Client keine Wahl: eine vorhersagbare Maske waere schlechter als eine
/// fehlende Schnittstelle, denn eine Seite prueft `if (window.WebSocket)` und
/// richtet sich danach.
```

## L563-565 · `let bytes = bytes_of(i, &arg);`

```
// Ein `ArrayBuffer` oder eine Sicht darauf geht als BINAERER Rahmen
// raus, alles andere als Text — so steht es im Vertrag, und
// Seitencode verlaesst sich darauf.
```

## L606 · `for (n, v) in [("CONNECTING", 0.0), ("OPEN", 1.0), ("CLOSING", 2.0), ("CLOSED", 3.0)] {`

```
// Die vier Konstanten des Vertrags, am Prototyp UND am Konstruktor.
```

## L626-627 · `Err(e) => return Err(i.throw_kind("SyntaxError", &e)),`

```
// `SyntaxError` ist hier die richtige Art der SPRACHE — die
// Spezifikation nennt fuer eine kaputte Adresse genau sie.
```

## L630-637 · `let same = page.as_ref().is_some_and(|p| p.host.eq_ignore_ascii_case(&sock.host));`

```
// **S3: gleiche Herkunft, und der Grund steht im Kopf dieser Datei.**
// Ein WebSocket hat keine Antwortpruefung, die ihn schuetzt — wer
// eine fremde Herkunft durchlaesst, verlaesst sich darauf, dass der
// Server Nein sagt. Der `Origin` faehrt trotzdem mit, damit ein
// Server, der spaeter zustimmen darf, es auch kann.
// Verglichen wird der WIRT, nicht das Schema: `wss://` gehoert zu
// `https://` wie `ws://` zu `http://`, und die Schemata stehen
// deshalb nie beide gleich da.
```

## L640-643 · `let e = i.throw_kind("Error", &alloc::format!(`

```
// **Der NAME zaehlt.** Seitencode prueft `e.name === 'SecurityError'`
// (so steht es in der Spezifikation), nicht den Text. `throw_kind`
// kennt nur die Fehlerarten der Sprache, also wird der Name hier
// gesetzt — dieselbe Stelle wie bei `crypto.getRandomValues`.
```

## L714 · `assert!(Socket::new(1, "ws://a.test/", "https://e.test", true, [0; 16]).is_err());`

```
// **Ein `ws://` aus einer sicheren Seite ist gemischter Inhalt.**
```

## L727-728 · `assert!(req.contains("\r\nOrigin: https://example.test\r\n"));`

```
// **Der `Origin` faehrt mit** — S3: ein Server, der eine fremde
// Herkunft annehmen darf, kann es nur, wenn er sie sieht.
```

## L731 · `let bad = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\`

```
// Eine Antwort mit FALSCHEM Accept wird abgelehnt.
```

## L746 · `fn motor(js: &str) -> Interp {`

```
/// Einen Motor mit `WebSocket` und einer Seite als Herkunft.
```

## L757-758 · `fn server_spricht(i: &mut Interp, text: &str) {`

```
/// Den Handschlag beantworten und einen Textrahmen nachschieben — so,
/// wie der echte Server es tut.
```

## L780-785 · `#[test]`

```
/// **Die halbe Strecke war ungeprueft.** Alles darueber misst die
/// LEITUNG — Rahmen hinein, Rahmen hinaus. Was eine SEITE davon sieht,
/// stand in keinem Test: dass `onopen` faellt, dass `e.data` eine
/// Zeichenkette ist und `JSON.parse` sie frisst. Genau diese Strecke
/// liegt zwischen „der Server hat geantwortet" und „die Seite hat es
/// gemerkt", und genau dort lief ein CDP-Kommando in einen Timeout.
```

## L802-805 · `#[test]`

```
/// **Ein Behandler, der wirft, darf nicht still sein.** Vorher verschluckte
/// `fire` den Wurf: die Zustellung hatte funktioniert, die Seite sah nur
/// einen Timeout ohne Grund. Und der zweite Behandler muss trotzdem laufen
/// — im Browser steht jeder fuer sich.
```

## L824 · `let frame = [0x81u8, 0x02, b'h', b'i'];`

```
// Ein unmaskierter Textrahmen „hi", in DREI Haeppchen.
```

## L836 · `let mut b = vec![0x01u8, 0x02, b'a', b'b'];   // Text, FIN aus`

```
// Text, FIN aus
```

## L837 · `b.extend_from_slice(&[0x00, 0x01, b'c']);      // Fortsetzung`

```
// Fortsetzung
```

## L838 · `b.extend_from_slice(&[0x80, 0x01, b'd']);      // Fortsetzung, FIN an`

```
// Fortsetzung, FIN an
```

## L867 · `let mittel = alloc::vec![b'x'; 200];`

```
// 126 Bytes -> die 16-Bit-Form.
```

## L874 · `let gross = alloc::vec![b'y'; 70_000];`

```
// Und ueber 65535 die 64-Bit-Form.
```

## L886 · `let ev = s.feed(&[0x81, 0x82, 1, 2, 3, 4, b'h', b'i'], &mut feste_maske());`

```
// §5.1: ein Server maskiert NIE.
```

## L896 · `let ev = s.feed(&[0x88, 0x02, 0x03, 0xE8], &mut feste_maske());  // 1000`

```
// 1000
```

## L899 · `let mut s2 = sock("wss://a.test/ws");`

```
// Ein Abriss OHNE Close-Rahmen ist 1006 — und den sendet nie jemand.
```

## L907 · `fn hidden(v: Value) -> Prop {`

```
/// Eine versteckte Eigenschaft — dasselbe Muster wie in `fetch.rs`.
```

## L913 · `fn bytes_of(i: &mut Interp, v: &Value) -> Option<Vec<u8>> {`

```
/// Die Bytes hinter einem `ArrayBuffer` oder einer Sicht darauf, sonst `None`.
```

