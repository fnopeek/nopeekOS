# npkFS — Work Paper: Sicherheits- & Performance-Review

**Stand:** 2026-10-23 · kernel v0.666.1 · reines Code-Review, **nichts
reproduziert, nichts gefixt**

**Historie:** Erstfassung 2026-10-22 (Funde 1–4 aus Code abgeleitet). Am
2026-10-23 durch vier parallele Code-Audit-Läufe **gegengeprüft** und erweitert
(Disk-Parser, Trust-Boundary oberhalb des FS, Performance). Die Urteile
(CONFIRMED / PLAUSIBEL / WIDERLEGT) stehen jeweils am Fund; neue Funde in §5–§7.

**Scope:** npkFS (`kernel/src/storage/npkfs/`) plus die Schichten, die direkt
darauf vertrauen: WASM-Host-Funktionen (`wasm/host_core.rs`, `wasm.rs`),
Picker-/Launcher-Grants (`shade/widgets/mod.rs`), 9p-Streaming
(`microvm/devices/p9_async.rs`, `virtio_9p_pci.rs`), die On-Disk-Parser
(`btree.rs`, `journal.rs`, `bitmap.rs`, `sb_io.rs`, `format.rs`,
`storage/gpt.rs`, `storage/fat32.rs`).

**Leitfrage:** Nicht „ist diese Funktion korrekt?", sondern „welche Annahme
über die Schicht darunter macht korrekter Code, und wo gilt sie nicht?".

---

## 0. Was solide ist (bestätigt im Code)

- **Inhalte:** jedes Objekt (Blobs, Trees, Manifeste) ist AES-256-GCM-
  verschlüsselt, Key + Nonce aus dem Hash abgeleitet (`storage.rs` `put`/`get`).
  Manipulation von Ciphertext, Tag oder Hash-Feld → `Corrupt`.
- **Extent-Prüfung gegen manipulierte Platten:** `checked_extent_blocks`
  (`storage.rs:752`) prüft Datenbereich, Plattenende, Überläufe, bevor etwas
  daraus dimensioniert wird. `get()` nutzt zusätzlich `try_reserve_exact`.
  Indirect-Chain mit Zykluserkennung (`read_indirect_chain`, Klammer 255).
- **Lock-freier Read-Tail in `get`:** Race „entfernt + gc + realloc" landet
  im AEAD-Tag → `Corrupt`, nie ein Escape. `get` droppt den Lock vor dem DMA.
- **Pfadprüfung der Sandbox:** `private_area_owner` arbeitet segmentweise,
  `is_trust_critical_path` trimmt und prüft Segmentgrenzen; `parse_path`
  lehnt leere, `.`- und `..`-Segmente ab. Prüfung und FS sehen denselben Namen.
- **9p-Gast-Confinement:** `join_confined` (`virtio_9p_pci.rs:1369`) hält jeden
  Pfad im `root`-Präfix, `..` klemmt auf `root`, eingebettete `/`/NUL werden
  abgelehnt, `t_attach` lehnt `.`/`..`/leer ab. **Gast kann home nicht
  verlassen, erreicht `sys/`/`priv/`/`.system/` nicht.** (Audit bestätigt.)
- **Path-Grant-Matching exakt:** `check_path_grant` (`capability.rs:304`) nutzt
  `e.path == path`, keine Präfix-/Trailing-Slash-Aufweitung. Grants werden an
  Instanz-Ende widerrufen (`revoke_path_grants`).
- **rename/copy in/aus dem Trust-Store:** prüfen `is_trust_critical_path` auf
  BEIDEN Seiten und brauchen globales WRITE; ein Quell-Grant impliziert kein
  Ziel-WRITE. `rename` verweigert Verschieben in den eigenen Unterbaum.
- **GC:** `mark_incomplete` verhindert Sweep bei unvollständigem Mark.

> **Aber — die Kehrseite, siehe §7:** „Inhalte sind AEAD" gilt nur für
> Objekt-*Nutzdaten*. Die *Metadaten* (Superblock, B-Baum-Knoten, Bitmap,
> Journal, GPT, FAT) sind **Klartext mit UNGEKEYTEN Checksummen**. „Checksumme
> stimmt" ist damit **keine** Vertrauensgrenze gegen einen Angreifer, der die
> Platten-Bytes schreibt — er rechnet jede Checksumme kostenlos nach.

---

## 1. FUND — GC löscht Chunk, den ein neuer Stream per Dedup wiederverwendet

**Schwere (korrigiert):** NIEDRIG–MITTEL — **lokale, dauerhafte Korruption
EINER Datei**, kein DoS.
**Urteil:** Mechanismus (a/b/c) **CONFIRMED**, Headline-Konsequenz (d)
**WIDERLEGT**.

