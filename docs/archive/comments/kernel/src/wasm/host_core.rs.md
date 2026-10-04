# `kernel/src/wasm/host_core.rs` @ 5e0102684

## L1-10 · `#![allow(clippy::too_many_arguments)]`

```
//! Host-Funktionen, motorneutral.
//!
//! Jede Funktion hier arbeitet auf `(&mut HostState, Argumente)` und sonst
//! nichts. Was sich je Motor unterscheidet, ist nur, WIE der Zustand
//! beschafft wird: der Interpreter reicht `caller.data_mut()` durch, der
//! Compiler holt ihn aus dem vmctx. Beide fahren dieselbe Implementierung —
//! zwei Host-Schichten zu vergleichen wuerde die Host-Schichten messen.
//!
//! Hier stehen die Funktionen, die den Gastspeicher NICHT anfassen. Wer ihn
//! braucht, bekommt zusaetzlich `mem: &mut [u8]`.
```

## L26-27 · `pub(crate) fn read_str(data: &[u8], ptr: i32, len: i32) -> Option<String> {`

```
/// Eine UTF-8-Zeichenkette aus dem Gastspeicher. Der Rumpf ist der von
/// `read_wasm_str`, nur ohne den Motor davor.
```

## L37 · `pub(crate) fn read_bytes(data: &[u8], ptr: i32, len: i32) -> Option<alloc::vec::Vec<u8>> {`

```
/// Bytes aus dem Gastspeicher, oder nichts, wenn der Bereich nicht passt.
```

## L46-47 · `pub(crate) fn write_bytes(data: &mut [u8], ptr: i32, bytes: &[u8]) -> i32 {`

```
/// `bytes` in den Gastspeicher an `ptr` schreiben; liefert die Laenge oder -1,
/// wenn der Bereich nicht passt.
```

## L60-61 · `fn wifi_poll_into(`

```
/// Gemeinsamer Rumpf der beiden poll-Funktionen: eine Nachricht aus `dequeue`
/// in den Gastpuffer holen (durch `max` begrenzt), Laenge oder -1.
```

## L144-156 · `pub(crate) fn npk_random_bytes(mem: &mut [u8], _ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {`

```
/// Zufall aus dem CSPRNG des Kernels in den Speicher des Moduls.
///
/// **Ohne Kapabilitaet, wie `npk_unix_time`.** Zufall ist keine Ressource des
/// Benutzers und verraet nichts ueber ihn: er gibt Bytes HERAUS und liest
/// nichts. Ihn zu gaten hiesse, jedem Modul eine Berechtigung zu geben, die
/// niemand je verweigern wuerde.
///
/// Die Quelle ist derselbe ChaCha20-Strom wie fuer Kapabilitaetsmarken —
/// aus RDRAND geseedet, alle 64 Bloecke neu verschluesselt. Eine zweite,
/// schwaechere Quelle fuer „bloss eine Seite" waere genau die Falle: aus
/// `crypto.getRandomValues` baut Seitencode Sitzungsmarken.
///
/// Liefert die Zahl der geschriebenen Bytes, oder -1.
```

## L159-161 · `if len > 65_536 { return -1; }`

```
// Derselbe Deckel, den die Webplattform kennt (WebCrypto 10.1.1): mehr
// als 64 KiB auf einmal verlangt niemand, und ohne Deckel haelt ein
// Modul den RNG-Mutex beliebig lange.
```

## L165-167 · `let Some(end) = start.checked_add(n) else { return -1 };`

```
// `checked_add`: ein umlaufendes `start + n` gaebe start > end und
// brauchte den Kernel beim Schneiden zum Absturz — ein Halt, den der
// Gast ausloest.
```

## L179-181 · `if token_id < 0 { return 0; }`

```
// One table, in palette.rs — the local copy here stopped at
// Danger and silently answered 0 for Page/AccentRing/… long
// after those tokens existed.
```

## L208-209 · `crate::shade::with_compositor(|comp| comp.start_flash());`

```
// Worker cores may only set the state; core 0 ticks and paints
// it from poll_render.
```

## L224-226 · `let terminal_idx = ctx.terminal_idx;`

```
// Prefer promoting the spawning terminal to a widget so
// the app owns a single window. Only create a fresh one
// if no terminal backed this worker (direct-launch path).
```

## L239-240 · `crate::shade::with_compositor(|comp| comp.focus_window(id));`

```
// Overlay path wants focus on the new widget (drun
// style); promotion does not focus, so fix up.
```

## L263-268 · `if ok {`

```
// The overlay path always wants this window focused — drun
// and any other launcher style app drives keyboard from
// here. The promote-or-create branch above already focuses,
// but if the app calls set_overlay a second time (or after
// some other host fn shifted focus elsewhere) we need to
// re-claim it so keys don't end up routed at a stale window.
```

## L379-382 · `let terminal_idx = ctx.terminal_idx;`

```
// Promote the spawning terminal to a widget window if there
// is one; otherwise create a fresh widget window. Unlike
// the overlay path we do NOT focus it — the dock is a
// background overlay that never owns keyboard focus.
```

## L481-500 · `pub(crate) fn npk_acpi_mem_read(ctx: &mut HostState, hi: i32, lo: i32) -> i32 {`

```
/// Ein Byte aus einer SystemMemory-Operationsregion lesen.
///
/// Manche Firmware spricht mit ihrem Embedded Controller nicht ueber die
/// ISA-Ports, sondern ueber ein speichergemapptes Fenster — auf einem
/// Lenovo IdeaPad lesen `_STA` und `_BST` des Akkus `0xFE800008`. Ohne
/// diesen Zugang erfindet der Interpreter dort eine 0, und die Firmware
/// schliesst daraus auf "kein Akku".
///
/// **Sicherheit.** Die Frage aus dem Checkpoint lautet: kann ein Modul
/// damit aus seinem Sandkasten? Drei Schranken sagen nein:
///
///  * `Rights::HARDWARE` — dasselbe Recht wie fuer die EC-Ports, und das
///    hat genau ein Modul.
///  * **Nur LESEN.** Ein Schreibzugriff auf beliebiges MMIO koennte
///    Geraete umprogrammieren; der bleibt auf dem Notizblock des
///    Interpreters und geht nirgendwohin.
///  * **Niemals Arbeitsspeicher.** Jede Adresse, die in einem als nutzbar
///    gemeldeten RAM-Bereich liegt, wird abgelehnt — und darin liegen
///    Kernel, Halde und die linearen Speicher aller Module. Kennt der
///    Kernel die Karte nicht, gilt alles als RAM, also alles als tabu.
```

## L508-510 · `use core::sync::atomic::{AtomicU32, Ordering};`

```
// Nur die ersten paar melden. Der Treiber misst fuer immer, und
// eine Absage je Runde ist nach einer Minute eine Flut — die
// AUSKUNFT ist einmal wertvoll, die Wiederholung nie.
```

## L527 · `pub(crate) fn npk_ec_query(ctx: &mut HostState) -> i32 {`

```
/// Eine anstehende EC-Abfrage abholen. -1 = nichts anliegend / kein Recht.
```

## L667 · `16 => { let (eax, _, _) = crate::interrupts::cpuid15(); eax as i64 },`

```
// CPUID 0x15 raw values for diagnostics
```

