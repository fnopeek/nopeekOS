# Neu auslegen auf Verlangen — und was es kosten darf

Stand 2026-09-17 · beak 0.184.0 · Kernel 0.340.0. Alle Zahlen hier sind
gemessen; wo etwas geschlossen und nicht gemessen ist, steht es dabei.

## Der Befund

DuckDuckGos Ergebnisseite zu „stansstad" zeigt die Karte im Wissenskasten
nicht. Die Karte ist **kein Canvas** — sie ist ein statisches PNG
(`//external-content.duckduckgo.com/ssv2/?…`, 640x157, 105 KB), und Chromium
baut auf DEMSELBEN eingefrorenen Bestand genau dieses `<img>`.

Was sie aufhält, ist eine einzige Lesung. DDGs Modul misst den Kasten, den es
eben eingehängt hat:

```js
useEffect(() => { if (!imageUrl) return void setL(ref.current.offsetWidth); … },
          [imageUrl, o, location]);
…
l && location ? <StaticMap/> : null
```

Ein Getter auf `Element.prototype.offsetWidth` mit einer Logzeile je Lesung
sagt den ganzen Fall in einer Zeile:

```
OW BUTTON.CoNKxMvh5bQ0fGtpNvIx  -> 129  connected=true
OW DIV.wYu4suoXF9TNnHFAfIQY     ->   0  connected=true   ← der Kartenkasten
```

**beaks Geometrie ist das letzte BILD, keine Frage ans Layout.** Ein Element,
das ein Skript im selben Schritt eingehängt hat, steht nicht darin und meldet
0. Ein Bild später ist derselbe Kasten **652 px** breit — es fragt nur niemand
mehr, denn die Abhängigkeiten des Effekts ändern sich nie wieder. **Eine
einmalige Messung macht aus einem Bild Verspätung einen dauerhaften Fehler.**

Gegenprobe: denselben Getter 0 durch eine Zahl ersetzen lassen, und beak baut
byte-gleich dieselbe Karte wie Chromium, samt `size=600x157` in der Adresse.

## Was wir heute haben

Nichts davon. Es gibt **genau einen** Layout-Ort — `do_layout()` in
`tools/wasm/beak/src/lib.rs:1050`, gerufen aus der Bildschleife. Die Maschine
bekommt das Ergebnis nur gereicht (`Interp::set_geometry`, ein Aufruf, im
Malpfad). Von einem Getter zurück ins Layout führt kein Weg; jede Kastenfrage
antwortet aus `Interp::geometry`.

## Wie gross die Fläche ist — gezählt, nicht geschätzt

Zensus über alle Kastenzugriffe (`offsetWidth/Height`, `clientWidth/Height`,
`scrollWidth/Height`, `getBoundingClientRect`). Gezählt wird jede Lesung, die
**0 auf einem eingehängten Element** zurückgibt, und danach wird geprüft, ob
dasselbe Element ein Bild später einen Kasten hat.

| Seite | Lesungen | 0 trotz eingehängt | Elemente | später nicht null |
|---|---:|---:|---:|---:|
| DDG `?q=stansstad` | 219 | 4 | 3 | **3** |
| sandbox.nopeek.ch | 77 | 3 | 2 | **2** |
| DDG-Startseite | 0 | 0 | 0 | — |

Die drei auf DDG: `DIV.jnfopIud3XnBlXjNFBlD`, **`DIV.wYu4suoXF9TNnHFAfIQY`**
(der Kartenkasten), `SPAN.expandableItem`. Alle drei heilen ein Bild später —
es sind also samt und sonders falsche Antworten, keine echten Nullen.

**Beim LADEN ist das wenig. Beim BEDIENEN ist Geometrie die zweithäufigste
API.** Aus unserem eigenen Zensus über zwölf Korpusseiten
(`<tools>/jsscope/out/apirank.json`):

```
Laden     270 662 Aufrufe — davon Geometrie  1 037  =  0,4 %
Bedienen   30 465 Aufrufe — davon Geometrie  1 447  =  4,7 %   ← Faktor 12
```

`Element.getBoundingClientRect` steht beim Bedienen auf **Platz 2** nach
Seitenverbreitung (6 von 12 Seiten, 815 Aufrufe). Das sind Aufklappmenüs,
Tooltips, Sticky-Köpfe, Ziehen — und die tun alle dasselbe: einhängen, dann
messen. `docs/spec/BROWSER.md` sagt es seit dem Entwurf.