### Befund (bestätigt)
- (a) `gc()` (`fs.rs:394`) hält `ROOT_MUTEX` über den ganzen Lauf, prüft
  `stream_active()` nur zweimal: Start (`fs.rs:409`) und vor dem Sweep
  (`fs.rs:486`). Die Sweep-Schleife (`fs.rs:493-510`) prüft **nicht** je
  Iteration. `ROOT_MUTEX` gated die Storage-Schicht NICHT — `storage::remove`
  und die `has`/`put` des Streams nehmen `FS.lock` und geben ihn zwischendrin
  frei; ein Schreiber auf einem anderen Kern läuft dazwischen (9p-Persist-Worker
  auf eigenem Kern, `p9_async.rs:106`).
- (b) `stream_begin()` ist lock-frei (`fs.rs:293`, nur `fetch_add`) und läuft
  ohne `ROOT_MUTEX`; der 9p-Weg (`p9_async.rs:205`) ruft
  `fs::open_streaming_write` direkt, ohne `ensure_dirs`.
- (c) Dedup ist `!has()`-gated (`flush_chunk` `fs.rs:726`, `append_stored_chunk`
  `fs.rs:684` schreibt NIE, vertraut `has()`); `finish()` (`fs.rs:745`) prüft
  die Chunk-Hashes vor dem Veröffentlichen des Manifests nicht nach.

### Konsequenz (korrigiert)
- **Real:** Schreiber liest `has(H)=true` kurz bevor `gc` `remove(H)` ausführt,
  überspringt den `put`, zeichnet `H` auf; `gc` löscht `H`; `finish()`
  veröffentlicht ein Manifest, das auf das fehlende `H` zeigt. `fs::read` →
  `stitch_chunked` → `storage::get(H)=None` → `Corrupt` (`fs.rs:621`).
