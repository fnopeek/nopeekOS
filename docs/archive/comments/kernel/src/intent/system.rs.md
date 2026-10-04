# `kernel/src/intent/system.rs` @ 5e0102684

## L1 · `use crate::{kprint, kprintln};`

```
//! System intents: status, time, help, about, caps, audit, halt, set/get/config
```

## L60-73 · `pub fn intent_cores() {`

```
/// `cores` / `cpu` — trustworthy per-core CPU instrumentation.
///
/// Diagnosis step 0 for the scheduler rework: the WASM `top` is broken
/// (self-reported busy-TSC can't see spinners; APERF/MPERF absent on
/// AMD/qemu). This measures the opposite, directly: it double-samples
/// the per-core HALTED-cycle counters (recorded at every HLT/MWAIT site)
/// over a fixed window and reports the ground truth.
///
///   BUSY% = 100 − halted%  → a spinning core never halts → shows ~100%
///   HALTS/s + avg residency → many short halts = spurious-wake spin;
///                             few long halts = healthy deep idle.
///
/// Output goes to serial via kprintln (primary I/O), bypassing the
/// broken WASM top entirely.
```

## L79 · `let mut s0 = [(0u64, 0u64); 256];`

```
// Snapshot 1
```

## L95 · `let kl0 = crate::smp::fiber::kick_latency_snapshot(); // clears max`

```
// clears max
```

## L99 · `let rp0 = crate::microvm::devices::net_dataplane::rx_pass_stats(); // clears gap_max`

```
// clears gap_max
```

## L103-105 · `let window_ms: u64 = 500;`

```
// Sample window. Idle Core 0 honestly with HLT (the normal shell-idle
// path) instead of busy-waiting, so Core 0's own halt counter advances
// and its BUSY% reflects reality rather than this command spinning.
```

## L109 · `crate::interrupts::halt_until(Some(deadline), crate::smp::per_core::WAKE_HLT_FALLBACK);`

```
// Core 0 has no periodic tick (stage 3e): halt to the deadline.
```

## L113 · `let wall1 = crate::interrupts::rdtsc();`

```
// Snapshot 2
```

## L134 · `let rp1 = crate::microvm::devices::net_dataplane::rx_pass_stats(); // gap_max = window peak`

```
// gap_max = window peak
```

## L148 · `let halt_pct = ((dhalt as u128) * 100 / (dwall as u128)) as u64;`

```
// True busy% = 100 − (halted cycles / wall cycles).
```

## L152 · `let halts_per_s = dcount * 1000 / window_ms;`

```
// Halt entries per second (window is window_ms long).
```

## L155 · `let avg_us = if dcount > 0 { (dhalt / dcount) / tsc_per_us } else { 0 };`

```
// Average residency per halt, in µs.
```

## L160 · `let role: &str = if c == 0 {`

```
// ROLE: classify from the measured signals, not a hardcoded guess.
```

## L166 · `alloc::boxed::Box::leak(alloc::format!("native task '{}'", t).into_boxed_str())`

```
// Leaked once per `cores` call; a handful of bytes.
```

## L191-195 · `let labels = crate::smp::per_core::WAKE_LABELS;`

```
// Wake-source breakdown: which cause returned each halt this
// window. The decisive number is UNATTR = HALTS − Σcauses: large
// here means the HLT returned with NO guest ISR — KVM resuming
// the vCPU on a host event (host HZ tick) past the emulated HLT.
// That is a QEMU/KVM artifact, not a bare-metal idle bug.
```

## L198-199 · `kprint!("        wakes:");`

```
// Build "cause=N/s" only for non-zero causes to keep it terse.
// (kprintln has no String; print inline per cause.)
```

## L213 · `kprintln!("        timer: fires={}/s late_max={}us",`

```
// Deadline timer: one-shot fires, and the worst wake past its deadline.
```

## L219-221 · `let vlabels = crate::microvm::cpu::VMEXIT_LABELS;`

