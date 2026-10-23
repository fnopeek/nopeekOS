# Keystore / Vault — Plan-Paper

**Stand:** 2026-10-23 · Entwurf, noch nicht entschieden · keine Zeile Code
geschrieben

**Herkunft:** Florian-Wunsch 2026-06-07 (beim wifid-Bringup: „kein Fan davon,
den WLAN-Key im Klartext zu speichern — wir brauchen wohl sowas wie ein
keystore"), neu aufgegriffen 2026-10-23: „zukünftig werden wir Kennwörter etc.
sowas wie ein Vault bauen, und nicht im Filesystem als File ablegen". Löst den
Audit-Fund **F-M3** aus `NPKFS_SECURITY_REVIEW.md` (WLAN-PSK welt-lesbar) und
ersetzt die heute nur *behauptete* Zusage in `docs/spec/WIFI_CLASS_ABI.md` §6
(„nur `wifid` liest den PSK") durch einen echten Mechanismus.

**Leitsatz:** Ein Geheimnis ist keine Datei. Es darf nicht über den generischen
FS-Pfad erreichbar sein, sondern nur über eine Zugriffskontrolle, die an eine
Capability (und optional an den Login) gebunden ist.

---

## 0. Stand heute — was ist, was fehlt

- **At-rest IST verschlüsselt.** npkFS AEAD-wrappt jedes Objekt (AES-256-GCM,
  Key aus dem Content-Hash) — auf der SSD liegt `sys/config/wifi_psk` nicht im
  Klartext. Das ist NICHT die Lücke.
- **Die Lücke ist die Zugriffskontrolle am Host-Rand.** Jedes WASM-Modul mit dem
  Default-Cap `READ` ruft `npk_fetch("sys/config/wifi_psk")` und bekommt den
  entschlüsselten Klartext. `npk_fs_list` versteckt `sys/`/`.system/` nicht (F-M3).
  Ein Browser-Bug o. Ä. liest damit WLAN-Passwörter. Es gibt KEIN per-Geheimnis-
  Gating.
- **`NETCTL` existiert, schützt aber das Falsche.** Der Cap-Bit `NETCTL`
  (`capability.rs:47`, geprüft in `host_core.rs:4286/4297` u. a.) gated die
  WiFi-*Steuerung* (scan, connect, EAPOL-Pfad) — NICHT die PSK-*Lesung*. Die
  PSK-Lesung ist ein gewöhnliches `npk_fetch` und kennt `NETCTL` nicht.
- **Zusätzliche Schreib-Lücke (F-M4 / Fund-2-Klasse):** die `own_config`-Ausnahme
  (`host_core.rs:2403`) lässt ein Modul namens `wifi`/`wifi_psk`
  `sys/config/wifi_psk` OHNE WRITE-Cap schreiben.
- **Dokumentierte Absicht ≠ Mechanismus.** `WIFI_CLASS_ABI.md` §6 sagt „nur
  `wifid` liest es" und §1-Tabelle führt den Klartext-PSK als etwas, das der
  *Treiber nie sieht*. Beim Treiber stimmt das (er bekommt nur fertige PTK/GTK
  via SET_KEY); beim *generischen FS-Zugriff* stimmt es nicht. Der Keystore macht
  die Absicht wahr.

**Fazit:** Es ist ein reines Zugriffskontroll-Problem am Host-ABI, kein
Krypto-at-rest-Problem. Die Bausteine sind da (AES-256-GCM, Capability-System);
es fehlt der *gated Pfad* dorthin.

---

## 1. Anforderungen

1. **Ein Geheimnis ist über den generischen FS-Pfad NICHT erreichbar.** Kein
   `npk_fetch`/`npk_fs_list`/`npk_fs_read` sieht je ein Secret — auch nicht mit
   globalem READ. Das ist die harte Grenze.
2. **Zugriff ist Capability-gegated**, pro Geheimnis-Klasse. Nur der berechtigte
   Manager liest (WLAN-Secrets: nur `wifid` via `NETCTL`/eine neue
   `KEYSTORE`-Right).
3. **Schreiben ist ebenso gegated** — die `own_config`-Namensfalle (F-M4) darf
   nicht auf Secrets greifen.
4. **Enumeration verrät nichts.** Ein Modul darf nicht einmal *sehen*, welche
   Secrets existieren, wenn es sie nicht lesen darf (Listung blendet den
   Keystore vollständig aus — nicht nur die Werte).
5. **Einziger Nutzer, kein Migrationscode** (Projekt-Policy): die bestehende
   `sys/config/wifi_psk` wird einmal per `wlan`-Intent neu provisioniert, nicht
   migriert. **Kein Reformat** (siehe §5).
6. **Boot-Tauglichkeit bedenken:** WLAN muss ggf. VOR dem Login verbinden (OTA,
   Zeit). Ein login-gebundener Vault darf den Netz-Bringup nicht verklemmen
   (§4, Spannungsfeld).

---

## 2. Designoptionen (noch nicht entschieden)

### Option A — gegateter Pfad-Präfix `sys/keystore/*`
Ein reservierter Präfix, den `npk_fetch`/`npk_fs_list`/`npk_store`/`delete` nur
mit einer `KEYSTORE`-Right (oder klassenspezifisch, z. B. `NETCTL` für
`sys/keystore/net/*`) freigeben — analog wie `sys/wasm/` heute
schreibgeschützt ist (`is_trust_critical_path`).
- **+** Minimaler Eingriff, nutzt den vorhandenen Gating-Mechanismus.
- **+** Ein Ort, eine Regel.
- **−** Der generische FS-Pfad kennt das Geheimnis weiterhin *als Pfad* — die
  Grenze ist eine `if`-Prüfung, kein struktureller Riegel. Ein vergessener
  Zugangsweg (neue Host-Fn) leakt wieder. (Genau diese Klasse Fehler ist F-M3.)

### Option B — dedizierte Host-Fns `npk_secret_get/put(class, name)` (empfohlene Richtung)
Secrets liegen in einem Namensraum, den der generische FS-Layer GAR NICHT
auflöst; der einzige Zugang sind cap-gegatete Host-Fns.
- `npk_secret_put(class, name, bytes)` — braucht die Schreib-Right der Klasse.
- `npk_secret_get(class, name) -> bytes` — braucht die Lese-Right der Klasse.
- `class` bindet an eine Capability-Right: `net` → `NETCTL`, später `app` →
  eine per-App-Right, etc.
- **+** Struktureller Riegel: kein FS-Pfad führt je zum Geheimnis, also kann
  keine künftige FS-Host-Fn es versehentlich freilegen. Das ist die robuste
  Antwort auf F-M3 (dort war die Lücke genau ein generischer Pfad).
- **+** Enumeration ist von Natur aus dicht (kein Pfad zum Listen).
- **−** Neue ABI-Fläche (zwei Host-Fns + Rights-Mapping), mehr als ein `if`.

> **Empfehlung:** B als Ziel, intern auf einem reservierten npkFS-Präfix
> gespeichert (z. B. `sys/keystore/<class>/<name>`), der über den generischen
> Pfad NIE aufgelöst wird (A als Speicher-Detail, B als einziges Tor). So ist das
> Geheimnis strukturell unerreichbar und die Speicherung bleibt simpel.

### Option C — login-gebundener Wrap (zusätzlich, nicht alternativ)
Secrets zusätzlich unter einem Schlüssel verpackt, der erst aus der
User-Authentifizierung ableitbar ist — vor dem Login nicht entschlüsselbar.
- **+** Stärkste Stufe: ein Kompromittieren vor dem Login (oder Offline-Zugriff,
  vgl. Rollback-Fund §3 im Security-Review) liest die Secrets nicht.
- **−** Spannung mit §1.6: WLAN-Verbindung vor dem Login (OTA) braucht den PSK,
  bevor ein User authentifiziert ist. Auflösung denkbar: Netz-Secrets in einer
  Klasse, die an einen *Geräte*-Schlüssel gebunden ist (verfügbar beim Boot),
  App-/Nutzer-Secrets an den *Login*-Schlüssel. Zwei Vertrauensstufen.
- Entscheidung später; C baut auf B auf.

---

## 3. Betroffene Stellen (wenn gebaut)

- **Die EINE Lesestelle heute:** `wifid` holt den PSK über `npk_fetch` aus
  `sys/config/wifi_psk`. Das ist der einzige Punkt, der auf
  `npk_secret_get("net", ssid)` umgestellt wird (`WIFI_CLASS_ABI.md` §6 markiert
  ihn bereits als die Stelle).
- **Die Schreibstelle:** der `store /sys/config/wifi_psk <pass>`-Intent
  (`intent/net.rs:288`, `intent/system.rs:1687`, Hilfetext) → neuer
  `wlan`-Intent-Unterbefehl, der `npk_secret_put("net", …)` ruft.
- **`intent/history.rs:21`** führt `sys/config/wifi_psk` schon als „zu heiß für
  die Platte" (aus dem Verlauf gefiltert) — der Filter bleibt, der Pfad ändert
  sich.
- **Host-ABI:** neue Fns in `host_core.rs` + beide ABI-Wege; `KEYSTORE`-Right
  (oder Wiederverwendung `NETCTL` für die net-Klasse) in `capability.rs`.
- **`is_trust_critical_path`** (`wasm.rs:1238`): der Keystore-Präfix wird dort
  (oder im FS-Resolver) als „nicht über den generischen Pfad auflösbar"
  verankert — zusammen mit dem F-H3-Fix (`.system`/`sys/config`), der ohnehin
  ansteht.

---

## 4. Spannungsfeld Boot vs. Login

WLAN verbindet heute teils vor dem Login (OTA, NTP). Ein rein login-gebundener
Vault (Option C) würde das verklemmen. Richtung: **zwei Secret-Klassen mit zwei
Wrap-Keys** — `net`/Geräte-Secrets an einen beim Boot verfügbaren
Geräteschlüssel (heute: der npkFS-`master_key`), `app`/Nutzer-Secrets an den
Login. So bleibt der Netz-Bringup möglich, während App-Passwörter erst nach Auth
lesbar sind. Nicht entschieden; hier nur benannt, damit C nicht naiv gebaut wird.

---

## 5. On-Disk-Impact — kein Reformat

Der Keystore ist **Code-only** (neue Host-Fns + gegateter Präfix + Right). Das
npkFS-Format ändert sich nicht — ein Secret ist weiterhin ein AEAD-Objekt, nur
an einem Pfad, den der generische Layer nicht auflöst. Bestehende Installation
läuft weiter (normaler Kernel-`update`). Die eine bestehende
`sys/config/wifi_psk` wird **nicht migriert**, sondern einmal per `wlan`-Intent
neu gesetzt (Projekt-Policy: ein Nutzer, kein Migrationscode) und die alte Datei
gelöscht. Das deckt sich mit `NPKFS_SECURITY_REVIEW.md` §8 (F-M3-Zeile:
„berührt nur die PSK-Datei, kein Reformat").

Login-Wrap (Option C) bleibt ebenfalls Code-only, solange keine neuen
persistenten Header-Felder nötig werden.

---

## 6. Offene Entscheidungen

1. Option A vs. B (Empfehlung: B mit A als Speicherdetail).
2. Eigene `KEYSTORE`-Right oder Wiederverwendung `NETCTL` für die net-Klasse?
3. Option C jetzt mitdenken (zwei Wrap-Keys) oder später?
4. Namensraum: `sys/keystore/<class>/<name>` oder flach pro Klasse?
5. Reicht WLAN v1 (eine net-Klasse), oder von Anfang an app-Secrets mitdenken
   (Browser-Passwörter, vgl. `beak`-Cookies)?

---

## 7. Querverweise

- `NPKFS_SECURITY_REVIEW.md` §5 F-M3 (PSK welt-lesbar), F-M4 (Picker-Grant),
  Fund 2 (`own_config`-Ausnahme) — die Lücken, die der Keystore schließt.
- `docs/spec/WIFI_CLASS_ABI.md` §1 (Rollentabelle), §6 (Credential-Speicher) —
  die Zusage, die der Keystore erzwingbar macht.
- Memory `project_keystore.md` — die ursprüngliche Skizze (2026-06-07).