- **WIDERLEGT:** KEIN permanenter GC-Wedge, KEINE volllaufende Platte. `gc`
  markiert Chunk-Hashes direkt aus dem **Manifest**, ohne die Chunk-Körper zu
  lesen (`mark_file_object` `fs.rs:356`). Ein genuin fehlendes Objekt gibt dort
  `true` zurück („nichts zu markieren, kein Fehler", `fs.rs:359`) → **kein**
  `mark_incomplete`. Der fehlende Chunk wird bei jedem späteren `gc` still neu
  als erreichbar markiert und einfach nicht gefegt. (`mark_incomplete` triggert
  nur ein fehlender TREE — und Trees werden nur unter `ROOT_MUTEX` geschrieben,
  das `gc` hält, also nie dedup-geraced.)

### Fix (Skizze) — **kein Format-Eingriff, Code-only**
`stream_begin()` in `open_streaming_write` unter `ROOT_MUTEX` setzen (und den
9p-Weg über denselben Wrapper führen). Dann kann während eines GC-Laufs kein
Stream beginnen; jeder vorher begonnene wird vom ersten `stream_active()`-Check
erfasst.

### Test
Self-Test: Orphan-Blob erzeugen, `gc()` auf einem Kern, auf einem zweiten im
Sweep einen Stream mit identischem Chunk öffnen + `finish`, danach Lesen.

---

## 2. FUND — `sys/config/picker` ist Vertrauensanker, aber nicht geschützt

**Schwere:** MITTEL
**Urteil:** **CONFIRMED.** (Realistisch ist nur Weg 1; siehe unten.)

### Befund (bestätigt)
- `is_trust_critical_path` (`wasm.rs:1238`) schützt NUR `sys/wasm`, `sys/certs`,
  `sys/firmware` — **nicht** `sys/config/picker`/`launcher`.
- `picker_module_name()` (`wasm.rs:195`) liest `sys/config/picker` server-seitig
  und entscheidet, welches Modul den Dialog stellt. `finish_pick`
  (`shade/widgets/mod.rs:640`) gibt dem Requester `READ|WRITE` auf den Pfad, den
  der Picker zurückgibt — **ohne** `is_trust_critical_path`-Prüfung.

### Wege hinein
1. **(realistisch)** Jede App mit globalem WRITE: `npk_store("sys/config/picker",
   "<eigenes Modul>")` → `is_trust_critical_path` passt (nicht kritisch),
   `private_area_allows` passt, `has_write` passt → `upsert` (`host_core.rs:2430`).
2. **(kaum erreichbar)** Ohne WRITE über die `own_config`-Ausnahme
   (`host_core.rs:2403`): ein Modul namens `picker` darf `sys/config/picker`
   schreiben. Setzt aber Installation unter `sys/wasm` voraus — das IST
   trust-critical. Deshalb kein realistischer Weg.

### Folge
Gefälschte Dateidialoge, Confused Deputy (Nutzer wählt „bericht.txt", Picker
gibt dem Opfer den Grant auf `.system/config`), Mitlesen der Nutzerauswahl.

### Fix (Skizze) — **kein Format-Eingriff, Code-only**
`sys/config/picker`, `sys/config/launcher` (und mehr, siehe §5-F) in
`is_trust_critical_path` aufnehmen — gilt dann für store/delete/rename/copy, UND
schließt die Picker-Grant-Lücke (§5-F4), weil die Store-Prüfung dem Grant
vorausgeht. Zusätzlich die `own_config`-Ausnahme für reservierte Namen sperren.

---

## 3. DESIGNGRENZE — Offline-Rollback und alte Keyslots

**Schwere:** MITTEL (Angreifer braucht die Platte)
**Urteil:** **CONFIRMED — eher UNTERSCHÄTZT.**

### Befund (bestätigt)
- DEK wird einmal gezogen und **nie rotiert**; Passphrasenwechsel re-wrappt nur
  (`security/disk_key.rs:66` `change()`: `open(old)` → `keyslot::create(new,&dek)`
  → `set_keyslot(0, slot)`). Jeder Keyslot-Blob, der diesen DEK wrappt, öffnet
  das Volume für immer.
- `set_keyslot` (`storage.rs:239`) schreibt nur den **nächsten** SB-Slot
  (`sb_io.rs:123`, `generation % 8`). Nach einem Wechsel stehen in bis zu 7 von 8
  Ring-Slots gültige Superblöcke mit dem ALTEN Keyslot-0 (alte Umhüllung
  desselben DEK) und älterem `root_tree_hash`.
- Metadaten ungekeyt: SB-Checksumme (`format.rs:158`), B-Baum-Knoten
  (`btree.rs:26`) ungekeytes BLAKE3; Journal/Bitmap ohne MAC. **Kein** monotoner
  Zähler / TPM-NV / Anti-Rollback irgendwo.
- **Verschärfung ggü. Erstfassung:** Weil der SB fälschbar ist, ist der Angreifer
  nicht auf die 7 Kopien beschränkt — er kann einen beliebigen alten Root
  nehmen, dessen Objekte noch da sind (GC hält alle 8 Ring-Roots), die
  `generation` hochsetzen und die ungekeyte Checksumme neu rechnen, sodass
  `read_best` ihn wählt.

### Folge
Widerrufene Passphrase öffnet offline weiter; jeder früher committete Stand
(alte Module, alte Zertifikate, alte `.system/config`) ist zurückspielbar.
(Am laufenden System greift der Angriff nicht — `read_best` nimmt die höchste
`generation`, der DEK-Open liest die In-Memory-SB-Keyslots. Exposition ist
offline/at-boot.)

### Optionen (unterschiedlicher On-Disk-Impact, siehe §8)
- **(a) Alle 8 SB-Slots nach Passphrasenwechsel neu schreiben** — Code-only,
  kein Format-Eingriff. Schließt nur die Keyslot-Reste, nicht den Rollback.
- **(b) Gekeyter SB-/Metadaten-MAC** — verhindert das Fälschen neuer SBs/Knoten,
  aber **kein** Replay. **Format-Eingriff** (Checksummenfeld ändert Bedeutung).
- **(c) Anti-Rollback mit monotonem Zähler (TPM-NV o. ä.)** — nur das stoppt
  Replay; braucht Hardware + vermutlich neue SB-Felder = **Format-Eingriff**.
- Ohne (c): dokumentieren, dass ein Offline-Angreifer zurücksetzen kann.

---

## 4. Klein (alle bestätigt)

- **`store`-TOCTOU — CONFIRMED, Impact NIEDRIG:** `npkfs::store` prüft „existiert"
  außerhalb `ROOT_MUTEX` (`mod.rs:103`), `paths::store` erlaubt danach Ersetzen
  (`allow_replace=true`, `paths.rs:313`) → zwei gleichzeitige `store` desselben
  neuen Pfads überschreiben sich still statt `ObjectExists`. **Aber** alle
  strict-store-Caller sind serialisiert (Installer/Intents), also real kaum
  auslösbar — Verlust der Strict-Create-Garantie, keine Korruption.
- **Veralteter Kommentar — CONFIRMED:** `paths.rs:116` „Trees stay plaintext" —
  falsch, `storage::put` verschlüsselt alles (`storage.rs:581`). Ebenso der
  „Tree → unencrypted"-Kommentar bei `mkdir` (`paths.rs:350`). Gefahr: jemand
  „repariert" nach dem Kommentar.
- **Fehlertext — CONFIRMED:** `types.rs:92` „max 63", tatsächlich
  `MAX_NAME_LEN = 255` (`object.rs:23`). Kosmetisch.

---

## 5. NEUE FUNDE aus dem Audit (nicht in der Erstfassung)

### 🔴 F-H1 — btree `num_entries` ungeklammert → OOB-Panic = Kernel-Halt (DoS)
**HIGH · selbst am Code verifiziert.** `btree.rs:142/195/256/408` nehmen das rohe
`u16` `num_entries` (gelesen `btree.rs:48`) direkt als Schleifengrenze;
`leaf_entry` (`btree.rs:81`) sliced `buf[off..off+112]` mit
`off = 16 + idx*112`, `internal_key` (`btree.rs:63`) `buf[off..off+32]` mit
`off = 16 + idx*40`. Crafted-Knoten mit `num_entries = 0xFFFF` (+ neu
gerechneter gültiger Checksumme): bei `idx≈36` (Leaf) bzw. `idx≈102` (Internal)
läuft `off` über 4096 → Rust-Slice-Panic → **Kernel-Halt**. Erreichbar beim
ERSTEN `lookup` über einen manipulierten Baum; Knoten sind unverschlüsselt, kein
Key nötig. Die rekursiven fsck-Pfade klammern bereits (`.min(MAX_…)`,
`btree.rs:346`), die drei Hot-Paths nicht.
**Fix — Code-only, kein Format-Eingriff:** `num_entries` in `read_header` (oder
je strict-Schleife) auf Knotenkapazität klammern.

### 🔴 F-H2 — `total_blocks` nie gegen Gerätegröße geprüft → OOM-Panic beim Mount (DoS)
**HIGH.** `bitmap.rs:39/46` (`load_args`, via `mount` `storage.rs:178`) rechnet
`byte_count = total_blocks.div_ceil(8)` und macht `vec![0u8; byte_count]` —
**infallibel**. Der Geometrie-Check prüft nur interne Konsistenz, nie
`total_blocks ≤ blkdev::block_count()`. Crafted-SB mit `total_blocks = 2^40` →
128-GB-Allok → `handle_alloc_error` → **Halt**. `get()` nutzt bewusst
`try_reserve_exact` — `load_args` bekam das nie.
**Fix — Code-only:** `total_blocks > blkdev::block_count()` ablehnen, Bitmap auf
`try_reserve`.

### 🔴 F-H3 — `.system/config` von jeder App mit globalem WRITE schreib-/löschbar
**HIGH · Gate selbst verifiziert.** `is_trust_critical_path` (`wasm.rs:1238`)
deckt `.system/` und `sys/config/` **nicht**. Der `.system/`-Schutz
(`validate_user_name`, `mod.rs:221`) hängt NUR am Shell-Intent (`intent/fs.rs`),
nicht am WASM-Host-Pfad — `npkfs::upsert/store/delete/...` rufen nur `validate()`
(leer+NUL). Also ist `is_trust_critical_path` das einzige Tor, und es hat das
Loch. `npk_store(".system/config", …)` einer App mit globalem WRITE durchläuft
`is_trust_critical_path`=false → `private_area_allows`=true (erstes Segment
`.system` ≠ `priv`) → `has_write`=true → `upsert`.
`.system/config` wird bei jedem Login (`gui/login.rs:340`) und Boot
(`main.rs:386`) neu gelesen; Überschreiben setzt sicherheitsrelevante Settings:
- `autostart` → beliebiges installiertes Modul bei jedem Boot (Persistenz)
- `static_dns` → systemweiter DNS-MITM
- `net.allow_plain_http` → HTTPS-Downgrade systemweit
- `name` → verschiebt `home/<user>` (MicroVM-9p-Root)
Dieselbe Lücke erlaubt **Löschen** von `.system/config` (`host_core.rs:4175`) →
Config-Wipe-DoS. **Breiter und schwerer als Fund 2** — `.system/config` ist ein
zentraler Pfad. Angreifer: jede WASM-App, die WRITE deklariert (loft tut das;
Browser/Editoren üblich).
**Fix — Code-only:** `.system`/`.system/*` **ganz** in `is_trust_critical_path`
(kein legitimer WASM-Schreiber dort; `config::save` und die Intents gehen über
den Kernel-npkFS-Pfad), plus die konkreten Schlüssel `sys/config/picker` und
`sys/config/launcher` — **nicht** den ganzen `sys/config/`-Präfix, weil eine App
ihre eigenen Settings unter `sys/config/<modul>` über die `own_config`-Ausnahme
schreibt (die muss weiter gehen). Schließt F-H3, F-M4 und erweitert Fund 2. Die
Netz-Credential-Schlüssel (`sys/config/wifi`, `wifi_psk`) bewusst NICHT hier,
sondern auf dem Keystore-Track (`KEYSTORE.md`), sonst bräche wifid.
**Bestätigt:** der legitime Config-Schreiber (`config::save` →
`npkfs::upsert(..., CAP_NULL)`) umgeht den Host-Gate; nur `npk_store`/`delete`/
`rename`/`copy`/`mkdir` prüfen `is_trust_critical_path`, `npk_fetch` (Lesen)
nicht — Lesezugriffe bleiben also unberührt.

### 🟠 F-M1 — fsck `self_check`: Extent-`block_count` (u64) als Schleifengrenze → ~2⁶⁴-Hang (DoS)
**MITTEL.** `storage.rs:389/399`: `for b in 0..ext.block_count { fsck_mark(... start_block+b) }`.
`block_count` unvalidiert; `fsck_mark` returned zwar je Block früh, die SCHLEIFE
läuft aber `block_count`-mal. `block_count = u64::MAX` → effektiv Endlosschleife,
`start_block+b` überläuft. **Einzige Extent-Konsumstelle OHNE**
`checked_extent_blocks`. fsck ist intent-getriggert, läuft nicht beim Mount.
**Fix — Code-only:** Extents durch `checked_extent_blocks` vor dem Iterieren.

### 🟠 F-M2 — FAT32 `find_entry` ohne Zyklus-/Iterationsdeckel → Endlosschleife (DoS)
**MITTEL.** `storage/fat32.rs:455`: die `loop`-Kette über `fat_next(cluster)` hat
keinen visited-Set/Zähler (anders als `chain_len`, Deckel 100 000). Manipulierter
ESP mit zyklischer Cluster-Kette → `update_kernel` (OTA-Pfad, `:512`) hängt.
Scope: FAT ist nur ESP-Writer + OTA-Reader auf der eigenen NVMe (kein
USB-Mass-Storage-FAT im Baum), also geringere Reichweite, aber ein crafted ESP
erreicht es. Dazu klein: `from_esp` Overflow in `data_start`, `read_sector` ohne
Partitionsgrenze (liest in feste 512-B-Puffer, nicht speicher-unsicher).
**Fix — Code-only:** visited-Set/Deckel in `find_entry`.

### 🟠 F-M3 — `wifi_psk` (Klartext) + `.system/config` welt-lesbar mit Default-READ-Caps
**MITTEL.** Jede App bekommt per Default `READ|EXECUTE|RENDER`
(`capability.rs:372`). `npk_fs_list` (`host_core.rs:3045`) filtert nur fremde
`priv/`, versteckt `.system/`/`sys/` NICHT (anders als `npkfs::walk_recursive`,
das `.system/` überspringt — aber die Host-Fn nutzt den Weg nicht). Ein
rekursives `npk_fs_list("",1)` + `npk_fetch` liest damit:
- `sys/config/wifi_psk` — die **Klartext-WLAN-Passphrase** (`intent/net.rs:288`;
  deckt sich mit `wifi_psk im Klartext!` im Memory)
- `.system/config` (alle Settings), `.system/history` (Befehlsverlauf)
`priv/<other>`-Schutz selbst ist solide; das Problem ist, dass ein Geheimnis im
welt-lesbaren Bereich liegt.
**Fix:** `.system/`/`sys/` in `npk_fs_list`/`npk_fetch` verstecken ODER PSK
geschützt ablegen/verschlüsseln. Erste Variante Code-only; zweite berührt nur
die PSK-Datei (kein Reformat — einmal `wlan`-Intent neu, siehe §8).
**Eigener Design-Track — nicht im FS patchen:** Florian 2026-10-23: Kennwörter
gehören künftig in einen Vault/Keystore, nicht als Datei ins FS. Richtung und
offene Entscheidungen in `docs/plan/KEYSTORE.md` (Entwurf, wird neu durchdacht);
F-M3 ist dort der auslösende Fund.

### 🟠 F-M4 — Picker-Grant kann WRITE auf `.system/config` aushändigen (PLAUSIBEL)
**MITTEL · PLAUSIBEL.** `finish_pick` grantet `READ|WRITE` auf den Picker-Pfad
ohne `is_trust_critical_path`-Prüfung. Weil `.system/config` nicht trust-critical
ist (F-H3) und `npk_fs_list` es nicht versteckt (F-M3), kann die loft/pick-UI es
anzeigen → Nutzerklick händigt einer App OHNE globales WRITE ein WRITE-Grant auf
die Systemconfig aus. PLAUSIBEL, weil es davon abhängt, dass das pick/loft-Modul
diese Ordner zeigt (deren WASM-Code nicht gelesen).
**Fix:** durch F-H3-Fix mitgeschlossen (Store-Prüfung geht dem Grant voraus).

### 🟡 F-L — Low (Hardening)
- **Rekursive fsck-Walks:** `iter_subtree`/`fsck_subtree` (`btree.rs:314/354`)
  halten `[u8;4096]` je Frame, Tiefe 64 → ~260 KB Stack → möglicher
  Kernel-Stack-Overflow bei 64 verschachtelten Internal-Knoten. (Strict-Pfade
  iterativ, Tiefe 8, ein Puffer — ungefährdet.) Fix: iterativ/Heap-Puffer.
- **`journal_head` ungeprüft** (`journal.rs:96`): treibt Lese-Offsets; fällt aber
  graceful (`entry_count` geklammert, Offsets in-bounds, Out-of-Range → Mount
  schlägt sauber fehl, Frees range-validiert). Minor-Hardening.
- **GPT-Detect ohne CRC + Size-Overflow** (`storage/gpt.rs:98`): Read-Pfad prüft
  nur Signatur, nicht die CRC32 (Funktion existiert, nur Write nutzt sie);
  `usable_sectors` überläuft bei `end_lba=u64::MAX`. Downstream von `blkdev`
  abgefangen, keine OOB. Robustheit, nicht Speichersicherheit.

### Vollständige Enumeration — welche `sys/config/*` / `.system/*` liest der Kernel für Sicherheitsentscheidungen?
| Pfad | Gelesen für | Schreibgeschützt vor App? |
|---|---|---|
| `sys/config/picker` (`wasm.rs:197`) | Picker = Grant-Autorisierer | **NEIN** (Fund 2) |
| `sys/config/launcher` (`shade/mod.rs:1096`) | Launcher/Einstieg | **NEIN** (Fund 2) |
| `.system/config` (`config.rs:16`) | autostart, name, static_dns, allow_plain_http | **NEIN** (F-H3) |
| `sys/config/wifi` (`net.rs:268`) | Netzconfig | **NEIN** (WRITE / own_config `wifi`) |
| `sys/config/wifi_psk` (`net.rs:288`) | WLAN-Passphrase (Klartext) | **NEIN** — welt-lesbar (F-M3) |
| `sys/certs/*` | TLS-Root-CAs | JA (`is_trust_critical_path`) |
| `sys/firmware/*` | Geräte-Firmware | JA |
| `sys/wasm/*` | Modulcode | JA |

---

## 6. PERFORMANCE (eigene Dimension, alles neu)

### P1 — GC-Sweep: M Orphans = M separate 4-Phasen-Commits, Mark+Sweep unter ROOT_MUTEX (größter Hebel)
`fs.rs:391` (`gc`), Sweep-Schleife `fs.rs:493` → `storage::remove` → voller
4-Phasen-`commit` (`storage.rs:965`: Journal → Bitmap → `cache.flush` →
`blkdev::flush` BARRIER → FUA-SB → Journal-Finalize). Pro Orphan EINE synchrone
SSD-Durability-Barriere, **alles unter `ROOT_MUTEX`**. Auto-GC bei
`GC_PRESSURE_THRESHOLD = 128` Mutationen; jeder Write orphant alten Blob + jeden
neu gebauten Ahnen-Tree (~Tiefe+1). 128 Writes bei Tiefe ~4 ⇒ ~600+ Orphans ⇒
~600 Barrieren in Folge, die JEDEN Write/Delete/Rename/mkdir/stream-finish
blockieren — zig bis hunderte ms seriell, auf 9p/virtualisiert weit mehr. Der
Code gibt es selbst zu (`fs.rs:389`).
**Billiger:** Sweep in EINEN Commit batchen — die Maschinerie existiert schon
(`pending_old_blocks` + `journal.record_free`, von einem `commit` geleert, wie
`commit_root` sie für Puts nutzt). M Barrieren → 1.

### P2 — `put` hält den globalen FS-Lock über den DMA-Write (`get` nicht)
`storage.rs:586` nimmt `FS.lock()`, hält ihn über die Extent-DMA-Schleife
(`:640-671`) bis Fn-Ende. `get` droppt den Lock bewusst vor dem DMA
(`storage.rs:801`, mit Begründungskommentar). Verschlüsselung passiert schon vor
dem Lock; serialisiert ist nur der Roh-Write. Trifft jeden nicht-streaming
`write`/`upsert` (Browser-Save, Font/Bild).
**Billiger:** wie `get` — Extents sind unter dem Lock reserviert, also
Extent-Liste kopieren, `drop(lock)`, DMA entsperrt, Lock für den btree-Insert neu.

### P3 — kein decodierter Tree-Cache + O(K²) Verzeichnis-Rewrite
`fetch_tree` (`paths.rs:92`) = `get` (btree-Lookup + DMA + AES-GCM-Decrypt +
postcard-Decode) **ohne Memoisierung** (der 64-Slot-BlockCache cached rohe
Blöcke, nicht das Decrypt+Decode). Schreiben von K Dateien in dasselbe
Verzeichnis schreibt dessen Tree K-mal neu, je O(K) Einträge ⇒ **O(K²)**
Encode/Encrypt/Hash/DMA + K−1 Orphan-Trees (speisen P1). Installer schreibt in
Masse (`install_data/assets/mod.rs:220`). Boot/Render: `fetch` für Fonts/Icons
und `exists`/`stat`/`list` re-decrypten jeden Tree auf dem Pfad jedes Mal.
**Billiger:** kleiner decodierter-`Object`/Tree-Cache per Content-Hash (Hash
unveränderlich → Invalidierung trivial, Eintrag für immer gültig). Batch-Insert-
API, die einen Verzeichnis-Tree einmal für N Dateien baut.

### P4 — redundante btree-Traversierung + ungekeytes Node-Re-Hash auf jedem Read
`put` macht den btree-Lookup **3×** (Dedup `storage.rs:565`, Race-Recheck `:593`,
dann in `btree::insert` `:195`), `paths::store` noch ein `has` davor. Jeder
Node-Read ruft unbedingt `verify_node_checksum` = `blake3` über 4064 B
(`btree.rs:100`), **auch bei Cache-Hit oder gerade geschriebenem Knoten**. Ein
Datei-Write ≈ Blob + ~Tiefe Trees = ~5 Puts; ~3 Traversierungen × bis 8 Knoten ⇒
bis ~120 BLAKE3-über-4KB je Write.
**Billiger:** ein „lookup-or-insert"-Durchgang (der Race-Recheck ist unter
`ROOT_MUTEX` fast tot); „verified/clean"-Bit je Cache-Slot → Hit überspringt den
Re-Hash.

