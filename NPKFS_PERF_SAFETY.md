# npkFS — Work Paper: Performance & Korruptions-Sicherheit

**Stand:** 2026-06-18 · kernel v0.224.0 · Session-Vorbereitung (morgen besprechen)
**Scope:** zwei gekoppelte Themen am npkFS — (1) nichts darf korrupt gehen,
(2) mehr Read/Write-Speed + weniger CPU-Verschwendung (Krypto, Multicore, I/O).

Dieses Paper ist die Grundlage für die Umsetzungs-Session. Es entscheidet
noch nichts hart — es legt Fakten, Funde, Hebel und eine vorgeschlagene
Reihenfolge auf den Tisch. Offene Entscheidungen stehen am Ende.

---

## 0. Ausgangslage / was schon gefixt ist

Frühere Sessions (Memory `project_filesystem_perf`) haben mehrere
Korruptions-Quellen geschlossen. Im aktuellen Code **bestätigt vorhanden**:

- **gc `mark_incomplete`** — gc sweept nicht mehr bei unvollständigem Mark
  (`kernel/src/storage/npkfs/fs.rs:405–450`, `mark_file_object` ist
  `#[must_use]`). Verhindert, dass gc aus lokaler Korruption ein
  unmountbares FS macht.
- **`MAX_INTERNAL_KEYS` 102 → 101** — Checksum-Trailer-Overlap weg
  (`format.rs:110`). Schloss die „high-entropy Child-Pointer → OutOfRange"-
  Quelle im btree-Write-Pfad.
- **`journal_head` erst nach `prepare()`** (`storage.rs:755`) — Post-Crash-
  Free-Leak weg.

**Trotzdem** berichtet Florian: gc/FS „zerschiesst es teilweise immer noch".
→ Es gibt einen **verbliebenen Korruptions-Vektor**. Hauptkandidat siehe §1.

---

## 1. KORRUPTION — Hauptfund: keine Write-Barriere im NVMe-Treiber

### Befund
Der 4-Phasen-Commit (`storage.rs:743 commit()`) ist als crash-sicheres
Protokoll gebaut: journal(committed=0) → bitmap+SB → flush → journal(committed=1)
→ flush. Die Korrektheit hängt daran, dass die zwei `cache.flush()`-Aufrufe
**echte Durability-Barrieren** sind, die die Reihenfolge erzwingen:
*„die neue SB wird nie durable, bevor das, was sie referenziert, durable ist."*

**Das stimmt auf echter Hardware nicht.** Der NVMe-Treiber
(`kernel/src/drivers/nvme.rs`):
- benutzt **nur** `NVM_WRITE` (Opcode 0x01) — `write_block`, `write_extent`,
  `write_blocks_batch`;
- setzt **kein FUA-Bit** (`cdw12 = 7`, Bit 30 = 0);
- gibt **nie** ein NVMe-FLUSH (Opcode 0x00) ab (im ganzen File kein 0x00-Flush);
- schaltet die **Volatile Write Cache (VWC) des Controllers nie ab** (das
  einzige `ADM_SET_FEATURES` bei Init setzt die Queue-Zahl, nicht VWC).

Per NVMe-Spec darf der Controller ein Write **als abgeschlossen melden,
sobald es im flüchtigen Cache liegt** — nicht erst auf NAND. Ohne FLUSH/FUA:
- können bestätigte Writes bei **Power-Loss / Hard-Reset verloren** gehen;
- können sie **gegenüber der NAND-Persistenz umsortiert** werden.

### Warum das genau die dokumentierte Korruption erzeugt
Innerhalb von Phase 2 gehen **Daten + Bitmap + SB in einem einzigen
`flush()`-Batch** (`write_blocks_batch`, ungeordnet). Reorder + Crash:
- **SB landet auf NAND, der neue btree-root / die Bitmap nicht** → Mount
  liest SB → btree_root zeigt auf einen Block mit stale/garbage Inhalt →
  `read_node`-Checksum-Fail (Corrupt) ODER, wenn dort ein alter gültiger
  Knoten liegt, **strukturell-valider aber falscher Baum** → Lost Objects /
  OutOfRange beim Descent.
