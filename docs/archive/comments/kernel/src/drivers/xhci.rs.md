# `kernel/src/drivers/xhci.rs` @ 5e0102684

## L1-4 · `use crate::{kprintln, pci, paging, memory};`

```
//! xHCI USB Host Controller Driver
//!
//! Minimal implementation for USB HID boot-protocol keyboards.
//! Polling model, single device, no hubs.
```

## L10 · `fn r32(base: u64, off: u32) -> u32 {`

```
// === MMIO helpers ===
```

## L13 · `unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) }`

```
// SAFETY: MMIO read from mapped, uncacheable region
```

## L17 · `unsafe { core::ptr::write_volatile((base + off as u64) as *mut u32, val); }`

```
// SAFETY: MMIO write to mapped, uncacheable region
```

## L21 · `unsafe { core::ptr::read_volatile((base + off as u64) as *const u8) }`

```
// SAFETY: MMIO read
```

## L25 · `w32(base, off, val as u32);`

```
// SAFETY: 64-bit MMIO write (low then high)
```

## L30 · `const CAP_CAPLENGTH:  u32 = 0x00;`

```
// === Capability register offsets (from BAR0) ===
```

## L38 · `const OP_USBCMD:  u32 = 0x00;`

```
// === Operational register offsets (from oper_base) ===
```

## L47 · `const CMD_RUN:  u32 = 1 << 0;`

```
// USBCMD bits
```

## L50 · `const CMD_INTE: u32 = 1 << 2; // interrupter enable`

```
// interrupter enable
```

## L52 · `const STS_HCH: u32 = 1 << 0;  // HC Halted`

```
// USBSTS bits
```

## L53 · `const STS_HCH: u32 = 1 << 0;  // HC Halted`

```
// HC Halted
```

## L54 · `const STS_EINT: u32 = 1 << 3; // event interrupt (write 1 to clear)`

```
// event interrupt (write 1 to clear)
```

## L55 · `const STS_CNR: u32 = 1 << 11; // Controller Not Ready`

```
// Controller Not Ready
```

## L57 · `const PORTSC_CCS:   u32 = 1 << 0;  // Current Connect Status`

```
// PORTSC bits
```

## L58 · `const PORTSC_CCS:   u32 = 1 << 0;  // Current Connect Status`

```
// Current Connect Status
```

## L59 · `const PORTSC_PED:   u32 = 1 << 1;  // Port Enabled`

```
// Port Enabled
```

## L60 · `const PORTSC_PR:    u32 = 1 << 4;  // Port Reset (hot — USB2)`

```
// Port Reset (hot — USB2)
```

## L61 · `const PORTSC_WPR:   u32 = 1 << 31; // Warm Port Reset (USB3 link recovery)`

```
// Warm Port Reset (USB3 link recovery)
```

## L63 · `const PORTSC_PLS_MASK: u32 = 0xF << 5; // Port Link State`

```
// Port Link State
```

## L64 · `const PORTSC_PP:    u32 = 1 << 9;  // Port Power`

```
// Port Power
```

## L67 · `const PORTSC_PRC:   u32 = 1 << 21; // Port Reset Change`

```
// Port Reset Change
```

## L68 · `const PORTSC_CSC:   u32 = 1 << 17; // Connect Status Change`

```
// Connect Status Change
```

## L69 · `const PORTSC_PEC:   u32 = 1 << 18; // Port Enabled Change`

```
// Port Enabled Change
```

## L70 · `const PORTSC_WRC:   u32 = 1 << 19; // Warm Port Reset Change`

```
// Warm Port Reset Change
```

## L71 · `const PORTSC_PLC:   u32 = 1 << 22; // Port Link State Change`

```
// Port Link State Change
```

## L72 · `const PORTSC_RO: u32 = (1 << 0) | (1 << 3) | (0xF << 10) | (1 << 30);`

```
/// Read-only bits and RW state bits — Linux `XHCI_PORT_RO` / `XHCI_PORT_RWS`.
```

## L76-79 · `fn port_neutral(sc: u32) -> u32 {`

```
/// PORTSC value that changes nothing when written back — Linux
/// `xhci_port_state_to_neutral`. Masking only the RW1C change bits is not
/// enough: PED is RW1CS too, and a 1 written back DISABLES an enabled port.
/// A second reset of an already enabled port then never completes.
```

## L84 · `const SPEED_FULL:  u32 = 1;`

```
// Port speeds
```

## L90 · `const TRB_NORMAL:         u32 = 1 << 10;`

```
// TRB types (in control field bits [15:10])
```

## L104 · `const TRB_CYCLE:     u32 = 1 << 0;`

```
// TRB control bits
```

## L106 · `const TRB_IOC:       u32 = 1 << 5;  // Interrupt On Completion`

```
// Interrupt On Completion
```

## L107 · `const TRB_IDT:       u32 = 1 << 6;  // Immediate Data`

```
// Immediate Data
```

## L109 · `const TRB_BSR:       u32 = 1 << 9;  // Block Set Address Request (address device)`

```
// Block Set Address Request (address device)
```

## L110 · `const TRB_DIR_IN:    u32 = 1 << 16; // Direction: IN`

```
// Direction: IN
```

## L111 · `const TRB_TRT_NO:    u32 = 0;       // Transfer Type: No Data`

```
// Transfer Type: No Data
```

## L112 · `const TRB_TRT_IN:    u32 = 3 << 16; // Transfer Type: IN Data`

```
// Transfer Type: IN Data
```

## L114 · `const EVT_TRANSFER:      u32 = 32 << 10;`

```
// Event TRB types (bits [15:10] of control)
```

## L120 · `const CC_SUCCESS:        u32 = 1;`

```
// Completion codes
```

## L124 · `const EP_TYPE_CONTROL:      u32 = 4;`

```
// Endpoint types in endpoint context
```

## L128 · `const USB_GET_DESCRIPTOR: u8 = 6;`

```
// USB request types
```

## L134 · `#[allow(dead_code)]`

```
// Descriptor types
```

## L143 · `static HID_TO_ASCII: [u8; 57] = [`

```
// HID usage code to ASCII table (boot protocol, US layout base)
```

## L145 · `0, 0, 0, 0,                                       // 0x00-0x03`

```
// 0x00-0x03
```

## L146 · `b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h',  // 0x04-0x0B`

```
// 0x04-0x0B
```

## L147 · `b'i', b'j', b'k', b'l', b'm', b'n', b'o', b'p',  // 0x0C-0x13`

```
// 0x0C-0x13
```

## L148 · `b'q', b'r', b's', b't', b'u', b'v', b'w', b'x',  // 0x14-0x1B`

```
// 0x14-0x1B
```

## L149 · `b'y', b'z',                                        // 0x1C-0x1D`

```
// 0x1C-0x1D
```

## L150 · `b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', // 0x1E-0x27`

```
// 0x1E-0x27
```

## L151 · `b'\n', 0x1B, 0x08, b'\t', b' ',                    // 0x28-0x2C (enter,esc,bs,tab,space)`

```
// 0x28-0x2C (enter,esc,bs,tab,space)
```

## L152 · `b'-', b'=', b'[', b']', b'\\',                     // 0x2D-0x31`

```
// 0x2D-0x31
```

## L153 · `0, b';', b'\'', b'', b',', b'.', b'/',            // 0x32-0x38`

```
// 0x32-0x38
```

## L168-186 · `static HID_TO_ASCII_DE: [char; 57] = [`

```
// Swiss German layout: remap HID usage codes
// Key differences: z↔y swap, number row shifted chars, special chars
// (HID 0x64 = non-US `<`/`>` lives OUTSIDE this 57-entry range and is
//  special-cased in `hid_to_char` so plain `<` and shift `>` work.)
// **Aus `/usr/share/X11/xkb/symbols/ch`, `xkb_symbols "basic"` (German
// (Switzerland)) — nicht aus dem Gedaechtnis.** Hier standen bis 0.335.0 die
// ZWEITbelegungen dieser Tasten (`[ ] ; '`), also genau die Zeichen, die
// AltGr ohnehin liefert; ein Umlaut liess sich auf einer USB-Tastatur
// ueberhaupt nicht tippen.
//
// **Und das war der zweite Anlauf.** 0.333.0 hatte dieselbe Tabelle im
// PS/2-Treiber repariert und diese hier uebersehen — die NUC haengt an USB,
// also aenderte sich am Geraet gar nichts. Es gibt ZWEI Tastaturtreiber, und
// wer einen davon anfasst, hat die Haelfte angefasst
// ([[feedback_verify_the_call_path]]).
//
// Gegengeprueft wird das jetzt maschinell: `tools/kbcheck.py` liest die
// xkb-Referenz, kalibriert die Indexzuordnung an unseren EIGENEN
// US-Tabellen und stellt beide Treiber daneben.
```

## L192 · `'z', 'y',                                        // z/y getauscht`

```
// z/y getauscht
```

## L195 · `'\'','^', 'ü', '¨', '$',`

```
// 0x2D..0x31: AE11 AE12 AD11 AD12 BKSL
```

## L197 · `'\0','ö', 'ä', '§', ',', '.', '-',`

```
// 0x32..0x38: (nicht belegt) AC10 AC11 TLDE AB08 AB09 AB10
```

## L207 · `'+', '"', '*', 'ç','%', '&', '/', '(', ')', '=',   // AE04 Shift = ç`

```
// AE04 Shift = ç
```

## L209 · `'?', '', 'è', '!', '£',                           // AD11 è · AD12 ! · BKSL £`

```
// AD11 è · AD12 ! · BKSL £
```

## L210 · `'\0','é', 'à', '°', ';', ':', '_',                  // AC10 é · AC11 à · TLDE °`

```
// AC10 é · AC11 à · TLDE °
```

## L213 · `const KEY_BUF_SIZE: usize = 32;`

```
// Key buffer — IRQ-safe SPSC ring (producer: IRQ/poll, consumer: main thread)
```

## L223 · `unsafe { KEY_BUF[head] = k; }`

```
// SAFETY: single producer (IRQ or poll), head only written here
```

## L229 · `#[derive(Clone, Copy)]`

```
/// Mouse event from USB HID boot protocol.
```

## L232 · `pub buttons: u8,  // bit 0=left, 1=right, 2=middle`

```
// bit 0=left, 1=right, 2=middle
```

## L236-240 · `pub hscroll: i8,`

```
/// Waagrechtes Rollen (AC Pan), positiv = nach RECHTS.
///
/// Eine eigene Achse und kein Vorzeichen an `scroll`: beide koennen
/// im selben Ereignis stehen, und ein Touchpad liefert sie auch
/// gleichzeitig. Die USB-Boot-Maus kennt sie nicht und laesst sie 0.
```

## L244 · `const MOUSE_BUF_SIZE: usize = 128;`

```
// Mouse event buffer — IRQ-safe SPSC ring (producer: IRQ/poll, consumer: main thread)
```

## L250-255 · `static POINTER_LOCK: spin::Mutex<()> = spin::Mutex::new(());`