### Niedriger
- **P5** `list`/`walk_recursive` (`mod.rs:269`, `paths.rs:440`) läuft je
  Verzeichnis von der Root neu → O(N·Tiefe) statt O(N). Kalt, reine
  Verschwendung; Subtree-Hash durch die Rekursion reichen.
- **P6** Bitmap `find_run` (`bitmap.rs:125`) bit-weise, `count_free` (`:257`)
  scannt jeden Block beim Mount (1 TB ≈ 268M Iterationen); große Allok halbiert
  und re-scannt die ganze Bitmap je Schritt. Word-weise scannen
  (`u64::count_ones`, 0xFF-Words überspringen), free-run-Hinweis.
- **P7** `stitch_chunked` (`fs.rs:616`) materialisiert die ganze Datei im RAM
  (kein Streaming-Reader) — RAM-Klippe bei Multi-GB-Reads.
- **P8** BlockCache 64 Slots (256 KB), linearer Scan — hält das btree-Arbeitsset
  großer Stores nicht, GC/Puts thrashen.

### Nicht anfassen (geprüft, korrekt)
Streaming-`flush_pending` alle 16 Chunks (gewollt), `get`-Lock-Drop vor DMA,
deferred-TRIM-Batching, O(1)-`copy` (Content-Addressing), elidiertes
BLAKE3-on-read (AEAD-Tag authentisiert schon).