- **SB durable, Bitmap-Sync nicht** → von der neuen Root referenzierte Blöcke
  stehen auf Disk als **frei** → nächste `alloc` gibt sie erneut aus →
  **Block-Doppel-Allokation** = exakt der „high-entropy Child-Pointer →
  OutOfRange"-Fingerprint aus dem Memory.

### Warum „nur manchmal" / warum bisher übersehen
- Sauberer `reboot` power-cycelt den Controller **nicht** → VWC überlebt →
  keine Korruption. Trifft nur **echten Power-Loss / Hard-Reset / Crash
  während Writes** (Akku leer, Deckel, Reset beim Entwickeln).
- **QEMU `cache=writeback` versteckt es komplett** (Host-RAM überlebt
  Gast-Crash) → in der Demo-VM nie reproduzierbar.
- Passt zur Bare-Metal-Notebook-Historie und zum „recurring saga"-Muster.

### Caveat (ehrlich)
Das eine live berichtete `save … OutOfRange` (ohne Power-Loss) ist damit
**nicht zwingend** erklärt — das kann eine vorher eingepflanzte Inkonsistenz
aus einem früheren unsauberen Shutdown gewesen sein, die später live
hochkam. Darum braucht es §2 (fsck) als Beweis-Instrument: erst damit wissen
wir, ob die Barriere die *letzte* Quelle ist.

### Fix (Skizze)
- NVMe: `nvme_flush()` (Opcode 0x00, nsid) implementieren **oder** FUA-Bit
  (`cdw12 |= 1<<30`) für kritische Writes.
- Commit neu ordnen mit **echten** Barrieren — minimal:
  - Phase 2: Daten + Bitmap schreiben → **FLUSH** ▷ Barriere → dann SB
    schreiben (ggf. FUA) → **FLUSH** ▷ Barriere.
  - Phase 3: journal committed=1 → **FLUSH**.
  - (Die genaue Minimal-Zahl der Barrieren in §4 zusammen mit Perf.)
- HW-**Power-Loss-Test** zur Bestätigung (Stecker ziehen während Write-Last).

---

## 2. KORRUPTION — Zweitfund: kein fsck / Mount-Reconcile

Es gibt **keinerlei** Mittel, doppelt-referenzierte oder
allocated-but-unreachable Blöcke zu erkennen oder zu reparieren. Folgen:
- Korruption wird erst beim zufälligen Descent (OutOfRange) sichtbar — zu spät.
- Wir können **nicht beweisen**, ob §1 die letzte Quelle ist.

### Vorschlag: `fsck`-Intent (read-only zuerst)
Scan über den btree (`btree::iter_all`) → **Block-Refcount-Map** über alle
referenzierten Extents + indirect-chains + btree-Knoten:
- Block 2× referenziert → **DOUBLE-ALLOC** (Korruption).
- Block referenziert, aber in Bitmap als frei → **inkonsistent**.
- Block in Bitmap belegt, aber von niemandem referenziert → **Leak**
  (gc-reclaimbar).
- Extent/Child-Pointer out-of-range → **corrupt pointer**.
Optional `--repair` (vorsichtig, hinter Bestätigung).

Doppelter Nutzen: **Diagnose-Beweis** für §1 **+** Vorher/Nachher-Messpunkt
für die Perf-Arbeit.

---

## 3. PERFORMANCE — wo die Zeit hingeht

### Serielle Kette pro 1-MB-Chunk (Write), ein Core
```
blake3-verify (~0.3ms) → AES-key-expand → alloc+copy 1MB → AES-GCM (~1ms)
  → DMA-write (spin-wait, ~1.9ms)
```
Gemessenes Ceiling ~250–300 MB/s. Raw-NVMe 524 MB/s, AES-NI 737–1163,
BLAKE3 2400+. **Kein einzelner Baustein ist der Engpass — die serielle
Verkettung ist es.**

### Drei getrennte Perf-Achsen

**Achse A — Single-Stream-Durchsatz (enc+dma seriell).**
Die Stufen überlappen nicht. Beim chunked-Format ist jeder 1-MB-Chunk
**eigenständig verschlüsselt** (eigener Key/Nonce aus eigenem Hash) =
*embarrassingly parallel*. Pipeline (Chunk N+1 verschlüsseln, während N DMAt)
**oder** Chunks auf mehrere Cores verteilen → DMA wird der Boden →
**~1,8–2× (Richtung ~500 MB/s)**.

