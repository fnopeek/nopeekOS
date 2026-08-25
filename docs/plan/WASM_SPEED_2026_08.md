# Was der Interpreter kostet — und was ein Compiler brächte

Stand 2026-08-25, beak 0.40.0, Notebook. Entscheidungspapier zu der Frage:
lohnt es sich, WASM zu übersetzen statt zu interpretieren — und wenn ja, wie
gross wäre das Ding, das man dafür bauen muss.

Jede Zahl hier ist gemessen. Die Methode steht dabei, weil sie den Unterschied
zwischen einer Zahl und einer Behauptung ausmacht.

## Die Konstante: 435 M Instruktionen/s

`beakbench` fährt die Engine unter **derselben** wasmi wie der Kernel —
geprüft, nicht angenommen:

| | Kernel | beakbench |
|---|---|---|
| wasmi | 1.1.0 | 1.1.0 |
| `default-features` | aus | aus |
| features | `prefer-btree-collections` | `prefer-btree-collections` |
| `consume_fuel` | an | an |

Fuel ist eine exakte Instruktionszahl. Geteilt durch die Millisekunden vom
Gerät ergibt das den Durchsatz. Damit die Division zulässig ist, müssen beide
Seiten **dieselben Bytes** sehen — deshalb wurde der Inhalt vorher verglichen:

| Seite | Fuel (warm) | Gerät | Inhaltsabweichung | M instr/s |
|---|---:|---:|---|---:|
| Stansstad | 413 M | 930 ms | **byte-identisch** | **444** |
| srf, Lauf 2 | 723 M | 1689 ms | 12 B von 339 708 | **428** |
| srf, Lauf 1 | 723 M | 1935 ms | 1054 B | 374 |

**Das Notebook fährt wasmi mit rund 435 M Instruktionen/s.** Dieser
Entwicklungsrechner schafft 1183 M/s, ist also 2,7× schneller.

Die 15 % Streuung zwischen zwei Läufen derselben Seite kurz nacheinander sind
echt (Takt/Temperatur) und der Grund, warum hier eine Spanne steht.

**Nur die WARME Paarung gilt.** Kalt weichen die Verhältnisse ab —
beakbench 775/413 = 1,88 gegen Gerät 1320/930 = 1,42 — weil das Gerät bei
`relayout: navigation` längst geparst und gecacht hat, während beakbenchs
`run 0` innerhalb von `layout_ext` wirklich kalt parst. Warm ist beides
dasselbe: alle Caches heiss, reines Box-Layout.

### Gegenprobe

Aus 435 M/s folgt für Stansstad warm 949 ms. Gemessen wurden 930 ms — und
unabhängig davon steht in `memory/project_beak_pointer_and_repaint.md` seit
der 0.28er-Runde 1060–1110 ms für Wikipedia am Gerät. Die Kette stimmt.

## Wie viel des Browsers überhaupt im Modul steckt

`beak-engine` hat **kein einziges `extern "C"` und kein
`wasm_import_module`** — während `parse`, `cascade`, `layout` und `paint`
laufen, verlässt die Ausführung das Modul nicht ein einziges Mal. Damit ist
die Aufteilung direkt aus beaks eigenem Log ablesbar (srf, Lauf 1):

| | ms | Anteil |
|---|---:|---:|
| reine Engine (interpretiert, null Host-Aufrufe) | 3030 | **80,2 %** |
| Netz (Kernel, nativ) | 580 | 15,3 % |
| Host-Aufrufe, Compositor, Aufbau | 170 | 4,5 % |

## Der Interpreter-Aufschlag: 37×

Dieselbe Engine, dieselbe Seite, dieselbe Breite, auf demselben Rechner:

| | stansstad @1400, parse+cascade+layout |
|---|---:|
| nativ (release + LTO) | **17,0 ms** |
| unter wasmi | **636 ms** |
| | **37×** |

Wichtig für die Diagnose: **das ist der Preis des Interpretierens, nicht der
von WASM.** Übersetzt man dieselbe `.wasm` nach x86-64, bleibt als
eingebauter Aufschlag nur die Schrankenprüfung auf den linearen Speicher —
und die ist auf x86-64 mit Guard-Pages fast gratis. Wir halten die
Seitentabellen selbst, das ist für uns leichter als für jede gehostete
Runtime.

## Wie gross wäre der Compiler?

`tools/wasmscan` (liegt neben dem Repo) liest `beak.wasm` — 1111 Funktionen,
458 718 Instruktionen — und zählt aus:

| | |
|---|---|
| **verschiedene Opcodes insgesamt** | **146** |
| davon reines WASM-MVP | **99,6 %** |
| `bulk_memory` (`memory.copy`/`fill`) | 0,2 % |
| `saturating_float_to_int`, `sign_extension` | 0,1 % |
| SIMD, Threads, Exceptions, GC, Reference Types | **null** |
| **60 Opcodes decken** | **98,78 %** aller Instruktionen |
| Host-Fläche des ganzen Browsers | **17 Importe** |

Die Spitze ist langweilig, und das ist die gute Nachricht: `LocalGet` 28 %,
`I32Const` 14 %, `LocalSet` 6 %, `I32Add` 6 %, `End` 5 %, `LocalTee` 4,5 %,
`BrIf` 4,3 %. Ein Registermaschinen-Zielbild ohne Überraschungen.

