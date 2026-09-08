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

**Stand 2026-09-08 · beak 0.132.0 · Kernel 0.329.0** (Rest: `git log`)

**▶ Als nächstes: `IntersectionObserver`/`ResizeObserver` (782 Aufrufe im
Zensus), dann htmx (`XPathEvaluator`) und die Ligaturen.** Rangliste:
`docs/plan/WEB_PLATFORM_GAPS.md` §0a.

**0.132.0 + Kernel 0.329.0: `crypto.getRandomValues` mit echtem Zufall.**
Neue Hostfunktion `npk_random_bytes` aus `security::csprng` (ChaCha20, aus
RDRAND geseedet) — ohne Kapabilität wie `npk_unix_time`, registriert in
BEIDEN Wegen (forge und wasmi). **`crypto` erscheint nur, wenn eine echte
Quelle da ist:** eine Seite prüft `if (window.crypto)`, und `Math.random`
als sichere Quelle auszugeben wäre schlimmer als die Lücke.

**google.ch: der Weg ist fertig, die Tür ist zu.** beak fährt die ganze
Kette — Startseite, Formular, Einwilligung, `/save`, Botguard, Token — und
bekommt `/sorry` + 429. Nachgemessen: weder die IP (derselbe Anschluss
bekommt mit Firefox-Kennung 200) noch die Kennung (drei Kennungen, dieselbe
Seite). Google bewertet den Token. Das ist die Wand aus
`memory/feedback_no_ua_impersonation.md`.

**0.129.0: sechs Schriften beim Start, zwei gebraucht.** `beakbench` sagt,
dass das Schriftrastern die Halde von 11 auf 89 MiB treibt und das Layout
nichts drauflegt; die neue Sonde `examples/heapcheck.rs` trennt GEHALTEN von
SPITZE und zeigt, dass die 40 MB **Dauerbedarf** sind (6,7 MB je Gesicht,
2,9 KB je Glyphe — fontdue umreisst jede Glyphe beim Parsen, siehe
`assets/subset.sh`). Keine Seite fasst alle sechs an (ddg 0, selftest 2,
tailwind 5), also werden sie **faul** geladen: Start 435 → 0 ms, srf
89 → 44 MiB, alle zwölf `gate`-Hashes byte-identisch. Stand:
`memory/project_beak_memory_footprint.md`.

**0.328.0 (Kernel): was `memory.grow` dazulegte, kam nie zurück.** Der
generierte Code wächst über `forge_rt::grow`, die den **vmctx** schreibt;
`Memory::size` blieb auf dem Startwert, und `Memory::drop` gab genau den
frei. Alles, was eine Modulhalde während eines Laufs dazunahm, blieb bis zum
Neustart abgebildet. beak starb danach an „Halde erschöpft, 318 Seiten" —
und 318 ist die **Startgrösse** aus seinem Binärbild, der Lauf war also
unschuldig. `Memory::grow` hielt die Grösse korrekt nach und wurde von
niemandem gerufen; er ist weg. **Am Gerät bestätigt:**
`[npk] forge: Instanz gibt 79 MB zurueck (59 MB davon gewachsen)` — so viel
ging bisher bei jedem beak-Lauf verloren. Offen und davon getrennt: warum
eine 26-KB-Seite die Halde überhaupt auf 79 MB treibt (talc gibt Seiten nie
zurück, die SPITZE wird zum Dauerbedarf).

**0.128.0: der klassische Clearfix mass null.** Räumung ist Platz IM Kasten,
kein Schub AUF ihn — die Oberkante des Elters wanderte mit hinunter. Die
Regel stand wörtlich im Code, acht Zeilen unter der Stelle, an der sie
fehlte, im Pfad für `::after` mit `clear`. **WPT 4481 → 4485, +4/−0**,
Baseline neu gesegnet; alle zwölf `gate`-Hashes byte-identisch.

**0.126.0: eine freigegebene Adresse gab ihren Rumpf an die nächste
Funktion.** `func_chunks` schlüsselte den übersetzten Rumpf nach
`Rc::as_ptr`. Gibt das erste `<script>` seinen Syntaxbaum frei, kann eine
Funktion des zweiten genau dort liegen — und bekommt beim Aufruf **fremden
Code**. Kein Absturz, keine Meldung. Sichtbar wurde es an Alpine.js; auf dem
Baumläufer (`NOVM=1`) lief derselbe Code sauber. Ein `Weak` hält die Zelle
jetzt belegt. **Und der erste Test dafür lief gegen den Fehler grün durch** —
er hoffte auf eine Kollision statt die Invariante zu prüfen.

**Der Fund kam aus einem neuen Prüfstand: dreizehn echte Bibliotheken**
(`<memory-dir>/../tools/libprobe/`), jede mit einer echten Benutzung statt
eines „geladen?"-Hakens. **10 von 13 grün** (war 6): jQuery, Bootstrap,
Alpine, React, Preact, lodash, dayjs, axios, marked. Dafür gebaut:
`Object.prototype.toString` packt Primitive ein (+12 test262),
`Symbol.toStringTag` an allen DOM-Schnittstellen, `document.implementation`,
**MutationObserver** — und in einer zweiten Runde (0.127.0) sechs
Sprachlücken, die d3, chart.js und Vue gezeigt haben: `target / 2` war ein
Regex, **Löcher waren Werte** (sieben Feldmethoden), `{__proto__:null}`,
`class X extends null`, **jede Proxy-Falle lief mit `this === undefined`**,
`hasOwnProperty` ging am Proxy vorbei, und **`with (o) { … }`** gab es nicht.
**test262 exec 81,27 → 81,76 %.** Offen: nur noch htmx (`XPathEvaluator`).

**0.125.0: `location` war ein Datenobjekt — es gab in beak überhaupt keine
Navigation per Skript.** `replace` ein TypeError, `href = u` schrieb still
eine Eigenschaft um. Jede Anmeldung, die nach dem POST weiterleitet, lief
ins Leere. Jetzt ganz gebaut, `javascript:`/`data:` gesperrt, acht Sprünge
in Folge sind Schluss. Gefunden an Googles Sperrseite: die rechnet in beak
ihren Botguard-Token korrekt aus und rief dann `location.replace` — der
TypeError landete in ihrem EIGENEN `.catch`, und unser Lauf meldete
„0 gescheitert". Google bleibt trotzdem eine Sperrseite (Policy, nicht
Können — `memory/project_beak_search_engines.md`).

**Ligaturen (GSUB) sind weiter offen.** Symbolschriften bilden ihr Zeichen
als Ligatur; fontdue läuft mit `load_substitutions: false`, also ist
`fos-icon` 1 px statt 24. Danach: `body` 600 statt 937. Stand:
`memory/project_beak_web_app_stack.md`.

**Die Zahlen, und sie messen NICHT dasselbe:**

    test262 exec    81,76 %   (V8 auf demselben Korpus: 99,41 %)
    test262 parse   96,84 %
    DOM-Aufrufe     98,5 % gedeckt  (`tests/apigap.rs`, Chromium-Zensus)
    WPT (CSS)       4485/5200 = 86,2 % ohne Testvehikel (roh 79,4 %)
    Bibliotheken    12 von 13 (`<tools>/libprobe/`)
    beak:selftest   Sprache 55/55, Dokument 35/35
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