**Grenze der Zahl, ausdrücklich:** der Zensus sieht nur die **Nullen**. Eine
veraltete, aber nicht-null Antwort (Element hat sich seit dem letzten Bild
bewegt) ist damit nicht messbar. Die Tabelle ist eine Untergrenze.

## Der Preis eines erzwungenen Layouts — gemessen

Host-seitig, DDG-Ergebnisseite @1902 px, 1015 Kästen, je erzwungenem
Neuauslegen (`pagerun … GEOMEVERY=1 PHASEDBG=1`):

| Schritt | ms |
|---|---:|
| `to_dom` (Baum zurückschreiben) | 0,8 |
| Blätter sammeln (1,06 MB CSS) | 2,8 |
| `layout_ext` gesamt | **76,0** |
| — davon `dom::parse` | **0,0** (übersprungen, Skriptbaum ist gesetzt) |
| — davon Kaskade (`collect_all`) | **15,8** |
| — davon Box-Layout | **51,1** |
| — Rest (inline-SVG, CSS-Bilder, `element_rects`) | ~8 |

Gegenprobe über den ganzen Lauf: 3,09 s mit einem Layout gegen 5,11 s mit 27
→ **78 ms je Layout**, dieselbe Zahl.

Chromium braucht für dasselbe 1–3 ms. **Ein Layout je Lesung ist damit
ausgeschlossen** — 219 Lesungen wären 17 Sekunden. Das Gerät ist langsamer als
der Host; ein Faktor wird hier bewusst NICHT gebildet, weil er nur aus
derselben Seite auf beiden Seiten kommen dürfte
([[feedback_host_profile_is_not_the_device]]). Der einzige saubere
Geräteanker, den wir haben, ist eine andere Seite: Notebook, Wikipedia
Stansstad @1902 px, box-only 150 ms.

## Stand: S1–S3 sind gebaut (beak 0.185.0, 2026-09-17)

Der Entwurf unten steht so, wie er gebaut wurde. Was daraus geworden ist:

* **Schritt 1 der Reihenfolge, inkrementelle Kaskade — erledigt als
  INHALTS-Schluessel.** `scripted_gen` ist aus dem Schluessel des
  Blatt-Zwischenspeichers heraus; an seiner Stelle steht der Fingerabdruck der
  `<style>`-Bloecke, und die `url()` der `style`-Attribute werden bei einem
  Treffer nachgetragen. A/B auf derselben Engine: **Kaskade 19,8 → 0,1 ms.**
  Ein voller `collect_all` steht damit nur noch an, wenn die Seite wirklich ein
  Stilblatt aendert. Der Teilbaum-Neuaufloeser aus dem Text unten war dafuer
  gar nicht noetig — er bleibt als Posten fuer `style::resolve`, das IM
  Box-Layout sitzt.
* **S1/S2/S3 — gebaut.** `Interp::relayout` (Haken wie `clock`), `ensure_box`
  vor allen elf Kastenzugaengen, `FORCED_LAYOUT_CAP = 4` je Bild mit
  Konsolenzeile. Der Wirt haengt `host_relayout` ein; die `Engine` ist dafuer
  aus `main` in eine Globale gewandert und merkt sich den `FormState` des
  letzten Auslegens.
* **Gemessen danach:** ein erzwungenes Layout **78 → 55 ms**, DDGs
  Seitenaufbau **+270 ms**, die Karte steht mit derselben Adresse, die
  Chromium baut. Tore unveraendert (468 Tests, Selbsttest 64/64 + 45/45, WPT
  4556 +9/−0, vier Galerien und zwoelf Render-Hashes Zahl fuer Zahl gleich).
  Neue Selbsttestzeile **`fresh`**; ohne den Haken sagt sie
  `NEIN: frisch eingehaengt meldet 0/0 statt 240`.
* **Offen:** Punkt 4 der Reihenfolge — das Box-Layout selbst, 51 ms fuer 1015
  Kaesten. Solange es das kostet, ist der Deckel von 4 kein Luxus.
* **Noch nicht am GERAET gelaufen.** Alles oben ist host-seitig.

## Der Entwurf

### S1 — Wächter, und die enge Fassung zuerst

Neu ausgelegt wird nur, wenn **beides** gilt:

1. Der Baum ist seit dem letzten Layout schmutzig (`Doc::dirty` gibt es
   schon: `touch()` setzt es, `to_dom()` löscht es), **und**
