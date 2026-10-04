# `tools/wasm/beak-engine/src/site.rs` @ 5e0102684

## L1-26 · `extern crate alloc;`

```
//! Herkunft und Site — die zwei Begriffe, auf denen jede Grenze im Web steht.
//!
//! `cookies.rs` sagte bis 0.101.0 im Kopf: `SameSite` sei nicht gebaut, weil
//! es „a notion of the initiating context" brauche, „which arrives with
//! scripting". Das Skripting ist da. Hier ist der Begriff.
//!
//! **Herkunft** (*origin*) = Schema + Host + Port. Zwei Dokumente derselben
//! Herkunft dürfen einander lesen.
//!
//! **Site** = Schema + *registrierbare Domain*. Gröber als die Herkunft:
//! `app.example.com` und `api.example.com` sind verschiedene Herkünfte, aber
//! dieselbe Site — und genau darauf beruht, dass eine Anwendung mit ihrer
//! eigenen API sprechen kann, ohne dass ein Keks zu Fremden fliesst.
//!
//! ## Warum die echte Liste eingebettet ist
//!
//! Die registrierbare Domain ist NICHT „die letzten zwei Bestandteile". Bei
//! `a.github.io` und `b.github.io` wären das beide `github.io` — zwei
//! fremde Nutzerseiten würden als dieselbe Site gelten und einander Kekse
//! schicken. Am Zielkorpus gemessen (691 Hosts) trifft das 7 Hosts, alle in
//! die gefährliche Richtung.
//!
//! Deshalb liegt die echte Public Suffix List daneben (10 321 Regeln,
//! 144 KB). Gegen 3,86 MB `beak.wasm` ist das nichts, und eine
//! Sicherheitsgrenze approximiert man nicht, wenn die genaue Antwort so
//! wenig kostet. Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §5.3.
```

## L32-50 · `const PSL: &str = include_str!("public_suffix_list.dat");`

```
/// Die Liste, sortiert und ohne Kommentare — sortiert, damit die Suche eine
/// Binärsuche ist und kein Durchlauf über 10 780 Zeilen je Keks.
///
/// **So entsteht sie neu** (die Liste ändert sich, also gehört das
/// aufgeschrieben statt erraten):
///
/// 1. `curl -O https://publicsuffix.org/list/public_suffix_list.dat`
/// 2. Kommentare (`//`) und Leerzeilen weg.
/// 3. **Jede Nicht-ASCII-Regel bekommt ihre Punycode-Form DAZU** — die
///    Originalliste führt `公司.cn` nur in Unicode, ein Host aus einer URL
///    ist aber Punycode. Ohne diesen Schritt greift keine einzige
///    IDN-Endung, und zwei fremde Seiten darunter gelten als dieselbe Site.
///    Das offizielle Testorakel hat genau das gefunden; meine eigenen
///    Proben nicht.
/// 4. Sortieren und entdoppeln (`sorted(set(...))`).
///
/// Die Vektoren in `psl_vectors.txt` kommen aus `tests/test_psl.txt`
/// desselben Projekts, ASCII-normalisiert — **auskommentierte Zeilen sind
/// keine Vektoren**, auch das war ein Fehler des ersten Entwurfs.
```

## L53-54 · `#[derive(Debug, Clone, PartialEq, Eq)]`

