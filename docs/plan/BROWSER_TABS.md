# BROWSER_TABS.md — Tabs, und was der Browser sonst noch braucht

> **Stand 2026-09-10, beak 0.163.0 / Kernel 0.336.0: Schritt 1-3 und 5 sind
> gebaut.** Ein `Vec<Box<Doc>>` mit `ACTIVE`, ein Streifen, Strg+T/W/1-9,
> Mittelklick, `npk_open` → Tab. Gefahren ist Entwurf **(b)**: EIN lebendiger
> Motor, der Rest eingefroren — Schritt 3 ist damit erledigt, nur mit LRU = 1
> statt 2-3. Was offen bleibt, steht in **A9** ganz unten.

> Offenes Papier, 2026-09-10, beak 0.150.0 / Kernel 0.332.0.
> Florians Auftrag: „überleg dir, wie wir tabs implementieren. und welche
> features unser browser sonst noch braucht."
>
> Teil A ist der Entwurf für Tabs. Teil B ist die Rangliste dessen, was
> daneben fehlt — **gemessen, nicht gefühlt**, denn beides konkurriert um
> dieselbe Zeit, und zwei Posten in Teil B treffen den Benutzer härter als
> das Fehlen von Tabs.

---

# Teil A — Tabs

## A0. Die ehrliche Form der Aufgabe

**Tabs sind kein Tabstrip.** Der Streifen ist die letzten fünf Prozent.

beak ist heute ein Programm für **ein** Dokument, und das steht nicht in
einem Kommentar, sondern in der Struktur:

| Gemessen im Baum (2026-09-10) | |
|---|---|
| `static mut` in `tools/wasm/beak/src/lib.rs` | **79** |
| davon: das EINE Dokument beschreibend | die große Mehrheit |
| Motor-Instanzen (`beak_engine::Engine`) | 1 |
| JS-Sitzungen (`static mut JS`) | 1 |
| Verlauf | ein globales Feld `HIST[HIST_MAX]` + `HIST_POS` |
| Adresse | `URL_BUF` (Dokument) + `EDIT_BUF` (Textfeld) |

Ein Tab ist also nicht „noch ein Streifen oben", sondern: **die Seite muss
ein Wert werden.** Solange sie 79 globale Variablen ist, gibt es keinen
zweiten Ort, an dem eine zweite Seite stehen könnte.

Das ist keine schlechte Nachricht. Es ist dieselbe Arbeit, die beak
ohnehin schuldet — siehe A2.

## A1. Was eine Seite wirklich kostet

Ohne diese Zahlen ist jeder Entwurf eine Meinung.

```
srf.ch, gehaltene Halde                      44 MiB   (war 89 vor 0.129.0)
arcade.ch, dekodierte Bilder                131 MB    (gemessen 2026-09-10)
ein JS-Realm                                973 KB    (0.126.0)
DOM + Stylesheet einer echten Seite         „Megabytes" (DOC_SLOTS-Kommentar)
```

Und die Deckel, die den Rahmen setzen:

```
forge MAX_MEMORY_BYTES                        8 GB    — das Modul ist NICHT die Grenze
Kernel MAX_RESERVED_BYTES (alle Abrufe)      64 MB
Kernel MAX_JOBS_PER_OWNER                       4
Kernel WORKER_COUNT                             1     ← siehe A6
beaks statische Puffer (.bss, GETEILT)      ~47 MB    HTML 3 + CSS 8 + Skript 8
                                                      + Bilder 24 + Schriften 4
```

**Die wichtigste Zeile ist die letzte.** Die 47 MB statischer Puffer sind
Abholpuffer — sie gehören dem Abruf, der gerade läuft, nicht der Seite. Sie
vervielfachen sich **nicht** mit den Tabs. Was sich vervielfacht, ist die
gehaltene Halde je Seite: **rund 44 MiB für eine echte Nachrichtenseite,
plus Bilder.**

Zehn offene Tabs auf srf-Niveau sind also ~440 MiB, und ein einziger
arcade.ch-Tab legt 131 MB Bilder obendrauf. Das entscheidet A3.

