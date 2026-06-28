# Befund: Bildschirmauflösung drückt den Download-Speed (Framebuffer ↔ Netzwerk-Pump)

Status: **Hypothese, im Code verankert, noch nicht per Messung bestätigt.**
Zuerst reproduzieren (Abschnitt 4), dann umbauen.

## 1. Was beobachtet wurde

Höhere Bildschirmauflösung → langsamerer Download im Browser-MicroVM.
Auf den ersten Blick unzusammenhängend — ist es aber nicht.

## 2. Der Mechanismus (verankert im Code)

In der ausgelieferten Intel-Konfig (Fiber-Modus, `VMX_VCPU_AS_FIBER = true`,
`kernel/src/microvm/cpu/mod.rs:373`) läuft der Browser-VM **nicht** auf Core 0,
sondern als Fiber auf einem **Worker-Core**. Auf genau diesem Worker-Core laufen
zwei Dinge **seriell im selben VM-Exit-Loop**:

1. **`nat::pump` → `net::poll()`** bei *jeder* VM-Exit-Iteration
   (`kernel/src/microvm/cpu/vmx/enable.rs:1762`, `kernel/src/microvm/devices/nat.rs:844`).
   Das ist der **einzige** Treiber, der den I226-V-RX-Ring leert (Polling-NIC —
   nichts anderes ruft `handle_frame`, `nat.rs:841`).
2. **Die virtio-gpu-Pixelkopie**: `TRANSFER_TO_HOST_2D` (guest→host_pixels,
   `kernel/src/microvm/devices/virtio_gpu_pci.rs:670`) **plus** `write_frame`
   (zweiter voller BGRA→u32-Pass, `kernel/src/shade/surface.rs:75`) laufen
   **synchron im FLUSH-VM-Exit** auf demselben Worker-Core.

Kausalkette:
**höhere Auflösung → Pixelkopie pro FLUSH-Exit kostet mehr (skaliert ~linear mit
der Fläche) → zwischen zwei `net::poll()` vergeht mehr Echtzeit → RX-Ring wird
seltener geleert → TCP-Fenster stallen → Download langsamer.**
Der Pump läuft pro Exit weiter, aber seine **Echtzeit-Kadenz** sinkt.

720p→1080p = 3,69 MB → 8,29 MB pro Frame = **~2,25×**, und mehrfach pro Frame.

Nebeneffekt auf Core 0: Core 0 ruft im Render-Loop *auch* `net::poll()`
(`kernel/src/intent/mod.rs:1010`), es gibt einen Single-Drainer-CAS-Guard
(`kernel/src/net/mod.rs:75`) und das volle Back-Buffer-Recomposite
(`kernel/src/shade/mod.rs:472`, immer `pitch × screen_h`) + MMIO-Blit laufen
**immer auf Core 0**. Core 0 ist also mitbeteiligt, aber der dominante Hebel ist
die Serialisierung **Pixelkopie ↔ Pump auf dem Worker-Core**.

## 3. Wichtigste Einsicht für den Fix

Das Problem ist **Serialisierung, nicht Bandbreite.** Ein Core memcpy't 8 MB in
<1 ms. Der Schaden entsteht, weil die Kopie **mit dem Netzwerk-Pump auf demselben
Core verschränkt** ist und **mehrfach pro Frame** passiert. → Lösung = entkoppeln
und Arbeit reduzieren, **nicht** „schneller kopieren".

## 4. Zuerst: reproduzieren (10 Min, mit vorhandenen Countern)

Mess-Hooks existieren bereits: `net::tcp::debug_progress()` (`in_flight` /
`buffered`) + die Pump-Heartbeats aus der v0.156-Arbeit.

- Gleiche große Datei downloaden, einmal **720p**, einmal **1080p**
  (live-resize via EDID vorhanden).
- Wenn die Hypothese stimmt: bei höherer Auflösung **`in_flight` stapelt hoch /
  `buffered` wächst**, Pump-Heartbeat-Intervalle werden größer → Pump-Starvation
  durch Pixelkopie. Eindeutiges Vorher/Nachher-Signal.

## 5. Fix-Richtung (nach bestätigter Messung)

**Zuerst die zwei „gratis" Sparmaßnahmen (low-risk, unabhängig von Multicore):**
- **Doppelten Voll-Pass killen:** TRANSFER kopiert BGRA, dann macht `write_frame`
  *nochmal* einen vollen BGRA→u32-Pass. Surface nativ in BGRA halten → ein voller
  Pass gespart (~−⅓ Worker-Arbeit pro Frame).