```
/// Serialisiert ALLE Einspeiser des Zeigers.
///
/// Der Ring war als Einzelerzeuger gebaut ("single producer (IRQ or poll)")
/// und ist es nicht mehr: PS/2 speist aus dem Timer-IRQ ein, USB aus dem
/// Drain — der seit 0.371.0 auch aus dem Netzpfad auf einem Worker-Kern
/// laeuft —, und ein WASM-Treiber (Touchpad) von einem beliebigen Kern.
```

## L258 · `fn push_mouse_locked(evt: MouseEvent) {`

```
/// Den Ring beschicken. **Nur mit gehaltener `POINTER_LOCK` rufen.**
```

## L263-264 · `unsafe { MOUSE_BUF[head] = evt; }`

```
// SAFETY: die Sperre macht daraus genau einen Schreiber; `head`
// wird nur hier geschrieben.
```

## L270-286 · `fn inject_pointer(evt: MouseEvent, cheap: bool) {`

```
/// Ein Zeigerereignis einspeisen — aus JEDER Quelle.
///
/// Zwei Dinge dahinter sind Lese-Aendern-Schreiben und muessen zusammen
/// geschehen: der Ringschub und `cursor::update_atomic`. Der Name des
/// zweiten taeuscht — es ist eine FOLGE einzelner Atomzugriffe, und sie
/// traegt dabei die VORIGE Tastenlage nach. Verschraenken sich zwei
/// Laeufe, geht ein Klick verloren oder es entsteht einer, den niemand
/// gemacht hat.
///
/// Waehrend die Sperre gehalten wird, sind die Interrupts DIESES Kerns
/// aus: sonst liefe sein eigener Timer-IRQ hier hinein und drehte sich auf
/// der Sperre fest, die er selbst haelt.
///
/// `cheap` waehlt den Malweg: die Maus bewegt nur den Zeiger
/// (`request_cursor_move`), ein PS/2- oder Treiberereignis verlangt ein
/// volles Bild. Beides liegt AUSSERHALB der Sperre — es ist teuer und
/// braucht sie nicht.
```

## L301-304 · `pub fn inject_mouse(evt: MouseEvent) {`

```
/// Inject a mouse event from another input source (e.g. the PS/2 touchpad on
/// the i8042 aux port) into the same ring `poll_mouse` drains, so the main
/// loop's existing `poll_mouse()` delivers it with no extra plumbing. Marks the
/// pointer available on first event.
```

## L309-310 · `pub fn poll_keyboard() -> Option<u8> {`

```
/// Poll for a key from the USB keyboard. Called from keyboard.rs.
/// Reads from software buffer only — timer IRQ drains hardware.
```

## L314 · `let head = KEY_HEAD.load(Ordering::Acquire);`

```
// Read from software key buffer (filled by timer IRQ)
```

## L318 · `let k = unsafe { KEY_BUF[tail] };`

```
// SAFETY: single consumer (main thread), tail only written here
```

## L324 · `let rk = REPEAT_KEY.load(Ordering::Relaxed);`

```
// Timer-based key repeat (lock-free: read repeat state from atomics)
```

## L326-327 · `if let Some(b) = take_tail() { return Some(b) }`

```
// Wartet noch die zweite Haelfte eines Zeichens? Die zuerst — vor dem
// naechsten Tastendruck, sonst geht sie verloren.
```

## L353 · `static REPEAT_KEY: AtomicU8 = AtomicU8::new(0);`

```
// Lock-free key repeat state (written by timer IRQ, read by poll_keyboard)
```

## L360-361 · `pub fn poll_mouse() -> Option<MouseEvent> {`

```
/// Poll for a mouse event from USB mouse.
/// Reads from software buffer only — timer IRQ drains hardware.
```

## L368 · `let evt = unsafe { MOUSE_BUF[tail] };`

```
// SAFETY: single consumer (main thread), tail only written here
```

## L376 · `pub fn mouse_available() -> bool { MOUSE_AVAILABLE.load(Ordering::Relaxed) }`

```
/// Check if USB mouse is available.
```

## L384-388 · `pci_addr: pci::PciAddr,`

```
/// PCI-Adresse dieses Controllers. Ohne sie laesst sich nicht sagen, ob
/// ein anderer Weg (der NIC-Scan) GENAU DIESEN zurueckgesetzt hat oder
/// einen daneben — und "irgendeiner wurde angefasst, also alles
/// wegwerfen" wirft auf einer Maschine mit mehreren Controllern eine
/// funktionierende Tastatur weg.
```

## L391 · `oper: u64,          // operational registers base`

```
// operational registers base
```

## L392 · `rt: u64,            // runtime registers base`

```
// runtime registers base
```

## L393 · `db: u64,            // doorbell array base`

```
// doorbell array base
```

## L394 · `ctx_size: usize,    // 32 or 64`

```
// 32 or 64
```

## L396 · `dcbaa: u64,`

```
// DMA regions (physical = virtual, identity-mapped)
```

## L413 · `data_buf: u64,      // general-purpose DMA buffer (4KB)`

```
// general-purpose DMA buffer (4KB)
```

## L416 · `intr_ep_dci: u8,     // DCI of interrupt IN endpoint`

```
// DCI of interrupt IN endpoint
```

## L417 · `prev_keys: [u8; 6],  // previous HID report keys`

```
// previous HID report keys
```

## L418 · `repeat_key: u8,      // key currently held for repeat`

```
// key currently held for repeat
```

## L419 · `repeat_shift: bool,  // shift state when repeat started`

```
// shift state when repeat started
```

## L420 · `repeat_altgr: bool,  // altgr state when repeat started`

```
// altgr state when repeat started
```

## L421 · `repeat_start: u64,   // tick when key was first pressed`

```
// tick when key was first pressed
```

## L422 · `repeat_last: u64,    // tick when last repeat was emitted`

```
// tick when last repeat was emitted
```

## L423 · `port_num: u32,       // connected port number`

```
// connected port number
```

## L424 · `error_count: u32,    // consecutive transfer errors`

```
// consecutive transfer errors
```

## L425-432 · `has_keyboard: bool,`

```
// Port probed during keyboard search but wasn't keyboard (reuse for mouse)
// Mouse device (second USB device on same controller)
/// Traegt DIESER Controller die Tastatur?
///
/// Bisher sagte das die globale Flagge `AVAILABLE`, und der Drain
/// schloss per `else` auf "dann ist es die Tastatur". Sobald mehr als
/// ein Geraet am Controller haengt, ist dieser Schluss falsch: ein
/// NIC-Ereignis landete damit in `process_hid_report`.
```

## L435 · `nic_device_ctx: u64,`

```
/// DMA-Satz fuer ein Netzgeraet an DIESEM Controller.
```

## L438-441 · `nic: Option<NicRings>,`

```
/// Das Netzgeraet, wenn eines hier haengt. **Kein eigener
/// Controller-Zustand mehr:** frueher trug `NicXhci` eine zweite
/// `XhciState` mit eigenem Dequeue-Zeiger auf denselben Ereignisring,
/// und wer zuerst drainte, nahm dem anderen seine Ereignisse weg.
```

## L458-461 · `const MAX_CTRLS: usize = 4;`

```
/// Wieviele xHCI-Controller wir gleichzeitig fuehren.
///
/// Florians IdeaPad hat zwei (04:00.3 und 04:00.4), Desktops gelegentlich
/// drei. Vier ist Platz mit Luft, und die Tabelle kostet nur Zeiger.
```

## L464-469 · `static CTRLS: spin::Mutex<[Option<XhciState>; MAX_CTRLS]> =`

```
/// **Ein Zustand JE CONTROLLER.** Vorher gab es genau einen, und daraus
/// folgten zwei Fehler, die wie verschiedene aussahen: eine Maus am
/// ZWEITEN Controller wurde nie gesucht (`init_mouse` lief nur ueber den
/// Controller der Tastatur), und der USB-Dongle-Scan raeumte den
/// Controller aus, auf dem der Zeiger sass. Beides ist dieselbe Annahme —
/// ein Controller hat einen Besitzer.
```

## L473-474 · `fn store_ctrl(state: XhciState) -> bool {`

```
/// Einen hochgefahrenen Controller ablegen: ersetzt den Eintrag mit
/// derselben PCI-Adresse, sonst der erste freie Platz.
```

## L495-499 · `pub fn init() -> bool {`

```
// `ctrl_has_hid` gab es hier, um beim NIC-Scan den Controller mit dem
// Eingabegeraet ZULETZT anzufassen. Die Reihenfolge war noetig, solange das
// Anfassen einen Reset bedeutete. Seit `nic_probe` auf dem LAUFENDEN
// Controller sucht, gibt es nichts mehr zu schonen — und der ganze Grund
// fuer die Schonung ist weg.
```

## L501-502 · `pub fn init() -> bool {`

```
/// Initialize xHCI controller and enumerate USB keyboard.
/// Tries all xHCI controllers until one with a connected device is found.
```

## L505 · `for bus in 0u16..=255 {`

```
// Find all xHCI controllers (class 0C:03:30) and bring up each
```

## L527-530 · `if init_controller(pci_dev) { found = true; }`

```
// Kein frueher Ausstieg mehr: ein Controller, den
// wir nie hochgefahren haben, kann spaeter auch kein
// Geraet hergeben — und die Maus des IdeaPad sass
// genau auf dem, den wir uebersprungen haben.
```

## L540-543 · `fn bring_up_controller(dev: pci::PciDevice, max_slots_en: u32) -> Option<XhciState> {`

```
/// Bring a controller from PCI-discovered to running: map BAR0, halt+reset,
/// set up command/event rings + scratchpad, start it, power and settle ports.
/// Returns the running state with no device enumerated yet. `max_slots_en`
/// caps how many device slots the HC will accept (Address Device).
```

## L554-562 · `if dev.addr.bus > 0 {`

```
// Enable mem-space + bus-mastering on EVERY bridge in the path to the
// controller, not just the top-level one. The Titan Ridge TB3 xHCI sits
// behind a chain of bridges (bus 0 → … → its bus); upstream DMA (command +
// event ring) only traverses a bridge whose Bus Master Enable is set. MMIO
// works without it, so the controller "runs" and ports are visible, but
// every command (Enable Slot) times out because its completion never DMAs
// back to the event ring. Every ancestor bridge's [secondary..subordinate]
// range contains the target bus, so enabling all matching bridges across
// all buses covers the whole chain.
```

## L575 · `pci::write32(ba, 0x04, (bcmd | 0x06) as u32); // mem + bus-master`

```
// mem + bus-master
```

## L584 · `let bar0_raw = pci::read32(dev.addr, 0x10);`

```
// BAR0 (64-bit)
```

## L593 · `let map_size = 64 * 1024u64;`

```
// Map BAR0 (64KB)
```

## L605 · `let caplength = r8(mmio, CAP_CAPLENGTH) as u32;`

```
// Read capability registers
```

## L623 · `let xecp_off = ((hccparams1 >> 16) & 0xFFFF) as u32 * 4;`

