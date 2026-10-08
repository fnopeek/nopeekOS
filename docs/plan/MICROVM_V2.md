# microVM v2 — Grundgerüst, App-Schichten aus apk, schneller Kaltstart

*Diskussionspapier, 2026-10-07. Richtung entschieden; der Snapshot ist gemessen und verworfen (§0.5); Details offen (§9).*

## Idee in drei Sätzen

Wir liefern nur noch ein **Grundgerüst** (Kernel, Alpine-Basis, unser PID 1,
cage). Was der Nutzer haben will, holt seine Maschine **einmalig aus den
offiziellen Alpine-Quellen** in eine eigene App-Schicht — kuratiert über einen
Store. Gestartet wird **kalt, aber schnell**: Linux bis cage in rund einer
halben Sekunde, die Platten bei Bedarf aus npkFS gelesen und im Hintergrund
vorab geladen. Ein Snapshot war geplant und ist nach der Messung gestrichen
(§0.5).

## 0. Erst beweisen, dann bauen

v2 ist ein **Neustart**, nicht ein Umbau von v1. Bevor irgendetwas vom Store
gebaut wird, beantwortet ein Prototyp genau eine Frage: **spart Fortsetzen aus
npkFS gegenüber einem guten Kaltstart überhaupt Zeit?**

### 0.1 Was neu ist und was bleibt

- **Neu:** Gerätemodell, Lebenszyklus einer VM, Speicherverwaltung des Gasts,
  Steuerkanal.
- **Bleibt:** die CPU-Ebene (`cpu/svm`, `cpu/vmx`: VMCB/VMCS, Intercepts,
  MSR/CPUID/XCR0-Härtung, Interrupt-Injektion, HLT-Behandlung). Sie ist auf
  beiden Herstellern am Blech erarbeitet und im Code-Review (D12-D24)
  geprüft — sie neu zu schreiben heisst, dieselben Wände noch einmal zu
  finden.

### 0.2 Firecracker als Konzept, Jailer als Vorbereitung

- **Firecracker übernehmen wir als Konzept, ab Tag 1:** so wenige Geräte wie
  möglich, kein ACPI (haben wir schon: `acpi=off`). Jedes Gerät, das eine VM
  nicht braucht, ist Angriffsfläche weniger (§6.2, Ring 3). Firecracker selbst
  hat kein GPU, Ton und Eingabe; die kommen bei uns dazu.
- **Jailer bauen wir jetzt nicht** (Geräteemulation aus Ring 0 heraus ist ein
  eigenes Projekt). Aber das Gerätemodell wird **jailer-fähig** geschrieben:
  jedes Gerät ist reine Logik auf `(Zustand, Gastspeicher-Zugriff,
  Byte-Puffer)`, ohne Kernel-Globale, ohne rohe Zeiger. Dann lässt es sich
  später herauslösen, ohne neu zu schreiben — und es ist sofort host-seitig
  testbar und fuzzbar (Ring 3, §6.2).

### 0.3 Der Prototyp

Gast: 1 vCPU, 256 MiB, Kernel + initramfs, PID 1 meldet `ready` über
virtio-console. Kein GPU, kein Netz, keine Platte. Gemessen wird mit TSC vom
Befehl bis zur Meldung `ready`:

| # | Weg | Erwartung (grob) |
|---|---|---|
| M0 | **heute**, v1 bis cage/LibreWolf, zerlegt in Kernel · Userspace · cage · App | ~3-4 s gesamt (alte Notiz) — wo es liegt, ist unbekannt |
| M1 | **optimierter Kaltstart** v2 bis `ready` (kein udev, statisches `/dev`, `quiet`) | ~100-300 ms |
| M2 | **Fortsetzen, alles vorab geladen** | belegte Seiten / npkFS-Lesen (~250-300 MB/s) — bei ~80 MB belegt ~300 ms |
| M3 | **Fortsetzen, faul geladen** | erste Anweisung in ms, bis `ready` abhängig von der Arbeitsmenge |

### 0.4 Entscheid nach der Messung

- **M3 klar unter M1** (Faktor 3+): Snapshot lohnt sich → §5 wird gebaut.
- **M1 schon klein** (unter ~200 ms) und M3 kaum besser: **kein Snapshot.**
  v2 ist dann Store + schneller Kaltstart, und die ganze Zufalls- und
  Gerätezustandsfrage (§5) fällt weg.
- Unabhängig davon zeigt M0, wo die heutigen Sekunden wirklich liegen. Liegt
  der Grossteil im **App-Start** (LibreWolf, Chromium), spart kein Snapshot an
  Punkt A ihn ein — dann ist das ehrliche Ergebnis, dass der Gewinn des Stores
  die Apps sind, nicht die Startzeit.

### 0.5 Ergebnis: kein Snapshot

Gemessen am 2026-10-07 mit `kernel/src/microvm/boottime.rs` (Kernel
0.496.0/0.497.0), LibreWolf als App, Zeit ab dem Startbefehl.

**M0, heutiger Start:**

| Abschnitt | QEMU (Ryzen 9600X) | Notebook (Ryzen 5700U) |
|---|---|---|
| Abbilder aus npkFS | 15 ms | 23 ms |
| VM aufsetzen: beide Platten ganz vorab geladen | 728 ms | 1356 ms |
| Kernel entpacken | 91 ms | 115 ms |
| Gastkernel bis PID 1 | 1540 ms (fast alles serielle Konsole) | 273 ms |
| PID 1 bis „cage next“ | 170 ms | 100 ms |
| **bis cage** | **2,56 s** | **1,87 s** |
| cage bis Bild #50 / #200 | +4,9 s / +7,4 s | +4,7 s / +7,2 s |

**M1, optimierter Kaltstart** (Platten bei Bedarf aus npkFS in 1-MiB-Stücken,
Gast mit `loglevel=5`):

| | QEMU | Notebook |
|---|---|---|
| VM aufsetzen | 316 ms (sqfs dort noch ein Blob, §0.6) | **40 ms** |
| **bis cage** | **0,97 s** | **0,58 s** |
| bis Bild #50 | 6,26 s (vorher 7,45) | 5,62 s (vorher 6,53) |

**Entscheid:** Ein Snapshot an Punkt A könnte am Notebook höchstens noch
~0,4-0,5 s sparen. Dafür bräuchte es Gerätezustand für jedes Gerät, faules
Laden des RAM über NPT/EPT und das Neusäen des Zufalls — der schwierigste und
riskanteste Teil von v2. **Gestrichen.** M2/M3 wurden nicht gebaut; die Zahl,
gegen die sie hätten gewinnen müssen, ist schon zu klein. v2 ist **Store +
schneller Kaltstart**.