## A2. Der Schritt, der in JEDEM Entwurf zuerst kommt: `Page` als Wert

```rust
struct Page {
    url: String,            // die Adresse des DOKUMENTS
    edit: String,           // was im Textfeld steht  (seit 0.147.0 getrennt!)
    hist: Vec<String>,
    hist_pos: usize,
    scroll_y: i32,
    engine: Engine,         // oder ein Griff in einen geteilten Motor, siehe A3
    js: Option<js::Session>,
    nav: NavState,          // NAV_JOB, NAV_STAGE, NAV_URL, NAV_PUSH_HIST, …
    content_gen: u64,
    forms: FormState,
}
```

**Das ist die ganze Arbeit, und sie zahlt sich schon bei EINEM Tab aus.**
Heute muss `navigate()` daran denken, ~20 Statics von Hand zurückzusetzen;
vergisst es eine, trägt die neue Seite einen Rest der alten. Genau diese
Klasse war der Fehler von 0.147.0: **`URL_BUF` war die Adresse des Dokuments
UND der Inhalt des Textfelds**, und daraus wurde ein Datenschutzfehler (jedes
Präfix einer Eingabe ging an den DNS) plus ein Loch in der Reichweiten-Grenze.
Ein `Page` mit zwei Feldern hätte das unmöglich gemacht.

Reihenfolge, mechanisch und jederzeit prüfbar:

1. `Page` anlegen, **ein** `static mut PAGE: Page`. Jede Static wandert
   einzeln hinein, der Rest des Codes ändert sich nur an der Zugriffsstelle.
   Nach jedem Schritt: `beak:selftest` 55/55 · 39/39 und WPT unverändert.
2. Erst wenn `PAGE` steht und nichts mehr daneben lebt, wird daraus
   `static mut TABS: Vec<Page>` + `ACTIVE: usize`.
3. Der Tabstrip ist danach ein Nachmittag.

**Prüfbar heißt prüfbar:** ein Zähltest (`grep -c '^static mut'`) und die
Regel „diese Zahl darf nur fallen". Sonst wandert Schritt 1 nie zu Ende
([[feedback_count_it_dont_sample_it]]).

## A3. Drei Entwürfe, und warum nur einer den Kontakt überlebt

### (a) N Motoren, alle lebendig

Jeder Tab hält seinen `Engine`, seine Bilder, seinen JS-Realm.

- **Kosten:** 10 Tabs × 44 MiB = 440 MiB gehalten, plus Bilder. Auf einer
  NUC mit 8–16 GB machbar, aber es ist auch die Fassung, die am
  schnellsten in `memory.grow`-Absagen läuft — und die Kette dahin haben
  wir schon einmal falsch gelesen
  ([[feedback_the_number_in_the_panic_was_the_initial_value]]).
- **Gewinn:** Tabwechsel ist sofort. Hintergrund-Timer laufen.
- **Urteil:** funktioniert für 3–5 Tabs. Für 20 nicht.

### (b) Ein lebendiger Motor + eingefrorene Tabs ← **empfohlen**

Nur der aktive Tab hält DOM, Stylesheet, Bilder und JS-Realm. Ein Tab, der
in den Hintergrund geht, wird auf das eingedampft, was ihn wiederherstellt:

```
Adresse · Verlauf · Rollposition · Titel · Favicon · Formularwerte
```

Das sind **Kilobytes**, nicht Megabytes. Beim Zurückwechseln wird die Seite
neu geholt — und genau dafür gibt es schon zwei Dinge:

- **Der seiten-übergreifende Bildspeicher** (`IMG_CACHE_BUDGET`, seit heute
  256 MB): die Bilder der zuletzt besuchten Seiten liegen noch da, ein
  Rückwechsel kostet keine Anfrage für sie.
- **`DOC_SLOTS = 3`**: der Motor hält die letzten drei geparsten Bäume samt
  Stylesheet. Für „zwei Tabs hin und her" ist das genau die richtige Zahl,
  und sie ist schon gebaut.

