# microVM v2 — Grundgerüst, App-Schichten aus apk, Snapshot statt Boot

*Diskussionspapier, 2026-10-07. Richtung entschieden, Snapshot hängt am Prototyp (§0), Details offen (§9).*

## Idee in drei Sätzen

Wir liefern nur noch ein **Grundgerüst** (Kernel, Alpine-Basis, unser PID 1,
cage). Was der Nutzer haben will, holt seine Maschine **einmalig aus den
offiziellen Alpine-Quellen** in eine eigene App-Schicht — kuratiert über einen
Store. Danach wird nicht mehr gebootet, sondern ein in npkFS abgelegter
**Snapshot fortgesetzt**, und das in Millisekunden.

## 0. Erst beweisen, dann bauen

v2 ist ein **Neustart**, nicht ein Umbau von v1. Bevor irgendetwas vom Store
gebaut wird, beantwortet ein Prototyp genau eine Frage: **spart Fortsetzen aus
npkFS gegenüber einem guten Kaltstart überhaupt Zeit?**

### 0.1 Was neu ist und was bleibt

- **Neu:** Gerätemodell, Lebenszyklus einer VM, Speicherverwaltung des Gasts,
  Steuerkanal, Snapshot.
- **Bleibt:** die CPU-Ebene (`cpu/svm`, `cpu/vmx`: VMCB/VMCS, Intercepts,
  MSR/CPUID/XCR0-Härtung, Interrupt-Injektion, HLT-Behandlung). Sie ist auf
  beiden Herstellern am Blech erarbeitet und im Code-Review (D12-D24)
  geprüft — sie neu zu schreiben heisst, dieselben Wände noch einmal zu
  finden.

### 0.2 Firecracker als Konzept, Jailer als Vorbereitung

- **Firecracker übernehmen wir als Konzept, ab Tag 1:** so wenige Geräte wie
  möglich, kein ACPI (haben wir schon: `acpi=off`), jeder Gerätezustand von
  Anfang an speicherbar. Das ist keine zusätzliche Komplexität, sondern die
  Voraussetzung für den Snapshot — jedes Gerät ist ein Zustand, der mit muss.
  Firecracker selbst hat kein GPU, Ton und Eingabe; die kommen bei uns dazu,
  aber erst nach dem Beweis.
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
  Gerätezustandsfrage (§5.4) fällt weg.
- Unabhängig davon zeigt M0, wo die heutigen Sekunden wirklich liegen. Liegt
  der Grossteil im **App-Start** (LibreWolf, Chromium), spart kein Snapshot an
  Punkt A ihn ein — dann ist das ehrliche Ergebnis, dass der Gewinn des Stores
  die Apps sind, nicht die Startzeit.

## 1. Warum

| heute (v1) | v2 |
|---|---|
| `release/apps` 3,8 GB, davon LibreWolf 3,4 GB, jede Fassung als eigener Satz `cpio.gz` + `sqfs` | Release trägt nur das Grundgerüst (Grössenordnung `alpine-wayland` 16 MB + cage) |
| eine App: der Browser | jede App, die im Store steht |
| jede App-Aktualisierung = unser Release | Aktualisierung = `apk upgrade` gegen Alpine, Rezept bleibt gleich |
| Start = Linux bootet | Start = Snapshot fortsetzen |

**Ehrlich benannt:** gewonnen wird **Startzeit** und **Speicher** (geteilte
Seiten). Die Laufzeitleistung einer App ändert sich dadurch nicht.

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
- **Snapshot** — RAM + vCPU + Gerätezustand einer laufenden Maschine an einem
  definierten Punkt (§5.6).

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
5. Optional sofort: erster Start + Snapshot (§5).

**Vertrauensmodell:** Der Wirt vertraut dem Inhalt der Schicht **nichts** — sie
läuft nur je in einer VM, und die VM ist die Grenze (Prinzip 5). Die Signatur
schützt den Nutzer vor einem manipulierten Spiegel, nicht den Wirt vor der App.
Daraus folgt: die Signaturprüfung im Gast genügt.

**Basis ändert sich** → alle App-Schichten und Snapshots darauf sind ungültig →
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

**Zusammenhang mit dem Snapshot:** Punkt B (§5.6) liegt VOR dem Einhängen von
App-Daten und Freigaben. Damit enthält kein Snapshot offene 9P-fids oder
Seiten aus App-Daten, und Freigaben werden bei jedem Start neu erteilt — sie
können sich zwischen zwei Starts geändert haben.

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

## 5. Snapshot statt Boot

### 5.1 Was gespeichert wird

- Gast-RAM, seitenweise (4 KiB), **Nullseiten als Loch** — nicht als
  verschlüsselte Nullen (AES-GCM macht aus Nullen Rauschen). Das
  Seitenverzeichnis vermerkt „leer“ ausdrücklich.