```
// VM-exit mix — only when a guest ran during the window. Tells us
// WHY the dedicated core is busy: mmio-heavy = the guest is rendering
// (legit); hlt/intr-heavy = idle spin (the run loop should yield/sleep).
```

## L233-236 · `let iolabels = crate::microvm::cpu::IO_PORT_LABELS;`

```
// Break the `io` exit bucket down by port. During heavy RX this is
// expected to be dominated by `pic` (the 8259 EOI, one outb 0x20 per
// device IRQ since the guest runs noapic) — proving the io storm is
// the interrupt-ack path, not the data path.
```

## L285-287 · `let (wi, wt, wp, ws) = (`

```
// Net RX worker wakeup attribution: irq = event-driven (host RX MSI-X
// woke it, ~µs); timeout = fell to the 2ms fallback (host IRQ did NOT
// fire → silent polling = the cold-start floor); polled = no MSI-X.
```

## L299-300 · `let (kk, kt) = (kw1.0.saturating_sub(kw0.0), kw1.1.saturating_sub(kw0.1));`

```
// BSP consumer park: kicked = the worker's kick woke it (event-driven);
// timeout = it fell to the 2ms fallback = the typical ~3ms cold floor.
```

## L306-309 · `let kln = kl1.1.saturating_sub(kl0.1);`

```
// kick→resume LATENCY (the irqfd-gap probe): how long from the worker's RX
// kick to the parked BSP vCPU actually resuming. µs = IPI-prompt (3ms RTT
// is elsewhere); ms = kicked-but-host-descheduled wake (nested oversub) =
// the structural irqfd gap → the real per-packet-round-trip cost.
```

## L317-319 · `let tapfull = rh1.0.saturating_sub(rh0.0);`

```
// Tap backpressure: tapfull/s = the ring was full, so the producer
// dropped (healthy in moderation — the far end slows down). injfalse/s =
// the GUEST had no RX buffer, so the frame stayed in the tap.
```

## L332-335 · `let txp = tx1.0.saturating_sub(tx0.0);`

```
// Outbound TX rate (the b1-vs-b2 upload discriminator). Read TOGETHER with
// the worker core's BUSY% above: high segs/s + worker pegged ~100% = the
// SW-TSO emit pipeline is the cap (b1); the same Mbit with the worker idle
// = cwnd × inflated bridge RTT (b2, an ACK-clock the emit can't lift).
```

## L344-347 · `let rpf = rp1.0.saturating_sub(rp0.0);`

```
// Full-path RX cadence (the rxlat/drops line above is BLIND in full mode).
// batch = avg frames drained per non-empty pass; gap_max = peak µs between
// passes. Decisive read: small batch + ~1.5ms gap = park-cadence (lever a,
// RTT-bound); large batch (+ guest ring full) = receiver-drain (lever b).
```

## L355-357 · `let gt = gt1.saturating_sub(gt0);`

```
// Effective guest HZ: the guest programs 1 kHz (CONFIG_HZ=1000); injected
// only while VMRUN runs, so a parky (slow) connection sees <1000 = the
// timer freezing under the 2ms parks = the "1000 vs 100" lottery.
```

## L361-365 · `let gdelta = gcy1.saturating_sub(gcy0);`

```
// Host-time breakdown: where the dedicated guest cores actually SPENT
// their cycles this window. guest% = in VMRESUME (the guest really ran);
// a high mmio/io% with low guest% PROVES the host burns the core on
// exit-handling (mmio decode / PIC EOI) and the guest is starved — its
// "0% CPU" is because it never gets scheduled, not because nothing runs.
```

## L380-381 · `{`

```
// Where each vCPU's host thread is — printed even when no VM exit
// happened in the window, which is exactly the hang case.
```

## L393 · `{`

```
// Every figure above assumes one TSC across cores (smp::init).
```

## L417-418 · `pub fn intent_history(args: &str) {`

```
/// `history` prints this window's lines, `history clear` wipes the log
/// that survives the reboot along with every window's ring.
```

