# `kernel/src/main.rs` @ 5e0102684

## L1-4 · `#![no_std]`

```
//! nopeekOS Kernel
//!
//! Not Unix. Not POSIX. No legacy.
//! A system built for AI as the operator, with humans as the conductor.
```

## L14-16 · `pub mod boot_info;`

```
// Boot info handed off from the UEFI stub to kernel_main.
// `mod boot_uefi` does the UEFI-side collection; `kernel_main` reads
// the populated struct without any UEFI ABI knowledge.
```

## L20 · `mod drivers;`

```
// ── Module groups ──────────────────────────────────────────────
```

## L41 · `mod interrupts;`

```
// ── Standalone modules ────────────────────────────────────────
```

## L87-91 · `kprintln!("[npk] AI-native Operating System v{}", env!("CARGO_PKG_VERSION"));`

```
// Die ECHTE Version, nicht eine hartkodierte. Hier stand seit je
// "v0.1.0", und eine falsche Zahl ist schlechter als keine: einen
// Geraetelauf, der das alte Bild gebootet hat, erkennt man sonst nur am
// WORTLAUT einer Logzeile — und das nur, wenn man sie gerade geaendert
// hat. [[feedback_log_the_version_in_the_trace]]
```

## L112 · `serial::start_capture();`

```
// Start capturing boot log for debug shell (needs heap)
```

## L118 · `framebuffer::init_from_boot_info(boot_info);`

```
// Framebuffer init (needs memory + paging for MMIO mapping)
```

## L121-122 · `acpi::set_rsdp(boot_info.acpi_rsdp);`

```
// ACPI: cache the RSDP the UEFI stub picked up via the
// EFI Configuration Table, then walk RSDT/XSDT for the FADT.
```

## L131-132 · `smbus::init();`

```
// SMBus host controller (Intel i801) — used by the battery status driver
// on notebooks. No-op when absent (desktops/QEMU).
```

## L135 · `if xhci::init() {`

```
// USB keyboard (xHCI) — before any user input is needed
```

## L139-143 · `if xhci::init_mouse() {`

```
// Die Maus NICHT geschachtelt: sie haengt am Controller, nicht an der
// Tastatur. Steht die Tastatur am i8042 und die Maus an USB, gab die
// alte Schachtelung gar keinen Zeiger — `init()` meldet false, weil es
// nach einer TASTATUR sucht. `init_mouse` faellt ohne laufenden
// Controller von selbst durch.
```

## L148-149 · `if keyboard::init_mouse() {`

```
// PS/2 touchpad/mouse on the i8042 aux port — only when there's no USB
// mouse (e.g. the notebook's internal touchpad). Gives a basic cursor.
```

## L154-155 · `interrupts::init_apic_timer();`

```
// APIC timer: if PIT doesn't work (NUC/UEFI-only), use Local APIC for 100Hz ticks.
// Must be after xhci::init so poll_events_irq can drain USB events.
```

## L157-158 · `ioapic::init();`

```
// I/O APIC: found and every non-firmware pin masked; nothing is routed
// through it until a driver asks (docs/plan/CORES_AND_EVENTS.md, 3a).
```

## L161 · `smp::init();`

```
// SMP: discover cores via ACPI MADT, boot Application Processors
```

## L164-166 · `smp::fiber::self_test();`

```
// Fiber scheduler Stage 1: validate the context-switch primitive on
// Core 0 (see docs/plan/SCHEDULER_FIBERS.md). Prints `[fiber] self-test OK`.
// Isolated — nothing in the live app path uses fibers yet.
```

## L169-172 · `tss::init();`

```
// TSS install (BSP). Replaces the boot GDT with a clone that has
// a real long-mode TSS descriptor in slot 3, then `ltr`s it.
// VMX host-state validation rejects HOST_TR_SELECTOR=0, so this
// must run before microvm::init.
```

## L175-177 · `microvm::init();`

```
// MicroVM (Phase 12): vendor-detect (Intel VMX / AMD SVM), probe
// capabilities. Host-state setup reads TR via `str` and walks the
// GDT for the TSS base — both covered by tss::init() above.
```

## L191 · `if let Some(t) = rtc::read_unix_time() {`

```
// RTC: immediate wall clock (no network needed)
```

## L201-215 · `let net_up = virtio_net::init() | intel_nic::init();`

