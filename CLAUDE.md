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

**Stand 2026-09-09 · beak 0.148.0 · Kernel 0.330.0** (Rest: `git log`)

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

**▶ Als nächstes: das RASTER.** beak rechnet Wikipedias `main.mw-body` mit
`grid-template: … / minmax(0,59.25rem) min-content` falsch aus und lässt die
Spalten über ihren Behälter hinauslaufen (1299 + 248 > 1492); die
Artikelspalte ist dadurch ~79 px zu breit. In WPT ist es dieselbe Familie:
`css-grid/column-align-items`, `row-auto-repeat`, `grid-lanes-subgrid` — nach
`CSS2/bidi` (bewusst zurückgestellt) der größte verbliebene Block.

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

    test262 exec    81,76 %   (V8 auf demselben Korpus: 99,41 %)
    test262 parse   96,84 %
    DOM-Aufrufe     99,4 % gedeckt  (`tests/apigap.rs`, Chromium-Zensus)
    WPT (CSS)       4542/5201 = 87,3 % ohne Testvehikel (roh 80,4 %)
    Bibliotheken    13 von 13 (`<tools>/libprobe/`)
    beak:selftest   Sprache 55/55, Dokument 39/39
    Kastengeometrie Bootstrap 413/415 · Tailwind 165/165 (`<tools>/gallery/`)
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