## L427-442 · `pub fn intent_power(args: &str) {`

```
/// `akku` / `battery` — Smart-Battery diagnostic. Shows whether the i801
/// SMBus controller was found, dumps the raw SBS registers read from the
/// pack at address 0x0B, and prints the decoded charge + status. Lets us
/// tell "no controller" from "controller but no battery on the bus" from
/// "battery present but odd values" without a serial cable.
/// Wieviel Strom zieht das CPU-Package — gemessen, nicht geschaetzt.
///
/// Der Anlass: das IdeaPad zieht im Leerlauf 21,4 W (aus `_BST`,
/// deckungsgleich mit 2,5 h auf 53,5 Wh), dasselbe Blech unter Linux
/// 5-8 W. Es gibt acht plausible Verdaechtige und EINE Zahl; diese hier
/// trennt den groessten Block ab. Ist das Package 3 W, sind C-States und
/// Tickless die falsche Baustelle und der Strom geht an Bildschirm,
/// PCIe-Links, NVMe und die Peripherie.
///
/// Das Fenster wird mit `hlt` verbracht — gemessen werden soll der
/// LEERLAUF und nicht die Messung.
```

## L459-461 · `let secs: u64 = args.parse().unwrap_or(2).clamp(1, 300);`

```
// Laenger als bei `cores`: Energie ist ein Integral, und ein langes
// Fenster mittelt die Zacken weg, die das Dock und die Bar je Sekunde
// machen. `power 5` misst fuenf Sekunden.
```

## L474-475 · `let mut jumps = 0u32;`

```
// The AMD SMU updates the package counter in lumps, rarer in deep idle:
// count the updates, and measure between the first and the last.
```

## L478-479 · `let mut first: Option<(u64, u32)> = None;`

```
// Erste und letzte Fortschreibung: dazwischen ist die Energie ganz
// verbucht, an den Raendern eines festen Fensters nicht.
```

## L483 · `let d = (crate::interrupts::rdtsc() + tsc_hz / 50).min(deadline);`

```
// Kern 0 hat keinen Takt mehr (Stufe 3e): alle 20 ms nachsehen.
```

## L508-509 · `let halt_pct = ((halt1.saturating_sub(halt0) as u128) * 100`

```
// Wieviel des Fensters war Core 0 wirklich angehalten? Eine Wattzahl
// ohne diese Angabe laesst offen, ob gerade Leerlauf gemessen wurde.
```

## L547-551 · `fn power_cstate(arg: &str) {`

```
/// `power cstate` — deep idle, as an experiment you switch on and measure:
///   power cstate            show what the CPU offers
///   power cstate <n> [us]   idle through CStateBaseAddr+n, only for halts
///                           of at least <us> microseconds (default 200)
///   power cstate off        back to hlt (C1)
```

## L603-605 · `if let Some(d) = crate::battery::detail() {`

```
// A notebook whose pack sits behind the EC reports through the firmware
// (`aml`: `_BST`/`_BIF`). That is the common case, and it carries what
// the SMBus path never had: the draw of the WHOLE machine.
```

## L623-624 · `let regs: [(&str, u8); 6] = [`

```
// Raw register dump (each is a 16-bit SMBus word read). None = NAK /
// no device answering at that address/register.
```

## L673 · `let to_mw = |v: u32| -> Option<u64> {`

```
// mA/mAh need the voltage to become mW/mWh.
```

## L705-708 · `fn intent_ec_battery_dump() {`

```
/// Dump the EC's 256-byte RAM so we can reverse-engineer the battery
/// fields on this machine (HP Elite/Dragonfly stores charge as plain EC-RAM
/// fields: remaining cap, full cap, status — read by the DSDT's _BST). Find
/// the offset whose byte ≈ the known charge %, and the 16-bit capacity pair.
```

## L724 · `for row in 0..16usize {`

```
// 16 bytes per row, hex.
```

## L734 · `kprint!("  Candidate %% bytes (val 1..100):");`