**Was die Messung sonst sagt:**
- Die Wartezeit ist jetzt die **App** (5-7,5 s nach cage), nicht Linux. Ein
  Snapshot vor dem App-Start hätte daran nichts geändert.
- Die App-Phase wurde mit M1 auf beiden Geräten ~0,4 s langsamer: LibreWolf
  liest über die Sitzung **243 von 253** sqfs-Stücken. Bedarfsweises Lesen
  spart also keine Menge, es verschiebt nur den Zeitpunkt in die App-Phase
  hinein. Daher §0.6.
- Speichern von `home.img`: 168 von 512 Stücken geschrieben, 344 übernommen
  (vorher immer alle 512).

### 0.6 Nächste Schritte am Start

- **Vorauslesen im Hintergrund** (Kernel 0.498.0): ab dem Öffnen der Platte
  liest ein Worker auf einem anderen Kern die Stücke, die beim letzten Lauf
  berührt wurden, zuerst, bei der sqfs danach den Rest. Die berührte Menge
  steht neben dem Abbild (`<pfad>.hot`). Entschlüsselt wird ohne das
  Slot-Lock; der Gast liest selbst nur, was der Worker noch nicht hat.
- **Installer schreibt grosse Assets in Stücken**, wie der OTA-Weg; sonst
  bleibt eine frisch installierte sqfs ein Blob, der nur ganz lesbar ist.
- **Die App-Phase messen** (§0.7), bevor dort optimiert wird.
- `sleep 1` vor cage ersetzen (PID 1 im Gast, braucht neues initramfs).

### 0.7 App-Start: Ergebnis — es war die Kompression

Gemessen am 2026-10-07, LibreWolf, Fenster 946x1074.

**Wo es nicht liegt:**
- **Nicht am Rendering:** Firefox nativ auf dem Entwicklungsrechner (9600X),
  frisches Profil, Zeitstempel aus `Services.startup.getStartupInfo()`:
  erstes Bild 0,49 s, Fenster fertig 0,53 s **mit Software-Rendering**
  (mit GPU 0,6 s).
- **Nicht an musl oder am Build:** genau das LibreWolf aus der sqfs, per
  `bwrap` nativ auf dem Wayland des Rechners, mit der `user.js` des Gasts:
  0,51 s / 0,55 s.
- **Nicht an unserem VMM:** derselbe Gast unter QEMU/KVM
  (`tools/microvm_kvm.py`) war ebenso langsam wie bei uns.
- **Nicht an cage:** ohne Kompression startet cage in 0,10 s.

**Wo es liegt:** Der Gast entpackt jeden Block der sqfs beim ersten Zugriff,
auf einem Kern; `libxul.so` hat 184 MB, `omni.ja` 42 MB.

| sqfs | KVM 9600X: cage → App | KVM: Start → Fenster | Notebook: cage → App | Notebook: Start → Bild #50 |
|---|---|---|---|---|
| gzip | 0,76 s | 3,5 s | 1,00 s | 4,49 s |
| **zstd** | 0,28 s | 2,3 s | **0,55 s** | **3,23 s** |
| unkomprimiert | 0,10 s | 1,9 s | 0,27-0,62 s | 2,76-3,12 s |

Unkomprimiert liest der Gast 609 statt 222 MB aus npkFS; am Notebook
verschiebt sich der Engpass damit vom Entpacken zum Lesen und Entschlüsseln,
und der Vorsprung schmilzt. **Entscheid: zstd** (Kernel 0.501.0) — fast so
schnell, kleinste Datei, ein Drittel des RAM für vorausgelesene Stücke.
Der alte Grund für gzip (ein Absturz am Stack-Canary `gs:[0x28]` unter zstd)
tritt nicht mehr auf; er war ein Fehler unseres VMM.

