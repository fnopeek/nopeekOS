# Vendorter H.264-Dekoder

`rusty_h264-decoder` + `rusty_h264-common` 0.16.0, BSD-2-Clause,
© Mata Network — <https://github.com/remade-with-rust/rusty_h264>.

Hier im Baum statt aus der Registry, aus zwei Gruenden. Der erste ist ein
Fehler, der zweite ist der wichtigere.

## 1. Der no_std-Arm baut upstream nicht

Genau EINE Zeile, in `rusty_h264-decoder/src/mb16.rs`:

    pub(crate) mod edcstat {
    +   use alloc::vec::Vec;

Ohne `std` bringt der Prelude `Vec` nicht mit, und dieses verschachtelte
`mod` hat keinen eigenen Import; mit `std` faellt es niemandem auf. Es ist
Diagnosecode — das `eprintln!` darin ist im no_std-Arm ohnehin ein No-op.
**Das ist der einzige Unterschied zum veroeffentlichten Stand.** Gemeldet
gehoert er trotzdem.

## 2. Ein Dekoder ist die Flaeche, die wir besitzen wollen

Ein Mediendekoder ist die klassischste Angriffsflaeche, die es gibt. Dass
er `#![forbid(unsafe_code)]` traegt und gefuzzt panikfrei ist, war der
Grund, ihn zu waehlen; dass er dann bei einem `cargo update` unbemerkt
wechselt, waere der Grund, es zu bereuen. Im Baum ist jede Aenderung ein
Diff, den jemand liest.

## Wie er gebaut wird

`--no-default-features`, dazu `libm`. Damit bleibt der **skalare** Arm:

- **`asm` MUSS aus bleiben.** Es sind portable Rust-SIMD-Kerne, und forge
  kennt `simd128` nicht. Eine Funktion, die forge nicht uebersetzt, bekommt
  den Trap-Stumpf — das Modul stuerzt beim ersten Aufruf ab, es wird nicht
  langsamer. Gemessen ist der Arm ohnehin nur 1,14x auf normalem Material.
- **`std` und `global-alloc` bleiben aus**: tune bringt seine Halde selbst
  mit (`nopeek_widgets::heap`).

Nicht mitkopiert: `tests/`, `examples/`, `benches/` und die zugehoerigen
Dev-Abhaengigkeiten (eine davon ist ein Fenstersystem). Die Abschnitte sind
aus den `Cargo.toml` entfernt; sonst ist alles unveraendert.

## Beim Aktualisieren

1. Neue Fassung entpacken, `src/` und `Cargo.toml` ersetzen.
2. Pruefen, ob der Patch aus (1) noch noetig ist.
3. Beispiel-/Test-Abschnitte wieder entfernen.
4. `python3 tools/forge-gate.py` auf das gebaute tune.wasm — eine neue
   Fassung kann einen Opcode mitbringen, den forge nicht kennt.
5. `<tools>/mediabench/src/pipeline_probe.rs` gegen ffmpeg: die Pixel
   muessen byte-gleich bleiben.

---

# Vendorter AAC-Dekoder

`rusty_aac` 0.5.0, Apache-2.0, © Mata Network — dieselbe Familie wie der
H.264-Dekoder oben, und aus demselben Grund gewaehlt: **keine einzige
Laufzeitabhaengigkeit** und ein eigener MDCT. Der naechstbeste Kandidat
(`soundkit-aac-lc`) zieht `rustfft` — std-gebunden, gross, SIMD —, und der
IMDCT IST der Kern eines AAC-Dekoders; ihn ueber eine fremde FFT zu holen,
die wir dann auch portieren muessten, ist der laengere Weg.

## Was portiert wurde (upstream ist `std`)

Der Dekodierpfad braucht erstaunlich wenig: von den 61 `std::`-Stellen des
Crates liegen fast alle in `encode.rs` — **4940 der 8602 Zeilen**, und wir
dekodieren nur.

1. **`encode` und `latm` hinter dem Merkmal `encode`**, aus per Vorgabe. Die
   Dateien bleiben liegen, damit sich der Vendor gegen upstream diffen
   laesst. `latm` haengt am `BitWriter` des Encoders; MP4 traegt rohe
   AAC-Rahmen, also brauchen wir es nicht.
2. **`src/chanorder.rs`** — die eine Funktion aus `encode.rs`, die der
   Dekoder wirklich braucht (`aac_to_interleave_order`), woertlich kopiert.
3. **`src/mathshim.rs`** — `sin`/`cos`/`sqrt`/`abs`/`powf`/… sind in `std`,
   nicht in `core`. Statt 35 Aufrufstellen auf `libm::sinf(x)` umzuschreiben
   (und damit den Diff gegen upstream zu zerstoeren) steht dort ein Trait mit
   denselben NAMEN; ein `use` je Datei genuegt. Dazu ein `OnceLock` auf
   `spin::Once`, weil dessen Methode `call_once` statt `get_or_init` heisst.
4. `std::` → `core::` fuer Konstanten, `fmt`, `ops`, `cmp`, `mem`; die
   `impl std::error::Error` faellt weg (gibt es in core nicht, die Meldung
   traegt `Display`).
5. `use alloc::…` fuer `Vec`/`String`/`Box`/`vec!`/`format!`.

**`simd` bleibt aus** (Vorgabe ist jetzt `default = []`) — wie beim H.264
und aus demselben Grund: forge kennt kein `simd128`, und was forge nicht
uebersetzt, trappt.

## Geprueft

`<tools>/mediabench/src/aac_probe.rs`: MP4 durch UNSEREN Demuxer, Rahmen
durch diesen Dekoder, PCM gegen `ffmpeg -f s16le`.

    Handyvideo, 44,1 kHz stereo, 267 Rahmen:
      Spitze wir 14435, ffmpeg 14435
      bester Versatz 2112 Samples · Korrelation 1,000000
      SNR 99,8 dB · groesste Abweichung 1 LSB

AAC ist nach ISO 14496-3 float-basiert und ausdruecklich NICHT bit-exakt
vorgeschrieben; 1 LSB ist Rundung. **Und der Versatz von 2112 ist genau das
`edit_start`, das unser Demuxer aus der Edit-List liest** — ffmpeg schneidet
exakt diese AAC-Vorlaufsamples weg. Die Edit-List ist damit nicht mehr eine
Vermutung ueber Lippensynchronitaet, sondern eine gemessene Zahl.

## Der Zusatz, ohne den er unbrauchbar ist: `imdct_fast`

Upstream wertet die IMDCT **direkt** aus — `cos()` in der inneren Schleife,
2048 x 1024 = 2,1 Millionen Transzendentenaufrufe je langem Block. Der
Modulkopf sagt es selbst vorher: *„can be swapped for an FFT-based fast path
later without changing results."*

Gemessen, was es kostet:

    direkt:   948 ms je Sekunde Ton (Ryzen 9600X)  ->  x11,8 = 1118 % eines
                                                       Geraetekerns
    schnell:  3,0 ms je Sekunde Ton                ->  4 % eines Kerns

**Faktor 316.** Mit der direkten Fassung verhungerte am Geraet alles andere:
der Bildweg kam auf 4 Dekodierungen je Sekunde und die Mailbox lief
durchgehend leer.

`imdct_fast` ist Zeile fuer Zeile die UMKEHRUNG von `mdct_fast`, das
upstream schon hatte — dieselben Twiddles, dieselbe M-Punkt-FFT. Beide
Drehungen sind orthogonale 2x2-Matrizen, also ihre eigene Umkehrung; die
inverse FFT laeuft ueber die Konjugierten-Regel.

**Die direkte Fassung bleibt als ORAKEL stehen**, und ein Test haelt beide
bei N = 16, 64, 256, 2048 gegeneinander. Genau der hat auch den einzigen
Fehler gefunden: ich hatte den Massstab mit `1/N` GERATEN, das Orakel sagte
„Verhaeltnis exakt 16 bei N=16", und damit war der Faktor 1. Eine Konstante
in einer Transformationskette gehoert gemessen.

Am Ergebnis aendert die Umstellung nichts: derselbe ffmpeg-Vergleich, 1 LSB
und 99,8 dB wie vorher.