## L672-675 · `19 => unsafe { core::arch::x86_64::_rdtsc() as i64 },`

```
// 19 → raw TSC reading (monotonic, high-resolution).
// Combine with key=10 (tsc_mhz) to convert ticks → time.
// 64-bit TSC fits in i64 (sign bit unused for ~150 years
// at 2 GHz), so the cast is safe.
```

## L678-679 · `20..=29 => crate::process::sys_info(key),`

```
// ── Process tracking (keys 20-29) → process table ──
// 20: count, 21: pid_at_index, 22-29: query by PID
```

## L682-687 · `30..=34 => bench_sys_info(key),`

```
// ── Bench probes (keys 30-34) → cached on first call ──
// 30: BLAKE3 MB/s, 31: AES-GCM enc MB/s,
// 32: AES-GCM dec(in-place) MB/s,
// 33: raw blkdev write MB/s, 34: raw blkdev read MB/s.
// First call across any of these triggers ~100 ms of
// measurement; results live in BENCH_CACHE until reboot.
```

## L690-695 · `40 => fsck_sys_info(),`

```
// ── fsck self-check (key 40) → read-only integrity scan ──
// Runs on every call (NOT cached), logs a full report to
// serial, and returns the total problem count (0 = clean,
// -1 = scan error). testdisk calls this at the END of its run
// so corruption surfaces in-flight — a reboot would brick the
// mount before we could ever see it.
```

## L698-706 · `50 => if crate::config::get("log.drivers").as_deref() == Some("1") { 1 } else { 0 },`

```
// ── 50: sollen Treiber ihre Diagnosezeilen drucken? ──────────
//
// Jede Fundgeschichte in diesem Baum haengt an einer Logzeile, die
// jemand VORHER eingebaut hat — die Zeilen gehoeren also nicht
// geloescht. Sie gehoeren nur nicht in den Normalbetrieb: rohe
// Deskriptoren, Registerspuren, Sekundenzaehler. Ein Treiber fragt
// das EINMAL beim Start und schweigt danach.
//
// Vorgabe AUS. `set log.drivers 1` holt alles zurueck.
```

## L716 · `if crate::smp::fiber::yield_sleep(ms as u64) {`

```
// The normal path: we run inside a fiber → yield to the scheduler.
```

## L721-723 · `let freq = crate::interrupts::tsc_freq();`

```
// Fallback: not inside a fiber (degenerate no-worker host, or a
// one-shot wasm on Core 0) → HLT-idle until the deadline. NO
// core-stealing helper (that was the nesting hazard).
```

## L733-735 · `const WAIT_INPUT: i32 = 1;`

```
/// `npk_wait` mask bit: an input event is waiting — a widget event
/// (`npk_event_poll`), a key in the terminal buffer (`npk_input_poll`), or
/// the app's window is gone.
```

## L737-738 · `const WAIT_IRQ: i32 = 2;`

```
/// Driver: the device IRQ it registered (`npk_irq_register`) fired since
/// `npk_wait` last reported it.
```

## L740 · `const WAIT_NET_TX: i32 = 4;`

```
/// Driver registered as the WASM NIC: the IP stack queued a frame.
```

## L742 · `const WAIT_WIFI_CMD: i32 = 8;`

```
/// Driver: wifid queued a command (`npk_wifi_poll_cmd`).
```

## L744 · `const WAIT_WIFI_EVENT: i32 = 16;`

```
/// Manager (NETCTL): the driver queued an event (`npk_wifi_poll_event`).
```

## L746-748 · `const WAIT_STATE: i32 = 32;`

```
/// A watched topic changed — windows, battery, volume (`crate::notify`).
/// RENDER-gated like the calls that read them (`npk_bar_state`,
/// `npk_battery`).
```

## L751-753 · `fn wait_ready(ctx: &mut HostState, mask: i32) -> i32 {`

```
/// Which of `mask`'s conditions hold right now. Bits the caller may not
/// wait on (a driver bit without a driver, an event bit without NETCTL)
/// never fire.
```

## L792-793 · `fn input_ready(ctx: &HostState) -> bool {`

```
/// Is input waiting for this app? Also true once its widget window is
/// gone, so a parked app wakes and leaves its loop.
```

## L805-819 · `pub(crate) fn npk_wait(ctx: &mut HostState, mask: i32, timeout_ms: i32) -> i32 {`

```
/// `npk_wait(mask, timeout_ms)` — park until something in `mask` happens,
/// or `timeout_ms` passes (< 0: no timeout). Returns the bits that fired,
/// 0 on timeout. Bits: `WAIT_INPUT` 1, `WAIT_IRQ` 2, `WAIT_NET_TX` 4,
/// `WAIT_WIFI_CMD` 8, `WAIT_WIFI_EVENT` 16, `WAIT_STATE` 32.
///
/// The event-driven replacement for `loop { poll; npk_sleep(16) }`: the
/// app's fiber gives up its core and costs nothing until an event is pushed
/// for it (`widgets::push_event`, `wasm::push_app_key` signal it) or its
/// deadline comes. `docs/plan/CORES_AND_EVENTS.md` §3.3.
///
/// Every bit is gated by what the caller already owns: the IRQ bit by the
/// vector ITS driver registered, the NIC bit by being the registered WASM
/// NIC, the command bit by being a driver, the event bit by NETCTL — the
/// same rights the matching `npk_*_poll` calls check. It parks only the
/// caller's own fiber.
```

## L827-829 · `use crate::smp::fiber::{SIG_EVENT, SIG_IRQ, SIG_TX, SIG_WIFI};`

```
// Register where the app's input will be pushed. The widget window is
// created lazily by the app's first scene commit, so do it here, every
// time — a map insert and a store, at the rate the app waits.
```

## L842-844 · `let _ = crate::irq::arm(hw.irq_vector);`

```
// The interrupt must wake THIS core, and a level line the
// ISR masked is released: the driver waits again, so it has
// serviced the device (`irq::arm`).
```

## L882 · `let recheck = crate::interrupts::rdtsc() + freq / 100;`

```
// Not in a fiber: halt in place, looking again every 10 ms.
```

## L909 · `let flushed = crate::smp::per_core::flush_busy(core_id);`

```
// Flush work done since last checkpoint, update process table
```

## L930-932 · `if crate::smp::fiber::wait(crate::smp::fiber::SIG_EVENT, deadline).is_none() {`

```
// Park until a key is pushed (it signals this fiber) or the
// deadline. A halt here used to hold the whole core — `top` waits in
// this call, and a video in a fiber on the same core stood still.
```

## L934 · `let recheck = crate::interrupts::rdtsc() + freq / 100;`

```
// Not in a fiber (a one-shot on Core 0): halt in place.
```

## L941 · `crate::smp::per_core::set_active(core_id, true);`

```
// Resume work tracking
```

## L962-964 · `if idx < 0 {`

```
// -1 = the everything-sink: every write, whichever terminal it was
// routed to. A remote console bound to one index goes silent as soon
// as output is redirected elsewhere.
```

## L982-992 · `fn net_allowed(ctx: &mut HostState) -> bool {`

