# Wo der Wirt CPU verbrennt — Bestandsaufnahme 2026-10-17

Florian, 2026-10-17: „möchte ich besser verstehen wo wir unnötig cpu
verbraten, und ich glaube unsere netzwerkkarte braucht zuviel rechenpower..
hat die karte nicht selber ressourcen um die berechnungen zu machen?"

Vier Durchgänge über den Code (Netz, GPU, Hypervisor, Gast-RAM), jeweils
gegen das Original (Linux igc, KVM, vhost, virglrenderer, i915). **Nichts
davon ist gemessen**; Prozentzahlen sind Schätzungen aus dem Code. Vor und
nach jedem Paket am NUC messen: `cores` (Busy-% je Kern, `net worker`
irq/timeout/polled/busy, Exits/s, vCPU-Phase), `set microvm_stats on`
(`VT_DEVLOCK`), `npkbench_vm` im Gast-cmdline.

Erledigt ist mit ✅ markiert: Paket A in **Kernel 0.622.0** (released);
G12, V5 halb, V9 und R in **Kernel 0.623.0** (lokal committet, **nicht
released**).

## N. Netz — die Karte kann es, wir lassen sie nicht

Die I225 kann Prüfsummen (TX und RX), TSO, RSS über 4 Queues und
Interrupt-Moderation. Genutzt war nur TSO für Gast-Superframes.

| # | Stelle | Was | Linux | Aufw. |
|---|---|---|---|---|
| N1 ✅ | `net_dataplane.rs` | Arbeiter pollte 1 ms-Fenster, solange in den letzten 50 ms ein Paket kam — bei jedem Strom ~1 Kern | vhost `busyloop_timeout` 0; KVM `halt_poll_ns` wächst/schrumpft | S |
| N2 ✅ | `igc.rs` `recv` | RXCSUM-Status verworfen; NAT prüfte jedes TCP-Segment für den Gast in Software | `igc_rx_checksum` → `CHECKSUM_UNNECESSARY` → DATA_VALID | S |
| N3 ✅ | `igc.rs` `send_inner`, `nat.rs` | Prüfsumme jedes ACK/UDP/Einzelsegments in Software | `igc_tx_csum` Kontextdeskriptor + TXSM; Rest `skb_checksum_help` | S–M |
| N4 ✅ | `nat.rs` Prüfsummen | 16-Bit-Schleife | `do_csum` breit — `net/csum.rs` | S |
| N5 ✅ | `nat.rs` UDP eingehend | volle Neuberechnung (verdeckte auch kaputte Datagramme) | `udp_manip_pkt` inkrementell | S |
| N6 | `igc.rs` | 1 Queue, kein RSS, ein Vektor; NAPI-Kern und Arbeiterkern reichen jedes Paket über `TAP` weiter | bis 4 Queues, RSS, Vektor je Queue | L |
| N7 | `nat.rs`, `ipv4.rs`, `eth.rs` | Kopien: RX 4, TX-TSO 4, ACK 6 (+3–4 Allokationen, 2 Nullungen); GRO-Akku 64 KB je Rahmen neu | build_skb, Seiten wiederverwenden, frags | M |
| N8 | `igc.rs:337` | EITR fest 648 (~6000/s), kein Budget je NAPI-Lauf | adaptives ITR, Budget 64 | S–M |
| N9 | `nat.rs:698`, `TX_LIMIT` 256 KB | TSO abgelehnt → Software-Zerlegung in ~44 Segmente | — (Linux wartet in der qdisc) | M |
| N10 | `igc.rs` `tx_reserve` | spinnt bis 200 µs unter `DEVICE` (nur Host-Senden) | `netif_stop_queue` | S |
| N11 | `net/ipv4.rs`, `net/tcp.rs` | Host-Stack prüft eingehende Prüfsummen GAR NICHT (Korrektheit, nicht CPU) | `tcp_v4_rcv` → `skb_checksum_init` | S |
| N12 | `intent/http.rs` | native HTTPS-Abholung pollt mit 10 kHz und hält den Kern | Socket-Warteschlange, NAPI weckt | S–M |

## G. GPU

Das Modul schläft korrekt auf dem MSI, RPS und RC6 laufen; die Messbuilds
0.23.4/.5 (Frequenz fest auf RP0) haben gezeigt: der Takt ist nicht der
Engpass. Es sind Serialisierung und die CPU-Arbeit drumherum.

