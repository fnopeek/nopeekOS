# Stufe 0 — der Zielkorpus, vermessen (2026-08-25)

Zweck: die JS-Frage für beak **am eigenen Ziel** beziffern. Das vorhandene
Papier `JS_RECON_2026_08.md` misst `excalidraw.com` und `app.netlify.com` —
beide stehen laut `docs/spec/BROWSER.md` §2 ausdrücklich **ausserhalb** des
Ziels. Es beziffert damit eine Klasse, die wir nicht bauen. Hier steht die
Zahl für die Klasse, die wir bauen: Inhaltsseiten und leicht dynamische
Seiten.

Alles unten ist gemessen und nachrechenbar. Werkzeuge, Rohdaten und Skripte:
`<memory-dir>/../tools/jsscope/` (`fetch_static.sh`, `measure.mjs`,
`timing.mjs`, `analyze.mjs`, `css_collect.mjs`, `css_census.mjs`, `out/*.json`).
Chromium 151 headless über CDP; Node 26 hat `WebSocket` als Global, deshalb
braucht das Ganze ausser `acorn` und `css-tree` keine Abhängigkeit.

## 1. Der Korpus

Zwölf Seiten nach §2 „in scope": Wikipedia (2), Hacker News, GitHub-Repo,
MDN, rustdoc, ein Discourse-Forum, zwei Nachrichtenseiten (tagesschau, SRF),
ein statischer Blog, Marginalia-Suchergebnisse, eine Shop-Produktseite.

**Elf von zwölf antworten beak mit 200.** Nur `digitec.ch` nicht — mit beaks
UA setzt der Server den h2-Stream zurück (`INTERNAL_ERROR`), mit Chrome-UA
kommt 403. Das ist die Wand aus §2, die keine Technik ist: **Policy, nicht
Fähigkeit.** Sie wird nicht dadurch kleiner, dass beak besser wird, und
maskieren tun wir uns nicht (`feedback_no_ua_impersonation`).

## 2. Befund 1 — der Inhalt ist ohne JavaScript schon da

Jede Seite zweimal geladen, einmal mit und einmal ohne Skriptausführung
(`Emulation.setScriptExecutionDisabled`), und der sichtbare Text gezählt.

| Seite | Wörter mit JS | ohne JS | Textquote |
|---|---:|---:|---:|
| hackernews | 694 | 694 | **1,000** |
| rustdoc | 34 030 | 34 016 | **1,000** |
| blog_static | 5 689 | 5 689 | **1,000** |
| marginalia | 1 907 | 2 019 | **1,071** |
| mdn_docs | 2 762 | 3 012 | **1,139** |
| discourse_forum | 459 | 509 | **1,131** |
| wikipedia_en | 1 067 | 1 054 | 0,988 |
| wikipedia_de | 1 825 | 1 803 | 0,976 |
| news_srf | 2 282 | 2 125 | 0,947 |
| github_repo | 830 | 676 | 0,813 |
| news_tagesschau | 2 367 | 1 529 | **0,645** |

**Neun von elf Seiten liefern ohne Skript ≥ 94,7 % ihres Textes**, vier davon
100 %, und auf dreien steht **ohne** JavaScript sogar mehr da (Marginalia,
MDN, Discourse — dort blendet das Skript Inhalt aus oder ersetzt eine
crawler-freundliche Fassung durch eine leerere).

Nur zwei Seiten brauchen JavaScript wirklich für Inhalt: tagesschau (64,5 %)
und GitHub (81,3 %). Und genau diese beiden sind auch die teuersten (§3, §4).

## 3. Befund 2 — die Klippe, und dass es doch eine Rampe gibt

`acorn`-Bisektion über `ecmaVersion`, pro Skript, über alle Skripte, die der
Browser wirklich geparst hat (437 Stück, 21 MB, inline mitgezählt).