```
// BIOS/OS handoff via extended capabilities
```

## L629 · `let cmd_val = r32(oper, OP_USBCMD);`

```
// Halt controller
```

## L637 · `w32(oper, OP_USBCMD, CMD_HCRST);`

```
// Reset controller
```

## L648 · `let dcbaa = alloc_dma(1, "DCBAA");`

```
// Allocate DMA structures (all page-aligned, zeroed)
```

## L661-663 · `let nic_device_ctx = alloc_dma(1, "nic dev ctx");`

```
// Ein DRITTER Satz, fuer ein Netzgeraet am selben Controller. Ohne ihn
// muesste es sich einen mit Tastatur oder Maus teilen — und genau daran
// ist heute alles gestorben, was als drittes kam.
```

## L675 · `write_trb(cmd_ring, NUM_CMD_TRBS - 1, cmd_ring, 0, TRB_LINK | TRB_CYCLE | (1 << 1)); // Toggle Cycle`

```
// Set up Link TRBs at end of rings (wrap back to start)
```

## L676 · `write_trb(cmd_ring, NUM_CMD_TRBS - 1, cmd_ring, 0, TRB_LINK | TRB_CYCLE | (1 << 1)); // Toggle Cycle`

```
// Toggle Cycle
```

## L683-684 · `unsafe {`

```
// Set up Event Ring Segment Table (1 entry)
// SAFETY: writing to DMA-allocated, zeroed memory
```

## L687 · `core::ptr::write_volatile(seg, evt_ring);           // ring base address`

```
// ring base address
```

## L689 · `NUM_EVT_TRBS as u64);                            // ring size`

```
// ring size
```

## L692 · `let sp_hi = (hcsparams2 >> 21) & 0x1F;`

```
// Scratchpad buffers
```

## L702 · `unsafe { core::ptr::write_volatile((sp_array as *mut u64).add(i), page); }`

```
// SAFETY: writing to DMA array
```

## L705-706 · `unsafe { core::ptr::write_volatile(dcbaa as *mut u64, sp_array); }`

```
// DCBAA[0] = scratchpad array pointer
// SAFETY: writing to DMA array
```

## L711 · `w32(oper, OP_CONFIG, max_slots_en); // MaxSlotsEn`

```
// Program controller
```

## L712 · `w32(oper, OP_CONFIG, max_slots_en); // MaxSlotsEn`

```
// MaxSlotsEn
```

## L714 · `w64(oper, OP_CRCR, cmd_ring | 1); // cycle bit = 1`

```
// cycle bit = 1
```

## L716 · `let ir0 = rt + 0x20; // interrupter 0 offset`

```
// Program Event Ring (interrupter 0)
```

## L717 · `let ir0 = rt + 0x20; // interrupter 0 offset`

```
// interrupter 0 offset
```

## L718 · `w32(ir0, 0x08, 1);              // ERSTSZ = 1 segment`

```
// ERSTSZ = 1 segment
```

## L719 · `w64(ir0, 0x18, evt_ring);       // ERDP`

```
// ERDP
```

## L720 · `w64(ir0, 0x10, evt_seg_table);  // ERSTBA (write AFTER ERSTSZ)`

```
// ERSTBA (write AFTER ERSTSZ)
```

## L721 · `w32(ir0, 0x00, r32(ir0, 0x00) | 0x02); // IMAN.IE = 1`

```
// Enable interrupter (for event ring to work, even in polling mode)
```

## L722 · `w32(ir0, 0x00, r32(ir0, 0x00) | 0x02); // IMAN.IE = 1`

```
// IMAN.IE = 1
```

## L724 · `w32(oper, OP_USBCMD, CMD_RUN);`

```
// Start controller
```

## L732-734 · `if pci::program_msix(dev.addr, 0, crate::interrupts::XHCI_VECTOR,`

```
// Interrupter 0 by MSI-X to Core 0 (stage 3b). Until now the event ring
// was drained only from the timer tick. Without MSI-X the tick keeps
// doing it, as before.
```

## L765 · `for p in 0..max_ports {`

```
// Power on all ports
```

## L774-778 · `const POLL_MS:   u64 = 10;`

```
// Wait for device attachment + link training. A flat 500 ms was half a
// second of every boot spent staring at ports that had already settled.
// Poll instead and leave once the picture stops changing: USB 2.0 §7.1.7.3
// wants 100 ms of connect debounce, so that is the floor, and a device
// that shows up late keeps the window open rather than being missed.
```

## L780 · `const DEBOUNCE:  u64 = 100;   // §7.1.7.3 TATTDB`

```
// §7.1.7.3 TATTDB
```

## L781 · `const SETTLE_MS: u64 = 100;   // quiet time before we call it done`

```
// quiet time before we call it done
```

## L782 · `const CAP_MS:    u64 = 500;   // what the flat wait used to be`

```
// what the flat wait used to be
```

## L796-797 · `if connected > 0`

```
// Nothing connected yet → keep waiting; we cannot tell "empty port"
// from "slow device", so an idle controller still costs the full cap.
```

## L806 · `for p in 0..max_ports {`

```
// Debug: show all port states
```

## L818 · `fn init_controller(dev: pci::PciDevice) -> bool {`

```
/// Initialize xHCI controller and enumerate a USB keyboard (HID boot protocol).
```

## L825-845 · `for p in 0..state.max_ports {`

```
// JEDEN belegten Port anfassen, und was keine Tastatur ist, gibt seinen
// Slot ZURUECK.
//
// Vorher stand hier: "Non-keyboard devices are left alone (slot stays
// allocated, no cleanup)" — das erste fremde Geraet behielt Slot und
// DMA-Satz, damit `init_mouse` es ohne neues Aufzaehlen uebernehmen
// konnte. Daraus folgten drei Dinge, die sich widersprachen: es gab nur
// ZWEI Saetze, also einen Deckel von zwei angefassten Ports (auf einem
// Notebook verbrauchen Kamera, Fingerabdruckleser und Bluetooth ihn,
// bevor die Buchse mit der Tastatur drankommt); ein spaeter probierter
// Port nahm dem gemerkten seinen Satz weg; und liegengebliebene Slots
// liessen ein spaeteres Address Device auf demselben Port scheitern.
//
// Nachgestellt in QEMU (Maus auf dem ersten Port, ein weiteres Geraet
// dahinter — die Reihenfolge von Florians IdeaPad): die Maus wurde als
// "composite mouse device" ERKANNT und war danach nicht mehr
// ansprechbar, weder ueber den gemerkten Slot noch ueber einen
// frischen.
//
// Wer aufraeumt, braucht nichts davon: ein Satz reicht fuer beliebig
// viele Ports, der Deckel faellt weg, und jeder Anlauf faengt sauber an.
```

## L850-851 · `unsafe {`

```
// Sauber anfangen: der Satz kann vom vorigen Port her Reste tragen.
// SAFETY: eigener DMA-Speicher, kein Geraet zeigt mehr darauf.
```

## L861 · `kprintln!("[npk] xhci: resetting port {}...", p + 1);`

```
// Reset port
```

## L865 · `continue;   // kein Slot vergeben — kostet keinen Satz`

```
// kein Slot vergeben — kostet keinen Satz
```

## L870 · `kprintln!("[npk] xhci: enable slot...");`

```
// Enable Slot
```

## L879-880 · `unsafe {`

```
// Set DCBAA entry for this slot
// SAFETY: writing to DMA array
```

## L888 · `let max_packet = match state.port_speed {`

```
// Address Device
```

## L904 · `kprintln!("[npk] xhci: getting config descriptor...");`

```
// Get Configuration Descriptor (9 bytes first to get total length)
```

## L916 · `let fetch_len = total_len.min(512) as u16;`

```
// Get full Configuration Descriptor
```

## L925 · `let (kbd_iface, intr_ep, intr_max_pkt, intr_interval) =`

```
// Check for keyboard interface
```

## L930-933 · `kprintln!("[npk] xhci: port {} not a keyboard, releasing slot {}", p + 1, slot_id);`

```
// Keine Tastatur: Slot ZURUECKGEBEN. Ein liegengelassener
// Slot laesst ein spaeteres Address Device auf demselben
// Port scheitern — genau daran ist die Maus des IdeaPad
// gestorben, nachdem sie erkannt war.
```

## L942-943 · `state.port_num = p;`

```
// Tastatur gefunden. Sie behaelt den Satz, mit dem sie aufgezaehlt
// wurde; der zweite Satz bleibt unberuehrt fuer die Maus.
```

## L951 · `if !usb_set_protocol(&mut state, kbd_iface, 0) {`

```
// Set Protocol = Boot Protocol (0)
```

## L953 · `}`

```
// Non-fatal, some keyboards default to boot protocol
```

## L956 · `let _ = usb_set_idle(&mut state, kbd_iface);`

```
// Set Idle (rate=0)
```

## L959 · `let ep_dci = (intr_ep & 0x0F) * 2 + 1;`

```
// Configure Endpoint (interrupt IN)
```

## L967 · `schedule_interrupt_transfer(&mut state);`

```
// Schedule first interrupt transfer
```

## L981-994 · `let _ = connected;`

```
// Den laufenden Controller BEHALTEN.
//
// Der Zustand wurde frueher nur auf dem Erfolgspfad abgelegt, und
// `init_mouse` braucht ihn. Eine Maschine mit USB-Maus aber
// PS/2-Tastatur bekam damit gar keinen USB-Zeiger: die Tastatur fehlt,
// also `false`, also fiel der ganze hochgefahrene Controller weg —
// samt der Maus, die daran haengt. Gemeldet an einem Lenovo IdeaPad,
// dessen Tastatur am i8042 sitzt.
//
// IMMER ablegen, auch ohne Tastatur und auch ohne angeschlossenes
// Geraet: der Controller LAEUFT jetzt, und was spaeter dort eingesteckt
// wird — eine Maus, ein Netzadapter — findet nur, wer ihn kennt. Die
// frueher noetige Wette ("der erste mit Geraeten ist die bessere")
// faellt mit der Tabelle weg, jeder bekommt seinen Platz.
```

## L997 · `false // No keyboard found on any port`

```
// No keyboard found on any port
```

## L1000-1007 · `pub fn init_mouse() -> bool {`

```
/// Eine USB-Maus suchen — auf JEDEM Controller, nicht nur auf dem der
/// Tastatur.
///
/// Vorher lief das ueber genau einen Zustand, also ueber den Controller,
/// auf dem die Tastatur gefunden wurde. Steckt die Maus an einer Buchse,
/// die zu einem anderen gehoert, wurde sie nie gesucht: der Port war
/// bestromt (die Maus leuchtete), aber nie adressiert. Gemessen auf dem
/// IdeaPad, Tastatur auf 04:00.3, Maus auf 04:00.4.
```

## L1025 · `fn probe_mouse(state: &mut XhciState) -> bool {`

```
/// Der Gang auf EINEM Controller.
```

## L1030-1038 · `for p in 0..max_ports {`