```
/// Eine Herkunft: Schema, Host, Port. Der Port ist ausgerechnet, nicht
/// geraten — `https://a.de` und `https://a.de:443` sind dieselbe Herkunft.
```

## L63-65 · `pub fn header(&self) -> String {`

```
/// Die Textform, wie sie in einen `Origin:`-Kopf gehört. Der
/// Vorgabeport steht NICHT drin — sonst passt sie nicht auf das, was
/// ein Server in `Access-Control-Allow-Origin` zurückschreibt.
```

## L76-82 · `pub fn origin_of(url: &str) -> Option<Origin> {`

```
/// Die Herkunft einer Adresse. `None`, wenn die Adresse keine hat —
/// `about:`, `data:`, `beak:selftest` und alles andere ohne Autorität.
///
/// **Ein Dokument ohne Herkunft bekommt keine Kekse und darf nichts holen.**
/// Das ist kein Sonderfall, den man wegdrückt: `beak:selftest` hat wirklich
/// keine, und die Trennung wurde am Gerät schon einmal sichtbar
/// (`cookies: Seite setzte 2, 0 held`).
```

## L98 · `let hostport = match hostport.rfind('@') {`

```
// Ein Nutzer-Teil (`user@host`) gehört nicht zur Herkunft.
```

## L103 · `let (host, port_s) = if let Some(end) = hostport.strip_prefix('[').and_then(|r| r.find(']')) {`

```
// IPv6 steht in eckigen Klammern und enthält selbst Doppelpunkte.
```

## L124-126 · `host: host.to_ascii_lowercase(),`

```
// Der Host ist ohne Rücksicht auf Gross/Klein zu vergleichen; der
// Pfad NICHT. Genau hier wird es entschieden, damit es nirgends
// sonst nochmal getan werden muss.
```

## L132-139 · `pub fn registrable_domain(host: &str) -> Option<String> {`

```
/// Die **registrierbare Domain** eines Hosts — ein Bestandteil mehr als das
/// öffentliche Suffix (ES: „eTLD+1").
///
/// `www.bbc.co.uk` -> `bbc.co.uk` · `a.github.io` -> `a.github.io` ·
/// `example.com` -> `example.com` · `co.uk` -> `None` (ein Suffix allein ist
/// keine registrierbare Domain, und ein Keks darauf gehört niemandem).
///
/// Eine IP-Adresse hat keine: sie IST schon die kleinste Einheit.
```

## L146-148 · `if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {`

```
// Ein LEERER Bestandteil heisst: das ist kein Host. `.example.com` sieht
// wie einer aus und ist keiner — die offiziellen Vektoren prüfen genau
// das, und der erste Entwurf hat brav `example.com` daraus gemacht.
```

## L152-154 · `let mut best: Option<usize> = None; // Anzahl Bestandteile des Suffix`

```
// Die längste passende Regel gewinnt (PSL-Algorithmus). Gesucht wird von
// der längsten Kandidatenform abwärts, damit die erste Übereinstimmung
// schon die längste ist.
```

## L155 · `let mut best: Option<usize> = None; // Anzahl Bestandteile des Suffix`

```
// Anzahl Bestandteile des Suffix
```

## L160-161 · `if psl_has(&alloc::format!("!{cand}")) {`

```
// Ausnahmeregel (`!city.kawasaki.jp`) sticht alles: das Suffix ist
// dann EIN Bestandteil kürzer als die Regel.
```

## L171 · `if start + 1 <= labels.len() {`

```
// Platzhalterregel (`*.ck`): trifft, wenn der REST danach passt.
```

## L180-181 · `let suffix_labels = best.unwrap_or(1);`

```
// Keine Regel getroffen: die Vorgabe der PSL ist „`*`", also ist das
// letzte Bestandteil das Suffix. Das ist der Normalfall (`example.com`).
```

## L185-187 · `return None;`

```
// Der Host IST ein öffentliches Suffix (`co.uk`, `github.io`).
// Darauf gibt es keine registrierbare Domain — und damit auch
// keinen Keks, der irgendwem gehört.
```

## L193-198 · `pub fn same_site(a: &str, b: &str) -> bool {`

```
/// Gehören zwei Hosts zur selben Site?
///
/// **Das ist die Frage hinter `SameSite`**, und sie ist gröber als die
/// Herkunft: `app.x.de` und `api.x.de` — verschiedene Herkünfte, dieselbe
/// Site. Zwei Hosts ohne registrierbare Domain (IP-Adressen) sind dieselbe
/// Site, wenn sie derselbe Host sind, sonst nicht.
```

## L202-203 · `_ => a.eq_ignore_ascii_case(b),`

```
// Kein Suffix-Wissen anwendbar (IP, `localhost`): dann zählt der
// Host selbst. Erben tut hier niemand etwas.
```

## L208-210 · `pub fn same_site_url(a: &str, b: &str) -> bool {`

```
/// Dieselbe Site UND dasselbe Schema — das ist, was `SameSite` wirklich
/// meint (*schemeful same-site*). `http://x.de` und `https://x.de` gelten
/// als verschieden, sonst hebelt ein Klartext-Zwischenstück die Regel aus.
```

## L225-237 · `fn psl_has(rule: &str) -> bool {`

```
/// Binärsuche in der sortierten Liste. Die Liste ist EIN Block mit
/// Zeilenumbrüchen; ein `Vec` daraus zu bauen hiesse, 10 321 Scheiben beim
/// Start anzulegen, und gesucht wird selten genug, dass die Suche über die
/// Zeilen billiger ist als das Aufbauen.
///
/// **Gesucht wird auf BYTES, nicht auf `str`.** Die Liste enthält die
/// Unicode-Schreibweise internationalisierter Endungen; eine Halbierung
/// landet dort mitten in einem Zeichen, und `&PSL[..mid]` bricht ab. Der
/// erste Entwurf tat genau das und ist im Test gestorben — auf einer
/// fremden Seite wäre es ein Absturz aus dem Nichts gewesen.
/// Byte-Vergleich ist hier ausserdem das Richtige und nicht bloss das
/// Sichere: die Liste ist byteweise sortiert, und ein Host aus einer URL
/// ist ASCII (Punycode).
```

## L244 · `let start = hay[..mid].iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);`

```
// Auf den Anfang der Zeile zurückgehen, in der `mid` liegt.
```

## L251-253 · `if end + 1 <= lo {`

```
// Die Zeile bei `mid` ist zu klein — hinter ihr weitersuchen.
// Ohne den Fortschritt-Zwang stünde die Schleife still, wenn
// `mid` immer wieder in dieselbe Zeile fällt.
```

## L284-290 · `#[test]`

```
/// **Die offiziellen Vektoren der Public Suffix List selbst.**
///
/// Meine eigenen Proben zu bestehen heisst wenig — ich habe sie
/// geschrieben. Das hier ist ein Orakel, das jemand anders aufgestellt
/// hat: `tests/test_psl.txt` aus dem PSL-Projekt, auf die ASCII-Fälle
/// reduziert (ein Host aus einer URL ist Punycode).
/// [[feedback-cross-check-the-probe-against-a-real-engine]]
```

## L311 · `assert_eq!(registrable_domain("com"), None);`

```
// Die Beispiele stehen so auf publicsuffix.org.
```

## L320-321 · `#[test]`

```
/// Genau die sieben Hosts, an denen „die letzten zwei Bestandteile"
/// falsch gewesen wäre — gemessen am Zielkorpus, nicht ausgedacht.
```

## L331-332 · `assert!(!same_site("tc39.github.io", "bakkot.github.io"));`

```
// Und der Punkt der ganzen Übung: zwei fremde Nutzerseiten unter
// derselben Endung sind NICHT dieselbe Site.
```

## L337-339 · `#[test]`

```
/// Die IDN-Endungen müssen in PUNYCODE dastehen, nicht nur in Unicode —
/// sonst greift keine von ihnen, und `a.公司.cn` wäre dieselbe Site wie
/// `b.公司.cn`. Der Test bewacht Schritt 3 der Neuerzeugung.
```

## L356 · `assert!(same_site("www.bbc.co.uk", "news.bbc.co.uk"));`

```
// Länderendungen mit zwei Teilen
```

## L373 · `assert_eq!(origin_of("https://x.de:443/").unwrap().header(), "https://x.de");`

```
// Der Vorgabeport steht nicht im Kopf, ein anderer schon.
```

## L377 · `assert_eq!(origin_of("https://a.de/x"), origin_of("https://a.de:443/y"));`

```
// Gleiche Herkunft heisst: alle drei Teile gleich.
```