```
// Leads: bytes that look like a charge % (1..=100).
```

## L742 · `kprint!("  Candidate capacities (u16 1000..65000):");`

```
// Leads: 16-bit LE values in a plausible capacity range (mAh or mWh).
```

## L752-753 · `let bsel = crate::ec::read(0x86).unwrap_or(0xff);`

```
// Decoded HP-EC battery — offsets from the DSDT Field(ECRM): BSEL/BFC_/
// BRC_/BST_ (mAh). This is what the bar segment shows.
```

## L781 · `let mut line = alloc::format!("  {:05x}: ", i);`

```
// offset + hex + ASCII (field NameSegs are ASCII, easy to spot)
```

## L800-803 · `pub fn intent_dsdt() {`

```
/// `dsdt` — dump only the battery-relevant AML: every EmbeddedControl
/// OperationRegion+Field (field NameSegs → EC byte offsets, e.g. BRC/BFC)
/// and the _BST/_BIF/_BIX methods. Small enough to copy from the console;
/// from this we map the real remaining/full-charge EC offsets.
```

## L812-816 · `let mut dumped: [usize; 8] = [usize::MAX; 8];`

```
// We now know EC0.BTST reads fields BSEL/BST_/BPR_/BRC_/BPV_ and BTIF
// reads BDC_/BFC_/BDV_. We need their EC byte offsets → dump the
// enclosing Field() definition(s). Find each field NameSeg, scan back to
// the FieldOp (0x5B 0x81) that declares it, dump from there so the
// bit-offset accumulation (incl. Offset() skips) is visible from the top.
```

## L824 · `let mut k = 0;`

```
// first occurrence of the NameSeg
```

## L832 · `let mut s = at;`

```
// scan back for the FieldOp 0x5B 0x81
```

## L840 · `dsdt_dump_range(b, name, at.saturating_sub(8), 256);`

```
// no FieldOp found nearby — just dump around the name
```

## L844 · `if nd < dumped.len() && dumped[..nd].contains(&s) { continue; } // dedup`

```
// dedup
```

## L850-853 · `pub fn intent_dsdt_full() {`

```
/// `dsdt full` — base64-dump the entire DSDT to the console, framed by
/// markers, so the aml.wasm interpreter dev-harness can reconstruct the exact
/// bytes the kernel sees (cross-check against Linux's acpidump). Generic ACPI
/// diagnostic — no device-specific logic.
```

## L863 · `let mut i = 0;`

```
// Encode in 48-byte input chunks (→ 64 base64 chars per line).
```

## L885-893 · `pub fn intent_dsdt_send(ip: [u8; 4], port: u16) {`

```
/// `dsdt send <ip> <port>` — stream the raw DSDT bytes over TCP to a
/// `nc -l <port>` listener (exact bytes, no base64, no terminal-mirror ring
/// overflow). Paced in small chunks so the NIC TX ring drains. The DSDT
/// carries its own length at header bytes 4..8, so the receiver can self-verify
/// the transfer is complete. Generic ACPI diagnostic.
/// `dsdt send <ip> <port>`: DSDT and every SSDT, back to back, over one TCP
/// connection. Each table carries its own length at bytes 4..8, so the
/// receiver splits them (`nc -l <port> > tables.bin`). The SSDTs belong in
/// it: CPU `_CST`, the display's `_BCL`/`_BCM` and more often live there.
```

## L918 · `let b = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };`

```
// SAFETY: `acpi::dsdt` / `find_table_nth` return mapped tables.
```

## L929-930 · `let t = crate::interrupts::rdtsc();`

```
// Pace ~10 ms per KB as before: without it only 61 of ~200 KB
// arrived — the close overtook what was still queued.
```

## L943-945 · `pub fn intent_ec_watch(args: &str) {`

```
/// `ec watch [s]` — read-only look at how firmware events reach us: is ACPI mode on
/// (PM1_CNT.SCI_EN), which GPE status bits rise, does the EC raise SCI_EVT.
/// Nothing is written and no event is taken — aml keeps draining the EC.
```