---

## 7. Das verbindende Muster

Die **gesamte Metadaten-Schicht** (Superblock, B-Baum, Bitmap, Journal, GPT,
FAT) ist **Klartext mit UNGEKEYTEN Checksummen** (BLAKE3 in btree/SB, CRC32 in
GPT, nichts in FAT). Nur Objekt-*Nutzdaten* sind AEAD. Die Parse-Pfade (`mount`,
`btree::lookup`, `journal::replay`, `bitmap::load_args`) laufen vor/ohne jeden
Disk-Key. „Checksumme stimmt" ist deshalb **keine** Schranke gegen einen, der die
Platten-Bytes schreibt — er rechnet sie kostenlos nach. Das ist die gemeinsame
Wurzel von Fund 3, F-H1, F-H2 und den Parser-Funden. `blkdev::read_block`
grenzt zwar gegen die Gerätekapazität ab (graceful `OutOfRange`), aber
`block*SECTORS_PER_BLOCK` kann wrappen (in-range, aber falsch).

Zwei getrennte Härtungsgrade, mit UNTERSCHIEDLICHEM On-Disk-Impact (§8):
- **robuste Parser** (Clamps, `try_reserve`, Zyklusdeckel) — macht den Kernel
  gegen crafted Bytes panikfest, **ohne Format-Eingriff**. Das ist F-H1/H2/M1/M2
  und die Lows. Dringend und billig.