- vCPU-Zustand (VMCB/VMCS-Felder, GPR, FPU/XSAVE, MSRs, LAPIC).
- Gerätezustand **jedes** Geräts: virtqueues (Indizes, Adressen), virtio-blk,
  -net, -gpu (Ressourcen + Scanout), -snd, -input, -9p, IOAPIC, PIC, PIT.
- Basisfassung + App-Schicht-Hash, gegen die der Snapshot gilt.

### 5.2 Teilen über Inhaltsadressierung

npkFS adressiert nach Inhalt — zwei Snapshots auf derselben Basis teilen
gleiche Seiten ohne eigenen Mechanismus. Das ist der Grund, warum „die 0 Bytes
gratis“ sind und die übrigen fast.

### 5.3 Fortsetzen in Millisekunden — faul laden

1 GB RAM vorab lesen + entschlüsseln kostet hunderte ms. Deshalb:

- NPT/EPT zunächst leer; der erste Zugriff auf eine Seite → Fehler → Seite aus
  npkFS holen, einsetzen, weiter (Prinzip wie Firecracker/UFFD).
- Loch → frische Nullseite, kein Plattenzugriff.
- Vorab nur die **Arbeitsmenge** des letzten Starts (beim Snapshot mitgemessen),
  Rest faul.
- Mehrere Klone derselben Basis: gemeinsame Seiten **copy-on-write**.

### 5.4 Identität und Zufall — Pflicht

Wer denselben Snapshot zweimal fortsetzt — zwei Klone, oder einfach zweimal
dieselbe App an zwei Tagen —, startet zweimal mit **demselben Zustand des
Zufallsgenerators** → gleiche TLS-Schlüssel und Nonces. Das ist der
sicherheitskritische Teil des Snapshots.

**vmgenid geht bei uns nicht direkt:** es ist ein ACPI-Gerät, und unser Gast
bootet `acpi=off` mit MP-Tabelle (`linux/mptable.rs`). Stattdessen:

- Snapshot **nur an Punkt A** (§5.6): der Kernel ist oben, PID 1 wartet, **noch
  kein Prozess, der Zufall verbraucht hat**, keine Verbindung offen.
- Beim Fortsetzen schickt der Wirt über den Steuerkanal (§5.7) frischen Zufall
  aus `npk_random_bytes`. PID 1 speist ihn mit `RNDADDENTROPY` ein und erzwingt
  `RNDRESEEDCRNG`, **bevor** es irgendetwas anderes startet.
- Neue `/etc/machine-id` je Start.
- Punkt B (App bereits gestartet) ist damit **ausgeschlossen**: die App hätte
  eigenen Zufall im Userspace (BoringSSL, NSS), den kein Neusäen des Kernels
  erreicht.

### 5.5 Zeit

Nach dem Fortsetzen ist die Gastuhr um die Ruhezeit falsch.

- TSC-Versatz so setzen, dass der Gast-TSC monoton weiterläuft.
- PID 1 stellt die Uhr (`clock_settime`) aus der Meldung `resumed` — derselbe
  Wert, den heute `nopeektime=` auf der Kommandozeile mitbringt, nur dass die
  Kommandozeile nach einem Snapshot schon gelesen ist.

### 5.6 Wo der Snapshot gezogen wird

| Punkt | Vorteil | Nachteil |
|---|---|---|
| **A: Basis bereit** (Kernel oben, PID 1 wartet) | einer für alle Apps, Zufall sauber neu säbar | App startet kalt |
| B: App gestartet | App sofort da | Zufall der App geklont — **nicht** |
| C: App warm mit Sitzung | am schnellsten | dazu Sitzungszustand — **nicht** |

**Nur A.** Ein Snapshot für alle Apps; was die App danach braucht, ist ihr
gewöhnlicher Kaltstart (Chromium ~1-2 s) statt Kernel + Basis + App.

### 5.7 Steuerkanal Wirt ↔ PID 1

**Heute gibt es keinen.** Der Wirt spricht nur einmal, beim Boot, über die
Kommandozeile (`nopeektime=`, `nopeekbench=`); der Gast spricht nur Log über
`/dev/kmsg` und meldet sein Ende durch Ausschalten.

Gebraucht wird er für:

| Richtung | Meldung | wozu |
|---|---|---|
| Wirt → Gast | `resumed { zeit, zufall, machine_id }` | §5.4, §5.5 |
| Wirt → Gast | `start { app, freigaben, netz }` | App-Daten + Freigaben einhängen, App starten |
| Wirt → Gast | `quit` | App **sauber** beenden (SIGTERM, warten), dann `sync` — heute wird die App beim Schliessen hart abgeschnitten, vermutlich der Grund für das verlorene LibreWolf-Profil |
| Wirt → Gast | `disk_grown`, `grant_changed` | `resize2fs`, Freigabe aus-/einhängen |
| Gast → Wirt | `ready` | Punkt A erreicht → Snapshot |
| Gast → Wirt | `exited { code }`, `build { fortschritt, ergebnis }` | Verwaltung, Bau-VM |