## Was es brächte — gemessen, nicht geschätzt (2026-08-25, abends)

Hier stand eine Literaturschätzung („3–10× über einem Interpreter"). Sie ist
durch eine Messung ersetzt.

**Die Methode.** wasmtime bringt zwei Compiler mit: **Winch** ist einstufig und
optimiert nicht — genau die Bauart, die wir selbst bauen würden. **Cranelift**
optimiert und ist die Obergrenze, nicht das Ziel. `tools/aotbench/runner`
(neben dem Repo) lädt **dieselbe `beakbench.wasm`**, die die Fuel-Zahlen oben
geliefert hat, und fährt sie in EINEM Prozess unter der Kernel-wasmi und unter
beiden Compilern, mit derselben Phasenfolge und denselben zwei
Host-Funktionen. Gegenprobe, dass wirklich dasselbe gemessen wird: das warme
Layout von srf verbrennt unter allen Läufen **723 M Fuel** — die Zahl aus der
Tabelle ganz oben.

Warmes Layout, Entwicklungsrechner (Ryzen 5 9600X), Median aus 3:

| Seite | wasmi | winch | winch +Fuel | cranelift | clif +Fuel |
|---|---:|---:|---:|---:|---:|
| Stansstad @1400 | 340,9 ms | 35,8 ms | 62,7 ms | 23,0 ms | 27,6 ms |
| Main Page @1400 | 615,1 ms | 61,3 ms | 106,4 ms | 39,4 ms | 47,6 ms |
| srf @1902 | 598,5 ms | 60,4 ms | 112,7 ms | 35,3 ms | 43,7 ms |
| **gegen wasmi** | 1,00× | **9,5–10,0×** | 5,3–5,8× | 14,8–16,9× | 12,3–13,7× |

**Ein einstufiger Compiler holt 9,5–10× — das obere Ende der geschätzten
Spanne, nicht die Mitte.** Und Cranelift legt danach nur noch 1,7× drauf: die
Optimierung ist NICHT, wo das Geld liegt. Ein simpler Einpass-Compiler holt
rund 85 % dessen, was überhaupt zu holen ist.

Dasselbe an `python.wasm`, damit die Zahl nicht an einer Kostenklasse hängt
(CPython ist selbst eine Dispatch-Schleife, beak sind enge Rust-Schleifen —
`memory/project_python_ecosystem.md` warnt ausdrücklich davor, das eine aufs
andere zu übertragen). Wanduhr des ganzen Prozesses, beide Compiler
**vorübersetzt** als `.cwasm`, weil unser Entwurf beim `install` übersetzt:

| Last | nativ 3.13 | wasmi | winch | cranelift |
|---|---:|---:|---:|---:|
| `pass` | 25 ms | 189 ms | 16 ms | 11 ms |
| `import json, re, os` | 24 ms | 289 ms | 25 ms | 17 ms |
| Schleife 1e6 | 62 ms | 2074 ms | 199 ms | 113 ms |
| `''.join` 200k | 32 ms | 635 ms | 60 ms | 35 ms |
| json round-trip 20k | 33 ms | 600 ms | 56 ms | 33 ms |
| **gegen wasmi** | | 1,00× | **10,4–11,8×** | 15,5–18,2× |

Zwei Kostenklassen, dieselbe Antwort. Nebenbei: **unter einem Compiler startet
unser wasm-CPython schneller als das native `python3` des Systems** (16 gegen
25 ms) — die gefrorene stdlib im Zip schlägt hunderte Einzeldateien von Platte.

## Fuel: der Preis steht nicht in der Spezifikation, sondern in der Umsetzung

Das Papier führte „Fuel fällt weg" als Posten 2 der Kosten. Auch das ist jetzt
gemessen, und es fällt nicht weg.

**In unserer wasmi kostet der Zähler nichts.** Schleife 1e6, Fuel an: 2036 ms,
Fuel aus: 2043 ms. Rauschen. Die 10× oben sind also kein Fuel-Artefakt.

Im übersetzten Code kostet er sehr wohl — aber sehr unterschiedlich:

| | ohne Fuel | mit Fuel | Aufschlag |
|---|---:|---:|---:|
| winch, srf-Layout | 60,4 ms | 112,7 ms | **+87 %** |
| cranelift, srf-Layout | 35,3 ms | 43,7 ms | **+24 %** |
| winch, python (3 Lasten) | | | +113…160 % |
| cranelift, python (3 Lasten) | | | +38…46 % |

Der Grund steht in Winchs eigenem Quelltext (`winch-codegen/src/codegen/mod.rs`,
`fuel_before_visit_op`), er ist nicht erschlossen:

> *„Winch does not utilize a local-based cache to track fuel consumption.
> Instead, each increase in fuel necessitates loading from and storing to
> memory. […] One potential optimization is to designate a register as
> non-allocatable, when fuel consumption is enabled, effectively using it as a
> local fuel cache."*

Winch sammelt Fuel bereits **pro Basisblock** — es schreibt den Zähler nur bei
jedem Kontrollflusswechsel durch den Speicher. Cranelift hält ihn in einem
Register. Das ist der ganze Unterschied zwischen +87 % und +24 %.

**Damit fällt Posten 2 der Kostenliste.** Fuel bleibt als Ressourcengrenze
erhalten, wenn der Compiler den Zähler in einem festgenagelten Register hält
und pro Basisblock einmal abzieht — und den Registerallokator schreiben wir
selbst. Timer-Preemption für kooperative Fibers ist damit **keine
Voraussetzung** mehr, sondern eine spätere Option.

## Was das am Gerät heisst

Der Entwicklungsrechner fährt 723 M / 598,5 ms = **1208 M Instr/s**, das
Notebook 435 — Faktor 2,78. Damit rechnen sich die Messungen oben um.
Die letzte Spalte ist eine **Projektion**, keine Messung: Winchs Zeit ohne Fuel
plus Cranelifts Fuel-Aufschlag von 24 %, also das, was ein Einpass-Compiler mit
registergehaltenem Zähler kosten sollte.

| srf am Notebook | heute | winch-Klasse | + Fuel im Register (proj.) |
|---|---:|---:|---:|
| ein warmes Layout | 1662 ms | 168 ms | **~208 ms** |
| die 11 Relayouts | **18,3 s** | 1,8 s | **~2,3 s** |
| `init`, 6 Schriftschnitte | 1321 ms | 172 ms | ~213 ms |

Und die beiden Hebel multiplizieren sich: mit dem behobenen Relayout-Sturm
(1 Layout statt 11) plus Compiler stehen dort **~0,2 s** statt 18,3 s.

Für `python` am Notebook, mit demselben Faktor: Start `pass` von ~525 ms auf
**~55 ms**, die 1e6-Schleife von ~5,8 s auf **~0,7 s**.

## Was der Compiler zusätzlich kostet

Zwei Posten, die die Messung neu aufwirft:

- **Übersetzungszeit beim `install`.** Winch braucht für 7,4 MB `python.wasm`
  **0,14 s**, für 5,6 MB `beakbench.wasm` 49 ms; Cranelift 0,45 s bzw. 485 ms
  (auf 12 Kernen, Cranelift parallelisiert). Ein Einpass-Compiler ist am Gerät
  also im Bereich einer halben Sekunde — beim `install`, nicht beim Start.
- **Codegrösse.** Winch macht aus 7,44 MB wasm **23,4 MB** nativ (3,1×),
  Cranelift 15,9 MB. Das ist mehr als `MAX_MODULE_SIZE` (16 MB) und muss beim
  Ablegen in npkFS eingeplant werden — der Deckel gilt für das `.wasm`, der
  Codeblob ist ein zweites Objekt.

## Was es kostet

Drei Posten, die in der Begeisterung untergehen:

1. **Der Kernel enthielte einen Codegenerator.** W^X-Verwaltung, und eine
   neue Angriffsfläche im vertrauenswürdigsten Teil des Systems.
2. ~~**Fuel als Ressourcengrenze fällt weg.**~~ **Erledigt durch die Messung
   oben.** Übersetzter Code kann mitzählen: Zähler in einem festgenagelten
   Register, einmal pro Basisblock abziehen, prüfen nur an Rücksprüngen und
   Aufrufen. Cranelift zahlt dafür 24 %. Timer-Preemption bleibt eine Option,
   ist aber keine Voraussetzung mehr.
3. **Was die Sandbox liefert, ersetzt Rusts Speichersicherheit nur zum
   Teil.** Sie gibt vier Dinge; Rust ersetzt eines davon, und auch das nur,
   wenn wir aus QUELLE übersetzen und `unsafe` verbieten:

   | | ersetzt Rust das? |
   |---|---|
   | Speichersicherheit im Modul | ja, unter den zwei Bedingungen |
   | Confinement (das Modul erreicht nur Übergebenes) | nur aus Quelle — ein fertiges Binary führt beliebigen Code aus |
   | Ressourcengrenzen | nein |
   | Fehlerisolation (Panik ≠ Kernel-Panik) | nein |

   Punkt 2 kollidiert mit Architekturprinzip 4 („runtime-generated"): ein zur
   Laufzeit erzeugtes Werkzeug ist per Definition nicht vorab vertrauenswürdig.

**Der entscheidende Punkt:** ein AOT-Compiler für die `.wasm`, die wir schon
ausliefern, ändert an keinem dieser vier Posten etwas. Format, Signatur, OTA,
Capability-Gating bleiben, wie sie sind. Das ist ein ganz anderer Umbau als
„native Module statt WASM".

**Was übrig bleibt, ist Posten 1, und der ist echt.** Heute ist die Isolation
vollständig die des Interpreters: alles läuft in Ring 0, identisch abgebildet,
64 GB über 1-GB-Seiten. Übersetzter Code ist nur so eingesperrt, wie unser
eigener Codegenerator ihn einsperrt. Zwei Dinge folgen daraus, und beide sind
Teil des Bauplans, nicht Zubehör:

- **Speicherzugriffe brauchen keine Prüfbefehle, aber sie brauchen Wachseiten.**
  Die lineare Speicherung bekommt eine eigene virtuelle Region oberhalb der
  64 GB, 8 GB Adressraum je Instanz, nichts davon abgebildet ausser den
  tatsächlichen Seiten. Ein i32-Index kann dann nicht hinausreichen, und der
  Seitenfehler-Behandler muss den Treffer als Modul-Trap erkennen statt als
  Kernel-Panik.
- **Validieren ist Pflicht, nicht Kür.** `intent_run` lädt cwd-relativ aus
  npkFS (`kernel/src/intent/wasm.rs`); ungeprüftes WASM kann die Engine
  erreichen, Signatur hin oder her. Die Prüfung fällt aber in denselben
  Durchlauf wie die Codeerzeugung — der abstrakte Typstapel wird für die
  Registerzuteilung ohnehin geführt.

## Stand der Technik

Sprachbasierte Isolation statt Hardware ist kein neuer Einfall:
**Singularity** (Microsoft Research, 2005, Software-Isolated Processes,
alles aus verifizierter IL vom vertrauenswürdigen Compiler, IPC ~100×
billiger), **Theseus** (2020, Rust, ein Adressraum, „cells"), **RedLeaf**
(OSDI 2020, Rust, Domänen über Ownership). Alle drei geben viel Mühe an
genau den Posten, der oben als 4. steht: eine Panik im Modul einfangen und
seine Ressourcen zurückholen.

## Empfehlung

Die Reihenfolge unten war die vom Nachmittag. Nach der Messung steht der
Compiler nicht mehr hinter dem Relayout-Sturm an — beide Hebel multiplizieren
sich, und der Compiler wirkt auf **jedes** Modul, nicht auf beak allein.
Entscheidung vom Abend: **Compiler zuerst, `python.wasm` als erstes
Umbauprojekt** (42 Importe, reine Rechenlast, gemessene Ausgangswerte).

1. **Der Compiler**, als AOT-Schritt beim `install`, auf der `.wasm`, die wir
   sowieso signieren. Nicht als neues Modulformat.
2. **Fuel im Register**, von Anfang an mitgebaut — nicht nachgerüstet. Winchs
   +87 % gegen Cranelifts +24 % ist der Unterschied zwischen 5× und 10×.
3. **Der Relayout-Sturm** bleibt fällig und wird nicht billiger: 11 Layouts à
   208 ms sind auch nach dem Compiler noch 2,3 s, und der Fix macht daraus
   ein einziges. `memory/project_beak_relayout_storm.md` hat die Ursache.

## Der Zielbefehlssatz — ausgezählt über ALLE Module

`tools/wasmscan` über alle 21 ausgelieferten `.wasm` in `release/modules/`,
Vereinigungsmenge (nicht ein Stichprobenmodul):

| | |
|---|---|
| **verschiedene Opcodes** | **164** |
| SIMD, Atomics/Threads, Reference Types, Exceptions, `MemoryInit`/`DataDrop`, Table-Ops | **null** |
| über MVP hinaus | nur `MemoryCopy`/`Fill`, `sign_extension`, `TruncSat` |
| 30 Opcodes decken | 95,94 % aller Instruktionen |
| 60 Opcodes decken | 99,18 % |

`python.wasm` bringt gegenüber `beak.wasm` **genau einen** zusätzlichen
Opcode. Der Schwanz ab Rang 81 ist Fliesskomma-Kleinkram (`F64Ceil`,
`I64Rotr`, `F32Copysign` …), je 2–4 x86-Befehle. Die Spitze ist langweilig:
`LocalGet` 23,4 %, `I32Const` 17,0 %, `I32Add` 7,5 %, `BrIf` 5,8 %.

## Der Bauplan, und was Stufe 1 schon ergeben hat

`forge/core` (no_std + alloc) und `forge/harness` (Host-Prüfstand), nach dem
Muster von `tools/wasm/aml/{core,harness}`.

**Decoder und Validator schreiben wir nicht.** `wasmparser` ist `#![no_std]`,
liegt über wasmi **schon im Kernelbaum** (`bitflags` als einzige Abhängigkeit),
und wasmi aktiviert dort bereits `validate` — ohne `std`, ohne `serde`, ohne
hashbrown. Uns bleibt die Codeerzeugung. Bei ungeprüftem Eingang ist das nicht
nur Ersparnis: einen WASM-Validator selbst zu schreiben wäre der Teil, den man
am wenigsten selbst erfinden will.

Daraus folgt eine Eigenschaft, die den Entwurf trägt: **die Feature-Menge ist
der Vertrag zwischen Validator und Codegenerator.** `forge_core::features()` ist
`WASM1 + bulk_memory + sign_extension + saturating_float_to_int` und sonst
nichts — kein SIMD, keine Threads, keine Reference Types, kein Multi-Value.
Was den Validator passiert, kann der Generator per Konstruktion erzeugen; ein
Modul mit einem Opcode, den wir nicht können, wird abgelehnt statt falsch
übersetzt. Eine Erweiterung ist dann eine Entscheidung, kein Versehen.

Eine Ausnahme musste sein, und sie ist reine Kodierung: **`CALL_INDIRECT_-
OVERLONG`**. LLVM schreibt den Tabellen-Immediate von `call_indirect` als
überlanges LEB, was vor Reference Types unzulässig war. Ohne diese Erlaubnis
scheitern **13 der 21 Module** — genau die, die überhaupt ein `call_indirect`
haben. Sie lässt keinen Opcode und keine Semantik zusätzlich zu.

**Stufe 1 steht und ist gegengeprüft.** Alle 21 Module in `release/modules/`
validieren, null Ablehnungen, und die Instruktionszahlen decken sich mit denen
von `wasmscan` (beak 458 718, python 1 882 853) — zwei unabhängige Decoder,
dieselbe Zahl. Zwei Zahlen daraus gehen direkt in den Entwurf:

| | |
|---|---|
| **tiefster Operandenstapel, über ALLE Module** | **26** (python; beak 20) |
| **Instruktionen je Basisblock** | **5,8** im Mittel |

Der Operandenstapel ist winzig — er passt in Register, die Rahmen bleiben
klein, und ein Spill-Bereich ist die Ausnahme statt die Regel. Und 5,8
Instruktionen je Block heisst: ein `sub rFuel, imm` je 5,8 wasm-Instruktionen,
also rund +17 % Befehle, aber **null Speicherverkehr** — genau die Stelle, an
der Winch +87 % zahlt und Cranelift +24 %.

Die Registerbelegung, die daraus folgt (wir haben keinen fremden ABI zu
bedienen): ein festgenageltes Register für den vmctx-Zeiger, eins für den
Fuel-Rest, eins für die Basis der linearen Speicherung — `I32Load` allein ist
4,8 % aller Instruktionen, eine gesparte Ladung je Zugriff zählt. Bleiben 11
frei zuteilbare GPR, mehr als genug für einen Stapel der Tiefe 26.

### Welches Modul zuerst — ausgezählt, nicht abgewogen

Eine Funktion ist übersetzbar, wenn **jeder** ihrer Opcodes erzeugt werden
kann; Teilkredit gibt es nicht. `forge_harness --roadmap` beantwortet damit die
eigentliche Frage — nicht „wie viele Opcodes sind fertig", sondern „wie viele
FUNKTIONEN schaltet der nächste frei" — und die Antwort ist die Arbeitsfolge.

| Modul | Funktionen | Importe | voll übersetzbar ab | Orakel |
|---|---:|---:|---:|---|
| **wallpaper** | 51 | 4 | **68 Opcodes** | Pixelpuffer an `npk_set_wallpaper`, byte-identisch |
| **iris** | 80 | 14 | **80 Opcodes** | Canvas, byte-identisch; PNG-Inflate als Last |
| beakbench | 945 | 1 | 144 Opcodes | exakte Zahlen, Messreihe steht schon |
| **python** | 9939 | 42 | **147 Opcodes** | Prüfstand `../tools/pywasi` fertig, 42 wasi-Fn |

Die Modulwahl ändert an der Compilerarbeit fast nichts — beak braucht 146,
python 147, die Vereinigung 164. Sie entscheidet nur, **wie früh man testen
kann**. `wallpaper` läuft bei 68 von 164, also nach etwa der halben Arbeit;
`python` verlangt praktisch alles.

Die python-Kurve hat zwei Klippen, und sie diktieren die Reihenfolge:

```
nach 59 Opcodes:        14,5 % der Funktionen — aber nur  1,0 % der Instruktionen
Opcode 60 = BrIf:       +1382 Funktionen              →   5,4 %
Opcode 81 = GlobalSet:  +3510 Funktionen              →  88,3 %   <- __stack_pointer
```

Die ersten 59 Opcodes kaufen **ein Prozent**. Ohne Kontrollfluss (`BrIf`, `Br`,
`Loop`, `Return`, `BrTable`) und ohne `GlobalSet` — CPythons Stapelzeiger, den
fast jede Nicht-Blatt-Funktion schreibt — ist jede Teilabdeckung wertlos. Nach
der Häufigkeitsliste zu bauen wäre genau falsch.

Daraus die Rollenteilung: **`python` und `beakbench` sind die Messlatte ab
sofort** (beide Prüfstände stehen, Ausgangswerte gemessen, null Stub-Arbeit),
**`wallpaper` ist das Hochzieh-Ziel**, `iris` die zweite Stufe für zwölf
Opcodes mehr.

### forge, gemessen: python 9,2–14,3×, beak 5,4–8,1×

Nicht mehr Abdeckung, sondern Tempo. `forge_harness --run` fährt
`beakbench.wasm` unter beiden Motoren in einem Prozess; `--python` fährt
`python.wasm` unter beiden in je einem Kindprozess, weil ein wasi-Programm
über `proc_exit` hinausspringt statt zurückzukehren. Beide Seiten benutzen
**dieselbe** wasi-Implementierung — zwei Host-Schichten zu vergleichen würde
die Host-Schichten messen. Übersetzen und Instanziieren liegen bei beiden
ausserhalb der Messung, weil AOT beim `install` passiert.

**python** (CPython 3.13, Ausgabe UND Status byte-gleich):

| Last | wasmi | forge | Faktor |
|---|---:|---:|---:|
| `pass` | 184,3 ms | 12,9 ms | **14,31×** |
| `import json, re, os` | 232,2 ms | 22,1 ms | **10,52×** |
| Schleife 1e6 | 1854,4 ms | 163,5 ms | **11,34×** |
| `''.join` 200k | 571,5 ms | 57,1 ms | **10,01×** |
| json round-trip 20k | 401,2 ms | 43,5 ms | **9,22×** |

**Das ist auf oder über Winch-Niveau** (10,4–11,8× auf denselben Lasten).

**beak** (Ergebnisse byte-gleich, Fuel 723 152 293 auf srf — dieselbe Zahl wie
in der Ausgangsmessung, also dieselbe Arbeit):

| | wasmi | forge | Faktor |
|---|---:|---:|---:|
| `parse` | 36,5 ms | 4,6 ms | **7,97×** |
| `cascade` | 233,8 ms | 28,8 ms | **8,12×** |
| `init` (6 Schriftschnitte) | 443,0 ms | 69,7 ms | 6,35× |
| `layout` warm, srf @1902 | 599,1 ms | 104,4 ms | **5,74×** |

**Codegrösse 7,8 Byte je wasm-Instruktion** — unter Winchs 8,56 und über
Cranelifts 4,87.

### Warum `layout` zurückbleibt

Innerhalb von beak fällt die Zahl mit der Speicherlast: `parse`/`cascade`
8,0–8,1×, `init` 6,3×, `layout` 5,7×. Die naheliegende Lesart — **es ist die
Last, nicht der Compiler**: `layout` ist die Phase mit dem meisten
Speicherverkehr und den meisten Allokationen, also war von vornherein ein
kleinerer Teil ihrer Zeit Interpreter-Dispatch. Wo nichts zu interpretieren
war, kann ein Compiler nichts einsparen. Dass python — eine reine
Dispatch-Schleife — bei 9–14× liegt, passt dazu.

Das ist eine **Schlussfolgerung, keine Messung**. Entscheiden liesse sie sich
mit `beakbench`s Allokationszählern oder einem Fuel-Profil der Layout-Phase.

### Was der Registercache gebracht hat

| | vorher | nachher |
|---|---:|---:|
| python `pass` | 7,89× | **14,31×** |
| python Schleife 1e6 | 5,32× | **11,34×** |
| beak `parse` | 4,32× | **7,97×** |
| beak `layout` warm | 3,67× | **5,74×** |
| Byte je wasm-Instruktion | 14,8 | **7,8** |

In dieser Reihenfolge gebaut, jeder Schritt einzeln gemessen (beak `layout`):

1. **Cache-Register getrennt vom Operator-Scratch** → 4,77×. Der Wertestapel
   benutzt `rsi/rdi/r8/r9/r10`, die Operatoren `rax/rcx/rdx/r11`. Weil sich
   die beiden Mengen nicht schneiden, musste **kein einziger Operator** etwas
   von Registerzuteilung lernen. Der Preis ist ein Zug rein und raus — und
   der wird von der Rename-Stufe meist ganz wegoptimiert, ein Store gefolgt
   von einem Load nicht.
2. **Locals und Konstanten faul** → 5,61×. `local.get` ist 23,4 % aller
   Instruktionen, `i32.const` 17 % — zwei Fünftel aller Pushes. Beide
   erzeugen jetzt **gar nichts**, bis jemand den Wert wirklich will. Die
   Falle dabei: wasm kopiert beim `local.get`, also muss ein `local.set`
   jeden noch offenen Verweis auf dieses Local vorher festschreiben.
3. **Operanden in den Befehl gefaltet** → 5,78×. `add eax, [rbp-16]` statt
   Laden-dann-Addieren, `add eax, 5` statt Konstante-in-Register.
4. **Ergebnis bleibt im Cache-Register** → 5,74× (aber python +1,5×). Die
   heissen Operatoren rechnen direkt im Cache statt in `rax`, also kein Zug
   mehr rein und keiner raus.
5. **Eintritts-Trampolin** → Codegrösse 8,1 → 7,8. `r13`/`r14` werden EINMAL
   an der Grenze zu nativem Code gesetzt statt in jeder Funktion gerettet und
   wiederhergestellt: sechs Befehle je Aufruf, die nur am Übergang nötig sind.

Dazu eine Einsparung, die direkt aus dem Wachseiten-Entwurf fällt: **die
Speicherbasis bewegt sich nie.** Die Reservierung steht fest, `memory.grow`
macht nur mehr davon lesbar — also muss `r13` nach einem Aufruf NICHT neu
geladen werden. Eine Implementierung, die Speicher beim Wachsen umkopiert,
müsste das nach jedem Aufruf tun.

### Fuel im Register: 0–6 % statt Winchs 87 %

Der Zähler lebt in `r15`, einmal vom Eintritts-Trampolin geladen und beim
Verlassen zurückgeschrieben. Abgezogen wird **einmal pro Basisblock** (gemessen
5,8 wasm-Instruktionen), geprüft nur dort, wo ein Lauf überhaupt endlos werden
kann: an Schleifenköpfen und am Funktionseintritt. Dazwischen darf der Zähler
ins Negative laufen — er wird an der nächsten Prüfung eingefangen, und genau
deshalb braucht kein Block eine eigene Prüfung.

| | ohne Fuel | mit Fuel | Preis |
|---|---:|---:|---:|
| beak `layout` warm | 5,79× | 5,79× | 0 % |
| beak `parse` | 7,97× | 7,73× | 3 % |
| beak `cascade` | 8,12× | 7,71× | 5 % |
| python `pass` | 14,31× | 13,50× | 6 % |
| python Schleife 1e6 | 11,34× | 10,89× | 4 % |
| Byte je wasm-Instruktion | 7,8 | 9,0 | +15 % |

**Zum Vergleich: Cranelift zahlt +24 %, Winch +87 %.** Der Unterschied ist
genau der, den Winchs Quelltext selbst benennt — der Zähler im Register statt
im Speicher.

**Der Zählstand deckt sich mit dem des Interpreters bis auf 1,9 %**
(709 681 859 gegen 723 152 293 auf beaks warmem srf-Layout). Das ist keine
Kosmetik: die Budgets im Kernel — 10 G für `run`, `PYTHON_FUEL` — sind alle
gegen den Interpreter kalibriert und müssen dasselbe bedeuten. Dafür werden
die rein strukturellen Operatoren (`block`, `loop`, `else`, `end`) NICHT
berechnet; mit ihnen lag die Zählung 7,2 % zu hoch. Bulk-Speicher nach BYTE zu
berechnen wurde probiert und ist falsch — es schiesst um 20–30 % über, der
Interpreter rechnet eine Kopie also etwa pauschal. Woher die letzten 1,9 %
kommen, ist offen.

Zwei Fehler auf dem Weg, beide typisch für die Sorte:

- **Der Schleifenkopf war HINTER der Prüfung markiert.** Die Rücksprungkante
  sprang damit daran vorbei — die Schleife wurde genau einmal geprüft, beim
  Eintritt, also nie. Sichtbar nur, weil der Test eine echte Endlosschleife
  fährt und hängen blieb; im Disassemblat stand es dann in einer Zeile
  (`jne 0x3d` statt `jne 0x34`).
- **`js` statt `jle`.** Ein Budget von exakt null ist auch leer, aber `js`
  feuert nur bei negativ — ein Block wäre noch durchgekommen.

Der Test dazu fährt eine Schleife, die von sich aus nie endet: mit kleinem
Budget MUSS sie gestoppt werden, mit demselben Budget und endlichem Lauf darf
sie es nicht, und mit Budget null darf die Funktion gar nicht erst anlaufen.

### Der Trap-Pfad: ein Trap ist ein ERGEBNIS

Bis hierher waren `ud2`, #DE und ein Seitenfehler gleichermassen tödlich. In
wasm ist ein Trap aber kein Fehler des Systems, sondern **das Ergebnis eines
Moduls** — es ist fertig, und sonst ist nichts kaputt. Also muss er berichtbar
sein.

**Der Rückweg steht im vmctx.** Das Eintritts-Trampolin schreibt `rsp`, `rbp`
und eine Fortsetzungsadresse hinein, bevor irgendetwas trappen kann. Die
Trap-Routine des Moduls ist dann vier Befehle lang:

```
mov [r14+TRAP_CODE], rax     ; der Grund
mov rbp, [r14+TRAP_RBP]
mov rsp, [r14+TRAP_RSP]
jmp [r14+TRAP_RESUME]
```

Das rollt **jede Tiefe** von wasm-Rahmen ab, ohne den nativen Stack zu
entwirren. Und weil es nur `r14` braucht — das Register, das erzeugter Code
nie ändert — kann ein Fehlerbehandler es erreichen, indem er den unterbrochenen
Kontext dorthin zeigen lässt.

**Genau das tut die Host-Seite für #PF und #DE**: der Signalbehandler prüft, ob
der unterbrochene Befehlszeiger im Codebereich des Moduls liegt, und setzt dann
`rip` auf die Trap-Routine und `rax` auf den Grund. Ein Fehler irgendwo sonst
bleibt ein echter Fehler. **Der Kernel wird in seinen #PF- und
#DE-Behandlern dasselbe tun** — dieselbe Trap-Routine, derselbe
Instanz-Kontext; der einzige Unterschied ist, dass er die unterbrochenen
Register aus einem Trap-Rahmen liest statt aus einem `ucontext`.

Je Grund gibt es einen Stumpf von zwei Befehlen (`mov eax, grund` /
`jmp trap_routine`), hinter dem Rücksprung, damit der häufige Pfad durchfällt.
So überlebt der Grund bis zur Laufzeit: `unreachable`, Speicher daneben,
Tabellenindex daneben, falsche Signatur, Divisionsfehler, Fuel aufgebraucht,
Funktion nicht übersetzt.

**Der eigentliche Gewinn ist der Prüfstand.** Alle Trap-Prüfungen liefen bis
jetzt in einem Kindprozess und schauten, an welchem Signal er starb. Jetzt
laufen sie in-process und prüfen den **Grund**:

```
Traps: melden sich — Wachseite, Tabelle, Signatur, Bulk-Raender,
Division, unreachable und Fuel melden sich mit dem RICHTIGEN Grund
```

Darunter die Fälle, die vorher nicht unterscheidbar waren: ein leerer
Tabellenplatz meldet `BAD_SIGNATURE` und nicht „daneben", ein Index genau
hinter der Tabelle meldet `TABLE_OUT_OF_BOUNDS`, und `INT_MIN % -1` meldet
gar nichts, weil es antworten muss. Auf dem heissen Pfad kostet das nichts —
es sind ausschliesslich kalte Stümpfe.

### Am Gerät gelaufen (v0.308.0/0.308.1)

`forge selftest`: **18/18 wie auf dem Host**, unter QEMU und auf dem Blech —
Wachseite, Tabellengrenze, Division, `unreachable` und Fuel melden sich mit
demselben Grund. Und die erzeugte Codegrösse ist auf allen drei Maschinen
**byte-identisch**: beak 3 876 815 B, python 17 036 090 B.

Beim Übersetzen zeigte sich etwas, das man nur im Verhältnis sieht. Auf dem
Entwicklungsrechner skaliert es linear mit der Ausgabegrösse (python 4,4× so
gross, 4,25× so lang); am Gerät brauchte dasselbe Verhältnis **33,7×**. Ein
konstanter Aufschlag kann eine Kurve nicht krümmen — was sie krümmt, muss mit
der Aufgabengrösse mitwachsen und nur auf einer Seite existieren. Das war der
Kernel-Allokator (`heap.rs`, eine einfach verkettete Freiliste) in Verbindung
mit einem Entwurfsfehler in forge: es hielt alle 9 939 Funktionspuffer
gleichzeitig am Leben und baute erst am Ende zusammen.

Der Linker nimmt die Bytes jeder Funktion jetzt sofort und gibt ihren Puffer
zurück:

| | Host | QEMU vorher | QEMU nachher | Blech vorher | Blech nachher |
|---|---:|---:|---:|---:|---:|
| beak | 28 ms | 280 | **100** | 490 | **240** |
| python | 119 ms | 5300 | **530** | 16 510 | **1450** |

Das Verhältnis python/beak fiel dabei von 33,7× auf 6,0× (Blech) und von
18,9× auf 5,3× (QEMU) — der Host liegt bei 4,25×. **Auf dem Host war von
alldem nichts zu messen**, dort war der Fix sogar fünf Millisekunden
schlechter. Wer ihn dort bewertet hätte, hätte ihn verworfen.

Nebenbei ordnet sich damit auch QEMU ein: das Blech ist 2,78× langsamer als
der Entwicklungsrechner, 240 ms / 2,78 sind 86 ms, und QEMU misst 100. Dieses
QEMU läuft also praktisch nativ, und der verbleibende Abstand zum Host von
rund 3,3× ist nicht Hardware, sondern die Kernel-Umgebung — dieselbe einfach
verkettete Freiliste, nur nicht mehr quadratisch belastet.

### Was das am Gerät hiesse

Mit dem gemessenen Verhältnis auf die bekannten Gerätezahlen angewandt —
**Projektion, keine Messung**:

| srf am Notebook | heute | mit forge |
|---|---:|---:|
| ein warmes Layout | 1662 ms | **~290 ms** |
| die 11 Relayouts | **18,3 s** | **~3,2 s** |
| dazu der Relayout-Sturm-Fix | | **~0,3 s** |

python-Start (`pass`) ~519 ms → **~36 ms**; die 1e6-Schleife ~5,2 s →
**~0,46 s**.

### Codegrösse, gemessen — die Zielmarke für den Registercache### Codegrösse, gemessen — die Zielmarke für den Registercache

`.text` gegen `.text` (aus `wasmtime compile`-Ausgaben mit `readelf -S`; die
Dateigrösse taugt dafür nicht, sie enthält eh_frame, addrmap, traps, rodata):

| | B je wasm-Instruktion | gegen wasm-Code |
|---|---:|---:|
| **forge**, Operandenstapel im Speicher | **14,7** | 6,3× |
| Winch, mit Registercache | **8,56** | 3,68× |
| Cranelift, optimierend | **4,87** | 2,09× |

python.wasms Codeabschnitt hat 2,33 B je Instruktion. Der Abstand zu Winch —
Faktor 1,72 — ist genau der Preis der Entscheidung „Operandenstapel im
Speicher": jedes `i32.add` ist Laden/Laden/Addieren/Speichern statt drei Byte.
**8,56 B/Instr ist damit die Zahl, die der Registercache schlagen muss**, und
sie steht fest, bevor er gebaut wird.

### Die Stufen

1. **Decoder + Validator + Modellbildung** — steht, 21/21, gegengeprüft.
2. **Codeerzeugung für einen ersten Ausschnitt**, gegen wasmi differenziell
   geprüft: gleiche Eingabe, gleiches Ergebnis, gleicher Speicher danach.
3. **Voller Befehlssatz**, dieselbe Differenzprüfung über alle Exporte aller
   Module.
4. **Fuel im Register + Traps** (Division durch null, `unreachable`,
   Typfehler beim indirekten Aufruf, Zugriff daneben über die Wachseite).
5. **Kernel**: übersetzen beim `install`, Codeblob in npkFS, W^X beim
   Abbilden, Seitenfehler als Modul-Trap statt Kernel-Panik.

## Womit gemessen wurde

- `tools/aotbench/runner` — lädt `beakbench.wasm` und fährt es in einem
  Prozess unter Kernel-wasmi, winch und cranelift, je mit und ohne Fuel.
  Gegenprobe ist das verbrannte Fuel: es muss 723 M sein.
- `tools/aotbench/bench.sh` — dasselbe für `python.wasm` über vier Laufzeiten
  inklusive nativem CPython, beide Compiler vorübersetzt.
- `tools/wasmtime/` — vorgebautes wasmtime 48.0.1, nur als Messlatte.
- `tools/wasmscan` — der Opcode-Zensus.
- `PYWASI_NOFUEL=1` in `../tools/pywasi` — preist den Zähler auf der
  Interpreter-Seite aus.