Weitere Funde dabei: `sleep 1` vor cage ersetzt durch Warten auf den
seatd-Socket (5 ms); die `sync`-Schleife beginnt erst nach 15 s;
`browser.startup.page=3` lädt die Tabs der letzten Sitzung übers Netz und
verfälscht spätere Bildzähler (deshalb gilt Bild #50, nicht #200).

**Notebook, Start bis LibreWolf (Bild #50):**

| Stand | Zeit |
|---|---|
| v1 | 6,53 s |
| Platten bei Bedarf, leiser Gast (0.497) | 5,62 s |
| + Vorauslesen (0.498) | 5,23 s |
| + ohne `sleep 1` (0.499) | 4,31 s |
| + zstd (0.500) | 3,23 s |
| Startmass ab hier „Fenster“ statt Bild #50: zstd, DNS/ARP behoben (0.505) | 2,5 s |
| + lz4 (0.506) | **1,8 s** |

**Der „langsame erste Start“ war DNS** (Kernel 0.504.0, microvm-init
0.4.28). LibreWolf löst beim Start den eigenen Rechnernamen auf; `nopeek`
stand nicht in der `/etc/hosts` des Gasts, also ging bei jedem Start eine
Anfrage an 1.1.1.1. Und der ARP-Eintrag des Wirts verfiel 30 s nach dem
Lernen: das erste Paket danach ging an L2-Broadcast und wurde vom Router
verworfen. Folge: nach jeder Pause über 30 s fiel die Anfrage weg, und musl
fragte erst nach 2,5 s neu. Jetzt kennt der Gast seinen Namen, und ein
abgelaufener ARP-Eintrag wird weiter benutzt und nachgefragt (wie Linux
`NUD_STALE`). Am Notebook danach auch nach 124 s Pause kein Ausreisser.

**Startmass ab Kernel 0.505.0: `app: window up`** — das erste Bild nach dem
App-Start, das zu mindestens der Hälfte nicht schwarz ist (Konsole und
leere cage-Ausgabe sind schwarz, ein Browserfenster nicht). Bild #50 zählt
das Nachladen der Seiten mit und liegt 0,5–0,7 s später.

**Notebook, Kernel 0.505.0** (Läufe 2–4, Fenster 946 und 1902 breit):

| Abschnitt | Zeit |
|---|---|
| Start → cage | ~0,62 s, davon bis 150 ms Streuung bis Bild #1 (GPU-Start) |
| cage → App gestartet | ~0,48 s |
| App gestartet → Fenster | ~1,31 s, unabhängig von der Fenstergrösse |
| **Start → Fenster** | **~2,5 s** |

Nativ, dasselbe LibreWolf aus der sqfs per `bwrap`, warm, 9600X:
erstes Bild **0,32 s** (`tools/librewolf_native_start.py`). Kalt (Cache
geleert) steht aus — erst damit ist klar, wie viel der 1,31 s ein
unvermeidbarer Kaltstart ist.

**lz4 statt zstd** (Kernel 0.506.0, `assets/librewolf-0.5.0`, 311 statt
239 MB, Inhalt identisch). Notebook, vier Läufe: cage → App 0,48 → 0,23 s,
App → Fenster 1,31 → 0,98 s, **Start → Fenster 2,5 → 1,8 s**. Die 72 MB
mehr liest das Vorauslesen im Hintergrund (285–293 von 297 Stücken). Unter
KVM war der Gewinn kleiner (2,18 → 1,83 s): der langsamere Kern entpackt
langsamer und gewinnt mehr.

**Der RT-Wächter kostete die halbe App-Zeit** (Kernel 0.508.0,
microvm-init 0.4.29). PID 1 forkt einen Wächter, der cubeb/AudioIPC-Threads
auf `SCHED_RR` setzt; er öffnete alle 50 ms die `comm`-Datei JEDES Threads,
immer. Das konkurriert mit dem Anlegen von Threads, und daraus besteht ein
Browserstart. LibreWolf meldet im Gast selbst (Autoconfig →
`getStartupInfo()` → `/dev/kmsg`), KVM 9600X: Prozess → erstes Bild
**830 → 405 ms**, nativ dasselbe Programm ~310 ms. Jetzt sucht der Wächter
nur, solange ein Wiedergabe-Stream offen ist
(`/proc/asound/card0/pcm0p/sub0/status` ≠ `closed`).

Ausgeschlossen auf dem Weg dorthin, jeweils gemessen: Netz/DNS beim Start
(nativ keine Anfrage), Entpacken (raw −60 ms), Fenstergrösse (−55 ms),
cage/pixman (nativ verschachtelt gleich), vCPU-Zahl, `idle=poll`,
`mitigations=off`, THP, vorab belegter Gastspeicher (−25 ms), `fsync` des
Profils (`cache=unsafe`), Zufall (crng nach 241 ms). Rechnen, Systemaufrufe,
Prozessstart und Speicher kosten im Gast dasselbe wie nativ.

**Was die Messzeilen ab Kernel 0.510 am Notebook gezeigt haben** (jede
`browser`-Sitzung schreibt `[lw-startup]` und `[boottime] app phase …`):

| Befund | Ergebnis |
|---|---|
| Interrupt-Zustellung an vCPUs | Mittel 4–8 µs, 90 % < 10 µs — kein Engpass |
| Geräte-Lock, Exit-Bearbeitung | ~25 ms bzw. ~110 ms über alle vCPUs — kein Engpass |
| Takt der vCPU-Kerne (APERF/MPERF) | ~4,07 GHz, voller Boost — keine Bremse |
| 4-KB-Gastseiten | unter KVM +40 %; **0.514: Bedarfsbereich in 2-MB-Seiten** (am Notebook ohne sichtbaren Gewinn, in QEMU −30 %) |
| **virtio-blk ohne `SEG_MAX`** | **51 276 Anfragen à 4 KB → 0.517: `seg_max = 128`, 3 755 Anfragen** |
| `readahead()` der libxul (184 MB) | ruft LibreWolf vor `main`; unter KVM ohne Wirkung abgeschaltet (402 ms gleich) |

Notebook, Kernel 0.518: **Start → Fenster 1,70 s** (Linux bis cage 0,61 s,
cage → App 0,21 s, App → Fenster 0,89 s). LibreWolf selbst: `main` 176 ms,
Fenster 473 ms, erstes Bild 892 ms. Unter KVM auf dem 9600X: 10 / 210 /
405 ms. Ungeklärt bleibt vor allem `main` (176 gegen 10 ms).

**Fenster:** Ab 0.515/0.518 erscheint ein Linux-Fenster erst mit dem ersten
App-Bild (≥ 50 % nicht schwarz): beim Anlegen einmal gekachelt, damit der Gast
seine Grösse kennt, dann versteckt; das erste Bild deckt es auf und gibt den
Fokus. Nach „browser exited“ werden keine Bilder mehr übernommen.

**Auflösung (Kernel 0.519/0.520):** Der Gast-Benchmark (`microvm_cmdline
npkbench_cpu`) gibt den CPU-Faktor Notebook (Zen 2) gegen Desktop (Zen 5):
Rechnen 2,0×, Speicher 1,9×, Systemaufrufe 3,7×, Prozessstart 3,2× (der
Spectre-Schutz von Zen 2 kostet jeden Kernel-Eintritt). Der Prozess-Sampler
(`microvm_stats` → `[lw-proc]`) zeigt: **unter KVM startete LibreWolf
zweistufig**, weil `tools/microvm_kvm.py` QEMU hart beendet und das Profil
beschädigt (prefs.js mit NUL-Zeilen, 56 `prefs-N.js`, `InvalidateCaches=1`);
`main` misst ab dem zweiten Prozess, die „10 ms“ waren geschönt. Am Notebook
startet LibreWolf einstufig; die ~170 ms vor `main` sind Rechenzeit im
Gastkernel (`sys` 160 ms bei +200 ms: exec, Abbilden, Seitenfehler,
squashfs entpacken), kein Warten (`runqueue` ≈ 0). **Unser VMM bremst den
App-Start nicht messbar**; der Abstand zum Desktop ist der Prozessor.

**Folgerungen:**
- Ein hart gestoppter Gast beschädigt das App-Profil. Der Steuerkanal mit
  sauberem `quit` (§5.1, Stufe 1) schützt Daten, nicht nur Startzeit.
- Der KVM-Prüfstand braucht ein frisches oder sauber geschlossenes Profil,
  sonst vergleicht er gegen einen Neustart.

**Offen:**
- Kalte native Messung (nativ war kalt = warm, ~0,3 s).
- Streuung bis Bild #1 (~150 ms) beim GPU-Start des Gasts.
- cage → App 0,48 s: war Mesa/LLVM beim Laden, siehe §5.2.

**Mehrkerniges Entpacken bringt nichts** (2026-10-08, KVM 9600X, 6 vCPUs,
946x1074, je drei Läufe): `SQUASHFS_COMPILE_DECOMP_MULTI_PERCPU` gegen
`SINGLE` — cage → App 272 gegen 273 ms, cage → volles Fenster 1,36 gegen
1,37 s. LibreWolf liest im Wesentlichen aus einem Prozess nacheinander per
Seitenfehler; ein Entpacker je CPU hilft nur gleichzeitigen Lesern. Nicht
übernommen. Achtung beim Kconfig: einstellbar ist nur `COMPILE_DECOMP_*`,
`DECOMP_*` ist ein versteckter Hilfsschalter.

**QEMU mit zstd** (Kernel 0.501.0, 2 vCPUs, 2542x1314, zweiter Lauf):
Aufsetzen 21 ms (vorher 316, die sqfs liegt jetzt in Stücken), bis cage
0,70 s (vorher 0,97), cage → App 0,43 s, bis Bild #50 3,57 s (vorher 6,26).
Der erste Lauf war dort nur ~0,3 s langsamer.

### 0.7a Methode, falls die Frage wiederkommt

Vor jeder Massnahme die Zerlegung, mit derselben Methode wie M0:

| Vergleich | trennt |
|---|---|
| LibreWolf **nativ** auf dem Entwicklungsrechner, kalt (Seitencache geleert) und warm | was die App selbst braucht |
| **dasselbe Gastabbild unter QEMU/KVM** auf Linux | unseren VMM gegen Gast und App |
| unser VMM mit **zwei Fenstergrössen** | ob die Zeit mit den Pixeln wächst (Software-Rendering) |
| unser VMM mit **1, 2, 6 vCPUs** | ob sie mit den Kernen fällt |

Kandidaten danach, je nach Befund:
- **Schliessen = Schlafen:** das Fenster schliessen hält die VM an statt sie zu
  beenden; ein erneutes Öffnen ist sofort da. Dieselbe Instanz läuft weiter,
  also kein geklonter Zufall. Kostet RAM, solange sie schläft.
- **Vorstart beim Systemstart** für die zuletzt benutzte App, unsichtbar, auf
  Wunsch.
- **Ruhezustand je App** (Hibernate): einmalig fortsetzbar, das Abbild wird beim
  Fortsetzen gelöscht — dann gibt es kein zweites Fortsetzen desselben
  Zustands. Braucht den Gerätezustand aus dem gestrichenen §5; nur, wenn die
  beiden Punkte davor nicht reichen.
- **GPU-Beschleunigung** im Gast, falls das Rendering die Zeit frisst.

## 1. Warum

| heute (v1) | v2 |
|---|---|
| `release/apps` 3,8 GB, davon LibreWolf 3,4 GB, jede Fassung als eigener Satz `cpio.gz` + `sqfs` | Release trägt nur das Grundgerüst (Grössenordnung `alpine-wayland` 16 MB + cage) |
| eine App: der Browser | jede App, die im Store steht |
| jede App-Aktualisierung = unser Release | Aktualisierung = `apk upgrade` gegen Alpine, Rezept bleibt gleich |
| Start bis cage 1,9-2,6 s | Start bis cage ~0,6 s (gemessen, §0.5) |

**Ehrlich benannt:** gewonnen werden **Apps, Releasegrösse und die Startzeit von
Linux**. Die Startzeit der App selbst und ihre Laufzeitleistung ändern sich
dadurch nicht (§0.7).

## 2. Begriffe

- **Grundgerüst (Basis)** — `linux-virt.bzImage`, Basis-`sqfs` (Alpine + Wayland
  + cage + unser PID 1 + `squashfs-tools`), von uns signiert, per OTA.
  **Optional:** wer keine Linux-Apps will, lädt auch die Basis nie — sie kommt
  erst mit der ersten App.
- **Rezept** — Liste von apk-Paketen + Startbefehl + Rechte einer App, z. B.
  `{ pkgs: [chromium], exec: "chromium --ozone-platform=wayland", caps: [net, snd] }`.
  Klein, von uns signiert, Teil des Stores.
- **App-Schicht** — Ergebnis von `apk add` über der Basis, liegt in npkFS. Ein
  **Zwischenspeicher**, aus dem Rezept jederzeit neu baubar.
- **App-Daten** — ext4-Abbild je App, getrennt von allem anderen (§4b).

Schichten von unten nach oben: **Basis (ro) → App-Schicht (ro nach Bau) →
App-Daten (rw)**, dazu Freigaben auf Nutzerordner. Jede Schicht ist einzeln
ersetzbar. **Je App eine eigene VM** (entschieden) — die Grenze ist die App.

## 3. Der Store

- Quelle sind **ausschliesslich die offiziellen Alpine-Repos** (`main`,
  `community`). Kein fremdes Repo, kein Binär-Upload.
- Wir testen vorab und vergeben je Eintrag einen Status:
  - **geprüft** — läuft in unserer Umgebung (Wayland/cage, virtio-gpu, snd,
    net), im Store sichtbar.
  - **experimentell** — klar gekennzeichnet, startet, keine Zusage.
  - nicht gelistet = nicht installierbar.
- Der Store selbst ist eine von uns signierte Liste von Rezepten (gleicher
  ECDSA-P-384-Schlüssel und gleicher OTA-Weg wie Module). Er pinnt
  **Alpine-Release** (z. B. 3.24), nicht einzelne Paketversionen —
  Sicherheitsupdates von Alpine sollen ankommen.
- Nur musl-Software aus Alpine. Chromium statt Chrome. glibc-Software (gcompat,
  Flatpak) ist **ausdrücklich nicht** Teil von v2.

## 4. Installation einer App

1. Nutzer wählt einen Eintrag. Anzeige: Status, Downloadgrösse, Rechte.
2. Eine **Bau-VM** startet aus der Basis, App-Schicht leer und beschreibbar,
   Netz nur zu den Alpine-Spiegeln.
3. PID 1 führt `apk add --no-cache <pkgs>` aus. **apk prüft die Signaturen**
   gegen die Alpine-Schlüssel, die fest in der Basis liegen.
4. Die Schicht wird versiegelt (ro), in npkFS abgelegt, Rezept + Basisfassung
   daneben vermerkt.

**Vertrauensmodell:** Der Wirt vertraut dem Inhalt der Schicht **nichts** — sie
läuft nur je in einer VM, und die VM ist die Grenze (Prinzip 5). Die Signatur
schützt den Nutzer vor einem manipulierten Spiegel, nicht den Wirt vor der App.
Daraus folgt: die Signaturprüfung im Gast genügt.

**Basis ändert sich** → alle App-Schichten darauf sind ungültig →
neu bauen aus dem Rezept, im Hintergrund, App-Daten bleiben.

**Format der Schicht: sqfs je App** (entschieden, Begründung §4a).

## 4a. Format der App-Schicht

Heute hängt der Gast `/dev/vdb` (sqfs, ro, Userspace) und `/dev/vda` (ext4,
512 MiB, Profil). Für die App-Schicht standen zwei Wege zur Wahl:

| | **ext4-Oberschicht** (overlayfs upper als Abbild) | **sqfs je App** |
|---|---|---|
| Bau | `apk add` schreibt direkt hinein, fertig | `apk add` in tmpfs-Oberschicht, danach `mksquashfs` |
| Unveränderlich | nur per Konvention (ro einhängen) | vom Format her |
| Grösse | feste Abbildgrösse vorab, Leerraum + Löschreste | genau der Inhalt, komprimiert |
| Prüfbar | Hash über ein Abbild mit Zufallsanteil (Zeitstempel, Belegung) | ein Hash über ein dichtes Abbild, dm-verity später möglich |
| Wirtscode | keiner | keiner (`mksquashfs` läuft in der Bau-VM) |
| npkFS-Dedup (1-MiB-Stücke) | gut zwischen Fassungen | schlecht über Fassungen (Kompression verschiebt alles) |
| Laufzeit | kein Entpacken | Entpacken (zstd/lz4, klein) |
| passt zu heute | wie `home.img` | wie `userspace.sqfs` — derselbe Weg |

**Entscheid: sqfs.** Die Schicht soll versiegelt sein, und das ist sqfs vom
Format her, ext4 nur durch Disziplin. Der Dedup-Nachteil zählt wenig: eine
App-Schicht wird bei einem Update ohnehin ganz neu gebaut, und alte Fassungen
werden weggeräumt.

**Bauablauf** (alles in der Bau-VM, der Wirt rechnet nur Blöcke):

    overlay: lower = base.sqfs, upper = tmpfs
    apk add <pkgs>
    mksquashfs <upper> /dev/vdc -comp zstd     # vdc = leeres Ausgabegerät
    poweroff

Der Wirt liest die Länge aus dem sqfs-Superblock (`bytes_used`) und legt genau
so viel in npkFS ab. Whiteouts aus der Oberschicht übernimmt `mksquashfs`
unverändert, also löscht eine App auch eine Datei der Basis korrekt.
`squashfs-tools` gehört dafür in die Basis. Die Bau-VM braucht RAM für die
tmpfs-Oberschicht (Chromium ~300 MB, FreeCAD mehr) — das Rezept nennt eine
Grösse.

**Laufzeit:** `overlay: lower = app.sqfs:base.sqfs, upper = tmpfs`. Was die
App am System ändert, ist beim nächsten Start weg — gewollt. Was bleiben soll,
gehört in die App-Daten (§4b).

## 4b. Daten zwischen Gast und System

Heute (LibreWolf): `/dev/vda` = ext4-Profil, beim Schliessen GANZ gespeichert
(512 MiB im Wirts-RAM, Streaming in 1-MiB-Stücke); `npkhome` = 9P auf
**das ganze `home/<user>/`, lesend und schreibend**. Für einen Browser, den wir
selbst liefern, vertretbar — für jede App aus einem Store nicht. Eine App aus
`community` bekommt sonst jede Datei des Nutzers.

Drei Arten von Daten, drei Wege:

1. **App-Daten** (Profil, Einstellungen, Cache) — gehören nur der App.
   ext4-Abbild je App über virtio-blk, `$HOME` der App. Blockgerät statt 9P,
   weil Apps viele kleine Dateien und `fsync` schreiben und 9P dafür langsam
   ist. Für den Wirt undurchsichtig, und das ist richtig so.
   - **Wächst dynamisch** statt fester 512 MiB: kleiner Start, der Wirt
     vergrössert die Kapazität (virtio config change), PID 1 ruft
     `resize2fs` online.
   - **Wahlfreier Zugriff statt ganzem Abbild im RAM**: Lesen/Schreiben je
     1-MiB-Stück gegen npkFS, nur geänderte Stücke zurück. Das Ganze-im-RAM
     von heute skaliert nicht auf mehrere Apps gleichzeitig.
2. **Nutzerdateien** (Dokumente, Downloads) — gehören dem Nutzer, die App
   bekommt **Kapabilitäten auf Ordner**, nicht `home/`.
   - Je Freigabe ein eigenes 9P-Gerät mit eigenem Tag, gewurzelt am
     freigegebenen Ordner (der 9P-Server sperrt heute schon auf seine Wurzel),
     lesend oder lesend+schreibend.
   - Das Rezept **schlägt vor** (FreeCAD: `Dokumente` rw, Chromium:
     `Downloads` rw), der Nutzer bestätigt beim ersten Start, und er kann es
     jederzeit zurücknehmen. Ohne Freigabe sieht die App keine Nutzerdatei.
   - Später: **Dateiwahl über das System** (Portal) — die App fragt, loft zeigt
     den Dialog, freigegeben wird genau eine Datei. Das ist die saubere
     Kapabilitätsform und macht Ordnerfreigaben für die meisten Apps
     überflüssig. Nicht in der ersten Stufe.
3. **Flüchtiges** (Zwischenablage, Drag & Drop) — eigener Kanal über cage,
   nie über das Dateisystem. Offen (§9).

Freigaben werden bei jedem Start neu erteilt — sie können sich zwischen zwei
Starts geändert haben.

## 4c. Verwaltung: die App „Linux-Apps“

Ein kleines Systemprogramm (WASM-Modul wie loft), das als einziges die
Einstellungen der App-VMs ändern darf (eigene Kapabilität). Je App:

- Status (geprüft/experimentell), installierte Fassung, Basisfassung
- **Freigaben** auf Ordner (lesend / lesend+schreibend), zurücknehmbar
- **Netz:** aus · Internet · Internet + Heimnetz (Reichweite wie bei Seiten)
- **Ton, Mikrofon, Kamera** — einzeln
- **RAM** und **Plattenplatz** der App-Daten (Obergrenze)
- Starten, Beenden, neu bauen, entfernen (samt App-Daten auf Nachfrage)

**Woher die Werte kommen:** Vorgaben stehen im signierten Store-Eintrag
(von uns geprüft). Was der Nutzer ändert, liegt getrennt als **Übersteuerung**
in npkFS (`sys/microvm/apps/<id>/overrides`) und überlebt Store-Updates. Gilt:
Übersteuerung vor Vorgabe. RAM wird nach oben durch den freien Wirtsspeicher
begrenzt, nicht durch uns.

**Die Liste prüfen wir.** Jeder Eintrag wird von uns gegen das gepinnte
Alpine-Release gefahren und erst dann als geprüft freigegeben. Gepinnt wird
der **Zweig** (z. B. 3.24), nicht die Paketversion — Alpine hält alte
Versionen im Zweig nicht vor, und Sicherheitsupdates sollen ankommen. Folge:
ein `apk upgrade` innerhalb des Zweigs kann eine Version bringen, die wir nicht
gefahren haben. Das nehmen wir hin; beim Zweigwechsel wird die ganze Liste neu
geprüft.

## 5. Start und Steuerung

Gestartet wird kalt (§0.5). Ein Snapshot mit Fortsetzen war hier geplant —
RAM, vCPU und Gerätezustand in npkFS, faul geladen über NPT/EPT, Zufall über
PID 1 neu gesät, nur an Punkt „Basis bereit“, weil die App sonst ihren eigenen
Zufall geklont hätte und der Gast `acpi=off` bootet (kein vmgenid). Gemessen
hätte er am Notebook höchstens ~0,5 s gespart. **Verworfen**; die Begründung
steht in §0.5, damit niemand die Frage ohne neue Zahlen wieder aufmacht.

### 5.1 Steuerkanal Wirt ↔ PID 1

**Heute gibt es keinen.** Der Wirt spricht nur einmal, beim Boot, über die
Kommandozeile (`nopeektime=`, `nopeekbench=`); der Gast spricht nur Log über
`/dev/kmsg` und meldet sein Ende durch Ausschalten.

Gebraucht wird er für:

| Richtung | Meldung | wozu |
|---|---|---|
| Wirt → Gast | `start { app, freigaben, netz }` | App-Daten + Freigaben einhängen, App starten |
| Wirt → Gast | `quit` | App **sauber** beenden (SIGTERM, warten), dann `sync` — heute wird die App beim Schliessen hart abgeschnitten, vermutlich der Grund für das verlorene LibreWolf-Profil |
| Wirt → Gast | `disk_grown`, `grant_changed` | `resize2fs`, Freigabe aus-/einhängen |
| Gast → Wirt | `ready`, `first_frame` | Meilensteine für die Zeitachse (§0.7) |
| Gast → Wirt | `exited { code }`, `build { fortschritt, ergebnis }` | Verwaltung, Bau-VM |

**Entscheid: virtio-console** (ein Port, `/dev/hvc0`). `CONFIG_VIRTIO_CONSOLE=y`
ist im Gastkernel schon gesetzt, vsock nicht; vsock brächte Sockets auf
beiden Seiten, die wir für einen Partner nicht brauchen. Protokoll:
längenpräfigierte Meldungen mit Obergrenze, feste Typen, der Wirt verwirft
alles Unbekannte und beendet die VM bei einer Verletzung. **Nur PID 1 (root)
öffnet das Gerät** — die App läuft nicht als root (§6) und kommt nicht heran.

### 5.2 Eigener Compositor im Gast (npkwm)

**Gebaut (Kernel 0.524.0, microvm-init 0.5.0), Stufe 1 von 3.** cage ist
ersetzt durch `npkwm` (`microvm/linux/npkwm/`), C auf wlroots 0.20, aus
tinywl. Er läuft als eigener Nutzer `wl` (uid 1001), getrennt von der App
(`app`, 1000): nur `wl` öffnet den Port, nur `wl` bekommt über seatd GPU und
Eingabe, die App erreicht ihn nur über seinen Socket (`/tmp/wlrt`, setgid
`app`). PID 1 trägt ihn eingebettet und legt ihn nach `/tmp`; die sqfs bleibt
unverändert.

Verhalten wie cage: ein Fenster ohne Elter füllt den Ausgang (maximiert), ein
Dialog steht mittig in eigener Grösse, Popups werden an den Ausgang geklemmt,
die Dekoration bleibt beim Client.

**Fensterprotokoll** auf dem zweiten virtio-console-Port `npk.windows`,
gleiche Rahmen wie §5.1, höchstens 512 Bytes je Meldung, jede Nutzlast
beginnt mit dem Ausgang (`u8`):

| Richtung | Typ | Inhalt |
|---|---|---|
| Wirt → Gast | `0x01 close` | allen Fenstern ohne Elter auf dem Ausgang `xdg_toplevel.close` |
| Wirt → Gast | `0x02 focus` | der Nutzer hat das Fenster dieses Ausgangs fokussiert |
| Wirt → Gast | `0x03 resize` | `u16` Breite, `u16` Höhe: neue Kachelgrösse |
| Gast → Wirt | `0x81 hello` | Protokollfassung (3) |
| Gast → Wirt | `0x82 title` | UTF-8-Titel des obersten Fensters des Ausgangs |
| Gast → Wirt | `0x83 open` | die App hat ein Fenster geöffnet: ein Wirtsfenster für diesen Ausgang |
| Gast → Wirt | `0x84 closed` | auf dem Ausgang ist kein Fenster mehr |

Der Wirt bereinigt den Titel (ungültiges UTF-8 ersetzt, Steuerzeichen
entfernt, höchstens 200 Bytes) und zeigt ihn in der Bar
(`Window::caption`); der Fenstername, an dem das Dock Apps erkennt, bleibt.
Mod+Q schickt `close` statt Strg+Q zu tippen; die App endet wie über ihren
eigenen Schliessknopf (unter KVM: `sessionstore-final-state-write-complete`,
`rc=0` 0,2 s nach `close`). Danach wie gehabt 3 s → `MSG_QUIT`, 10 s → hart.

**Startzeit, und der Fund dabei.** Zwischen Compositor-Start und App-Start
lagen unter KVM ~0,75 s — bei cage genauso. npkwm selbst ist nach 11 ms
bereit; die Zeit lag **vor `main()`**: Alpines `libwlroots` bringt die
GLES2- und Vulkan-Renderer mit und über sie Mesa und **libLLVM (191 MB)**, und
musl löst jede Relokation beim Laden auf. Deshalb wird wlroots jetzt im
Bau selbst gebaut, nur mit DRM, libinput und pixman, und statisch gelinkt
(831 KB, keine Abhängigkeit mehr auf EGL/Mesa/LLVM). Danach bezahlte
LibreWolf dieselbe Rechnung: sein `glxtest` lädt Mesa und LLVM, um eine GPU
abzutasten, die es hier nicht gibt. `glxtest` und `vaapitest` sind jetzt
`/bin/false` (Software-Rendering ist ohnehin erzwungen).

KVM 9600X, 6 vCPUs, 1280×900, frisches Profil, je drei Läufe:

| | cage (0.523) | npkwm (0.524) |
|---|---|---|
| Compositor-Start → App-Start | ~0,72 s | 0,05 s |
| Start → Fenster steht | 3,55 s | 2,73–2,95 s |

Am Notebook offen; dort lag „cage → App“ schon bei ~0,2 s (§0.7), der
Gewinn kann also kleiner sein.

**Stufe 2 gebaut (Kernel 0.525.0, microvm-init 0.6.0): jedes App-Fenster
ein eigenes nopeek-Fenster** (Entscheid 2026-10-08: ein abgetrennter Tab
soll nicht das Hauptfenster überdecken). Ausgang k ist virtio-gpu-Scanout
k, der Wirt bietet 8 an (`MAX_OUTPUTS`). Ein neues Fenster ohne Elter
nimmt einen Ausgang, der da und leer ist (Ausgang 0 beim Start), sonst
fragt npkwm mit `open` nach einem und hält das Fenster versteckt, bis der
Ausgang kommt: der Wirt legt ein Fenster an und schaltet den Scanout zu
(Display-Ereignis). Dialoge bleiben beim Ausgang ihres Elters, Popups bei
ihrem Fenster. Mod+Q auf einem von mehreren Fenstern schliesst nur dieses
(`close` für seinen Ausgang); erst beim letzten läuft die Abfolge mit
`MSG_QUIT`. Meldet der Gast `closed`, schliesst der Wirt das Fenster, ausser
es ist das letzte (dann endet die App ohnehin).

**Der Zeiger trägt sein Fenster selbst:** x des Tablets ist in 8 gleiche
Abschnitte geteilt, der Abschnitt ist der Ausgang. Eine Zeigerbewegung
hängt so nie davon ab, dass eine `focus`-Meldung vorher ankommt; `focus`
setzt nur die Tastatur.

**Grösse über das Protokoll:** der Wirt schickt `resize`, npkwm setzt den
Modus selbst (eigener CVT-Modus). Weg sind die 150 ms Wartezeit, bis eine
Grösse ruht, und der 100-ms-Zyklus aus Trennen und Neuverbinden, den cage
brauchte, weil wlroots einen neuen Modus bei verbundenem Ausgang
ignoriert. Unter KVM: neue Grösse nach ~30 ms, LibreWolf neu ausgelegt
nach ~65 ms.

Unter KVM geprüft: `open`, verstecktes Fenster, `closed`, `resize`. Mehrere
sichtbare Fenster zeigt QEMU nicht (es schaltet keinen zweiten Ausgang
zu); das prüft der Gerätelauf.

**Weiter:**
- **Stufe 3 — Zwischenablage** über denselben Port, nur Text, nur auf
  Nutzeraktion.
- Mit v2 braucht das Grundgerüst weder cage noch Alpines wlroots, Mesa oder
  LLVM; die Bibliotheken von npkwm (libdrm, libinput, libudev, libseat,
  pixman, xkbcommon, wayland, libdisplay-info) gehören dann ausdrücklich in
  die Paketliste.

## 6. Sicherheit: wenn jemand in der Linux-Büchse ausbricht

**Grundannahme: der ganze Gast ist feindlich.** Eine App aus dem Store, ein
Exploit über eine Webseite, ein manipuliertes Paket — wir planen so, als hätte
der Angreifer **root im Gast**. Die Frage ist nur, was er dann hat.

### 6.1 Heute

- **Ring 1 steht (Kernel 0.523.0, microvm-init 0.4.35):** LibreWolf läuft
  als `app` (uid 1000), der Compositor seit 0.524 als `wl` (1001, §5.2),
  PID 1, seatd und der Steuerkanal bleiben root.
  Der Gastkernel hat `USER_NS`, und jeder Kindprozess läuft wie bei nativem
  Firefox mit seccomp in eigenem Nutzer- und Netz-Namensraum (Web Content,
  RDD, Utility, WebExtensions), unter KVM nachgezählt. Zwei Dinge hielten das
  auf: der Wechsel in die sqfs war ein `chroot`, und ein gechrooteter Prozess
  bekommt vom Kernel keinen Nutzer-Namensraum (jetzt `MS_MOVE` auf `/` wie
  `switch_root`); und `media.cubeb.sandbox=false` senkt die Inhalts-Sandbox
  von Stufe 4 auf 3. Beides ist weg, ebenso `MOZ_DISABLE_RDD/UTILITY_SANDBOX`.
  Ton mit voller Sandbox am Notebook bestätigt (YouTube, 2026-10-08).
- Ring 2 bleibt vorerst so (Entscheid 2026-10-08): `home/` bleibt freigegeben,
  bis die Dateiwahl über das System (Portal, §4b) kommt.
- root im Gast hat: das **ganze `home/<user>/` lesend und schreibend** (9P),
  Netz über NAT, Ton, Eingaben, Bildschirm.
- **Das ist heute das grösste Risiko, nicht der Ausbruch aus der VM.** Eine
  Webseite, die LibreWolf knackt, liest und löscht jede Datei des Nutzers.

### 6.2 Drei Ringe

**Ring 1 — im Gast: die App ist nicht root.**
- Gastkernel mit `MULTIUSER`, `SECCOMP_FILTER`, `USER_NS`/`PID_NS`/`NET_NS`,
  damit die Sandboxen von Chromium und Firefox überhaupt laufen.
- App läuft als eigener Nutzer, PID 1 und der Steuerkanal bleiben root.
- Ziel: ein Renderer-Fehler bleibt in der Sandbox der App, ein Fehler in der
  App bleibt beim App-Nutzer.

**Ring 2 — die VM-Grenze: root im Gast bekommt genau die Freigaben.**
- Das ist die eigentliche Zusage und muss auch gelten, wenn Ring 1 fällt.
- Kein 9P auf `home/`, nur freigegebene Ordner (§4b).
- Netz nur mit Kapabilität, Reichweite wie bei Seiten (Heimnetz nur auf
  ausdrückliche Freigabe). Der NAT des Wirts setzt das durch, nicht der Gast.
- Ton/Mikrofon/Kamera nur, wenn freigegeben — das Gerät existiert sonst gar
  nicht in der VM.
- Grenzen auf Wirtsseite: RAM, Platte, CPU-Anteil. Eine VM kann den Wirt
  nicht aushungern (dieselbe Regel wie `MAX_INSTANCE_BYTES` für Module).
- Die Bau-VM hat keine App-Daten, keine Freigaben, Netz nur zu Alpine.

**Ring 3 — der Wirt: der Ausbruch aus der VM.**
- Die Angriffsfläche ist **unser VMM, und der läuft in Ring 0 unseres
  Kernels**: Exit-Behandlung, `insn_decoder.rs`, alle virtio-Geräte (blk, net,
  9p, gpu, snd, input), NAT, PCI-Konfiguration, IOAPIC/PIC/PIT, MSR/CPUID.
  Ein Fehler dort ist kein Ausbruch in ein Modul, sondern **in den Kernel**.
  Das ist eine Ausnahme von Prinzip 5 (WASM als Vertrauensgrenze), und das
  Papier benennt sie, statt sie zu verschweigen.
- Massnahmen, kurzfristig:
  - **Weniger Fläche:** jede VM bekommt nur die Geräte, die ihre Freigaben
    brauchen. Keine Gerätemodelle „für alle Fälle“.
  - **Jede Zahl vom Gast ist feindlich:** Queue-Adressen, Längen, Indizes,
    9P-Pfade — durch eine Zugriffsschicht auf Gastspeicher mit
    Grenzprüfung (`guest_mem.rs`), kein roher Zeiger aus einer Gastzahl.
  - **Geräte host-seitig testbar machen und fuzzen** — wie
    `js/websocket.rs`: Logik auf Byte-Puffern, ohne VM laufbar.
  - **Seitenkanäle:** IBPB/L1D-Flush/VERW bei Eintritt und Austritt nach dem,
    was die CPU meldet — einzeln nachzuprüfen gegen das, was KVM tut.
  - **Verletzung = VM aus.** Eine unmögliche Anfrage wird nicht repariert,
    sondern beendet die VM, wird geloggt und in der Verwaltung angezeigt.
- Langfristig (eigenes Papier): Geräteemulation aus Ring 0 heraus, in eine
  isolierte Komponente mit nur ihren Kapabilitäten — das Gegenstück zu
  Firecrackers Jailer. Offen, ob das mit dem Datenpfad (Netz 1,9 Gbit) geht.

### 6.3 Was sonst fremde Daten sind

- **Plattenstücke** und die `.hot`-Liste: kommen aus npkFS und sind von diesem
  Wirt geschrieben (AES-GCM); Offsets des Gasts werden gegen die Abbildlänge
  geprüft, ein unlesbares Stück lässt die Platte mit IOERR scheitern statt
  Nullen zu liefern.
- **Steuerkanal:** §5.1.
- **Store:** von uns signiert. **App-Schichten** signieren wir nicht (sie
  entstehen beim Nutzer) — apk prüft gegen die Alpine-Schlüssel. Ein
  manipuliertes Paket ist dann Ring 1/2, nicht Ring 3.

## 7. Speicher und Platte

- Release schrumpft um die App-Bundles. `release/apps` gehört danach nicht mehr
  in den OTA-Weg; nur Basis + Store-Liste.
- Beim Nutzer: Basis einmal, App-Schichten je App, App-Daten je App. Eine
  Aufräumfunktion löscht Schichten ohne Rezept oder mit veralteter Basis.
- Gelesene Plattenstücke liegen im Wirts-RAM, solange die VM läuft (die sqfs
  fast ganz, gemessen 243 von 253 MiB).

## 8. Reihenfolge

Der Prototyp aus §0 ist gelaufen; der Start ist §0.6, die App §0.7.

0. **Gast härten** (Ring 1) und **9P auf `home/` durch Ordnerfreigaben
   ersetzen** (Ring 2) — gilt schon für LibreWolf heute und ist das grösste
   offene Risiko.
1. **Steuerkanal** (virtio-console) + PID-1-Protokoll; erster Nutzen: `quit`
   beendet die App sauber.
2. **App-Schichten über apk:** Rezept, Bau-VM, sqfs, App-Daten je App. Chromium
   als erster Eintrag.
3. **Verwaltung „Linux-Apps“** mit Vorgaben aus dem Store und Übersteuerung.
4. **App-Start** nach dem Befund aus §0.7.
5. **Store-Oberfläche**; danach fällt das LibreWolf-Bundle weg.

Ring 3 (Fläche verkleinern, Geräte fuzzen) läuft neben jeder Stufe mit.
Jede Stufe wird auf AMD **und** Intel gefahren (Lehre aus
`MICROVM_NET_REBUILD.md`).

## 9. Offen

- Zwischenablage und Drag & Drop zwischen App-VM und System.
- Geräteemulation aus Ring 0 heraus — Machbarkeit und Kosten (eigenes Papier).
- Welche Seitenkanal-Massnahmen beim VM-Wechsel heute schon greifen —
  nachmessen.

## 10. Entschieden

- **v2 ist ein Neustart** über der bestehenden CPU-Ebene (§0.1).
- **Firecracker als Konzept** (wenige Geräte), **Jailer nicht jetzt**, aber
  Gerätemodell jailer-fähig (§0.2).
- **Kein Snapshot** — gemessen, zu wenig Gewinn für das Risiko (§0.5).
- **Je App eine eigene VM.**
- **App-Schicht = sqfs**, gebaut in der Bau-VM (§4a).
- **Keine Ordnerfreigabe ohne Zustimmung**; `home/` als Ganzes bekommt keine
  App (§4b).
- **Steuerkanal = virtio-console**, nur PID 1 hat Zugriff (§5.1).
- **Eigener Compositor** (npkwm, C auf wlroots, statisch und ohne GL) statt
  cage; Fensterprotokoll auf `npk.windows`; jedes App-Fenster wird ein
  eigenes nopeek-Fenster; die Grösse geht über das Protokoll (§5.2).
- **Platten bei Bedarf aus npkFS**, im Hintergrund vorab geladen (§0.6).
- **Verwaltung „Linux-Apps“:** Vorgaben aus dem signierten Store,
  Übersteuerung durch den Nutzer, auch beim RAM (§4c).
- **Die Liste prüfen wir**, gepinnt auf den Alpine-Zweig (§4c).
- **Grundannahme: root im Gast ist feindlich**; die Zusage ist, dass er nur die
  Freigaben bekommt (§6).
- **LibreWolf** bleibt, bis v2 bewiesen ist, dann fällt das Bundle weg. Ob
  jemand Linux-Apps nutzt, entscheidet der Nutzer — die Basis ist optional.
  Kein Migrationscode fürs Profil.
- **Chromium** statt Chrome; kein glibc/Flatpak in v2.