- **authentische Metadaten** (gekeyter MAC auf SB/btree/bitmap/journal) — macht
  die Checksumme zur echten Schranke, **ändert aber das On-Disk-Format**. Das ist
  die tiefe Hälfte von Fund 3 und optional/später (§8).

---

## 8. On-Disk-Impact — was braucht Neuformatierung/Neuinstallation?

**Kurz: fast alles ist Code-only (reiner OTA `update`, bestehende npkFS-Platte
läuft weiter). Format-/Reinstall-Eingriff braucht NUR die tiefe
Metadaten-Authentifizierung aus Fund 3 — und die ist optional/später.**

| Fix | On-Disk-Format? | Bestehende Installation |
|---|---|---|
| F-H1 btree-Clamp | nein | läuft weiter — `update` |
| F-H2 `total_blocks`-Check | nein | läuft weiter — `update` |
| F-H3 `.system`+`sys/config` in Trust-Path | nein (reine ACL-Logik) | läuft weiter — `update` |
| Fund 1 GC×Stream (stream_begin unter Lock) | nein | läuft weiter — `update` |
| Fund 2 Picker (= F-H3-Fix) | nein | läuft weiter — `update` |
| F-M1 fsck-Extent-Guard | nein | läuft weiter — `update` |
| F-M2 FAT-Zyklusdeckel | nein | läuft weiter — `update` |
| F-M3 `.system`/`sys` verstecken | nein | läuft weiter — `update` |
| F-M4 Picker-Grant | nein (= F-H3) | läuft weiter — `update` |
| F-L Lows (fsck-Stack, journal_head, GPT-CRC) | nein | läuft weiter — `update` |
| Fund 4 (b)(c) Kommentar/Text | nein | läuft weiter — `update` |
| **Alle Performance-Fixes P1–P8** | **nein** (Objektformat/btree bleiben identisch; alles Laufzeit/In-Memory) | läuft weiter — `update` |
| Fund 3 (a) alle 8 SB-Slots neu schreiben | nein (nur Schreibverhalten bei Passphrasenwechsel) | läuft weiter — `update`; Keyslot-Reste auf bestehender Platte werden erst beim nächsten Wechsel/Rewrite bereinigt |
| **Fund 3 (b) gekeyter SB-/Metadaten-MAC** | **JA** — Checksummenfeld ändert Bedeutung; alter SB validiert nicht gegen neuen Kernel | **Reformat/Neuinstall** (Policy: einziger Nutzer, kein Migrationscode) |
| **Fund 3 (c) Anti-Rollback-Zähler (TPM-NV)** | **JA** — vermutlich neue SB-Felder | **Reformat/Neuinstall** + Hardware-Abhängigkeit |