## L948-949 · `let (take, rest) = match args.trim().strip_prefix("take") {`

```
// `ec watch take [s]`: also fetch each event (QR_EC) and print the raw
// answer — this steals it from aml, whose `_Qxx` then does not run.
```

## L960 · `let (smi_cmd, acpi_en, pm1a_evt, pm1a_cnt, gpe0, gpe0_len, flen) = unsafe {`

```
// SAFETY: FADT mapped above; fields at their ACPI 6.5 §5.2.9 offsets.
```

## L976 · `let cnt = unsafe { inw(pm1a_cnt as u16) };`

```
// SAFETY: PM1a_CNT is an I/O port named by the FADT.
```

## L988 · `v[i] = unsafe { inb(gpe0 as u16 + (off + i) as u16) };`

```
// SAFETY: inside the GPE0 block the FADT names.
```

## L1005 · `let mut last_ec = unsafe { inb(0x66) };`

```
// SAFETY: EC status port and PM1a status, both I/O ports of the FADT/EC.
```

## L1078 · `kprintln!("[npk] WARNING: This will disable the display!");`

```
// Test PLL re-lock with firmware values (will kill display!)
```

## L1103-1104 · `let _pre_log = crate::serial::stop_capture();`

```
// Capture serial output during init (survives black screen)
// Stop normal capture, start fresh for GPU init
```

## L1111 · `let gpu_log = crate::serial::stop_capture();`

```
// Save GPU init log to npkFS (readable after reboot)
```

## L1113 · `crate::serial::start_capture();`

```
// Restore pre-existing capture
```

## L1116 · `let log_data = alloc::format!("{}\n--- GPU INIT RESULT: {:?} ---\n", gpu_log,`

```
// Store log in npkFS (unencrypted, no cap needed — use zero cap)
```

## L1160-1161 · `crate::framebuffer::init_from_gpu();`

```
// Always reinit console — display hardware is already at new mode
// even if pipe re-enable timed out
```

## L1185 · `if let Some((phys_a, phys_b, pages)) = crate::framebuffer::shadow_phys_info() {`

```
// Map shadow buffers into GGTT for GPU blit
```

## L1234 · `let bcs_ok = crate::gpu::supports_blit();`

```
// BCS blitter status
```

## L1237-1239 · `kprintln!("  Verified: {} (readback={:#010x})",`

```
// Readback self-test: did the blit actually PAINT? Distinguishes
// "copy broken" (verified=no) from "copy ok but scanout wrong"
// (verified=yes but screen black) on Tiger Lake bring-up.
```

## L1261 · `if crate::gpu::is_native() {`

```
// BCS register dump (always, for debug)
```

## L1267 · `if let Some((pa, pb, pages)) = crate::framebuffer::shadow_phys_info() {`

```
// Shadow buffer info
```

## L1295-1298 · `"windows" | "wins" => {`

```
// Die Fensterliste mit ihrem ECHTEN Zustand — Art, Terminal, PID,
// Wurzel-Flag. `window_lines` (was das Dock liest) zeigt nur Titel,
// und genau daran laesst sich nicht sehen, warum eine „geschlossene"
// App weiterlebt.
```

## L1304-1306 · `let mut busy = alloc::string::String::new();`

```
// Und wer haelt noch ein Terminal besetzt? Ein Fenster kann weg
// sein, waehrend die App darin weiterlaeuft — dann steht hier
// ein Terminal auf `app`, zu dem es kein Fenster mehr gibt.
```

## L1326 · `for i in 0..8u8 { crate::intent::destroy_session(i); }`

```
// Destroy pre-shade sessions so terminals start clean
```

## L1418 · `pub const BOOT_LOG_DIR:       &str = "sys/log";`

```
/// Where `persist_boot_log` files this boot, and the one before it.
```

## L1423-1426 · `fn print_log(log: &str, pat: &str) {`