**Achse B — Multicore-Skalierung (DAS Strukturproblem).**
`static FS: Mutex<Option<State>>` (`storage.rs:39`) ist **ein globaler
Spinlock über alles** — cache, bitmap, btree, journal, SB. In `get()`
(`storage.rs:556–646`) wird er **über den ganzen DMA + Decrypt gehalten**.
→ Zwei Cores können **nie** gleichzeitig FS-I/O machen, egal welche Datei.
Bei „wir haben jetzt Multicore" ist das FS der globale Flaschenhals.
- Reads sind content-addressed = **immutable**. Der Lock muss nur
  btree-Lookup + Extent-Liste schützen; **DMA + Decrypt gehen lock-frei** →
  N-Core-Parallel-Reads.
- **Vorsicht:** gegen gc-Free absichern. Reachable-Objekte werden von gc nie
  freigegeben, aber ein lock-freier Read eines gerade von gc freigegebenen
  (orphan) Blocks ist ein Race → braucht Epoch/Refcount- oder
  RW-Lock-Design. Echte Concurrency-Arbeit.

**Achse C — CPU-Multitasking (Spin-Wait).**
`io_command` (`nvme.rs:~`) und `write_blocks_batch` **busy-loopen**
(`core::hint::spin_loop`, bis 5 Mio Iterationen) auf die Completion —
**unter `FS.lock` + `NVME.lock`**. Der Core verbrennt Zyklen im Leerlauf
statt zu yielden → blockiert andere Fibers auf dem Core und andere Cores.
Fix = **IRQ-getriebene Completion + Fiber-yield** = das bestehende
**Host-Device-IRQ-Projekt** (Memory `project_host_device_irq`, MSI-X-Core
schon HW-validiert, NVMe 2185 IRQs Optane). Gibt den Core während I/O frei
und erlaubt höhere Queue-Depth.

### Verschlüsselung konkret
AES-NI **ist aktiv** (737–1163 MB/s gemessen — Soft-AES wäre ~50). Krypto ist
**nicht** der Per-MB-Engpass. Aber:
- `aead_encrypt_aes` (`crypto/aead.rs:271`) nutzt `Aead::encrypt` (nicht
  in-place) → **alloziert neuen Vec + kopiert 1 MB** pro Chunk vor dem
  Verschlüsseln. `encrypt_in_place` in einen Puffer mit Tag-Reserve spart
  alloc + memcpy.
- `Aes256Gcm::new_from_slice` macht pro Objekt eine Key-Expansion
  (per-object-key, nicht cachebar — aber mit AES-NI billig).
- Größter Krypto-Hebel = **nicht ein schnellerer Cipher**, sondern
  Parallelisieren/Pipelinen über Chunks (Achse A).