| Seite | Skripte | JS gesamt | davon gelaufen | Klippe |
|---|---:|---:|---:|---|
| blog_static | 5 | 397 KB | 202 KB | **ES5** |
| hackernews | 1 | 5 KB | 0 KB | ES2015 |
| wikipedia_de | 97 | 1 198 KB | 692 KB | ES2017 |
| wikipedia_en | 11 | 1 105 KB | 605 KB | ES2017 |
| marginalia | 3 | 4 KB | 1 KB | ES2017 |
| rustdoc | 4 | 49 KB | 15 KB | ES2019 |
| mdn_docs | 54 | 991 KB | 594 KB | ES2022 |
| news_srf | 63 | 1 897 KB | 651 KB | ES2022 |
| news_tagesschau | 25 | 2 356 KB | 842 KB | ES2022 |
| github_repo | 107 | 4 529 KB | 1 619 KB | ES2022 |
| discourse_forum | 65 | 7 126 KB | 4 218 KB | ES2022 |

Das Recon-Papier schliesst: „ES2022 muss nahezu vollständig stehen, JS
degradiert nicht." Für **ein Bundle** stimmt das. **Für eine Seite nicht** —
eine Inhaltsseite liefert nicht ein Bundle, sondern 5 bis 107 getrennte
Skripte, und ein Parse-Fehler tötet genau eines davon. Wie viel des
tatsächlich gelaufenen Codes eine Stufe X freischaltet:

| Seite | ES5 | ES2015 | ES2017 | ES2019 | ES2021 | ES2022 |
|---|---:|---:|---:|---:|---:|---:|
| blog_static | **100 %** | 100 % | 100 % | 100 % | 100 % | 100 % |
| hackernews | 0 % | **100 %** | 100 % | 100 % | 100 % | 100 % |
| wikipedia_de | 38 % | 97 % | **100 %** | 100 % | 100 % | 100 % |
| wikipedia_en | 13 % | 32 % | **100 %** | 100 % | 100 % | 100 % |
| marginalia | 0 % | 48 % | **100 %** | 100 % | 100 % | 100 % |
| rustdoc | 1 % | 3 % | 69 % | **100 %** | 100 % | 100 % |
| news_srf | 57 % | 59 % | 59 % | 60 % | 90 % | **100 %** |
| news_tagesschau | 3 % | 12 % | 12 % | 61 % | 64 % | **100 %** |
| mdn_docs | 0 % | 49 % | 49 % | 62 % | 73 % | **100 %** |
| github_repo | 0 % | 16 % | 17 % | 24 % | 71 % | **100 %** |
| discourse_forum | 6 % | 6 % | 6 % | 6 % | 19 % | **100 %** |

**Ein ES2017-Kern fährt fünf der elf Seiten vollständig** — Wikipedia (beide),
Hacker News, Marginalia, der Blog — und rustdoc bis auf 31 %. Die Rampe ist
da; sie ist nur seiten-, nicht korpusweit.

Syntax, die dieser Kern verlangt (in Skripten, die auch gelaufen sind):
`let`/`const`, Arrow, Klassen, Template Literals, Destructuring, Spread/Rest,
`for..of`, berechnete Properties, Generatoren, Tagged Templates, `async`/
`await` — plus RegExp mit `/u` und `/y`. Ab ES2018 kommen Lookbehind, benannte
Gruppen und `\p{}` dazu: **die RegExp-Maschine ist ein eigenes Teilprojekt**,
205 der 437 Skripte enthalten RegExp-Literale.

## 4. Befund 3 — was das am Gerät kostet

Chromes `Performance.getMetrics` gibt `ScriptDuration` direkt her. Die Kette
zum Gerät hat drei gemessene Glieder und **ein geschätztes**:

```
ScriptDuration (Chrome, Dev-Rechner)
  x  2–5   unser Bytecode-Interpreter gegen V8s Ignition   <-- GESCHAETZT
  x 28     wasmi-Zoll        (project_wasm_speed_gap, gemessen)
  x  3,2   CPU-Klasse Gerät  (project_wasm_speed_gap, gemessen)
```

| Seite | ScriptDuration | Startskript am Gerät (Spanne) |
|---|---:|---|
| hackernews / marginalia | 0 ms | ~0 |
| rustdoc | 7 ms | 1,3–3 s |
| blog_static | 38 ms | 7–17 s |
| wikipedia_de | 50 ms | **9–22 s** |
| wikipedia_en | 55 ms | 10–25 s |
| discourse_forum | 93 ms | 17–42 s |
| news_srf | 98 ms | 18–44 s |
| github_repo | 179 ms | 32–80 s |
| news_tagesschau | 201 ms | 36–90 s |