```
/// Print only the lines containing `pat` (case-insensitive), or everything
/// when `pat` is empty. A boot log is thousands of lines; the one line that
/// answers the question is somewhere in the middle, and there is no generic
/// `>` redirect to pipe it through `grep`.
```

## L1448-1449 · `let a = args.trim();`

```
// `dmesg [prev] [pattern]` — the pattern is what makes a boot log usable
// when the interesting line scrolled past twenty minutes ago.
```

## L1456-1457 · `if prev {`

```
// A boot that ended in a reboot is exactly the one you want to read
// afterwards, and by then it is no longer in RAM.
```

## L1469 · `let log = crate::serial::stop_capture();`

```
// Stop capture, print, restart — so dmesg output itself isn't appended
```

## L1474 · `print_log(&log, pat);`

```
// Print without going through capture (direct serial + framebuffer)
```

## L1480-1485 · `pub fn persist_boot_log() {`

```
/// File this boot's log in npkFS, keeping the previous one alongside it.
///
/// Called once the system is up: everything before this point is in the
/// snapshot, and the master key exists so the object is encrypted at rest
/// like every other. A boot that never gets here leaves the previous log
/// untouched — which is the one worth reading in that case.
```

## L1490-1492 · `if let Err(e) = crate::npkfs::fs::ensure_dirs(BOOT_LOG_DIR) {`

```
// `store` walks to the parent and fails if it isn't there — it creates
// no intermediate directories. `ensure_dirs` is the idempotent variant
// the mkdir intent uses; it is fine with the directory already existing.
```

## L1498-1499 · `if let Ok((prev, _)) = crate::npkfs::fetch(BOOT_LOG_PATH) {`

```
// Rotate by copy, not `rename`: rename refuses an existing target, and
// the store is content-addressed so this costs a reference, not bytes.
```

## L1506-1507 · `Ok(_)  => kprintln!("[npk] boot log: {} ({} bytes)", BOOT_LOG_PATH, log.len()),`

```
// One line, so it is self-evident from the log itself that the log
// was filed — and where.
```

## L1529 · `option_env!("RUSTC_VERSION").unwrap_or("nightly")`

```
// Embedded at compile time via env
```

## L1595-1596 · `fn help_row(cmd: &str, what: &str) {`

```
/// One command and what it does. `*` makes it a status line, so the terminal
/// colours the marker and steps `(…)` asides back — see `shade::terminal`.
```

## L1601 · `fn help_group(label: &str, cmds: &str) {`

```
/// One group in the overview: a short label, then the commands it holds.
```

## L1606 · `fn help_note(text: &str) {`

```
/// A dimmed aside: subtitles, hints, "see also". `.` dims the whole line.
```

## L1617-1621 · `pub fn intent_help_topic(topic: &str) {`

```
/// Help. ASCII only, on purpose: the terminal font draws 0x20..0x7E and
/// silently SKIPS anything above it, while the column arithmetic still counts
/// the bytes. The old help was full of `─`, `·` and `✓` — 297 of them — so
/// every rule appeared as a blank line and every separator as a gap. It only
/// ever looked right on the serial console.
```

## L1849-1851 · `if key == "code.scheme" {`

```
// `code.scheme` is a closed set — a typo would otherwise store
// silently and fall back to auto, which looks like the key did
// nothing at all.
```

## L1861-1863 · `if let (Some(scheme_light), theme_light) =`

```
// Honour an explicit mismatch, but say it out loud — a light
// scheme on a dark canvas is legible-ish, the other way round
// is not.
```

## L1877-1878 · `if key == "shade.chrome_opacity"`

```
// The shared panel knob is the master: it drops any per-panel
// override so it always moves bar AND dock.
```

## L1892-1893 · `pub fn intent_unset(args: &str) {`

```
/// `unset <key>`: forget a setting. For the glass keys that means "back to
/// automatic" (`shade glass` shows what automatic chose).
```

## L1908-1912 · `fn apply_config_change(key: &str) {`