2. das gefragte Element hat **gar keinen Kasten** — `layout_seq` liefert
   nichts, es ist seit dem letzten Bild entstanden.

Punkt 2 ist die enge Fassung, und sie deckt **100 % der oben gezählten
Fälle** bei minimalem Preis: Leseschleifen über bestehende Elemente kosten
nichts, „hat sich um drei Pixel verschoben" kostet nichts. Sie ist bewusst
schmaler als der Browser — dort ist jede Lesung nach einer Änderung exakt.
Der Unterschied ist benannt, nicht versteckt.

Die weite Fassung (jede Lesung auf schmutzigem Baum) ist S1b und wird erst
gebaut, wenn eine gemessene Seite sie braucht.

### S2 — Der Rückweg in das Layout

`Interp` braucht einen Weg zum Layouter. Das Muster steht schon da:

```rust
pub clock: Option<fn() -> f64>,        // interp.rs:627, existiert
pub relayout: Option<fn(&mut Interp)>, // neu, dieselbe Bauart
```

Der Haken hängt an der `Engine`, und die ist heute eine **lokale Variable** in
der Bildschleife (`beak/src/lib.rs:5775`). Sie muss in ein Static, wie die 23
anderen Zustände, die dem Dokument gehören. `do_layout` nimmt ohnehin nur
`&Engine`, der Borrow tut also nicht weh; `to_dom()` braucht `&mut Doc`, und
das liegt im `Interp`, den der Haken übergeben bekommt.

`pagerun` bekommt denselben Haken, sonst misst die Probe wieder sich selbst.

### S3 — Deckel gegen Layout-Thrashing

Eine Seite, die in einer Schleife schreibt und liest, erzwingt sonst 78 ms je
Durchlauf. Also ein Budget je Tick (Vorschlag: 4 erzwungene Layouts), danach
die alte Antwort — **und eine Zeile auf der Konsole, wenn der Deckel greift.**
Ein Deckel, der stillschweigend eine falsche Zahl liefert, ist schlimmer als
keiner ([[feedback_a_cap_set_from_a_guess_is_below_the_normal_case]]).

### Was NICHT gebaut wird

Kein Notnagel für DDG, keine Sonderbehandlung für `offsetWidth`. Die Regel
gilt für alle acht Kastenzugriffe und `getBoundingClientRect`/`getClientRects`
gleich, oder sie gilt nicht ([[feedback_no_local_workarounds]]).

---

## Damit es bezahlbar wird: wo die Zeit im Layout steht

Der Entwurf oben ist nur so gut wie die 78 ms. Zwei Posten machen sie aus.

### 1. Die Kaskade wird bei JEDER Änderung ganz weggeworfen (15,8 ms)

`raster.rs:1031` mischt `self.scripted_gen` in den Schlüssel des
Stilblatt-Zwischenspeichers. Eine einzige DOM-Änderung — ein `classList.toggle`
— macht damit die **ganze** Kaskade ungültig, und `collect_all` läuft über
1,06 MB CSS neu. Das ist genau die Eigenschaft, die ein erzwungenes
Neuauslegen teuer macht.

Der richtige Schnitt ist bekannt und in der Fuel-Karte belegt:
`style::resolve(apply)` 22,5 % und `sheet.matched(+sort)` 8,2 % des
Box-Layouts sind Arbeit **je Element**. Eine Kaskade, die nur den schmutzigen
Teilbaum neu auflöst, statt den Zwischenspeicher zu leeren, nimmt den grössten
Teil der 15,8 ms weg. Das ist ein eigener Posten und der erste, den ich
angehen würde — er zahlt auch ohne Relayout-auf-Verlangen, bei jedem
`classList.toggle` der Welt.

Nebenbefund derselben Messung: der Dokument-Zwischenspeicher ist
**inhaltsgeschlüsselt** — er greift nur, wenn die Seite byte-gleich
zurückkommt. Eine lebende Seite zahlt beim zweiten Laden den vollen Preis.

### 2. Das Box-Layout, 51 ms für 1015 Kästen = 50 µs je Kasten

Die Fuel-Karte (Wikipedia Stansstad @1902, 411 M Fuel je warmem Layout):