```
// Kein Wiederverwendungspfad mehr.
//
// Es gab einen: das erste fremde Geraet der Tastatursuche behielt
// Slot und DMA-Satz, und die Maussuche uebernahm beides, ohne neu
// aufzuzaehlen. Das spart einen Port-Reset und war jede Zeile Aerger
// wert, die es gekostet hat — der Satz gehoerte laengst einem spaeter
// probierten Port, Slot und Ring passten nicht zusammen, und es kam
// kein Transfer zurueck. Seit die Tastatursuche aufraeumt, gibt es
// auch nichts mehr wiederzuverwenden.
```

## L1040-1058 · `for p in 0..max_ports {`

```
// Rueckfall: die uebrigen Ports FRISCH aufzaehlen — und den vorher
// angefassten Port NICHT auslassen.
//
// Er wurde ausgelassen, weil der Wiederverwendungspfad ihn schon
// bedient hatte. Der kann aber fehlschlagen, und auf Florians IdeaPad
// tut er das aus einem Grund, der hier nicht zu reparieren ist: nach
// dem gemerkten Port probiert die Tastatursuche WEITERE Ports, und die
// benutzen denselben zweiten DMA-Satz (`mouse_ep0_ring`,
// `mouse_device_ctx`). Adressiert sich einer davon, gehoert der Satz
// ihm — der gemerkte Slot und der EP0-Ring passen dann nicht mehr
// zusammen, und es kommt kein Transfer zurueck.
//
// Die Folge war, dass die EINZIGE Beruehrung der echten Maus der
// kaputte Weg war: Port 2 trug sie („composite mouse device"), und der
// Rueckfall uebersprang genau ihn. Ein frischer Anlauf setzt den Port
// zurueck und legt Slot und Ring gemeinsam neu an.
//
// Der tiefere Posten bleibt: ZWEI feste DMA-Saetze reichen fuer zwei
// Geraete, nicht fuer drei.
```

## L1060-1065 · `if state.has_keyboard && p == kbd_port { continue; }`

```
// Den Port der Tastatur auslassen — aber NUR, wenn dieser
// Controller ueberhaupt eine traegt. Ohne Tastatur steht
// `port_num` auf seinem Anfangswert 0, und damit wurde auf jedem
// tastaturlosen Controller der erste Port uebersprungen. Eine Maus
// dort waere unauffindbar gewesen, ohne dass irgendwo etwas
// gemeldet haette.
```

## L1083 · `kprintln!("[npk] xhci: mouse: resetting port {}...", port + 1);`

```
// Reset port
```

## L1094 · `let saved_slot = state.slot_id;`

```
// Save keyboard EP0 context (reuse control transfer functions)
```

## L1102-1103 · `unsafe {`

```
// Clear mouse EP0 ring + device context (may have stale data from keyboard probe)
// SAFETY: zeroing DMA memory
```

## L1111 · `state.ep0_ring = state.mouse_ep0_ring;`

```
// Switch to mouse EP0 context (fresh state)
```

## L1120 · `state.mouse_ep0_cycle = state.ep0_cycle;`

```
// Save mouse EP0 state back
```

## L1124 · `state.slot_id = saved_slot;`

```
// Restore keyboard EP0 context
```

## L1136 · `kprintln!("[npk] xhci: mouse: enable slot...");`

```
// Enable Slot for mouse
```

## L1146-1147 · `unsafe {`

```
// Set DCBAA entry for mouse slot
// SAFETY: writing to DMA array
```

## L1155 · `let max_packet = match state.port_speed {`

```
// Address Device
```

## L1170 · `kprintln!("[npk] xhci: mouse: getting config descriptor...");`

```
// Get Configuration Descriptor (9 bytes first)
```

## L1181 · `let fetch_len = total_len.min(512) as u16;`

```
// Get full Configuration Descriptor
```

## L1188 · `let (mouse_iface, intr_ep, intr_max_pkt, intr_interval) =`

```
// Parse for HID mouse interface + interrupt IN endpoint
```

## L1197 · `if !usb_set_config(state, config_val) {`

```
// Set Configuration
```

## L1203 · `if !usb_set_protocol(state, mouse_iface, 0) {`

```
// Set Protocol = Boot Protocol (0)
```

## L1205 · `}`

```
// Non-fatal
```

## L1208 · `let _ = usb_set_idle(state, mouse_iface);`

```
// Set Idle (rate=0)
```

## L1211 · `let ep_dci = (intr_ep & 0x0F) * 2 + 1;`

```
// Configure Endpoint (interrupt IN for mouse)
```

## L1215 · `let saved_intr_ring = state.intr_ring;`

```
// Configure endpoint using mouse's interrupt ring
```

## L1226 · `schedule_mouse_interrupt_transfer(state);`

```
// Schedule first interrupt transfer for mouse
```

## L1242 · `if dtype == 4 && len >= 9 {`

```
// Interface descriptor (type 4)
```

## L1247 · `in_mouse_iface = iface_class == 3 && iface_subclass == 1 && iface_protocol == 2;`

```
// HID class=3, boot subclass=1, mouse protocol=2
```

## L1254 · `if dtype == 5 && len >= 7 && in_mouse_iface {`

```
// Endpoint descriptor (type 5)
```

## L1262 · `if (ep_attr & 0x03) == 3 && (ep_addr & 0x80) != 0 {`

```
// Interrupt IN endpoint
```

## L1276 · `let buf = state.data_buf + 3072;`

```
// Mouse uses data_buf+3072 (keyboard uses data_buf+2048)
```

## L1294 · `let scroll = r8(buf, 3) as i8;`

```
// Byte 3 = scroll wheel (if present, boot protocol may not have it)
```

## L1297 · `if dx != 0 || dy != 0 || buttons != state.mouse_prev_buttons || scroll != 0 {`

```
// Only push event if something changed (movement, button, or scroll)
```

## L1299-1310 · `inject_pointer(MouseEvent { buttons, dx, dy, scroll, hscroll: 0 }, true);`

```
// Durch DIESELBE Stelle wie PS/2 und ein Treibermodul — der USB-Weg
// schob den Ring vorher selbst und schrieb die Position daneben
// fort, also genau die zwei Schritte, die zusammengehoeren.
//
// Der Zeiger wird IN den Schatten komponiert (shade::render_frame_*),
// also traegt der naechste Blit die neue Position mit — kein eigener
// MMIO-Schreibzugriff aus dem IRQ, der gegen einen laufenden Blit
// rennen und ueber schnellen Flaechen flackern koennte.
//
// `true` = der billige Weg: nur den Zeiger bewegen. `handle_mouse`
// hebt selbst auf ein volles Bild an, wenn es ein Ziehen, Klicken
// oder Rollen ist.
```

## L1316 · `fn alloc_dma(pages: usize, _name: &str) -> u64 {`

```
// === Helper functions ===
```

## L1321 · `unsafe { core::ptr::write_bytes(a as *mut u8, 0, pages * 4096); }`

```
// SAFETY: zeroing allocated DMA memory
```

## L1330 · `let deadline = crate::interrupts::ticks() + 50; // 50 ticks = 500ms at 100Hz`

```
// Tick-based timeout (500ms) — CPU-speed independent
```

## L1331 · `let deadline = crate::interrupts::ticks() + 50; // 50 ticks = 500ms at 100Hz`

```
// 50 ticks = 500ms at 100Hz
```

## L1345 · `unsafe {`

```
// SAFETY: writing to DMA-allocated memory
```

## L1357 · `unsafe {`

```
// SAFETY: reading from DMA memory
```

## L1373 · `for _ in 0..100 {`

```
// Walk extended capability list to find USB Legacy Support (ID=1)
```

## L1378-1379 · `w32(mmio, off, cap | (1 << 24));`

```
// Found USB Legacy Support capability
// Set OS Owned Semaphore (bit 24)
```

## L1381 · `let deadline = crate::interrupts::ticks() + 100;`

```
// Wait for BIOS Owned Semaphore (bit 16) to clear (1s timeout)
```

## L1387 · `let ctl_off = off + 4;`

```
// Disable SMI (clear USBLEGCTLSTS enable bits)
```

## L1389 · `w32(mmio, ctl_off, r32(mmio, ctl_off) & 0x0000_001F); // keep RO/RW1C, clear enables`

```
// keep RO/RW1C, clear enables
```

## L1409-1410 · `fn usb_get_string(state: &mut XhciState, idx: u8, out: &mut [u8]) -> usize {`

```
/// Read USB string descriptor `idx` into `out` as ASCII (best effort:
/// UTF-16LE low bytes, printable only). Returns the byte count written.
```

## L1413 · `if !usb_control_transfer(state, 0x80, USB_GET_DESCRIPTOR,`

```
// wValue = (STRING desc type 0x03 << 8) | index, wIndex = langid 0x0409.
```

## L1431-1435 · `pub fn list_devices() {`

```
/// Enumerate every USB device on every xHCI controller and print a table
/// (controller BDF, port, VID:PID, identifying class, product name). Read-only
/// probing: each device is slot-enabled, addressed, queried, then released.
/// Brings each controller up fresh, so a USB keyboard/mouse on a probed
/// controller may need re-plugging afterwards.
```

## L1470-1471 · `fn enumerate_controller(dev: pci::PciDevice) -> u32 {`

```
/// Bring up one controller, address each connected device, print its identity.
/// Returns how many devices were reported.
```

## L1485-1488 · `if sc & PORTSC_PED == 0 {`

```
// USB3 (SuperSpeed) root ports are link-trained and Enabled (PED) at
// connect; a USB2-style Port Reset on an already-enabled port confuses
// it and the following Address Device fails. Only PR-reset ports that
// aren't enabled yet (USB2 low/full/high speed).
```

## L1502 · `unsafe {`

```
// SAFETY: writing this slot's device-context pointer into the DMA DCBAA
```

## L1521 · `if !usb_get_descriptor(&mut state, DESC_DEVICE, 18) {`

```
// Device descriptor (18 bytes): VID/PID, device class, iProduct.
```

## L1533-1535 · `let (mut iclass, mut isub, mut iproto) = (0u8, 0u8, 0u8);`

```
// First interface descriptor → its class. Devices whose device-class is
// 0 (per-interface) or 0xFF (vendor) defer the real class here — most
// USB-NICs do exactly this.
```

## L1597 · `fn usb_vendor_product(vid: u16, pid: u16) -> &'static str {`

```
/// Known USB chips relevant to the driver catalog (NIC dongles first).
```

## L1612 · `let sc = r32(state.oper, off);`

```
// Linux SetPortFeature(PORT_RESET): neutral state + PR.
```

## L1616-1618 · `let deadline = crate::interrupts::ticks() + 50;`

```
// Done when PR has self-cleared and PRC is set (hub_port_wait_reset) —
// not merely when PED reads 1, which an enabled port still does in the
// instant before the controller starts the reset. 500 ms timeout.
```

## L1632 · `fn post_command(state: &mut XhciState, param: u64, status: u32, mut control: u32) {`

```
// === Command Ring operations ===
```

