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

**Stand 2026-09-06 · beak 0.115.0 · Kernel 0.326.0** (Rest: `git log`)

Zwei Fäden laufen parallel.

**▶ Als nächstes: Ligaturen (GSUB).** Symbolschriften bilden ihr Zeichen als
Ligatur; fontdue läuft mit `load_substitutions: false`, also ist `fos-icon`
1 px statt 24 — kein Schriftfehler, ein GSUB-Fehler. Danach: `body` 600 statt
937 (die Seite hält das Fenster auf). Stand:
`memory/project_beak_web_app_stack.md`.

**0.115.0 hat drei Fehler geschlossen, die alle am Gerät sichtbar waren.**
Ein Feld zeigte Getipptes erst beim Verlassen an und blendete dabei den Text
daneben weg — der Schnellweg beim Tippen ersetzte die falschen Zeichenbefehle,
weil die notierte Spanne eines Steuerelements an DREI Stellen verschoben
wurde, ohne mitgezogen zu werden. Ein Riegel rechnet die Spanne jetzt nach und
legt lieber aus, als fremde Befehle zu überschreiben. Zweitens: die Quer-Größe
eines Flex-Kastens steht auch fest, wenn sie aus `min-height`/`max-height`
kommt — damit sitzt die Fritzbox-Anmeldung mittig statt oben zu kleben, und
die zwei WPT-Verluste aus 0.109.0 sind zurück. Drittens zentriert
`margin: auto` jetzt auch ein blockweites Steuerelement. **WPT 4477 -> 4481,
+5/−0, Baseline neu gesegnet.** Kastengeometrie gegen Chromium:
`memory/project_beak_render_oracle.md`.

**Farbverläufe sind seit 0.114.0 gebaut** — linear/radial, je auch
`repeating-`, mit dem Winkel, den der KASTEN einer Ecke vorgibt, Kachelung
über `background-size` und `in oklab` gelesen-und-fallengelassen. Vorher war
ein Verlauf ein flacher Kasten, und die Fritzbox-Anmeldung hatte deshalb
einen weissen Kopf mit weisser Schrift darauf. WPT 4474 -> 4477, kein Test
verloren. Offen nach Häufigkeit im Korpus: `conic` (11 von 255), ein Verlauf
am `html`-Kasten, `calc()` in einer Stopp-Lage (2 von 255).
Stand: `memory/project_beak_gradients.md`.

Gebaut seit 0.104: ES-Module, Custom Elements, Formular-Brücke samt
`submit`-Ereignis, nachgeladene Stilblätter, `load`/`DOMContentLoaded`, und
**`@font-face`** (WOFF2 mit Brotli und `glyf`-Rückbau, gegen
`woff2_decompress` an fünf Schriften geprüft: 4620 Zeichen rasterisiert, 0
abweichend).

**Der Browser-Vergleich liegt in `<memory-dir>/../tools/mirror/`** — ein
eingefrorenes Spiegelbild der Seite lokal ausgeliefert, dieselbe Sonde in
Chromium und in beak, Kastengeometrie statt Pixel. Das war das offene Stück
im Renderorakel.

**Das Werkzeug dafür ist `beak-engine/examples/pagerun.rs`** — es fährt die
ganze Skriptrunde einer Seite host-seitig, in EINER Sitzung, mit Modulgraph,
und `DUMP=1` zeigt den Baum danach. „Laufen die Skripte" ist nicht dieselbe
Frage wie „haben sie etwas gebaut", und `jsrun` (eine Datei allein)
beantwortet die falsche.

**`beak`**, der eigene Browser: **Stage 1 läuft — die Seite reagiert.** Eigene
JS-Maschine (Lexer, Parser, RegExp, DOM-Bindung) und die Wirtsumgebung. Sie
läuft auf einer **Befehlsmaschine** statt eines Baumläufers — der Zustand ist
ein Feld, nicht der Rust-Stapel. **Der Umbau ist mit 0.86.0 abgeschlossen**;
seit BigInt (0.94.0) laufen 99,6 % der Programme darauf.

