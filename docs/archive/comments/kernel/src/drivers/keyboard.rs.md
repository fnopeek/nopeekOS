# `kernel/src/drivers/keyboard.rs` @ 5e0102684

## L1-5 · `use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering, AtomicU32};`

```
//! PS/2 Keyboard Driver
//!
//! IRQ1 handler reads scancodes from port 0x60.
//! Scancode Set 1 with US and DE_CH layouts.
//! USB keyboards work via BIOS legacy PS/2 emulation.
```

## L14-18 · `const BUF_SIZE: usize = 512;`

```
// Lock-free ring buffer for decoded key events (interrupt-safe).
// 512 (was 64): a pasted line (e.g. a long URL) arrives as a fast key burst;
// the shell drains one key per loop iteration, so a small ring overflowed mid-
// paste and silently dropped the tail of the line. 512 absorbs any pasteable
// line (matches INPUT_BUF_SIZE).
```

## L24 · `static SHIFT: AtomicBool = AtomicBool::new(false);`

```
// Modifier state (shared across all keyboard drivers)
```

## L32 · `pub fn set_super(held: bool) { SUPER.store(held, Ordering::Release); }`

```
// --- Public modifier API (used by shade, any driver can write) ---
```

## L34 · `pub fn set_super(held: bool) { SUPER.store(held, Ordering::Release); }`

```
/// Set Super/GUI/Meta key state. Called by any keyboard driver.
```

## L37 · `pub fn set_shift(held: bool) { SHIFT.store(held, Ordering::Release); }`

```
/// Set Shift key state. Called by any keyboard driver.
```

## L40 · `pub fn set_ctrl(held: bool) { CTRL.store(held, Ordering::Release); }`

```
/// Set Ctrl key state. Called by any keyboard driver.
```

## L43 · `pub fn is_super_held() -> bool { SUPER.load(Ordering::Acquire) }`

```
/// Query modifier state (used by shade::input).
```

## L49 · `const KEY_UP: u8 = 0x80;`

```
// Special key codes (escape sequences sent as ESC [ X)
```

## L61-62 · `pub fn init() {`

```
/// Initialize PS/2 keyboard controller.
/// Safe on systems without PS/2 (returns silently).
```

## L65 · `let status = inb(STATUS_PORT);`

```
// Check if PS/2 controller exists (0xFF = no controller)
```

## L68-71 · `kprintln!("[npk] ps2: no i8042 — a built-in keyboard must come from USB or I2C-HID");`

```
// Sagen, dass es ihn nicht gibt. Vorher war das stumm, und damit
// war die Frage "haengt die eingebaute Tastatur ueberhaupt am
// i8042?" am Geraet nicht zu beantworten — ohne Tastatur laesst
// sich auch kein Diagnosebefehl tippen.
```

## L73 · `return; // No PS/2 controller (USB-only system)`

```
// No PS/2 controller (USB-only system)
```

## L76 · `for _ in 0..1000 {`

```
// Flush any pending data from controller buffer (with timeout)
```

## L82 · `outb(STATUS_PORT, 0xAE);`

```
// Enable keyboard (send 0xAE to command port)
```

## L85-91 · `wait_write();`

```
// Scancode-UEBERSETZUNG (Konfigbit 6). `decode_scancode` liest Satz 1,
// und den liefert der Controller NUR mit eingeschalteter Uebersetzung.
// Eine Maschine, die rein ueber UEFI startet, kann ihn mit geloeschtem
// Bit uebergeben; die Tastatur sendet dann Satz 2, und jeder Scancode
// decodiert zu Unsinn oder zu gar nichts — genau das Bild einer
// "toten" Tastatur. Lesen, aendern, schreiben: wo das Bit schon steht,
// passiert nichts.
```

## L109 · `wait_write();`

```
// Enable scanning (send 0xF4 to data port)
```

## L120-128 · `static PS2_MOUSE_ENABLED: AtomicBool = AtomicBool::new(false);`

