# CLAUDE.md – nopeekOS Development Guide

## What is nopeekOS?

An AI-native operating system, rethought from scratch.
Not a Unix clone. Not POSIX. No legacy.

See README.md for the full vision and phase planning.

## Architecture Principles (DO NOT violate)

1. **Capabilities, not Permissions** – No chmod, no ACLs, no root
2. **Intents, not Commands** – Express intention, not instructions
3. **Content-addressed, not path-addressed** – No filesystem tree
4. **Runtime-generated, not pre-installed** – Tools built on demand
5. **Formally bounded** – WASM sandbox as trust boundary

## Code Rules

- Language: Rust (no_std, nightly, edition 2024)
- Target: `x86_64-nopeek` — our own spec in `targets/`, = bare metal WITH
  SSE/AVX2/AES-NI. Do not go back to overriding features on
  `x86_64-unknown-none`; that contradicts its softfloat ABI and rustc is
  turning it into a hard error.
- No POSIX, no libc, no std
- Every resource is capability-gated
- Panic = Kernel Panic = Halt (no recovery in Phase 1)
- All `unsafe` blocks MUST have a SAFETY comment
- Serial is primary I/O, not VGA
- Comments in English, minimal
- Hardware drivers: follow Linux source 1:1 (see memory/feedback_linux_strict.md)

## Build & Run

```bash
./build.sh build        # Compile only
./build.sh qemu         # Build + QEMU (development)
./build.sh debug        # Build + QEMU with GDB stub
./build.sh release      # Build + sign (ECDSA P-384) → release/ for OTA
./build.sh vbox         # Build + VirtualBox (demo)
./build.sh vbox-clean   # Remove VirtualBox VM
./build.sh installer    # Two-pass installer build (bundled assets)
./build.sh usb /dev/sdX # Build installer + flash USB stick
./build.sh usb-full /dev/sdX  # USB stick + LibreWolf bundle (~290 MB,
                              # browser ready on first boot, no OTA needed)
./build.sh qemu-installer-full  # QEMU installer test with bundle
```

## Current Status

**Stand 2026-09-12 · beak 0.171.0 · Kernel 0.336.0, AM GERAET GELAUFEN** (Rest: `git log`)

**0.170.0: vier Releases ohne eine Zeile im Selbsttest.** Kein neues
Merkmal, sondern das, was die Prüfseite selbst als Regel führt: *„Ohne diese
Zeilen sagt ein gruener Lauf nur, dass nichts KAPUTT ist — nicht, dass das
Neue am Geraet geht."* Alles aus 0.166–0.169 stand host-seitig gemessen da und
in `beak:selftest` gar nicht. Jetzt: getaggte Vorlagen (samt der IDENTITAET,
an der lit-html seinen Zwischenspeicher schluesselt), `yield*` in drei Zeilen
(Delegation · `throw` weiterreichen · das Ergebnisobjekt ROH), die Gestalt
eines async-Generators — **Sprache 55/55 → 62/62**. Dazu zwei Zeilen, die
nicht mitgezaehlt werden koennen und deshalb eigene sind: der ASYNCHRONE Teil
(`for await`, `yield*` im async-Generator, und dass drei `next()` sich
ANSTELLEN) meldet nach, wie `micro` es tut, und der KASTEN eines
Steuerelements wird beim Klick NACHGERECHNET statt gezeigt (Feld 100x34,
Kaestchen 13 — die Masse aus 0.168, aber mit der Schrift des Geraets).

**0.171.0: der Tabstreifen, nach zwei Geraetefunden.** Florian: „wir muessen
tabs optisch noch bisschen mehr hervorheben und das x symbol malt bisschen
unscharf". Beides hatte einen Grund, und der zweite betrifft nicht nur das `x`.