- **Damage respektieren:** Linux schickt in TRANSFER ein Damage-Rect
  (`r.x/y/w/h`), aber fbcon/DRM dirty't oft den ganzen Scanout. Echtes
  Damage-Tracking durchreichen → nur geänderte Rechtecke kopieren.
  (Auch: das Voll-Recomposite in `shade/mod.rs:472` könnte das Tile-Clipping
  nutzen, das der MMIO-Blit schon hat.)

**Dann der eigentliche Hebel:**
- **Pixel-Pfad raus aus dem Pump-Core (Multicore / dedizierter Render-Core).**
  Den teuren Pixel-/Recomposite-Pfad vom VM-Pump-Core wegnehmen, damit
  `net::poll()` nicht hinter dem Memcpy wartet. Pump-Core soll so wenig
  Pixelarbeit wie möglich tun.
  Erwartung: bei hoher Auflösung verschwindet die Download-Delle weitgehend,
  weil die Pump-Kadenz entkoppelt ist.

Reihenfolge: Messung → BGRA-nativ + Damage → Render-Core-Entkopplung.

---

# Das große Ganze: Kontentions-Karte (System-weit)

Das Framebuffer↔Pump-Problem ist **kein Einzelfall**, sondern eine Instanz von
zwei Mustern, die sich durch den ganzen Kernel ziehen.

## Das Muster in einem Satz

Zwei Subsysteme beißen sich genau dann, wenn sie **denselben Core** oder
**denselben globalen Lock** teilen — auch wenn sie fachlich nichts miteinander
zu tun haben. Die heißesten Netz-Stellen sind schon sauber entkoppelt
(POLLING-CAS-Guard, FRAME_POOL, `poll_rx_only`, ENGINE clone-and-drop); die
verbleibende Kontention ist konzentriert und größtenteils billig.

## Was fundamental ist (NICHT jagen)

Core-0-only für Compositor / Framebuffer / MMIO und BSP-IRQ-Routing ist
**fundamental** (Hardware nicht thread-safe, `shade/mod.rs:957`). Das einzige
*inzidentelle* Core-0-Pinning ist die Session-/Intent-Raw-Pointer-Tabelle
(`intent/mod.rs:152-193`) — zwingt Shell-Logik auf Core 0, ist aber kein
Hardware-Zwang.

## Die Kontentions-Hotspots

### ① Globaler `FS`-Lock — die breiteste Fehlkopplung
`kernel/src/storage/npkfs/storage.rs:556-615`. Ein `Mutex` serialisiert *jeden*
Storage-Zugriff **und wird über den langsamen Teil gehalten** (NVMe-DMA +
AES-GCM-Decrypt, ~10 ms / 4 MB). Es warten dadurch aufeinander, ohne Bezug:
Core-0-Render (Icons/Fonts/Wallpaper) ⟷ Worker-Core Browser-Profil-`save()` ⟷
OTA ⟷ idle-GC ⟷ jeder `npk_fs_read` einer WASM-App.
Spürbar: Browser-Tile schließen (`save()` auf Worker) → Compositor-Hitch auf
Core 0.
**Fix (low):** im `get` Baum-Lookup unter Lock, Extent-Liste rauskopieren, Lock
**droppen**, *dann* lesen + entschlüsseln. Der `put`-Pfad macht das beim Encrypt
schon richtig (`storage.rs:390`).

### ② NVMe busy-spinnt ~4 ms und hält dabei ZWEI globale Locks
`kernel/src/drivers/nvme.rs:940`. Completion per Spin-Loop (`50_000_000` Iter),
während `FS.lock` + `NVME.lock` gehalten werden. MSI-X ist allokiert und als
funktionierend bestätigt (`nvme.rs:287`), nur nicht zum Treiben verdrahtet.
**Fix (mittel, größter Einzelhebel):** poll → IRQ. Befreit Core 0 *und* Worker
vom Spin, der #① zusätzlich verschärft.