```
// ── PS/2 mouse / touchpad (i8042 aux port) ──
//
// On UEFI machines the legacy PIC is masked, so IRQ12 (aux) never fires — same
// situation as the keyboard. We enable the aux device and read its 3-byte
// packets in the shared i8042 drain (`poll_ps2`), feeding them into the same
// pointer ring the USB mouse uses (`xhci::inject_mouse`). Only enabled when no
// USB mouse is present (so QEMU/NUC USB-mouse setups are untouched). A Synaptics
// touchpad in PS/2-compatibility mode shows up here as a standard relative
// mouse — basic cursor + click (gestures/precision need the I2C-HID path later).
```

## L131-139 · `static PS2_IRQ_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// True once a PS/2 mouse is up and the i8042 is drained from the Core-0
/// TIMER IRQ (`poll_ps2_irq`) instead of the run loop. Mirrors how the USB
/// mouse is serviced (`xhci::poll_events_irq` in the same handlers) → the
/// polled PS/2 mouse samples at the timer rate independent of the loop's
/// HLT/spin (fixes laggy-mouse-when-a-window-is-open). When set, the IRQ is
/// the SOLE i8042 drainer and `read_key` must NOT poll (the IRQ would
/// preempt it between the STATUS and DATA reads → wrong byte). Stays false
/// on USB-mouse hosts (QEMU/NUC) so their input path is byte-for-byte
/// unchanged.
```

## L145 · `fn ps2_read() -> Option<u8> {`

```
/// Read one byte from the i8042 output buffer with a bounded spin (None on timeout).
```

## L155 · `fn mouse_write(b: u8) -> Option<u8> {`

```
/// Send a command byte to the aux device (0xD4 prefix) and read its reply.
```

## L158 · `unsafe { outb(STATUS_PORT, 0xD4); }   // next data-port byte goes to aux`

```
// next data-port byte goes to aux
```

## L164-165 · `pub fn init_mouse() -> bool {`

```
/// Initialize the PS/2 touchpad/mouse on the i8042 aux port. No-op (returns
/// false) if a USB mouse is already up or no aux device responds.
```

## L169 · `if inb(STATUS_PORT) == 0xFF { return false; }   // no i8042 controller`

```
// no i8042 controller
```

## L170 · `wait_write();`

```
// Enable the aux device.
```

## L173-174 · `wait_write();`

```
// Read controller config, enable the aux clock (clear bit5), keep the
// keyboard bits. We poll, so leave the aux-IRQ bit alone.
```

## L183-197 · `unsafe {`

```
// Reset (0xFF) antwortet ACK 0xFA, dann Selbsttest 0xAA, dann die
// Geraete-ID: 0x00 Standard-PS/2 (3 Byte), 0x03 mit Rad (4 Byte),
// 0x04 fuenf Tasten (4 Byte).
//
// Die drei Bytes wurden verworfen, und `mouse_write` prueft das ACK
// nicht — die Zeile darunter meldete also "touchpad/mouse on i8042
// aux", sobald IRGENDEIN Byte zurueckkam. Auf einem Geraet, dessen
// Touchpad in Wahrheit an I2C-HID haengt, ist das eine falsche
// Auskunft, und eine falsche ist schlimmer als keine: sie laesst
// niemanden weitersuchen.
// Erst den Aux-Port SELBST pruefen: 0xA9 "test auxiliary interface"
// antwortet 0x00 wenn er in Ordnung ist, 0x01/0x02 Taktleitung haengt,
// 0x03 Datenleitung haengt, 0x04 kein Aux-Port. Das ist billiger und
// eindeutiger, als aus der Antwort auf ein Reset zu raten — und wir
// haben den Befehl nie benutzt.
```

## L204-206 · `let mut ack = None;`

```
// Reset (0xFF), MIT Wiederholung auf 0xFE ("Resend"). Das schreibt das
// PS/2-Protokoll so vor, und wir haben es nie getan — ein einzelnes
// 0xFE hat der alte Code sogar als "Geraet vorhanden" gewertet.
```

## L212 · `let bat = ps2_read();   // 0xAA self-test`

```
// 0xAA self-test
```

## L213 · `let id  = ps2_read();   // device id`

```
// device id
```

## L215-218 · `let why = match aux_test {`

```
// Die Zahl allein kostet den naechsten Leser eine Suche, und sie sagt
// sehr Verschiedenes: 0x00 heisst "Port in Ordnung, nur nichts dran",
// alles andere heisst "dieser Kanal funktioniert nicht" — und das
// trennt "Touchpad haengt woanders" von "wir machen etwas falsch".
```

## L230-232 · `if ack != Some(0xAA) && bat != Some(0xAA) && id != Some(0xAA) {`

```
// Lenient: der Selbsttest ist das eigentliche Lebenszeichen, und
// mancher Controller verschluckt das ACK — also reicht 0xAA an
// irgendeiner der drei Stellen. Fehlt es ganz, ist der Aux-Port leer.
```

## L240 · `let _ = mouse_write(0xF6);   // set defaults`

```
// set defaults
```

## L241 · `let _ = mouse_write(0xF4);   // enable data reporting (→ 0xFA)`

```
// enable data reporting (→ 0xFA)
```

## L243-246 · `PS2_IRQ_ACTIVE.store(true, Ordering::Release);`

```
// Init done → hand i8042 draining to the Core-0 timer IRQ (poll_ps2_irq),
// so the polled mouse samples at the timer rate regardless of the run
// loop. Set LAST so the IRQ can't race the init reads/acks above. From
// here read_key reads BUF only (the IRQ is the sole drainer).
```

## L252-257 · `static LAST_MOUSE_TSC: AtomicU64 = AtomicU64::new(0);`

```
/// Host TSC of the last PS/2 mouse byte. On UEFI machines IRQ12 is masked,
/// so the mouse is *polled* in the Core-0 run loop. When a window is focused
/// the loop HLTs between 100 Hz timer ticks → the mouse samples + cursor
/// redraw at only ~100 Hz with pipeline latency → laggy. `core0_idle_tick`
/// reads this so it can keep the loop SPINNING (full sample rate, smooth
/// cursor) while the mouse is moving, and HLT (low power) once it stops.
```

## L260-263 · `pub fn mouse_active_within(ms: u64) -> bool {`

```
/// True if a PS/2 mouse byte arrived within ~`ms` of now — i.e. the user is
/// actively moving the pointer. Used by the Core-0 idle path to spin instead
/// of HLT during interaction. False on hosts with no PS/2 mouse (TSC stays 0,
/// `saturating_sub` keeps it false) so USB/IRQ setups are unaffected.
```

## L276 · `fn feed_mouse(byte: u8) {`

```
/// Assemble a 3-byte PS/2 mouse packet and inject it as a pointer event.
```

## L281 · `if byte & 0x08 == 0 { return; }   // bit3 sync must be set, else resync`

```
// bit3 sync must be set, else resync
```

## L294 · `if b0 & 0xC0 != 0 { return; }      // X/Y overflow → bogus packet, drop`

```
// X/Y overflow → bogus packet, drop
```

## L295 · `let buttons = b0 & 0x07;           // bit0=left,1=right,2=middle (== MouseEvent)`

```
// bit0=left,1=right,2=middle (== MouseEvent)
```

## L302 · `dy: clamp(-dy),   // PS/2 +Y is up; screen +Y is down`

```
// PS/2 +Y is up; screen +Y is down
```

## L304-305 · `hscroll: 0,`

```
// Das PS/2-Basispaket hat drei Bytes und keine Rolldaten;
// erst IntelliMouse (4 Bytes) traegt sie.
```

## L313 · `pub fn has_key() -> bool {`

```
/// Check if a key is available (IRQ buffer or polled port 0x60).
```

## L318 · `if unsafe { inb(STATUS_PORT) } & 0x21 == 0x01 {`

```
// PS/2 byte waiting in the i8042 output buffer (keyboard, not aux)?
```

## L325-326 · `pub fn read_key() -> Option<u8> {`

```
/// Read next raw key byte from buffer. Falls back to PS/2 polling, then
/// xHCI USB keyboard. Prefer `read_event()` for typed KeyEvent w/ modifiers.
```

## L331 · `let key = unsafe { KEY_BUF[tail] };`

```
// SAFETY: single consumer (main loop), IRQ only writes via push_key
```

## L336-342 · `if !PS2_IRQ_ACTIVE.load(Ordering::Relaxed) {`

```
// PS/2 polling: with the legacy PIC masked (UEFI/APIC machines) IRQ1
// never fires, so the buffer above stays empty — but the i8042 still
// latches scancodes. Read them on demand. (No-op if no PS/2 keyboard.)
// SKIP when the Core-0 timer IRQ owns the i8042 (PS2_IRQ_ACTIVE): polling
// here would race the IRQ on the single output buffer (it can preempt us
// between the STATUS and DATA reads → misread byte). Keyboard bytes then
// arrive via the IRQ → push_key → the BUF read above.
```

## L348 · `crate::xhci::poll_keyboard()`

```
// xHCI USB keyboard (real driver, no legacy emulation needed)
```

## L352-353 · `fn poll_ps2() -> Option<u8> {`

```
/// Poll the i8042 PS/2 controller for one keyboard scancode. Skips aux
/// (PS/2 mouse/touchpad) bytes so they don't get misread as keystrokes.
```

## L356-367 · `let tsc_2ms = crate::interrupts::tsc_freq() / 500;`

```
// Drain the shared i8042 output buffer: aux (touchpad) bytes feed the
// mouse assembler, keyboard bytes decode to a char. Draining aux is
// mandatory — an unread aux byte sits in the single output buffer and
// would block keyboard reads.
//
// The PS/2 device delivers one byte per ~1 ms, paced by our reads. At a
// 100 Hz poll that splits each 3-byte mouse packet across ~30 ms → the
// cursor lags and overshoots ("rubber-band"). So while a packet is
// mid-assembly we briefly (TSC-bounded ~2 ms) wait for the next byte to
// finish the packet in one poll — full sample rate, ≤10 ms latency. The
// wait only happens during active movement (PM_IDX != 0) and a byte
// budget caps total work per call.
```

## L372 · `if status == 0xFF { return None; }       // no controller`

```
// no controller
```

## L374-375 · `if PS2_MOUSE_ENABLED.load(Ordering::Relaxed)`

```
// Buffer empty. Finish an in-flight mouse packet rather than
// splitting it across polls.
```

## L393 · `let b = inb(DATA_PORT);`

```
// bit5 set → byte is from the aux device (touchpad/mouse).
```

## L400-401 · `if let Some(b) = take_tail() { return Some(b) }`

```
// Wartet noch die zweite Haelfte eines Zeichens? Die zuerst —
// vor dem naechsten Scancode, sonst geht sie verloren.
```

## L407 · `}`

```
// Modifier / release / 0xE0 prefix produced no char — keep draining.
```

## L412-423 · `static PS2_ROUTED: AtomicBool = AtomicBool::new(false);`

```
/// Drain the i8042 from the Core-0 TIMER IRQ (called next to
/// `xhci::poll_events_irq`). Non-blocking — NO busy-wait (IRQ context); a
/// 3-byte mouse packet split across ticks completes on the next tick
/// (≤ one timer period). Keyboard scancodes → `push_key` (the BUF
/// `read_key` reads); aux bytes → `feed_mouse` (→ `update_atomic` +
/// `request_render`, exactly like the USB mouse). This is the SOLE i8042
/// drainer once active. No-op until `PS2_IRQ_ACTIVE` (set after init, only
/// when a PS/2 mouse exists) so init can't race the IRQ and USB-mouse hosts
/// are untouched. Core-0 only (the timer handlers run on the BSP).
/// The i8042's keyboard IRQ is routed (`enable_irq` succeeded). Without it
/// `read_key` polls the port — and with an aux device the tick-free Core 0
/// must drain it itself (`needs_poll`).
```

## L425 · `static PS2_PRESENT: AtomicBool = AtomicBool::new(false);`

```
/// An i8042 answered (status register not 0xFF).
```

## L428 · `pub fn needs_poll() -> bool {`

```
/// Must Core 0 poll the i8042 because no interrupt tells it of a byte?
```

## L433-441 · `pub fn enable_irq() {`

```
/// Route the i8042 through the I/O APIC to Core 0 (stage 3b). Call on
/// Core 0 after `ioapic::init`.
///
/// Linux sets the controller's interrupt-enable bits when it registers the
/// IRQ (`I8042_CTR_KBDINT`, `I8042_CTR_AUXINT`); firmware usually leaves
/// KBDINT on, but nothing promises it. The output buffer is flushed first
/// (`i8042_flush`), or a pending scancode would be read as the config byte.
/// From here the interrupt owns the i8042 (`PS2_IRQ_ACTIVE`): `read_key`
/// stops polling the port and takes keys from the buffer.
```

## L443 · `if unsafe { inb(STATUS_PORT) } == 0xFF {`

```
// SAFETY: i8042 port I/O at boot, on Core 0, interrupts off below.
```

## L445 · `return; // no controller`

```
// no controller
```

## L462 · `unsafe {`

```
// SAFETY: i8042 command/data ports; nothing else touches them yet.
```

## L497-500 · `unsafe {`

```
// SAFETY: ring-0 i8042 port access; only Core-0 interrupt handlers call
// this (the i8042 IRQ, and the timer tick as fallback) — they do not
// nest — and once active nothing else touches the i8042 (read_key reads
// BUF).
```

## L505 · `return; // no controller / output buffer empty`

```
// no controller / output buffer empty
```

## L509 · `let b = inb(DATA_PORT);`

```
// Aux (touchpad/mouse) byte.
```

## L525-527 · `pub fn read_event() -> Option<crate::input::KeyEvent> {`

```
/// Read next key as a typed KeyEvent. Converts ANSI escape sequences
/// (ESC [ A/B/C/D/H/F/2/3/5/6) to KeyCode variants. Captures current
/// modifier state (Shift, Ctrl, Alt, AltGr, Super) in the event.
```

## L535 · `if let Some(bracket) = read_key() {`

```
// ESC — might be start of ANSI escape sequence (pushed as 3 bytes by push_arrow)
```

## L555 · `}`

```
// ESC + non-bracket: return ESC (bracket byte is lost — rare edge case)
```

## L564 · `c => Some(KeyEvent::char(c, mods)), // control chars etc.`

```
// control chars etc.
```

## L572 · `unsafe { KEY_BUF[head] = key; }`

```
// SAFETY: single producer (IRQ handler), consumer only reads via read_key
```

## L576 · `}`

```
// Drop key if buffer full
```

## L579-581 · `pub fn inject_byte(byte: u8) {`

```
/// Inject a raw key byte into the keyboard buffer as if it came from hardware.
/// Used by the remote debug shell to drive the focused window from outside.
/// Clients should send ANSI escape sequences for arrow/nav keys (ESC [ A etc).
```

## L586 · `fn push_arrow(code: u8) {`

```
/// Push an arrow key as ANSI escape sequence: ESC [ A/B/C/D
```

## L595 · `KEY_DEL => b'3', // ESC [ 3 ~`

```
// ESC [ 3 ~
```

## L601 · `push_key(0x1B); // ESC`

```
// ESC
```

## L614-617 · `static PENDING_VOL: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);`

```
// Media keys and unmapped codes arrive in the IRQ handler, where neither
// the console nor the audio state (it notifies subscribers, under a lock)
// may be touched. The ISR only records; `apply_deferred` acts on the shell
// fiber, which the ISR wakes anyway.
```

## L620 · `static PENDING_UNKNOWN: [core::sync::atomic::AtomicU64; 2] =`

```
/// Unmapped extended codes seen but not yet reported (bit per code).
```

## L623 · `static SEEN_UNKNOWN: [core::sync::atomic::AtomicU64; 2] =`

```
/// Already reported once (bit per code).
```

## L626 · `static MUTED_FROM: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0xFF);`

```
/// Volume before mute; 0xFF = not muted.
```

## L637-638 · `fn note_unknown_extended(code: u8) {`

```
/// Every extended key we do not map, reported once per code — the Fn row
/// of a new notebook is found this way instead of guessed.
```

## L647 · `pub fn apply_deferred() {`

```
/// Act on what the IRQ handler recorded. Called from the shell's idle step.
```

## L676-679 · `static UTF8_TAIL: spin::Mutex<crate::input::Utf8Tail> =`

```
/// Decode a raw scancode into an ASCII character (handles modifiers + extended).
/// Der Rest einer UTF-8-Folge — siehe `input::Utf8Tail` fuer das Warum.
/// Eigene Instanz, weil dieser Treiber ein eigener Erzeuger ist; die
/// Rechnung ist die geteilte.
```

## L683-687 · `fn take_tail() -> Option<u8> {`

```
/// Das naechste wartende Byte, oder `None`. **Muss vor dem Lesen eines neuen
/// Scancodes gerufen werden.**
/// Taken by the timer ISR (`poll_ps2_irq`) AND by `read_key` with IF=1 —
/// the non-ISR side must mask interrupts, or a tick inside the lock spins
/// forever on it.
```

## L692 · `fn decode_scancode(scancode: u8) -> Option<u8> {`

```
/// Scancode → erstes Byte des Zeichens; der Rest wandert in `UTF8_TAIL`.
```

## L699 · `if scancode == 0xE0 {`

```
// Extended prefix: set flag, wait for next scancode
```

## L713 · `if is_extended {`

```
// Handle extended scancodes (arrow keys, Home, End, etc.)
```

## L715-717 · `match code {`

```
// Modifiers: handle BOTH press and release (must run before the
// release-gate below, or the key-up event is swallowed and the
// modifier stays stuck — was the case for Super on PS/2 polling).
```

## L719 · `0x1D => { CTRL.store(!released, Ordering::Relaxed); return None; }   // Right Ctrl`

```
// Right Ctrl
```

## L720 · `0x38 => { ALT_GR.store(!released, Ordering::Relaxed); return None; } // AltGr (Right Alt)`

```
// AltGr (Right Alt)
```

## L721 · `0x5B | 0x5C => { set_super(!released); return None; }                // Super/Meta (left/right)`

```
// Super/Meta (left/right)
```

## L736-738 · `0x20 => { toggle_mute(); return None; }`

```
// Media keys, Set 1 as the i8042 translates them (Linux atkbd
// keymap: E0 20 KEY_MUTE, E0 2E KEY_VOLUMEDOWN, E0 30
// KEY_VOLUMEUP). On a notebook the Fn row sends these.
```

## L746 · `match code {`

```
// Normal scancodes — modifiers
```

## L767-769 · `if ctrl && shift && code == 0x2E {`

```
// Ctrl+Shift+C → copy the selection (terminal drag-selection or focused
// Input/TextArea) via a ShadeAction (runs in the main loop, not here).
// In a terminal Ctrl+C must stay SIGINT, so copy is the Shift variant.
```

## L774-775 · `if ctrl && shift && code == 0x2F {`

```
// Ctrl+Shift+V → paste the clipboard. Plain Ctrl+V still reaches widgets
// as 'v'+ctrl. Scancode 0x2F = 'v'.
```

## L780-781 · `if SUPER.load(Ordering::Relaxed) && matches!(code, 0x02..=0x0A) {`

```
// Mod+1..9 / Mod+Shift+1..9 by scancode, before the layout turns the
// digit into "+" / "!" / nothing (see push_workspace_key). Set-1 0x02='1'.
```

## L787-791 · `crate::intent::request_cancel();`

```
// Ctrl+C → terminal SIGINT: cancel a running foreground intent
// (download/OTA). Set here so it works even when the main loop is
// blocked in a download — the timer IRQ's poll_ps2_irq still decodes
// scancodes and reaches this point. (The 0x03 is also buffered for the
// normal read path.)
```

## L799 · `if alt_gr && is_de {`

```
// AltGr: special characters (de_CH layout)
```

## L811 · `pub fn irq_handler() {`

```
/// IRQ1 handler — called from interrupts.rs.
```

## L816-818 · `while let Some(b) = take_tail() { push_key(b) }`

```
// Ein Zeichen ausserhalb von ASCII besteht aus mehreren Bytes; sie
// muessen DIREKT hintereinander in den Ring, sonst steht ein
// Tastendruck zwischen den Haelften eines Buchstabens.
```

## L823 · `fn scancode_to_char_us(code: u8, shift: bool, caps: bool) -> Option<char> {`

```
/// Scancode Set 1 → ASCII (US layout)
```

## L827 · `0,   0x1B, b'1', b'2', b'3', b'4', b'5', b'6',  // 0x00-0x07`

```
// 0x00-0x07
```

## L828 · `b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t', // 0x08-0x0F`

```
// 0x08-0x0F
```

## L829 · `b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',  // 0x10-0x17`

```
// 0x10-0x17
```

## L830 · `b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',  // 0x18-0x1F`

```
// 0x18-0x1F
```

## L831 · `b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',  // 0x20-0x27`

```
// 0x20-0x27
```

## L832 · `b'\'',b'', 0,   b'\\',b'z', b'x', b'c', b'v',   // 0x28-0x2F`

```
// 0x28-0x2F
```

## L833 · `b'b', b'n', b'm', b',', b'.', b'/', 0,   b'*',   // 0x30-0x37`

```
// 0x30-0x37
```

## L834 · `0,   b' ',                                         // 0x38-0x39`

```
// 0x38-0x39
```

## L851-853 · `let ch = if shift { SHIFTED[code as usize] } else { NORMAL[code as usize] };`

```
// Die US-Tabelle bleibt Bytes — sie IST ASCII, jedes Zeichen darin passt
// in eins. Nur die Rueckgabe ist ein Zeichen, damit beide Layouts
// dieselbe Form haben.
```

## L863-864 · `fn altgr_char_de(code: u8) -> Option<char> {`

```
/// AltGr characters for Swiss German (de_CH) keyboard layout.
/// PS/2 Scancode Set 1 → ASCII.
```

## L867 · `0x03 => Some('@'),   // AltGr+2`

```
// AltGr+2
```

## L868 · `0x04 => Some('#'),   // AltGr+3`

```
// AltGr+3
```

## L869 · `0x08 => Some('|'),   // AltGr+7`

```
// AltGr+7
```

## L870 · `0x0D => Some('~'),   // AltGr+^`

```
// AltGr+^
```

## L871 · `0x12 => Some('€'),   // AltGr+e — xkb: AD03 dritte Ebene EuroSign`

```
// AltGr+e — xkb: AD03 dritte Ebene EuroSign
```

## L872 · `0x1A => Some('['),   // AltGr+ü`

```
// AltGr+ü
```

## L873 · `0x1B => Some(']'),   // AltGr+¨`

```
// AltGr+¨
```

## L874 · `0x28 => Some('{'),   // AltGr+ä`

```
// AltGr+ä
```

## L875 · `0x2B => Some('}'),   // AltGr+$`

```
// AltGr+$
```

## L876 · `0x56 => Some('\\'),  // AltGr+<`

```
// AltGr+<
```

## L881 · `fn scancode_to_char_de(code: u8, shift: bool, caps: bool) -> Option<char> {`

```
/// Scancode Set 1 → ASCII (Swiss German / DE_CH layout)
```

## L883-887 · `if code == 0x56 {`

```
// ISO-Extra key (102-key layout, left of Z): scancode 0x56.
// Plain → `<`, Shift → `>`, AltGr → `\` (latter handled by
// altgr_char_de above). The key is OUT OF RANGE of the layout
// arrays below (which only cover 0x00–0x39), so it needs to
// be special-cased before the array index.
```

## L892-902 · `#[rustfmt::skip]`

```
// **Die Tabellen stammen aus `/usr/share/X11/xkb/symbols/ch`,
// `xkb_symbols "basic"` (German (Switzerland)) — nicht aus dem
// Gedaechtnis.** Der Unterschied ist keiner, den man raten kann: auf dem
// DEUTSCHschweizer Layout liegt `ü` UNGESCHIFTET und `è` auf Shift, beim
// franzoesischschweizerischen genau andersherum (dieselbe Datei, Block
// `fr`, ueberschreibt die drei Tasten).
//
// Bis hierher stand in beiden Tabellen die Zweitbelegung dieser Tasten:
// `[ ] ; '` — also genau die Zeichen, die AltGr ohnehin liefert. Ein
// Umlaut liess sich damit auf dieser Maschine ueberhaupt nicht tippen,
// und bei `ç` stand woertlich `non-ASCII→0`.
```

## L905 · `'\0','\u{1B}','1','2','3','4','5','6',        // 0x00-0x07`

```
// 0x00-0x07
```

## L906 · `'7', '8', '9', '0', '\'','^','\u{8}','\t',      // 0x08-0x0F  0x0D = ^ (Totaste, hier direkt)`

```
// 0x08-0x0F  0x0D = ^ (Totaste, hier direkt)
```

## L907 · `'q', 'w', 'e', 'r', 't', 'z', 'u', 'i',     // 0x10-0x17  (z/y getauscht)`

```
// 0x10-0x17  (z/y getauscht)
```

## L908 · `'o', 'p', 'ü', '¨', '\n','\0','a', 's',      // 0x18-0x1F  AD11=ü, AD12=¨`

```
// 0x18-0x1F  AD11=ü, AD12=¨
```

## L909 · `'d', 'f', 'g', 'h', 'j', 'k', 'l', 'ö',     // 0x20-0x27  AC10=ö`

```
// 0x20-0x27  AC10=ö
```

## L910 · `'ä', '§', '\0','$', 'y', 'x', 'c', 'v',      // 0x28-0x2F  AC11=ä, TLDE=§`

```
// 0x28-0x2F  AC11=ä, TLDE=§
```

## L911 · `'b', 'n', 'm', ',', '.', '-', '\0','*',      // 0x30-0x37`

```
// 0x30-0x37
```

## L912 · `'\0',' ',                                    // 0x38-0x39`

```
// 0x38-0x39
```

## L917 · `'\0','\u{1B}','+','"', '*', 'ç','%', '&',     // 0x00-0x07  AE04 Shift = ç`

```
// 0x00-0x07  AE04 Shift = ç
```

## L918 · `'/', '(', ')', '=', '?', '','\u{8}','\t',      // 0x08-0x0F`

```
// 0x08-0x0F
```

## L920 · `'O', 'P', 'è', '!', '\n','\0','A', 'S',       // AD11 Shift = è, AD12 Shift = !`

```
// AD11 Shift = è, AD12 Shift = !
```

## L921 · `'D', 'F', 'G', 'H', 'J', 'K', 'L', 'é',     // AC10 Shift = é`

```
// AC10 Shift = é
```

## L922 · `'à', '°', '\0','£', 'Y', 'X', 'C', 'V',      // AC11 Shift = à, TLDE Shift = °, BKSL Shift = £`

```
// AC11 Shift = à, TLDE Shift = °, BKSL Shift = £
```

## L932-933 · `if caps && ch.is_alphabetic() {`

```
// Feststelltaste kehrt die Schreibung um — und zwar UNICODE-weise, sonst
// bliebe `ü` als einziges Zeichen der Tabelle davon unberuehrt.
```