**Der Atlas fuehrt 16, 24, 32, 48 und 64 — sonst nichts.** Ein Icon in
`size: 12` bekommt also das 16er und wird auf 4:3 verkleinert. Das Verkleinern
mittelt korrekt ueber Flaechen (der Kommentar an der Rufstelle sagte noch
„nearest-neighbour" und war veraltet), und GENAU deshalb wird ein 1,5 px
breiter Phosphor-Strich dabei weich: eine Flaechenmittelung eines duennen
Diagonalstrichs auf 0,75× IST unscharf. Der Knopf verlangt jetzt 16, also ein
1:1-Blit, und `TAB_BTN` waechst von 20 auf 22, damit es hineinpasst
(`TAB_CHARS` rechnet sich selbst nach). **Benannt und nicht gefixt:** `volume`
verlangt 20 und wird aus dem 24er verkleinert — dieselbe Ursache, anderes
Modul.

**Der aktive Tab bekommt einen zwei Pixel hohen Akzentstreifen oben.** Der
Unterschied war bis hierher `Surface` gegen `SurfaceElevated` plus eine
Textfarbe — richtig nach §3 `tab`, aber am Geraet zu leise. Der Streifen liegt
IM Tab, nicht darueber (sonst verschoebe er die Beschriftung des aktiven gegen
die der anderen), und die Polsterung ist dafuer vom Tab in seine innere Zeile
gewandert. Dabei beinahe in eine Falle getreten: **in einer `Column` ist
`align` die QUERachse**, also die Breite — mit `Align::Start` haette der
Streifen seine natuerliche Breite bekommen, und die ist bei einer Zeile ohne
Kinder null.

**0.170.1, und der erste Gerätelauf sagte genau das, wofür die Seite da
ist.** Florian am 2026-09-12: `Sprache 62/62 · Dokument 39/39`, der
asynchrone Teil vollstaendig, und **`ctlbox: Feld 100x34, Kaestchen 13`** —
die UA-Masse aus 0.168 kommen am Geraet an. Das einzige `NEIN` war MEINS: die
`IntersectionObserver`-Zeile prueft eine Momentaufnahme, als waere sie
invariant, und ueberschrieb ihr eigenes richtiges JA, sobald man scrollt
(`oben -56` — und das stimmte, die Urteilszeile war dann wirklich draussen).
Das Urteil faellt jetzt EINMAL, beim ersten vollstaendigen Paar, und bleibt
stehen. **Ein Test, der einen Zustand prueft, muss sagen, WANN er ihn
prueft.**

**Und eine Zeile prueft eine Abwesenheit:** `$262` darf auf einer Seite NIE
stehen. Es ist das Wirtsobjekt des Konformanzlaeufers; sein `evalScript` waere
ein zweiter Weg, Code an der Skript-Zustellung vorbei laufen zu lassen. Ein
versehentlich angeschalteter Wirt faellt sonst niemandem auf.

**0.169.0: `yield*` — der dritte Weg in eine angehaltene Maschine.**
`79,73 → 82,74 %` (+2394 Tests), `for await` 64,9 → **89,9 %**, und die
Absage `yield-delegate` (2560 Ruempfe) ist aus beiden Listen verschwunden.

**Delegation hat in beak NIE funktioniert** — auch im gewoehnlichen Generator
nicht: der Uebersetzer sagte ab, der Baumlaeufer dahinter warf „generators are
not supported". Der Grund, warum es kein Anbau war, steht in einem Satz: **an
der Anhaltestelle muss die Maschine WISSEN, womit sie wieder angeworfen
wurde** — mit einem Wert, einem Wurf oder einem `return` —, um genau das an
den inneren Iterator weiterzureichen. `Vm::send` liefert nur einen Wert,
`inject_throw` wickelt sofort ab, `close` gibt auf. Also `Vm::Resume` mit drei
Werten, `send_throw`/`send_return` daneben, und `at_delegate()` als die Frage,
die `gen.throw()` und `gen.return()` vorher stellen: steht die Maschine an
einem `yield*`, geht beides WEITER statt zu wirken.

`yield*` selbst ist eine SCHLEIFE in Befehlen, kein Befehl — `DelegateStart` ·
`DelegateCall` · (`Await`) · `DelegateStep` · `YieldDelegate` · `Jump`.
Derselbe Schnitt wie bei `for await`, und aus demselben Grund: zwischen dem
Anstossen des inneren Iterators und dem Auswerten seines Ergebnisses liegt im
async-Generator ein Anhaltepunkt, und den kann man nicht in einen Befehl
falten. Drei Feinheiten, die die Tests prueften: **das Ergebnisobjekt des
INNEREN geht unveraendert hinaus** (im gewoehnlichen Generator — es noch
einmal einzupacken gaebe `{value:{value:1,done:false},done:false}`), ein
**erschoepfter innerer Iterator nach `gen.return(v)` beendet den AEUSSEREN
Rumpf** (derselbe Ausgang wie `Op::Ret`, deshalb dort herausgeloest), und ein
**umgehuellter synchroner Iterator laesst nur seinen `value` abwarten**, nicht
sein Ergebnis — ohne das kam aus `yield* [Promise…]` das Versprechen selbst.

Nebenbefund beim Bauen: **der async-Generator brauchte dieselbe MARKE** —
mit einem gewoehnlichen `Op::Yield` sah seine Anhaltestelle aus wie jede
andere, und ein `agen.throw(e)` wickelte den aeusseren Rumpf ab, statt ihn
weiterzureichen.

**▶ Als naechstes in JS:** `$262.IsHTMLDDA` steht jetzt oben (≈1545 Tests
ueber zwei Meldungen — der `[[IsHTMLDDA]]`-Exot, ein Objekt, das sich wie
`undefined` VERHAELT; eine Aenderung am Objektmodell), dann `\p{…}` 886
(davon 700 `Script_*`, also ein viel kleineres Ziel), `import()` 392,
`createRealm` 353.

**0.168.0: die UA-Masse eines Steuerelements standen als EINE Zahl fuer
alle da.** `PAD_Y = 3`, ein 1-px-Rahmen, `CTL_PAD_X = 6` — geschaetzt, nicht
gemessen. Die neue Vorlage `tools/fixtures/controls.html` stellt jedes
Steuerelement VIERMAL hin (nackt · nur gepolstert · nur gerahmt · beides), und
aus den vier Hoehen faellt jedes Mass einzeln heraus: **ein Feld und ein Knopf
tragen 2 px Rahmen je Seite, ein `<select>` und ein `<textarea>` einen; die
senkrechte Polsterung ist 1 px, beim `<textarea>` 2, beim `<select>` null (die
zwei Pixel stecken in seinem Widget); waagrecht polstert ein TEXTfeld 2 px und
ein Knopf 6.** Dazu drei echte Fehler: **`layout_box_inner` ueberschrieb die
Hoehe, die `control_box` schon richtig aufgeloest hatte** — ohne `box-sizing`,
also ohne Polsterung und Rahmen, und ein `<input>` in einer Flex-Zeile kam
20 statt 34 px hoch heraus (die Quer-Achse desselben Fehlers, den 0.166
geschlossen hat) · `<textarea>` nahm `cols=30 rows=3` statt der 20 und 2 aus
HTML §4.10.11 und schnitt `rows * Zeilenhoehe` erst am Ende ab (drei Pixel bei
vier Zeilen) · `min-height` rechnete mit der UA-Untergrenze statt mit der
WIRKLICHEN Polsterung.

**Und die Eigenbreite kommt aus der Schrift, nicht aus der Breite der Null.**
`size=n` mal der Breite von „0" war bis zu 50 px zu schmal. Richtig ist
`n × mittlere Zeichenbreite + (breitestes Zeichen − mittleres)` — beides echte
Tabellen der Schrift (`OS/2.xAvgCharWidth`, Umrisskasten aus `head`), gelesen
in `gsub.rs`, wo die Tabellen ohnehin schon offenstehen. **Gemessen ueber
fuenf Stuetzstellen (`size` 1, 5, 10, 20, 40), und erst die dritte sagt, ob
die Gerade stimmt** — zwei Punkte passen auf jede. Zwischenstand unterwegs:
mit `hhea.advanceWidthMax` blieb ein KONSTANTER Versatz von 37 px ueber alle
fuenf, und ein Fehler, der sich mit der Groesse nicht aendert, sitzt im
konstanten Glied.

Ergebnis auf der Vorlage: **47 von 49 Kaesten byte-gleich mit Chromium**, die
zwei Reste sind ein Pixel bei `size=1` und `size=5`. Bootstrap 17-64-px-Eimer
23 → 20; `ua.html` und Tailwind unveraendert. WPT **4546 → 4547**, und der
eine Rueckgang ist keiner: `input-number-text-size.tentative` prueft eine
VORSCHLAGS-Regel (implizites `size` aus `min`/`max`), die wir nicht bauen —
sie lag mit 0,34 % unter der Schwelle, weil unser Feld zu schmal war, und
sagt bei 0,67 % jetzt die Wahrheit. **Offen und benannt:** die Zeilenhoehe um
ein Steuerelement herum (ein `<textarea>` auf einer Zeile laesst darunter
11 px zu wenig Platz — die Grundlinie eines atomaren Inline, ein eigener
Posten).

**0.167.0: async-Generatoren — und der Uebersetzer sagte fuer den GANZEN
Chunk ab.** `74,54 → 79,73 %` (+4110 Tests, gleicher Nenner), Befehlsmaschine
`99,0 → 99,8 %` der Programme. **Sie sind nicht die Summe von Generator und
async-Funktion, sondern ihre Verschraenkung** — und deshalb war es EIN Posten:
`Step` kennt `Yield` und `Await` laengst, die `Vm` haelt an beidem an, es
fehlte der Vertrag darum herum. Drei Stuecke: eine **Anfrage-Schlange**
(`agen.next()` gibt sofort ein Versprechen zurueck, auch mitten im `await`;
drei `next()` muessen sich anstellen statt die Maschine dreimal anzuwerfen),
**`yield x` wartet seinen Wert ERST ab** (ES 15.5.5 — die Regel steht im
UEBERSETZER, ein `Op::Await` vor jedem `Op::Yield`, damit `Op::Yield` eine
Bedeutung behaelt), und **`for await` haelt MITTEN in der Schleife an**
(`Op::IterNext` ruft und liest in einem Schritt — hier aufgeteilt in rufen ·
warten · auswerten). Der grosse Nebengewinn steht in keiner Testzahl: **vier
der fuenf Absagestellen waren „eine Funktion DANEBEN ist ein
async-Generator"**, also fielen 2518 Programme komplett auf den Baumlaeufer.
`%AsyncFromSyncIterator%` ist bewusst kein Objekt — wir merken `is_async` am
Iteratoreintrag und warten nur den `value` ab.

**▶ Als naechstes in JS: `yield*`.** 2560 Rumpfabsagen, 2629 Fehler — und
**Delegation hat in beak noch NIE funktioniert**, auch im gewoehnlichen
Generator nicht. Kein Nachmittag: `yield*` muss an der Anhaltestelle WISSEN,
womit es wieder angeworfen wurde (Wert, Wurf oder `return`), um es an den
inneren Iterator weiterzureichen — `Vm::send` liefert nur einen Wert,
`inject_throw` wickelt gleich ab. Das braucht einen dritten Weg in die
angehaltene Maschine.

**0.166.0: fuenf Kastenfehler, die WPT nicht sehen kann — und eine
Testquote, die FALLEN musste.** Zwei Haelften.

**(1) Steuerelemente und Floats, gegen Chromium auf Vorlagen aus sechs
Zeilen.** WPT steht bei 4546 vorher wie nachher; gefunden hat alles fuenf
`<tools>/gallery/run.py`. **Ein Ausgleich ueberlebte die Luecke, die er
ausglich**: `flex_metrics` zog einem Steuerelement Polsterung + Rahmen ab,
weil `intrinsic_width` das 2026-09-04 noch nicht tat — seit 0.145.0 tut es
das selbst, und seither zog es ZWEIMAL ab, waehrend `resolve_flex_line` nur
einmal wieder drauflegt. Bootstrap-Knopf 48 statt 74 px, und der wachsende
Nachbar bekam die Differenz. **Ein Steuerelement nimmt die Breite, die es
bekommt** — `layout_box_inner` legt seine Raender nicht an (Vertrag der
Flex-/Raster-/Zellenwege), aber `flow_children` uebergab die Breite des
UMGEBUNGSkastens: `display:block; width:100px; margin-left:50px` malte
1902 px auf x = 0. Dieselbe Ursache am Float, deshalb sass Bootstraps
`.form-check-input` auf der Polsterkante — jede Checkbox, jeder Radioknopf.
Dazu: **ein Prozent an einem `inline-block` loeste sich ZWEIMAL auf**
(`col-6` = 469 statt 939; `place_float` kennt und benennt die Falle seit
0.138.0, der Inline-Weg war der letzte ohne sie), und **ein negativer Rand an
einem Float liess den Kasten WACHSEN** statt ihn zu verschieben. Bootstrap
168 → 171 identisch, groesste Abweichung 470 → 47 px.

**(2) Der test262-Laeufer war die groessere Luecke als die Sprache.** Er
uebersprang jede `async`-Datei mit „Promises gibt es noch nicht" — die gibt
es seit 0.92.0. **5485 Dateien lagen als „uebergangen" im Bericht.**
Angeschaltet: **59 132 / 79 325 = 74,54 %** statt 56 639 / 69 194 = 81,86 %.
Die Quote faellt um sieben Punkte und der Lauf wird um 2493 Tests besser —
dasselbe Ereignis. Dafuer gebaut: **`$262`** (`global`,
`detachArrayBuffer`, `evalScript`, `gc`), und es erscheint NUR, wenn der Wirt
es bestellt, genau wie `crypto` — `evalScript` waere sonst ein zweiter Weg an
der Skript-Zustellung vorbei. Dazu `$DONE` im Laeufer statt
`doneprintHandle.js` (das schreibt mit `print` auf die Ausgabe; wir leeren
`run_jobs` und lesen das Ergebnis aus dem globalen Objekt).

**Und getaggte Templates, in KEINER der beiden Maschinen gebaut** — der
Uebersetzer sagte ab, der Baumlaeufer dahinter warf. Damit starb jede Seite
mit lit-html, styled-components oder graphql-tag an der ersten Zeile ihrer
Bibliothek. **Die Identitaet ist der eigentliche Vertrag**: dieselbe Stelle
muss bei jeder Auswertung denselben Gegenstand liefern (lit schluesselt seine
`WeakMap` damit), also Adresse als Schluessel UND die rohen Zeichenketten
daneben — eine Adresse ist nur belegt eine Identitaet. Nebenbei drei
Lexer-Fehler, die nur ein getaggtes Template zeigt: die Fehlererholung frass
das schliessende Akzentzeichen, `\1`–`\9` sind im Template verboten und
wurden angenommen, und OHNE Marke ist jede ungueltige Flucht ein
Fruehfehler — das wurde still geschluckt.

**0.165.0: eine Spalte, die nicht schrumpfen kann, macht den Tisch nicht
breiter.** Florian: „text bricht immer noch raus.. manchmal". Im Bild brechen
ALLE sechs Zeilen auf derselben zu grossen Breite — also kein Umbruchfehler,
sondern ein Kasten, der zu breit ist. `auto_columns` verteilte anteilig und
klemmte danach mit `.max(minw[c])` — **und was das `.max` dazulegte, wurde
niemandem weggenommen.** Eine Bildspalte (Minimum = ihre Breite) schnappt
zurück, die Differenz addiert sich zur Tischbreite: „Today's featured
picture" kam auf **1548 statt 1296**, der Text lief 252 px aus seinem Kasten.
Jetzt erst jedes Minimum sichern, dann den Rest im Verhältnis von
`pref - minw` (Chromium: 404 | 892). **„Manchmal" hat einen Grund:** die
Vorlage der Hauptseite wechselt TÄGLICH zwischen Bild oben und Bild daneben,
und nur die zweispaltige Fassung trifft es — der Korpusstand stapelt, deshalb
sah es weder die Galerie noch der erste Lauf.

**0.164.0: der Knopf neben „Appearance", und was er verdeckte.** Florian
schickte einen Screenshot und die Zeile `Befehlsspanne passt nicht zum
Steuerelement`. Zwei Fehler, beide gemessen statt geraten. **(1) Ein `drain`
ist auch eine Umbaustelle.** Der Inhalt eines `<button>` und ein atomarer
Inline-Kasten legen aus und ziehen die Befehle wieder heraus — die
Stapelbereiche nahmen sie nicht mit, also zeigten die auf die NÄCHSTEN Befehle
der Seite. Die Fläche eines Knopfes landete hinter seiner eigenen Beschriftung
(graue Kiste), seine Spanne zerriss, jeder Klick kostete ein Auslegen. Die
Regel stand schon im File — `spec_rollback` sagt sie wörtlich. **(2) Ein
Flex-Item hat seinen eigenen Formatierungskontext** (css-flexbox-1 §4);
isoliert war nur der BEHÄLTER, zwischen den Geschwistern lief die Float-Liste
weiter. Auf der Wikipedia-Hauptseite räumte der Float der linken Spalte den
Clearfix der rechten: `#mp-itn` war **566×696 statt 531×351**. Dafür läuft das
Kastenorakel jetzt auch auf einer echten Seite (`PIN_ALL=1` ist dort Pflicht).
WPT 4545 unverändert.

**0.163.0: Tabs — und der Streifen war wirklich die letzten fünf Prozent.**
`docs/plan/BROWSER_TABS.md` sagt es voraus: erst muss die Seite ein WERT
werden. Die letzten 24 `static mut`, die dem Dokument gehörten, sind in
`struct Doc` gewandert (**47 → 23**, und die 23 sind vier Gruppen mit einem
Grund, der eine zweite Seite überlebt: Abholpuffer · der BILDpuffer · der
LAUF auf dem Stapel · Fenster und Werkzeug; die Tabs legen dann eine zurück,
aus `DOC` wurden `TABS` + `ACTIVE`). Danach war `Vec<Box<Doc>>` + `ACTIVE`
klein. **`Box`, weil `js_session()` ein `&'static mut` INS Dokument
gibt** — ein umziehender Vec liesse es auf alten Speicher zeigen. Gefahren
ist Entwurf (b): **ein lebendiger Motor, der Rest eingefroren** — ein
Hintergrundtab ist Adresse, Verlauf, Rollstand und Titel, sonst nichts, und
beim Zurückwechseln wird neu geholt (`DOC_SLOTS = 3` und der Bildspeicher
machen es billig). Der Preis ist benannt: Skriptzustand überlebt den Wechsel
nicht. **Schritt 2 des Papiers ist übersprungen und das war richtig** — „alle
Tabs lebendig" heisst N Motoren, also mehr Arbeit als (b). Dazu Strg+T/W/1-9
(Strg+Tab gibt es nicht: `Event::Chord` trägt einen Buchstaben aus
`KeyCode::Char`, und `Tab` ist keiner) und `npk_open` → Tab. **Der Kernel
stellte die MITTLERE Maustaste nie zu**, obwohl beide Zeigerwege sie liefern
— ohne sie gäbe es „Link in neuem Tab öffnen" gar nicht. Nebenbefund:
**`ptr::write` lässt den alten Wert NICHT fallen**, und `GEOM` wurde so
geschrieben — 66 KB Kästen je Neuauslegung, nie zurück.

**0.161–0.162: und dann war die Umrechnung selbst dran.** beak legt in ganzen
Zahlen aus, CSS rechnet in Brüchen — jedes `as i32` schnitt ab, und das
ADDIERT sich, weil jeder Kasten auf der Unterkante des vorigen aufsetzt (8 px
bis zum Seitenende). Jetzt wird gerundet, an allen 53 Stellen: **nur die
Hälfte zu ändern ist schlimmer als gar nichts** — `CSS2/floats-019` stellt
`padding-top: 1.1in` gegen `margin: 1.1in`, und ein gerundeter Rand neben
einer abgeschnittenen Polsterung ergibt 106 gegen 105. Dazu: `smaller` und
`larger` sind eine STUFE der Skala (/1,2 und ×1,2), nicht 0,85 und 1,15.
Seitenversatz danach: **1 px** statt 8.

**0.157–0.160: der Randzusammenfall, das UA-Blatt und der Tabellenkasten.**
Drei Funde aus einer Kette. Erst `min-height`: es sperrt den Schlussrand
richtig ein und rechnete ihn trotzdem zur Höhe dazu (650 px statt 100). Dann
die allgemeine Regel darunter — **fällt der Rand durch JEDES Kind, gehört er
an den OBERRAND des Elters** (§8.3.1); weil er erst nach dem Auslegen bekannt
ist und zugleich bestimmt, wo die Kinder stehen (ein durchgefallenes Kind kann
einen Float enthalten), fährt `flow_block_impl` diesen einen Fall ein zweites
Mal, über `spec_rollback`. Dazu: **eine Räumung SETZT die Oberkante** (§9.5.2).
Dann fiel am `<hr>` ein Pixel auf, und es war nicht das `<hr>`: **das halbe
UA-Blatt war Geschmack statt Spezifikation** — alle sechs
Überschriftengrössen und -ränder, `ul`/`ol`, `dd`, `blockquote`, `figure`,
`pre`, `caption`, `address`, `fieldset`/`legend`. Gefunden mit einer neuen
Vorlage, `tools/fixtures/ua.html`: **nackte Elemente, kein einziges Blatt**,
durch `<tools>/gallery/run.py` gegen Chromium — Bootstrap und Tailwind setzen
diese Vorgaben zurück und sagen deshalb NICHTS über sie. Dieselbe Vorlage fand
zuletzt eine **Tabelle, die sich 1886 px breit meldet und 157 px breit malt**
(`record_inspect` bekam den angebotenen Streifen), und eine `<caption>` über
der ganzen Fensterbreite. Erster Lauf 60 von 62 Kästen anders, elf über 64 px;
danach 17 identisch und **keine Abweichung über 16 px**.

**0.141.0: GSUB-Ligaturen.** Eine Symbolschrift bildet ihr Zeichen als
Ligatur — `<i class="fos-icon">home</i>` mass 1 px statt 24, weil die vier
Buchstaben je eine leere Glyphe sind. fontdue *substituiert nicht* (kein
Shaper, sagt es selbst); `src/gsub.rs` liest die Ligaturtabelle über
`ttf-parser`, das ohnehin im Baum liegt. `Fonts::pick` gibt jetzt ein `Face`
mit Schrift UND Ligaturen, und geformt wird an allen vier Textstellen — sonst
misst man das eine und malt das andere. `letter-spacing` unterdrückt
Ligaturen (css-text-3 §8.2). Der allokationsfreie Schnellpfad bleibt für jede
Schrift ohne GSUB, und dass die sechs eingebauten keine haben, ist jetzt ein
Test. Werkzeug: `examples/ligcheck.rs`.

**0.147.0 / Kernel 0.330.0: jeder Tastendruck in der Adresszeile ging an
den DNS.** Florians Log zeigte sechzehn Abfragen, jede einen Buchstaben
kürzer als die davor — kein Netzfehler, sondern jemand, der die Adresse
rückwärts löscht. `URL_BUF` war die Adresse des DOKUMENTS **und** der Inhalt
des Textfelds; `Event::InputChange` rief `set_url`, und das meldet dem Kernel
den Netzkontext, den der SELBST auflöst. Zuerst ein Datenschutzfehler: jedes
Präfix einer Eingabe ging an den Auflöser. Und zweitens ein Loch in der
Reichweiten-Grenze — `ctx.net_reach` ist die Klasse, gegen die JEDE Anfrage
der laufenden Seite geprüft wird, und `192.168.1.1` in die Zeile zu tippen
(ohne Enter) öffnete der offenen öffentlichen Seite das Heimnetz. Die Zeile
hat jetzt ihren eigenen Puffer. Im Resolver dazu: die Fehlermeldung nannte
„5,5 s (4 Versuche)" als KONSTANTE, während die Antwort auf Bein 1 in
Millisekunden kam — und ein „den Namen gibt es nicht" wird jetzt gemerkt,
eine Zeitüberschreitung ausdrücklich NICHT. **0.148.0** räumt die zwei
Meldungen auf, die dabei als Fehler gelesen wurden und keine waren.

**0.144–0.146: die Komponentengalerien, und was sie fand.** Florians
Vorschlag: „tailwind css komplett holen und dann jedes element durchspielen..
gleiche für bootstrap etc." Gebaut als `<tools>/gallery/run.py` — dieselbe
Vorlage durch Chromium UND durch beak, dieselbe Sonde, verglichen werden
KÄSTEN. **Fünf Fehler im ersten Lauf, alle im Flex** und alle zuerst in
`getBoundingClientRect` sichtbar: ein Flexkasten meldete die Breite seines
STREIFENS (1902 statt 400) · eine Flex-Spalte streckte den INHALT statt des
Außenkastens (jedes Kind eines gepolsterten Items 32 px zu breit) · die
Innenbreite eines Steuerelements war doppelt gerahmt (Knopfgruppe 285 statt
205) · Tabellenteile hatten **gar keinen** Kasten · und ein Kasten mit eigenem
Formatierungskontext wurde ZWEIMAL aufgezeichnet — `getBoundingClientRect`
gibt die Vereinigung, und die aus 256 und 1902 ist 1902. Dazu: der freie
Platz einer Tabelle wird jetzt im VERHÄLTNIS der Inhaltsbreiten verteilt, wie
Chromium es tut. Stand: Bootstrap 413 von 415 Kästen (161 identisch),
Tailwind 165 von 165. **Vor dem Benutzen `<tools>/gallery/README.md` lesen** —
ohne `--hide-scrollbars` und gepinnte Schrift misst der Vergleich sich selbst.

**0.144.0: die Formular-Fläche.** Florian: „wir haben mühe mit form sachen..
und was sicher auch noch nicht sauber ist. sind checkboxen." Ausgezählt statt
gestochert: `el.click()` fehlte ganz (mit der Reihenfolge der Spezifikation —
erst umschalten, dann zustellen, bei `preventDefault` zurück), **ein Klick auf
ein `<label>` aktiviert sein Steuerelement** — auch OHNE Skript, denn der
Schnellweg der Shell sprang ab, wenn die Seite keinen Behandler hat —, dazu
`form`/`labels`/`disabled`/`readOnly`/`required`, `label.htmlFor`/`control`,
`indeterminate` und volles `FormData`. Offen und benannt: die
Validierungs-API und `select.add`.

**Das RASTER war der nächste Posten — es ist erledigt, und die Rangliste
darunter war falsch gelesen.** Nachgemessen 2026-09-10: Wikipedias
`main.mw-body` kommt in beak und Chromium auf DIESELBEN Kästen (1220 breit,
Spalte 948 = 59,25 rem). Die genannten WPT-Familien (`column-align-items`,
`row-auto-repeat`, `grid-lanes-subgrid`) sind sämtlich `display: grid-lanes`
— also Vehikel, die `vehicles.py` ausschliesst, und keine Arbeit. Vom Raster
bleiben **13 echte** Tests. Ausgezählt ohne Vehikel:

    22  CSS2/bidi                     8  css-grid/positioned-grid-items
    14  CSS2/margin-collapse          7  css-text/word-space-transform
    12  CSS2/table-anonymous-objects  6  CSS2/abspos · flex-flow · contain-intrinsic-size

**▶ Als nächstes: `CSS2/bidi`** (22 Tests, wir haben keinen Bidi-Algorithmus)
und `table-anonymous-objects` (12, aufaddierte Aufrundung der Spalten). Vom
Randzusammenfall bleiben nach 0.158.0 noch 14 — sie sind einzeln gemessen und
nicht mehr eine Familie mit einer Ursache.

**Immer erst `python3 tests/vehicles.py`** — die Rangliste der rohen Familien
führt sonst zu einem Posten, den es nicht gibt.

Shadow DOM bleibt gemessen KEINE
Web-Anforderung, sondern die Bauweise EINER Seite: 89 % der 906 Aufrufe
kommen von MDN allein, und in allen sechzehn Korpusseiten (jede Google-Seite
eingeschlossen) steht null Shadow DOM. Rangliste:
`docs/plan/WEB_PLATFORM_GAPS.md` §0a. Als *Schnittstelle* gibt es
`ShadowRoot` seit 0.140.0 trotzdem — ein `instanceof` gegen einen fehlenden
Namen wirft, statt `false` zu ergeben, und daran starb htmx.

**0.140.0: XPath, und damit 13 von 13 Bibliotheken grün.** htmx sucht seine
`hx-on:`-Attribute per XPath. Der Zensus zählt über zwölf Zielseiten **null**
XPath-Aufrufe — das entscheidet die GRÖSSE, nicht das Ob: kein Sonderfall für
htmx' einen Ausdruck, aber auch kein volles XPath 1.0 mit Namensräumen.
`js/xpath.rs` ist ein echter Lexer/Parser/Auswerter für die Sprache, die eine
Seite schreibt; was fehlt, steht im Modulkopf.

**0.139.0: `@import` wird geholt.** `sandbox.nopeek.ch` band ein Blatt ein,
das nichts als fünfzehn `@import`-Zeilen enthielt — die ganze Gestaltung kam
nie an (3592 statt 80094 Bytes CSS). Eine vierte Ladestufe, rundenweise; der
Aufwand war die Kaskadenreihenfolge, denn ein Import gehört VOR sein Blatt.
Offen und benannt: die Medienabfrage eines Imports wird übersprungen.

**0.135–0.137: WPT 4485 → 4515 (+30/−1), Formularfamilie 20/21.** Florian
fragte nach den Radioknöpfen — die Antwort lag dreimal woanders als der
Testname sagt. **`centering-00x` scheiterte an der REFERENZ**, nicht an uns:
sie steckt ein `<div>` in einen `<button>`, und wir legten Button-Kinder gar
nicht aus. Jetzt tun wir es, im Formatierungskontext, den der Knopf SELBST
ansagt (Tailwind schreibt `flex` auf fast jeden Symbolknopf).
**`appearance: none` nahm nur die Fläche weg, nicht das Widget** — jetzt auch
Rahmen, Haken, Punkt und `<select>`-Pfeil, und `::before` wird darin
ausgelegt (so ist jede eigene Checkbox im Web gebaut). Dann **drei Prozente
gegen die falsche Achse**: `top`/`bottom` lasen die BREITE, eine Tabellenzelle
war kein bestimmter Umgebungskasten, und **alle vier Polsterungen in Prozent
waren null** — der Grund stand in den Typen, nicht im Layout (`pad_*` ist ein
aufgelöstes `f32`, die Kaskade sieht keinen Umgebungskasten). Nebenbei fand
die Probe drei Dinge, die WPT nie zeigt: ein inline `<svg>` mass sich nicht,
**ein `<img>` als Flex-Item malte NICHTS** (auf tailwind.com das Logo), und
**`margin-top: calc(…)` wurde ganz verworfen** — Tailwind v4 schreibt jedes
`my-*` genau so.

**0.138.0: die Stapelbereiche verschachteln sich.** Das war der größte
Strukturposten, und er war ein Datenstruktur-Problem, kein Regelproblem: ein
Aufklappmenü verschwand UNTER dem Inhalt danach, sobald sein Panel nicht
hinter einer offenen Textzeile stand — also bei fast jedem echten Menü. Fünf
Messungen sagten „Heben hilft nicht", und alle fünf hatten recht: die
Bereichsliste war FLACH, ein `position:relative`-Elter verschluckte die
Bereiche seiner Kinder. `z_order` baut jetzt einen Baum. Damit bekommt jeder
positionierte Kasten seinen Bereich — auch **Tabellenteile**, **positionierte
Floats** und **Flex-/Rasterkinder**, die nie eine Meldestelle erreicht hatten.
Preis: `clip_overflow` durfte nicht mehr aussteigen (467 Befehle entkamen
ihren `overflow:hidden`-Kästen), also schreibt `clip_ops` jetzt zurück, wohin
jeder Befehl gewandert ist. Nebenbei zwei Prozentbreiten, die sich zweimal
auflösten: `float:left; width:50%` kam als VIERTEL heraus, und absolut
gesetzt kam `width:18%` als 18 % von 18 %. Details:
`docs/spec/CONFORMANCE.md`.

**0.134.0: die Rollmasse antworteten 0, und das Layout hatte die Zahlen
nicht.** 582 Aufrufe fragen `scrollHeight` & Co., 78 `offsetParent`. Das
Papier nannte es „eine Leitung" — halb richtig: die Polsterung fuhr im
Layoutkasten gar nicht mit, „positioniert" auch nicht, und die Rollfläche
des Dokuments stand nirgends (jetzt `Geometry::content` aus
`Layout::height`). **`scrollTop` bleibt an gewöhnlichen Elementen 0, und das
ist wahr**: beak klemmt `overflow` nicht ab. Nebenbefund und der größere:
**`window.scrollY` stand fest auf 0** — `set_viewport` legte es einmal als
Zahl ab und niemand zog es je nach. Dazu ~400 Aufrufe Kleinkram
(`toggleAttribute`, `isConnected`, `attributes`/`Attr`/`NamedNodeMap`,
ARIA-Spiegelung, `replaceChildren`, Element-Geschwister). **DOM-Deckung
98,9 → 99,4 %; die Platzhalterliste in `tests/apigap.rs` ist leer.**

**0.133.0: `IntersectionObserver` + `ResizeObserver`** — ein Commit, weil
beide an derselben Sache hängen: der Geometrie nach dem Layout, nicht an
einem Zeitgeber. Gemessen in `set_geometry`, zugestellt am
Microtask-Kontrollpunkt. **Der Wirt muss FRAGEN** (`box_observations_pending`
nach jedem Bild) — sonst hätte eine Seite ohne Zeitgeber und ohne Ereignisse
ihre Beobachter angemeldet und nie einen Rückruf gesehen.

**0.132.0 / Kernel 0.329.0: `crypto.getRandomValues` mit echtem Zufall** —
`npk_random_bytes` aus `security::csprng`, ohne Kapabilität wie
`npk_unix_time`, in BEIDEN Wegen registriert. **`crypto` erscheint nur, wenn
eine echte Quelle da ist.**

**google.ch: der Weg ist fertig, die Tür ist zu.** beak fährt die ganze
Kette — Startseite, Formular, Einwilligung, `/save`, Botguard, Token — und
bekommt `/sorry` + 429. Nachgemessen: weder die IP noch die Kennung; Google
bewertet den Token. Die Wand aus `memory/feedback_no_ua_impersonation.md`.

**Kernel 0.328.0: was `memory.grow` dazulegte, kam nie zurück.**
`Memory::drop` gab die STARTgrösse frei; 59 MB gingen je beak-Lauf verloren.
**Die Zahl in der Panik WAR die Startgrösse** — der Lauf war unschuldig.

**Älteres, je ein Satz** (Details: `git log`, `memory/MEMORY.md`): 0.129.0
Schriften faul geladen, srf 89 → 44 MiB · 0.128.0 der Clearfix mass null,
WPT 4481 → 4485 · 0.127.0 sechs Sprachlücken aus d3/chart.js/Vue, test262
81,27 → 81,76 % · 0.126.0 **eine freigegebene Adresse gab ihren Rumpf an die
nächste Funktion** (`Rc::as_ptr` als Schlüssel), gefunden am neuen Prüfstand
`<tools>/libprobe/` (12 von 13 grün, offen nur htmx) · 0.125.0 `location`
war ein Datenobjekt — es gab gar keine Navigation per Skript.

**Die Zahlen, und sie messen NICHT dasselbe:**

    test262 exec    82,74 %   (V8 auf demselben Korpus: 99,41 %)
                              0.166 fiel die Zahl auf 74,54 %, weil 5485
                              async-Tests endlich im Nenner stehen
    test262 parse   96,87 %
    DOM-Aufrufe     99,4 % gedeckt  (`tests/apigap.rs`, Chromium-Zensus)
    WPT (CSS)       4547/5180 = 87,8 % ohne Testvehikel (roh 80,5 %)
    Bibliotheken    13 von 13 (`<tools>/libprobe/`)
    beak:selftest   Sprache 62/62, Dokument 39/39 (+ async + Kasten)
    Kastengeometrie Bootstrap 413/415 (170 identisch) · Tailwind 165/165
                    · ua.html 62/62 · controls.html 49/49 (47 byte-gleich)
                    (`<tools>/gallery/`, die dritte Vorlage ist NACKT)
    Halde           Schriften faul; srf 44 MiB (war 89)

Das eigene Testziel ist **`beak:selftest`** — eine Prüfseite aus dem
Binärbild, die nichts holt und ihr Ergebnis auf dem Schirm UND im Log sagt.
Sie läuft auch host-seitig (`beak-engine/examples/selftest.rs`).

**Das Werkzeug für Seiten ist `beak-engine/examples/pagerun.rs`** — es fährt
die ganze Skriptrunde host-seitig, in EINER Sitzung, mit Modulgraph. `DUMP=1`
zeigt den Baum danach, `NAVIGATION` die verlangte Adresse, `NOVM=1` fährt den
Baumläufer statt der Befehlsmaschine (die Gegenprobe, die den Chunk-Fehler
gefunden hat).

**Die gemessene WPT-Zahl steht in `docs/spec/CONFORMANCE.md` und nirgends
sonst** — zwei Nenner, und der zweite wird mit
`tools/wasm/beak-engine/tests/vehicles.py` aus der gesegneten Baseline
HERGELEITET, nie weitergetragen. **Vor jeder WPT-Planung dieses Werkzeug
laufen lassen:** 347 der 1166 Fehler sind `display: grid-lanes`, und kein
Dateiname sagt es.

**WLAN (AX200)**: ⏸ pausiert, die Verbindung läuft (Download 116 Mbit auf
HT40, Upload möglich). Das Intent **`wlan`** ist das Werkzeug dafür. Beim
Wiedereinstieg NUR den obersten Abschnitt von
`memory/project_wifi_stability_handover.md` lesen.

Alles darunter — Kernel, npkFS, Netz, Compositor, Panels, Apps, MicroVM —
ist gebaut und in Betrieb. Überblick: `README.md`.

Wo der Stand wirklich steht:

- `memory/MEMORY.md` — Index auf die Themen-Files, wird laufend gepflegt
- `git log` — die harte Wahrheit
- `docs/spec/` — lebende Verträge · `docs/plan/` — offene Papiere ·
  `docs/archive/` — erledigt/überholt

> Dieser Abschnitt bleibt **kurz**. Session-Verlauf gehört ins Memory, nicht
> hierher; der alte Verlauf liegt in `docs/archive/CHANGELOG_2026.md`.

## Commit-Message Convention (since v0.54.x)

First line encodes which OTA path the change needs, so users know
whether a `update` is enough or modules must be `install`-ed too:

- `kernel-only:` — `update` suffices, no module rebuild
- `module <name>:` — only `install <name>` required
- `abi+kernel:` — kernel + all SDK-using apps, coordinated release
- `kernel+module <name>:` — both, because they belong together
- **Known bug:** `run wifi` on worker core crashes; `driver wifi` on Core 0 works
  (MMIO `map_page` conflict with 1GB huge pages).

## Release-Flow Plumbing (mandatory)

`./build.sh release` regenerates `release/kernel.bin` + `release/manifest`
+ all `release/modules/*.sig` with the ECDSA P-384 update key. Skipping
this step means OTA users keep getting the LAST signed release — every
`update` is a silent downgrade to whatever was last in `release/`.
Bitter lesson from v0.85.0–0.85.5: pushed source, forgot release-build,
user's `update` rolled back to v0.84.3 every time → consistent
"wrong passphrase" lockout because v0.84.3 ChaCha20 couldn't decrypt
v0.85.x AES-GCM keycheck.

Sequence for any kernel/module change:

```
# bump the version, then sync the lock — builds run --locked, so a stale
# Cargo.lock aborts the release instead of silently re-resolving:
cargo update --offline -p nopeekos-kernel

./build.sh build      # verify it builds
git commit -m "..."   # source change
./build.sh release    # target/ → signed release/
git add release/ && git commit -m "release: sign + publish vX.Y.Z"
git push
```

`release` does NOT compile WASM. A changed module must be built and staged
first (`tools/stage-module.sh <mod>`, which also writes `.version`);
`aml` and `wifid` live one level deeper than the script expects and are
staged by hand.

USB reinstall pulls `target/` directly and bypasses this — that's why
USB-installed builds appeared to work while OTA kept downgrading.

## Security Checkpoint

Before every commit:
"Can a WASM module escape its sandbox through this change?"
If the answer isn't clearly "No" → don't commit.