```
// Both PCI NICs unconditionally (`|`, not `||`): a short circuit left the
// second one uninitialised on a machine with two, and the survivor then held
// the whole data path — including traffic that belonged elsewhere.
//
// The USB dongle is NOT free to probe: nic_attach halts and RESETS an xHCI
// controller, which drops every device already addressed on it. So it is only
// worth that price when no PCI NIC came up (the HP notebook, which has no
// wired port).
//
// Seit Kernel 0.366.0 ist der Preis kleiner: `nic_attach` probiert den
// Controller, auf dem Tastatur/Maus stehen, ZULETZT — und wenn der Dongle
// auch dort nicht ist, zaehlt es sie wieder auf. Auf einer Maschine mit
// zwei Controllern (IdeaPad) ueberlebt die USB-Maus den Scan damit ganz.
// Haengt der Dongle am SELBEN Controller, gewinnt weiterhin er; das loest
// erst ein gemeinsamer Controller-Zustand (docs/plan/INPUT_I2C_HID.md).
```

## L219-223 · `if !xhci::mouse_available() && keyboard::init_mouse() {`

```
// Der Dongle-Scan hat jeden angefassten Controller zurueckgesetzt,
// also auch eine USB-Maus darauf. Jetzt ist der PS/2-Zeiger wieder
// frei: oben stieg `init_mouse` vor seiner eigenen Diagnose aus,
// weil die USB-Maus da noch lebte — und danach wusste niemand, ob
// am Aux-Port ueberhaupt etwas haengt.
```

## L228-230 · `keyboard::enable_irq();`

```
// The i8042 by interrupt (stage 3b) — only now: `init_mouse` above reads
// the controller's answers itself, and an active IRQ would take them.
// The tick still drains as fallback.
```

## L235-238 · `netdev::refresh_link_state();`

```
// Fill the carrier cache BEFORE the first DHCP. send() refuses to use an
// interface without a link, and the wired carrier is only known through
// this cache — which is otherwise first written by the ~1 Hz link tick,
// long after boot DHCP has already given up.
```

## L242-243 · `if net::dhcp::run_blocking(5000) {`

```
// Boot is the one place where waiting is right: nothing else runs yet,
// there is no prompt to take away, and NTP below wants an address.
```

## L247-248 · `net::seed_active();`

```
// Seed the active-interface tracker so the periodic link tick only
// re-configures on a real change (cable pulled, WiFi associated).
```

## L260-261 · `net::napi::start();`

```
// From here the host NIC is drained on its RX interrupt, not by
// Core 0's shell loop.
```

## L269 · `kprintln!("[npk] Initializing WASM Runtime...");`

```
// Select random color scheme for login screen aurora background
```

## L271-272 · `kprintln!("[npk] Initializing WASM Runtime...");`

```
// Debug shell disabled — enable when needed:
// if netdev::is_available() { shell::start_debug_listener(); }
```

## L278-283 · `let mut mounted = false;`

```
// === Identity: Passphrase → Master Key ===
//
// First boot:       Setup wizard (storage, name, passphrase, settings)
// Subsequent boots: Enter passphrase → verify against keycheck
//
// No users. No accounts. Your passphrase IS your identity.
```

## L285 · `let mut mounted = false;`

```
// Try to mount existing npkFS first
```

## L288 · `if install::has_installer() && nvme::is_available() {`

```
// Installer build (USB stick): always install to NVMe, never ask for passphrase
```

## L303-309 · `let partition_found = if nvme::is_available() {`

```
// Normal boot: detect GPT partition layout, mount existing npkFS.
// Set BOTH offset AND size — without the size, block_count()
// overshoots into the backup-GPT region and the bitmap can
// hand out blocks that fail to write with OutOfRange.
// A GPT we cannot read is not the same as a disk without one.
// Collapsing both into "offset stays 0" pointed the superblock
// ring at the GPT and the ESP — and the format below at them too.
```

## L332-336 · `kprintln!("[npk] npkfs: mount failed: {}", e);`

```
// "mount failed" and "there is no filesystem here" are
// different facts. This branch used to format on either,
// so a single unreadable superblock destroyed the
// installation it was meant to open — and looked, from
// the outside, like the filesystem had broken by itself.
```

## L350-352 · `let gpt_says_raw = matches!(gpt::has_gpt_header(), Some(false));`

```
// Format ONLY on a disk that is provably empty where we
// would write: every slot read, every slot zero, and no
// partition table we failed to place ourselves inside of.
```

## L376 · `let salt = npkfs::install_salt().unwrap_or_else(|| {`

```
// Per-installation random salt (generated at mkfs, stored in superblock)
```

## L387 · `if !setup::run_fresh_install(&salt) {`

```
// === First boot: Setup Wizard (identity, settings) ===
```

## L394-398 · `install::seed_bundled_assets();`