### Billige Micro-Opts (geringes Risiko)
- **Redundanten `blake3::hash`-Verify in `storage::put` droppen**
  (`storage.rs:376`, ~0,3 ms/MB, „~8% gratis"). Der Hash kam vom paths-Layer
  aus denselben Bytes; Re-Hash kann nur bei HW-Speicherfehler abweichen.
  Ggf. hinter Flag für untrusted Caller behalten.
- In-place-Encrypt (siehe oben).
- `write_extent`-Submissions mit Queue-Depth batchen (Reads tun das via
  `read_multi_extent` schon).
- Block-Cache ist nur **64 Slots (256 KB), global** (`cache.rs:10`) →
  Metadata-Thrashing bei großen Writes. Größer / metadata-only erwägen.

---

## 4. ⚠️ KOPPLUNG Sicherheit ↔ Performance

Die Barriere aus §1 **fügt synchrone Round-Trips pro Commit hinzu** → Small-
File-Write wird *langsamer*, wenn wir nicht gleichzeitig gegensteuern:
- **Flush-Anzahl minimieren:** SB mit FUA + ein FLUSH statt zwei, bei
  korrekter Ordnung.
- **Commits batchen:** mehrere kleine Writes in einen Commit amortisieren die
  Barriere (heute schon teilweise via `pending_old_blocks`-Batching in
  `commit_root`).

→ **Konsequenz:** Commit-Pfad **einmal** umbauen (Korrektheit + Flush-Minimierung
zusammen), nicht zweimal.

---

## 5. Hebel-Ranking (Gewinn / Risiko / Aufwand)

| # | Hebel | Achse | Erwarteter Effekt | Risiko | Aufwand |
|---|---|---|---|---|---|
| 1 | fsck/Self-Check (read-only) | Safety | Beweist Wurzel + Messpunkt | niedrig | M |
| 2 | NVMe FLUSH/FUA-Barriere + Commit-Reorder | Safety | schließt Korruptions-Vektor | mittel | M |
| 3 | Flush-Minimierung + Commit-Batching | A/Safety | hält Small-File-Write trotz Barriere schnell | mittel | M |
| 4 | Lock-free Reads (FS.lock nicht über DMA+Decrypt) | B | N-Core-Parallel-Reads | mittel | L |
| 5 | IRQ-Completion statt Spin-Wait | C | Core frei bei I/O, höhere QD | mittel | L (Projekt läuft) |
| 6 | Pipeline/Parallel enc+dma (chunked) | A | ~1,8–2× Large-File-Write | mittel | M |
| 7 | blake3-verify droppen + in-place-encrypt | A | ~10–20% Single-Stream | niedrig | S |
| 8 | Größerer/metadata-Block-Cache | A/B | weniger Metadata-Thrash | niedrig | S |

(Alle Zahlen = Schätzung. **HW-Messung Pflicht**, nicht raten — und NICHT in
QEMU `cache=writeback`, das lügt bei Write-Zahlen.)

---

## 6. Vorgeschlagene Reihenfolge

1. **fsck/Self-Check (read-only)** — Wurzel beweisen + Baseline messen.
2. **Commit-Pfad gemeinsam umbauen** — FLUSH/FUA-Barriere (Korrektheit) +
   Flush-Minimierung & Commit-Batching (Perf) in einem Rutsch.
3. **Lock-free Reads + IRQ-Completion** (Multicore-Achsen B+C) — der große
   Multicore-Win.
4. **Micro-Opts** (hash-drop, in-place-encrypt, Pipeline) als Politur mit
   HW-Messung.

Reihenfolge laut Florian flexibel — Logik hier: Safety-Beweis zuerst, dann
den Commit-Pfad nur einmal anfassen, dann Multicore, dann Politur.

---

## 7. Offene Entscheidungen (morgen)

1. **Multicore-Tiefe:** reichen lock-free **Reads** als erster Schritt, oder
   auch **parallele Writes** (deutlich mehr Concurrency-Design — feinere
   Locks statt globalem FS.lock)?
2. **Barriere-Strategie:** FUA-pro-kritischem-Write **oder** explizites
   NVMe-FLUSH zwischen Phasen? (FUA = weniger Round-Trips, aber pro Write;
   FLUSH = ein Sync-Punkt, deckt den ganzen Cache.)
3. **VWC abschalten** als zusätzlicher Gürtel? (Einfacher, aber kostet
   Write-Perf permanent — Barriere ist die saubere Lösung.)
4. **fsck-`--repair`** jetzt mitbauen oder erst read-only-Diagnose?
5. **Messbasis:** welche HW für die Vorher/Nachher-Messung — Notebook-NVMe
   (echt) als Referenz, QEMU nur für Reads.

---

## Referenzen (Code)
- Commit-Protokoll: `kernel/src/storage/npkfs/storage.rs:743` (`commit`),
  `:309` (`commit_root`), `:357` (`put`), `:556` (`get`).
- gc: `kernel/src/storage/npkfs/fs.rs:371` (`gc`), `:336` (`mark_file_object`).
- btree COW: `kernel/src/storage/npkfs/btree.rs` (`insert`/`delete`/`fixup_*`).
- Journal: `kernel/src/storage/npkfs/journal.rs`.
- Bitmap: `kernel/src/storage/npkfs/bitmap.rs`.
- Block-Cache: `kernel/src/storage/npkfs/cache.rs:10` (64 Slots),
  `:126` (`flush`).
- NVMe: `kernel/src/drivers/nvme.rs` — `write_blocks_batch:702`,
  `io_command` (Spin-Wait), `write_block:1267`, kein FLUSH/FUA.
- Krypto: `kernel/src/crypto/aead.rs:271` (`aead_encrypt_aes`),
  `:281` (`aead_decrypt_aes_in_place`), `:379` (`derive_object_key`).
- Memory: `project_filesystem_perf`, `project_host_device_irq`.