- **Kosten:** ein Tabwechsel auf einen alten Tab kostet einen Neuaufbau
  (~0,2–0,8 s auf gemessenen Seiten), nicht 0 ms.
- **Gewinn:** 20 Tabs kosten so viel wie einer. Kein neues Speichermodell.
- **Der Preis, den man benennen muss:** eine Seite mit Zustand im DOM
  (halb ausgefülltes Formular, offenes Menü, Warenkorb per Skript) verliert
  ihn. Formularwerte werden gerettet (`forms.rs` kennt sie bereits, das ist
  die Grundlage von `layout_forms`), Skriptzustand nicht.
  **Ausweg:** die zwei bis drei zuletzt benutzten Tabs bleiben lebendig
  (LRU), der Rest friert ein. Das ist, was jeder mobile Browser tut, und
  es deckt „ich wechsle zwischen zwei Seiten hin und her" vollständig ab.

### (c) Ein Prozess je Tab

Was Chrome tut, und **architektonisch ist es das, was nopeekOS eigentlich
will**: die Sandbox ist ohnehin je Modul, ein bösartiger Tab könnte den
anderen dann nicht einmal theoretisch nahekommen.

- **Was dagegen steht, heute:** der Compositor gibt einer App **ein**
  Widget-Fenster (`WidgetScene` je `WindowId`). Mehrere beak-Instanzen wären
  mehrere Fenster, kein Tabstrip. Dazu bräuchte es einen Weg, Bild und
  Ereignisse eines fremden Moduls in das Fenster eines anderen zu hängen —
  eine echte Erweiterung des Fenstermodells, kein Detail.
- **Urteil:** das richtige Fernziel, kein Nachmittag. **(b) verbaut es
  nicht** — ein eingefrorener Tab ist genau die Beschreibung, die ein
  Prozess später zum Wiederherstellen bekäme.

## A4. Was ein Tab besitzt und was geteilt bleibt

Die Trennlinie ist nicht Geschmack, sie ist Kosten:

| Gehört dem Tab | Bleibt geteilt | Warum geteilt |
|---|---|---|
| Adresse, Textfeld, Verlauf, Rollposition | **Schriften** (`Fonts`) | 6 Gesichter + Webfonts; das Rastern trieb die Halde einmal von 11 auf 89 MiB |
| DOM, Stylesheet, Layout | **Bildspeicher** über Navigationen | genau dafür gebaut |
| Bilder DIESER Seite, CSS-Bilder | **Abholpuffer** (~47 MB `.bss`) | es läuft ohnehin ein Abruf zur Zeit |
| JS-Realm, Zeitgeber, Microtasks | **Keksglas** | ein Glas je Profil, nicht je Tab — sonst ist man in einem Tab angemeldet und im anderen nicht |
| Formularzustand, `content_gen` | **Widget-Baum** | ein Fenster |

**Das Keksglas ist die einzige Zeile, bei der ein Fehler weh tut.** Kekse
sind pro Site, nicht pro Tab — wer sie je Tab hält, baut aus Versehen
Container-Tabs und wundert sich über Abmeldungen.

## A5. Zeitgeber im Hintergrund — der Teil, der beißt

Ein eingefrorener Tab hat keine JS-Sitzung, also auch keine Zeitgeber. Für
die 2–3 LEBENDIGEN Hintergrund-Tabs aus dem LRU stellt sich die Frage
trotzdem, und sie ist keine Kleinigkeit: `[beak] load: Baum geaendert,
7 Zeitgeber` auf arcade.ch, **139 Zeitgeber** im host-seitigen Lauf.

Die Regel, die Browser gefunden haben und die hier genauso gilt:

- **Sichtbarer Tab:** volle Rate, `requestAnimationFrame` läuft.
- **Hintergrund, lebendig:** `setTimeout`/`setInterval` werden auf
  **≥ 1 s** gedrosselt, `requestAnimationFrame` läuft **gar nicht** (es
  gibt kein Bild, für das es sich lohnt).