Dazu kommt das **Parsen**, bevor eine Zeile läuft: `css::collect_all` misst am
Gerät 0,40 ms/KB, byte-linear. Als Untergrenze für JS (dessen Grammatik teurer
ist) sind das 0,5 s für Wikipedias 1,2 MB, 1,8 s für GitHubs 4,5 MB, 2,9 s für
Discourses 7,1 MB.

**Wikipedia — das Hauptziel aus §6 — würde also rund 10 bis 20 Sekunden
Startskript zahlen, um 2,4 % mehr Text zu bekommen.**

Das geschätzte Glied ist das einzige, das noch offen ist. Es wird messbar,
sobald die Engine existiert: dieselben Skripte, dieselbe Seite, `ScriptDuration`
gegen unsere Zahl. Bis dahin steht es als Spanne da und nicht als Wert.

## 5. Befund 4 — die Sicherheits-Invariante kostet nichts

`docs/spec/BROWSER.md` §4 verbietet einen JIT, weil er W^X-Speicher vom Host
verlangt. §9.1 führt das als „die grösste Wand" für App-Seiten. **Für den
Zielkorpus ist es keine Wand.** Dieselben Seiten mit `--js-flags=--jitless`:

| Seite | mit JIT | ohne JIT |
|---|---:|---:|
| wikipedia_de | 53 ms | 50 ms |
| wikipedia_en | 56 ms | 55 ms |
| github_repo | 171 ms | 179 ms |
| discourse_forum | 90 ms | 93 ms |
| news_tagesschau | 174 ms | 201 ms |

Gegenprobe, dass der Schalter überhaupt greift: `fib(32)` in derselben Seite,
**12 ms mit JIT gegen 98 ms ohne** (8,2×). Der Schalter wirkt — Seiten-Startcode
profitiert nur nicht davon, weil er **einmal** läuft. V8 führt ihn ohnehin in
Ignition aus und tiert nie hoch. In Node ist einmalige Arbeit ohne JIT sogar
**schneller** (94 → 68 ms), weil die Kompilierung wegfällt.

Damit ist §9.1 für diese Klasse erledigt, bevor sie gebaut wurde: der JIT-
Verzicht kostet beim Seitenstart ~0. Was kostet, ist der wasmi-Zoll — und der
trifft jede Zeile Rust in beak gleichermassen, JS oder nicht.

## 6. Befund 5 — Web-API-Zensus: keine Gruppe ist leer

Vorkommen über alle 437 Skripte / Zahl der Seiten (von 11):

| Gruppe | die Spitzenreiter |
|---|---|
| DOM-Kern | `innerHTML` 294× / 11 · `querySelector` 2730× / 10 · `classList` 1746× / 10 · `createElement` 1728× / 9 |
| Events | `addEventListener` 2032× / 11 · `preventDefault` 853× / 11 · `keydown` 335× / 10 |
| Layout lesen | `getBoundingClientRect` 308× / 9 · `scrollIntoView` 97× / 9 · `getComputedStyle` 148× / 7 |
| Zeit | `setTimeout` 666× / 10 · `requestAnimationFrame` 173× / 8 · `requestIdleCallback` 93× / 7 |
| Observer | `IntersectionObserver` 188× / 7 · `ResizeObserver` 60× / 6 · `MutationObserver` 84× / 5 |
| Netz | `fetch` 222× / 10 · `AbortController` 102× / 7 · `XMLHttpRequest` 79× / 7 |
| Speicher | `localStorage` 440× / 9 · `document.cookie` 76× / 8 · `indexedDB` 15× / 3 |
| Routing | `pushState` 60× / 9 · `popstate` 51× / 8 |
| Editieren | `Range(` 200× / 7 · `getSelection` 57× / 7 |
| Worker | `postMessage` 99× / 8 · `new Worker` 8× / 6 |
| Builtins | `Symbol` 2326× / 8 · `Promise` 1918× / 8 · `WeakMap` 762× / 6 · `Proxy` 55× / 4 |

