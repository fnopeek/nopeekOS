# HTML-/Format-Lückenanalyse — Stand 2026-08-25, beak 0.38.0

Schwesterpapier zu `CSS_GAP_2026_08.md`. Dort ging es um Eigenschaften; hier
geht es um **Elemente, Bildformate und Verhalten**. Anlass war die Frage, ob
WebP/AVIF/GIF und die Interaktions-Eigenschaften (`cursor`, `pointer-events`,
`user-select`, `transition`, `animation`) jetzt an der Reihe sind.

Antwort: **nein** — die grösste Lücke ist ein Element, das wir schon haben,
aber dessen Verhalten fehlt.

## Woher die Zahlen kommen

Korpus ist `jsscope/` (11 echte Seiten, mit beaks eigenem UA geholt), derselbe,
gegen den die JS-Frage vermessen wurde. Alle Zensen sind vollständige
Auszählungen, keine Stichproben:

- Tag-Zensus über `jsscope/html/*.html`
- CSS-Zensus mit `css-tree` über `jsscope/css/*.css` (kein Regex — siehe die
  Falle in `tools/README.md`), 87 755 Deklarationen, 82 637 abgedeckt = 94,2 %
- Bildformate aus `<img src>`, `<img srcset>`, `<source type>`, CSS `url()`
  und `data:`-URIs getrennt gezählt

## Der Befund: die Lücke ist Verhalten, nicht Vokabular

Der Tag-Zensus findet an **Standard**-Elementen, die beak nicht kennt, nur:
`legend` (4), `wbr` (4), `area`/`map` (36, eine Seite), `hgroup` (1). Alles
andere Unbekannte sind Custom Elements (`mdn-*`, `react-partial`,
`clipboard-copy`, `turbo-frame`) — die sind per Spec inline, und ihr Kasten
kommt aus dem CSS der Seite. `video`/`audio`/`canvas`/`select`/`textarea`/
`dialog`: **0× im ganzen Korpus**.

Die Element-Seite ist also zu. Was fehlt, ist das *Verhalten* von Elementen,
die wir längst parsen.

## Rang 1 — `<details>`/`<summary>` hat keine Auf/Zu-Logik

`style.rs` gab beiden nur `display: block`; auf `open` fragte nichts ab. Ein
zugeklapptes `<details>` malte beak **vollständig ausgeklappt**.

| Seite | `<details>` | davon `open` | falsch offen |
|---|---:|---:|---:|
| mdn_docs | 119 | 2 | **117** |
| rustdoc | 407 | 397 | 10 |
| news_tagesschau | 5 | 0 | 5 |
| github_repo | 2 | 1 | 1 |
| **Summe** | **533** | 400 | **133** |

533 Vorkommen auf 4 von 11 Seiten. MDN besteht aus zugeklappten Abschnitten —
daraus wurde eine Endlosseite.

Kein JavaScript im Spiel: Auf/Zu ist UA-Verhalten. **Wichtig:** Verbergen ohne
Aufklappen wäre eine Verschlechterung — der Inhalt wäre dann unerreichbar
statt nur unaufgeräumt. Beides gehört in denselben Commit.

Nacktes Textkind direkt im `<details>` (also nicht in einem Kindelement):
**0 von 446** untersuchten. Deshalb reicht eine Regel über Element-Kinder;
der Textknoten-Fall ist ein benannter, gemessener Rest.

## Rang 2 — WebP ja, AVIF nein, GIF kaum

`<picture>` ist **schon richtig**: 236 `<source type="image/webp">` stehen
exakt 236 `image/jpeg` gegenüber, und `picture.rs` überspringt undekodierbare
Typen (`decodable_type`, mit Test). Dort kostet WebP nichts.

Das Loch ist der direkte `<img>`:

| Format | `<img src>` | CSS `url()` | `data:` |
|---|---:|---:|---:|
| webp | **87** | 0 | 0 |
| gif | 2 | 14 | 12 |
| avif | 0 | 0 | 0 |
| svg | 30 | 689 | 388 |
| png | 115 | 55 | 18 |
| jpg | 59 | 0 | 0 |

Die 87 stehen alle auf **srf.ch** — 87 von 118 Bildern der Seite, ohne jeden
Fallback. Drei Viertel der Bilder einer Nachrichtenseite bleiben leer.

- **AVIF: null Vorkommen im ganzen Korpus.** Nicht bauen.
- **GIF: 28 gesamt.** LZW ist billig (erstes Einzelbild genügt), der Ertrag
  klein.
- SVG ist das meistbenutzte Format überhaupt — und schon da.

## Rang 3 — `<noscript>` steht bei uns falschherum

Das UA-Sheet setzte `noscript` auf `display: none`. Ein Browser **ohne**
Skript rendert den Inhalt (HTML §4.12.2) — das ist wörtlich beaks Fall. Der
Parser macht es schon richtig: `RAWTEXT` in `dom.rs` ist nur `script`/`style`,
der Inhalt ist also als Markup geparst und liegt bereit.

8 Vorkommen auf 6 Seiten, 1535 Bytes. **Nachgemessen am 2026-08-25, und die
erste Zählung war falsch:** es sind keine Lazy-Load-Fallbacks. Hineingesehen:

| Seite | Inhalt | wert? |
|---|---|---|
| wikipedia_de/en | 1×1-`CentralAutoLogin`-Zählpixel, `position:absolute` | nein |
| news_srf | `<noscript class="nojs-banner">` — „bitte JS einschalten" | nein |
| mdn_docs | „Enable JavaScript to view this browser compatibility table." | nein |
| rustdoc | `<link rel=stylesheet href=noscript.css>` — **eigenes Stylesheet für skriptlose Browser** | **ja** |
| marginalia | zwei Blöcke, ungeprüft | ? |

Gebaut und getestet (295 Tests grün), aber **nicht ausgeliefert**: das Gate sagt
+19 px leere Zeile auf beiden Wikipedias (der `<noscript>`-Inline erzeugt eine
Zeilenbox, obwohl sein einziges Kind out-of-flow ist) für null Inhalt — und
beak würde anfangen, Wikipedias Zählpixel zu holen. Auf einem System namens
nopeekOS ist „die Spec sagt es" dafür kein ausreichender Grund; das ist eine
Entscheidung, keine Regelbefolgung.

Der eine echte Ertrag ist rustdocs `noscript.css`. Wenn `<noscript>` kommt,
dann dafür — und dann gehört die leere Zeilenbox vorher gefixt.

## Rang 4 — `@font-face`

75 `@font-face`-Regeln. Fehlende Namen: `src` 75, `font-display` 53,
`unicode-range` 16. Geladen werden **73 woff2, 30 woff, 30 ttf**.

Die Familien sind nicht nur Geschmack: **KaTeX_\*** (13 Faces — Mathe auf
MDN/Wikipedia) und **Font Awesome** (6 Faces — Icons). Beides fällt auf Inter
zurück, also falsche Zeichen statt Formeln und Icons.

Preis: woff2 ist Brotli, und das ist die teure Hälfte. woff1 ist zlib (haben
wir). 73 zu 30 heisst aber: der Umweg über woff1 deckt nur ein Drittel.

## Was die Messung als „nicht bauen" ausweist

Hier kippt die naheliegende Lesart der CSS-Fehlliste. Die grossen Zahlen
gehören zu Eigenschaften, die im Standbild nichts tun:

| Eigenschaft | Dekl. | warum kein Ertrag |
|---|---:|---|
| `transition` | 626 (+ Langformen 222) | braucht einen Zustandswechsel; das Standbild **ist** der Startzustand |
| `animation` | 261 (+ Langformen 299) | siehe unten — nur **10 Regeln** verlieren Inhalt |
| `cursor` | 686 | es gibt keine SDK-/Compositor-Schnittstelle für Zeigerformen; erst ABI, dann Eigenschaft |
| `pointer-events` | 372 | `hit_test` prüft nur Link-Kästen, kann also nichts blockieren |
| `user-select` | 265 (mit `-webkit-`) | beak hat keine Textauswahl |

**Die Animations-Messung im Detail.** Über alle 11 Seiten: 312 Regeln mit
`animation`/`animation-name`, 67 `@keyframes`, die bei `from`/`0%` verbergen —
aber nur **10 Regeln** setzen selbst `opacity: 0`/`visibility: hidden` und
animieren sich sichtbar. Nur diese 10 verlieren ohne Animationsmaschine
Inhalt, und es sind Modals und Toasts. Wer die will, wendet einmal den
`to`/`100%`-Block an; eine Zeitachse braucht es dafür nicht.

**Der eine echte Rest bei `pointer-events`:** `hover_at` (`layout.rs`) sammelt
*alle* Kästen, ein `pointer-events: none`-Overlay bekommt also fälschlich
`:hover`. Fünf Zeilen, kleiner Ertrag.

**`transform` ist eine halbe Sache, aber nicht dringend.** 842
`transform`-Deklarationen, davon 408 mit `rotate`/`scale`, die `style.rs`
bewusst verwirft (nur `translate` überlebt). Aufgeteilt: 102 in `@keyframes`,
55 hinter `:hover`/Zustandsklassen, **251 im Ruhebild**. Die Stichprobe der
251 ist aber fast durchweg Icon-Spiegelung (`.rtl … scaleX(-1)`,
`rotate(90deg)` auf Pfeilen) — falschherum zeigende Icons, kein verlorener
Inhalt. Und es braucht einen affinen Rasterpfad. Nicht jetzt.

## Sichtbar, offen, nachrangig

`-webkit-line-clamp` 44 (`text-overflow` haben wir, das Zeilen-Clamping
nicht), `clip-path` 71, `scrollbar-width` 36, `text-shadow` 23,
`backdrop-filter` 17, `mix-blend-mode` 17, `hyphens` 15.

## Reihenfolge

1. **`<details>`/`<summary>` + Klick-Umschalter**, dazu `<dialog>` ohne `open`
   (dieselbe Zeile, verhindert Modal-Inhalt mitten im Fliesstext)
2. **`<noscript>`** — eine Zeile im UA-Sheet
3. **WebP-Decoder** — schliesst srf.ch
4. **`@font-face`** — braucht Brotli, eigene Runde

## Grenzen dieser Messung

11 Seiten, deutsch/schweizerisch und entwicklerlastig (MDN, GitHub, rustdoc,
Hacker News, Discourse). Ein Webshop oder ein Video-Portal würde
`object-fit`, `<video>` und `aspect-ratio` anders gewichten. Der Korpus steht
in `jsscope/corpus.txt` und lässt sich erweitern; die Zensus-Skripte laufen
unverändert weiter.