- **Eingefroren:** nichts.

### Und hier steht ein Kommentar, der mit Tabs abläuft

`js/dombind.rs` beantwortet `document.visibilityState` heute mit einer
Konstanten, und die Begründung daneben ist gut — **solange sie gilt**:

> „beak malt genau ein Dokument, und es ist sichtbar, solange es laeuft —
> das ist keine Hoeflichkeit, sondern der Zustand."

```rust
getter(&document_proto, "visibilityState", |_, _, _| Ok(Value::str("visible")), &fp);
getter(&document_proto, "hidden",          |_, _, _| Ok(Value::Bool(false)),    &fp);
meth  (&document_proto, "hasFocus",        |_, _, _| Ok(Value::Bool(true)), 0,  &fp);
```

**Am Tag, an dem es zwei Tabs gibt, sind alle drei eine Lüge** — und zwar
die schlimmste Sorte: eine Seite im Hintergrund fragt `document.hidden`,
bekommt `false` und pollt fröhlich weiter. Der Zensus zählt 50 Aufrufe auf
`visibilityState`/`hidden`; die Seiten fragen wirklich.

Der Kommentar nennt seine eigene Bedingung, also gehört er auf die Liste
([[feedback_a_comment_that_names_its_condition_expires]] — genau so ist
0.123.0 mit „solange beak keine Skripte hat" passiert). **Drei Zeilen aus
Schritt 4, und ohne sie ist die Drosselung wirkungslos**, denn eine Seite,
der man sagt, sie sei sichtbar, verhält sich wie eine sichtbare.

Dazu fehlt `visibilitychange` als Ereignis. `requestAnimationFrame` gibt es
(`js/builtins.rs`, neben `setTimeout`/`setInterval`/`requestIdleCallback`).

**Und die Falle, die wir schon einmal getreten haben:** das Zeitbudget hing
an `tick`, das nur eingebaute Schleifen rufen
([[feedback_the_clock_only_ticks_where_someone_asks]]). Wer die Drosselung
an einer Stelle einbaut, an der ein Hintergrund-Tab nie vorbeikommt, hat
sie nicht eingebaut.

## A6. Zwei Tabs, die gleichzeitig laden — die Stelle, an der es hakt

```rust
// kernel/src/intent/fetch.rs
const WORKER_COUNT: usize = 1;   // EINS, deliberately
const MAX_JOBS_PER_OWNER: usize = 4;
```

Der Kommentar dort ist ehrlich und sagt genau, was zu tun wäre:

> „Two would let a click start its document while the picture batch it
> replaces is still on the wire — but it would also make PARALLEL use of
> `intent::http` the normal case, and that client has never run that way:
> the connection pools are spin-locked, and `pool_take` closes a stale
> session while holding the lock. Whether that is safe under two callers is
> a question to answer by reading it, not by assuming it."

**Für Tabs heißt das:** zwei ladende Tabs werden serialisiert. Der zweite
wartet einen Rundlauf (100–300 ms) auf den ersten. Das ist erträglich und
sichtbar — aber es ist auch die Zahl, die „Tabs fühlen sich langsam an"
erzeugt, sobald man drei Seiten auf einmal öffnet.

Drei Möglichkeiten, in der Reihenfolge, in der man sie angehen sollte:

1. **Nichts tun und es sagen.** Der zweite Tab zeigt seinen Ladebalken; das
   ist ehrlich. Für v1 richtig.
2. **`WORKER_COUNT` auf 2 heben — NACHDEM die Frage im Kommentar
   beantwortet ist.** Das ist Lesearbeit an `pool_take`, kein Konstantendreh
   ([[feedback_read_the_gate_before_committing]] in Geist: der Kommentar
   nennt die Bedingung, also gilt sie, bis jemand nachsieht).
3. Erst danach: mehr als 4 Aufträge je Besitzer.

## A7. Der Tabstrip selbst

`docs/spec/UI_REFRESH.md` §195 legt ihn schon fest: **36 px,
`SurfaceElevated`, Tabs unten bündig**. Er braucht **kein neues Widget** —
eine `Row` aus `Button`/`Icon` mit `Modifier::NodeId` reicht, und die
Auswahl ist `Modifier::Active`.

Was er an ABI braucht: nichts. Was er an Verhalten braucht:

- `npk_open` liefert schon heute einen Öffnen-Wunsch als **`Event::Open`**
  an die laufende Instanz („Singleton + tabs" steht wörtlich im Kernel).
  **Das ist der fertige Haken für „in neuem Tab öffnen" aus anderen Apps.**
- Mittelklick auf einen Link → neuer Tab im Hintergrund. Der Klickweg
  kennt die Taste bereits (`Event::MouseButton`).
- `Strg+T` / `Strg+W` / `Strg+Tab` → `Event::Chord` gibt es im ABI.

## A8. Staffelung

| Schritt | Inhalt | Beweist | Risiko |
|---|---|---|---|
| 1 | `Page` als Wert, ein Tab | 79 Statics → eine Struktur; Selftest + WPT unverändert | mechanisch, groß |
| 2 | `Vec<Page>` + Tabstrip, alle Tabs lebendig | Umschalten, Öffnen, Schließen | Speicher |
| 3 | LRU: 3 lebendig, Rest eingefroren | 20 Tabs kosten wie 3 | Zustandsverlust benennen |
| 4 | `visibilityState`/`hidden`/`hasFocus` ehrlich machen + `visibilitychange` + Zeitgeber-Drosselung | Hintergrund frisst keine CPU | die `tick`-Falle; drei Getter, die heute lügen dürfen |
| 5 | Mittelklick, `Strg+T/W/Tab`, `npk_open` | fertig anfühlen | — |
| später | `WORKER_COUNT` 2, nach dem Lesen von `pool_take` | zwei Tabs laden wirklich parallel | Nebenläufigkeit |

**Schritt 1 ist mehr als die Hälfte der Arbeit und liefert für sich genommen
noch keinen einzigen Tab.** Das muss man vorher wissen und aushalten.

## A9. Was 0.163.0 wirklich gebaut hat — und was nicht

Nachgetragen am Bau, nicht vorher geplant. Drei Dinge kamen anders.

**Der Zaehler ist angekommen.** `static mut` in `tools/wasm/beak/src/lib.rs`:
77 (0.151.0) → 46 → 23. Die 23 sind keine Reste, sondern vier Gruppen mit
einem Grund, der eine zweite Seite ueberlebt: 13 Abholpuffer, 3 fuer den
BILDPUFFER (`LAST_W/H/SY` — der Puffer ist einer), 3 fuer den Skriptdeckel
(es laeuft ein Stueck Seitencode), 3 fuer Fenster und Werkzeug. Plus `TABS`.

**Schritt 2 („alle Tabs lebendig") ist uebersprungen worden, und das war
richtig.** Der Entwurf sah ihn als Zwischenstufe vor; am Baum ist er gar nicht
baubar: der Motor haelt EINEN Baum, EIN Stilblatt und die Bilder EINER Seite,
und `HTML_BUF`/`CSS_BUF` sind je ein Puffer. „Alle lebendig" heisst also N
Motoren — mehr Arbeit als (b), nicht weniger. Gegangen wurde direkt (b).

**Schritt 4 ist keiner mehr.** `visibilityState`/`hidden`/`hasFocus` sagen
weiter „sichtbar", und das ist **wahr geblieben, nur aus einem anderen
Grund**: ein Hintergrundtab ist eingefroren, hat also gar keine JS-Sitzung —
wer fragt, ist der sichtbare Tab. Die Begruendung im Kommentar von
`js/dombind.rs` ist entsprechend ausgetauscht, samt dem Tag, an dem sie
ablaeuft. Damit entfaellt auch die Zeitgeber-Drosselung aus A5: ein
eingefrorener Tab hat keine Zeitgeber.

**Was der Kernel dazu brauchte** (`kernel+module beak:`): der Compositor hat
Links und Rechts an Apps zugestellt, **die mittlere Taste nie** — obwohl
beide Zeigerwege sie liefern (`b0 & 0x07` bei PS/2, dasselbe Bit im
HID-Boot-Protokoll) und `MouseButton::Middle` seit je im ABI steht. Ohne sie
gaebe es „Link in neuem Tab oeffnen" ueberhaupt nicht. Bewusst OHNE
Treffertest: ein Mittelklick ist keine zweite Art, einen Knopf zu druecken,
und ihn auf `hit_test` zu legen liesse jeden Knopf im System darauf
reagieren.

**Nebenbefund beim Umbau, und er kostete Speicher:** `ptr::write`
ueberschreibt OHNE den alten Wert fallen zu lassen. `GEOM` wurde so
geschrieben — also blieb bei JEDER Neuauslegung der vorige `Vec<ElemRect>`
liegen (gemessen: 2123 Kaesten a 32 B = 66 KB auf der Wikipedia-Hauptseite),
und `hit_all` steht genau auf den Seiten MIT Skripten an, also auf denen, die
am oeftesten neu auslegen. Dasselbe in `subresources_cancel`. Als
Feldzuweisung faellt der alte Wert, wie er soll.

### Offen, in der Reihenfolge, in der es weh tut

1. **Der Streifen rollt nicht.** Zehn Tabs a 160 px sind 1600 px; der elfte
   schoebe das `+` aus dem Fenster, also ist bei zehn Schluss. Was das hebt,
   ist ein `Widget::Scroll` mit `Axis::Horizontal` — der einzige Kasten, den
   der Compositor wirklich abschneidet. Damit faellt zugleich die zweite
   Kruecke: das Etikett wird heute auf 17 Zeichen gekuerzt, gerechnet mit
   7 px je Zeichen, weil eine App die Schrift des Compositors nicht messen
   kann und ueberstehender Text in den NACHBARN gemalt wuerde.
2. **Skriptzustand ueberlebt den Wechsel nicht** — der benannte Preis von
   (b). Formularwerte auch nicht: `forms.rs` kennt sie, aber `Page` ist eine
   Schleifenvariable und wird beim Wechsel neu gebaut. Sie zu retten ist ein
   Feld in `Doc` und die kleinste sichtbare Verbesserung, die hier steht.
3. **Kein „Tab wiederherstellen"** (Strg+Shift+T) und keine Reihenfolge per
   Ziehen.
4. **LRU 2-3 lebendige Tabs** braucht mehrere Motoren — §A3, unveraendert.
5. `WORKER_COUNT` 2 — §A6, unveraendert. Heute serialisiert sich das von
   selbst: ein Hintergrundtab holt NICHTS, er wird nur angelegt.

---

# Teil B — Was der Browser SONST braucht

Geordnet nach *gemessener* Härte für den Benutzer, nicht nach Aufwand. Alle
Aussagen sind am Baum geprüft, nicht erinnert.

## B1. Kekse auf Unterressourcen — **die schlimmste**

```rust
// kernel/src/intent/fetch.rs
Work::One  { method, host, path, headers, body, cap, tls, from_reach }  // ← headers
Work::Many { urls, cap, from_reach }                                    // ← KEINE
```

Der Stapel-Weg trägt **keine Kopfzeilen**. Bilder, Stylesheets und Skripte
gehen also **unangemeldet** raus, während das Dokument angemeldet geholt
wurde. Auf jeder Seite hinter einer Anmeldung heißt das: Text da, Bilder
kaputt — und es sieht aus wie ein Bildfehler.

Steht seit 2026-07 in `BROWSER.md` §5 als offen. Es ist der Posten, der
„beak kann keine angemeldeten Seiten" bedeutet, und er ist nicht groß:
`Work::Many` um `headers` erweitern und den Aufrufer die Kekse je Adresse
setzen lassen.

## B2. Kekse überleben den Neustart nicht

Das Glas ist reiner Speicher (`cookies.rs::global()`, keine npkFS-Anbindung).
Jeder Neustart meldet den Benutzer überall ab. Zusammen mit B1 ist das
„beak ist ein Lesegerät, kein Browser".

## B3. Nicht-ASCII-Tastatur — **für einen Schweizer Benutzer hart**

```rust
KeyCode::Char(b) if (0x20..0x7F).contains(&b) => …
```

Wörtlich ASCII. **`ä`, `ö`, `ü`, `é`, `à` lassen sich in kein Formular
tippen** — nicht in ein Suchfeld, nicht in ein Anmeldefeld. Das trifft bei
jeder deutschsprachigen Suche.

## B4. Text auf der Seite lässt sich nicht markieren

`grep` über `tools/wasm/beak/src/lib.rs`: **0 Treffer** für Seitenauswahl.
Die Adresszeile kann es seit heute — die Seite nicht. Markieren und
Kopieren ist nach „Links anklicken" die meistbenutzte Handlung in einem
Browser überhaupt.

Vorarbeit ist da: das Layout kennt seine Textläufe und ihre Kästen
(`hit_all`, `hitchk`), der Trefferweg steht.

## B5. Was es gar nicht gibt (je 0 Treffer im Baum)

| | Bemerkung |
|---|---|
| **Suchen in der Seite** (`Strg+F`) | braucht dieselbe Textlauf-Kenntnis wie B4 |
| **Herunterladen / Speichern unter** | eine Datei aus dem Netz auf npkFS |
| **Zoom** | `Event::Zoom` gibt es im ABI, beak nutzt es nicht |
| **Lesezeichen / Startseite** | |
| **`<video>` / `<audio>`** | ehrliche Grenze; keine Dekoder, kein Ziel für v1 |
| **Drucken / PDF** | |

## B6. Aus der Messung, nicht aus dem Gefühl

- **Raster** — `347 von 1105` WPT-Fehlern sind `display: grid-lanes`. Der
  mit Abstand größte einzelne CSS-Block, und Wikipedias `main.mw-body`
  rechnet deshalb falsch.
- **`overflow` wird nicht abgeklemmt** — `scrollTop` ist an gewöhnlichen
  Elementen wahrheitsgemäß 0, aber das heißt auch: **Unterbereiche einer
  Seite scrollen nicht.** Jedes Seitenmenü mit eigenem Scrollbereich ist
  abgeschnitten.
- **`position: sticky` versetzt nicht** — jede klebende Kopfzeile steht
  falsch.
- **Bidi** — `CSS2/bidi`, 22 Tests, kein Algorithmus. Arabisch und Hebräisch
  sind unbenutzbar, nicht nur unschön.
- **Steuerelement-GEOMETRIE** — seit 0.150.0 stimmen die Farben, die Maße
  nicht: Kästchen 14 px statt 13, Knöpfe zu breit, keine 2-px-Rundung.

## B7. Die Rangliste, wenn man sie zu einer machen muss

1. **B1 Kekse auf Unterressourcen** — ohne das gibt es keine angemeldeten Seiten.
2. **B3 Umlaute** — ohne das kann man auf Deutsch nicht suchen.
3. **B2 Kekse dauerhaft** — ohne das ist jede Anmeldung eine Sitzung lang.
4. **B4/B5 Markieren + Suchen in der Seite** — die meistbenutzte fehlende Handlung.
5. **Teil A, Tabs** — ab hier fühlt es sich wie ein Browser an.
6. **B6 Raster + overflow + sticky** — die Seiten sehen dann auch richtig aus.

**Tabs stehen bewusst auf Platz 5.** Sie sind das, was ein Browser
*ausmacht* — aber ein Browser, in dem man sich nicht anmelden und keinen
Umlaut tippen kann, wird von einem zweiten Tab nicht besser. Und Schritt A2
(`Page` als Wert) ist ohnehin so groß, dass er nicht zwischen zwei
kleinere Sachen passt.