| Anteil | je Aufruf | Funktion |
|---:|---:|---|
| 22,5 % | 31 969 | `style::resolve(apply)` |
| 15,1 % | 16 164 | `Inline::flow(linebreak)` |
| 10,1 % | 797 | `pseudo_content` (53 874 Aufrufe) |
| 9,4 % | 6 599 | `flow_children` |
| 8,2 % | 11 704 | `sheet.matched(+sort)` |
| 7,5 % | 4 954 | `collect_inline` |

`pseudo_content` + `styled(key+lookup)` sind zusammen 13,5 % und fast reines
Schlüssel-Hashen für Zwischenspeicher-Abfragen. Ein Versuch daran ist 2026-08-25
gemessen und **verworfen** worden (jeder Filter war teurer als das, was er
einspart); wer es nochmal versucht, muss den Test auf **eine u64-Bloom-Abfrage**
bringen — die Engine hat `Bloom`/`bloom_covers` schon. Zu schlagen sind 411 M.

**Der eigentliche Hebel ist aber keiner dieser Posten, sondern die Menge:**
1015 Kästen neu rechnen, weil ein Teilbaum sich geändert hat, ist die falsche
Arbeit. Inkrementelles Layout auf dem schmutzigen Teilbaum ist der Posten, der
die 51 ms auf einen Bruchteil bringt — und der einzige, der mit der Seitengrösse
skaliert statt mit der Konstanten davor.

---

## Die andere Hälfte: liegt es am JIT?

**Nein, und das ist gemessen.** Der Abstand zerfällt in drei Teile, und nur
einer davon ist ein Compilerproblem.

### Teil 1 — der WASM-Aufschlag. Erledigt, und es war unser eigener Compiler.

`forge` übersetzt beim `install` WASM nach x86-64, einmal, ohne JIT
(`forge/core/`). Gemessen gegen wasmi, gleiche Eingabe, gleiches Ergebnis,
gleiches Fuel:

| | wasmi | forge | Faktor |
|---|---:|---:|---:|
| beak `parse` | 36,5 ms | 4,6 ms | **7,97×** |
| beak `cascade` | 233,8 ms | 28,8 ms | **8,12×** |
| beak `layout` warm | 599,1 ms | 104,4 ms | **5,74×** |
| python `pass` | 184,3 ms | 12,9 ms | **14,31×** |

**Dieser Hebel ist gezogen.** Und die Zahl sagt selbst, wo die Grenze liegt:
`layout` gewinnt am wenigsten, weil es die Phase mit dem meisten
Speicherverkehr ist — **wo nichts zu interpretieren war, kann ein Compiler
nichts einsparen.**

### Teil 2 — Interpreter gegen JIT: ~60×, und das schliesst nur ein JIT

Dieselben Schleifen in JS und Python:

| Probe | V8 (JIT) | CPython | beak | beak/CPython |
|---|---:|---:|---:|---:|
| arithmetik | 4 ms | 246 ms | 1087 ms | 4× |
| `a[i]` | 3 ms | 132 ms | 992 ms | 7× |
| `o.x` | 2 ms | 167 ms | 1309 ms | 7× |

Die ~60× zwischen CPython und V8 hat CPython genauso. Den holt kein Trick am
Dispatch ein, und einen eigenen JS-JIT zu bauen ist eine andere Grössenordnung
als forge — forge übersetzt einen typisierten, validierten Befehlssatz ohne
Deoptimierung; ein JS-JIT braucht Typrückschluss, Inline-Caches, Wächter und
einen Rückfallpfad.

### Teil 3 — unsere eigenen 4–7×, und die liegen im Objektmodell

Das ist der erreichbare Teil. Aus dem Profil (24 091 Abtastungen,
`<tools>/prof/`):

1. **Namen nachschlagen ~18 %** — hashbrown 7,9 + memcmp 4,6 + `env_peek` 3,1
   + `load_ident` 4,0 + `[u8]::eq` 1,7 + `Cell<isize>::get` 2,3. Es wird je
   Zugriff eine **Zeichenkette** gehasht und verglichen. Der Fix ist, den Namen
   zur Übersetzungszeit auf **(Tiefe, Fach)** aufzulösen. Grösstes verbliebenes
   Stück.
2. **`Vm::step` 10,1 %** — der grosse `match` plus ein `Rc<RefCell<Env>>`-Klon
   je Befehl (`src/js/vm.rs`, `fn step`; die Zeilennummer aus dem Profil von
   0.119.0 stimmt nicht mehr, der Posten schon).
3. **`ptr::write::<Value>` 5,2 %** — 24 Byte je Push. Das ist NaN-Boxing,
   nichts anderes.