```
/// **Ein roher Socket ist mindestens so maechtig wie `npk_http_*`** und
/// gehoert an dasselbe Recht.
///
/// Bis Kernel 0.337.0 prueften die fuenf `npk_tcp_*` GAR NICHTS: die Tabelle
/// in `forge_glue::resolve` loest nach NAMEN auf, Importe werden beim Laden
/// nicht gegen die Kapabilitaet gehalten, und ein Modul ohne `.npk.caps`
/// bekommt `READ | EXECUTE | RENDER` — also kein `NET`. Damit konnte jedes
/// Modul, das den Namen importiert, eine Verbindung zu jeder Adresse und
/// jedem Port aufmachen und beliebige Bytes tauschen. Das ist genau die
/// Frage aus dem Sicherheits-Checkpoint von `CLAUDE.md`, und die Antwort war
/// nicht „nein".
```

## L1003-1014 · `const MAX_TLS: usize = 8;`

```
// ── TLS-Stromsocket ──────────────────────────────────────────────────────
//
// **Der Strom war schon da, nur nicht herausgefuehrt.** `crypto::tls` bietet
// `tls_connect`, `tls_send`, `tls_poll` und `tls_close` auf einem
// gewoehnlichen `tcp_handle` — das ist ein Byte-Socket, und was fehlte, war
// allein die Tuer fuer ein Modul. Gebraucht wird sie fuer `wss://`: ein
// WebSocket ist TLS plus ein Handschlag plus Rahmen, und die beiden letzten
// gehoeren in die Engine, nicht hierher.
//
// **Anders als das Jobsystem von `npk_http_*`:** dort ist eine Anfrage ein
// Auftrag mit Anfang und Ende, hier ist es eine LANGE Verbindung, die
// meistens still ist. Deshalb eine eigene Tabelle statt einer Warteschlange.
```

## L1019-1020 · `pid: u32,`

```
/// Wem er gehoert. Ein Griff wird NUR dem Prozess beantwortet, der ihn
/// geoeffnet hat — dieselbe Regel wie bei den HTTP-Griffen.
```

## L1028 · `fn tls_slot_ok(ctx: &mut HostState, handle: i32) -> Option<usize> {`

```
/// Gemeinsamer Riegel fuer jeden Zugriff auf einen bestehenden Griff.
```

## L1040-1047 · `pub(crate) fn npk_tls_connect(mem: &mut [u8], ctx: &mut HostState,`

```
/// `npk_tls_connect(ip, port, host_ptr, host_len) -> Griff | -1`
///
/// **Blockiert fuer den Handschlag** (gemessen 60 ms: 10 ms TCP + 50 ms TLS)
/// — dieselbe Groessenordnung, die `open_tls` im HTTP-Weg ohnehin kostet, und
/// er faellt einmal je Verbindung an. Was ein Modul NICHT darf, ist zehn
/// Sekunden blockieren: ein Modul ist eine Faser, und die haelt ihren
/// Arbeitskern an. Wird das je spuerbar, ist die Antwort dieselbe wie bei
/// `npk_tcp_connect` — in `start` und `status` teilen.
```

## L1053 · `let bare: String = String::from(host.split(':').next().unwrap_or(&host));`

```
// Der Name gehoert in SNI und in die Zertifikatspruefung, ohne Port.
```

## L1056-1061 · `let ip = match crate::intent::http::resolve_checked(&bare, Some(ctx.net_reach)) {`

```
// **Der NAME kommt herein, nicht die Adresse** — und damit macht
// `resolve_checked` beides in einem Zug: aufloesen UND die Reichweite
// pruefen. Ein Modul, das selbst aufloest, braeuchte dafuer einen eigenen
// DNS-Zugang, und dann liefe die Aufloesung an der Klasse vorbei, gegen
// die JEDE Anfrage der laufenden Seite geprueft wird (0.147.0). Genau das
// Loch nochmal, nur ueber einen anderen Socket.
```

## L1090 · `pub(crate) fn npk_tls_send(mem: &mut [u8], ctx: &mut HostState,`

```
/// `npk_tls_send(handle, ptr, len) -> 0 | -1`
```

## L1107-1111 · `pub(crate) fn npk_tls_recv(mem: &mut [u8], ctx: &mut HostState,`

```
/// `npk_tls_recv(handle, ptr, cap) -> n | 0 (noch nichts) | -1 (zu/Fehler)`
///
/// **Kommt SOFORT zurueck.** `tls_poll` sammelt nur, was der TCP-Stapel schon
/// hat; ein Browser fragt das in jedem Bild, und Warten waere der Preis fuer
/// nichts.
```

## L1127 · `pub(crate) fn npk_tls_close(ctx: &mut HostState, handle: i32) -> i32 {`

```
/// `npk_tls_close(handle) -> 0`
```

## L1145-1153 · `match crate::net::tcp::connect_start(ip, port as u16) {`

```
// **Die REICHWEITE gilt hier bewusst NICHT.** Sie ist die Regel einer
// SEITE: `ctx.net_reach` sagt, welche Klasse das gerade geladene Dokument
// erreichen darf, damit eine oeffentliche Seite nicht ins Heimnetz greift.
// Ein Modul ist keine Seite — es ist installierte Software mit einer
// erklaerten Kapabilitaet, und bei `debug` IST das Heimnetz der Zweck (es
// schreibt sein Protokoll an ein `nc -lk` auf dem Entwicklerrechner).
//
// Der TLS-Weg unten ist der andere Fall: den faehrt beak fuer eine Seite,
// und dort gilt sie.
```

## L1216-1221 · `pub(crate) fn npk_pci_bind_class_n(ctx: &mut HostState, class: i32, subclass: i32, index: i32) -> i32 {`

```
/// Bind the `index`-th PCI device of a class. Same rights check as
/// `npk_pci_bind_class`; `index` 0 is exactly the old behaviour.
///
/// Damit kann ein Treiber die Geraete seiner Klasse DURCHGEHEN, statt den
/// ersten nehmen zu muessen. Das Urteil, welches taugt, bleibt bei ihm —
/// der Kernel kennt keine Lautsprecher.
```

## L1258-1259 · `_ => return -1,`

```
// Ohne PCI-Geraet gibt es keine Konfigurationsadresse — hier zu
// antworten hiesse, auf 00:00.0 zu greifen.
```

## L1269-1270 · `_ => return -1,`

```
// Ohne PCI-Geraet gibt es keine Konfigurationsadresse — hier zu
// antworten hiesse, auf 00:00.0 zu greifen.
```

## L1281-1282 · `_ => return -1,`

```
// Ohne PCI-Geraet gibt es keine Konfigurationsadresse — hier zu
// antworten hiesse, auf 00:00.0 zu greifen.
```

## L1286 · `let cmd = pci::read32(hw.pci_addr, 0x04);`

```
// Also enable memory space
```

## L1289-1291 · `pci::enable_bus_master_path(hw.pci_addr);`

```
// Und auf jeder Bridge darueber: ohne Bus Master DORT leitet sie die
// Anfrage des Geraets nicht nach oben weiter, und das Geraet bekommt
// einen Master Abort auf voellig gueltiges RAM.
```

## L1298 · `let hw = match ctx.hw.as_mut() { Some(h) if h.is_pci => h, _ => return -1 };`

```
// Ohne PCI-Geraet gibt es keine Konfigurationsadresse.
```

## L1301-1303 · `if hw.irq_vector != 0 { return hw.irq_vector as i32; }`

```
// One vector per driver. Registering again returns the same one — the
// pool has 16 vectors and is never freed, so a loop around this call
// would otherwise take them all.
```

## L1311-1320 · `pub(crate) fn npk_irq_register_gsi(ctx: &mut HostState, gsi: i32, flags: i32) -> i32 {`

```
/// Register I/O APIC input `gsi` for this driver — the interrupt line of a
/// device that is not on PCI (the IdeaPad touchpads' GPIO controller, found
/// in its ACPI `_CRS`). `flags`: bit 0 level-triggered, bit 1 active-low.
///
/// Gated like `npk_mmio_map_phys`, whose register window such a driver
/// needs anyway: HARDWARE, and a bound or mapped device (`ctx.hw`). One
/// vector per driver, and a GSI that already has an owner (the keyboard,
/// another driver) is refused by `ioapic::route` — a module cannot take a
/// line away from its owner. Level lines are one-shot: the kernel ISR masks
/// them, `npk_wait` unmasks them when the driver waits again.
```

## L1334-1336 · `pub(crate) fn npk_sci_arm(ctx: &mut HostState, gpe: i32) -> i32 {`

```
/// `npk_sci_arm(gpe) -> vector | -1`: the AML driver takes the ACPI SCI for
/// its EC's GPE. Same ownership as `npk_irq_register_gsi` — the vector is
/// this module's, and `npk_wait(WAIT_IRQ)` arms it. Once per boot.
```

## L1370 · `pub(crate) fn npk_sci_service(ctx: &mut HostState) -> i32 {`

```
/// `npk_sci_service() -> mask`: ack the SCI sources (see `sci::service`).
```

## L1378 · `fn owns_vector(ctx: &HostState, vector: i32) -> bool {`

```
/// Is `vector` the one THIS module's driver registered?
```

## L1384-1385 · `if !owns_vector(ctx, vector) { return -1; }`

```
// Arming re-routes the device's MSI to the calling core — only for the
// module that owns it.
```

## L1399-1400 · `_ => return -1,`

```
// Ohne PCI-Geraet gibt es keine Konfigurationsadresse — hier zu
// antworten hiesse, auf 00:00.0 zu greifen.
```

## L1415-1416 · `if bar_base == 0 && bar_raw & 0x01 == 0 {`

```
// If BAR is unassigned (UEFI didn't configure it), assign it now.
// assign_bar_mmio sizes the BAR internally; we just need the base.
```

## L1423-1425 · `let cmd = pci::read32(hw.pci_addr, 0x04);`

```
// Size the BAR: disable memory, write 0xFFFFFFFF, read back, restore.
// Safe at this point because the driver hasn't started using the
// BAR yet (mmio_map_bar is the first access after pci_bind).
```

## L1441-1442 · `if let Err(e) = crate::paging::map_page(`

```
// SAFETY: identity-mapped MMIO region for bound PCI device BAR.
// map_page splits huge pages to set NO_CACHE for MMIO access.
```

## L1459-1486 · `pub(crate) fn npk_mmio_map_phys(ctx: &mut HostState, hi: i32, lo: i32, pages: i32) -> i32 {`

```
/// Einen PHYSISCHEN MMIO-Bereich abbilden, der nicht zu einem PCI-Geraet
/// gehoert.
///
/// Gebraucht fuer Hardware, die die Firmware nur ueber ACPI ansagt: auf
/// AMD-Renoir/Lucienne haengen die I2C-Controller (Touchpad!) an fester
/// MMIO im FCH und tauchen im PCI-Raum gar nicht auf. `npk_mmio_map_bar`
/// greift dort nicht.
///
/// Die Abbildung landet in derselben Handle-Tabelle wie ein BAR, also
/// lesen und schreiben die vorhandenen `npk_mmio_read*`/`write*` sie
/// unveraendert.
///
/// **Sicherheits-Checkpoint: kann ein Modul damit aus seinem Sandkasten?**
/// Ein Modul mit `Rights::HARDWARE` kann heute schon ein PCI-BAR abbilden
/// und DMA anfordern; der Zuwachs ist begrenzt, aber nicht null. Vier
/// Schranken:
///
///  * **`Rights::HARDWARE`** — dasselbe Recht wie EC-Ports und DSDT.
///  * **Niemals Arbeitsspeicher.** Geprueft wird JEDE Seite der Spanne,
///    nicht nur die erste: eine Spanne, die am Rand eines Lochs beginnt,
///    darf nicht in den RAM hineinreichen. Kennt der Kernel die Karte
///    nicht, gilt alles als RAM, also alles als tabu.
///  * **Niemals LAPIC oder IOAPIC.** Eine Schreibung nach
///    `0xFEE0_0000` ist ein Interrupt an einen beliebigen Vektor auf einem
///    beliebigen Kern — das ist Codeausfuehrung im Kernel, nicht
///    Geraetezugriff. Der IOAPIC daneben routet fremde Interrupts.
///  * **Deckel** auf die Spanne (16 Seiten = 64 KiB) und auf die Zahl der
///    Abbildungen (`MAX_MMIO_MAPS`), wie bei einem BAR.
```

## L1504 · `if (0xFEE0_0000..0xFEF0_0000).contains(&a) || (0xFEC0_0000..0xFED0_0000).contains(&a) {`

```
// LAPIC [0xFEE00000, 0xFEF00000) und IOAPIC [0xFEC00000, 0xFED00000).
```

## L1511 · `if ctx.hw.is_none() {`

```
// Ein Zustand ohne PCI-Geraet, falls das Modul nie gebunden hat.
```

## L1532-1533 · `if let Err(e) = crate::paging::map_page(`

```
// SAFETY: geprueft — kein RAM, kein Interruptcontroller. NO_CACHE,
// weil Geraeteregister nicht zwischengespeichert werden duerfen.
```

## L1551-1564 · `pub(crate) fn npk_pointer_inject(`

```
/// Eine Zeigerbewegung einspeisen.
///
/// Geht in dieselbe Schlange, aus der PS/2 und USB schon kommen
/// (`xhci::inject_mouse`) — kein zweiter Weg in den Compositor. Damit
/// stehen Touchpad, Maus und was noch kommt nebeneinander, statt sich zu
/// verdraengen.
///
/// `dx`/`dy` sind hier `i32`, im Ereignis `i8`: eine schnelle Bewegung wird
/// in Schritte ZERLEGT, statt geklemmt zu werden. Klemmen hiesse, dass der
/// Zeiger bei schnellen Strichen zurueckbleibt.
///
/// Rechte: `Rights::HARDWARE`. **`npk_key_inject` prueft daneben GAR
/// KEINS** — jedes Modul kann Tastendruecke in die Shell schreiben. Das ist
/// ein eigener Befund und ausdruecklich nicht die Vorlage hier.
```

## L1576-1577 · `for step in 0..16 {`

```
// Hoechstens ein paar Schritte — eine absurde Zahl darf keine Schleife
// aufhalten, und mehr als 16 x 127 Punkte ist keine Handbewegung.
```

## L1588 · `scroll: if step == 0 { s } else { 0 },`

```
// Der Rollwert gehoert EINMAL dazu, nicht an jeden Teilschritt.
```

## L1607 · `unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) as i32 }`

```
// SAFETY: validated MMIO region within mapped BAR
```

## L1621 · `unsafe { core::ptr::write_volatile((base + off as u64) as *mut u32, value as u32) }`

```
// SAFETY: validated MMIO region within mapped BAR
```

## L1636 · `unsafe { core::ptr::read_volatile((base + off as u64) as *const u16) as i32 }`

```
// SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
```

## L1650 · `unsafe { core::ptr::write_volatile((base + off as u64) as *mut u16, value as u16) }`

```
// SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
```

## L1655-1660 · `pub(crate) fn npk_mmio_read8(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {`

```
// 8-bit MMIO. Realtek rtw88 (RTL8822CE) rechnet seinen halben Registersatz
// in Bytes: die Power-Sequenz ist ein read8/write8-Interpreter, und Register
// wie REG_SYS_FUNC_EN+1 sind als EINZELNES Byte gemeint. Ein 32-Bit-RMW ist
// dafuer kein Ersatz — er liest und schreibt drei Nachbarbytes mit, und bei
// Registern mit Leseeffekt ist das ein anderer Vorgang. Linux waehlt die
// Breite absichtlich; wir uebernehmen sie.
```

## L1671 · `unsafe { core::ptr::read_volatile((base + off as u64) as *const u8) as i32 }`

```
// SAFETY: validated MMIO region within mapped BAR
```

## L1685 · `unsafe { core::ptr::write_volatile((base + off as u64) as *mut u8, value as u8) }`

```
// SAFETY: validated MMIO region within mapped BAR
```

## L1700 · `let lo = unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) } as u64;`

```
// SAFETY: validated MMIO region within mapped BAR
```

## L1717 · `unsafe {`

```
// SAFETY: validated MMIO region within mapped BAR
```

## L1736 · `let phys = match crate::memory::allocate_contiguous_below(page_count, 0x1_0000_0000) {`

```
// DMA buffers MUST be below 4GB — PCIe TX BD has 32-bit address field
```

## L1741 · `unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }`

```
// SAFETY: zeroing freshly allocated DMA memory
```

## L1748-1763 · `pub(crate) fn npk_dma_alloc_below(ctx: &mut HostState, pages: i32, limit_mb: i32) -> i32 {`

```
/// Wie `npk_dma_alloc`, aber der Treiber nennt die OBERGRENZE selbst.
///
/// `npk_dma_alloc` sucht unter 4 GB, und `allocate_contiguous_below` sucht
/// von oben nach unten — das Ergebnis liegt damit immer direkt unter dem
/// PCI-MMIO-Loch. Auf AMD-Blech ist genau dort TSEG/DPR: die CPU liest und
/// schreibt dort, ein GERAET wird abgewiesen, und der Chip bekommt einen
/// Master Abort auf eine Adresse, die aussieht wie gueltiges RAM.
///
/// Welche Adressen ein Geraet erreichen kann, ist Geraetewissen und gehoert
/// deshalb in den Treiber, nicht in eine Konstante hier. `limit_mb <= 0`
/// heisst 4 GB, also das alte Verhalten; mehr als 4 GB gibt es nicht, weil
/// jedes Geraet mit 32-Bit-Deskriptoren sonst stillschweigend falsch laege.
///
/// Sicherheit: derselbe Weg, dieselben Deckel, dieselbe Bitmap. Der Ruger
/// waehlt eine Obergrenze, keine Adresse — er kann sich damit keinen
/// fremden Speicher aussuchen.
```

## L1785 · `unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }`

```
// SAFETY: zeroing freshly allocated DMA memory
```

## L1812 · `unsafe { core::ptr::read_volatile((phys + off as u64) as *const u32) as i32 }`

```
// SAFETY: reading from validated DMA buffer
```

## L1826 · `unsafe { core::ptr::write_volatile((phys + off as u64) as *mut u32, value as u32) }`

```
// SAFETY: writing to validated DMA buffer
```

## L1854-1856 · `if !crate::wasm::private_area_allows(&name, &ctx.module_name) {`

```
// Der private Bereich eines anderen Moduls ist zu — auch mit READ auf
// alles. Der EIGENE ist dagegen offen, auch ohne READ: siehe
// `wasm::is_own_private`.
```

## L1900-1901 · `match start.checked_add(n) {`

```
// checked_add: a wrapping `start + n` would panic the KERNEL on
// the slice index — a guest-triggerable halt.
```

## L1924-1925 · `match start.checked_add(n) {`

```
// checked_add: a wrapping `start + n` would produce start > end and
// panic the KERNEL on the slice index — a guest-triggerable halt.
```

## L1948-1949 · `match start.checked_add(n) {`

```
// checked_add: a wrapping `start + n` would produce start > end and
// panic the KERNEL on the slice index — a guest-triggerable halt.
```

## L1972-1973 · `match start.checked_add(n) {`

```
// checked_add: a wrapping `start + n` would produce start > end and
// panic the KERNEL on the slice index — a guest-triggerable halt.
```

## L1990-1993 · `if is_trust_critical_path(&name) {`

```
// Apps may not write the module store or the trust store — see
// is_trust_critical_path.
// Checked BEFORE the grant path so a per-file grant can never
// become a way in there.
```

## L1998-1999 · `if !crate::wasm::private_area_allows(&name, &ctx.module_name) {`

```
// Der private Bereich eines anderen Moduls ist zu — auch mit READ und
// WRITE auf alles. Siehe `wasm::private_area_owner`.
```

## L2005-2015 · `let own_config = alloc::format!("sys/config/{}", ctx.module_name);`

```
// Three ways to be allowed to write, narrowest last:
//   1. blanket WRITE from `.npk.caps`
//   2. a grant for exactly this file — what the user handed over
//      by picking the path in a trusted dialog
//   3. the app's OWN settings file, `sys/config/<module>`. An
//      app that keeps preferences shouldn't need write access to
//      the whole store for it, and the name is the kernel's to
//      derive — a module can't claim someone else's.
//   4. der EIGENE private Bereich, `priv/<module>/…` — er braucht gar
//      keine Kapabilitaet, denn er ist keine Datei im Speicher der
//      Maschine, sondern der Zustand dieses Programms.
```

## L2025-2034 · `const PRIVATE_WRITE_MAX: i32 = 1024 * 1024;`

```
// **Was der private Bereich NICHT sein soll: ein Weg, ohne WRITE die
// Platte zu fuellen.** Ein Modul ohne Schreibrecht darf seinen eigenen
// Zustand behalten — das ist der Zweck —, aber „Zustand" hat eine
// Groessenordnung. Wer mehr braucht, braucht WRITE und damit die Frage
// an den Benutzer.
//
// Offen und benannt: das ist ein Deckel je SCHREIBVORGANG, kein
// Gesamtkontingent. Viele kleine Dateien laufen daran vorbei. Ein echtes
// Kontingent muesste den Bereich bei jedem Schreiben auszaehlen; das
// gehoert gemessen, bevor es gebaut wird.
```

## L2047-2050 · `match crate::npkfs::upsert(&name, &data[start..end], cap_id) {`

```
// Insert-or-replace: apps with state to persist (panel
// configs, etc.) re-write the same key on every change. The
// strict-create `store` would fail on the second write and
// leave the app's state diverging from disk.
```

## L2141-2142 · `let owner_wid = ctx.widget_window_id;`

```
// Remember which capability owns this window, so a later grant
// (loft opening a file in an already-running editor) can find it.
```

## L2154-2155 · `let payload: alloc::vec::Vec<u8> = mem[bytes_start..bytes_end].to_vec();`

```
// Heap copy, 200–600 bytes for a typical tree. The borrow checker
// used to force it; with `mem` a parameter it no longer does.
```

## L2160-2162 · `if prev_window == 0 {`

```
// First commit from a module that was spawned as a terminal:
// promote that terminal to a widget in place so the app only
// owns one window (not a terminal + a widget side-by-side).
```

## L2179-2180 · `if result > 0 && ctx.widget_window_id == 0 {`

```
// Positive return = newly allocated window id → store so
// subsequent commits from this app reuse the same slot.
```

## L2184-2186 · `if result < 0 { result } else { 0 }`

```
// Collapse "new-window id" into success for the callee —
// the WASM ABI contract is that any non-negative return
// means "commit accepted". Negatives still propagate.
```

## L2213-2224 · `pub(crate) fn npk_canvas_commit_yuv(`

```
/// `npk_canvas_commit_yuv(canvas_id, y_ptr, u_ptr, v_ptr, ys, cs, w, h, flags)`
///
/// Upload a planar 4:2:0 frame (I420) instead of BGRA. `ys`/`cs` are row
/// strides in bytes, so a decoder can hand over its padded planes without
/// repacking them. `flags`: bit 0 = Rec. 709 (else Rec. 601), bit 1 = full
/// range (else limited/studio).
///
/// **Why this exists next to `npk_canvas_commit`:** converting Y′CbCr to
/// BGRA inside a module costs 145 % of a core at 1080p30 — measured, and
/// more than decoding the frame. Here the conversion happens in the blit,
/// natively and only for the pixels that land in the canvas rect, and the
/// frame crosses the boundary at 1.5 bytes per pixel instead of 4.
```

## L2240-2242 · `let Some(need_y) = ys.checked_mul(h as usize) else { return -1 };`

```
// How much of each plane the blit may touch. `canvas::commit_i420`
// checks the same thing against the copied Vec; this check is about a
// different question — whether the module's own memory holds it.
```

## L2309-2315 · `let mut tmp = alloc::vec![0u8; need];`

```
// Read the actual displayed MMIO framebuffer (not a shadow
// buffer): it always holds the final composite (background +
// windows + cursor) that's physically on screen. The shadow
// double-buffer can be mid-swap when we (on a worker core)
// read it, yielding a stale background-only frame — the
// reason an earlier shadow capture missed all the windows.
// Row-by-row into a tight BGRA temp (pitch may exceed w*4).
```

## L2319-2320 · `unsafe {`

```
// SAFETY: the GOP framebuffer is identity-mapped and valid
// for pitch*height bytes; we read w*4 ≤ pitch per row.
```

## L2344-2345 · `if !crate::shade::widgets::widget_window_exists(window_id) { return -1; }`

```
// -1 also covers "window was closed by shade" (e.g. Mod+Shift+Q)
// so the app can fall out of its poll loop instead of spinning.
```

## L2372-2373 · `let entries = match crate::npkfs::fs::list("sys/wasm") {`

```
// v2: `sys/wasm` is a real directory. List immediate children
// directly instead of scanning + prefix-filtering the whole tree.
```

## L2409-2410 · `if name.contains('/') { return -1; }`

```
// Confine to sys/wasm/<bare-name>: reject any separator so a caller
// can't traverse out of the module directory.
```

## L2450 · `if name.contains('/') || name.contains("..") || name.is_empty() {`

```
// Path validation — refuse absolute paths, traversal, prefix reuse.
```

## L2461-2464 · `let rights = capability::widget_rights_from_wasm(&bytes);`

```
// Grant exactly the rights the app declares in its `.npk.caps`
// section (e.g. spell asks for WRITE to save files); apps with
// no declaration get the safe default (no WRITE). Per-app, not
// a blanket grant.
```

## L2471-2474 · `let spawned = crate::shade::with_compositor(|comp| {`

```
// Create a new terminal-kind window with its own terminal
// buffer and focus it. The widget-kind launcher that called
// us then closes itself (`npk_close_widget`), leaving the
// new loop + running app on screen.
```

## L2489-2490 · `crate::intent::reset_session_prompt(term_idx);`

```
// Fresh session prompt so the terminal isn't stuck on the
// caller's old prompt state when the app exits.
```

## L2517-2520 · `if !crate::microvm::cpu::vm_fiber_mode()`

```
// Reject only on the pure cooperative Core-0 path (BSP-only),
// so the caller falls back to typing the intent at the prompt.
// Fiber mode (vCPU as a pool fiber) AND the dedicated-core path
// both support launching from a worker, so allow those.
```

## L2582-2594 · `pub(crate) fn npk_acpi_table(`

```
/// Die `index`-te ACPI-Tabelle mit dieser Signatur in den Modulspeicher.
///
/// `sig` traegt die vier Zeichen little-endian in einem `i32` — so, wie
/// sie im Speicher stehen (`SSDT` = 0x54445353).
///
/// **Warum es das braucht:** eine Firmware verteilt ihren Namespace ueber
/// die DSDT UND beliebig viele SSDTs, und Linux laedt sie alle in
/// denselben (`acpi_tb_load_namespace`). Wer nur die DSDT liest, dem
/// fehlen Namen, die woanders deklariert sind — auf Florians IdeaPad die
/// Basis der Region, in der die Freigabebits der I2C-Controller stehen.
///
/// Rueckgabe: Laenge, oder die BENOETIGTE Laenge wenn der Puffer zu klein
/// ist, oder -1 (kein Recht / gibt es nicht).
```

## L2610 · `let src = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };`

```
// SAFETY: find_table_nth hat [addr, addr+len) abgebildet.
```

## L2627 · `return len as i32; // too small: tell the caller the needed size`

```
// too small: tell the caller the needed size
```

## L2629 · `let src = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };`

```
// SAFETY: acpi::dsdt() mapped [addr, addr+len) for us.
```

## L2647-2659 · `pub(crate) fn npk_audio_buffered(_ctx: &mut HostState, slot: i32) -> i32 {`

```
/// `npk_audio_buffered(slot)` — bytes still sitting in the slot's ring,
/// or -1 for a closed/invalid slot.
///
/// **This is the play clock.** Without it an app can only estimate what has
/// been heard from the wall clock (`submitted - elapsed * rate`), and the
/// wall clock and the audio crystal drift apart. For music nobody notices;
/// for lipsync over a film the error accumulates, which is why every player
/// that shows pictures makes the audio output its master clock.
///
/// Ungated and without an ownership check, exactly like `npk_audio_submit`
/// and `npk_audio_close` next to it — the audio slots have no owner today.
/// That is a gap worth its own decision, not one to half-close here: a read
/// that is stricter than the write beside it buys nothing.
```

## L2689-2690 · `let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();`

```
// v2: directories are real Tree objects, listings come straight
// from them — no scan + prefix-filter pass.
```

## L2694-2698 · `if !crate::wasm::private_area_allows(prefix_for_list, &ctx.module_name) {`

```
// **Auflisten ist auch Lesen.** Ein fremder privater Bereich darf nicht
// einmal als NAME erscheinen: dass `priv/tune` existiert, ist schon eine
// Auskunft. Deshalb zwei Schritte — der verlangte Pfad muss erlaubt
// sein, UND jeder Eintrag wird noch einmal an seinem vollen Pfad
// geprueft. Ohne den zweiten waere `list("")` das Loch neben der Tuer.
```

## L2710 · `let entries = match crate::npkfs::fs::list(prefix_for_list) {`

```
// Non-recursive: one directory's immediate children.
```

## L2722 · `fn dfs(`

```
// Recursive: DFS the subtree, emit relative paths.
```

## L2751 · `if !visible(&child_abs) { continue }`

```
// Nicht bloss ueberspringen: gar nicht erst hineinsteigen.
```

## L2791 · `return -1;   // still: ein fremder privater Bereich EXISTIERT nicht`

```
// still: ein fremder privater Bereich EXISTIERT nicht
```

## L2830 · `crate::shade::force_redraw();`

```
// Force compositor full redraw
```

## L2880-2881 · `Err(crate::net::tcp::TcpError::WouldBlock) => -2,`

```
// Backpressure, not a failure: too much is still unacked.
// A module that treats this as fatal drops a live connection.
```

## L2917 · `unsafe {`

```
// SAFETY: copying from validated DMA buffer to WASM linear memory
```

## L2945 · `unsafe {`

```
// SAFETY: copying from WASM linear memory to validated DMA buffer
```

## L2961 · `if hw.registered_as_netdev { return -1; } // already registered`

```
// already registered
```

## L2970 · `if let Some(h) = ctx.hw.as_mut() {`

```
// Re-borrow after register call
```

## L2984 · `crate::shade::terminal::write_idx(idx as usize, &s);`

```
// Write to specific terminal (worker-core safe)
```

## L2987 · `kprint!("{}", s);`

```
// Fallback: write to active terminal via kprint
```

## L3013-3015 · `crate::shade::terminal::stream_push_global(&s);`

```
// ...and to the remote mirror, which was blind to every app
// that logs this way. Outside the SERIAL lock: the sink takes
// its own, and holding two is how a deadlock is built.
```

## L3056-3057 · `accept_gzip: tls,`

```
// The document itself: 4,1x-9,9x fewer bytes on the wire,
// measured (docs/plan/JS_SCOPE_CONTENT_WEB.md §8).
```

## L3059-3060 · `try_h2: tls,`

```
// And over HTTP/2: this is the request Wikimedia
// throttles, four of them per page load (BROWSER.md §8.1).
```

## L3066-3067 · `let ok = res.is_ok();`

```
// A failed request must not leave the previous request's final
// URL readable as if it were this one's.
```

## L3071-3072 · `ctx.http_content_type =`

```
// Same rule: a stale Content-Type would make the next document
// decode against the last one's charset.
```

## L3075-3076 · `ctx.http_last_error = match &res {`

```
// Same rule for the reason: cleared on success, so a caller can
// never read a stale error and attribute it to this request.
```

## L3084-3085 · `write_bytes(mem, buf_ptr, &out[..write_len])`

```
// Bounds-checked write: buf_ptr is guest-controlled, and a
// wrapping `start + len` would panic the KERNEL on the slice index.
```

## L3103-3106 · `if !crate::intent::http::method_is_safe(&method) {`

```
// The method sits at the very front of the request line and the
// headers end it — a newline in either rewrites the request, and
// everything after it is read as a SECOND one. This is the check
// that stops a sandboxed app from smuggling requests through us.
```

## L3152-3153 · `accept_gzip: tls,`

```
// The browser asks for gzip and the kernel unpacks it; an app
// cannot set `Accept-Encoding` itself (RESERVED_HEADERS).
```

## L3157-3159 · `from_reach: Some(ctx.net_reach),`

```
// Alles, was ueber die WASM-Grenze kommt, ist Seitencode — auch
// wenn beak es weiterreicht. Die Reichweite steht am Kontext, nicht
// an der Anfrage, und der Kernel hat sie selbst ausgerechnet.
```

## L3174-3176 · `let ok = res.is_ok();`

```
// Same rule as npk_http_request throughout: everything is cleared
// on failure, so a caller can never read one request's answer and
// attribute it to the next.
```

## L3195-3204 · `pub(crate) fn npk_net_context(`

```
// ── Fetching without standing still ────────────────────────────────────────
//
// The same two requests as `npk_http_send` / `npk_http_request_many`, split
// into "start it" and "collect it". Between the two the module keeps running:
// it paints, it reads keys, its peer fibers get their turns. The wait happens
// on a worker fiber on another core (`intent::fetch`).
//
// A handle is answered only to the process that opened it — `ctx.pid`, not
// the caller's word — so guessing a small integer cannot read another app's
// document.
```

## L3206-3218 · `pub(crate) fn npk_net_context(`

```
/// Start one request. Returns a handle (>= 1), or -1 with the reason in
/// `npk_http_last_error`. Same validation as `npk_http_send`: it is the same
/// request, only nobody waits for it here.
/// Den Reichweiten-Kontext dieses Moduls setzen.
///
/// **Der Kernel glaubt dem Modul den Namen, aber nicht die Klasse.** beak
/// reicht die Adresse des Dokuments ein; welcher Netzbereich das ist,
/// rechnet diese Funktion selbst aus — sonst waere die Grenze eine, die das
/// Modul im Sandkasten selbst zieht, und das ist keine.
///
/// Eine Adresse ohne Herkunft (`beak:selftest`, `about:blank`) und alles,
/// was sich nicht aufloesen laesst, faellt auf `Public` zurueck: die
/// strengste Klasse, nicht die bequemste.
```

## L3292-3293 · `ctx.http_last_error = Some(alloc::format!("url\t{}", e));`

```
// A refusal at the door has to name itself the same way a failed
// exchange does, or the caller's error page says "unknown".
```

## L3311-3317 · `pub(crate) fn npk_http_begin_many(`

```
/// Start a batch. Returns a handle (>= 1) or -1; the answer comes back
/// through `npk_http_take_many`.
///
/// Ohne Kopfzeilen — die Fassung, die vor 0.333.0 die einzige war. Sie
/// bleibt, damit ein Modul, das gegen sie gebaut wurde, weiter LAEUFT: eine
/// geaenderte Signatur waere ein Bindefehler, und ein Bindefehler heisst
/// „die App startet gar nicht", nicht „ein Bild fehlt".
```

## L3325-3339 · `pub(crate) fn npk_http_begin_many_hdr(`

```
/// Wie [`npk_http_begin_many`], aber mit einer KEKSZEILE JE ADRESSE.
///
/// **Warum es das geben muss.** Bis hierher trug nur der Weg fuer eine
/// einzelne Anfrage (`Work::One`) Kopfzeilen; der Stapelweg trug keine. Das
/// Dokument kam also angemeldet zurueck und JEDE Unterressource darin —
/// Bilder, Blaetter, Skripte — anonym. Auf einer Seite hinter einer
/// Anmeldung sieht das aus wie ein Bildfehler und ist keiner.
///
/// `hdrs` ist ein Block aus Zeilen, POSITIONELL zu `urls`: Zeile `i` ist der
/// Wert der `Cookie`-Kopfzeile fuer `urls[i]`, oder leer. Leere Zeilen
/// werden NICHT weggefiltert — sonst verrutschen die Positionen, und ein
/// Keks landete an der falschen Adresse.
///
/// Das Keksglas gehoert dem Browser, nicht dem Kernel: hier wird nur
/// weitergereicht und geprueft.
```

## L3361-3365 · `let cookies: alloc::vec::Vec<String> = if hdrs_len > 0 {`

```
// Die Kekse: eine Zeile je Adresse, in derselben Reihenfolge. Geprueft
// wird jede mit demselben Massstab wie eine Kopfzeile am
// Einzelanfrage-Weg — eine, die nicht besteht, wird zu KEINEM Keks statt
// zu einer abgelehnten Anfrage: ein fehlender Keks ist ein Bild, das
// anonym kommt, eine Absage waere gar kein Bild.
```

## L3396-3397 · `pub(crate) fn npk_http_poll(ctx: &mut HostState, handle: i32) -> i32 {`

```
/// 1 = an answer is waiting, 0 = still running, -1 = it failed (call
/// `npk_http_take` for the reason), -2 = no such handle.
```

## L3402-3408 · `pub(crate) fn npk_http_take(mem: &mut [u8], ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {`

```
/// Collect a finished single request. Bytes written on success; -1 if the
/// request failed; -2 if the handle is unknown; -3 while it is still running
/// (and only then does the job survive the call).
///
/// Fills exactly the five getters `npk_http_send` fills, and clears them the
/// same way — a caller must never read one request's answer and attribute it
/// to the next.
```

## L3430-3437 · `pub(crate) fn npk_http_take_many(`

```
/// Collect a finished batch: the bodies back to back in `out`, one
/// little-endian i32 length per URL in `lens` (-1 for one that failed).
/// Returns how many URLs the batch had, or -1 / -2 / -3 as above.
///
/// Touches none of the response getters — a batch has one status per URL and
/// no headers, exactly as `npk_http_request_many` has always had it, and
/// clobbering the document's headers with a picture's would be worse than
/// silence.
```

## L3443-3445 · `match crate::intent::fetch::result_count(ctx.pid, handle) {`

```
// Asked BEFORE taking: `take` destroys the job, so a length table too
// small to hold the answer has to be refused while the answer still
// exists — otherwise a caller that sized it wrong loses the batch.
```

## L3463-3465 · `if reply.body.len() > out_max as usize { return -1; }`

```
// Refused, not truncated. The length table describes the WHOLE blob, so a
// short write would leave the caller slicing bodies out of bytes that were
// never written. (`npk_http_take` may truncate — there is no table there.)
```

## L3472-3473 · `pub(crate) fn npk_http_cancel(ctx: &mut HostState, handle: i32) -> i32 {`

```
/// Stop caring about a handle. Always 0 — a browser cancels on every
/// navigation and must not have to know which state it caught.
```

## L3498-3499 · `const MAX_URLS: usize = 64;`

```
// Bound the work a single call can ask for, and make sure the
// guest actually gave us room for one length per URL.
```

## L3511-3513 · `Some(b) if blobs.len() + b.len() <= total_cap => {`

```
// Drop a body that would overrun the caller's buffer
// rather than truncating it — half an image decodes to
// garbage, whereas a missing one draws a placeholder.
```

## L3537 · `if app.is_empty() || app.contains('/') || app.contains("..") { return -1; }`

```
// Module name only — no path traversal into the store.
```

## L3541-3544 · `if let Some(arg_str) = arg.clone() {`

```
// Singleton + tabs: if the target app already has a widget
// window (titled with its module name), deliver the open as an
// Event::Open to that instance and focus it instead of
// spawning a duplicate. Only when there's something to open.
```

## L3553-3555 · `if let Some(cap) = crate::shade::widgets::window_cap(id.0) {`

```
// Same deal as a pick: the user pointed at this file
// (a double-click in the file manager), so the app may
// read and save it — and nothing else.
```

## L3578-3580 · `if let Some(a) = arg.as_deref() {`

```
// Launching an app ON a file is the user pointing at it — grant
// that one path so the app can save it back without holding
// WRITE over the whole store.
```

## L3585-3590 · `let win = match crate::shade::with_compositor(|c| c.create_widget_window(&app)) {`

```
// Create the widget window NOW (synchronously, titled with the
// module name) instead of lazily on first scene_commit. The
// app spawns asynchronously, so without this a rapid second
// open (e.g. a double-click = two opens) would see no window
// yet and spawn a DUPLICATE instance. Pre-creating lets the
// next open find it and route an Event::Open tab instead.
```

## L3595 · `crate::shade::focus_window(win); // bring the editor to the front`

```
// bring the editor to the front
```

## L3631 · `let requester = ctx.widget_window_id;`

```
// Only a windowed app can receive the reply event.
```

## L3646-3647 · `let start = if start.trim().is_empty() || start.contains("..") {`

```
// A start dir is a hint, not authority — the picker re-resolves
// it and the user can navigate anywhere regardless.
```

## L3653-3654 · `let suggest = if suggest.contains('/') || suggest.contains("..") {`

```
// The suggestion is a bare filename; a path here would let a
// caller pre-aim the save at a directory the user never saw.
```

## L3676-3677 · `let arg = alloc::format!("{}\0{}\0{}",`

```
// Wire the request as the launch argument:
//   "<open|save>\0<start-dir>\0<suggested-name>"
```

## L3681-3682 · `let win = match crate::shade::with_compositor(|c| {`

```
// Floating + centred, like the launcher — a dialog must not
// re-tile the workspace behind it.
```

## L3698-3699 · `if let Some(s) = crate::shade::widgets::take_pick(win.0) {`

```
// Undo the half-open session, else the requester can never
// ask again (has_open_pick would keep saying "one is up").
```

## L3720-3721 · `crate::shade::focus_window(crate::shade::window::WindowId(session.requester));`

```
// Hand focus back to the app that asked, so the user carries on
// where they left off instead of on a closing dialog.
```

## L3755-3756 · `if is_trust_critical_path(&name) {`

```
// Apps may not delete modules or trust anchors — see
// is_trust_critical_path.
```

## L3784-3785 · `if is_trust_critical_path(&old) || is_trust_critical_path(&new) {`

```
// Neither source nor destination may be module or trust store —
// renaming in would plant an unverified module or anchor.
```

## L3790-3792 · `if !crate::wasm::private_area_allows(&old, &ctx.module_name)`

```
// BEIDE Seiten. Nur die Quelle zu pruefen liesse eine Datei in einen
// fremden privaten Bereich schieben, nur das Ziel liesse eine aus einem
// herausholen.
```

## L3821-3822 · `if !crate::wasm::private_area_allows(&old, &ctx.module_name)`

```
// Wie beim Umbenennen: beide Seiten. Eine Kopie AUS einem fremden
// privaten Bereich heraus waere derselbe Diebstahl wie ein Lesen.
```

## L3850-3851 · `crate::wifi::note_manager_core();`

```
// The manager's own fiber is calling: remember its core so the
// microvm keeps vCPUs off it (see wifi::note_manager_core).
```