### ③ Framebuffer ↔ Pump (der Ursprungsfund) — präzisiert
- Doppelter Voll-Frame-Pass: TRANSFER kopiert BGRA, dann `write_frame` *nochmal*
  elementweiser `from_le_bytes`-Pass (`shade/surface.rs:75-77`), obwohl die Bytes
  byte-identisch sind (bewiesen durch `blit_to_host_fb`, das `host_pixels` roh
  ins FB memcpy't). → `copy_from_slice` statt Scalar-Loop, near-zero Risiko.
- `SURFACES`-Lock contended zwischen Worker-`write_frame` und Core-0-Render —
  Buffer ist „already double-buffer-shaped" → Dirty-Vec unter kurzem Lock
  rausswappen, lock-frei blitten.
- Volles `pitch×screen_h`-Recomposite bei *jedem* Surface-Flush
  (`shade/mod.rs:470`), auch wenn nur das Tile sich ändert: ~480 MB/s bei
  1080p60 für nichts.
- Bare Mouse-Move feuert das **volle** `request_render()` statt des fertigen
  Cursor-only-Pfads (`drivers/xhci.rs:1157`).

### ④ Worker-Recv wartet auf Core-0-TX
`kernel/src/net/tcp.rs:933`. `tick_connections` baut+sendet Segmente
(`send_seg`, NIC-Doorbell) **während es `CONNECTIONS` hält** — Worker-`recv`
blockiert hinter Core-0s TX.
**Fix (low):** Segmente unter Lock einsammeln, Lock droppen, dann senden.

### ⑤ Per-Paket-Overhead auf dem RX-Hotpath
`kernel/src/drivers/netdev.rs:220`. `netdev::recv` macht `active()` +
`DEVICE.lock()` **pro Paket** → bei einem Burst N Lock-Zyklen + N Dispatches.
**Fix (mittel):** `recv_batch`/`drain_into`, Lock einmal nehmen, ganzen Ring
ziehen.

### ⑥ Residente Apps stapeln auf einen Worker-Core
`kernel/src/wasm.rs:271`. dock/bar/loft spawnen via `spawn_fiber` →
Work-Stealing „piles every fiber onto the one awake core". Der vCPU-Pfad
vermeidet das bewusst mit `reserve_ap_core` (`microvm/cpu/mod.rs:519`).
**Fix (mittel):** dasselbe `fiber::admit`-auf-distinkten-Core für die Panels.
(Kein Hard-Pin dank `yield_sleep`, aber sie frieren hinter einem co-lokalisierten
Intent.)

### ⑦ `ENGINE`-Lock über den ganzen One-Shot-WASM-Call
`kernel/src/wasm.rs:437`. Zwei `run`/`wallpaper`-Decodes können nicht parallel
laufen. Der resident-Pfad klont die Engine und droppt den Lock sofort
(`wasm.rs:295`) — der One-Shot-Pfad nicht.
**Fix (trivial):** Einzeiler, gleiche Technik.

## Billige Wins, nach Bang/Buck

| # | Win | Aufwand | Effekt |
|---|-----|---------|--------|
| 1 | `FS.lock` vor Read+Decrypt droppen (`storage.rs:603`) | niedrig | entkoppelt **alles** Storage vom langsamen Teil |
| 2 | NVMe poll→IRQ (`nvme.rs`, MSI-X liegt bereit) | mittel | befreit Cores vom 4-ms-Spin-mit-2-Locks |
| 3 | `surface.rs:75-77` Scalar-Loop → bulk copy | sehr niedrig | killt einen Voll-Frame-Pass (FB-Win) |
| 4 | `ENGINE` klonen+droppen (`wasm.rs:437`) | trivial | parallele WASM-Decodes |
| 5 | Kein Voll-Render bei Bare-Mouse-Move (`xhci.rs:1157`) | niedrig | kein Recomposite pro Maus-Delta |
| 6 | `send_seg` aus `CONNECTIONS`-Lock raus (`tcp.rs:933`) | niedrig | Worker-recv wartet nicht auf Core-0-TX |
| 7 | RX-Batch-Drain unter einem `DEVICE`-Lock (`netdev.rs:220`) | mittel | killt Per-Paket-Lock auf RX |
| 8 | Surface-Double-Buffer-Swap (`SURFACES`-Lock kürzen) | mittel | Core 0 ↔ vCPU entkoppelt |
| 9 | Recomposite auf Tile clippen statt Vollbild (`mod.rs:470`) | mittel | ~480 MB/s bei 1080p60 gespart |
| 10 | Residente Fibers auf distinkte Cores verteilen (`wasm.rs:271`) | mittel | keine Panel-Co-Location |

**Wenn nur drei: #1, #2, #3** — räumen die zwei breitesten Fehlkopplungen
(Storage-Lock-über-Crypto, NVMe-Spin) plus den Framebuffer-Doppelpass weg.

## Schon richtig gemacht (nicht anfassen)
POLLING-CAS-Single-Drainer (`net/mod.rs:75`), FRAME_POOL-Recycling
(`nat.rs:192`), `poll_rx_only` ohne tcp-tick/render (`net/mod.rs:107`),
ENGINE clone-and-drop im resident-Pfad (`wasm.rs:295`), Encrypt+BLAKE3 außerhalb
des Locks im `put` (`storage.rs:390`), idle-GC gated auf quiet+unfocused
(`intent/mod.rs:934`), VM-`save()` nur bei close auf Worker-Core (nicht
per-frame), FIBER_QUEUES per-core sharded (`smp/fiber.rs:151`).