4. **Vier frische `Vec` je Aufruf** (~2,5 %) — ein Rahmenvorrat wäre billig.

Dazu die Halde: `Rc` sammelt keine Ringe ein, und Reacts Fiberbaum IST einer —
gemessen 90 % der Bytes in Ringen (2076 → 207 MB). Das ist nicht Tempo, aber
es ist Allokatordruck, und der steht in jedem der Profile oben mit drin.

### Was forge NICHT hat — und was es wert wäre

`forge/core/src/lib.rs:130` sagt es ausdrücklich: **kein SIMD, keine Threads.**

- **SIMD (`v128`).** Würde forge um einen Befehlssatz erweitern und die
  Rust-Seite müsste mit `+simd128` gebaut werden. Für das **Layout** bringt es
  vermutlich wenig — das ist Zeigerverfolgung und Allokation, nicht
  Vektorarbeit. Für **Rasterung und Blit** wäre es der natürliche Ort.
  **Ungemessen**, und der Grund ist ein eigener Befund: `tests/diag.rs`, das
  den Malzeit-Zähler trägt, **baut zurzeit nicht** (`DrawOp::Gradient` und
  `Shadow` sind neu und im `match` nicht abgedeckt). Bevor hier jemand SIMD
  plant, muss diese Zahl auf dem Tisch liegen.
- **Threads.** Ein Modul ist heute einfädig. Layout auf einem Arbeitskern wäre
  ein grosser Umbau (Wachstum der ABI, geteilter Speicher, ein zweiter
  Trap-Pfad) und ist erst interessant, wenn inkrementelles Layout die
  Konstante gesenkt hat.
- **Nativ statt WASM ist keine Option.** Der Kernel läuft auf
  `x86_64-nopeek` mit AVX2/AES-NI, beak nicht: es ist ein WASM-Modul, und die
  Sandbox IST die Vertrauensgrenze (Architekturprinzip 5). Die Kernelflags
  helfen beak nicht, und beak nativ zu fahren würde das Prinzip aufgeben.

## Reihenfolge

Nach gemessenem Gewicht, nicht nach Aufwand:

1. **Inkrementelle Kaskade** — `scripted_gen` aus dem Schlüssel, schmutziger
   Teilbaum statt Leeren. Zahlt bei JEDEM `classList.toggle`, nicht nur hier.
2. **Relayout auf Verlangen, enge Fassung** (S1 + S2 + S3). Erst danach, weil
   es sonst 78 ms je Aufruf kostet statt einen Bruchteil.
3. **Namen auf (Tiefe, Fach)** im JS-Motor — das grösste Stück der eigenen
   4–7×.
4. **Inkrementelles Box-Layout.** Der grösste Posten und der teuerste Umbau.
5. Offen und ungemessen: Malzeit (erst `tests/diag.rs` reparieren), dann
   SIMD-Frage stellen.

## Womit gemessen wurde

- `pagerun` mit zwei neuen Schaltern (2026-09-17, beide standardmäßig aus):
  **`GEOMEVERY=1`** legt nach jeder Zeitgeber-Runde ein Bild ein, wie der Wirt
  — ohne ihn reicht die Probe die Geometrie genau EINMAL ein und jedes später
  eingehängte Element meldet für immer 0. **`PHASEDBG=1`** gibt die drei
  Phasen je Neuauslegen aus.
- **Jede Probe an `load` hängen, nicht an `setTimeout(…, 5000)`** — eine echte
  Uhr lässt einen 5-s-Zeitgeber mitten im Skriptlauf feuern, also VOR der
  ersten Messung. Diese Falle hat mich in dieser Sitzung eine Fehldiagnose
  gekostet ([[feedback_a_test_of_a_state_must_say_when]]).
- `<tools>/ddgmirror.py <dir> stansstad`, dann `pagerun` und die gemeldeten
  `dyn-Skript fehlt:` zweimal nachziehen; Gegenprobe mit
  `<tools>/mirror/mirror.py` + `chromium --headless --dump-dom` auf demselben
  Bestand.
- `<tools>/jsscope/out/apirank.json` — der Aufrufzensus über zwölf Seiten.
- Fuel-Karte und forge-Zahlen: `docs/plan/WASM_SPEED_2026_08.md`,
  `memory/project_beak_layout_perf.md`, `memory/project_beak_js_engine_speed.md`.