```
/// Make a changed rendering key visible at once, not on the next incidental
/// redraw. Panel translucency is baked into the panel's pixel buffer at
/// rasterize time, so a recomposite alone would show the old value until the
/// app next commits (up to a minute for the bar) — the cached scenes are
/// re-rasterized first, the same step the `theme` intent takes.
```

## L1958 · `core::arch::asm!("cli");`

```
// Disable interrupts first
```

## L1961 · `crate::acpi::reset();`

```
// Method 1: ACPI reset register (if available from FADT)
```

## L1964-1965 · `core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x02u8);`

```
// Method 2: PCI CF9 reset (Intel chipsets)
// Must write 0x02 first (enable reset), then 0x06 (trigger)
```

## L1971 · `core::arch::asm!("out dx, al", in("dx") 0x64u16, in("al") 0xFEu8);`

```
// Method 3: Keyboard controller reset (port 0x64)
```

## L1975 · `let null_idt: [u8; 6] = [0; 6];`

```
// Method 4: Triple-fault (guaranteed reboot on any x86)
```

## L2043-2044 · `pub fn intent_mouse(args: &str) {`

```
/// `mouse [speed <n>] [size <n>]` — show or set pointer speed (25..=600 %) and
/// cursor size (50..=300 %). Persisted to `mouse_speed` / `mouse_size` config.
```

## L2079-2081 · `pub fn intent_lsusb() {`

```
/// `usb` / `lsusb` — enumerate every device on every xHCI controller and
/// print VID:PID + class + product. Identifies dongles (NICs etc.) so the
/// driver catalog can match the right WASM driver.
```

## L2134 · `(0x8086, 0x2723) => "Intel Wi-Fi 6 AX200",`

```
// Intel WiFi
```

## L2148 · `(0x8086, 0x15F3) => "Intel I225-V (2.5GbE)",`

```
// Intel Ethernet
```

## L2157 · `(0x8086, 0x46A6) => "Intel Alder Lake-N [UHD Graphics]",`

```
// Intel GPU
```

## L2168 · `(0x8086, 0xF1A8) => "Intel SSD 660p/670p",`

```
// Intel NVMe
```

## L2171 · `(0x8086, 0x4617) => "Intel Alder Lake Host Bridge",`

```
// Intel Host Bridge / ISA / misc
```

## L2194 · `(0x8086, 0x54C8) => "Intel Alder Lake-N HD Audio",`

```
// Intel HD Audio
```

## L2199 · `(0x8086, 0x54BE) => "Intel Alder Lake-N PCIe RP #7",`

```
// Intel PCI-to-PCI bridges (Alder Lake-N)
```

## L2203 · `(0x8086, 0x461E) => "Intel Alder Lake Thunderbolt 4",`

```
// Intel Thunderbolt / USB
```

## L2209 · `(0x144D, 0xA808) => "Samsung 970 EVO Plus",`

```
// Samsung NVMe
```

## L2213 · `(0x1AF4, 0x1000) => "VirtIO Network (legacy)",`

```
// Virtio (QEMU)
```

## L2219 · `(0x10EC, 0x8168) => "Realtek RTL8111/8168",`

```
// Realtek
```

## L2225 · `(0x1E4B, 0x1202) => "MAXIO MAP1202 NVMe SSD",`

```
// MAXIO NVMe
```

## L2228 · `(0x8086, 0x100E) => "Intel 82540EM (QEMU e1000)",`

```
// QEMU/VBox
```

## L2246 · `core::arch::asm!("out dx, al", in("dx") 0xf4u16, in("al") 0u8);`

```
// Try QEMU exit (harmless on real hardware)
```

## L2249 · `crate::acpi::power_off();`

```
// ACPI S5 power-off (port discovered from FADT at boot)
```

## L2252 · `let slp_s5: u16 = (5 << 10) | (1 << 13);`

```
// Fallback: hardcoded common PM1a_CNT ports
```

## L2257 · `let null_idt: [u8; 6] = [0; 6];`

```
// Last resort: triple-fault reboot
```

