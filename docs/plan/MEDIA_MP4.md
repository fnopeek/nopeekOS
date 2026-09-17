# MP4 abspielen — wohin es gehoert und was es kostet

Stand 2026-09-17. Sidequest von Florian: „ich moechte mp4 abspielen koennen,
die frage ist ob wir das in iris oder in tune? … vlc loest das sauber cpu
only, evtl. koennen wir uns von dort etwas bedienen."

Alle Zahlen dieses Papiers sind **gemessen**, auf dem Ryzen 5 9600X und mit
den beiden bekannten Faktoren auf das Geraet gerechnet: **forge/nativ 3,7x**
(gemessen an beak, `project_wasm_aot_question`) und **Geraet/Ryzen 3,2x**.
Wo etwas geschlossen und nicht gemessen ist, steht es dabei.

## Die Antwort: tune. Und iris bleibt, was es ist.

**Ein Videospieler ist ein Audiospieler, der zusaetzlich malt** — nicht ein
Bildbetrachter, der sich bewegt.

Was ein Film braucht, hat **tune** schon und iris hat nichts davon: die
Audio-Mailbox samt Resampler (`sink.rs`), die Spieluhr, den Vorlauf von
600 ms samt Gegendruck (`flush` sagt „Ring voll"), Pause/Seek als
Schliessen+Oeffnen des Slots, die Wiedergabeliste, den Transport, und vor
allem den **Formatschnitt** `Source` = `info()` / `next_block()` / `seek()`,
der genau fuer diesen Fall gebaut wurde.

Was ein Film von **iris** braucht, sind rund dreissig Zeilen: `npk_canvas_
commit`, `npk_canvas_rect`, `Widget::Canvas`. Das ist abgeschrieben, nicht
portiert.

iris ist ausserdem im falschen Modell gebaut: es dekodiert eine ganze Datei
in eine Arena und zeigt sie. Es hat keine Uhr, keinen Strom, kein Seek, kein
Audio. Ein Film in iris hiesse, tune in iris nachzubauen.

## Was es kostet — gemessen, nicht geschaetzt

### Der Dekoder gibt es, und er ist gut

`rusty_h264-decoder` 0.16.0 (BSD-2, 233 KB): **rein Rust, `forbid(unsafe)`,
`no_std` + `alloc`**, Baseline + B-Slices + grosse Teile von High, CAVLC und
CABAC, bit-genau gegen Cisco `h264dec` geprueft. Kein C im Abhaengigkeitsbaum.
Gebaut und **auf echtem Material gemessen** (bester aus 5 Prozessen — die
Streuung zwischen Prozessen ist 35 %, ein Lauf taugt nicht):

| Ryzen, skalarer Arm | ms/Frame | Mpx/s |
|---|---:|---:|
| 1080p30, 5 Mbit, echter Inhalt | 3,25 | 638 |
| 720p30, 2,5 Mbit | 1,44 | 641 |
| 480p30, 1,2 Mbit | 0,68 | 605 |

Zum Vergleich auf derselben Datei: **ffmpeg ohne SIMD** (`-cpuflags 0`) liegt
bei 1,27–4,07 ms/Frame, also 1,0–1,4x SCHNELLER. Ein reiner Rust-Dekoder ist
hier kein Zoll, sondern gleichauf mit ffmpegs C-Pfad.

### Die Rechnung aufs Geraet

`x11,8` = forge 3,7 mal Geraet 3,2. „% Kern" heisst: Anteil eines Kerns bei
30 Bildern je Sekunde.

| | Dekode | + Farbraum IM Modul | + Farbraum im Compositor |
|---|---:|---:|---:|
| **1080p30** | 38,5 ms/F = **115 %** | **260 %** | **123 %** |
| **720p30** | 17,0 ms/F = **51 %** | 115 % | **55 %** |
| **480p30** | 8,1 ms/F = **24 %** | 53 % | **26 %** |

**Die mittlere Spalte ist der eigentliche Befund.** YUV nach BGRA umzurechnen
kostet IM MODUL bei 1080p **145 % eines Kerns** — mehr als das Dekodieren
selbst, und das bevor ein einziges Bit H.264 gelesen ist. Gemessen: 4,07 ms
je Frame auf dem Ryzen, ohne Vektorisierung und mit `opt-level = "s"`, also
genau so, wie das Modul wirklich gebaut wird.

**Das ist der Punkt, an dem VLC uns etwas beibringt, und es ist der erste
von drei.**

## Drei Stuecke aus VLC, und alle drei passen

### 1. Der Dekoder rechnet NIE in den Bildschirmfarbraum

VLC reicht `picture_t` in dem Format weiter, in dem der Dekoder es ohnehin
hat (I420), und der **vout** rechnet einmal um — mit SIMD, in Zielgroesse.
Unser `npk_canvas_commit` nimmt nur BGRA, also muss heute das Modul
umrechnen: 145 % eines Kerns bei 1080p, und dazu 8,3 MB je Frame durch die
Modulgrenze statt 3,1 MB.

**Vorschlag: der Leinwand ein Format mitgeben** (`npk_canvas_commit_fmt`
oder ein Formatfeld). I420 kostet 1,5 Bytes je Pixel statt 4 — **62 %
weniger Bytes ueber die Grenze** — und der Compositor rechnet nativ um, wo
SSE2/AVX2 laut Zielspezifikation (`targets/x86_64-nopeek`) zur Verfuegung
stehen. Dass wir die Rechnung auf unserer Seite haben wollen, ist auch der
Grund, warum sie dort billig ist.

### 2. Umrechnen und Skalieren sind EIN Durchgang, in ZIELgroesse

Steht ein 1080p-Film in einem 1280x720-Fenster, rechnet VLC 0,92 Mpx um,
nicht 2,07. Unser Weg laedt heute erst voll hoch und skaliert danach. Im
selben Durchgang wie der Blit ist die Umrechnung **2,25x kleiner** und der
Zwischenpuffer faellt ganz weg.

### 3. Lieber ein Bild fallen lassen als zurueckfallen

VLCs Uhr ist die **Tonausgabe**, nie die Wanduhr; ein zu spaetes Bild wird
verworfen, ein zu frueh fertiges wartet. **Unsere Spieluhr ist die
Wanduhr** — `Sink::tick` rechnet `played` aus `now_ms`. Fuer Ton allein
traegt das (tune tut es seit 0.1.0). Fuer Lippensynchronitaet ueber einen
ganzen Film traegt es nicht: die HDA-Uhr und die Wanduhr laufen ppm-weise
auseinander, und der Fehler summiert sich.

Das ist derselbe Posten, den tunes eigene Notiz schon benennt: *„Ein
`npk_audio_buffered(slot)` waere die ehrlichere Quelle."* Fuer Video ist es
keine Verbesserung mehr, sondern die Voraussetzung.

## Und was im Compositor daneben liegt

Beim Nachmessen des Ausgabewegs zwei Funde, beide unabhaengig von Video:

**`canvas_blit` rechnet eine 64-Bit-DIVISION je Zielpixel.**
`(dx as u64 * sw as u64) / dst_w as u64`, zweimal je Pixel. Bei 1920x1080
sind das 2 Millionen Divisionen je Bild. Gemessen (Ryzen → Geraet x3,2):

| | Ryzen | Geraet | % Kern bei 30 fps |
|---|---:|---:|---:|
| heute (Division je Pixel) | 2,32 ms | 7,4 ms | 22 % |
| 16.16-Schrittweite statt Division | 1,04 ms | 3,3 ms | 10 % |
| 1:1-Zeilenkopie (Fenster in Originalgroesse) | 0,11 ms | 0,35 ms | 1 % |

Eine Festkomma-Schrittweite ist das, was jeder Skalierer seit dreissig Jahren
tut, und sie halbiert es. Der 1:1-Fall ist 21x billiger und heute nicht
vorhanden. **Das trifft iris und beak heute schon**, nicht erst Video.

**Ein Commit auf die Leinwand zeichnet das GANZE Fenster neu.**
`npk_canvas_commit` ruft `rerender_window(wid)`, also Widgetbaum-Durchgang
plus Rasterung des kompletten Fensters — 30-mal je Sekunde fuer eine
Bildflaeche, die sich allein geaendert hat. Das ist genau die Form, die beak
0.35.4 fuer ankommende Bilder schon einmal aufgeloest hat („nur was in der
sichtbaren Bahn liegt"). Fuer Video braucht es den Leinwandkasten als eigene
Schmutzflaeche.

## Der Umbau von tune

Florian: „btw tune bisschen umbauen sollen." Ja — und der Schnitt ist schon
da, er muss nur eine Ebene hoeher.

Heute: `Source` = ein Strom von Audioblöcken, `open()` entscheidet nach
Inhalt. Fuer einen Film fehlt die Ebene darueber, die EINE Datei in ZWEI
Stroeme teilt.

```
Demux (MP4 · WAV · MP3 · …)
  ├── AudioSource  → Resampler → Sink → Mailbox      (der heutige Weg)
  └── VideoSource  → Bildschlange → Leinwand
                      ^
                 Uhr = Sink (Tonausgabe), nicht die Wanduhr
```

- `Source` bleibt woertlich, was es ist, und heisst `AudioSource`. MP3 und
  WAV ruehren sich nicht an.
- `Demux` liefert `(Option<AudioSource>, Option<VideoSource>)`. Eine MP3 ist
  ein Demux mit einem Strom — **der Weg fuer Ton allein wird dadurch nicht
  laenger**, und das ist die Bedingung, unter der der Umbau richtig ist.
- Die Bildschlange ist klein (3-4 Bilder). Groesser hiesse mehr Latenz beim
  Seek und mehr Halde, nicht weniger Ruckler.
- Ohne Videostrom ist tune Zeile fuer Zeile das, was es heute ist.

**Der MP4-Demuxer wird selbst geschrieben**, nicht geholt: es sind ein paar
hundert Zeilen Boxen (`moov`/`trak`/`stbl`: `stsd` `stts` `stsc` `stco`
`stsz` `ctts`), und er ist der Teil, der als erster fremde Bytes anfasst.
Genau dort wollen wir unseren eigenen Code und keinen mit `std`-Naht.

**AAC** ist der Tonstrom jeder MP4. Gemessen kostet er **nichts**: 0,36 ms
je Sekunde Ton (ffmpeg, 1 Thread) — selbst mit x11,8 sind das 0,4 % eines
Kerns. Reine Rust-Dekoder gibt es (`symphonia-codec-aac`, `rusty_aac`), beide
`std`-gebunden; das ist eine Portierung, keine Forschung.

## Die Leiter — WIDERLEGT am Geraet (2026-09-17)

Die Vorhersage stand hier so:

| | vorhergesagt | was daraus wurde |
|---|---|---|
| **480p30** | 26 % eines Kerns | — |
| **720p30** | 55 % | — |
| **1080p30** | **123 %, „geht nicht"** | **die Vorhersage ist falsch** |

**Florians Geraetelauf spielt 2560x1440 bei 30 fps fluessig** — mehr als
doppelt so viele Pixel wie 1080p, und die Leiter sagte, schon 1080p sei
nicht zu schaffen. Fenster vergroessern, skalieren, Arbeitsflaeche wechseln:
laeuft weiter.

**Was daran falsch war, laesst sich eingrenzen, aber noch nicht aufteilen.**
1440p30 sind 110,6 Mpx/s, und derselbe Dekoder schafft auf dem Ryzen skalar
841 Mpx/s (auf genau dieser Datei gemessen). Also ist **forge x Geraet
zusammen hoechstens 7,6x**, nicht die 11,8x, mit denen die Tabelle gerechnet
hat. Welcher der beiden Faktoren daneben liegt, ist NICHT gemessen; der
Verdacht ist forge:

> **3,4-3,9x ist an beaks Box-Layout gemessen** — Zeigerjagd, Allokation,
> dichte Aufrufe. Ein Dekoder ist das Gegenteil: lange arithmetische
> Schleifen ueber Felder, mit Zugriffsmustern, die der Cache mag. Es gibt
> keinen Grund anzunehmen, dass derselbe Aufschlag gilt, und ich habe ihn
> trotzdem uebertragen.

**Die Lehre gehoert hierher und nicht in eine Fussnote:** ein Faktor, der an
EINER Last gemessen wurde, ist keine Eigenschaft des Uebersetzers. Die
Tabelle sah aus wie eine Messung und war eine Extrapolation.

**Was gemessen bleibt und weiter gilt:** die Dekoderzeiten je Frame auf dem
Ryzen, die Farbmathematik, der Blit, und dass die Farbraumrechnung im Modul
145 % eines Kerns kostet. Falsch war nur die Bruecke aufs Geraet.

## Was am Geraet WIRKLICH begrenzt: die Bitrate, nicht die Pixel

Der eine Fehler im Lauf war eine **Tag/Nacht-Ueberblendung**: Rueckstand
424 -> 647 -> 314 ms, danach wieder eingeholt. Harte Schnitte in derselben
Datei liefen ohne Rueckstand durch.

Das ist die ganze Physik in einem Satz: **Dekodierkosten folgen den BITS,
nicht den Pixeln.** Eine globale Helligkeitsaenderung laesst nichts
vorhersagen, also hat jeder Makroblock ein grosses Residuum — und das ueber
viele Bilder hintereinander. Ein Schnitt ist EIN teures Bild und wird von
der Reihenfolge-Warteschlange schon geschluckt; eine Ueberblendung ist eine
Kette.

Gebaut daraus (tune 0.2.4): **Vorlauf beim Dekodieren**, 1000 ms, gedeckelt
auf 128 MB. Die 1000 kommen aus den gemessenen 647, nicht aus dem Bauch; der
Bytedeckel ist der, der bei 1440p wirklich greift (23 Bilder = 775 ms), und
er steht in Bytes, weil ein Deckel nach Bildern bei jeder Aufloesung etwas
anderes kostet. Gefuellt wird nur, wenn die billigen Szenen davor Zeit
uebrig hatten.

## „Das koennen wir mit unserem Compiler beschleunigen"

Stimmt, und ohne forge waere dieses Papier gar nicht geschrieben: unter wasmi
stuende 1080p bei **880 %** und selbst 480p bei **180 %** eines Kerns. **forge
ist das, was die Frage ueberhaupt zu einer Frage macht.**

Was die verbleibenden 123 % auf unter 100 brächte, in der Reihenfolge, in der
es gemessen wirkt:

1. **Der allgemeine forge-Aufschlag, 3,4-3,9x.** Das ist der grosse Posten,
   und er ist NICHT SIMD — das wurde am 2026-09-01 an beak ausdruecklich
   falsifiziert. Der Weg ist benannt und einmal begangen: die Argumente
   direkt in die Argumentregister nahmen python von 3,66x auf 3,26x. Ein
   Dekoder ist dichte Rechnung auf Feldern; **was dort wirkt, ist die
   Registerzuteilung und die Schrankenpruefung**, nicht der Aufrufpfad.
   Gemessen ist das noch nicht — der Dekoder ist eine andere Last als beak.
2. **`simd128` in forge.** Wir haben die Hardware (Zielspezifikation:
   SSE/AVX2/AES-NI) und heute keinen Weg dorthin: in `forge/core` gibt es
   null Treffer auf `v128`. **Achtung, das ist keine stille Luecke** — eine
   Funktion, die forge nicht uebersetzt, bekommt den Trap-Stub. Ein Modul mit
   SIMD-Kernen stuerzt ab, es wird nicht langsamer. Bis dahin gilt: den
   Dekoder mit `--no-default-features` bauen (skalarer Arm).
   **Was es wert waere, ist gemessen und ernuechternd:** derselbe Dekoder mit
   und ohne seine portablen SIMD-Kerne, auf demselben Material, ist
   **1,14x** — bei 1080p 3,25 → 2,84 ms. Erst auf bitratendichtem Material
   (Handkamera, 0,09 bit/px) sind es **2,16x**. ffmpegs handgeschriebenes
   Assembler liegt bei 1,6x. **SIMD allein macht 1080p nicht auf.**
3. **Ein zweiter Kern.** Der Dekoder kann Frame-Level-Threading
   (`decode_stream_threaded`), heute `std`-gebunden. Wir haben Worker-Kerne.
   Das ist der Hebel, der 1080p wirklich oeffnet — und der groesste Umbau.
4. **Die Xe-Medien-Engine.** Das Blech hat einen H.264-Dekoder in Hardware.
   Das ist ein Treiberposten in der Groessenordnung von WLAN, nicht ein
   Nachmittag, und er steht hier nur, damit er benannt ist.

## Die Sicherheitsfrage, vorweg beantwortet

Ein Mediendekoder ist die klassischste Angriffsflaeche, die es gibt — die
CVE-Liste von ffmpeg ist im Wesentlichen diese eine Datei-Art. Deshalb:
**das Dekodieren bleibt im Modul, hinter der WASM-Grenze**, und zwar
vollstaendig. Was in den Kernel wandert, ist ausschliesslich die
Farbraumrechnung und das Skalieren — eine Funktion ohne Zustand, die feste
Groessen bekommt und Pixel zurueckgibt.

Der Dekoder ist `#![forbid(unsafe_code)]` und gefuzzt panikfrei. Das ist kein
Zufallsvorteil, sondern der Grund, ihn zu nehmen: eine Panik ist bei uns ein
toter Tab, kein Einstieg.

## Erster Schritt

Nicht der Dekoder. **Zuerst der Ausgabeweg**, weil er ohne eine Zeile H.264
messbar ist und weil iris und beak heute davon leben:

1. `canvas_blit` auf Festkomma-Schrittweite + 1:1-Schnellpfad (22 % → 1-10 %).
2. Der Leinwandkasten als eigene Schmutzflaeche statt `rerender_window`.
3. `npk_audio_buffered(slot)` — die ehrliche Uhr, die tune ohnehin fehlt.
4. I420 als Leinwandformat, Umrechnung im Blit.

Danach ist 720p eine Frage von Demux + Dekoder, und die Zahl dafuer steht
oben.

## ✅ Gebaut — Kernel 0.341.0, der ganze Ausgabeweg

Alle vier Posten des ersten Schritts. Noch nichts davon ist am Geraet
gelaufen; die Zahlen unten sind host-seitig gemessen und mit x3,2
umgerechnet.

**1. `canvas_blit` rechnet keine Division mehr je Pixel.** Die
Quellstelle WANDERT (ganzer Teil + Rest, Bresenham), und bei gleicher
Groesse laeuft eine Zeilenkopie. **Bit-gleich zur alten Fassung**, geprueft
ueber 35 Kombinationen aus Zielgroesse und Versatz
(`<tools>/mediabench/src/blit_equiv.rs`) — keine Renderhashes bewegen sich.

    1080p 1:1       2,32 → 0,11 ms Ryzen   (22 % → 1 % Kern bei 30 fps)
    1080p → 720p    1,02 → 0,68 ms        (10 % → 7 %)

Das trifft iris und beak sofort, ohne dass ein Modul etwas aendert.

**2. Die Geometrie hat EINEN Besitzer.** Fit, Zoom, Pan-Klemme und
Ausschnitt stehen jetzt in `canvas_fit`, damit die zwei Blits nicht
auseinanderlaufen koennen.

**3. `npk_canvas_commit_yuv(canvas_id, y, u, v, ys, cs, w, h, flags)`** —
planar 4:2:0 mit eigenen Zeilenschritten, `flags` Bit 0 = Rec.709,
Bit 1 = full range. Der Farbraum wird im BLIT gerechnet, in Zielgroesse:

    1080p Bild → 1080p Fenster   4,98 ms Ryzen → 16 ms Geraet = 48 % Kern
    1080p Bild →  720p Fenster   2,04 ms       → 6,5 ms       = 20 %
    1080p Bild →  480p Fenster   0,92 ms       → 2,9 ms       =  9 %

Gegen die 145 % + 22 %, die derselbe Schritt IM MODUL kostet: **167 % →
48 %.** Die Koeffizienten sind gegen ffmpeg geprueft
(`<tools>/mediabench/src/yuv_oracle.rs`): auf glattem Bild **max |Δ| = 1**
in allen vier Codierungen. Auf einem Bild mit harten Farbkanten sind es 3,
und das ist die Chroma-Hochtastung (ffmpeg interpoliert, wir wiederholen),
nicht die Matrix — bewiesen dadurch, dass ein 16-Bit-Bruch es dort
SCHLECHTER macht und auf dem glatten Bild gar nichts aendert.

Die Laengenpruefung steht an genau EINER Stelle (`canvas::commit_i420`),
und der Blit indiziert danach ohne eine einzige weitere Frage. Das ist
Absicht: zwei Stellen, die dieselbe Schranke rechnen, sind zwei Stellen,
die auseinanderlaufen koennen.

**4. Ein Bild auf die Leinwand legt das Layout nicht mehr neu aus.**
`rerender_window_pixels` nimmt das gespeicherte `layout_tree` — der
Widgetbaum hat sich bei einem Canvas-Commit nicht geaendert, also haette
`layout_scrolled` dasselbe Ergebnis noch einmal gerechnet.
**Die Rasterung laeuft weiterhin ueber das ganze Fenster**, und das ist
ebenfalls Absicht: nur den Leinwandkasten zu malen muesste wissen, was
DARUEBER liegt (ein offenes Menue, ein Popover, ein Scroll-Ausschnitt), und
ein Neuanstrich mit falscher Z-Reihenfolge loescht das Menue statt des
Bildes. Das Layout zu ueberspringen braucht diese Frage gar nicht erst.

**5. `npk_audio_buffered(slot)`** — die ehrliche Spieluhr. `audio::buffered`
gab es im Kernel schon, die virtio-snd-Bruecke meldet damit seit je ihre
Latenz an den Gast; **nur die Tuer zu einem WASM-Modul fehlte**. Ohne sie
schaetzt tune aus der Wanduhr, und fuer Lippensynchronitaet ueber einen
Film traegt das nicht.

### Was dabei aufgefallen ist und NICHT angefasst wurde

**Die Audio-Schlitze haben keinen Besitzer und keine Kapabilitaet.**
`npk_audio_submit`, `npk_audio_close` und `npk_audio_open` pruefen nichts
und tragen keine `pid`; jedes Modul kann in den Schlitz eines anderen
schreiben oder ihn schliessen. Dieselbe Form wie die fuenf `npk_tcp_*` vor
Kernel 0.337.0. `npk_audio_buffered` ist bewusst mit derselben Parität
gebaut — eine Lesefunktion strenger zu machen als das Schreiben daneben
kauft nichts. Der Posten gehoert in einen eigenen Schnitt: Schlitz-Tabelle
mit `pid`, und die Frage, ob Ton eine eigene Kapabilitaet braucht.

**Der I420-Blit ist der naechste SIMD-Kandidat** — 48 % eines Kerns bei
1080p sind nativ und skalar. Waagrechte Paare teilen sich ihr Chroma;
SSE2 steht in der Zielspezifikation. Kein ABI, rein intern, jederzeit
nachrüstbar.