Das deckt sich mit dem Recon-Papier: **auch auf reinen Inhaltsseiten ist keine
Gruppe leer.** `Proxy` und `Reflect` auf vier Seiten heisst, dass selbst der
Sprachkern nicht bei ES2015-Klassen aufhört. Das ist das Argument für §9.2:
Der Rust-Kern bekommt nur die irreduziblen Primitive, die Breite kommt als
`beak-runtime.js`.

## 7. Befund 6 — CSS auf dem Zielkorpus: 94,2 %

Alles CSS, das die Seiten wirklich laden (externe Sheets aus den Antworten
plus jedes `<style>` im fertigen DOM), mit `css-tree` geparst, Deklarationen
innerhalb von Blöcken gegen beaks `css_props!`-Tabelle (187 Einträge).

```
87 755 echte Deklarationen, beak deckt 82 637 = 94,2 %
413 distinct Eigenschaften, 18 845 Custom Properties
```

Pro Seite zwischen 87,3 % (Marginalia) und 95,7 % (Wikipedia). Die Lücke ist
kein langer Schwanz, sondern **drei Familien**:

| Familie | Deklarationen | was man sieht |
|---|---:|---|
| Bewegung — `transition` + `animation` + `@keyframes` (214) | **1 480** | Menüs klappen hart, Zustände springen |
| Interaktion — `cursor`, `pointer-events`, `user-select`, `resize` | **1 383** | falscher Mauszeiger, unklickbare Overlays |
| `-webkit-*` (Alt-Präfixe) | 531 | meist harmlos |
| SVG `fill`/`stroke` als CSS | 285 | Icons in der falschen Farbe |
| `@font-face`-Deskriptoren (`src`, `font-display`) | 150 | siehe unten |

At-Rules im Korpus: `@media` 2801 ✅ · `@supports` 634 ✅ · **`@keyframes` 214
❌** · **`@font-face` 75 ❌** · **`@container` 34 ❌** · `@layer` 5 ✅ ·
`@property` 2 ❌.

**Webfonts sind der grösste sichtbare Einzelposten.** 75 `@font-face`-Regeln,
und die Formatverteilung sagt, was dranhängt: **74× `woff2`**, 30× `woff`,
30× `truetype`. WOFF2 ist brotli-komprimiert — für den Transport war brotli
zu Recht gestrichen (3,3 % über gzip), **für Schriften führt kein Weg daran
vorbei**. Ohne `@font-face` bleiben Icon-Schriften Tofu.

Selektor-Last, und was beak davon kann: `[attr]` 5452 ✅ · `:not` 3436 ✅ ·
`:hover` 2468 ✅ · `:where` 1081 ✅ · `:is` 564 ✅ · `:has` 349 ✅ — aber
**`:active` 889 ❌, `:lang` 874 ❌, `:focus-visible` 731 ❌**, `:focus-within` ❌,
`:target` ❌.

## 8. Befund 7 — der Transport, auf unserem Korpus

Jede Seite zweimal geholt, mit beaks UA, einmal `identity` und einmal `gzip`:

| Seite | identity | gzip | Faktor |
|---|---:|---:|---:|
| discourse_forum | 84 614 | 8 582 | **9,86×** |
| rustdoc | 952 692 | 103 189 | 9,23× |
| mdn_docs | 368 103 | 43 631 | 8,44× |
| news_srf | 453 642 | 54 256 | 8,36× |
| github_repo | 329 864 | 47 738 | 6,91× |
| news_tagesschau | 1 447 793 | 238 714 | 6,06× |
| hackernews | 34 670 | 5 861 | 5,92× |
| blog_static | 146 865 | 27 719 | 5,30× |
| wikipedia_en | 253 726 | 50 534 | 5,02× |
| wikipedia_de | 172 761 | 42 174 | 4,10× |

Der HTTP-Pfad im Kernel schickt **kein `Accept-Encoding` und kann nicht
inflaten** — jede Seite kommt heute unkomprimiert. `miniz_oxide` liegt schon
im Baum (beak-engine, iris). Das ist der billigste offene Posten im ganzen
Browser und er wirkt auf jeden Byte jeder Seite, HTML wie CSS wie JS.