**Begründung, warum die Panic-Fixes KEIN Reformat brauchen:** F-H1/H2/M1/M2 sind
Lese-seitige Klammern. Eine LEGITIME Platte hat nie `num_entries` über Kapazität,
nie `total_blocks` über Gerätegröße, nie `block_count = u64::MAX` — für gültige
Daten ändert die Klammer nichts, sie fängt nur den Angreifer ab. F-H3 ist reine
Kernel-ACL-Logik, kein Byte auf der Platte. Die Perf-Fixes ändern nur, WIE/WANN
gelesen/geschrieben/gelockt wird, nicht das Format der Objekte oder des B-Baums.

**Einzige Ausnahme mit Datenberührung ohne Reformat:** F-M3-Variante „PSK
geschützt ablegen" verschiebt/verschlüsselt eine einzelne Datei — bestehende
Platte braucht einmal einen neuen `wlan`-Intent (PSK neu setzen), kein Reformat.

**Fazit für die Entscheidung:** Alles unter §1/§2/§4/§5/§6 und Fund 3(a) kann als
normaler signierter Kernel-`update` ausgerollt werden; keine bestehende
Installation muss neu formiert werden. Nur wenn wir die Metadaten echt
authentifizieren wollen (Fund 3 b/c), fällt ein Reformat an — das ist ein
bewusster, separater Schritt und nicht Teil der Panik-/ACL-Härtung.

