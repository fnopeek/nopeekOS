# `tools/wasm/beak-engine/src/js/url.rs` @ 5e0102684

## L1-13 · `use alloc::string::{String, ToString};`

```
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
```

## L22 · `#[derive(Clone, Default)]`

```
/// Die zerlegte Adresse. Alles Text — eine URL IST Text mit Grenzen darin.
```

## L55-56 · `pub fn parse_abs(input: &str) -> Option<Parts> {`

```
/// Eine absolute Adresse zerlegen. `None`, wenn kein Schema davorsteht —
/// dann ist sie relativ und braucht eine Grundlage.
```

## L71-79 · `let hostport = auth.rsplit('@').next().unwrap_or(auth);`

```
// Anmeldedaten in der Adresse werden verworfen, nicht als Host
// gelesen — `http://user@host/` hat den Host HINTER dem `@`.
//
// **Und sie kommen auch nicht zurueck.** Ein `URL`-Gegenstand haelt
// nur seinen `href`, und was nicht in `href()` steht, ueberlebt keinen
// Zugriff. Das ist hier die richtige Richtung: `location.href =
// "http://google.com@boese.example/"` wuerde in der Adresszeile
// aussehen wie Google. Was `URL.username` dazu sagt, steht bei den
// Zugriffsfunktionen.
```

## L89-90 · `if (p.scheme == "https" && p.port == "443") || (p.scheme == "http" && p.port == "80") {`

```
// Der Vorgabeport steht nicht im `href` — `https://x:443/` und
// `https://x/` sind dieselbe Adresse.
```

## L114 · `pub fn resolve(input: &str, base: &Parts) -> Parts {`

```
/// Eine relative Adresse gegen eine Grundlage aufloesen.
```

## L122 · `let mut s = String::from(&p.scheme);`

```
// Schemarelativ: Host neu, Schema von der Grundlage.
```

## L131 · `let dir = match base.path.rfind('/') { Some(i) => &base.path[..=i], None => "/" };`

```
// Wirklich relativ: ab dem letzten `/` der Grundlage.
```

## L139-140 · `fn norm(path: &str) -> String {`

```
/// `.` und `..` aufloesen. Ohne das ist `new URL("../x", base)` eine Adresse,
/// die es nicht gibt.
```

## L160 · `fn pct_decode(s: &str) -> String {`

```
// ── Prozentkodierung ────────────────────────────────────────────────────
```

## L203 · `pub fn parse_query(q: &str) -> Vec<(String, String)> {`

```
/// `a=1&b=2` in Paare. Ein Feld ohne `=` hat den leeren Wert.
```

## L222 · `const U_HREF: &str = "\0!url";`

```
// ── Die Anbindung an die Maschine ───────────────────────────────────────
```

## L224-226 · `const U_HREF: &str = "\0!url";`

```
/// Die zerlegte Adresse liegt als Text auf dem Objekt, nicht als Rust-Wert:
/// eine Seite darf `u.hash = "#x"` schreiben, und dann muss `u.href` sich
/// mitaendern. Ein eingefrorener Rust-Wert koennte das nicht.
```

## L228-229 · `const U_OWNER: &str = "\0!url.owner";`

```
/// Rueckverweis eines `URLSearchParams` auf sein `URL` — `p.set(…)` muss die
/// Adresse aendern, nicht nur die Kopie.
```

## L265 · `None => return i.type_err(&alloc::format!("invalid URL: {raw}")),`

```
// Kein Schema und keine Grundlage: das ist keine Adresse.
```

## L318-330 · `for k in ["username", "password"] {`

```
// `username`/`password` — 94 Aufrufe im Zensus, und was sie fragen, ist
// „steht da etwas?".
//
// **Sie sind IMMER leer, und das ist keine Luecke, sondern die Wahrheit
// ueber beaks Adressen:** `parse_abs` verwirft Anmeldedaten, weil
// `http://google.com@boese.example/` in einer Adresszeile aussieht wie
// Google. Eine Adresse in beak hat keine, also melden sie keine.
//
// Das Zuweisen wird ANGENOMMEN und tut nichts. Die Spezifikation kennt
// genau das (URL §6.2: „cannot have a username/password/port" — dort fuer
// `file:`); hier gilt es fuer jedes Schema. Zu werfen waere schlechter:
// eine Seite, die einen Benutzernamen setzt und ihn nie wieder liest,
// stuerbe an einer Zeile, die nichts bedeutet.
```

## L345-348 · `let g = new_obj(Some(i.realm.url_params_proto.clone()));`

```
// Das Objekt haelt seinen Eigentuemer, damit `set`/`append` in die
// Adresse zurueckschreiben. Ohne den Rueckverweis waere
// `u.searchParams.set(…)` eine stille Nulloperation — der haeufigste
// Weg, `URLSearchParams` falsch zu bauen.
```

## L366 · `let sp_ctor = native(Some(fp.clone()), |i, _, a| {`

```
// ── URLSearchParams ──────────────────────────────────────────────────
```

## L404-405 · `let mut pairs = sp_read(i, &t)?;`

```
// `set` ersetzt das ERSTE Vorkommen und wirft alle weiteren weg —
// `append` ist das, was mehrfach anhaengt.
```

## L468-469 · `fn sp_read(i: &mut Interp, t: &Value) -> C<Vec<(String, String)>> {`

```
/// Die Paare lesen — entweder aus der eigenen Zeichenkette oder, wenn das
/// Objekt zu einem `URL` gehoert, aus DESSEN Suchteil.
```