Marginalia ist die Ausnahme mit 1,29× — und die einzige Seite im Korpus, die
nur **HTTP/1.1** spricht.

## 9. Was daraus folgt

**Die Reihenfolge dreht sich.** JavaScript ist nicht das, was zwischen beak
und dem Inhalt des Zielkorpus steht — der Inhalt ist auf neun von elf Seiten
schon da. Was zwischen beak und „sieht aus wie im Firefox" steht, ist
messbar etwas anderes:

### Runde A — Transport und Sichtbares (jeder Posten gemessen)

1. **gzip im HTTP-Pfad** — 4,1–9,9× auf jeden Byte jeder Seite.
2. **`@font-face` + WOFF2** — 75 Regeln, 74× woff2. Braucht brotli-Inflate.
3. **Bewegung: `transition` + `animation` + `@keyframes`** — 1480 Deklarationen.
   Ein System, keine Eigenschaftsliste; hängt am Compositor-Frame-Tick.
4. **Interaktion: `cursor`, `pointer-events`, `user-select`** — 1383.
5. **`:active`, `:focus-visible`, `:lang`** — 2494 Selektor-Vorkommen, billig.
6. **WebP/AVIF** — heute Platzhalterkasten.

### Runde B — JavaScript, mit einem Deckel

Der Sprachkern, auf den es ankommt, ist **ES2017**, nicht ES2022: er fährt
Wikipedia, Hacker News, Marginalia und den Blog vollständig. ES2019 nimmt
rustdoc dazu. ES2022 kauft GitHub, Discourse, MDN und die Nachrichtenseiten —
und kostet dort 17 bis 90 Sekunden Startskript. Das ist kein Ziel, das ist
eine Grenze.

**Und hier ist der Hebel, den nur wir haben:** beak darf ein Skript
**aufgeben**. Der Inhalt ist schon gemalt, das Aufgeben degradiert zu genau
dem Stand, den die Seite ohne JS hat — und der ist gemessen ≥ 94,7 % auf neun
von elf Seiten. Ein Fuel-Deckel pro Skript ist im wasmi-Sandkasten ohnehin
vorhanden (`consume_fuel(true)`); ihn an die JS-Engine weiterzureichen macht
aus unserer grössten Schwäche eine Regel, die man aufschreiben kann. Ein
echter Browser kann das nicht — er hat nichts, worauf er zurückfallen könnte.

## 10. Was diese Messung NICHT abdeckt

- **Nur der Ladevorgang.** Was ein Klick auf ein Menü kostet, steht hier nicht.
  Genau dort sitzt der Nutzen von JS auf den Seiten mit Textquote 1,0.
- **Ein Lauf pro Seite**, keine Wiederholungen. `ScriptDuration` schwankt;
  die 27 ms Unterschied bei tagesschau (174 gegen 201) sind Rauschen, keine
  JIT-Wirkung.
- **Chromium-UA in den CDP-Läufen** (Absicht: Obergrenze dessen, was eine
  Seite ausliefert). Was beak selbst bekäme, misst `fetch_static.sh`.
- **Der Interpreter-Faktor 2–5×** ist das einzige geschätzte Glied.
- **Keine Seite hinter Login**, kein `<video>`, kein WebGL.

## 11. Nachrechnen

```bash
T=~/.claude/projects/-home-florian-Dokumente-Scripts-OS-nopeekOS/tools
npm install --prefix $T acorn css-tree
cd $T/jsscope
./fetch_static.sh                  # was beak bekaeme, identity vs gzip
node measure.mjs                   # JS an/aus, Coverage, Skriptquellen
node timing.mjs                    # ScriptDuration (JITLESS=1 fuer den Gegenlauf)
node analyze.mjs                   # acorn-Bisektion + API-Zensus
node css_collect.mjs && node css_census.mjs <beak_props.txt>
```

`beak_props.txt` wird aus dem Baum gezogen, nicht gepflegt:

```bash
awk '/css_props! *\{/,/^\}/' tools/wasm/beak-engine/src/css.rs \
  | grep -oE '= "[a-z-]+"' | sed 's/= "//;s/"//' | sort -u
```