## L1639 · `let link_ctrl = TRB_LINK | state.cmd_cycle | (1 << 1); // Toggle Cycle`

```
// Wrap: update Link TRB cycle bit and reset enqueue
```

## L1640 · `let link_ctrl = TRB_LINK | state.cmd_cycle | (1 << 1); // Toggle Cycle`

```
// Toggle Cycle
```

## L1645 · `ring_doorbell(state, 0, 0); // HC doorbell`

```
// HC doorbell
```

## L1648-1655 · `fn wait_command_completion(state: &mut XhciState) -> Option<(u32, u32)> {`

```
/// Auf die Antwort auf einen Befehl warten — und alles, was daneben
/// hereinkommt, ZUSTELLEN.
///
/// Vorher wurde jedes fremde Ereignis kommentarlos verworfen
/// ("Consume other events"). Solange nur ein Geraet am Controller hing,
/// konnte dabei nichts verloren gehen. Haengen Tastatur, Maus und ein
/// USB-Netzgeraet daran, verschluckt jeder Befehl im Betrieb einen Bericht
/// — und weil der Transfer dann nie nachgelegt wird, verstummt das Geraet.
```

## L1685 · `let _ = wait_command_completion(state); // best effort`

```
// best effort
```

## L1686-1687 · `unsafe {`

```
// Clear DCBAA entry
// SAFETY: writing to DMA array
```

## L1698 · `unsafe { core::ptr::write_bytes(state.ep0_ring as *mut u8, 0, 4096); }`

```
// SAFETY: zeroing DMA-allocated ring memory
```

## L1709 · `unsafe { core::ptr::write_bytes(input as *mut u8, 0, 4096); }`

```
// SAFETY: writing to DMA-allocated input context
```

## L1712-1713 · `unsafe {`

```
// Input Control Context: Add Slot (bit 0) + EP0 (bit 1)
// SAFETY: writing to DMA memory
```

## L1715 · `core::ptr::write_volatile((input + 4) as *mut u32, 0x03); // Add flags at offset 4`

```
// Add flags at offset 4
```

## L1718 · `let slot_off = input + ctx as u64;`

```
// Slot Context (at input + ctx_size * 1)
```

## L1720 · `let route_speed_entries = (1 << 27) | // Context Entries = 1`

```
// Context Entries = 1
```

## L1721 · `((state.port_speed as u32) << 20); // Speed`

```
// Speed
```

## L1722 · `unsafe {`

```
// SAFETY: writing slot context
```

## L1725 · `core::ptr::write_volatile((slot_off + 4) as *mut u32, ((port + 1) as u32) << 16);`

```
// Dword 1: Root Hub Port Number (1-based)
```

## L1729 · `let ep0_off = input + (ctx * 2) as u64;`

```
// EP0 Context (at input + ctx_size * 2)
```

## L1731 · `let ep_type_mps = (EP_TYPE_CONTROL << 3) | (3 << 1); // CErr=3, EP Type=Control`

```
// CErr=3, EP Type=Control
```

## L1733 · `unsafe {`

```
// SAFETY: writing EP0 context
```

## L1735 · `core::ptr::write_volatile((ep0_off + 4) as *mut u32, ep_type_mps | mps_field);`

```
// Dword 1: CErr + EP Type
```

## L1737 · `core::ptr::write_volatile((ep0_off + 8) as *mut u32, (state.ep0_ring as u32) | 1);`

```
// Dword 2-3: TR Dequeue Pointer (with DCS=1)
```

## L1740 · `core::ptr::write_volatile((ep0_off + 16) as *mut u32, 8);`

```
// Dword 4: Average TRB Length
```

## L1744 · `let slot_field = (state.slot_id as u32) << 24;`

```
// Post Address Device Command
```

## L1753 · `fn usb_control_transfer(state: &mut XhciState, bm_request: u8, b_request: u8,`

```
// === USB Control Transfers ===
```

## L1761 · `let setup_lo = bm_request as u32 | ((b_request as u32) << 8)`

```
// Setup Stage TRB
```

## L1770 · `if w_length > 0 {`

```
// Data Stage TRB (if needed)
```

## L1777 · `let status_dir = if w_length > 0 && dir_in { 0 } else { TRB_DIR_IN };`

```
// Status Stage TRB
```

## L1782 · `if *ep0 >= NUM_TR_TRBS - 1 {`

```
// Wrap check
```

## L1790 · `ring_doorbell(state, state.slot_id as u32, 1);`

```
// Ring doorbell for slot, target EP0 (DCI=1)
```

## L1793-1795 · `let deadline = crate::interrupts::ticks() + 100;`

```
// Auf die Antwort warten (1 s) — und dabei zustellen, was anderen
// gehoert. Erkannt wird das eigene Ereignis an der TRB-ADRESSE: sie
// liegt im EP0-Ring DIESES Geraets.
```

## L1812-1815 · `return ok;`

```
// Gehoert keinem Geraet, das wir kennen. Frueher galt JEDES
// Transferereignis als das eigene; diesen Fall so zu lassen ist
// die vorsichtige Wahl — sonst liefe ein Aufbau, dessen Ereignis
// wir nicht zuordnen koennen, in den Zeitablauf.
```

## L1821 · `unsafe { core::ptr::write_bytes(state.data_buf as *mut u8, 0, length as usize); }`

```
// SAFETY: zeroing DMA buffer
```

## L1838 · `fn find_keyboard_endpoint(state: &XhciState, total_len: usize) -> Option<(u8, u8, u16, u8)> {`

```
// === Descriptor parsing ===
```

## L1848-1849 · `while pos + 1 < total_len {`

```
// Single pass: collect keyboard endpoint AND check for mouse interface.
// If both exist, it's a composite mouse device — skip its keyboard interface.
```

## L1855 · `if dtype == 4 && len >= 9 {`

```
// Interface descriptor (type 4)
```

## L1860 · `if iface_class == 3 && iface_subclass == 1 && iface_protocol == 2 {`

```
// Track mouse interface (composite device detection)
```

## L1864 · `in_kbd_iface = iface_class == 3 && iface_subclass == 1 && iface_protocol == 1;`

```
// HID class=3, boot subclass=1, keyboard protocol=1
```

## L1871 · `if dtype == 5 && len >= 7 && in_kbd_iface && kbd_result.is_none() {`

```
// Endpoint descriptor (type 5)
```

## L1879 · `if (ep_attr & 0x03) == 3 && (ep_addr & 0x80) != 0 {`

```
// Interrupt IN endpoint
```

## L1888-1889 · `if has_mouse_iface {`

```
// Composite mouse device (has both mouse + keyboard interfaces) — skip.
// The keyboard interface is likely media keys, not the real keyboard.
```

## L1897 · `fn cmd_configure_endpoint(state: &mut XhciState, ep_dci: u8, max_pkt: u16, interval: u8) -> bool {`

```
// === Configure Interrupt Endpoint ===
```

## L1903 · `unsafe { core::ptr::write_bytes(input as *mut u8, 0, 4096); }`

```
// SAFETY: zeroing input context
```

## L1906-1907 · `unsafe {`

```
// Input Control Context: Add Slot (bit 0) + the endpoint (bit ep_dci)
// SAFETY: writing to DMA memory
```

## L1912 · `let slot_off = input + ctx as u64;`

```
// Slot Context: Context Entries = last valid endpoint index = ep_dci
```

## L1915 · `unsafe {`

```
// SAFETY: writing slot context
```

## L1920 · `let ep_off = input + (ctx * (ep_dci as usize + 1)) as u64;`

```
// Endpoint Context (at input + ctx_size * (ep_dci + 1))
```

## L1923 · `let xhci_interval = match state.port_speed {`

```
// Compute interval for xHCI (different from USB bInterval)
```

## L1929 · `let mut val = 0u8;`

```
// FS/LS: convert ms to 125us frames
```

## L1937-1939 · `unsafe {`

```
// Dword 0: Interval + mult=0 + LSA=0
// Dword 1: CErr=3, EP Type=Interrupt IN (7), MaxPacketSize
// SAFETY: writing endpoint context
```

## L1944 · `core::ptr::write_volatile((ep_off + 8) as *mut u32, (state.intr_ring as u32) | 1);`

```
// TR Dequeue Pointer with DCS=1
```

## L1947 · `core::ptr::write_volatile((ep_off + 16) as *mut u32, 8);`

```
// Average TRB Length
```

## L1959-1968 · `const EP_TYPE_BULK_OUT: u32 = 2;`

```
// === USB-NIC support: bulk endpoints (for the RTL8153 USB-Ethernet driver) ===
//
// A USB-NIC needs bulk IN (RX) + bulk OUT (TX) on top of the control endpoint
// the rest of this file already drives. We bring up the NIC's controller fresh
// (it lives on a different controller than the keyboard), address the device,
// configure its two bulk endpoints, and expose three primitives the class
// driver (rtl8153.rs) builds on: nic_control (register access via EP0),
// nic_bulk_out (TX), nic_bulk_in (RX poll). This is the USB *transport*; the
// chip logic lives in the class driver — the same split a future WASM driver
// would use via npk_usb_*.
```

## L1972 · `const NIC_BULK_BUF_PAGES: usize = 4;            // 16 KiB == RTL8153 rx_buf_sz`

```
// 16 KiB == RTL8153 rx_buf_sz
```

## L1974-1988 · `const NIC_RX_BUFS: usize = 128;`

```
// Bulk-IN buffers kept simultaneously in flight so the device always has a
// buffer to DMA into (Linux r8152 RTL8152_MAX_RX). With a single buffer the
// endpoint sits unarmed between each completion and the host's re-arm — at
// gigabit the chip's RX FIFO overflows in that gap and TCP collapses to a few
// Mbit. The bulk-IN TR ring uses slots 0..NIC_RX_BUFS with the link at slot
// NIC_RX_BUFS; producer + consumer walk it in lockstep so cycle bits line up.
// 128 deep (2 MiB) absorbs a gigabit burst (~16 ms) while the single-core stack
// drains+re-arms it. HW tally proved the loss root: rx_missed (chip RX FIFO
// overflow) ~4.6% on a 1 Gbit wire behind ~480-Mbit USB — the chip fills the
// ring faster than our per-packet TCP processing re-arms it, starves for armed
// buffers, and overflows its tiny internal FIFO. Since TCP settles at the USB
// bottleneck rate, only transient ramp bursts overflow; a deep ring absorbs them
// (re-arm catches up after the burst) without throttling the wire. 16→128 was
// the fix (8→16 in v0.219.42 only covered ~2 ms). Bounded by the 256-entry
// event ring + 256-TRB (one-page) bulk-IN ring, so ≤ ~255.
```

## L1990-1995 · `const NIC_TX_BUFS: usize = 16;`