**Entscheid: virtio-console** (ein Port, `/dev/hvc0`). `CONFIG_VIRTIO_CONSOLE=y`
steht schon in `nopeek-tiny.config`, vsock nicht; vsock brächte Sockets auf
beiden Seiten, die wir für einen Partner nicht brauchen. Protokoll:
längenpräfigierte Meldungen mit Obergrenze, feste Typen, der Wirt verwirft
alles Unbekannte und beendet die VM bei einer Verletzung. **Nur PID 1 (root)
öffnet das Gerät** — die App läuft nicht als root (§6) und kommt nicht heran.

## 6. Sicherheit: wenn jemand in der Linux-Büchse ausbricht

**Grundannahme: der ganze Gast ist feindlich.** Eine App aus dem Store, ein
Exploit über eine Webseite, ein manipuliertes Paket — wir planen so, als hätte
der Angreifer **root im Gast**. Die Frage ist nur, was er dann hat.

### 6.1 Heute

- LibreWolf läuft **als root** und **ohne Sandbox** — `nopeek-tiny.config`
  kommt von `tinyconfig` und hat weder `SECCOMP` noch Namespaces noch
  `MULTIUSER` (in der Datei nicht gesetzt; zu prüfen, ob der gebaute Kernel
  dieselbe ist). Ein Fehler im Renderer = root im Gast.
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

- **Snapshot-Dateien:** nur lesen, was dieser Wirt geschrieben hat
  (AES-GCM-Tag in npkFS), Gerätezustand beim Laden auf Wertebereiche prüfen.
- **Steuerkanal:** §5.7.
- **Store:** von uns signiert. **App-Schichten** signieren wir nicht (sie
  entstehen beim Nutzer) — apk prüft gegen die Alpine-Schlüssel. Ein
  manipuliertes Paket ist dann Ring 1/2, nicht Ring 3.

## 7. Speicher und Platte

- Release schrumpft um die App-Bundles. `release/apps` gehört danach nicht mehr
  in den OTA-Weg; nur Basis + Store-Liste.
- Beim Nutzer: Basis einmal, App-Schichten je App, ein Snapshot (Punkt A).
  Eine Aufräumfunktion löscht Schichten ohne Rezept oder mit veralteter Basis.

## 8. Reihenfolge

Vor allem anderen: **der Prototyp aus §0** und der Entscheid §0.4. Die Stufen
4 und 5 hängen davon ab.

0. **Gast härten** (Ring 1) und **9P auf `home/` durch Ordnerfreigaben
   ersetzen** (Ring 2) — gilt schon für LibreWolf heute und ist das grösste
   offene Risiko.
1. **Steuerkanal** (virtio-console) + PID-1-Protokoll; erster Nutzen: `quit`
   beendet die App sauber.
2. **App-Schichten über apk:** Rezept, Bau-VM, sqfs, App-Daten je App. Chromium
   als erster Eintrag.
3. **Verwaltung „Linux-Apps“** mit Vorgaben aus dem Store und Übersteuerung.
4. **Snapshot/Restore an Punkt A, vorab geladen:** Gerätezustand, Neusäen,
   TSC-Versatz.
5. **Faules Laden** über NPT/EPT-Fehler, Löcher, Arbeitsmenge vorab; mehrere
   VMs teilen die Seiten des Snapshots copy-on-write.
6. **Store-Oberfläche**; danach fällt das LibreWolf-Bundle weg.

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
- **Firecracker als Konzept** (wenige Geräte, Zustand speicherbar), **Jailer
  nicht jetzt**, aber Gerätemodell jailer-fähig (§0.2).
- **Erst der Prototyp** M0-M3, dann der Entscheid über den Snapshot (§0.4).
- **Je App eine eigene VM.**
- **App-Schicht = sqfs**, gebaut in der Bau-VM (§4a).
- **Keine Ordnerfreigabe ohne Zustimmung**; `home/` als Ganzes bekommt keine
  App (§4b).
- **Steuerkanal = virtio-console**, nur PID 1 hat Zugriff (§5.7).
- **Snapshot nur an Punkt A**; Zufall wird über PID 1 neu gesät (§5.4).
- **Verwaltung „Linux-Apps“:** Vorgaben aus dem signierten Store,
  Übersteuerung durch den Nutzer, auch beim RAM (§4c).
- **Die Liste prüfen wir**, gepinnt auf den Alpine-Zweig (§4c).
- **Grundannahme: root im Gast ist feindlich**; die Zusage ist, dass er nur die
  Freigaben bekommt (§6).
- **LibreWolf** bleibt, bis v2 bewiesen ist, dann fällt das Bundle weg. Ob
  jemand Linux-Apps nutzt, entscheidet der Nutzer — die Basis ist optional.
  Kein Migrationscode fürs Profil.
- **Chromium** statt Chrome; kein glibc/Flatpak in v2.