---

## 8a. Umsetzungsstand (2026-10-23)

**Erste Runde umgesetzt, Build grün (Kernel v0.669.0), Laufzeittest (QEMU/Gerät)
offen — Florians Gate. Alle Code-only, kein Format-Eingriff.**

- ✅ **F-H1** — `btree::read_header` klammert `num_entries` auf die
  Knotenkapazität (`BTREE_LEAF`→`MAX_LEAF_ENTRIES`, `BTREE_INTERNAL`→
  `MAX_INTERNAL_KEYS`), No-op auf gültigen Knoten.
- ✅ **F-H2** — `mount()` lehnt `sb.total_blocks > blkdev::block_count()` ab;
  `Bitmap::load_args` nutzt `try_reserve_exact` statt `vec![0u8; …]`.
- ✅ **F-H3 + Fund 2 + F-M4** — `is_trust_critical_path` deckt jetzt `.system`/
  `.system/*` und `sys/config/picker|launcher` (chirurgisch, siehe §5-F-H3).
- ✅ **Fund 1** — `open_streaming_write` nimmt `stream_begin()` unter
  `ROOT_MUTEX`, so kann kein Stream mitten im GC-Sweep beginnen.
- ✅ **F-M1** — `self_check` validiert jeden Extent (`extent_in_data_area`) vor
  `0..block_count`.
- ✅ **F-M2** — `fat32::find_entry` deckelt die Verzeichnis-Kette (100 000, wie
  `chain_len`).
- ✅ **Fund 4 (b)(c)** — falsche „plaintext"-Kommentare (`paths.rs`) korrigiert,
  Fehlertext `types.rs` auf „max 255".

**Noch offen (eigene Runden):** die Performance-Refactors §6 (P1 GC-Batching
zuerst, dann P2/P3/P4 — invasiver, eigener Test); Fund 1 Self-Test; Fund 3 (a)
8-Slot-Rewrite; F-M3 + Keyslot-Härtung (Keystore-Track `KEYSTORE.md`); F-L.

## 9. Vorgeschlagene Reihenfolge

1. **F-H1 + F-H2** (Panic-Schutz Parser) — kleine lokale Clamps, schließen zwei
   Kernel-Halts gegen crafted Platten/USB. Billigster Sicherheitsgewinn.
2. **F-H3** (`.system` + `sys/config` in `is_trust_critical_path`) — ein Fix,
   schließt F-H3, F-M4 und erweitert Fund 2. (Vorher: legitimer Config-Schreiber
   umgeht den Host-Gate — bestätigen.)
3. **P1** (GC-Sweep in einen Commit batchen) — größter Perf-Hebel, nutzt
   vorhandene `pending_old_blocks`/Journal-Maschinerie.
4. **Fund 1** (stream_begin unter `ROOT_MUTEX`) — Einzeiler + Self-Test.
5. **F-M1 / F-M2 / F-M3** (fsck-Hang, FAT-Zyklus, PSK/`.system` verstecken).
6. **P2 / P3 / P4** (put-Lock, Tree-Cache, Triple-Lookup).
7. **Fund 3 (a)** 8-Slot-Rewrite nach Passphrasenwechsel (Code-only).
8. **Entscheidung** Fund 3 (b/c) — nur mit Reformat; separat terminieren.
9. **Kleinkram** §4 (b)(c), §5-F-L.