```
// Async bulk-OUT (TX): a small ring of TX buffers so a frame can be posted and
// the call returns WITHOUT busy-waiting ~22 µs for the USB completion (the
// dominant per-packet cost — every ACK/dup-ACK was paying it inline in the IP
// stack). Completions are reaped lazily; buffers are reused round-robin and a
// buffer is never overwritten while in flight (tx_inflight < NIC_TX_BUFS).
// 2 KiB each covers MTU(1514)+tx_desc(8); 16 deep covers ACK bursts.
```

## L1999-2000 · `struct NicRings {`

```
/// Was NUR dem Netzgeraet gehoert. Der Controller darum herum ist
/// `XhciState` und wird geteilt.
```

## L2002 · `slot_id: u8,`

```
/// Slot und Port des Geraets — es ist eines unter mehreren.
```

## L2006 · `ep0_cycle: u32,`

```
/// Eigener EP0-Stand (Steuertransfers im Betrieb: Link-Status).
```

## L2011 · `in_buf: u64,   out_buf: u64,       // out_buf = base of NIC_TX_BUFS TX buffers`

```
// out_buf = base of NIC_TX_BUFS TX buffers
```

## L2012 · `tx_inflight: usize,                // posted-but-not-completed TX`

```
// posted-but-not-completed TX
```

## L2013 · `tx_next: usize,                    // round-robin TX buffer index`

```
// round-robin TX buffer index
```

## L2014 · `rx_armed: usize,                   // bulk-IN TRBs currently device-owned`

```
// bulk-IN TRBs currently device-owned
```

## L2015 · `trb_buf: [usize; NIC_RX_BUFS],     // bulk-IN ring slot -> buffer index`

```
// bulk-IN ring slot -> buffer index
```

## L2016 · `rx_done_buf: [usize; NIC_RX_BUFS], // FIFO of completed buffers...`

```
// FIFO of completed buffers...
```

## L2017 · `rx_done_len: [usize; NIC_RX_BUFS], // ...and their byte counts`

```
// ...and their byte counts
```

## L2023-2025 · `static NIC_RX_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
// USB-transport-layer profiling (read + reset via nic_take_stats). Lets a
// speed test see whether the bottleneck is the bulk RX path (few bytes /
// many empty polls / shallow ring) or above it (TCP/ACK/poll cadence).
```

## L2027 · `static NIC_RX_DELIV: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // calls returning data`

```
// calls returning data
```