| # | Stelle | Was | Original | Aufw. |
|---|---|---|---|---|
| G1 ✅ | `gpu/gpudev.rs` `ccmd`, `wait_seq_yield` | Gast-ccmd drehte `spin_loop` auf dem Arbeiterkern | ioctl schläft auf dem Zaun | S |
| G2 ✅ | `mm/mod.rs` `clflush_range` | `clflush` statt `clflushopt` (518k Zeilen je 4K-Bild beim host3d-FLUSH) | `clflush_cache_range` nimmt CLFLUSHOPT | S |
| G3 | `gpudev.rs` (Modul) 360/370/650 | Szenenquellen LLC-gebunden → CPU muss jedes Gastbild clflushen | GPU-geschriebenes Scanout nie per CPU flushen; Quelle uncached binden, nur CPU-Leser flushen. **Braucht Cache-Modus je BO im Job (ABI Kernel↔Modul)** — CPU-geschriebene Fensterpuffer dürfen NICHT uncached gelesen werden | M |
| G4 | Modul `gpudev.rs:725`, `gpgpu.rs:481` | ein Auftrag nach dem anderen; Szene und Kopie warten synchron, Gast-ccmds stehen dahinter; > 100 ms → CPU-Compositing | asynchron, Zaun; Preemption (RIGID 5.6) | M / L |
| G5 | `virtio_gpu_ctx.rs:299` | jeder ccmd eines SUBMIT_3D ein eigener Modul-Job, unter `BACKEND` | ein Strom = ein Job | M |
| G6 | `virtio_gpu_pci.rs`, `shade/mod.rs`, `gpudev.rs:1460` | drei 60-Hz-Uhren, echte vblank kommt nie an (`flip`/`wait_vblank` Stubs), controlq 16 | Zaun je Ausgabe bis zur vblank, Queue 256 (RIGID 5.9 — **erst nach G4 neu versuchen**) | M |
| G7 | `shade/mod.rs:692` | kein Pageflip, Software-Cursor wartet hinter bcs-Kopie | Ebenenflip, Cursor-Ebene | M / L |
| G8 | `shade/mod.rs:489` | Core 0 wartet auf die Szene und hält `CONSOLE` (RIGID 5.1 halb) | Zaun aus der Ereignisschleife | M |
| G9 | `virtio_ccmd.rs:337` | `set_domain` belegt → EAGAIN, Mesa/iHD wiederholen sofort (RIGID 1.9). **Versuch gpu_intel_tgl 0.27.0 zurückgenommen:** den Auftrag im Modul offen halten geht erst, wenn der Kernel Abschlüsse je `seq` führt — `COMPLETED` ist eine Hochwassermarke in Reihenfolge, und der nächste fertige Desktop-Job hätte den offenen ccmd als erledigt gemeldet. Dazu blockiert ein wartender ccmd die ganze controlq (ein Arbeiter) | `i915_gem_object_wait` | M |
| G10 | `virtio_gpu_i915.rs:115` | GEM_CREATE nullt + clflusht je 4K synchron, VT-d je Seite | Seiten bei Bedarf, kohärent ohne Flush | S–M |
| G11 | Modul `i915_request.rs:86` | jede Zaunabfrage ein Hostcall; keine Semaphore zwischen Engines | HWSP als Speicherlesen, `emit_semaphore_wait` | S–M / M |
| G12 ✅ | `gpu_backend.rs:486` | GPU-Arbeiter weckte alle 2 ms; Zaunberichte des Moduls läuten die Glocke schon (`note_gpu_kick`), jetzt 2 ms nur bei offenem Zaun/gehaltenem FLUSH/vblank-Pause, sonst 100 ms | eventfd | S |

## V. Hypervisor

| # | Stelle | Was | KVM | Aufw. |
|---|---|---|---|---|
| V1 ✅ | `vmx/enable.rs`, `svm/enable.rs` `halt_poll` | Gast pollt selbst (`cpuidle_haltpoll`) und schreibt `MSR_KVM_POLL_CONTROL=0`; der Wirt pollte trotzdem bis 200 µs je HLT | `kvm_arch_no_poll` | S |
| V2 | `VmShared::dev` | eine Sperre für fast jeden Exit (auch CPUID/HLT, lockfreie Doorbells, Demand-Fault); blk/sqfs laufen darunter synchron inkl. NVMe + Entschlüsseln | `kvm_io_bus` lockfrei, ioeventfd, Sperre je Gerät | M |
| V3 | `drivers/nvme.rs:272` | MSI-X eingerichtet, Abschluss trotzdem per Spin | IRQ-getrieben | M |
| V4 | `spec_mitigations.rs` | IBPB je Exit (nur Zen 2) ≈ halbe Exitkosten | Return-Thunks — `RETURN_THUNKS.md` | L |
| V5 (✅ halb) | `ept.rs:218` | Demand-Fault nullt 2 MB bzw. 511×4K unter globalen Sperren. **Kernel 0.623.0:** läuft vor der Gerätesperre; Nullen und `ALLOCATOR` bleiben | THP-Fault ohne globale Sperre | S–M |
| V6 | `run_guest_once` | FPU-Tausch je Exit, `current_core_id` per xAPIC-MMIO | Gast-FPU über ganze `KVM_RUN`, RDPID/GS | M–L |
| V7 | Timer/IRQ | Gast-Deadline als Host-One-Shot (Exit + Inject); AMD ohne AVIC | VMX-Preemption-Timer; AVIC | M / L |
| V8 | diverse | 2-ms/3-ms/8-ms-Sicherheitswecker (Netz ✅ jetzt 100 ms bzw. 10 ms mit TCP-Timern, GPU G12, vCPU) | eventgetrieben | S |
| V9 ✅ | `intent/mod.rs:1183` | PS/2-Maus hält Core 0 40 ms aus dem Parken | — | S |

## R. Gast-RAM

✅ **Kernel 0.623.0 (nicht released):** Speicher wird in 2-MB-Blöcken bei
Bedarf belegt und per Balloon-Reporting zurückgegeben, eine größere Zahl
kostet also nichts, bis der Gast sie benutzt. Die feste 2-GB-Konstante ist
weg: Deckel = halber Wirts-RAM (WSL2-Vorgabe; der Seitencache des Gastes
behält, was er gelesen hat), NUC also 8 GB. `RamLayout` teilt wie QEMU:
`[0, 3 GiB)` und ab 4 GiB; e820 mit vierten Eintrag, EPT/NPT je RAM-Slot ein
PD, Bereichsprüfung in `GuestMem`, das GPU-Fenster liegt über dem RAM-Ende
(`hostmem::configure`, pci-hole64).

Offen daneben: Scudo im Gast (`release_to_os_interval_ms=-1`),
`MAX_BLOB_TOTAL` 2 GB, `MAX_GEM_BYTES` 4 GB, kein `DEFLATE_ON_OOM`.
