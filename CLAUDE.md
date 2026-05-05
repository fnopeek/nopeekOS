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

- Language: Rust (no_std, nightly)
- Target: x86_64-unknown-none
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
```

## Current Status

- **Phase:** **12.2 ✅ + 12.3.0–12.3.2 ✅** — virtio-blk
  end-to-end mit npkFS-persistierter encrypted profile-image, virtio-net
  TX/RX-Pfade mit ARP-Loop. Kernel `v0.154.5`, microvm-init `0.3.2`,
  Linux `6.18.26-nopeek` (eigener Build, VIRTIO_BLK=y built-in). Phase
  12.3.3 (NAT zum Host-Stack + Cap-Filter), 12.3.4 (curl/HTTPS-test),
  12.4–12.6 (virtio-gpu, picker, Firefox) plus Microkernel-Refactor
  weiter offen.
- **2026-05-05 (sehr lange session, ~25 commits, v0.148.3 → v0.154.5):**
  - **Cleanup v0.148.4** — bootstrap WASM-Modules add/multiply/hello/fib
    raus (~140 LoC weg).
  - **Sequencing-Decision** — Microkernel-Refactor wandert von
    "zwischen 12.1 und 12.2" zu **nach 12.6 Firefox**. Code-Drift-Argument
    hielt nicht (Host-Backend ist Trap-and-Emulate, Guest-WASM-Driver ist
    Linux-spec-Client — teilen nur die Wire-spec). Time-to-Firefox: 4-6
    statt 6-9 Wochen.
  - **Spec-Update** — at-rest-AEAD ist AES-256-GCM (war ChaCha20 in der
    Spec, falsch — ChaCha lebt nur noch in TLS). Pattern B-mini
    (per-app-downloads-Subtree) vorgezogen in 12.5.
  - **Phase 12.2 (virtio-blk end-to-end), v0.149 → v0.153.1, ~10 commits:**
    - PCI-bus emu (slot 0 host-bridge, slot 1 virtio-blk)
    - BAR-sizing-Handshake (write 0xFFFFFFFF → size mask)
    - Modern virtio cap list (Common/Notify/ISR/Device cfg)
    - MMIO-BAR-trap mit Guest-Page-Walker für Inst-Fetch
      (decode-assists war auf KVM-nested-SVM nicht zuverlässig).
      Vendor-neutral, funktioniert auf VMX und SVM identisch.
    - VIRTIO_F_VERSION_1 + queue_size = MAX_QUEUE_SIZE
    - **Eigener Linux-Build** in `microvm-linux/` — defconfig + overlay
      nopeek-virt.config, VIRTIO_BLK=y/_NET=y/_PCI=y built-in, USB/Sound/
      DRM/HID raus. 9.5 MB bzImage statt Alpine's 12 MB. `bash
      microvm-linux/build.sh` lädt linux-6.18.26.tar.xz von kernel.org.
    - PKU+OSPKE in CPUID 7 ECX maskieren (XSAVE-consistency-check fix —
      sonst panic in `fpstate_reset`).
    - 8259 PIC stub (master/slave IMR readback, ICW1/2/3/4 sequence
      tracking) damit Linux's `request_irq` für virtio-pci INTx-fallback
      nicht -EINVAL zurückbekommt.
    - virtqueue-walker in `devices/virtqueue.rs`: split-virtqueue-Spec,
      avail-ring lesen, descriptor-chains folgen, used-ring schreiben mit
      release-fence. virtio-blk-spezifischer Service: 4 MB
      in-RAM-backing, IN/OUT/GET_ID/FLUSH-handler, status-byte writeback.
    - IRQ-Injection: VMX VM_ENTRY_INTR_INFO_FIELD, SVM VMCB.EVENT_INJ.
      8259-Stub ICW2 trackt Vector-Base damit IRQ 11 auf den richtigen
      Linux-Vector landet.
    - **Profile-Image-Persistenz**: `sys/microvm/profile.img` via npkFS
      auto-AES-256-GCM-encrypted (master_key), upsert-API (insert-or-
      replace). 4 MB save: enc 5.9 ms + BLAKE3 2.4 ms + NVMe-DMA 4 ms =
      ~26 ms. 4 MB load: ~9 ms.
    - PID-1 v0.3.0 erweitert: open(/dev/vda) + read(32 bytes) + hex+ASCII
      log. Magic-pattern "nopeekOS-microvm-blk\0+counter" überlebt zwei
      VM-runs (Run 1 = fresh, Run 2 = loaded → identical bytes).
  - **Phase 12.3 virtio-net (12.3.0–12.3.2), v0.154.0 → v0.154.5:**
    - virtio-net-pci device auf slot 2, VIRTIO_NET_F_MAC + _STATUS
      advertised, MAC `52:54:00:6E:70:6B`, GATEWAY_MAC
      `52:54:00:6E:70:01`, GATEWAY_IP `10.99.0.1`.
    - virtqueue.rs Helpers public (avail_idx/avail_ring/read_desc/used_push)
      damit virtio-net + virtio-blk dieselbe Mechanik teilen.
    - TX-Path: q1-notify → walk avail-ring → concat descriptor chunks →
      log eth+IP+L4-ports.
    - RX-Path: synth ARP-Reply für Gateway-IP — wenn TX ein ARP-Request
      für 10.99.0.1 ist, baue Reply mit GATEWAY_MAC, walke RX-q0-avail,
      schreibe in driver-buffer, used-ring update, IRQ inject.
    - PID-1 v0.3.2: SIOCSIFADDR + SIOCSIFFLAGS via ifreq[40] manuell
      (kein copy_from_slice → kein memcpy-link-error). UDP-poke an
      10.99.0.1:53 triggert ARP. Eth0 Bringup ohne Linux IP_PNP
      (das hängt in unserer microvm-env, late_initcall blockiert PID-1).
    - **End-to-End validated**: PID-1 sendto → Linux ARP-Request → wir
      antworten synthetisch → Linux's ARP-cache populated → echte UDP/IP-
      Frame mit GATEWAY_MAC als dst raus. Voll auf NUC-Hardware bestätigt.
- **Earlier (2026-05-05 — long session: SVM end-to-end + npkfs v3 +
  Popover + wallpapers, v0.142 → v0.148.3):**
  - **Phase 12.1 SVM end-to-end** (v0.142 → v0.143). Linux 6.18 bootet
    auf KVM nested SVM, PID-1 echo-roundtrip funktioniert. Drei Fixes
    auf dem Weg: MSRPM trap-all → pass-through (EFER LME muss durch),
    hypervisor-CPUID-leaf hide (kvm-clock divide-by-zero), 
    `tsc_early_khz=2000000` cmdline (AMD kein CPUID 0x15). Details
    in `memory/project_svm_bringup.md`.
  - **build.sh resource bump** (b2fd120) — qemu-RAM 256 MB → 1024 MB
    + disk.img 256 MB → 1024 MB (microvm linux brauchte mehr).
  - **Loft polish round 5** (v0.2.2 → v0.2.5): bump-allocator-state-
    mutation panic gefixt (alloc_reset BEFORE handle, mark recapture
    AFTER), neuer **`Modifier::Flex(u8)`** in SDK + kernel layout
    (CSS-style flex-grow für non-Spacer Children), magnifier 18 → 24
    px atlas-native, panel-padding raus für edge-to-edge sidebar +
    menu fill.
  - **npkFS Konsolidierung + v3 schema** (v0.145 + v0.146):
    - v0.145 — v1-leftover gelöscht (`btree.rs`, dead code), `v2/`
      subdir flach in `npkfs/` integriert, alle externen `npkfs::v2::*`
      → `npkfs::*` umbenannt (34 refs). Net –838 LoC.
    - v0.146 — schema-bump v2 → v3: `TreeEntry.mtime: u64` (UTC sec
      seit epoch, captured via `rtc::read_unix_time()`), magic
      `npkFS\x02\0\0` → `npkFS\x03\0\0`, mount-time guard für legacy.
      WASM ABIs erweitert: `npk_fs_list` 10 → 19 byte tail per record
      (mtime appended), `npk_fs_stat` 9 → 17 byte. Loft v0.2.4 nutzt
      mtime in der Modified-Spalte.
    - v0.146.1 — followup: loft's `dir_exists` checkte strikt `n == 9`,
      neue ABI gibt 17 → fix auf `n > 0`, npkfs2: → npkfs:
      log-strings.
  - **Echte Popovers** (Phase 11 vorgezogen, v0.147 + loft v0.2.5):
    - **`Modifier::NodeId(NodeId)`** — Widget-Tagging für Anchor-Lookup.
    - **`Widget::Popover { anchor, child, on_dismiss, modifiers }`** —
      finalised, floating layout an anchor-rect (auto-flip oben/unten).
    - Layout returnt jetzt `LayoutOutput { tree, anchors, popovers }`.
      Render: popovers drawn last (top z-order). Hit-test: popovers
      first (reverse-decl), click outside fires on_dismiss (außer auf
      anchor selbst — der toggled).
    - Loft v0.2.5: OpenMenu enum, **Ansicht** dropdown switched
      Grid/List view, **List view** mit Spalten Name/Size/Type/Modified
      (Modified via Howard-Hinnant civil_from_days, "YYYY-MM-DD HH:MM"
      UTC oder "—" bei mtime=0). Datei→Quit, Hilfe→About, Gehe zu
      →Home/Filesystem.
  - **Bundled wallpapers** (v0.148.0 → v0.148.3 + wallpaper v0.4.2):
    - `release/assets/wallpapers/<name>.png` ist die kanonische
      Source-of-Truth. build.sh Pass 2 staged jeden file in
      `install_data/assets/wallpapers/`. BUNDLED_ASSETS-Eintrag
      schreibt nach `sys/wallpapers/<name>` bei seed-time. setup.rs'
      `copy_system_wallpapers_to_user` kopiert nach
      `home/<user>/pictures/wallpapers/<name>` (idempotent — re-run
      clobbert keine umbenannten files).
    - Erstes wallpaper: `npk01.png` (downsized 4K → 1080p, 8.9 MB →
      1.3 MB; 4K-source dropte ~3-5 sec WASM-decode-time).
    - **Wallpaper module v0.4.2**: heap 64 MB → 256 MB (4K-decode
      OOM'd), max-fetch-buf 6 MB → 32 MB (truncierte 9 MB inputs →
      panic), idat-Vec mit `with_capacity(data.len())` pre-sized
      (verhindert ~16 MB doubling-leak im bump-alloc).
    - **`decode_with_wasm`** nutzt jetzt `INTERACTIVE_FUEL`
      (`u64::MAX/2`) statt heuristic — bundled+signed module hat
      keine DoS-surface, fuel-cap dort sinnlos.
  - **Vollständige Iterations-Historie** in
    `memory/project_microvm.md` + neuer `memory/project_npkfs_v3.md`
    + `memory/project_popover.md` + `memory/project_wallpapers.md`.
- **Earlier (2026-05-05 morning — SVM bring-up first push, v0.142 → v0.143):**
  - **v0.142.0 — 12.1.1c-svm Linux-Entry-Pfad** (+628 LoC):
    `enable::run_linux` + `run_linux_loop` + `setup_vmcb_linux` +
    `handle_linux_io` + `SerialState`, `npt::allocate_window_npt`
    (non-identity 256 MB + MMIO-scratch-Alias), VMCB-Konstanten
    (NRIP/CPUID/SHUTDOWN/MSR_PROT/IOIO_PROT/INTR). Substrate-Test
    smoke-validated post-refactor (exit=0x7B byte-identical zu v0.141).
  - **v0.143.0 — 3 Iterationen vom Compile zum echten Linux-Boot:**
    1. **MSRPM trap-all → pass-through** — trap-all absorbed Linux's
       `WRMSR EFER=LME` → CR0.PG ohne LME → legacy 32-bit paging →
       triple-fault nach 8 iters. Pass-through lässt CPU arch-state
       MSRs auto-via VMCB.SAVE handhaben (APM §15.11.1).
    2. **Hide hypervisor CPUID** — Leaf 1 ECX[31] cleared, Leafs
       0x4000_00xx zero. L2 Linux sah L1 KVMs Signature, aktivierte
       kvm-clock, divide-by-zero in `pvclock_tsc_khz` weil unser
       MSR-Handler die KVM_SYSTEM_TIME-Schreibe absorbierte.
    3. **`tsc_early_khz=2000000`** in Cmdline — AMD exposed kein
       CPUID 0x15, Linux fällt auf PIT-Calibration zurück, deadlocks
       gegen unsere Zero-Returning-IO-Emulation. Hint
       short-circuited das. Idle-threshold auch 200 → 5000 INTRs.
  - **End-to-end auf KVM nested SVM**:
    `[guest] [microvm-init] Hello from nopeekOS PID-1` →
    `[guest] [init] echo: hi-svm` → HLT nach 41355 VM-exits.
    Self-bestätigt durch User-Test auf AMD-Box.
  - **build.sh-Bump**: 256 MB → 1024 MB qemu-RAM + disk.img
    (256 MB-RAM OOM'd `microvm linux` weil 256 MB Guest-Window
    + Kernel + Heap nicht reinpasste).
  - **Vollständige Lessons** in `memory/project_svm_bringup.md`.
- **Earlier (2026-05-02 — late stragglers, freeze fix, panic detection, initramfs+pid1, v0.122 → v0.130):**
  - **v0.130 — initramfs + Rust-PID-1 (12.1.3).** Eigene `microvm-init`
    Crate (`microvm/linux/init/`, ~1.3 KB statisch gelinktes Linux ELF),
    no_std, no_main, raw syscalls (write/pause/reboot). Wird bei
    `./build.sh release` via `bsdtar --format newc + gzip` zu
    `release/assets/microvm-initramfs.cpio.gz` (694 Bytes), per ECDSA
    P-384 signiert, im Installer als `sys/microvm/initramfs.cpio.gz`
    in npkFS gepflanzt. `intent::microvm_linux` lädt's via npkfs::fetch,
    übergibt an `vmx::run_linux(bzimage, cmdline, initramfs)`. Loader
    in `bzimage::load_into_guest_ram` legt's bei Guest-Phys 0xC000000
    ab, setzt boot_params.hdr.ramdisk_image + ramdisk_size. Linux
    unpackt cpio → rootfs, exec'd /init. Erstes Userspace-Banner
    erwartet: "[microvm-init] Hello from nopeekOS PID-1".
  - **v0.129 — formal panic-detection (12.1.1d).** SerialState scant
    auf "Kernel panic - not syncing: ", erkennt Panic-Reason, klassifiziert
    den nachfolgenden triple-fault als erwartet. AMD-MSR-Spam-Filter
    daneben (LS_CFG/HWCR/NB_CFG werden auf Intel always-absent → kein
    Log).
  - **v0.128 — Pin-based external-interrupt-exiting fix.** Erster
    `microvm linux` froze NUC komplett (hard-reset nötig), weil
    Pin-based bit 0 = 0 → Host-LAPIC-IRQs gingen während Guest-Run
    direkt in Guest-IDT, mit echtem LAPIC-Acknowledge → ISR-stuck
    → Host-Tastatur/Timer tot nach VMXOFF. Fix: bit gesetzt, IRQs
    causen jetzt VM-exit reason 1, der `sti` am Ende von
    `run_guest_once` lässt den pending IRQ durch Host-IDT laufen.
    Architekturell wichtig: das war ein Host-Config-Bug, kein
    Guest-Escape — VMX-Hardware-Boundary hat gehalten. **Erster
    echter Trust-Boundary-Test bestanden**: Linux gepanict, Host
    bleibt responsiv.
  - **Linux 6.18.26 bootet komplett durch subsys-init.** Final state
    auf NUC: `Kernel panic - not syncing: VFS: Unable to mount root
    fs on "" or unknown-block(0,0)` → `Rebooting in 1 seconds..` →
    triple-fault (exit reason 2). = geplanter v0.121-Endstate, jetzt
    erreicht. 12.1.1c-Serie (3b3b1 → 3b3b23) komplett abgehakt.
  - **6 heutige Patches** räumten late CPU-Feature-Stolperer:
    v0.122 XSETBV-ack, v0.123 RDTSCP secondary-bit, v0.124
    USER_WAIT_PAUSE secondary-bit (für MWAIT-idle), v0.125 XSAVES
    + RDMSR/WRMSR-Handler (AMD-MSRs return 0, others ignore), v0.126
    256 MB Guest-RAM (von 64 → 256, SLAB-init OOM'd vorher) +
    #CP-Trap im EXCEPTION_BITMAP, v0.127 CET-Bits aus Guest-CPUID
    maskiert (CET vom Host, ohne Shadow-Stack-Setup im Guest = #CP).
  - **Pattern für CPUID/MSR-Stragglers etabliert**: enable wenn
    Linux's Code-Pfad's Capability spiegelbar ist (RDTSCP, MWAIT,
    XSAVES), hide wenn Guest dann Setup machen müsste den wir nicht
    spiegeln (CET), stub-return wenn AMD-spezifisch und Linux's
    fallback eh greift (RDMSR 0xc0011029).
  - **Vollständige Iterations-Historie** + Lessons in
    `memory/project_microvm.md`.
- **Earlier (2026-05-01 — Phase 12.1.0 + 12.1.1 in one push, v0.90 → v0.121):**
  - **VT-x MicroVM substrate from scratch to live earlycon-Stream**:
    VMXON/VMCS/VMCLEAR/VMPTRLD round-trip, host-state full round-
    trip mit GDT-walk-resolved TR-Base, TSS-install, VMLAUNCH gegen
    long-mode HLT-loop, EPT (1 GB identity → 16 MB non-identity →
    extension für IOAPIC/HPET/LAPIC-region), real-mode +
    unrestricted-guest, full VMRESUME-Loop mit GPR save/restore,
    CR3-load + I/O-bitmap (alle Ports trapped) + MSR-bitmap (zero)
    + CPUID pass-through + EFER load/save + dynamic IA-32e sync.
  - **bzImage-Loader**: Alpine `vmlinuz-virt` 6.18.26 (12 MB) als
    bundled installer-asset, landet in npkFS bei
    `sys/microvm/linux-virt.bzImage`. 32-bit boot protocol entry,
    boot_params + e820 + cmdline gefügt.
  - **`microvm` Shell-Intent** mit `test` / `linux-info` / `linux`.
    BSP-only (`is_core0_intent`) wegen TR/VMXE-state.
  - **Cmdline-Workaround**: `nolapic noapic acpi=off pci=off
    tsc=reliable` → Linux skipped Hardware-Probing, bootet als
    minimal-PC. Wird zurückgenommen sobald virtio-Backends da sind.
- **Pausiert für 12.1-Komplettierung**: TLS-Hardening
  (eigener TLS-1.3-Handshake `crypto/tls/mod.rs` 967 LoC, Plan
  `rustls` no_std + `rustls-rustcrypto`), TCP-data-retransmit,
  ASN.1-Parser-Swap zu RustCrypto `der`+`x509-cert`. Phase 10
  Polish-Queue (tile-subdivision, static visual effects, canvas
  escape hatch, loft round 4) auch parked.
- **Earlier (2026-04-29 — v0.89 crypto stack + network hardening):**
  - **X.509 conformance** (v0.89.0): full extension parser + chain
    enforcement of KeyUsage (`digitalSignature` for leaf,
    `keyCertSign` for CAs), ExtendedKeyUsage (`serverAuth` /
    `anyExtendedKeyUsage`), BasicConstraints `pathLenConstraint`, and
    rejection of unknown critical extensions. Closes the
    Symantec/DigiNotar-class mis-issuance vectors where a
    serverAuth-only cert could pass as a CA.
  - **RSA verify swap** (v0.89.0): deleted 340 LoC of hand-rolled
    BigInt math (schoolbook mul + long-division mod_reduce, lying
    "Montgomery" doc-comment). Now a thin wrapper over RustCrypto
    `rsa 0.9` + `crypto-bigint` (audited, constant-time). Net –300
    LoC. SHA-1 sig algo dropped from accepted set in the same pass —
    real chains since 2017 are SHA-256+ only and we never verify root
    self-signatures (matched by subject DN against embedded set).
  - **TCP ISN — RFC 6528** (v0.89.0): replaced
    `interrupts::ticks() as u32` with BLAKE3-keyed-hash of
    `(saddr, daddr, sport, dport)` under a per-boot CSPRNG secret,
    plus a tick-derived monotonic offset (~250 kHz step). Defeats
    off-path ISN prediction on listening sockets (debug reverse-mirror,
    future SSH).
  - **ARP cache-miss fix** (v0.89.1): `ipv4::send` used to fall back
    to L2 broadcast on a cold cache → most gateways drop unicast IP
    with broadcast MAC → first SYN dies, TCP-retry waits 1 s for
    passive cache-learn. Symptom: `debug <ip> <port>` needed 2–3
    attempts on fresh boot, fixed by a prior `ping`. Now: `ipv4::send`
    fires `arp::request` on miss (additive, packet still attempted),
    AND `tcp::connect` pre-resolves via new `arp::resolve(ip,
    timeout)` helper before any `CONNECTIONS.lock()` (~500 ms cap).
    First-try success on cold boot.
- **Crypto-stack risks still on the table (audit, 2026-04-29):**
  - TLS 1.3 handshake (`crypto/tls/mod.rs`, 967 LoC) — eigen, no
    audit. Realistic swap target is `rustls` no_std + alloc with
    `rustls-rustcrypto` provider. Eigene Session.
  - TCP data-retransmit fehlt komplett (`send()` is fire-once); SACK
    / window-scaling / timestamps fehlen. Verfügbarkeitsbug, kein
    Security.
  - Eigener kleiner ASN.1-Parser (`crypto/tls/asn1.rs`, 91 LoC) —
    sieht ok aus, defensive Length-Limits, aber CVE-historisch
    bug-empfindliche Ecke. RustCrypto `der` crate wäre der saubere
    Swap zusammen mit `x509-cert`.
- **Earlier (2026-04-28 evening — npkFS perf push v0.86 → v0.88.8):**
  - **NVMe PRP-list extents** (v0.86.0): 1 cmd per FS extent (was 1
    cmd per 4 KB block — 256× fewer SQ round-trips for 1 MB).
  - **NVMe parallel cmds in flight** (v0.87.0): up to 4 cmds on a
    single extent for SSD-channel parallelism.
  - **Bridge: drop redundant BLAKE3 in `fetch`** (v0.86.7): walk hash
    passed through instead of re-hashing plaintext (~0.6 ms/MB).
  - **`Object::decode` in-place** (v0.87.3): `Vec::drain` shifts the
    postcard prefix off — saves the fresh-Vec alloc + 1 MB memcpy
    (~0.9 ms/MB on 1 MB reads, ~13 ms on 16 MB).
  - **`storage::put` dedup-fastpath** (v0.87.6): btree::lookup BEFORE
    BLAKE3-integrity + AES-GCM-encrypt — 2.2 ms/MB saved on
    content-addressed rewrites.
  - **`paths::store` stream-hash** (v0.87.7): blob_content_hash via
    streaming BLAKE3 (no encode pass) → storage::has-skip on dedup
    hit. 1 MB write 325 → 558 MB/s.
  - **Skip BLAKE3-verify in `storage::get`** (v0.88.5): redundant
    against the AES-GCM tag (key + nonce both derived from hash —
    tampering anywhere fails the tag check). +27 % reads.
  - **`read_multi_extent`** (v0.88.8): up to 32 NVMe cmds in flight
    across multiple extents simultaneously — protects against bitmap
    fragmentation (a 1 MB blob split into 257 single-block extents
    used to take 8.5 ms; now ~1–2 ms).

  Bench (testdisk on AirDisk SSD, mixed sizes):
  - 1 MB read: 216 → 411 MB/s, 16 MB read: 195 → 395 MB/s, 100 MB
    read: 406 MB/s
  - 1 MB write (dedup): 208 → 479 MB/s, 16 MB write (dedup): 158 →
    759 MB/s, 100 MB write (dedup): 785 MB/s
  - Total throughput: read 251 → 370 MB/s (+47 %), write 217 → 491
    MB/s (+126 %)
- **Custom AES-GCM skeleton** (v0.88.0–v0.88.4): `crypto/aead_hw.rs`
  + `crypto/aead_hw_ghash.rs` are in-tree but NOT wired into the hot
  path — the custom 4-way-aggregated GHASH math didn't validate
  (`match=false` against `ghash` crate). Storage path back on the
  audited `aes-gcm 0.10`. See `memory/project_perf_session_apr28.md`.
- **Earlier (2026-04-28 morning):**
  - **npkFS v2** — content-addressed Git-style tree objects, real
    directories, walk-by-hash path resolution. Clean break, no
    migration. v1 deleted. See `NPKFS_V2.md`.
  - **HW Crypto + SSE/AVX2 bring-up** — AES-256-GCM (AES-NI +
    PCLMULQDQ), BLAKE3 AVX2, NVMe queue 256 + DMA pool 128, in-place
    AEAD decrypt. CR4 OSFXSR/OSXMMEXCPT/OSXSAVE + XSETBV in
    boot.s/trampoline.s before first Rust instruction. See
    `memory/project_hw_crypto.md`.
- **Resuming next (Phase 10 polish queue):**
  1. **Tile subdivision + full diff cache** — 512×512 grid + per-tile
     content-hash, so hover/key changes only re-rasterize the dirty
     tiles instead of the whole window (~3–5 d).
  2. **Static visual effects** (`Shadow` / `Transition` / `Scale`
     outside pseudo-states) — needs a compositing-layer pass
     (sub-tree → off-screen layer texture → blit with transform). ~1
     Woche, größerer Brocken.
  3. **P10.10 Canvas escape hatch** — `npk_canvas_commit` + `CANVAS`
     cap, on hold until ein konkreter Consumer (image viewer, chart)
     danach fragt.
  4. **Loft polish round 4** — dropdown menus once `Widget::Popover`
     lands (Phase 11+), `.trash`-click crash investigation.
- **Already in-place from earlier rounds (kept here as quick reference):**
  - Vocab v2 shipped (9 Modifier variants — Hover/Focus/Active/Disabled/
    WhenDensity/Rounded/MinWidth/MaxWidth/Scale, Wire ABI 0x01).
  - Apps complete: drun (Mod+D launcher), loft (file browser).
    Both on prefab cookbook (card/button/input/dialog/sidebar_pane/…).
  - SDF rounded corners (Hyprland-style concentric two-arc geometry).
  - `TextStyle::Heading` (ABI variant 5, 18 px regular).
  - `Widget::Input` self-editing — compositor owns cursor + key routing.
  - Layout leaf-padding (Text/Icon/Input/Checkbox/Canvas).
  - Two-theme palette (dark/light/auto, wallpaper-derived accent).
- **Parallel track:** Phase 9 SMP/event-driven (WiFi driver, per-core timer).
- **Completed features + full roadmap:** see `README.md`.
- **Phase 10 detail spec + progress:** see `PHASE10_WIDGETS.md`.
- **Vocab-v2 reference (for AI / app devs):** see `WIDGET_VOCAB.md`.
- **Active work / blockers:** see `memory/project_wifi_current.md`.

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
./build.sh build      # verify it builds
git commit -m "..."   # source change
./build.sh release    # target/ → signed release/
git add release/ && git commit -m "release: sign + publish vX.Y.Z"
git push
```

USB reinstall pulls `target/` directly and bypasses this — that's why
USB-installed builds appeared to work while OTA kept downgrading.

## Security Checkpoint

Before every commit:
"Can a WASM module escape its sandbox through this change?"
If the answer isn't clearly "No" → don't commit.