## L2028-2039 · `static NIC_CC_OK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
// **Warum kommen nur ~23 Vollzuege je Sekunde?** Der Ring ist voll (127
// armiert), der Chip laeuft nicht ueber (rx_missed = 0), und trotzdem
// meldet der Controller kaum Transfers fertig. Diese vier Zaehler trennen
// die Faelle, die bisher alle gleich aussahen:
//
//   CC_SUCCESS      der Puffer wurde GANZ gefuellt (16 KiB)
//   CC_SHORT_PACKET der Normalfall — der Chip hatte weniger und schloss ab
//   sonstige cc     ein Fehler, den wir bisher stumm neu armiert haben
//   residual        wieviel vom Puffer LEER blieb, aufsummiert
//
// Ein Uebergewicht von SHORT mit riesigem Rest heisst: der Chip schickt
// winzige Haeppchen und wir zahlen je Haeppchen einen ganzen Transfer.
```

## L2046 · `pub fn nic_take_cc() -> (u64, u64, u64, u64, u64) {`

```
/// (ok, short, other, letzter_fremder_cc, residual_summe) — jeweils genullt.
```

## L2053 · `static NIC_RX_EMPTY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // calls returning 0`

```
// calls returning 0
```

## L2054 · `static NIC_RX_ARMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // sum of in-flight depth at`

```
// sum of in-flight depth at delivery
```

## L2056 · `static NIC_TX_CYC:   core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // cycles spent waiting for `

```
// cycles spent waiting for TX completion
```

## L2058-2061 · `fn with_nic<R>(f: impl FnOnce(&mut XhciState) -> R) -> Option<R> {`

```
/// Negotiated USB link speed of the NIC: "SuperSpeed" / "High-Speed" /
/// "Full-Speed" / "Low-Speed". Full/Low would cap an Ethernet dongle far below
/// gigabit (Full-Speed = 12 Mbit), so this rules that enumeration fault in/out.
/// Auf dem Controller arbeiten, der das Netzgeraet traegt.
```

## L2072-2077 · `fn with_nic_ep0<R>(state: &mut XhciState, f: impl FnOnce(&mut XhciState) -> R) -> R {`

```
/// Den EP0-Satz auf das NETZGERAET stellen, `f` laufen lassen, zurueck.
///
/// `usb_control_transfer` arbeitet auf dem Satz, der gerade im Zustand
/// steht, und das ist der der Tastatur. Die Maus macht es beim Aufzaehlen
/// seit je so; das Netzgeraet braucht es im BETRIEB, weil `rtl8153` den
/// Link-Status ueber Steuertransfers liest.
```

## L2113 · `pub fn nic_take_stats() -> (u64, u64, u64, u64, u64, u64) {`

```
/// (rx_bytes, rx_deliveries, rx_empty_polls, sum_ring_depth, tx_calls, tx_wait_cycles), each reset to 0.
```

## L2126-2131 · `pub fn scan_all_controllers() {`

```
/// Read-only scan of EVERY xHCI controller in the system: PCI id, port count,
/// and each connected/USB3 port's protocol + link state. Finds whether USB-C
/// SuperSpeed routes through a SEPARATE controller (e.g. a Thunderbolt/Titan
/// Ridge xHCI we never bring up) — the dongle's USB2 half lands on the PCH xHCI,
/// but its SS half could be on a controller that's dark. Mem-decode is enabled
/// so BAR reads work; nothing is reset, halted, or addressed.
```

## L2157 · `let tb = vid == 0x8086 && matches!(did, 0x1574 | 0x15b5 | 0x15b6 | 0x15c1 | 0x15d4`

```
// Thunderbolt/USB4 host xHCIs are the usual hiding place for USB-C SS.
```

## L2165 · `pci::write32(addr, 0x04, (cmd | 0x02) as u32); // enable memory-space decode`

```
// enable memory-space decode
```

## L2189 · `if !ccs && ver != 3 { continue; } // show every USB3 port even if empty`

```
// show every USB3 port even if empty
```

## L2201-2207 · `pub fn tb_warm_reset() {`

```
/// Warm-reset the USB3 ports of every Thunderbolt/Titan Ridge xHCI and report
/// the before/after link state. A USB3 link stuck in Disabled/Inactive (PLS=4/6)
/// — exactly where the dongle's SS lanes sit on TB port 4 (CSC seen, link
/// Disabled) — only recovers via a WARM reset (WPR, bit 31); the normal port
/// path issues a hot reset (PR, bit 4) which can't revive a Disabled link. If a
/// port reaches Enabled (PED=1) at speed=4 afterwards, SuperSpeed trained and the
/// dongle is attachable on this controller. Touches only TB-host USB3 ports.
```

## L2259 · `w32(oper, off, port_neutral(before) | PORTSC_PP | PORTSC_WPR);`

```
// Issue warm reset: preserve PP, set WPR (RW1S, self-clearing).
```

## L2261 · `let deadline = crate::interrupts::ticks() + 50;`

```
// Wait up to ~500ms for the reset to complete (WRC or PRC), polling ticks.
```

## L2269 · `let sc = r32(oper, off);`

```
// Clear all change bits.
```

## L2273 · `let s2 = crate::interrupts::ticks() + 10;`

```
// Settle, then read the resulting state.
```

## L2289-2291 · `pub fn nic_dump_ports() {`

```
/// On-demand re-dump of the NIC controller's root ports + negotiated link speed.
/// The boot-time scan scrolls past fast; this lets the `nic` command print the
/// same USB2/USB3 protocol + ccs/speed table whenever asked.
```

## L2309-2310 · `pub fn nic_speed_class() -> u8 {`

```
/// USB link speed of the attached NIC as an r8152 coalesce class:
/// 2 = SuperSpeed, 1 = High, 0 = Full/other. Picks the RX aggregation timeout.
```

## L2319-2322 · `fn port_usb_version(mmio: u64, port0: u32) -> u8 {`

```
/// xHCI major USB revision of root port `port0` (0-based) from the Supported
/// Protocol extended capability (ID=2): 3 = USB3/SuperSpeed-capable, 2 = USB2,
/// 0 = unknown. The same physical USB-C jack appears as two logical ports — one
/// USB2, one USB3 — so this is how we tell whether a SuperSpeed port even exists.
```

## L2326 · `let port_num = port0 + 1; // Supported-Protocol port offset is 1-based`

```
// Supported-Protocol port offset is 1-based
```

## L2333 · `let cpo = d2 & 0xFF;          // compatible port offset (1-based)`

```
// compatible port offset (1-based)
```

## L2334 · `let cpc = (d2 >> 8) & 0xFF;   // compatible port count`

```
// compatible port count
```

## L2344-2347 · `fn dump_nic_ports(x: &XhciState) {`

```
/// Dump every populated/connected root port with its protocol + link state.
/// Tells us whether the dongle trained a SuperSpeed link (USB3 port, speed=4)
/// or fell back to the USB2 companion port (speed=3) — the difference between a
/// 480 Mbit ceiling and the 5 Gbit headroom we'd need for true gigabit.
```

## L2354 · `if !ccs && ver != 3 { continue; } // show all USB3 ports even if empty`

```
// show all USB3 ports even if empty
```

## L2366-2378 · `fn nic_probe(state: &mut XhciState, vid: u16, pid: u16, ep_in: u8, ep_out: u8) -> bool {`

```
/// Das Netzgeraet auf EINEM Controller suchen und einrichten — ohne ihn
/// zurueckzusetzen.
///
/// Das ist der ganze Punkt. Bis hierher fuhr der NIC-Weg sich den
/// Controller selbst hoch (`bring_up_controller` = anhalten + `HCRST`), und
/// damit verlor ALLES darauf seinen Slot: die Tastatur auf Florians
/// IdeaPad, und genauso ein Stick oder ein Headset. Der Controller LAEUFT
/// aber schon, seit `init()` ihn aufgezaehlt hat — es fehlt nur ein Slot
/// mehr darin.
///
/// Die Ports von Tastatur und Maus werden ausgelassen: dort sitzt ein
/// Geraet, das wir schon adressiert haben, und ein Port-Reset waere genau
/// der Schaden, den wir vermeiden wollen.
```

## L2383 · `let saved = (state.slot_id, state.ep0_ring, state.ep0_cycle,`

```
// Den eingestellten EP0-Satz merken — daran haengt die Tastatur.
```

## L2396-2397 · `unsafe {`

```
// Eigener, frischer Satz fuer dieses Geraet.
// SAFETY: DMA-Speicher dieses Controllers, kein Geraet zeigt darauf.
```

## L2411 · `unsafe {`

```
// SAFETY: Geraetekontext-Zeiger dieses Slots in der DMA-DCBAA
```

## L2425-2426 · `cmd_disable_slot(state, slot);`

```
// Nicht unseres: Slot ZURUECKGEBEN. Ein liegengelassener Slot
// laesst den naechsten Anlauf auf demselben Port scheitern.
```

## L2444 · `write_trb(in_ring, NIC_RX_BUFS, in_ring, 0, TRB_LINK | TRB_CYCLE | (1 << 1));`

```
// Bulk-IN laeuft ueber NIC_RX_BUFS Plaetze; Bulk-OUT behaelt den Ring.
```

## L2448 · `let in_dci = ep_in * 2 + 1;   // IN-Endpunkt`

```
// IN-Endpunkt
```

## L2449 · `let out_dci = ep_out * 2;     // OUT-Endpunkt`

```
// OUT-Endpunkt
```

## L2472 · `for b in 0..NIC_RX_BUFS { nic_arm_rx(state, b); }`

```
// RX-Ring vorfuellen, damit das Geraet ohne Wartezeit liefern kann.
```

## L2478 · `state.slot_id = saved.0;`

```
// Zurueckstellen, was die Tastatur braucht.
```

## L2488-2501 · `pub fn nic_attach(vid: u16, pid: u16, ep_in: u8, ep_out: u8) -> bool {`

```
/// `vid:pid` auf irgendeinem Controller finden und einrichten.
///
/// Zwei Wege, und der zweite ist der Notnagel:
///
/// 1. **Sanft** — auf jedem Controller, der SCHON LAEUFT (alle, seit
///    `init()` sie aufzaehlt). Nichts wird zurueckgesetzt, nichts verliert
///    seinen Slot.
/// 2. **Rueckfall** — ein Controller, den wir nicht fuehren, wird
///    hochgefahren (also zurueckgesetzt), abgelegt und dann GENAUSO
///    abgesucht. Eine Bauart, nicht zwei.
///
/// Ohne (2) koennte ein Controller, der beim Start nicht in die Tabelle
/// passte, das Netzgeraet fuer immer verstecken — und ohne Netz gibt es
/// kein Update zurueck.
```

## L2514 · `for bus in 0u16..=255 {`

```
// Rueckfall: ein Controller, den die Tabelle nicht kennt.
```

## L2553-2560 · `let mut addrs: [Option<pci::PciAddr>; MAX_CTRLS] = [None; MAX_CTRLS];`

```
// Rueckfall 2: die BEKANNTEN Controller, mit Reset — der alte Weg.
//
// Ohne das waere der Rueckfall wirkungslos: seit `init()` steht JEDER
// Controller in der Tabelle, also findet Rueckfall 1 nie einen. Und
// ohne Netz gibt es kein Update zurueck — auf dem IdeaPad ist der
// Dongle der einzige Weg, die WLAN-Karte (10ec:c822) hat bei uns keinen
// Treiber. Der schlechteste Ausgang muss "wie frueher" sein, nicht
// "abgeschnitten".
```

## L2605 · `fn ctrl_known(addr: pci::PciAddr) -> bool {`

```
/// Fuehrt die Tabelle diesen Controller schon?
```

## L2613 · `fn cmd_configure_bulk(state: &mut XhciState, in_dci: u8, in_ring: u64,`

```
/// Configure both bulk endpoints (IN + OUT) in one Configure Endpoint command.
```

## L2619 · `unsafe { core::ptr::write_bytes(input as *mut u8, 0, 4096); }`

```
// SAFETY: zeroing the input context
```

## L2623 · `unsafe {`

```
// SAFETY: Input Control Context — add Slot + both endpoints
```

## L2627 · `let slot_off = input + ctx as u64;`

```
// Slot context: Context Entries = highest DCI, plus speed
```

## L2638 · `unsafe {`

```
// SAFETY: endpoint context for a bulk EP
```

## L2654-2658 · `fn nic_arm_rx(state: &mut XhciState, buf_idx: usize) {`

```
/// Einen Bulk-IN-TRB auf RX-Puffer `buf_idx` scharf machen.
///
/// Der Erzeuger laeuft durch die Ringplaetze 0..NIC_RX_BUFS-1; der
/// Link-TRB auf Platz NIC_RX_BUFS wickelt ihn um und kippt das Zyklusbit,
/// genau eine Runde vor dem Verbraucher im Geraet.
```

## L2680-2682 · `pub fn nic_control(req_type: u8, request: u8, value: u16, index: u16, buf: &mut [u8], dir_in: bool) -> bool {`

```
/// EP0 control transfer for the NIC. `buf` carries the data stage (copied into
/// the controller's data_buf for OUT, read back for IN). Used for r8152
/// register access.
```

## L2687 · `unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), state.data_buf as *mut u8, len as usize); }`

```
// SAFETY: data_buf is a 4 KiB DMA page; len <= 2048
```

## L2693 · `unsafe { core::ptr::copy_nonoverlapping(state.data_buf as *const u8, buf.as_mut_ptr(), len as usize); }`

```
// SAFETY: as above
```

## L2700 · `pub fn nic_bulk_out(data: &[u8]) -> bool {`

```
/// Einen Rahmen senden. Feuern und vergessen.
```

## L2708-2710 · `drain(state);`

```
// Ereignisse abholen: gibt fertige TX-Puffer frei, nimmt RX-Completions
// entgegen — und stellt nebenbei Tastatur und Maus zu, die am SELBEN
// Ereignisring haengen. Genau dafuer gibt es nur noch eine Zustellstelle.
```

## L2713-2714 · `let full = |s: &XhciState| s.nic.as_ref().map(|n| n.tx_inflight >= NIC_TX_BUFS).unwrap_or(true);`

```
// Gegendruck: nur wenn ALLE TX-Puffer unterwegs sind, wird (begrenzt)
// gewartet — sonst ueberschrieben wir lebendes DMA.
```

## L2732-2733 · `unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, len); }`

```
// SAFETY: Platz `bidx` ist frei — tx_inflight < NIC_TX_BUFS heisst,
// der letzte Nutzer dieses Rundlauf-Platzes ist fertig.
```

## L2754 · `pub fn nic_bulk_in(buf: &mut [u8]) -> usize {`

```
/// Einen empfangenen Rahmen abholen. 0 = nichts da.
```

## L2762-2763 · `drain(state);`

```
// Alles Anstehende einsammeln — das haelt den Ereignisring frei, der
// sonst den Controller anhaelt.
```

## L2782 · `unsafe { core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), n_copy); }`

```
// SAFETY: `src` ist RX-Puffer `b` und traegt `len` empfangene Bytes.
```

## L2787 · `nic_arm_rx(state, b);   // Puffer ist frei, sobald er herauskopiert ist`

```
// Puffer ist frei, sobald er herauskopiert ist
```

## L2791 · `fn schedule_interrupt_transfer(state: &mut XhciState) {`

```
// === Interrupt Transfer (Keyboard Polling) ===
```

## L2796 · `let buf = state.data_buf + 2048;`

```
// Normal TRB: 8 bytes from data_buf+2048 (separate from control xfer buf)
```

## L2809-2818 · `pub fn msi_irq() {`

```
/// IRQ-safe: drain ALL xHCI events. Called from timer interrupt (100Hz).
/// This is the ONLY code that talks to xHCI hardware — main thread
/// only reads from software ring buffers (KEY_BUF / MOUSE_BUF).
/// The xHCI interrupt (MSI-X, stage 3b): acknowledge like Linux'
/// `xhci_irq` — USBSTS.EINT, then the interrupter's IMAN.IP, both
/// write-1-to-clear — and drain every controller's event ring. Only EINT is
/// written back, not the whole status word as Linux does: that would also
/// clear PCD and friends, which nothing here reads by interrupt yet.
/// `try_lock`: if a transfer on Core 0 holds the controllers, it drains the
/// ring itself, and the tick is the fallback.
```

## L2828-2831 · `MISSED_DRAIN.store(true, Ordering::Release);`

```
// Core 0 holds the controllers (a transfer). The ring stays
// undrained and the controller raises nothing new until it is —
// and there is no tick any more to catch it. The shell drains it on
// its next pass (`take_missed_drain`); the handler wakes it.
```

## L2838 · `pub fn take_missed_drain() -> bool {`

```
/// An interrupt could not drain the rings: the caller must.
```

## L2843-2844 · `static NEEDS_POLL: AtomicBool = AtomicBool::new(false);`

```
/// A controller came up without MSI-X: its event ring is drained only by
/// polling, and Core 0 has no tick to do it (stage 3e).
```

## L2851 · `pub fn repeat_active() -> bool {`

```
/// A USB key is held: its software repeat (`poll_keyboard`) needs the loop.
```

## L2864 · `struct Evt {`

```
/// Ein abgeholtes Ereignis vom Ereignisring.
```

## L2868-2869 · `param: u64,`

```
/// Bei einem Transferereignis: die Adresse des fertigen Transfer-TRB.
/// Damit — und nur damit — laesst sich sagen, WEM es gehoert.
```

## L2872 · `residual: u32,`

```
/// Nicht uebertragene Bytes (Transfer Event, Bits 23..0 von `status`).
```

## L2876-2880 · `fn next_event(state: &mut XhciState) -> Option<Evt> {`

```
/// Das naechste Ereignis abholen und den Cursor weiterstellen.
///
/// **Schreibt ERDP nicht.** Das tut `flush_erdp` einmal je Runde: ein
/// MMIO-Schreibzugriff kostet ~300 ns, und bei 128 Eintraegen waren das bis
/// zu 40 µs je Abholung — die Spitzen, die den USB-Drain aushungerten.
```

## L2900-2902 · `fn flush_erdp(state: &XhciState) {`

```
/// Den Ereignisring-Dequeue-Zeiger schreiben und "Event Handler Busy"
/// loeschen. Einmal je Runde, aber MINDESTENS einmal, wenn etwas abgeholt
/// wurde — sonst meldet der Controller keine weiteren Ereignisse.
```

## L2908 · `fn in_ring(a: u64, base: u64) -> bool {`

```
/// Liegt die TRB-Adresse `a` in dem Transferring, der bei `base` beginnt?
```

## L2913-2922 · `fn dispatch_transfer(state: &mut XhciState, e: &Evt) -> bool {`

```
/// Ein Transferereignis dem Geraet ZUSTELLEN, dem sein Ring gehoert.
///
/// **Die Adresse des TRB ist der Besitzausweis.** Vorher entschied ein
/// `else`: "nicht die Maus, also die Tastatur" — richtig, solange genau
/// zwei Geraete am Controller hingen, und still falsch, sobald ein drittes
/// dazukommt.
///
/// Gibt `false` zurueck, wenn das Ereignis niemandem hier gehoert (dann
/// gehoert es dem synchronen Warter, der gerade einen Steuertransfer oder
/// einen Befehl laufen hat).
```

## L2933 · `} else {`

```
// Missed Service Error — harmlos, die GUI hat die CPU belegt.
```

## L2940 · `state.has_mouse = false;`

```
// Geraet abgezogen: nicht nachlegen.
```

## L2951-2952 · `if let Some((ir, or)) = state.nic.as_ref().map(|n| (n.in_ring, n.out_ring)) {`

```
// Das Netzgeraet: seine Ringe sind die groessten, und im Betrieb
// kommen von dort die meisten Ereignisse.
```

## L2957 · `let ring_slot = ((a.wrapping_sub(ir)) / 16) as usize;`

```
// Die TRB-Adresse zurueck auf ihren Puffer rechnen.
```

## L2959-2960 · `match cc {`

```
// Den Vollzug einsortieren, BEVOR er verarbeitet wird — auch
// der Fehlerfall, der sonst stumm neu armiert wird.
```

## L2975-2979 · `if ok && n.rx_done_count < NIC_RX_BUFS {`

```
// Nur saubere Completions zustellen. Bei einem Fehler ist
// die Restlaenge keine gueltige Byteanzahl, und ein voller
// Ausgabespeicher darf den Puffer nicht verschlucken —
// in beiden Faellen sofort wieder scharf machen, wie
// Linux eine fehlerhafte URB neu einreicht.
```

## L3014 · `} else {`

```
// Missed Service Error — auch fuer die Tastatur harmlos.
```

## L3034-3039 · `fn drain(state: &mut XhciState) {`

```
/// Alle anstehenden Ereignisse abholen und zustellen.
///
/// **Eine** Runde fuer alle Rufer — vorher gab es `drain_all_events` (Timer-
/// IRQ) und `drain_events` (Hauptschleife) nebeneinander, und nur die zweite
/// zaehlte Fehler und erkannte ein abgezogenes Geraet. Welches Verhalten
/// galt, entschied, wer zuerst drankam.
```

## L3049-3052 · `dispatch_transfer(state, &e);`

```
// Ein Ereignis, das niemandem hier gehoert, ist eines, auf das
// gerade ein Steuertransfer wartet — er ist aber nicht hier,
// also ist es verwaist. Verwerfen, nicht an ein falsches Geraet
// geben.
```

## L3062-3063 · `static IS_DE_LAYOUT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);`

```
/// Gemerkte Tastaturbelegung — `config::get` allokiert, und das geht im
/// IRQ-Kontext nicht.
```

## L3066 · `pub fn cache_keyboard_layout() {`

```
/// Nach dem Laden der Konfiguration rufen.
```

## L3075-3076 · `pub fn poll_events() {`

```
/// Ereignisse aus der Hauptschleife abholen. Nur im fruehen Start noetig,
/// bevor der Timer-IRQ laeuft; danach drained der ausschliesslich.
```

## L3086 · `let shift = (modifiers & 0x22) != 0;  // L/R Shift`

```
// L/R Shift
```

## L3087 · `let ctrl = (modifiers & 0x11) != 0;   // L/R Ctrl`

```
// L/R Ctrl
```

## L3088 · `let alt_gr = (modifiers & 0x40) != 0; // Right Alt (AltGr)`

```
// Right Alt (AltGr)
```

## L3089 · `let super_held = (modifiers & 0x88) != 0; // L/R GUI (Super)`

```
// L/R GUI (Super)
```

## L3091 · `crate::keyboard::set_super(super_held);`

```
// Update shared modifier state (used by shade compositor)
```

## L3096 · `let is_de = IS_DE_LAYOUT.load(Ordering::Relaxed);`

```
// Use cached layout (IRQ-safe, no allocation)
```

## L3099 · `let first_key = keys.iter().find(|&&k| k != 0 && k != 1).copied().unwrap_or(0);`

```
// Find the first non-zero key in the current report for repeat tracking
```

## L3103 · `state.repeat_key = 0;`

```
// All keys released — stop repeat
```

## L3107 · `state.repeat_key = first_key;`

```
// Repeated key was released while another is held — follow current key
```

## L3120 · `state.repeat_key = first_key;`

```
// New key pressed — start repeat timer
```

## L3135 · `if key == 0 || key == 1 { continue; } // no key / error rollover`

```
// no key / error rollover
```

## L3136 · `if state.prev_keys.contains(&key) { continue; }`

```
// Only process newly pressed keys
```

## L3139-3142 · `if ctrl && shift && key == 0x06 {`

```
// Ctrl+Shift+C → copy the selection (terminal drag-selection or
// focused Input/TextArea). Routed as a ShadeAction so the actual
// copy runs in the main loop, not this poll/IRQ context. In a
// terminal Ctrl+C must stay SIGINT, so copy is the Shift variant.
```

## L3147-3148 · `if ctrl && shift && key == 0x19 {`

```
// Ctrl+Shift+V → paste the clipboard (terminal input line or focused
// Input/TextArea). Plain Ctrl+V still reaches widgets as 'v'+ctrl.
```

## L3153-3159 · `if ctrl && key == 0x06 {`

```
// Ctrl+C → mirror the PS/2 path (see decode_scancode): (1) terminal
// SIGINT, cancel the running foreground intent (download / OTA);
// (2) buffer control byte 0x03 so the compositor's clipboard
// intercept (handle_input_key → handle_clipboard_key) turns it into
// a copy when a text widget is focused. Without the push, USB copy
// was dead — the byte was swallowed and never reached the widget
// layer. Harmless elsewhere: loop consumes it as ^C. HID 0x06 = 'c'.
```

## L3166 · `if super_held && crate::shade::is_active() {`

```
// Mod+special keys: push shade actions directly (avoids ESC sequence race)
```

## L3175-3178 · `0x1E..=0x26 => { crate::shade::input::push_workspace_key(key - 0x1D, shift); true }`

```
// Digits 1..9 by HID code, not by character: with Shift the
// layout has already turned "1" into "+"/"!" (and de_CH's
// Shift+4 into nothing at all), so Mod+Shift+N never reached
// the character keybinds. HID 0x1E = '1'.
```

## L3185 · `match key {`

```
// Arrow keys and special multi-byte sequences (when mod NOT held)
```

## L3187 · `0x4F => { push_key(0x1B); push_key(b'['); push_key(b'C'); continue; } // Right`

```
// Right
```

## L3188 · `0x50 => { push_key(0x1B); push_key(b'['); push_key(b'D'); continue; } // Left`

```
// Left
```

## L3189 · `0x51 => { push_key(0x1B); push_key(b'['); push_key(b'B'); continue; } // Down`

```
// Down
```

## L3190 · `0x52 => { push_key(0x1B); push_key(b'['); push_key(b'A'); continue; } // Up`

```
// Up
```

## L3191 · `0x4A => { push_key(0x1B); push_key(b'['); push_key(b'H'); continue; } // Home`

```
// Home
```

## L3192 · `0x4D => { push_key(0x1B); push_key(b'['); push_key(b'F'); continue; } // End`

```
// End
```

## L3193 · `0x4B => { push_key(0x1B); push_key(b'['); push_key(b'5'); continue; } // PgUp`

```
// PgUp
```

## L3194 · `0x4E => { push_key(0x1B); push_key(b'['); push_key(b'6'); continue; } // PgDn`

```
// PgDn
```

## L3201-3202 · `while let Some(b) = take_tail() { push_key(b) }`

```
// Ein Zeichen ausserhalb von ASCII besteht aus mehreren Bytes;
// sie muessen DIREKT hintereinander in den Ring.
```

## L3208-3210 · `static UTF8_TAIL: spin::Mutex<crate::input::Utf8Tail> =`

```
/// Der Rest einer UTF-8-Folge — siehe `input::Utf8Tail`. Eigene Instanz,
/// weil dieser Treiber ein eigener Erzeuger ist; die Rechnung ist dieselbe
/// wie im PS/2-Treiber.
```

## L3214-3217 · `fn take_tail() -> Option<u8> {`

```
/// `UTF8_TAIL` is taken by the timer ISR (`poll_events_irq` →
/// `process_hid_report`) AND by `poll_keyboard` on Core 0 with IF=1 — so
/// the non-ISR side must mask interrupts, or a tick inside the lock spins
/// forever on it.
```

## L3222-3223 · `fn hid_to_char(key: u8, shift: bool, alt_gr: bool, is_de: bool) -> u8 {`

```
/// HID-Code → erstes Byte des Zeichens; der Rest wandert in `UTF8_TAIL`.
/// 0 heisst „diese Taste traegt kein Zeichen".
```

## L3230 · `fn hid_to_char_ch(key: u8, shift: bool, alt_gr: bool, is_de: bool) -> char {`

```
/// Convert HID keycode to a character. `'\0'` for unhandled keys.
```

## L3232 · `if alt_gr && is_de {`

```
// AltGr: special characters (de_CH)
```

## L3239-3242 · `if is_de && key == 0x64 {`

```
// ISO-Extra key (102-key layout, left of Z): HID 0x64. Plain →
// `<`, Shift → `>`, AltGr → `\` (latter via altgr_char_de_hid
// above). The key is outside the 57-entry layout arrays so it
// gets handled here before the index check.
```

## L3246-3249 · `if !is_de && key == 0x64 {`

```
// US layout 102-key keyboards also have a non-US `\` key at
// HID 0x64 — map plain to `\` (no shift). Most US users won't
// hit this but it stops the key from being silent if they have
// an EU-shape keyboard plugged in.
```

## L3259-3260 · `(if shift { HID_TO_ASCII_SHIFT[key as usize] }`

```
// Die US-Tabellen bleiben Bytes — sie SIND ASCII, jedes Zeichen
// darin passt in eins.
```

## L3270 · `0x58 => '\n', // Numpad Enter`

```
// Numpad Enter
```

## L3282 · `0x4C => '\u{7F}', // Delete`

```
// Delete
```

## L3283 · `_ => '\0',`

```
// Arrow keys are multi-byte — handled separately, not via repeat
```

## L3289-3290 · `fn altgr_char_de_hid(key: u8) -> Option<char> {`

```
/// AltGr characters for Swiss German (de_CH) keyboard layout.
/// HID usage codes → ASCII.
```

## L3293 · `0x08 => Some('€'),   // AltGr+e — xkb: AD03 dritte Ebene EuroSign`

```
// AltGr+e — xkb: AD03 dritte Ebene EuroSign
```

## L3294 · `0x1F => Some('@'),   // AltGr+2`

```
// AltGr+2
```

## L3295 · `0x20 => Some('#'),   // AltGr+3`

```
// AltGr+3
```

## L3296 · `0x24 => Some('|'),   // AltGr+7`

```
// AltGr+7
```

## L3297 · `0x2E => Some('~'),   // AltGr+^ (= key)`

```
// AltGr+^ (= key)
```

## L3298 · `0x2F => Some('['),   // AltGr+ü ([ key)`

```
// AltGr+ü ([ key)
```

## L3299 · `0x30 => Some(']'),   // AltGr+¨ (] key)`

```
// AltGr+¨ (] key)
```

## L3300 · `0x34 => Some('{'),   // AltGr+ä (' key)`

```
// AltGr+ä (' key)
```

## L3301 · `0x31 => Some('}'),   // AltGr+$ (\ key)`

```
// AltGr+$ (\ key)
```

## L3302 · `0x64 => Some('\\'),  // AltGr+< (non-US \)`

```
// AltGr+< (non-US \)
```