```
// Seed bundled assets into npkFS now that the master key is set,
// so font + WASM modules end up AEAD-encrypted like everything
// else. No-op on non-installer builds. If the user re-runs the
// installer on a dirty partition, we get a fresh seed with the
// new master key.
```

## L401 · `if framebuffer::is_available() {`

```
// === Subsequent boot: Verify passphrase ===
```

## L403-407 · `if gpu::native_detected() && gpu::native_auto_activate_ok() {`

```
// Activate native GPU + 4K before login screen — ONLY for
// validated generations (ADL-N). Other detected Gen12 (Tiger
// Lake) stay on GOP at boot (visible) and are activated manually
// via `gpu init` for bring-up, so an unproven BCS scanout can't
// black the desktop after every login.
```

## L417 · `let _master_key = gui::login::run(&salt);`

```
// Graphical login screen
```

## L420-423 · `if gpu::is_native() && gpu::supports_modeset() && gpu::current_hz() < 60 {`

```
// Auto-upgrade to highest refresh rate if monitor is now connected.
// Only on GPUs with a validated modeset path — a blit-only takeover
// (non-ADL-N Gen12 / Tiger Lake) must keep the firmware mode, else
// the DPLL/pipe reprogram blacks the scanout after login.
```

## L435 · `text_mode_auth(&salt);`

```
// Fallback: text-mode login (serial only, no framebuffer)
```

## L441 · `if framebuffer::is_available() {`

```
// Suppress framebuffer immediately after login (shade takes over)
```

## L446 · `config::load();`

```
// Load system config (after identity — config is encrypted at rest)
```

## L448-450 · `wasm::load_engine_default();`

```
// Welcher Motor die Module faehrt, steht in der Konfiguration und
// uebersteht damit einen Neustart — nur so laesst sich pruefen, was ueber
// Autostart und den Treiberweg hochkommt.
```

## L460-462 · `gui::text::init();`

```
// Inter Variable UI font — read from npkFS (seeded by installer), BLAKE3
// verified, parsed via fontdue. Login screen + terminals use Spleen
// bitmap; the UI font is only needed once widgets come up.
```

## L465-466 · `gui::icons::init();`

```
// Phosphor icon atlas — alpha-only bitmaps from npkFS
// (sys/icons/phosphor), parsed + cached for the CPU rasterizer.
```

## L469-473 · `tls::certstore::load_store();`

```
// Trust anchors delivered as data (sys/certs) — OTA assets plus
// whatever was trusted by hand. Loaded here, after login, because
// npkFS content is AEAD-encrypted and unreadable before the master
// key exists. Until this point the built-in floor is the trust set,
// which is exactly what the OTA path needs and no more.
```

## L476-478 · `gpu::ggtt_slab::init();`

```
// GGTT slab allocator — bookkeeping for tile / comp-layer / glyph
// slots in the GGTT slab region. Pure in-RAM tracker; actual GGTT
// writes land once the rasterizer (P10.5) is wired up.
```

## L481 · `intent::setup_home();`

```
// Create home directory and set as working directory
```

## L489 · `let session_id = {`

```
// Delegate a console session from root (no DELEGATE/REVOKE rights)
```

## L503 · `shell::start_listener();`

```
// Start npk-shell listener (encrypted remote access, port 4444)
```

## L509 · `if framebuffer::is_available() {`

```
// Start shade compositor (GUI_MODE already set after login)
```

## L511 · `if gpu::is_native() {`

```
// Initialize BCS blitter engine (GPU blit instead of CPU copy)
```

## L529-531 · `shade::start_autostart();`

```
// Autostart apps (e.g. the dock) via the widget-spawn path —
// never via `run`, which would capture input. Names from the
// `autostart` config key.
```

## L533 · `intent::apply_startup_wallpaper();`

```
// The stored wallpaper, or a random one if none was ever chosen.
```

## L538 · `intent::system::persist_boot_log();`

```
// Everything above is now in the log; file it before the loop takes over.
```

## L541-545 · `smp::fiber::admit_with_stack(0, shell_fiber, 0, 2 * 1024 * 1024);`

```
// The shell runs as a fiber on Core 0 (docs/plan/CORES_AND_EVENTS.md,
// 3c-1): Core 0 gets the same scheduler as a worker, so the compositor
// can become a fiber beside it (3c-2). 2 MiB like the boot stack —
// Core-0 intents run deep chains (TLS, HTTP) inline, and a fiber stack
// has no guard page.
```

## L561 · `fn text_mode_auth(salt: &[u8; 16]) {`

```
/// Text-mode passphrase authentication (fallback when no framebuffer).
```