**Zwei Zahlen, und sie messen NICHT dasselbe.** Die 99,6 % sagen, WO Code
läuft — ein abgelehntes Programm fährt der Baumläufer mit identischer
Bedeutung. Was GEHT, sagt test262:

    test262 exec    81,27 %   (V8 auf demselben Korpus: 99,41 %)
    Zielkorpus      437/437 geparst, 305/437 durchgelaufen
    DOM-Aufrufe     98,3 % gedeckt  (`tests/apigap.rs`, Chromium-Zensus)
    WPT (CSS)       4476/5192 = 86,2 % ohne Testvehikel (roh 79,4 %)

0.89.0–0.100.0 haben die Sprache in zehn Releases von 66,93 auf 80,65 %
gebracht: **Date** (richtig gerechnet, nicht mehr gestumpft), **eval**
(direkt und indirekt), **Proxy**, **BigInt** samt eigener Bignum und den
64-Bit-Sichten, die **Iterator-Hilfen**, die Empfängerprüfung überall, zwei
Dutzend ausgezählte Eingebaute — und zuletzt drei Runden am OBJEKTMODELL:
**0.98.0 der strenge Modus** (978 Varianten), **0.99.0 `defineProperty`
prüft wirklich** (1299), **0.100.0 private Felder mit Marke** (104), je
ohne eine einzige Regression.

**Und die Lehre aus allen dreien:** die naheliegende Zählung lag jedes Mal
daneben. Beim strengen Modus fehlte nicht die Strenge, sondern der
UNTERSCHIED — 56 % der Treffer lagen im LOCKEREN Modus. Bei
`defineProperty` war nicht die Prüfung das Problem, sondern das MODELL: in
`Prop` war „Feld fehlt" dasselbe wie „false". Die Rangfolge des Rests steht
in `memory/project_beak_js_language_gap.md`, gemessen statt geraten —
`T262_FAILDETAIL=<datei>` gibt jeden Fehler mit seiner Meldung, und die
grösste Meldung ist meist eine Sammelmeldung.

Das eigene Testziel ist **`beak:selftest`** — eine Prüfseite aus dem
Binärbild, die nichts holt und ihr Ergebnis auf dem Schirm UND im Log sagt.
Sie läuft auch host-seitig über dieselbe Datei
(`beak-engine/examples/selftest.rs`). Ein Lauf fand neun Lücken, die fremde
Seiten in Wochen nicht gezeigt hatten. **0.97.0 lief am Gerät voll grün
**0.100.0 lief am Gerät voll grün: Sprache 49/49,
Dokument 31/31, Klicks 4/4, Timer + Microtask.** Die 49 schliessen die acht
Zeilen ein, die nur am Gerät etwas beweisen — der Modus entscheidet sich
zur Laufzeit, und `this` im einfachen Aufruf ist die, die am ehesten wieder
kaputtgeht.

Die CSS-Runde davor ist zu Ende gebracht: das Eigenschafts-Gap ist
geschlossen, 93,7 % der Deklarationen auf Bootstrap + Wikipedia abgedeckt. Die
gemessene WPT-Zahl steht in `docs/spec/CONFORMANCE.md` und nirgends sonst —
**zwei Nenner**, roh und ohne Testvehikel, und der zweite wird mit
`tools/wasm/beak-engine/tests/vehicles.py` aus der gesegneten Baseline
HERGELEITET, nie weitergetragen. **Vor jeder WPT-Planung dieses Werkzeug
laufen lassen:** 347 der 1163 Fehler sind `display: grid-lanes`, und kein
Dateiname sagt es.

**WLAN (AX200)**: ⏸ pausiert, die Verbindung läuft (Download 116 Mbit auf HT40,
Upload erstmals möglich). Das Intent **`wlan`** ist das Werkzeug dafür —
Kernel-Sicht plus ein Klartext-Report, den der Treiber selbst veröffentlicht
(Rate, Retries, Airtime, 4-Way-Sprosse, Ring-Zustand) plus der wifid-Log.
Beim Wiedereinstieg NUR den obersten Abschnitt von
`memory/project_wifi_stability_handover.md` lesen — er hat Stand,
Betriebspunkt und die nächsten Schritte.

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
