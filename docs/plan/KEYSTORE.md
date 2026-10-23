# Keyvault — Design-Paper

**Stand:** 2026-10-23 · **Architektur beschlossen** (Diskussion mit Florian) ·
noch keine Zeile Code

**Was es ist:** ein kleiner Passwort-Manager mit GUI und zwei Berechtigungs-
Tiers. Löst Audit-Fund **F-M3** aus `NPKFS_SECURITY_REVIEW.md` (WLAN-PSK als
welt-lesbare Datei) und macht die bislang nur *behauptete* Zusage in
`docs/spec/WIFI_CLASS_ABI.md` §6 („nur `wifid` liest den PSK") zu einem echten
Mechanismus. Ersetzt `sys/config/wifi_psk`.

**Leitsatz:** Ein Geheimnis ist keine Datei. Es ist nie über den generischen
FS-Pfad erreichbar, sondern nur über den Vault, der es nach Policy herausgibt.

---

## 1. Zwei Tiers, zwei Autorisierungen

| Tier | Inhalt | Entsperrt durch | Zugriff beim Benutzen |
|---|---|---|---|
| **System** | WLAN-PSK, später VPN/Enterprise-Certs | **Geräteschlüssel** (automatisch nach Disk-Unlock) | **Capability** (z.B. `NETCTL`) |
| **User** | Website-Passwörter, Notizen | **separates Vault-Passwort** (nicht das Login) | Vault-Session (Passwort eingegeben) |

Die zwei Tiers sind **kein Timing-Thema, sondern reine Autorisierung.** Der
System-Tier hängt am Geräteschlüssel, damit autostart-Tools wie `wifid` das WLAN
nutzen können, **ohne dass der Nutzer je ein Vault-Passwort tippt**. Der
User-Tier verlangt explizites Entsperren (Password-Manager-Semantik).

---

## 2. Ein File, zwei kryptografische Kammern

Der Vault ist **EIN** npkFS-Objekt an einem trust-critical Pfad (`sys/vault`,
geschützt wie `sys/wasm` — nur das Vault-Modul darf ihn roh lesen). Das Keyslot-
Muster ist dasselbe, das npkFS für die Platte schon nutzt (`crypto/keyslot.rs`,
`security/disk_key.rs`):

```
sys/vault  (ein npkFS-Objekt)
├─ Header
│   ├─ Keyslots für System-DEK:  [Geräteschlüssel]  [Vault-Passwort]   ← zwei Wege rein
│   └─ Keyslots für User-DEK:    [Vault-Passwort]                      ← ein Weg rein
├─ System-Einträge  → AEAD(System-DEK)   nutzen = Cap, anzeigen = Passwort
└─ User-Einträge    → AEAD(User-DEK)     alles = Vault-Passwort (+ opt. Cap)
```

**Die tragende Eigenschaft:** der Geräteschlüssel entsperrt **nur** die
System-DEK, **nie** die User-DEK. Das Gerät kann also das WLAN-Passwort nutzen,
ohne je die User-Secrets lesen zu können. „Ein File" heißt deshalb **nicht** „ein
Schlüssel öffnet alles" — das File hat innere Schotten. Zwei DEKs (nicht eine),
weil eine gemeinsame DEK die Tier-Trennung aufhöbe.

**Migration (der Grund für ein File):**
- **User-Tier:** File kopieren, Vault-Passwort eingeben — der Passwort-Keyslot ist
  maschinenunabhängig. Fertig.
- **System-Tier:** der Geräteschlüssel-Slot ist auf der neuen Maschine tot. Aber
  die System-DEK hat einen **zweiten Slot aufs Vault-Passwort**: beim ersten
  Entsperren auf der neuen Maschine wird die System-DEK übers Passwort
  entschlüsselt und **unter den neuen Geräteschlüssel neu gewickelt**. Einmal
  Passwort auf der neuen Kiste → WLAN wieder boot-fähig.

---

## 3. Timing (korrigiert)

Nichts Netzfähiges läuft vor dem Disk-Unlock — das ist Absicht. Ablauf:
**Platte entsperren → Login → autostart lädt `wifid` → Verbindung.** Es gibt
**kein Pre-Login-Netz.**

Der **Geräteschlüssel** ist ein **domänen-getrennter Subkey aus dem
npkFS-Disk-Masterkey** (HKDF/BLAKE3-Ableitung mit Label „vault-system"). Er ist
nach dem Disk-Unlock verfügbar. Wichtig: das Vault-Modul bekommt **nie den rohen
Disk-Masterkey**, nur diesen Subkey über einen schmalen, ihm vorbehaltenen
Kernel-Aufruf — kompromittiertes Modul leakt dann nur den Vault-System-Subkey.

---

## 4. Besitzverhältnis (Option A — beschlossen)

Hält den Kernel schlank, das Tool pflegt sich separat.

- **Kernel = File-Wächter + Cap-Broker + Subkey-Geber, sonst nichts.**
  - schützt `sys/vault` in `is_trust_critical_path` (nur der Vault liest ihn roh);
  - prüft die Capability, wenn eine App ein System-Credential anfragt;
  - gibt dem Vault den domänen-getrennten Geräte-Subkey.
- **Vault-Modul = Krypto + Policy + GUI.** Hält die DEKs nur solange entsperrt,
  setzt die Policy (§5) durch, rendert das GUI (§6).

**Start: bei Bedarf, nicht im autostart einsortiert.** Der Vault ist kein
autostart-Eintrag (Reihenfolge-Abhängigkeiten sind fragil), sondern ein Dienst,
den der **Kernel beim ersten Secret-Zugriff hochzieht** — exakt wie der Picker
heute demand-gespawnt wird (`host_core.rs` spawnt `sys/wasm/<picker>` beim
`npk_pick`). Ablauf:

1. `wifid` (autostart, `NETCTL`) → `request("net", ssid)` an den Kernel-Broker.
2. Kernel prüft `NETCTL`, zieht den Vault-Dienst hoch, falls nicht da, leitet weiter.
3. Vault holt den Geräte-Subkey, entsperrt die System-DEK, leitet die PMK ab,
   gibt sie an `wifid`.

Order-unabhängig: die Anfrage **zieht den Vault hoch**. Danach bleibt er resident.
Der System-Tier braucht dafür **keine** User-Interaktion. **Notnagel** (nur falls
der Demand-Launch beim ersten WLAN je zu träge wäre): Kernel bedient den
System-Tier direkt, Modul nur fürs GUI — zieht Krypto in den Kernel, daher nicht
die erste Wahl.

---

## 5. Access-Policy: nutzen ≠ anzeigen ≠ verwalten

| Operation | Was nötig ist |
|---|---|
| **Verbinden** (wifid nutzt das Netz-Credential) | Capability `NETCTL`. Der Vault gibt **nicht den PSK, sondern die abgeleitete PMK** (PBKDF2 über SSID+PSK, `WIFI_CLASS_ABI` §5). Kompromittiertes `wifid` leakt dann nur einen netzspezifischen Schlüssel, nicht das wiederverwendbare Passwort. |
| **Anzeigen** (Klartext im GUI zeigen) | **Vault-Passwort** (User-Session entsperrt) — **auch** für System-Credentials. Benutzen ≠ Enthüllen: ein Hintergrund-Tool mit `NETCTL` darf das WLAN-Passwort nutzen, aber nicht auf Schirm/Log ausleiten. |
| **Erfassen / Ändern** | System-Tier: Capability (z.B. `NETCTL`) **oder** Vault-Passwort im GUI. User-Tier: Vault-Passwort. |

---

## 6. GUI (im v1-Umfang)

Ein WASM-App über den Compositor (wie loft/bar). Modi:
- **Entsperren:** Vault-Passwort eingeben → User-Tier offen für die Session,
  **auto-lock** nach Inaktivität / bei Logout.
- **User-Secrets:** Liste, Hinzufügen/Editieren/Löschen, Enthüllen/Kopieren.
- **System-Credentials:** Abschnitt für WLAN & Co. — anzeigen (Passwort nötig),
  erfassen/ändern (schreibt in den System-Tier), ein WLAN-Passwort hier setzen
  ersetzt den alten `store /sys/config/wifi_psk`-Weg.
- **Verwaltung:** Vault-Passwort setzen/ändern, Keyslots verwalten (später: MFA-
  Faktoren, §7).

**Offene Implementierungsfrage:** ein Modul (Dienst-first, GUI als Modus) oder
zwei (headless Dienst + dünnes GUI-Frontend, das am Dienst andockt)? Lean: **ein
Modul**, das demand-gespawnt headless startet und beim Launch aus dem Launcher
zusätzlich ein Fenster zeigt — die Session lebt im Dienst, das GUI ist die Sicht.
Beim Bauen entscheiden.

---

## 7. MFA — vorgemerkt, Architektur bleibt MFA-ready (nicht v1)

Zwei Sorten, beide vom Modell getragen:
- **Kryptografische Faktoren** (FIDO2 hmac-secret, Smartcard, Recovery-Key) →
  zusätzlicher **Keyslot-Input** für die User-DEK. Echter zweiter Faktor.
- **Verifikationsfaktoren** (TOTP) → das Modul prüft vor dem Entsperren (ein
  TOTP-Code taugt nicht als stabiler Schlüssel-Input, aber als Gate).

**Synergie (später):** der Vault könnte die Login-MFA-Secrets halten; Geräte-
Login-MFA wäre dann nur ein weiterer Vault-Nutzer mit Cap. Beim v1-Bau **nicht
verbauen** (Keyslot-Header erweiterbar lassen, Policy-Enforcement im Modul).

---

## 8. Kernel-ABI (minimal halten)

- `is_trust_critical_path`: `sys/vault` aufnehmen (nur Vault liest roh).
- **Cap-Broker + Demand-Launch** für Secret-Anfragen (ein schmaler Weg, am
  Picker-Spawn modelliert).
- **Geräte-Subkey-Aufruf**, nur dem Vault-Modul erlaubt, liefert den
  domänen-getrennten „vault-system"-Subkey (nie den rohen Disk-Masterkey).

Mehr nicht im Kernel. Liste/Add/Edit/Anzeige = alles im Modul.

---

## 9. v1-Umfang + Baureihenfolge

**Umfang:** Unterbau + WLAN-Migration + GUI (GUI direkt mit, spart späteren Umbau).

1. **Unterbau:** File-Format + Header mit Keyslots, zwei DEKs, AEAD-Einträge
   (nutzt das bestehende Keyslot-/AES-256-GCM-Material). `sys/vault` als
   trust-critical; Geräte-Subkey-Ableitung.
2. **Kernel-Broker + Demand-Launch + Subkey-Aufruf** (am Picker-Spawn modelliert).
3. **WLAN-Migration:** `wifid` fragt den Vault statt `npk_fetch("sys/config/
   wifi_psk")`; Vault liefert PMK. Alt-Datei entfällt (einmal `wlan`-Intent neu;
   kein Migrationscode — ein Nutzer). Schließt F-M3.
4. **GUI:** User-Secrets + System-Credential-Verwaltung + Entsperren/auto-lock.

Kein Reformat, alles Kernel-`update` + neues Modul.

---

## 10. Offen / bewusst später

- **Offline-Rollback** (erbt Fund 3 aus `NPKFS_SECURITY_REVIEW.md`): ein
  Angreifer mit der Platte kann einen alten Vault-Stand zurückspielen. Gehört zu
  „Metadaten echt authentifizieren", nicht v1.
- **MFA** (§7) und **Login-Integration** — Architektur bereit, Bau später.
- **Ein vs. zwei Module** fürs GUI (§6) — beim Bau entscheiden.

---

## 11. Querverweise

- `NPKFS_SECURITY_REVIEW.md` §5 F-M3 (der auslösende Fund), Fund 3 (Rollback).
- `docs/spec/WIFI_CLASS_ABI.md` §1/§5/§6 (Rollen, PMK-Ableitung, „Treiber sieht
  nie PSK" — jetzt erzwingbar).
- `crypto/keyslot.rs`, `security/disk_key.rs` (das wiederverwendete Keyslot-Muster).
- Memory `project_keystore.md` (ursprüngliche Skizze, jetzt von diesem Paper abgelöst).
