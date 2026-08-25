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

## Was es brächte

Ein einstufiger, nicht optimierender Compiler landet typischerweise 3–10×
über einem Interpreter — nicht bei den vollen 37×, die gehören optimiertem
nativem Code.

| bei 435 M/s | heute | 4× | 6× | 10× |
|---|---:|---:|---:|---:|
| Stansstad, warmes Layout | 949 ms | 237 ms | 158 ms | 95 ms |
| srf, warmes Layout | 1662 ms | 416 ms | 277 ms | 166 ms |
| srf, die 11 Relayouts zusammen | **18,3 s** | 4,6 s | 3,0 s | 1,8 s |

## Was es kostet

Drei Posten, die in der Begeisterung untergehen:

1. **Der Kernel enthielte einen Codegenerator.** W^X-Verwaltung, und eine
   neue Angriffsfläche im vertrauenswürdigsten Teil des Systems.
2. **Fuel als Ressourcengrenze fällt weg.** Übersetzter Code zählt nicht mehr
   mit; Limits bräuchten echte Timer-Preemption statt eines Zählers. Unsere
   Fibers sind kooperativ — das ist eigene Arbeit.
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
Capability-Gating bleiben, wie sie sind — nur Posten 2 (Fuel) muss ersetzt
werden. Das ist ein ganz anderer Umbau als „native Module statt WASM".

## Stand der Technik

Sprachbasierte Isolation statt Hardware ist kein neuer Einfall:
**Singularity** (Microsoft Research, 2005, Software-Isolated Processes,
alles aus verifizierter IL vom vertrauenswürdigen Compiler, IPC ~100×
billiger), **Theseus** (2020, Rust, ein Adressraum, „cells"), **RedLeaf**
(OSDI 2020, Rust, Domänen über Ownership). Alle drei geben viel Mühe an
genau den Posten, der oben als 4. steht: eine Panik im Modul einfangen und
seine Ressourcen zurückholen.

## Empfehlung

1. **Erst der Relayout-Sturm.** 11 volle Layouts à 1689 ms auf srf sind
   18,3 s und brauchen keinen Compiler — sie kommen daher, dass alle 118
   `<img>` der Seite ohne Grössenangabe kommen und jeder ankommende Pixel
   einen geratenen Kasten korrigiert.
2. **Dann der Compiler**, wenn er kommt: als AOT-Schritt beim `install`, auf
   der `.wasm`, die wir sowieso signieren. Nicht als neues Modulformat.
3. Vorher der Ersatz für Fuel. Ohne den ist der Compiler ein Loch im
   Ressourcenmodell.
