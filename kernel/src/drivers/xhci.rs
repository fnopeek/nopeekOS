//! xHCI USB Host Controller Driver
//!
//! Hubs after Linux hub.c, HID devices after usbhid (keyboards, mice,
//! receivers that are both) and one USB network device. Every device
//! enumeration meets stays addressed with its descriptors until it is
//! unplugged (`devices`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{kprintln, pci};
use crate::hw::{DmaRegion, Mmio};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering, fence};

// === Capability register offsets (from BAR0) ===
const CAP_CAPLENGTH:  u32 = 0x00;
const CAP_HCSPARAMS1: u32 = 0x04;
const CAP_HCSPARAMS2: u32 = 0x08;
const CAP_HCCPARAMS1: u32 = 0x10;
const CAP_DBOFF:      u32 = 0x14;
const CAP_RTSOFF:     u32 = 0x18;

/// Bytes of BAR0 mapped; covers capability, operational, runtime and
/// doorbell registers.
const BAR0_MAP: u64 = 64 * 1024;

// Interrupter 0 register set (from the runtime base), 32 bytes.
const IR0:    u32 = 0x20;
const IR_LEN: u64 = 0x20;

// === Operational register offsets (from oper_base) ===
const OP_USBCMD:  u32 = 0x00;
const OP_USBSTS:  u32 = 0x04;
#[allow(dead_code)]
const OP_DNCTRL:  u32 = 0x14;
const OP_CRCR:    u32 = 0x18;
const OP_DCBAAP:  u32 = 0x30;
const OP_CONFIG:  u32 = 0x38;

// USBCMD bits
const CMD_RUN:  u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const CMD_INTE: u32 = 1 << 2; // interrupter enable

// USBSTS bits
const STS_HCH: u32 = 1 << 0;  // HC Halted
const STS_EINT: u32 = 1 << 3; // event interrupt (write 1 to clear)
const STS_CNR: u32 = 1 << 11; // Controller Not Ready

// PORTSC bits
const PORTSC_CCS:   u32 = 1 << 0;  // Current Connect Status
const PORTSC_PED:   u32 = 1 << 1;  // Port Enabled
const PORTSC_PR:    u32 = 1 << 4;  // Port Reset (hot — USB2)
const PORTSC_WPR:   u32 = 1 << 31; // Warm Port Reset (USB3 link recovery)
#[allow(dead_code)]
const PORTSC_PLS_MASK: u32 = 0xF << 5; // Port Link State
const PORTSC_PP:    u32 = 1 << 9;  // Port Power
#[allow(dead_code)]
const PORTSC_SPEED_MASK: u32 = 0xF << 10;
const PORTSC_PRC:   u32 = 1 << 21; // Port Reset Change
const PORTSC_CSC:   u32 = 1 << 17; // Connect Status Change
const PORTSC_PEC:   u32 = 1 << 18; // Port Enabled Change
const PORTSC_WRC:   u32 = 1 << 19; // Warm Port Reset Change
const PORTSC_PLC:   u32 = 1 << 22; // Port Link State Change
/// Read-only bits and RW state bits — Linux `XHCI_PORT_RO` / `XHCI_PORT_RWS`.
const PORTSC_RO: u32 = (1 << 0) | (1 << 3) | (0xF << 10) | (1 << 30);
const PORTSC_RWS: u32 = (0xF << 5) | (1 << 9) | (0x3 << 14) | (0x7 << 25);

/// PORTSC value that changes nothing when written back — Linux
/// `xhci_port_state_to_neutral`. Masking only the RW1C change bits is not
/// enough: PED is RW1CS too, and a 1 written back DISABLES an enabled port.
/// A second reset of an already enabled port then never completes.
fn port_neutral(sc: u32) -> u32 {
    sc & (PORTSC_RO | PORTSC_RWS)
}

// Port speeds
const SPEED_FULL:  u32 = 1;
const SPEED_LOW:   u32 = 2;
const SPEED_HIGH:  u32 = 3;
const SPEED_SUPER: u32 = 4;

// TRB types (in control field bits [15:10])
const TRB_NORMAL:         u32 = 1 << 10;
const TRB_SETUP_STAGE:    u32 = 2 << 10;
const TRB_DATA_STAGE:     u32 = 3 << 10;
const TRB_STATUS_STAGE:   u32 = 4 << 10;
const TRB_LINK:           u32 = 6 << 10;
const TRB_TR_NOOP:        u32 = 8 << 10;
const TRB_ENABLE_SLOT:    u32 = 9 << 10;
#[allow(dead_code)]
const TRB_DISABLE_SLOT:   u32 = 10 << 10;
const TRB_ADDRESS_DEVICE: u32 = 11 << 10;
const TRB_CONFIGURE_EP:   u32 = 12 << 10;
const TRB_EVALUATE_CTX:   u32 = 13 << 10;
#[allow(dead_code)]
const TRB_NOOP_CMD:       u32 = 23 << 10;

// TRB control bits
const TRB_CYCLE:     u32 = 1 << 0;
const TRB_IOC:       u32 = 1 << 5;  // Interrupt On Completion
const TRB_IDT:       u32 = 1 << 6;  // Immediate Data
#[allow(dead_code)]
const TRB_BSR:       u32 = 1 << 9;  // Block Set Address Request (address device)
const TRB_DIR_IN:    u32 = 1 << 16; // Direction: IN
const TRB_TRT_NO:    u32 = 0;       // Transfer Type: No Data
const TRB_TRT_IN:    u32 = 3 << 16; // Transfer Type: IN Data

// Event TRB types (bits [15:10] of control)
const EVT_TRANSFER:      u32 = 32 << 10;
const EVT_CMD_COMPLETE:  u32 = 33 << 10;
const EVT_PORT_STATUS:   u32 = 34 << 10;

// Completion codes
const CC_SUCCESS:        u32 = 1;
const CC_SHORT_PACKET:   u32 = 13;

// Endpoint types in endpoint context
const EP_TYPE_CONTROL:      u32 = 4;
const EP_TYPE_INTERRUPT_OUT: u32 = 3;
const EP_TYPE_INTERRUPT_IN:  u32 = 7;

// USB request types
const USB_GET_DESCRIPTOR: u8 = 6;
const USB_SET_CONFIG:     u8 = 9;

// Descriptor types
#[allow(dead_code)]
const DESC_DEVICE:        u16 = 0x0100;
const DESC_CONFIG:        u16 = 0x0200;

const USB_GET_STATUS:     u8 = 0;
const USB_CLEAR_FEATURE:  u8 = 1;
const USB_SET_FEATURE:    u8 = 3;
const USB_SET_INTERFACE:  u8 = 11;

const NUM_CMD_TRBS: usize = 32;
const NUM_EVT_TRBS: usize = 256;
const NUM_TR_TRBS:  usize = 64;

// HID usage code to ASCII table (keyboard page, US layout base)
static HID_TO_ASCII: [u8; 57] = [
    0, 0, 0, 0,                                       // 0x00-0x03
    b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h',  // 0x04-0x0B
    b'i', b'j', b'k', b'l', b'm', b'n', b'o', b'p',  // 0x0C-0x13
    b'q', b'r', b's', b't', b'u', b'v', b'w', b'x',  // 0x14-0x1B
    b'y', b'z',                                        // 0x1C-0x1D
    b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', // 0x1E-0x27
    b'\n', 0x1B, 0x08, b'\t', b' ',                    // 0x28-0x2C (enter,esc,bs,tab,space)
    b'-', b'=', b'[', b']', b'\\',                     // 0x2D-0x31
    0, b';', b'\'', b'`', b',', b'.', b'/',            // 0x32-0x38
];

static HID_TO_ASCII_SHIFT: [u8; 57] = [
    0, 0, 0, 0,
    b'A', b'B', b'C', b'D', b'E', b'F', b'G', b'H',
    b'I', b'J', b'K', b'L', b'M', b'N', b'O', b'P',
    b'Q', b'R', b'S', b'T', b'U', b'V', b'W', b'X',
    b'Y', b'Z',
    b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')',
    b'\n', 0x1B, 0x08, b'\t', b' ',
    b'_', b'+', b'{', b'}', b'|',
    0, b':', b'"', b'~', b'<', b'>', b'?',
];

// Swiss German layout: remap HID usage codes (z/y swap, number row,
// special chars). HID 0x64 (non-US `<`/`>`) lies outside this 57-entry
// range and is special-cased in `hid_to_char`.
// Source: `/usr/share/X11/xkb/symbols/ch`, `xkb_symbols "basic"`.
// The PS/2 driver has the same table; keep both in sync.
// `tools/kbcheck.py` checks both against the xkb reference.
static HID_TO_ASCII_DE: [char; 57] = [
    '\0','\0','\0','\0',
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h',
    'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p',
    'q', 'r', 's', 't', 'u', 'v', 'w', 'x',
    'z', 'y',                                        // z/y swapped
    '1', '2', '3', '4', '5', '6', '7', '8', '9', '0',
    '\n','\u{1B}','\u{8}','\t',' ',
    // 0x2D..0x31: AE11 AE12 AD11 AD12 BKSL
    '\'','^', 'ü', '¨', '$',
    // 0x32..0x38: (unused) AC10 AC11 TLDE AB08 AB09 AB10
    '\0','ö', 'ä', '§', ',', '.', '-',
];

static HID_TO_ASCII_DE_SHIFT: [char; 57] = [
    '\0','\0','\0','\0',
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H',
    'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P',
    'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X',
    'Z', 'Y',
    '+', '"', '*', 'ç','%', '&', '/', '(', ')', '=',   // AE04 Shift = ç
    '\n','\u{1B}','\u{8}','\t',' ',
    '?', '`', 'è', '!', '£',                           // AD11 è · AD12 ! · BKSL £
    '\0','é', 'à', '°', ';', ':', '_',                  // AC10 é · AC11 à · TLDE °
];

// Key buffer — IRQ-safe SPSC ring (producer: IRQ/poll, consumer: main thread)
const KEY_BUF_SIZE: usize = 32;
static KEY_BUF: [AtomicU8; KEY_BUF_SIZE] = [const { AtomicU8::new(0) }; KEY_BUF_SIZE];
static KEY_HEAD: AtomicUsize = AtomicUsize::new(0);
static KEY_TAIL: AtomicUsize = AtomicUsize::new(0);

fn push_key(k: u8) {
    let head = KEY_HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % KEY_BUF_SIZE;
    if next != KEY_TAIL.load(Ordering::Acquire) {
        KEY_BUF[head].store(k, Ordering::Relaxed);
        KEY_HEAD.store(next, Ordering::Release);
    }
}

/// A pointer event: from a USB HID device, PS/2 or a driver module.
#[derive(Clone, Copy)]
pub struct MouseEvent {
    pub buttons: u8,  // bit 0=left, 1=right, 2=middle
    pub dx: i8,
    pub dy: i8,
    pub scroll: i8,
    /// Horizontal scroll (AC Pan), positive = right.
    ///
    /// A separate axis because a touchpad reports both in the same event.
    /// A USB mouse fills it from AC Pan; one without leaves it 0.
    pub hscroll: i8,
}

// Mouse event buffer — IRQ-safe SPSC ring (producer: IRQ/poll, consumer: main thread)
const MOUSE_BUF_SIZE: usize = 128;
/// Slots hold `MouseEvent::pack`; the head/tail pair orders them.
static MOUSE_BUF: [AtomicU64; MOUSE_BUF_SIZE] = [const { AtomicU64::new(0) }; MOUSE_BUF_SIZE];

impl MouseEvent {
    fn pack(self) -> u64 {
        u64::from_le_bytes([self.buttons, self.dx as u8, self.dy as u8,
            self.scroll as u8, self.hscroll as u8, 0, 0, 0])
    }

    fn unpack(v: u64) -> Self {
        let b = v.to_le_bytes();
        MouseEvent { buttons: b[0], dx: b[1] as i8, dy: b[2] as i8, scroll: b[3] as i8, hscroll: b[4] as i8 }
    }
}
static MOUSE_HEAD: AtomicUsize = AtomicUsize::new(0);
static MOUSE_TAIL: AtomicUsize = AtomicUsize::new(0);

/// Serializes all pointer producers: PS/2 from the timer IRQ, USB from the
/// drain (which also runs from the net path on a worker core), and WASM
/// drivers (touchpad) from any core.
static POINTER_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// Push into the ring. Caller holds `POINTER_LOCK`.
fn push_mouse_locked(evt: MouseEvent) {
    let head = MOUSE_HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % MOUSE_BUF_SIZE;
    if next != MOUSE_TAIL.load(Ordering::Acquire) {
        MOUSE_BUF[head].store(evt.pack(), Ordering::Relaxed);
        MOUSE_HEAD.store(next, Ordering::Release);
    }
}

/// Inject a pointer event from any source.
///
/// The ring push and `cursor::update_atomic` are both read-modify-write and
/// must happen together: the latter is a sequence of atomics that carries
/// the previous button state, so interleaving would lose or invent a click.
///
/// Interrupts on this core are off while the lock is held, otherwise its own
/// timer IRQ could spin on the lock it holds.
///
/// `cheap` picks the repaint path: a USB mouse only moves the cursor, a PS/2
/// or driver event requests a full frame. Both run outside the lock.
fn inject_pointer(evt: MouseEvent, cheap: bool) {
    crate::interrupts::without_interrupts(|| {
        let _g = POINTER_LOCK.lock();
        MOUSE_AVAILABLE.store(true, Ordering::Relaxed);
        crate::shade::cursor::update_atomic(evt.dx, evt.dy, evt.buttons);
        push_mouse_locked(evt);
    });
    if cheap {
        crate::shade::request_cursor_move();
    } else {
        crate::shade::request_render();
    }
}

/// Inject a mouse event from another input source (e.g. the PS/2 touchpad on
/// the i8042 aux port) into the same ring `poll_mouse` drains, so the main
/// loop's existing `poll_mouse()` delivers it with no extra plumbing. Marks the
/// pointer available on first event.
pub fn inject_mouse(evt: MouseEvent) {
    inject_pointer(evt, false);
}

/// Poll for a key from the USB keyboard. Called from keyboard.rs.
/// Reads from software buffer only — timer IRQ drains hardware.
pub fn poll_keyboard() -> Option<u8> {
    if !AVAILABLE.load(Ordering::Relaxed) { return None; }

    // The rest of a repeated escape sequence goes before anything newer.
    if let Some(b) = crate::interrupts::without_interrupts(|| REPEAT_SEQ.lock().pop()) {
        return Some(b);
    }

    // Read from software key buffer (filled by timer IRQ)
    let head = KEY_HEAD.load(Ordering::Acquire);
    let tail = KEY_TAIL.load(Ordering::Relaxed);
    if head != tail {
        let k = KEY_BUF[tail].load(Ordering::Relaxed);
        KEY_TAIL.store((tail + 1) % KEY_BUF_SIZE, Ordering::Release);
        return Some(k);
    }

    // Timer-based key repeat (lock-free: read repeat state from atomics)
    let rk = REPEAT_KEY.load(Ordering::Relaxed);
    // A pending second half of a character goes first, or it is lost.
    if let Some(b) = take_tail() { return Some(b) }
    if rk != 0 {
        let now = crate::interrupts::ticks();
        let start = REPEAT_START.load(Ordering::Relaxed);
        let last = REPEAT_LAST.load(Ordering::Relaxed);
        let held_ms = (now.wrapping_sub(start)) * 10;
        let since_last = (now.wrapping_sub(last)) * 10;
        if held_ms >= 500 && since_last >= 50 {
            REPEAT_LAST.store(now, Ordering::Relaxed);
            let is_de = crate::keyboard::is_de_layout();
            let shift = REPEAT_SHIFT.load(Ordering::Relaxed);
            let altgr = REPEAT_ALTGR.load(Ordering::Relaxed);
            // Navigation keys are escape sequences, not characters: ESC
            // now, the rest from REPEAT_SEQ on the next calls.
            if let Some(last) = nav_seq_final(rk) {
                crate::interrupts::without_interrupts(|| {
                    *REPEAT_SEQ.lock() = RepeatSeq { bytes: [b'[', last], next: 0 };
                });
                return Some(0x1B);
            }
            let ch = hid_to_char(rk, shift, altgr, is_de);
            if ch != 0 {
                return Some(ch);
            }
        }
    }

    None
}

/// Final byte of the `ESC [ x` sequence for a HID navigation key.
fn nav_seq_final(key: u8) -> Option<u8> {
    Some(match key {
        0x4F => b'C', // Right
        0x50 => b'D', // Left
        0x51 => b'B', // Down
        0x52 => b'A', // Up
        0x4A => b'H', // Home
        0x4D => b'F', // End
        0x4B => b'5', // PgUp
        0x4E => b'6', // PgDn
        _ => return None,
    })
}

/// The bytes after ESC of a repeated navigation key. USB keyboards do not
/// repeat in hardware (PS/2 ones do), so `poll_keyboard` repeats held keys
/// itself and must deliver a sequence, not just a character.
struct RepeatSeq {
    bytes: [u8; 2],
    next: usize,
}

impl RepeatSeq {
    fn pop(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.next)?;
        self.next += 1;
        Some(b)
    }
}

static REPEAT_SEQ: spin::Mutex<RepeatSeq> =
    spin::Mutex::new(RepeatSeq { bytes: [0; 2], next: 2 });

static AVAILABLE: AtomicBool = AtomicBool::new(false);
static MOUSE_AVAILABLE: AtomicBool = AtomicBool::new(false);

// Lock-free key repeat state (written by timer IRQ, read by poll_keyboard)
static REPEAT_KEY: AtomicU8 = AtomicU8::new(0);
static REPEAT_SHIFT: AtomicBool = AtomicBool::new(false);
static REPEAT_ALTGR: AtomicBool = AtomicBool::new(false);
static REPEAT_START: AtomicU64 = AtomicU64::new(0);
static REPEAT_LAST: AtomicU64 = AtomicU64::new(0);

/// Poll for a mouse event from USB mouse.
/// Reads from software buffer only — timer IRQ drains hardware.
pub fn poll_mouse() -> Option<MouseEvent> {
    if !MOUSE_AVAILABLE.load(Ordering::Relaxed) { return None; }

    let head = MOUSE_HEAD.load(Ordering::Acquire);
    let tail = MOUSE_TAIL.load(Ordering::Relaxed);
    if head != tail {
        let evt = MouseEvent::unpack(MOUSE_BUF[tail].load(Ordering::Relaxed));
        MOUSE_TAIL.store((tail + 1) % MOUSE_BUF_SIZE, Ordering::Release);
        return Some(evt);
    }
    None
}

/// Check if USB mouse is available.
pub fn mouse_available() -> bool { MOUSE_AVAILABLE.load(Ordering::Relaxed) }

#[allow(dead_code)]
pub fn is_available() -> bool { AVAILABLE.load(Ordering::Relaxed) }

#[allow(dead_code)]
struct XhciState {
    /// PCI address of this controller, so a reset by another path (the NIC
    /// scan) can be attributed to exactly this controller and not discard
    /// working devices on another one.
    pci_addr: pci::PciAddr,
    mmio: Mmio,
    oper: Mmio,         // operational registers
    rt: Mmio,           // runtime registers
    db: Mmio,           // doorbell array
    ctx_size: usize,    // 32 or 64
    max_ports: u32,
    /// HCIVERSION.
    hci_version: u16,
    dcbaa: DmaRegion,
    cmd_ring: DmaRegion,
    cmd_cycle: u32,
    cmd_enqueue: usize,
    evt_ring: DmaRegion,
    evt_cycle: u32,
    evt_dequeue: usize,
    evt_seg_table: DmaRegion,
    input_ctx: DmaRegion,
    /// Data stage of control transfers, `MAX_CONTROL_LEN` bytes.
    data_buf: DmaRegion,
    /// Every addressed device, until it is unplugged.
    devs: Vec<UsbDevice>,
    /// The HID interfaces of the devices.
    hid: Vec<HidIntf>,
    /// Pages unplugged devices gave back, for the next one.
    free_pages: Vec<DmaRegion>,
    /// Root ports with a Port Status Change Event not yet handled (`hotplug`).
    port_changes: [u64; 4],
    /// Completion code of the last command or control transfer, for the
    /// log when enumeration fails; 0 = it timed out.
    last_cc: u32,
    /// The enumeration step that failed last (`port_failed`).
    last_fail: &'static str,
    /// Devices enumeration gave up on, until their port changes: what
    /// `usb` lists beside the addressed ones.
    failed: Vec<UsbSeen>,
    /// The network device, if one is attached here. It shares this state
    /// and its event ring dequeue pointer; a second state on the same ring
    /// would steal events.
    nic: Option<NicRings>,
}

/// One state per controller. Devices of different kinds (keyboard, mouse,
/// NIC) may sit on any controller, and several on the same one.
static CTRLS: spin::Mutex<Vec<XhciState>> = spin::Mutex::new(Vec::new());

/// Store a running controller: replaces the entry with the same PCI
/// address, else adds it.
fn store_ctrl(state: XhciState) {
    let mut g = CTRLS.lock();
    match g.iter_mut().find(|s| s.pci_addr == state.pci_addr) {
        Some(s) => *s = state,
        None => g.push(state),
    }
}

/// Initialize xHCI controller and enumerate USB keyboard.
/// Tries all xHCI controllers until one with a connected device is found.
pub fn init() -> bool {
    let mut found = false;
    // Find all xHCI controllers (class 0C:03:30) and bring up each
    for bus in 0u16..=255 {
        for dev_num in 0u8..32 {
            for func in 0u8..8 {
                let addr = pci::PciAddr { bus: bus as u8, device: dev_num, function: func };
                let id = pci::read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 {
                    if func == 0 { break; }
                    continue;
                }
                let class_reg = pci::read32(addr, 0x08);
                let cls = ((class_reg >> 24) & 0xFF) as u8;
                let sub = ((class_reg >> 16) & 0xFF) as u8;
                let prog_if = ((class_reg >> 8) & 0xFF) as u8;
                if cls == 0x0C && sub == 0x03 && prog_if == 0x30 {
                    let vid = (id & 0xFFFF) as u16;
                    let did = ((id >> 16) & 0xFFFF) as u16;
                    let pci_dev = pci::PciDevice {
                        addr, vendor_id: vid, device_id: did,
                        bar0: pci::read32(addr, 0x10),
                        irq_line: pci::read8(addr, 0x3C),
                    };
                    // Bring up every controller: a device may sit on any
                    // of them.
                    if init_controller(pci_dev) { found = true; }
                }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }
    found
}

/// Bring a controller from PCI-discovered to running: map BAR0, halt+reset,
/// set up command/event rings + scratchpad, start it, power and settle ports.
/// Returns the running state with no device enumerated yet; every device
/// slot the controller has is enabled (`xhci_init`: `HCS_MAX_SLOTS`).
fn bring_up_controller(dev: pci::PciDevice) -> Option<XhciState> {

    crate::kdebug!("[npk] xhci: PCI {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        dev.addr.bus, dev.addr.device, dev.addr.function,
        dev.vendor_id, dev.device_id);
    let _ = pci::claim(dev.addr, pci::Owner::Kernel("xhci"));

    pci::enable_bus_master(dev.addr);
    let cmd = pci::read16(dev.addr, 0x04);
    pci::write32(dev.addr, 0x04, (cmd | 0x06) as u32);

    // Enable mem-space + bus-mastering on EVERY bridge in the path to the
    // controller, not just the top-level one. The Titan Ridge TB3 xHCI sits
    // behind a chain of bridges (bus 0 → … → its bus); upstream DMA (command +
    // event ring) only traverses a bridge whose Bus Master Enable is set. MMIO
    // works without it, so the controller "runs" and ports are visible, but
    // every command (Enable Slot) times out because its completion never DMAs
    // back to the event ring. Every ancestor bridge's [secondary..subordinate]
    // range contains the target bus, so enabling all matching bridges across
    // all buses covers the whole chain.
    if dev.addr.bus > 0 {
        for bus in 0u16..=255 {
            for d in 0u8..32 {
                for f in 0u8..8 {
                    let ba = pci::PciAddr { bus: bus as u8, device: d, function: f };
                    let bid = pci::read32(ba, 0x00);
                    if bid == 0xFFFF_FFFF || bid == 0 { if f == 0 { break; } continue; }
                    if pci::read8(ba, 0x0E) & 0x7F == 1 {
                        let sec = pci::read8(ba, 0x19);
                        let sub_bus = pci::read8(ba, 0x1A);
                        if dev.addr.bus >= sec && dev.addr.bus <= sub_bus {
                            let bcmd = pci::read16(ba, 0x04);
                            pci::write32(ba, 0x04, (bcmd | 0x06) as u32); // mem + bus-master
                        }
                    }
                    if f == 0 && pci::read8(ba, 0x0E) & 0x80 == 0 { break; }
                }
            }
        }
    }

    // BAR0 (64-bit)
    let bar0_raw = pci::read32(dev.addr, 0x10);
    let bar0 = if bar0_raw & 0x04 != 0 {
        pci::read_bar64(dev.addr, 0x10)
    } else {
        (bar0_raw & 0xFFFF_FFF0) as u64
    };
    if bar0 == 0 { kprintln!("[npk] xhci: BAR0 is zero"); return None; }

    // Map BAR0 (64KB)
    let map_size = BAR0_MAP;
    // SAFETY: BAR0 of this xHCI controller, its register window.
    let mmio = match unsafe { Mmio::map(bar0, map_size) } {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk] xhci: map failed: {:?}", e); return None; }
    };

    // Read capability registers
    let caplength = mmio.r8(CAP_CAPLENGTH) as u32;
    let hcsparams1 = mmio.r32(CAP_HCSPARAMS1);
    let hcsparams2 = mmio.r32(CAP_HCSPARAMS2);
    let hccparams1 = mmio.r32(CAP_HCCPARAMS1);
    let dboff = mmio.r32(CAP_DBOFF) & 0xFFFF_FFFC;
    let rtsoff = mmio.r32(CAP_RTSOFF) & 0xFFFF_FFE0;

    let max_slots = hcsparams1 & 0xFF;
    let hci_version = (mmio.r32(CAP_CAPLENGTH) >> 16) as u16;
    let max_ports = (hcsparams1 >> 24) & 0xFF;
    let ctx_size: usize = if hccparams1 & 0x04 != 0 { 64 } else { 32 };

    if rtsoff as u64 >= map_size || dboff as u64 >= map_size {
        kprintln!("[npk] xhci: register offsets outside BAR0 (rt={:#x} db={:#x})", rtsoff, dboff);
        return None;
    }
    let oper = mmio.sub(caplength, map_size - caplength as u64);
    let rt = mmio.sub(rtsoff, map_size - rtsoff as u64);
    let db = mmio.sub(dboff, map_size - dboff as u64);

    crate::kdebug!("[npk] xhci: ports={} slots={} ctx={}B", max_ports, max_slots, ctx_size);

    // BIOS/OS handoff via extended capabilities
    let xecp_off = ((hccparams1 >> 16) & 0xFFFF) as u32 * 4;
    if xecp_off > 0 {
        bios_handoff(mmio, xecp_off);
    }

    // Halt controller
    let cmd_val = oper.r32(OP_USBCMD);
    oper.w32(OP_USBCMD, cmd_val & !CMD_RUN);
    if !wait_for(oper, OP_USBSTS, STS_HCH, STS_HCH) {
        kprintln!("[npk] xhci: halt timeout");
        return None;
    }

    // Reset controller
    oper.w32(OP_USBCMD, CMD_HCRST);
    if !wait_for(oper, OP_USBCMD, CMD_HCRST, 0) {
        kprintln!("[npk] xhci: reset timeout (CMD)");
        return None;
    }
    if !wait_for(oper, OP_USBSTS, STS_CNR, 0) {
        kprintln!("[npk] xhci: reset timeout (CNR)");
        return None;
    }

    // Allocate DMA structures (all page-aligned, zeroed). Each device gets
    // its own context and EP0 ring when it is enumerated.
    let (Some(dcbaa), Some(cmd_ring), Some(evt_ring), Some(evt_seg_table),
         Some(input_ctx), Some(data_buf)) = (
        alloc_dma(1, "DCBAA"),
        alloc_dma(1, "cmd ring"),
        alloc_dma(1, "evt ring"),
        alloc_dma(1, "evt seg table"),
        alloc_dma(1, "input ctx"),
        alloc_dma(MAX_CONTROL_LEN.div_ceil(4096), "data buf"),
    ) else {
        kprintln!("[npk] xhci: DMA alloc failed");
        return None;
    };

    // Set up Link TRBs at end of rings (wrap back to start)
    write_trb(cmd_ring, NUM_CMD_TRBS - 1, cmd_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1)); // Toggle Cycle

    // Set up Event Ring Segment Table (1 entry)
    evt_seg_table.w64(0u32, evt_ring.phys());           // ring base address
    evt_seg_table.w64(8u32, NUM_EVT_TRBS as u64);       // ring size

    // Scratchpad buffers
    let sp_hi = (hcsparams2 >> 21) & 0x1F;
    let sp_lo = (hcsparams2 >> 27) & 0x1F;
    let num_scratchpad = ((sp_hi << 5) | sp_lo) as usize;
    if num_scratchpad > 0 {
        let Some(sp_array) = alloc_dma(1, "scratchpad array") else {
            kprintln!("[npk] xhci: scratchpad alloc failed"); return None;
        };
        for i in 0..num_scratchpad {
            let Some(page) = alloc_dma(1, "scratchpad page") else {
                kprintln!("[npk] xhci: scratchpad page alloc failed"); return None;
            };
            sp_array.w64(i * 8, page.phys());
        }
        // DCBAA[0] = scratchpad array pointer
        dcbaa.w64(0u32, sp_array.phys());
        crate::kdebug!("[npk] xhci: {} scratchpad buffers", num_scratchpad);
    }

    // Program controller
    oper.w32(OP_CONFIG, max_slots); // MaxSlotsEn
    oper.w64_lo_hi(OP_DCBAAP, dcbaa.phys());
    oper.w64_lo_hi(OP_CRCR, cmd_ring.phys() | 1); // cycle bit = 1

    // Program Event Ring (interrupter 0)
    let ir0 = rt.sub(IR0, IR_LEN);
    ir0.w32(0x08u32, 1);                            // ERSTSZ = 1 segment
    ir0.w64_lo_hi(0x18u32, evt_ring.phys());        // ERDP
    ir0.w64_lo_hi(0x10u32, evt_seg_table.phys());   // ERSTBA (write AFTER ERSTSZ)
    // Enable interrupter (for event ring to work, even in polling mode)
    ir0.w32(0x00u32, ir0.r32(0x00u32) | 0x02); // IMAN.IE = 1

    // Start controller
    oper.w32(OP_USBCMD, CMD_RUN);
    if !wait_for(oper, OP_USBSTS, STS_HCH, 0) {
        kprintln!("[npk] xhci: start failed");
        return None;
    }
    crate::kdebug!("[npk] xhci: controller running");

    // Interrupter 0 by MSI-X to core 0. Without MSI-X the timer tick drains
    // the event ring.
    if pci::program_msix(dev.addr, 0, crate::interrupts::XHCI_VECTOR,
                         crate::interrupts::current_apic_id()) {
        oper.w32(OP_USBSTS, STS_EINT);
        oper.w32(OP_USBCMD, oper.r32(OP_USBCMD) | CMD_INTE);
        crate::kdebug!("[npk] xhci: events by MSI-X on vector {}", crate::interrupts::XHCI_VECTOR);
    } else {
        NEEDS_POLL.store(true, Ordering::Relaxed);
    }

    let state = XhciState {
        pci_addr: dev.addr,
        mmio, oper, rt, db, ctx_size, max_ports, hci_version,
        dcbaa, cmd_ring, cmd_cycle: 1, cmd_enqueue: 0,
        evt_ring, evt_cycle: 1, evt_dequeue: 0, evt_seg_table,
        input_ctx, data_buf,
        devs: Vec::new(), hid: Vec::new(), free_pages: Vec::new(), port_changes: [0; 4], last_cc: 0,
        last_fail: "", failed: Vec::new(), nic: None,
    };

    // Power on all ports
    for p in 0..max_ports {
        let off = portsc_off(p);
        let sc = oper.r32(off);
        if sc & PORTSC_PP == 0 {
            oper.w32(off, port_neutral(sc) | PORTSC_PP);
        }
    }

    // Wait for device attachment + link training. Poll and leave once the
    // port picture stops changing: USB 2.0 §7.1.7.3 wants 100 ms of connect
    // debounce, so that is the floor, and a late device keeps the window open.
    const POLL_MS:   u64 = 10;
    const DEBOUNCE:  u64 = 100;   // §7.1.7.3 TATTDB
    const SETTLE_MS: u64 = 100;   // quiet time before we call it done
    const CAP_MS:    u64 = 500;   // upper bound
    let mut elapsed = 0u64;
    let mut last_change = 0u64;
    let mut connected = 0u32;
    while elapsed < CAP_MS {
        crate::interrupts::delay_ms(POLL_MS);
        elapsed += POLL_MS;
        let now = (0..max_ports)
            .filter(|&p| oper.r32(portsc_off(p)) & PORTSC_CCS != 0)
            .count() as u32;
        if now != connected {
            connected = now;
            last_change = elapsed;
        }
        // Nothing connected yet → keep waiting; we cannot tell "empty port"
        // from "slow device", so an idle controller still costs the full cap.
        if connected > 0
            && elapsed >= DEBOUNCE
            && elapsed - last_change >= SETTLE_MS
        {
            break;
        }
    }

    // Debug: show all port states
    for p in 0..max_ports {
        let sc = oper.r32(portsc_off(p));
        if sc & PORTSC_CCS != 0 {
            let speed = (sc >> 10) & 0xF;
            crate::kdebug!("[npk] xhci: port {} connected (speed={}, portsc={:#010x})", p + 1, speed, sc);
        }
    }

    Some(state)
}

/// Initialize one xHCI controller and every device on it.
fn init_controller(dev: pci::PciDevice) -> bool {
    let Some(mut state) = bring_up_controller(dev) else { return false };
    enumerate_root_ports(&mut state);

    let keyboard = state.hid.iter().any(|h| h.running && h.keyboard);
    if state.hid.is_empty() {
        let connected = (0..state.max_ports)
            .filter(|&p| state.oper.r32(portsc_off(p)) & PORTSC_CCS != 0)
            .count();
        crate::kdebug!("[npk] xhci: no HID device — {} of {} ports connected",
            connected, state.max_ports);
    }
    // Kept without a HID device too: the NIC scan looks for devices on it.
    store_ctrl(state);
    keyboard
}

/// Enumerate the device on every connected root port and everything
/// below the hubs among them. A port that changes meanwhile is left to
/// `hotplug`.
fn enumerate_root_ports(state: &mut XhciState) {
    let mut seen = [0u64; 4];
    for p in 0..state.max_ports {
        if state.oper.r32(portsc_off(p)) & PORTSC_CCS == 0 { continue; }
        crate::kdebug!("[npk] xhci: trying port {}", p + 1);
        seen[(p / 64) as usize] |= 1 << (p % 64);
        port_connect_root(state, p);
    }
    // The ports looked at are accounted for: from here a change bit there is
    // news, and the events our own resets raised are dropped. A device that
    // arrived during the loop keeps its change bit, and `hotplug` takes it.
    state.port_changes = [0; 4];
    for p in 0..state.max_ports {
        let off = portsc_off(p);
        let sc = state.oper.r32(off);
        let looked = seen[(p / 64) as usize] & (1 << (p % 64)) != 0;
        if !looked && sc & PORTSC_CCS != 0 {
            state.port_changes[(p / 64) as usize] |= 1 << (p % 64);
            HOTPLUG.store(true, Ordering::Release);
            continue;
        }
        if sc & PORTSC_CHANGES != 0 { state.oper.w32(off, port_neutral(sc) | (sc & PORTSC_CHANGES)); }
    }
    // Below the hubs: what their first look at their ports found.
    hub_events(state);
}

/// Whether `init` found a USB pointer. Enumeration finds every HID
/// interface at once, mice included.
pub fn init_mouse() -> bool {
    USB_POINTERS.load(Ordering::Relaxed) > 0
}

// === Hubs — Linux drivers/usb/core/hub.c ===
//
// A hub reports which of its ports changed through its status change
// endpoint; the interrupt only notes the bits, `hub_events` (from
// `hotplug`, or at once during enumeration) reads each port's status
// over control transfers and connects or disconnects what is there.

const USB_CLASS_HUB: u8 = 9;
const USB_DT_HUB: u16 = 0x29;
const USB_DT_SS_HUB: u16 = 0x2a;
/// `USB_DT_HUB_NONVAR_SIZE`.
const USB_DT_HUB_NONVAR_SIZE: usize = 7;
/// `sizeof(struct usb_hub_descriptor)`.
const USB_DT_HUB_SIZE: u16 = 15;
const USB_DT_SS_HUB_SIZE: u16 = 12;
/// `USB_MAXCHILDREN`, `USB_SS_MAXPORTS`.
const USB_MAXCHILDREN: u8 = 31;
const USB_SS_MAXPORTS: u8 = 15;

/// `bDeviceProtocol` of a hub.
const USB_HUB_PR_HS_MULTI_TT: u8 = 2;
const USB_HUB_PR_SS: u8 = 3;

/// `USB_RT_HUB`, `USB_RT_PORT`; IN adds `USB_DIR_IN`.
const USB_RT_HUB: u8 = 0x20;
const USB_RT_PORT: u8 = 0x23;
const USB_DIR_IN: u8 = 0x80;
const HUB_SET_DEPTH: u8 = 12;

const HUB_CHAR_TTTT: u16 = 0x0060;

/// wPortStatus.
const USB_PORT_STAT_CONNECTION: u16 = 0x0001;
const USB_PORT_STAT_ENABLE: u16 = 0x0002;
const USB_PORT_STAT_OVERCURRENT: u16 = 0x0008;
const USB_PORT_STAT_RESET: u16 = 0x0010;
const USB_PORT_STAT_LOW_SPEED: u16 = 0x0200;
const USB_PORT_STAT_HIGH_SPEED: u16 = 0x0400;
const USB_PORT_STAT_LINK_STATE: u16 = 0x01e0;
const USB_SS_PORT_LS_SS_INACTIVE: u16 = 0x00c0;
const USB_SS_PORT_LS_COMP_MOD: u16 = 0x0140;
/// wPortChange.
const USB_PORT_STAT_C_CONNECTION: u16 = 0x0001;
const USB_PORT_STAT_C_ENABLE: u16 = 0x0002;
const USB_PORT_STAT_C_SUSPEND: u16 = 0x0004;
const USB_PORT_STAT_C_OVERCURRENT: u16 = 0x0008;
const USB_PORT_STAT_C_RESET: u16 = 0x0010;
const USB_PORT_STAT_C_BH_RESET: u16 = 0x0020;
const USB_PORT_STAT_C_LINK_STATE: u16 = 0x0040;
const USB_PORT_STAT_C_CONFIG_ERROR: u16 = 0x0080;
/// wHubChange.
const HUB_CHANGE_LOCAL_POWER: u16 = 0x0001;
const HUB_CHANGE_OVERCURRENT: u16 = 0x0002;

/// Port features.
const USB_PORT_FEAT_ENABLE: u16 = 1;
const USB_PORT_FEAT_RESET: u16 = 4;
const USB_PORT_FEAT_POWER: u16 = 8;
const USB_PORT_FEAT_C_CONNECTION: u16 = 16;
const USB_PORT_FEAT_C_ENABLE: u16 = 17;
const USB_PORT_FEAT_C_SUSPEND: u16 = 18;
const USB_PORT_FEAT_C_OVER_CURRENT: u16 = 19;
const USB_PORT_FEAT_C_RESET: u16 = 20;
const USB_PORT_FEAT_LINK_STATE: u16 = 5;
const USB_PORT_FEAT_C_PORT_LINK_STATE: u16 = 25;
const USB_PORT_FEAT_C_PORT_CONFIG_ERROR: u16 = 26;
const USB_PORT_FEAT_BH_PORT_RESET: u16 = 28;
const USB_PORT_FEAT_C_BH_PORT_RESET: u16 = 29;
/// Hub features.
const C_HUB_LOCAL_POWER: u16 = 0;
const C_HUB_OVER_CURRENT: u16 = 1;
const USB_SS_PORT_LS_U3: u16 = 0x0003;

const PORT_RESET_TRIES: u32 = 5;
const HUB_SHORT_RESET_TIME: u64 = 10;
const HUB_LONG_RESET_TIME: u64 = 200;
const HUB_RESET_TIMEOUT: u64 = 800;
const HUB_BH_RESET_TIME: u64 = 50;
const DETECT_DISCONNECT_TRIES: u32 = 5;

/// Consecutive failed status transfers after which a hub's endpoint is no
/// longer resubmitted (`hub_irq`).
const HUB_IRQ_ERRORS: u32 = 10;

/// A hub's own state (Linux `usb_hub`).
struct Hub {
    nports: u8,
    ss: bool,
    multi_tt: bool,
    /// `hub_power_on_good_delay`.
    power_ms: u64,
    /// The status change endpoint: ring, buffer, DCI, bytes per transfer.
    ring: DmaRegion,
    cycle: u32,
    enqueue: usize,
    buf: DmaRegion,
    dci: u8,
    maxp: u16,
    /// Ports (bit = port number, bit 0 the hub) the endpoint reported
    /// (`event_bits`), and ports to check for a connection (`change_bits`).
    event_bits: u32,
    change_bits: u32,
    nerrors: u32,
    running: bool,
}

/// The hub interface of a configuration and its interrupt IN endpoint:
/// `(interface number, endpoint address, wMaxPacketSize, bInterval)`.
fn hub_interface(cfg: &[u8]) -> Option<(u8, u8, u16, u8)> {
    let mut i = 0;
    let mut cur: Option<u8> = None;
    while i + 2 <= cfg.len() {
        let len = cfg[i] as usize;
        if len < 2 || i + len > cfg.len() { break; }
        match cfg[i + 1] {
            0x04 if len >= 9 => {
                cur = (cfg[i + 5] == USB_CLASS_HUB && cfg[i + 3] == 0).then_some(cfg[i + 2]);
            }
            0x05 if len >= 7 => {
                if let Some(ifnum) = cur {
                    let addr = cfg[i + 2];
                    if addr & 0x80 != 0 && cfg[i + 3] & 0x3 == 3 {
                        let maxp = u16::from_le_bytes([cfg[i + 4], cfg[i + 5]]);
                        return Some((ifnum, addr, maxp, cfg[i + 6]));
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    None
}

/// A class or standard request to the hub, no data stage.
fn hub_request(state: &mut XhciState, slot: u8, rt: u8, req: u8, value: u16, index: u16) -> bool {
    usb_control_transfer(state, slot, rt, req, value, index, 0, false)
}

fn set_port_feature(state: &mut XhciState, slot: u8, port1: u8, feature: u16) -> bool {
    hub_request(state, slot, USB_RT_PORT, USB_SET_FEATURE, feature, port1 as u16)
}

fn clear_port_feature(state: &mut XhciState, slot: u8, port1: u8, feature: u16) -> bool {
    hub_request(state, slot, USB_RT_PORT, USB_CLEAR_FEATURE, feature, port1 as u16)
}

/// `usb_hub_port_status`: (wPortStatus, wPortChange).
fn hub_port_status(state: &mut XhciState, slot: u8, port1: u8) -> Option<(u16, u16)> {
    state.data_buf.fill(0u32, 0, 4);
    if !usb_control_transfer(state, slot, USB_DIR_IN | USB_RT_PORT, USB_GET_STATUS, 0, port1 as u16, 4, true) {
        return None;
    }
    Some((state.data_buf.r16(0u32), state.data_buf.r16(2u32)))
}

/// `hub_hub_status`: (wHubStatus, wHubChange).
fn hub_hub_status(state: &mut XhciState, slot: u8) -> Option<(u16, u16)> {
    state.data_buf.fill(0u32, 0, 4);
    if !usb_control_transfer(state, slot, USB_DIR_IN | USB_RT_HUB, USB_GET_STATUS, 0, 0, 4, true) {
        return None;
    }
    Some((state.data_buf.r16(0u32), state.data_buf.r16(2u32)))
}

fn hub_of(state: &XhciState, slot: u8) -> Option<&Hub> {
    dev_idx(state, slot).and_then(|d| state.devs[d].hub.as_ref())
}

fn hub_of_mut(state: &mut XhciState, slot: u8) -> Option<&mut Hub> {
    let d = dev_idx(state, slot)?;
    state.devs[d].hub.as_mut()
}

/// `hub_probe` and `hub_configure`: the status endpoint configured, the
/// hub descriptor read, the controller told it is a hub
/// (`xhci_update_hub_device`), then `hub_activate`.
fn hub_probe(state: &mut XhciState, slot: u8, device: &[u8], cfg: &[u8]) {
    let Some(d) = dev_idx(state, slot) else { return };
    let path = state.devs[d].seen.port.clone();
    let (level, speed) = (state.devs[d].place.level, state.devs[d].speed);
    if level == MAX_TOPO_LEVEL {
        kprintln!("[npk] xhci: hub on port {}: unsupported bus topology, hub nested too deep", path);
        return;
    }
    let Some((ifnum, ep_addr, w_max, interval)) = hub_interface(cfg) else {
        kprintln!("[npk] xhci: hub on port {}: no status change endpoint", path);
        return;
    };
    let protocol = device.get(6).copied().unwrap_or(0);
    let ss = protocol == USB_HUB_PR_SS;

    // `usb_set_configuration`: the endpoint, then SET_CONFIGURATION.
    let (Some(ring), Some(buf)) = (take_ring(state, "hub status"), take_page(state, "hub buf")) else {
        kprintln!("[npk] xhci: DMA alloc failed");
        return;
    };
    let dci = (ep_addr & 0x0F) * 2 + 1;
    let give_back = |state: &mut XhciState| {
        state.free_pages.push(ring);
        state.free_pages.push(buf);
    };
    if !cmd_configure_endpoints(state, slot, &[(dci, ring.phys(), EP_TYPE_INTERRUPT_IN, w_max, interval)]) {
        kprintln!("[npk] xhci: hub on port {}: configure endpoints failed", path);
        give_back(state);
        return;
    }
    if !usb_set_config(state, slot, cfg.get(5).copied().unwrap_or(1)) {
        kprintln!("[npk] xhci: hub on port {}: set configuration failed", path);
        give_back(state);
        return;
    }

    // `get_hub_descriptor`.
    let (dtype, size) = if ss { (USB_DT_SS_HUB, USB_DT_SS_HUB_SIZE) } else { (USB_DT_HUB, USB_DT_HUB_SIZE) };
    let mut desc = None;
    for _ in 0..3 {
        state.data_buf.fill(0u32, 0, size as u64);
        if usb_control_transfer(state, slot, USB_DIR_IN | USB_RT_HUB, USB_GET_DESCRIPTOR, dtype << 8, 0, size, true)
            && state.data_buf.r8(0u32) as usize >= USB_DT_HUB_NONVAR_SIZE
        {
            let mut b = [0u8; USB_DT_HUB_SIZE as usize];
            state.data_buf.copy_out(0u32, &mut b);
            desc = Some(b);
            break;
        }
    }
    let Some(desc) = desc else {
        kprintln!("[npk] xhci: hub on port {}: config failed, can't read hub descriptor", path);
        give_back(state);
        return;
    };
    let nports = desc[2];
    let maxchild = if ss { USB_SS_MAXPORTS } else { USB_MAXCHILDREN };
    if nports == 0 || nports > maxchild {
        kprintln!("[npk] xhci: hub on port {}: config failed, {} ports", path, nports);
        give_back(state);
        return;
    }
    let characteristics = u16::from_le_bytes([desc[3], desc[4]]);
    let power_ms = (desc[5] as u64 * 2).max(100);

    // The TT of a high speed hub; a multi-TT one is switched to alternate
    // setting 1, where it has one TT per port.
    let mut multi_tt = false;
    if protocol == USB_HUB_PR_HS_MULTI_TT {
        multi_tt = hub_request(state, slot, 0x01, USB_SET_INTERFACE, 1, ifnum as u16);
        if !multi_tt {
            kprintln!("[npk] xhci: hub on port {}: using single TT", path);
        }
    }
    let think_time = match characteristics & HUB_CHAR_TTTT {
        0x0000 if protocol != 0 => 666,
        0x0020 => 666 * 2,
        0x0040 => 666 * 3,
        0x0060 => 666 * 4,
        _ => 0,
    };

    let Some(d) = dev_idx(state, slot) else { return };
    state.devs[d].hub = Some(Hub {
        nports, ss, multi_tt, power_ms, ring, cycle: 1, enqueue: 0, buf, dci,
        maxp: (w_max & 0x7FF).min(4096), event_bits: 0, change_bits: 0, nerrors: 0, running: false,
    });
    if !xhci_update_hub_device(state, slot, nports, multi_tt, think_time, speed) {
        kprintln!("[npk] xhci: hub on port {}: config failed, can't update HCD hub info", path);
    }
    kprintln!("[npk] xhci: USB hub on port {}, {} port{}{}", path, nports,
        if nports == 1 { "" } else { "s" }, if ss { ", SuperSpeed" } else { "" });
    hub_activate(state, slot);
}

/// `xhci_update_hub_device`: the slot context marks the device as a hub
/// with its port count, multi-TT and TT think time.
fn xhci_update_hub_device(state: &mut XhciState, slot: u8, nports: u8, multi_tt: bool, think_time: u32,
                          speed: u32) -> bool {
    let Some(d) = dev_idx(state, slot) else { return false };
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    let dctx = state.devs[d].dctx;
    input.fill(0u32, 0, 4096);
    input.w32(4u32, 1); // SLOT_FLAG
    for k in 0..4u32 {
        input.w32(ctx + (k * 4) as usize, dctx.r32(k * 4));
    }
    let mut dw0 = input.r32(ctx) | (1 << 26); // DEV_HUB
    if multi_tt {
        dw0 |= 1 << 25; // DEV_MTT
    } else if speed == SPEED_FULL {
        dw0 &= !(1 << 25);
    }
    input.w32(ctx, dw0);
    let new_ctx = state.hci_version > 0x95;
    if new_ctx {
        input.w32(ctx + 4, input.r32(ctx + 4) | ((nports as u32) << 24)); // XHCI_MAX_PORTS
        let tt = if think_time != 0 { think_time / 666 - 1 } else { 0 };
        if state.hci_version < 0x100 || speed == SPEED_HIGH {
            input.w32(ctx + 8, input.r32(ctx + 8) | (tt << 16)); // TT_THINK_TIME
        }
    }
    input.w32(ctx + 12, 0); // dev_state
    let trb = if new_ctx { TRB_CONFIGURE_EP } else { TRB_EVALUATE_CTX };
    post_command(state, input.phys(), 0, trb | ((slot as u32) << 24));
    matches!(wait_command_completion(state), Some((cc, _)) if cc == CC_SUCCESS)
}

/// `hub_activate(HUB_INIT)`: hub depth for a SuperSpeed hub, power on
/// every port and wait for it to be good, then look at every port and note
/// those to be handled; the status endpoint starts.
fn hub_activate(state: &mut XhciState, slot: u8) {
    let Some(d) = dev_idx(state, slot) else { return };
    let level = state.devs[d].place.level;
    let Some(h) = state.devs[d].hub.as_ref() else { return };
    let (nports, ss, power_ms) = (h.nports, h.ss, h.power_ms);
    if ss && !hub_request(state, slot, USB_RT_HUB, HUB_SET_DEPTH, (level - 1) as u16, 0) {
        kprintln!("[npk] xhci: hub slot {}: set hub depth failed", slot);
    }
    // `hub_power_on`.
    for port1 in 1..=nports {
        set_port_feature(state, slot, port1, USB_PORT_FEAT_POWER);
    }
    wait_ms(state, power_ms);

    // init2: every port disabled after initialization (a USB 3 port only
    // pretends), change bits cleared, connected ports noted.
    let mut change_bits = 0u32;
    for port1 in 1..=nports {
        let Some((status, change)) = hub_port_status(state, slot, port1) else { break };
        if status & USB_PORT_STAT_ENABLE != 0 && !ss {
            clear_port_feature(state, slot, port1, USB_PORT_FEAT_ENABLE);
        }
        if change & USB_PORT_STAT_C_CONNECTION != 0 {
            clear_port_feature(state, slot, port1, USB_PORT_FEAT_C_CONNECTION);
        }
        if change & USB_PORT_STAT_C_ENABLE != 0 {
            clear_port_feature(state, slot, port1, USB_PORT_FEAT_C_ENABLE);
        }
        if change & USB_PORT_STAT_C_RESET != 0 {
            clear_port_feature(state, slot, port1, USB_PORT_FEAT_C_RESET);
        }
        if change & USB_PORT_STAT_C_BH_RESET != 0 && ss {
            clear_port_feature(state, slot, port1, USB_PORT_FEAT_C_BH_PORT_RESET);
        }
        if status & USB_PORT_STAT_CONNECTION != 0
            || change & USB_PORT_STAT_C_CONNECTION != 0
            || status & USB_PORT_STAT_OVERCURRENT != 0
            || change & USB_PORT_STAT_C_OVERCURRENT != 0
        {
            change_bits |= 1 << port1;
        }
    }
    if let Some(h) = hub_of_mut(state, slot) {
        h.change_bits |= change_bits;
        h.running = true;
    }
    hub_submit(state, slot);
    if change_bits != 0 {
        HOTPLUG.store(true, Ordering::Release);
    }
}

/// Queue the status change endpoint's next transfer.
fn hub_submit(state: &mut XhciState, slot: u8) {
    let Some(h) = hub_of_mut(state, slot) else { return };
    if !h.running { return; }
    write_trb(h.ring, h.enqueue, h.buf.phys(), h.maxp as u32, TRB_NORMAL | TRB_IOC | h.cycle);
    h.enqueue += 1;
    if h.enqueue >= NUM_TR_TRBS - 1 {
        write_trb(h.ring, NUM_TR_TRBS - 1, h.ring.phys(), 0, TRB_LINK | h.cycle | (1 << 1));
        h.cycle ^= 1;
        h.enqueue = 0;
    }
    let dci = h.dci as u32;
    ring_doorbell(state, slot as u32, dci);
}

/// `hub_irq`: the bitmap of changed ports is noted for `hub_events`, the
/// transfer queued again.
fn hub_irq(state: &mut XhciState, slot: u8, e: &Evt) {
    let Some(h) = hub_of_mut(state, slot) else { return };
    if !h.running { return; }
    if e.cc == CC_SUCCESS || e.cc == CC_SHORT_PACKET {
        h.nerrors = 0;
        let len = (h.maxp as usize).saturating_sub(e.residual as usize).min(4);
        let mut bits = 0u32;
        for i in 0..len {
            bits |= (h.buf.r8(i as u32) as u32) << (8 * i);
        }
        h.event_bits |= bits;
        HOTPLUG.store(true, Ordering::Release);
    } else {
        h.nerrors += 1;
        if h.nerrors >= HUB_IRQ_ERRORS {
            kprintln!("[npk] xhci: hub slot {}: status transfer failed {} times, completion code {}",
                slot, h.nerrors, e.cc);
            h.running = false;
            return;
        }
    }
    hub_submit(state, slot);
}

/// `hub_event` for every hub with something noted, until none has: hub
/// status changes, then each port's (`port_event`). Hubs found meanwhile
/// are handled in the same run.
fn hub_events(state: &mut XhciState) {
    loop {
        let next = state.devs.iter().find_map(|d| {
            d.hub.as_ref().filter(|h| h.event_bits | h.change_bits != 0).map(|_| d.slot)
        });
        let Some(slot) = next else { return };
        let Some(h) = hub_of_mut(state, slot) else { continue };
        let (events, changes, nports) = (h.event_bits, h.change_bits, h.nports);
        h.event_bits = 0;
        h.change_bits = 0;

        if events & 1 != 0 {
            if let Some((_, hubchange)) = hub_hub_status(state, slot) {
                if hubchange & HUB_CHANGE_LOCAL_POWER != 0 {
                    hub_request(state, slot, USB_RT_HUB, USB_CLEAR_FEATURE, C_HUB_LOCAL_POWER, 0);
                }
                if hubchange & HUB_CHANGE_OVERCURRENT != 0 {
                    kprintln!("[npk] xhci: hub slot {}: over-current change", slot);
                    wait_ms(state, 500);
                    hub_request(state, slot, USB_RT_HUB, USB_CLEAR_FEATURE, C_HUB_OVER_CURRENT, 0);
                    for port1 in 1..=nports {
                        set_port_feature(state, slot, port1, USB_PORT_FEAT_POWER);
                    }
                }
            }
        }
        for port1 in 1..=nports {
            let bit = 1u32 << port1;
            if (events | changes) & bit == 0 { continue; }
            // The hub may have gone while its ports were handled.
            if dev_idx(state, slot).is_none() { break; }
            port_event(state, slot, port1, changes & bit != 0);
        }
    }
}

/// The device on port `port1` of hub `hub`, if one is there.
fn port_child(state: &XhciState, hub: u8, port1: u8) -> Option<u8> {
    state.devs.iter().find(|d| d.place.parent == Some((hub, port1))).map(|d| d.slot)
}

/// `hub_port_warm_reset_required`.
fn warm_reset_required(ss: bool, status: u16) -> bool {
    ss && matches!(status & USB_PORT_STAT_LINK_STATE, USB_SS_PORT_LS_SS_INACTIVE | USB_SS_PORT_LS_COMP_MOD)
}

/// `port_event`.
fn port_event(state: &mut XhciState, hub: u8, port1: u8, mut connect_change: bool) {
    let Some(ss) = hub_of(state, hub).map(|h| h.ss) else { return };
    let udev = port_child(state, hub, port1);
    let Some((mut status, change)) = hub_port_status(state, hub, port1) else { return };

    if change & USB_PORT_STAT_C_CONNECTION != 0 {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_CONNECTION);
        connect_change = true;
    }
    if change & USB_PORT_STAT_C_ENABLE != 0 {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_ENABLE);
        // EM interference sometimes disables a badly shielded device.
        if status & USB_PORT_STAT_ENABLE == 0 && !connect_change && udev.is_some() {
            kprintln!("[npk] xhci: hub slot {} port {}: disabled by hub (EMI?), re-enabling...", hub, port1);
            connect_change = true;
        }
    }
    if change & USB_PORT_STAT_C_SUSPEND != 0 && !ss {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_SUSPEND);
    }
    if change & USB_PORT_STAT_C_OVERCURRENT != 0 {
        kprintln!("[npk] xhci: hub slot {} port {}: over-current change", hub, port1);
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_OVER_CURRENT);
        wait_ms(state, 100);
        let n = hub_of(state, hub).map_or(0, |h| h.nports);
        for p in 1..=n {
            set_port_feature(state, hub, p, USB_PORT_FEAT_POWER);
        }
        let power_ms = hub_of(state, hub).map_or(100, |h| h.power_ms);
        wait_ms(state, power_ms);
        if hub_port_status(state, hub, port1).is_some_and(|(s, _)| s & USB_PORT_STAT_OVERCURRENT != 0) {
            kprintln!("[npk] xhci: hub slot {} port {}: over-current condition", hub, port1);
        }
    }
    if change & USB_PORT_STAT_C_RESET != 0 {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_RESET);
    }
    if change & USB_PORT_STAT_C_BH_RESET != 0 && ss {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_BH_PORT_RESET);
    }
    if change & USB_PORT_STAT_C_LINK_STATE != 0 && ss {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_PORT_LINK_STATE);
    }
    if change & USB_PORT_STAT_C_CONFIG_ERROR != 0 && ss {
        kprintln!("[npk] xhci: hub slot {} port {}: config error", hub, port1);
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_PORT_CONFIG_ERROR);
    }

    // A USB 3 link left in SS.Inactive or Compliance after a disconnect
    // goes back to RxDetect on its own within 12 ms; otherwise a warm reset.
    let mut i = 0;
    while warm_reset_required(ss, status) {
        if i < DETECT_DISCONNECT_TRIES && udev.is_some() {
            i += 1;
            wait_ms(state, 20);
            match hub_port_status(state, hub, port1) {
                Some((s, _)) => status = s,
                None => return,
            }
            continue;
        }
        match hub_port_reset(state, hub, port1, HUB_BH_RESET_TIME, true) {
            Err(PortReset::NotConnected) if udev.is_none() => connect_change = false,
            Err(_) => hub_port_disable_ext(state, hub, port1, ss),
            Ok(_) => {}
        }
        if let Some(c) = udev {
            usb_disconnect(state, c);
            connect_change = true;
        }
        break;
    }

    if connect_change {
        hub_port_connect_change(state, hub, port1, change);
    }
}

/// `hub_port_disable` for a hub port: a USB 3 port's link to U3, a USB 2
/// port disabled.
fn hub_port_disable_ext(state: &mut XhciState, hub: u8, port1: u8, ss: bool) {
    if ss {
        hub_request(state, hub, USB_RT_PORT, USB_SET_FEATURE, USB_PORT_FEAT_LINK_STATE,
            port1 as u16 | (USB_SS_PORT_LS_U3 << 8));
    } else {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_ENABLE);
    }
}

/// `hub_port_connect_change`: whatever was on the port is disconnected,
/// the port debounced, and a device there connected.
fn hub_port_connect_change(state: &mut XhciState, hub: u8, port1: u8, change: u16) {
    if let Some(c) = port_child(state, hub, port1) {
        usb_disconnect(state, c);
    }
    let path = match dev_idx(state, hub) {
        Some(h) => format!("{}.{}", state.devs[h].seen.port, port1),
        None => return,
    };
    state.failed.retain(|s| s.port != path);
    let mut status = 0u16;
    if change & (USB_PORT_STAT_C_CONNECTION | USB_PORT_STAT_C_ENABLE) != 0 {
        match hub_port_debounce(state, hub, port1) {
            Some(s) => status = s,
            None => {
                kprintln!("[npk] xhci: port {}: connect-debounce failed", path);
                return;
            }
        }
    } else if let Some((s, _)) = hub_port_status(state, hub, port1) {
        status = s;
    }
    if status & USB_PORT_STAT_CONNECTION == 0 {
        return;
    }
    kprintln!("[npk] xhci: device connected on port {}", path);
    hub_port_connect(state, hub, port1, path);
}

/// `hub_port_debounce`: the port status once the connection held for
/// `HUB_DEBOUNCE_STABLE` ms, or None if it never settled.
fn hub_port_debounce(state: &mut XhciState, hub: u8, port1: u8) -> Option<u16> {
    let mut connection: Option<u16> = None;
    let mut stable = 0u64;
    let mut total = 0u64;
    loop {
        let (status, change) = hub_port_status(state, hub, port1)?;
        let now = status & USB_PORT_STAT_CONNECTION;
        if change & USB_PORT_STAT_C_CONNECTION == 0 && connection == Some(now) {
            stable += HUB_DEBOUNCE_STEP;
            if stable >= HUB_DEBOUNCE_STABLE { return Some(status); }
        } else {
            stable = 0;
            connection = Some(now);
        }
        if change & USB_PORT_STAT_C_CONNECTION != 0 {
            clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_CONNECTION);
        }
        if total >= HUB_DEBOUNCE_TIMEOUT { return None; }
        wait_ms(state, HUB_DEBOUNCE_STEP);
        total += HUB_DEBOUNCE_STEP;
    }
}

/// Why a hub port reset failed (`hub_port_wait_reset`'s errors).
enum PortReset {
    Busy,
    NotConnected,
    Again,
    Failed,
}

/// `hub_port_wait_reset`: the speed of the device once the reset ended.
fn hub_port_wait_reset(state: &mut XhciState, hub: u8, port1: u8, ss: bool, mut delay: u64)
                       -> Result<u32, PortReset> {
    let mut waited = 0;
    let (mut status, mut change) = (0u16, 0u16);
    while waited < HUB_RESET_TIMEOUT {
        wait_ms(state, delay);
        (status, change) = hub_port_status(state, hub, port1).ok_or(PortReset::Failed)?;
        if status & USB_PORT_STAT_RESET == 0 && status & USB_PORT_STAT_CONNECTION != 0 {
            break;
        }
        waited += delay;
        if waited >= 2 * HUB_SHORT_RESET_TIME {
            delay = HUB_LONG_RESET_TIME;
        }
    }
    if status & USB_PORT_STAT_RESET != 0 { return Err(PortReset::Busy); }
    if warm_reset_required(ss, status) { return Err(PortReset::NotConnected); }
    if status & USB_PORT_STAT_CONNECTION == 0 { return Err(PortReset::NotConnected); }
    if !ss && change & USB_PORT_STAT_C_CONNECTION != 0 {
        clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_CONNECTION);
        return Err(PortReset::Again);
    }
    if status & USB_PORT_STAT_ENABLE == 0 { return Err(PortReset::Busy); }
    Ok(if ss {
        SPEED_SUPER
    } else if status & USB_PORT_STAT_HIGH_SPEED != 0 {
        SPEED_HIGH
    } else if status & USB_PORT_STAT_LOW_SPEED != 0 {
        SPEED_LOW
    } else {
        SPEED_FULL
    })
}

/// `hub_port_reset`: a (warm, for USB 3) reset of the port, retried; the
/// device's speed after it.
fn hub_port_reset(state: &mut XhciState, hub: u8, port1: u8, mut delay: u64, mut warm: bool)
                  -> Result<u32, PortReset> {
    let Some(ss) = hub_of(state, hub).map(|h| h.ss) else { return Err(PortReset::Failed) };
    if ss && !warm {
        if let Some((s, _)) = hub_port_status(state, hub, port1) {
            warm = warm_reset_required(ss, s);
        }
    }
    let mut result = Err(PortReset::Failed);
    for i in 0..PORT_RESET_TRIES {
        let feature = if warm { USB_PORT_FEAT_BH_PORT_RESET } else { USB_PORT_FEAT_RESET };
        result = if set_port_feature(state, hub, port1, feature) {
            hub_port_wait_reset(state, hub, port1, ss, delay)
        } else {
            Err(PortReset::Failed)
        };
        let last = i == PORT_RESET_TRIES - 1;
        if matches!(result, Ok(_) | Err(PortReset::NotConnected)) || (matches!(result, Err(PortReset::Busy)) && last) {
            clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_RESET);
            if !ss { break; }
            clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_BH_PORT_RESET);
            clear_port_feature(state, hub, port1, USB_PORT_FEAT_C_PORT_LINK_STATE);
            match hub_port_status(state, hub, port1) {
                Some((s, _)) if warm_reset_required(ss, s) => warm = true,
                _ => break,
            }
        }
        delay = HUB_LONG_RESET_TIME;
    }
    if result.is_ok() {
        // TRSTRCY = 10 ms; plus some extra.
        wait_ms(state, 10 + 40);
    } else {
        kprintln!("[npk] xhci: hub slot {} port {}: cannot enable. Maybe the USB cable is bad?", hub, port1);
    }
    result
}

/// `hub_port_connect`: reset, enumerate and hand the device to its driver,
/// `PORT_INIT_TRIES` times; a port nothing would enumerate on is disabled.
fn hub_port_connect(state: &mut XhciState, hub: u8, port1: u8, path: String) {
    let Some(ss) = hub_of(state, hub).map(|h| h.ss) else { return };
    let mut speed = 0;
    for attempt in 0..PORT_INIT_TRIES {
        if attempt > 0 {
            kprintln!("[npk] xhci: port {} retrying ({}/{})", path, attempt + 1, PORT_INIT_TRIES);
        }
        let delay = if attempt == 0 { HUB_SHORT_RESET_TIME } else { HUB_LONG_RESET_TIME };
        match hub_port_reset(state, hub, port1, delay, false) {
            Ok(s) => {
                speed = s;
                let Some(h) = dev_idx(state, hub) else { return };
                let place = Place::below(&state.devs[h], port1, speed);
                if let Some(slot) = usb_port_init(state, place, speed) {
                    usb_new_device(state, slot);
                    return;
                }
            }
            Err(PortReset::NotConnected) => return,
            Err(_) => {}
        }
        match hub_port_status(state, hub, port1) {
            Some((s, _)) if s & USB_PORT_STAT_CONNECTION != 0 => {}
            _ => return,
        }
    }
    kprintln!("[npk] xhci: port {}: unable to enumerate USB device, port disabled", path);
    enumeration_failed(state, path, speed);
    hub_port_disable_ext(state, hub, port1, ss);
}

/// Wait `ms` while keeping every device on this controller served: the
/// event ring is drained meanwhile, as an interrupt would.
fn wait_ms(state: &mut XhciState, ms: u64) {
    let deadline = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 1000 * ms;
    while crate::interrupts::rdtsc() < deadline {
        drain(state);
        core::hint::spin_loop();
    }
}

// === USB HID — Linux drivers/hid/usbhid/hid-core.c ===
//
// One USB device may carry several HID interfaces (a receiver with keyboard
// on interface 0 and mouse plus media keys on interface 1), and each is a
// HID device of its own: its report descriptor, its interrupt endpoint, its
// reports. What a report means comes from the descriptor (`hid_core`), not
// from the boot protocol, which carries neither the wheel past 8 bits nor
// the consumer keys. The device stays in the report protocol it starts in;
// usbhid never sends SET_PROTOCOL.

// === USB devices — Linux usb_device, xHCI xhci_virt_device ===
//
// Every device enumeration addresses keeps its slot, its device context
// and its own EP0 ring until it is unplugged, whether a driver takes it or
// not: a driver that comes later (the network device) finds it there.

/// Where a device hangs in the tree (`usb_device`'s parent, portnum,
/// route, level and tt).
#[derive(Clone, Copy)]
struct Place {
    /// Root hub port (0-based) its path starts at.
    root_port: u32,
    /// Route string: the hub port below each tier, four bits per tier
    /// (xHCI 4.3.3).
    route: u32,
    /// The hub (its slot) and the 1-based port on it; None on a root port.
    parent: Option<(u8, u8)>,
    /// 1 on a root port, one more per hub (`usb_device.level`).
    level: u8,
    /// The transaction translator a low or full speed device behind a
    /// high speed hub goes through: the TT hub's slot, its port, multi-TT.
    tt: Option<(u8, u8, bool)>,
}

impl Place {
    fn root(port: u32) -> Self {
        Place { root_port: port, route: 0, parent: None, level: 1, tt: None }
    }

    /// Port `port1` of `hub`, for a device of `speed` (`usb_alloc_dev`, and
    /// the TT records of `hub_port_init`).
    fn below(hub: &UsbDevice, port1: u8, speed: u32) -> Self {
        let tier = (port1 as u32).min(15) << ((hub.place.level as u32 - 1) * 4);
        let tt = match (hub.place.tt, hub.hub.as_ref()) {
            (Some(tt), _) => Some(tt),
            (None, Some(h)) if speed != SPEED_HIGH && speed < SPEED_SUPER && hub.speed == SPEED_HIGH => {
                Some((hub.slot, port1, h.multi_tt))
            }
            _ => None,
        };
        Place {
            root_port: hub.place.root_port,
            route: hub.place.route + tier,
            parent: Some((hub.slot, port1)),
            level: hub.place.level + 1,
            tt,
        }
    }
}

/// An addressed device.
struct UsbDevice {
    slot: u8,
    place: Place,
    /// Slot context speed: 1 full, 2 low, 3 high, 4 super, 5 super+.
    speed: u32,
    dctx: DmaRegion,
    ep0: DmaRegion,
    ep0_cycle: u32,
    ep0_enqueue: usize,
    /// What enumeration read: descriptors and strings, for `usb`.
    seen: UsbSeen,
    /// The hub driver's state, if the device is a hub it took.
    hub: Option<Hub>,
}

/// `MAX_TOPO_LEVEL`: the root hub, five hubs, a device (USB 2.0 §4.1.1).
const MAX_TOPO_LEVEL: u8 = 6;

fn dev_idx(state: &XhciState, slot: u8) -> Option<usize> {
    state.devs.iter().position(|d| d.slot == slot)
}

/// The device's path as Linux names it below its root port: "3", "3.1.4".
fn dev_path(state: &XhciState, place: Place) -> String {
    match place.parent {
        None => format!("{}", place.root_port + 1),
        Some((hub, port1)) => match dev_idx(state, hub) {
            Some(h) => format!("{}.{}", state.devs[h].seen.port, port1),
            None => format!("?.{}", port1),
        },
    }
}

fn speed_name(speed: u32) -> &'static str {
    match speed {
        SPEED_LOW => "low", SPEED_FULL => "full", SPEED_HIGH => "high", SPEED_SUPER => "super",
        5 => "super+", _ => "?",
    }
}

/// An interrupt OUT endpoint, for output reports (LEDs).
struct OutEp {
    dci: u8,
    ring: DmaRegion,
    cycle: u32,
    enqueue: usize,
}

/// The previous key set and key repeat of one keyboard interface.
struct KbdState {
    prev_keys: [u8; hid_core::input::KEYS_MAX],
    prev_len: usize,
    repeat_key: u8,
    repeat_shift: bool,
    repeat_altgr: bool,
    repeat_start: u64,
    repeat_last: u64,
}

impl KbdState {
    const fn new() -> Self {
        KbdState {
            prev_keys: [0; hid_core::input::KEYS_MAX], prev_len: 0,
            repeat_key: 0, repeat_shift: false, repeat_altgr: false,
            repeat_start: 0, repeat_last: 0,
        }
    }
    fn prev(&self) -> &[u8] { &self.prev_keys[..self.prev_len] }
}

/// One HID interface (Linux `usbhid_device` and its `hid_device`).
struct HidIntf {
    /// The device's slot.
    slot: u8,
    ifnum: u8,
    /// An application collection of this kind; decides what the kernel
    /// announces (`AVAILABLE`, `USB_POINTERS`).
    keyboard: bool,
    pointer: bool,
    in_dci: u8,
    in_ring: DmaRegion,
    in_cycle: u32,
    in_enqueue: usize,
    out: Option<OutEp>,
    /// Input report at 0, output report at `HID_OUT_OFF`.
    buf: DmaRegion,
    /// Longest input report with its ID byte (`usbhid_start`'s `insize`).
    insize: usize,
    /// The report copied out of `buf`; allocated here, filled in the
    /// interrupt, which must not allocate.
    report: Vec<u8>,
    hid: hid_core::Device,
    input: hid_core::input::Input,
    kbd: KbdState,
    error_count: u32,
    /// Transfers are queued. Cleared when the device is unplugged.
    running: bool,
}

/// Where the output report sits in `HidIntf::buf`.
const HID_OUT_OFF: u64 = 2048;
/// `HID_MAX_DESCRIPTOR_SIZE`.
const HID_MAX_DESCRIPTOR_SIZE: usize = 4096;
const HID_DT_HID: u8 = 0x21;
const HID_DT_REPORT: u8 = 0x22;
const HID_REQ_SET_IDLE: u8 = 0x0A;
const HID_REQ_SET_REPORT: u8 = 0x09;
const USB_CLASS_HID: u8 = 3;
const USB_INTERFACE_SUBCLASS_BOOT: u8 = 1;
const USB_INTERFACE_PROTOCOL_KEYBOARD: u8 = 1;

/// USB keyboards and pointers running, over all controllers.
static USB_KEYBOARDS: AtomicUsize = AtomicUsize::new(0);
static USB_POINTERS: AtomicUsize = AtomicUsize::new(0);

/// An endpoint from the configuration descriptor.
#[derive(Clone, Copy)]
struct EpDesc {
    addr: u8,
    /// wMaxPacketSize as given, with the high-speed transaction bits.
    max_pkt: u16,
    interval: u8,
}

/// A HID interface as the configuration descriptor describes it.
struct HidIntfDesc {
    ifnum: u8,
    subclass: u8,
    protocol: u8,
    /// `wDescriptorLength` of the report descriptor.
    report_len: usize,
    ep_in: EpDesc,
    ep_out: Option<EpDesc>,
}

/// The HID interfaces of a configuration, alternate setting 0 (the one
/// SET_CONFIGURATION selects). An interface without a HID descriptor or
/// without an interrupt IN endpoint is no HID device (`usbhid_parse`,
/// `usbhid_start`).
fn parse_hid_interfaces(cfg: &[u8]) -> Vec<HidIntfDesc> {
    struct Cur { ifnum: u8, alt: u8, class: u8, sub: u8, proto: u8,
                 report_len: Option<usize>, ep_in: Option<EpDesc>, ep_out: Option<EpDesc> }
    let mut out = Vec::new();
    let mut cur: Option<Cur> = None;
    let finish = |c: Cur, out: &mut Vec<HidIntfDesc>| {
        if c.alt != 0 || c.class != USB_CLASS_HID { return; }
        let (Some(report_len), Some(ep_in)) = (c.report_len, c.ep_in) else { return };
        out.push(HidIntfDesc { ifnum: c.ifnum, subclass: c.sub, protocol: c.proto,
            report_len, ep_in, ep_out: c.ep_out });
    };
    let mut pos = 0usize;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        let d = &cfg[pos..pos + len];
        match d[1] {
            4 if len >= 9 => {
                if let Some(c) = cur.take() { finish(c, &mut out); }
                cur = Some(Cur { ifnum: d[2], alt: d[3], class: d[5], sub: d[6], proto: d[7],
                                 report_len: None, ep_in: None, ep_out: None });
            }
            HID_DT_HID if len >= 9 => {
                if let Some(c) = cur.as_mut() {
                    // `usbhid_parse`: bNumDescriptors may disagree with
                    // bLength; only the first class descriptor counts.
                    if d[6] == HID_DT_REPORT && c.report_len.is_none() {
                        c.report_len = Some(u16::from_le_bytes([d[7], d[8]]) as usize);
                    }
                }
            }
            5 if len >= 7 => {
                if let Some(c) = cur.as_mut() {
                    let ep = EpDesc { addr: d[2], max_pkt: u16::from_le_bytes([d[4], d[5]]),
                                      interval: d[6] };
                    // Interrupt endpoints only; the first of each direction.
                    if d[3] & 0x03 == 3 {
                        if ep.addr & 0x80 != 0 {
                            if c.ep_in.is_none() { c.ep_in = Some(ep); }
                        } else if c.ep_out.is_none() {
                            c.ep_out = Some(ep);
                        }
                    }
                }
            }
            _ => {}
        }
        pos += len;
    }
    if let Some(c) = cur.take() { finish(c, &mut out); }
    out
}

/// Initial EP0 max packet size by speed — Linux `hub_port_init`. Full speed
/// starts at 64 and is corrected from the device descriptor; a full-speed
/// device with 64 driven at 8 sends packets the controller calls babble.
fn initial_mps0(speed: u32) -> u16 {
    match speed {
        SPEED_LOW => 8,
        SPEED_SUPER | 5 => 512,
        _ => 64,
    }
}

/// Address device `slot`, then read bMaxPacketSize0 and correct EP0 if it
/// differs (Linux `hub_port_init` with the new scheme: Address Device,
/// GET_DESCRIPTOR 8 bytes, `usb_ep0_reinit` → Evaluate Context).
fn address_device(state: &mut XhciState, slot: u8) -> bool {
    let Some(d) = dev_idx(state, slot) else { return false };
    let speed = state.devs[d].speed;
    let mps0 = initial_mps0(speed);
    if !cmd_address_device(state, slot, mps0) { return false; }
    if !usb_get_descriptor(state, slot, DESC_DEVICE, 8) { return false; }
    let b = state.data_buf.r8(7u32);
    let real = if speed >= SPEED_SUPER { 1u16 << b.min(15) } else { b as u16 };
    if real != 0 && real != mps0 {
        crate::kdebug!("[npk] xhci: slot {} ep0 max packet {} -> {}", slot, mps0, real);
        return cmd_evaluate_ep0(state, slot, real);
    }
    true
}

/// A zeroed DMA page: one given back by an unplugged device, else new.
fn take_page(state: &mut XhciState, name: &str) -> Option<DmaRegion> {
    match state.free_pages.pop() {
        Some(p) => { p.zero(); Some(p) }
        None => alloc_dma(1, name),
    }
}

/// A transfer ring on a fresh page: zeroed, Link TRB at the end.
fn take_ring(state: &mut XhciState, name: &str) -> Option<DmaRegion> {
    let r = take_page(state, name)?;
    write_trb(r, NUM_TR_TRBS - 1, r.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    Some(r)
}

/// `PORT_INIT_TRIES` (drivers/usb/core/hub.c): a device that fails reset,
/// addressing or its first descriptors is reset and tried again.
const PORT_INIT_TRIES: u32 = 4;

/// Enable a slot for a device of `speed` at `place`, address it and read
/// its device and configuration descriptors (the part of `hub_port_init`
/// after the reset, and `usb_get_configuration`). The device stays in
/// `devs` with its slot; None when a step failed, and then it is gone
/// again.
fn usb_port_init(state: &mut XhciState, place: Place, speed: u32) -> Option<u8> {
    let dctx = take_page(state, "usb dev ctx")?;
    let Some(ep0) = take_ring(state, "usb EP0") else {
        state.free_pages.push(dctx);
        return None;
    };
    let Some(slot) = cmd_enable_slot(state) else {
        kprintln!("[npk] xhci: enable slot failed");
        state.free_pages.push(dctx);
        state.free_pages.push(ep0);
        return None;
    };
    state.dcbaa.w64(slot as usize * 8, dctx.phys());
    let port = dev_path(state, place);
    state.devs.push(UsbDevice {
        slot, place, speed, dctx, ep0, ep0_cycle: 1, ep0_enqueue: 0,
        seen: UsbSeen {
            port, speed, device: Vec::new(), cfg: Vec::new(),
            manufacturer: String::new(), product: String::new(), failed: None,
        },
        hub: None,
    });

    let descs = (|| {
        if !address_device(state, slot) {
            port_failed(state, slot, "address device");
            return None;
        }
        if !usb_get_descriptor(state, slot, DESC_DEVICE, 18) {
            port_failed(state, slot, "device descriptor");
            return None;
        }
        let mut device = alloc::vec![0u8; 18];
        state.data_buf.copy_out(0u32, &mut device);
        // Configuration descriptor: 9 bytes for the total length, then all.
        if !usb_get_descriptor(state, slot, DESC_CONFIG, 9) {
            port_failed(state, slot, "configuration descriptor");
            return None;
        }
        let total = u16::from_le_bytes([state.data_buf.r8(2u32), state.data_buf.r8(3u32)]) as usize;
        if !usb_get_descriptor(state, slot, DESC_CONFIG, total as u16) {
            port_failed(state, slot, "configuration descriptor");
            return None;
        }
        let mut cfg = alloc::vec![0u8; total];
        state.data_buf.copy_out(0u32, &mut cfg);
        Some((device, cfg))
    })();
    match descs {
        Some((device, cfg)) => {
            let lang = if device[14] != 0 || device[15] != 0 { usb_lang(state, slot) } else { 0 };
            let manufacturer = usb_string(state, slot, lang, device[14]);
            let product = usb_string(state, slot, lang, device[15]);
            let d = dev_idx(state, slot)?;
            let s = &mut state.devs[d].seen;
            s.device = device;
            s.cfg = cfg;
            s.manufacturer = manufacturer;
            s.product = product;
            Some(slot)
        }
        None => {
            let Some(d) = dev_idx(state, slot) else { return None };
            let dev = state.devs.remove(d);
            usb_release_slot(state, slot);
            state.free_pages.push(dev.dctx);
            state.free_pages.push(dev.ep0);
            None
        }
    }
}

/// Which step of enumeration failed, with what the controller said.
fn port_failed(state: &mut XhciState, slot: u8, step: &'static str) {
    state.last_fail = step;
    let Some(d) = dev_idx(state, slot) else { return };
    let (place, speed) = (state.devs[d].place, state.devs[d].speed);
    let path = &state.devs[d].seen.port;
    let at = if place.parent.is_none() {
        format!("USB{}, {} speed, portsc {:#010x}", port_usb_version(state.mmio, place.root_port),
            speed_name(speed), state.oper.r32(portsc_off(place.root_port)))
    } else {
        format!("{} speed, behind a hub", speed_name(speed))
    };
    if state.last_cc == 0 {
        kprintln!("[npk] xhci: port {} {} timed out ({})", path, step, at);
    } else {
        kprintln!("[npk] xhci: port {} {} failed, completion code {} ({})", path, step, state.last_cc, at);
    }
}

/// `hub_port_disable` for a root port: a USB 3 port's link goes to U3, a
/// USB 2 port is disabled (PED, write 1 to clear). A device that would not
/// enumerate then stays quiet until it is plugged in anew.
fn hub_port_disable(state: &XhciState, port: u32) {
    let off = portsc_off(port);
    let sc = state.oper.r32(off);
    if port_usb_version(state.mmio, port) >= 3 {
        const PORT_LINK_STROBE: u32 = 1 << 16;
        const PLS_U3: u32 = 3 << 5;
        state.oper.w32(off, (port_neutral(sc) & !(0xF << 5)) | PORT_LINK_STROBE | PLS_U3);
    } else if sc & PORTSC_PED != 0 {
        state.oper.w32(off, port_neutral(sc) | PORTSC_PED);
    }
}

/// Disable Slot, and the slot's DCBAA entry cleared (`xhci_free_virt_device`).
fn usb_release_slot(state: &mut XhciState, slot: u8) {
    cmd_disable_slot(state, slot);
    state.dcbaa.w64(slot as usize * 8, 0);
}

/// `hub_port_connect` for a root port: reset, enumerate, hand the device
/// to its driver; tried `PORT_INIT_TRIES` times.
fn port_connect_root(state: &mut XhciState, port: u32) {
    let place = Place::root(port);
    let mut slot = None;
    let mut speed = 0;
    for attempt in 0..PORT_INIT_TRIES {
        if attempt > 0 {
            kprintln!("[npk] xhci: port {} retrying ({}/{})", port + 1, attempt + 1, PORT_INIT_TRIES);
        }
        if reset_port(state, port) {
            speed = (state.oper.r32(portsc_off(port)) >> 10) & 0xF;
            slot = usb_port_init(state, place, speed);
            if slot.is_some() { break; }
        } else {
            kprintln!("[npk] xhci: port {} reset failed", port + 1);
        }
        if state.oper.r32(portsc_off(port)) & PORTSC_CCS == 0 { return; }
    }
    match slot {
        Some(slot) => usb_new_device(state, slot),
        None => {
            kprintln!("[npk] xhci: port {}: unable to enumerate USB device, port disabled", port + 1);
            enumeration_failed(state, format!("{}", port + 1), speed);
            hub_port_disable(state, port);
        }
    }
}

/// Keep a device enumeration gave up on, for `usb`.
fn enumeration_failed(state: &mut XhciState, port: String, speed: u32) {
    state.failed.retain(|s| s.port != port);
    state.failed.push(UsbSeen {
        port, speed, device: Vec::new(), cfg: Vec::new(),
        manufacturer: String::new(), product: String::new(),
        failed: Some((state.last_fail, state.last_cc)),
    });
}

/// `usb_new_device` and the driver match after it: a hub goes to the hub
/// driver, HID interfaces to usbhid. Anything else stays addressed for a
/// driver that asks for it (`nic_attach`).
fn usb_new_device(state: &mut XhciState, slot: u8) {
    let Some(d) = dev_idx(state, slot) else { return };
    let (device, cfg) = (state.devs[d].seen.device.clone(), state.devs[d].seen.cfg.clone());
    if device.get(4) == Some(&USB_CLASS_HUB) || hub_interface(&cfg).is_some() {
        hub_probe(state, slot, &device, &cfg);
        return;
    }
    hid_probe(state, slot, &cfg);
}

/// Set up every HID interface of device `slot` (`usbhid_probe` for each).
fn hid_probe(state: &mut XhciState, slot: u8, cfg: &[u8]) {
    let descs = parse_hid_interfaces(cfg);
    if descs.is_empty() {
        crate::kdebug!("[npk] xhci: slot {} has no HID interface", slot);
        return;
    }
    let config_val = cfg.get(5).copied().unwrap_or(1);

    // Rings and buffers for every interface, then all endpoints in one
    // Configure Endpoint, then SET_CONFIGURATION — the order of Linux
    // `usb_set_configuration` (bandwidth first, request after).
    let first = state.hid.len();
    let mut eps: Vec<(u8, u64, u32, u16, u8)> = Vec::new();
    for d in &descs {
        let (Some(in_ring), Some(buf)) = (take_ring(state, "hid in"), take_page(state, "hid buf")) else {
            kprintln!("[npk] xhci: DMA alloc failed");
            break;
        };
        let in_dci = (d.ep_in.addr & 0x0F) * 2 + 1;
        eps.push((in_dci, in_ring.phys(), EP_TYPE_INTERRUPT_IN, d.ep_in.max_pkt, d.ep_in.interval));
        let out = match d.ep_out {
            Some(o) => take_ring(state, "hid out").map(|ring| {
                let dci = (o.addr & 0x0F) * 2;
                eps.push((dci, ring.phys(), EP_TYPE_INTERRUPT_OUT, o.max_pkt, o.interval));
                OutEp { dci, ring, cycle: 1, enqueue: 0 }
            }),
            None => None,
        };
        state.hid.push(HidIntf {
            slot, ifnum: d.ifnum, keyboard: false, pointer: false,
            in_dci, in_ring, in_cycle: 1, in_enqueue: 0, out, buf,
            insize: 0, report: Vec::new(), hid: hid_core::Device::default(),
            input: hid_core::input::Input::default(), kbd: KbdState::new(),
            error_count: 0, running: false,
        });
    }
    let path = dev_idx(state, slot).map(|d| state.devs[d].seen.port.clone()).unwrap_or_default();
    if !cmd_configure_endpoints(state, slot, &eps) {
        kprintln!("[npk] xhci: port {} configure endpoints failed", path);
        return;
    }
    if !usb_set_config(state, slot, config_val) {
        kprintln!("[npk] xhci: port {} set configuration failed", path);
        return;
    }

    for (n, d) in descs.iter().enumerate() {
        let i = first + n;
        if i >= state.hid.len() { break; }
        if !usbhid_parse(state, i, d) { continue; }
        usbhid_start(state, i, d);
    }
}

/// The device `slot` is gone (`usb_disconnect`): first every device below
/// it if it is a hub, then its interfaces stop, its slot is released, its
/// pages go back to the pool.
fn usb_disconnect(state: &mut XhciState, slot: u8) {
    let children: Vec<u8> = state.devs.iter()
        .filter(|d| d.place.parent.is_some_and(|(hub, _)| hub == slot))
        .map(|d| d.slot)
        .collect();
    for c in children {
        usb_disconnect(state, c);
    }
    let Some(d) = dev_idx(state, slot) else { return };
    let mut pages = Vec::new();
    for h in state.hid.iter_mut().filter(|h| h.slot == slot) {
        hid_stop(h);
        pages.push(h.in_ring);
        pages.push(h.buf);
        if let Some(o) = h.out.as_ref() { pages.push(o.ring); }
    }
    state.hid.retain(|h| h.slot != slot);
    if state.nic.as_ref().is_some_and(|n| n.slot_id == slot) {
        kprintln!("[npk] xhci: the network device is gone");
        state.nic = None;
    }
    let dev = state.devs.remove(d);
    if let Some(h) = dev.hub {
        pages.push(h.ring);
        pages.push(h.buf);
    }
    usb_release_slot(state, slot);
    state.free_pages.extend(pages);
    state.free_pages.push(dev.dctx);
    state.free_pages.push(dev.ep0);
    kprintln!("[npk] xhci: USB device on port {} gone", dev.seen.port);
}

// === Hotplug — Linux hub_event / hub_port_connect_change ===
//
// The controller reports every connect and disconnect as a Port Status
// Change Event. The interrupt only notes the port; the shell's idle step
// (`hotplug`) debounces it and enumerates or tears down, since that waits
// for the device's answers and must not run in the interrupt.

/// A port changed somewhere; `hotplug` has work.
static HOTPLUG: AtomicBool = AtomicBool::new(false);

const HUB_DEBOUNCE_TIMEOUT: u64 = 2000;
const HUB_DEBOUNCE_STEP: u64 = 25;
const HUB_DEBOUNCE_STABLE: u64 = 100;

/// Note a Port Status Change Event (port ID in bits 31:24).
fn note_port_change(state: &mut XhciState, e: &Evt) {
    let port1 = ((e.param as u32) >> 24) as usize;
    if port1 == 0 || port1 > 256 { return; }
    let p = port1 - 1;
    state.port_changes[p / 64] |= 1 << (p % 64);
    HOTPLUG.store(true, Ordering::Release);
}

/// The change bits of PORTSC (write 1 to clear).
const PORTSC_CHANGES: u32 = PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_PRC | PORTSC_PLC;

/// `hub_port_debounce`: the connection status once it held for
/// `HUB_DEBOUNCE_STABLE` ms, or None if it never settled.
fn hub_port_debounce_root(oper: Mmio, port: u32) -> Option<bool> {
    let off = portsc_off(port);
    let mut connection: Option<bool> = None;
    let mut stable = 0u64;
    let mut total = 0u64;
    loop {
        let sc = oper.r32(off);
        let now = sc & PORTSC_CCS != 0;
        if sc & PORTSC_CSC == 0 && connection == Some(now) {
            stable += HUB_DEBOUNCE_STEP;
            if stable >= HUB_DEBOUNCE_STABLE { return Some(now); }
        } else {
            stable = 0;
            connection = Some(now);
        }
        if sc & PORTSC_CSC != 0 {
            oper.w32(off, port_neutral(sc) | PORTSC_CSC);
        }
        if total >= HUB_DEBOUNCE_TIMEOUT { return None; }
        crate::interrupts::delay_ms(HUB_DEBOUNCE_STEP);
        total += HUB_DEBOUNCE_STEP;
    }
}

/// A hotplug task is queued or running.
static HOTPLUG_RUNNING: AtomicBool = AtomicBool::new(false);

/// From the shell's idle step: hand pending port changes to a worker. The
/// handling waits on devices for seconds when one misbehaves; the shell's
/// core must not wait with it. Events of other devices keep flowing: the
/// waits drain the event ring themselves.
pub fn hotplug() {
    if !HOTPLUG.load(Ordering::Acquire) { return; }
    if HOTPLUG_RUNNING.swap(true, Ordering::AcqRel) { return; }
    crate::smp::scheduler::spawn("usb hotplug", hotplug_task, 0);
}

fn hotplug_task(_: u64) {
    loop {
        hotplug_pass();
        HOTPLUG_RUNNING.store(false, Ordering::Release);
        // A change noted after this pass looked: take it, unless another
        // task already did.
        if !HOTPLUG.load(Ordering::Acquire) || HOTPLUG_RUNNING.swap(true, Ordering::AcqRel) {
            return;
        }
    }
}

/// Handle the ports that changed since the last pass: a device that came
/// is enumerated, one that went is torn down; then the hubs' ports.
fn hotplug_pass() {
    if !HOTPLUG.swap(false, Ordering::AcqRel) { return; }
    // Which ports, and whether each saw a connect change. Read and cleared
    // under the lock; the debounce waits outside it, so the interrupt keeps
    // draining every other device meanwhile.
    let mut work: Vec<(pci::PciAddr, Mmio, u32, bool)> = Vec::new();
    {
        let mut g = CTRLS.lock();
        for s in g.iter_mut() {
            for w in 0..4 {
                let mut bits = core::mem::take(&mut s.port_changes[w]);
                while bits != 0 {
                    let p = (w * 64) as u32 + bits.trailing_zeros();
                    bits &= bits - 1;
                    if p >= s.max_ports { continue; }
                    let off = portsc_off(p);
                    let sc = s.oper.r32(off);
                    if sc & PORTSC_CHANGES != 0 {
                        s.oper.w32(off, port_neutral(sc) | (sc & PORTSC_CHANGES));
                    }
                    let path = format!("{}", p + 1);
                    let gone = sc & PORTSC_CCS == 0
                        && (s.devs.iter().any(|d| d.place.parent.is_none() && d.place.root_port == p)
                            || s.failed.iter().any(|d| d.port == path));
                    if sc & PORTSC_CSC != 0 || gone {
                        work.push((s.pci_addr, s.oper, p, sc & PORTSC_CSC != 0));
                    }
                }
            }
        }
    }
    for (addr, oper, port, changed) in work {
        let connected = if changed {
            hub_port_debounce_root(oper, port).unwrap_or(false)
        } else {
            false
        };
        let mut g = CTRLS.lock();
        let Some(s) = g.iter_mut().find(|s| s.pci_addr == addr) else { continue };
        let root = s.devs.iter().find(|d| d.place.parent.is_none() && d.place.root_port == port).map(|d| d.slot);
        if let Some(slot) = root {
            usb_disconnect(s, slot);
        }
        let path = format!("{}", port + 1);
        s.failed.retain(|d| d.port != path);
        if connected {
            kprintln!("[npk] xhci: device connected on port {}", port + 1);
            port_connect_root(s, port);
        }
    }
    let mut g = CTRLS.lock();
    for s in g.iter_mut() {
        hub_events(s);
    }
}

/// `usbhid_parse`: SET_IDLE, then the report descriptor, parsed.
fn usbhid_parse(state: &mut XhciState, i: usize, d: &HidIntfDesc) -> bool {
    let ifnum = d.ifnum as u16;
    let slot = state.hid[i].slot;
    let rsize = d.report_len;
    if rsize == 0 || rsize > HID_MAX_DESCRIPTOR_SIZE {
        kprintln!("[npk] xhci: hid interface {}: weird size of report descriptor ({})", ifnum, rsize);
        return false;
    }
    let _ = usb_control_transfer(state, slot, 0x21, HID_REQ_SET_IDLE, 0, ifnum, 0, false);

    // `hid_get_class_descriptor`: up to four tries.
    let mut ok = false;
    for _ in 0..4 {
        state.data_buf.fill(0u32, 0, rsize as u64);
        if usb_control_transfer(state, slot, 0x81, USB_GET_DESCRIPTOR, (HID_DT_REPORT as u16) << 8,
            ifnum, rsize as u16, true)
        {
            ok = true;
            break;
        }
    }
    if !ok {
        kprintln!("[npk] xhci: hid interface {}: reading report descriptor failed", ifnum);
        return false;
    }
    let mut rdesc = alloc::vec![0u8; rsize];
    state.data_buf.copy_out(0u32, &mut rdesc);
    match hid_core::parse(&rdesc) {
        Ok(dev) => {
            let h = &mut state.hid[i];
            for app in dev.applications() {
                match app {
                    hid_core::GD_KEYBOARD => h.keyboard = true,
                    hid_core::GD_MOUSE | hid_core::GD_POINTER => h.pointer = true,
                    _ => {}
                }
            }
            h.hid = dev;
            true
        }
        Err(e) => {
            kprintln!("[npk] xhci: hid interface {}: parsing report descriptor failed: {:?}", ifnum, e);
            false
        }
    }
}

/// `usbhid_start`, then `hid_start_in`: size the input buffer, clear the
/// LEDs of a boot keyboard, queue the first IN transfer.
fn usbhid_start(state: &mut XhciState, i: usize, d: &HidIntfDesc) {
    let insize = state.hid[i].hid.max_report_bytes(hid_core::ReportType::Input)
        .min(HID_OUT_OFF as usize);
    if insize == 0 {
        crate::kdebug!("[npk] xhci: hid interface {}: no input reports", d.ifnum);
        return;
    }
    state.hid[i].insize = insize;
    state.hid[i].report = alloc::vec![0u8; insize];

    // Some keyboards do not work until their LEDs have been set.
    if d.subclass == USB_INTERFACE_SUBCLASS_BOOT && d.protocol == USB_INTERFACE_PROTOCOL_KEYBOARD {
        usbhid_set_leds(state, i);
    }

    let h = &mut state.hid[i];
    h.running = true;
    let (kbd, ptr) = (h.keyboard, h.pointer);
    if kbd {
        USB_KEYBOARDS.fetch_add(1, Ordering::Relaxed);
        AVAILABLE.store(true, Ordering::Relaxed);
    }
    if ptr {
        USB_POINTERS.fetch_add(1, Ordering::Relaxed);
        MOUSE_AVAILABLE.store(true, Ordering::Relaxed);
    }
    let slot = state.hid[i].slot;
    let path = dev_idx(state, slot).map(|x| state.devs[x].seen.port.as_str()).unwrap_or("?");
    kprintln!("[npk] xhci: USB HID on {:02x}:{:02x}.{} port {} interface {}{}{}",
        state.pci_addr.bus, state.pci_addr.device, state.pci_addr.function,
        path, d.ifnum,
        if kbd { " keyboard" } else { "" }, if ptr { " pointer" } else { "" });
    schedule_hid_in(state, i);
}

/// `usbhid_set_leds`: the output report holding NumLock, all zero — over
/// the interrupt OUT endpoint if there is one, else SET_REPORT.
fn usbhid_set_leds(state: &mut XhciState, i: usize) {
    let h = &state.hid[i];
    let Some(id) = h.hid.find_output_usage(hid_core::UP_LED | 0x01) else { return };
    let Some(report) = h.hid.output_report_zeroed(id) else { return };
    let ifnum = h.ifnum as u16;
    if report.len() > HID_OUT_OFF as usize { return; }
    if h.out.is_some() {
        h.buf.copy_in(HID_OUT_OFF, &report);
        let len = report.len() as u32;
        let slot = h.slot as u32;
        let buf = h.buf.phys() + HID_OUT_OFF;
        let Some(o) = state.hid[i].out.as_mut() else { return };
        write_trb(o.ring, o.enqueue, buf, len, TRB_NORMAL | TRB_IOC | o.cycle);
        o.enqueue += 1;
        if o.enqueue >= NUM_TR_TRBS - 1 {
            write_trb(o.ring, NUM_TR_TRBS - 1, o.ring.phys(), 0, TRB_LINK | o.cycle | (1 << 1));
            o.cycle ^= 1;
            o.enqueue = 0;
        }
        let dci = o.dci as u32;
        ring_doorbell(state, slot, dci);
    } else {
        let slot = h.slot;
        state.data_buf.copy_in(0u32, &report);
        let _ = usb_control_transfer(state, slot, 0x21, HID_REQ_SET_REPORT, (2u16 << 8) | id as u16,
            ifnum, report.len() as u16, false);
    }
}

/// Queue the next IN transfer of interface `i` (`hid_start_in`).
fn schedule_hid_in(state: &mut XhciState, i: usize) {
    let slot = state.hid[i].slot as u32;
    let h = &mut state.hid[i];
    write_trb(h.in_ring, h.in_enqueue, h.buf.phys(), h.insize as u32,
        TRB_NORMAL | TRB_IOC | h.in_cycle);
    h.in_enqueue += 1;
    if h.in_enqueue >= NUM_TR_TRBS - 1 {
        write_trb(h.in_ring, NUM_TR_TRBS - 1, h.in_ring.phys(), 0, TRB_LINK | h.in_cycle | (1 << 1));
        h.in_cycle ^= 1;
        h.in_enqueue = 0;
    }
    let dci = h.in_dci as u32;
    ring_doorbell(state, slot, dci);
}

/// `hid_irq_in`: a completed IN transfer of interface `i`.
fn hid_irq_in(state: &mut XhciState, i: usize, e: &Evt) {
    let ok = e.cc == CC_SUCCESS || e.cc == CC_SHORT_PACKET;
    if ok {
        state.hid[i].error_count = 0;
        let h = &mut state.hid[i];
        let len = h.insize.saturating_sub(e.residual as usize);
        let HidIntf { buf, report, hid, input, .. } = &mut *h;
        buf.copy_out(0u32, &mut report[..len]);
        let _ = hid.input_report(&report[..len], &mut |ev| input.event(&ev));
        let frame = input.sync();
        hid_deliver(h, &frame);
    } else if e.cc == 19 {
        // Missed Service Error: the CPU was busy; nothing is lost but time.
    } else {
        state.hid[i].error_count += 1;
        let n = state.hid[i].error_count;
        // Unplugged: do not re-queue. A root port says so; behind a hub
        // the hub reports it, and the errors only stop meanwhile.
        let root = dev_idx(state, state.hid[i].slot)
            .map(|d| state.devs[d].place)
            .filter(|p| p.parent.is_none())
            .map(|p| p.root_port);
        let gone = match root {
            Some(port) => n >= 3 && state.oper.r32(portsc_off(port)) & PORTSC_CCS == 0,
            None => n >= HUB_IRQ_ERRORS,
        };
        if gone {
            hid_stop(&mut state.hid[i]);
            return;
        }
    }
    schedule_hid_in(state, i);
}

/// The interface is gone: it no longer counts as a keyboard or pointer.
fn hid_stop(h: &mut HidIntf) {
    if !h.running { return; }
    h.running = false;
    // Whatever it held is let go: a key would repeat for ever, a button
    // would keep dragging.
    if h.kbd.repeat_key != 0 && REPEAT_KEY.load(Ordering::Relaxed) == h.kbd.repeat_key {
        REPEAT_KEY.store(0, Ordering::Relaxed);
    }
    if h.keyboard && h.input.modifiers() != 0 {
        crate::keyboard::set_super(false);
        crate::keyboard::set_shift(false);
        crate::keyboard::set_ctrl(false);
    }
    if h.pointer && h.input.buttons != 0 {
        inject_pointer(MouseEvent { buttons: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0 }, true);
    }
    if h.keyboard && USB_KEYBOARDS.fetch_sub(1, Ordering::Relaxed) == 1 {
        AVAILABLE.store(false, Ordering::Relaxed);
    }
    if h.pointer && USB_POINTERS.fetch_sub(1, Ordering::Relaxed) == 1 {
        MOUSE_AVAILABLE.store(false, Ordering::Relaxed);
    }
}

/// Forget every HID interface of a controller about to be dropped.
fn hid_forget(state: &mut XhciState) {
    for h in state.hid.iter_mut() { hid_stop(h); }
}

/// Hand what one report changed to the kernel (`input_sync`).
fn hid_deliver(h: &mut HidIntf, f: &hid_core::input::Frame) {
    if f.keys_changed {
        let mut keys = [0u8; hid_core::input::KEYS_MAX];
        let mut n = 0;
        for k in h.input.non_modifier_keys() {
            keys[n] = k;
            n += 1;
        }
        let mods = h.input.modifiers();
        process_hid_report(mods, &keys[..n], &mut h.kbd);
        h.kbd.prev_keys[..n].copy_from_slice(&keys[..n]);
        h.kbd.prev_len = n;
    }

    if f.buttons_changed || f.dx != 0 || f.dy != 0 || f.wheel != 0 || f.hwheel != 0 {
        // MouseEvent carries i8 deltas; a 12- or 16-bit report is split.
        let (mut dx, mut dy, mut wheel, mut hwheel) = (f.dx, f.dy, f.wheel, f.hwheel);
        loop {
            let step = |v: i32| v.clamp(-127, 127);
            let (sx, sy, sw, sh) = (step(dx), step(dy), step(wheel), step(hwheel));
            inject_pointer(MouseEvent {
                buttons: h.input.buttons, dx: sx as i8, dy: sy as i8,
                scroll: sw as i8, hscroll: sh as i8,
            }, true);
            dx -= sx; dy -= sy; wheel -= sw; hwheel -= sh;
            if dx == 0 && dy == 0 && wheel == 0 && hwheel == 0 { break; }
        }
    }

    for m in f.media.as_slice() {
        match m {
            hid_core::input::Media::Mute => crate::keyboard::toggle_mute(),
            hid_core::input::Media::VolumeUp => crate::keyboard::step_volume(5),
            hid_core::input::Media::VolumeDown => crate::keyboard::step_volume(-5),
            hid_core::input::Media::BrightnessUp => crate::keyboard::step_brightness(5),
            hid_core::input::Media::BrightnessDown => crate::keyboard::step_brightness(-5),
        }
    }
    for &u in f.unmapped.as_slice() { note_unmapped(u); }
}

/// Usages pressed that nothing maps, each logged once. Recorded in the
/// interrupt, printed by `log_unmapped` from the shell's idle step.
const UNMAPPED_SLOTS: usize = 16;
static UNMAPPED: [AtomicU64; UNMAPPED_SLOTS] = [const { AtomicU64::new(0) }; UNMAPPED_SLOTS];
/// Set in a slot once it has been printed.
const UNMAPPED_LOGGED: u64 = 1 << 32;
/// Set in a slot that holds a usage (usage 0 is a valid number).
const UNMAPPED_USED: u64 = 1 << 33;

fn note_unmapped(usage: u32) {
    for s in UNMAPPED.iter() {
        let v = s.load(Ordering::Relaxed);
        if v & UNMAPPED_USED != 0 {
            if v as u32 == usage { return; }
            continue;
        }
        if s.compare_exchange(0, UNMAPPED_USED | usage as u64, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            return;
        }
    }
}

/// Print usages seen since the last call.
pub fn log_unmapped() {
    for s in UNMAPPED.iter() {
        let v = s.load(Ordering::Relaxed);
        if v & UNMAPPED_USED == 0 || v & UNMAPPED_LOGGED != 0 { continue; }
        s.fetch_or(UNMAPPED_LOGGED, Ordering::Relaxed);
        kprintln!("[npk] usb hid: unmapped usage {:#010x}", v as u32);
    }
}

// === Helper functions ===

fn alloc_dma(pages: usize, _name: &str) -> Option<DmaRegion> {
    DmaRegion::alloc_zeroed(pages)
}


fn wait_for(base: Mmio, reg: u32, mask: u32, expected: u32) -> bool {
    // Tick-based timeout (500ms) — CPU-speed independent
    let deadline = crate::interrupts::ticks() + 50; // 50 ticks = 500ms at 100Hz
    loop {
        if base.r32(reg) & mask == expected { return true; }
        if crate::interrupts::ticks() >= deadline { return false; }
        core::hint::spin_loop();
    }
}

fn portsc_off(port: u32) -> u32 {
    0x400 + port * 0x10
}

fn write_trb(ring: DmaRegion, idx: usize, param: u64, status: u32, control: u32) {
    let off = idx * 16;
    ring.w32(off, param as u32);
    ring.w32(off + 4, (param >> 32) as u32);
    ring.w32(off + 8, status);
    fence(Ordering::SeqCst);
    ring.w32(off + 12, control);
}

fn read_trb(ring: DmaRegion, idx: usize) -> (u64, u32, u32) {
    let off = idx * 16;
    let lo = ring.r32(off) as u64;
    let hi = ring.r32(off + 4) as u64;
    let status = ring.r32(off + 8);
    let control = ring.r32(off + 12);
    (lo | (hi << 32), status, control)
}

fn ring_doorbell(state: &XhciState, slot: u32, target: u32) {
    fence(Ordering::SeqCst);
    state.db.w32(slot * 4, target);
}

fn bios_handoff(mmio: Mmio, mut off: u32) {
    // Walk extended capability list to find USB Legacy Support (ID=1)
    for _ in 0..100 {
        if off as u64 + 8 > mmio.len() { break; }
        let cap = mmio.r32(off);
        let id = cap & 0xFF;
        if id == 1 {
            // Found USB Legacy Support capability
            // Set OS Owned Semaphore (bit 24)
            mmio.w32(off, cap | (1 << 24));
            // Wait for BIOS Owned Semaphore (bit 16) to clear (1s timeout)
            let deadline = crate::interrupts::ticks() + 100;
            while mmio.r32(off) & (1 << 16) != 0 {
                if crate::interrupts::ticks() >= deadline { break; }
                core::hint::spin_loop();
            }
            // Disable SMI (clear USBLEGCTLSTS enable bits)
            let ctl_off = off + 4;
            mmio.w32(ctl_off, mmio.r32(ctl_off) & 0x0000_001F); // keep RO/RW1C, clear enables
            return;
        }
        let next = (cap >> 8) & 0xFF;
        if next == 0 { break; }
        off += next * 4;
    }
}

/// The first language of the device's strings (string descriptor 0), US
/// English when it has none (Linux `usb_get_langid`).
fn usb_lang(state: &mut XhciState, slot: u8) -> u16 {
    if usb_control_transfer(state, slot, 0x80, USB_GET_DESCRIPTOR, 0x0300, 0, 255, true)
        && state.data_buf.r8(0u32) >= 4 && state.data_buf.r8(1u32) == 0x03
    {
        let lang = u16::from_le_bytes([state.data_buf.r8(2u32), state.data_buf.r8(3u32)]);
        if lang != 0 { return lang; }
    }
    0x0409
}

/// String descriptor `idx` in `lang` (Linux `usb_string`), empty when the
/// device has none or does not answer.
fn usb_string(state: &mut XhciState, slot: u8, lang: u16, idx: u8) -> String {
    if idx == 0 { return String::new(); }
    if !usb_control_transfer(state, slot, 0x80, USB_GET_DESCRIPTOR, 0x0300 | idx as u16, lang, 255, true) {
        return String::new();
    }
    let blen = state.data_buf.r8(0u32) as usize;
    if blen < 2 || state.data_buf.r8(1u32) != 0x03 { return String::new(); }
    let units = (1..blen / 2).map(|i| {
        u16::from_le_bytes([state.data_buf.r8((2 * i) as u32), state.data_buf.r8((2 * i + 1) as u32)])
    });
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .into()
}

/// A device as enumeration met it.
#[derive(Clone)]
pub struct UsbSeen {
    /// Its path: the root port, then the hub port of each tier ("3.1.4").
    pub port: String,
    /// PORTSC speed: 1 full, 2 low, 3 high, 4 super, 5 super+.
    pub speed: u32,
    /// The 18-byte device descriptor; empty when enumeration failed.
    pub device: Vec<u8>,
    /// The whole configuration descriptor.
    pub cfg: Vec<u8>,
    pub manufacturer: String,
    pub product: String,
    /// The step that failed and the completion code (0 = timed out).
    pub failed: Option<(&'static str, u32)>,
}

/// One entry of `devices`.
pub struct UsbListed {
    pub ctrl: pci::PciAddr,
    /// What took the device: `hid`, `hub`, `net`, or empty.
    pub driver: &'static str,
    pub dev: UsbSeen,
}

/// Every device on every controller, copied out of the running state.
/// Nothing is sent to a device.
pub fn devices() -> Vec<UsbListed> {
    let g = CTRLS.lock();
    let mut out = Vec::new();
    for s in g.iter() {
        for d in &s.devs {
            let driver = if d.hub.is_some() { "hub" }
                else if s.hid.iter().any(|h| h.slot == d.slot) { "hid" }
                else if s.nic.as_ref().is_some_and(|n| n.slot_id == d.slot) { "net" }
                else { "" };
            out.push(UsbListed { ctrl: s.pci_addr, driver, dev: d.seen.clone() });
        }
        for f in &s.failed {
            out.push(UsbListed { ctrl: s.pci_addr, driver: "", dev: f.clone() });
        }
    }
    out
}

fn reset_port(state: &XhciState, port: u32) -> bool {
    let off = portsc_off(port);
    // Linux SetPortFeature(PORT_RESET): neutral state + PR.
    let sc = state.oper.r32(off);
    state.oper.w32(off, port_neutral(sc) | PORTSC_PR);

    // Done when PR has self-cleared and PRC is set (hub_port_wait_reset) —
    // not merely when PED reads 1, which an enabled port still does in the
    // instant before the controller starts the reset. 500 ms timeout.
    let deadline = crate::interrupts::ticks() + 50;
    loop {
        let sc = state.oper.r32(off);
        if sc & PORTSC_PR == 0 && sc & PORTSC_PRC != 0 {
            state.oper.w32(off, port_neutral(sc) | PORTSC_PRC);
            return sc & PORTSC_PED != 0;
        }
        if crate::interrupts::ticks() >= deadline { break; }
        core::hint::spin_loop();
    }
    false
}

// === Command Ring operations ===

fn post_command(state: &mut XhciState, param: u64, status: u32, mut control: u32) {
    control = (control & !TRB_CYCLE) | state.cmd_cycle;
    write_trb(state.cmd_ring, state.cmd_enqueue, param, status, control);
    state.cmd_enqueue += 1;
    if state.cmd_enqueue >= NUM_CMD_TRBS - 1 {
        // Wrap: update Link TRB cycle bit and reset enqueue
        let link_ctrl = TRB_LINK | state.cmd_cycle | (1 << 1); // Toggle Cycle
        write_trb(state.cmd_ring, NUM_CMD_TRBS - 1, state.cmd_ring.phys(), 0, link_ctrl);
        state.cmd_cycle ^= 1;
        state.cmd_enqueue = 0;
    }
    ring_doorbell(state, 0, 0); // HC doorbell
}

/// Wait for a command completion and dispatch every other event that
/// arrives meanwhile. Dropping a transfer event would leave its transfer
/// never re-queued, silencing that device.
fn wait_command_completion(state: &mut XhciState) -> Option<(u32, u32)> {
    let deadline = crate::interrupts::ticks() + 100;
    state.last_cc = 0;
    loop {
        if crate::interrupts::ticks() >= deadline { return None; }
        let e = match next_event(state) {
            Some(e) => e,
            None => { core::hint::spin_loop(); continue; }
        };
        flush_erdp(state);
        if e.trb_type == EVT_CMD_COMPLETE {
            state.last_cc = e.cc;
            return Some((e.cc, (e.control >> 24) & 0xFF));
        }
        if e.trb_type == EVT_TRANSFER {
            dispatch_transfer(state, &e);
        } else if e.trb_type == EVT_PORT_STATUS {
            note_port_change(state, &e);
        }
    }
}

fn cmd_enable_slot(state: &mut XhciState) -> Option<u8> {
    post_command(state, 0, 0, TRB_ENABLE_SLOT);
    let (cc, slot) = wait_command_completion(state)?;
    if cc != CC_SUCCESS { return None; }
    Some(slot as u8)
}

fn cmd_disable_slot(state: &mut XhciState, slot_id: u8) {
    let slot_field = (slot_id as u32) << 24;
    post_command(state, 0, 0, TRB_DISABLE_SLOT | slot_field);
    let _ = wait_command_completion(state); // best effort
    // Clear DCBAA entry
    state.dcbaa.w64(slot_id as usize * 8, 0);
}

/// Address Device for `slot` (`xhci_setup_addressable_virt_dev`): its
/// speed, route string, root port and TT in the slot context, EP0 on its
/// own ring.
fn cmd_address_device(state: &mut XhciState, slot: u8, max_packet: u16) -> bool {
    let Some(d) = dev_idx(state, slot) else { return false };
    let (place, speed, ring) = (state.devs[d].place, state.devs[d].speed, state.devs[d].ep0.phys());
    let ctx = state.ctx_size;
    let input = state.input_ctx;

    input.fill(0u32, 0, 4096);

    // Input Control Context: Add Slot (bit 0) + EP0 (bit 1)
    input.w32(4u32, 0x03); // Add flags at offset 4

    // Slot Context (at input + ctx_size * 1): Context Entries = 1, speed,
    // route string; MTT for a device behind a multi-TT hub.
    let slot_off = ctx;
    let mtt = if place.tt.is_some_and(|(_, _, multi)| multi) { 1 << 25 } else { 0 };
    input.w32(slot_off, (1 << 27) | (speed << 20) | mtt | (place.route & 0xF_FFFF));
    // Dword 1: Root Hub Port Number (1-based)
    input.w32(slot_off + 4, (place.root_port + 1) << 16);
    // Dword 2: the TT hub slot and port of a low or full speed device
    // behind a high speed hub.
    if let Some((tt_slot, tt_port, _)) = place.tt {
        input.w32(slot_off + 8, tt_slot as u32 | ((tt_port as u32) << 8));
    }

    // EP0 Context (at input + ctx_size * 2)
    let ep0_off = ctx * 2;
    let ep_type_mps = (EP_TYPE_CONTROL << 3) | (3 << 1); // CErr=3, EP Type=Control
    let mps_field = (max_packet as u32) << 16;
    // Dword 1: CErr + EP Type
    input.w32(ep0_off + 4, ep_type_mps | mps_field);
    // Dword 2-3: TR Dequeue Pointer (with DCS=1)
    input.w32(ep0_off + 8, (ring as u32) | 1);
    input.w32(ep0_off + 12, (ring >> 32) as u32);
    // Dword 4: Average TRB Length
    input.w32(ep0_off + 16, 8);

    post_command(state, input.phys(), 0, TRB_ADDRESS_DEVICE | ((slot as u32) << 24));
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

// === USB Control Transfers ===

/// Largest data stage: `wLength` is 16 bits.
const MAX_CONTROL_LEN: usize = 65535;

/// A control transfer on device `slot`'s EP0, the data stage in
/// `data_buf`.
fn usb_control_transfer(state: &mut XhciState, slot: u8, bm_request: u8, b_request: u8,
    w_value: u16, w_index: u16, w_length: u16, dir_in: bool) -> bool
{
    let Some(d) = dev_idx(state, slot) else { return false };
    let ring = state.devs[d].ep0;
    let mut cycle = state.devs[d].ep0_cycle;
    let mut enq = state.devs[d].ep0_enqueue;

    // The whole TD must fit before the Link TRB in the last slot. If it
    // does not, the rest of the ring becomes No-Op TDs and the TD starts
    // at the top after the link; otherwise its Data or Status stage would
    // land on the Link TRB and past the ring.
    let needed = if w_length > 0 { 3 } else { 2 };
    if enq + needed > NUM_TR_TRBS - 1 {
        for i in enq..NUM_TR_TRBS - 1 {
            write_trb(ring, i, 0, 0, TRB_TR_NOOP | cycle);
        }
        write_trb(ring, NUM_TR_TRBS - 1, ring.phys(), 0, TRB_LINK | cycle | (1 << 1));
        cycle ^= 1;
        enq = 0;
    }

    // Setup Stage TRB
    let setup_lo = bm_request as u32 | ((b_request as u32) << 8)
        | ((w_value as u32) << 16);
    let setup_hi = w_index as u32 | ((w_length as u32) << 16);
    let setup_param = setup_lo as u64 | ((setup_hi as u64) << 32);
    let trt = if w_length == 0 { TRB_TRT_NO } else if dir_in { TRB_TRT_IN } else { 0x02 << 16 };
    write_trb(ring, enq, setup_param, 8, TRB_SETUP_STAGE | TRB_IDT | trt | cycle);
    enq += 1;

    // Data Stage TRB (if needed)
    if w_length > 0 {
        let dir_bit = if dir_in { TRB_DIR_IN } else { 0 };
        write_trb(ring, enq, state.data_buf.phys(), w_length as u32, TRB_DATA_STAGE | dir_bit | cycle);
        enq += 1;
    }

    // Status Stage TRB
    let status_dir = if w_length > 0 && dir_in { 0 } else { TRB_DIR_IN };
    write_trb(ring, enq, 0, 0, TRB_STATUS_STAGE | TRB_IOC | status_dir | cycle);
    enq += 1;

    // Wrap check
    if enq >= NUM_TR_TRBS - 1 {
        write_trb(ring, NUM_TR_TRBS - 1, ring.phys(), 0, TRB_LINK | cycle | (1 << 1));
        cycle ^= 1;
        enq = 0;
    }
    state.devs[d].ep0_cycle = cycle;
    state.devs[d].ep0_enqueue = enq;

    // Ring doorbell for slot, target EP0 (DCI=1)
    ring_doorbell(state, slot as u32, 1);

    // Wait for the reply (1 s) and dispatch events of other devices. Our own
    // event is recognised by its TRB address lying in this device's EP0 ring.
    let deadline = crate::interrupts::ticks() + 100;
    state.last_cc = 0;
    loop {
        if crate::interrupts::ticks() >= deadline { return false; }
        let e = match next_event(state) {
            Some(e) => e,
            None => { core::hint::spin_loop(); continue; }
        };
        flush_erdp(state);
        if e.trb_type == EVT_PORT_STATUS { note_port_change(state, &e); }
        if e.trb_type != EVT_TRANSFER { continue; }
        let ok = e.cc == CC_SUCCESS || e.cc == CC_SHORT_PACKET;
        if in_ring(e.param, ring.phys()) {
            state.last_cc = e.cc;
            return ok;
        }
        dispatch_transfer(state, &e);
    }
}

fn usb_get_descriptor(state: &mut XhciState, slot: u8, desc_type: u16, length: u16) -> bool {
    state.data_buf.fill(0u32, 0, length as u64);
    usb_control_transfer(state, slot, 0x80, USB_GET_DESCRIPTOR, desc_type, 0, length, true)
}

fn usb_set_config(state: &mut XhciState, slot: u8, config_val: u8) -> bool {
    usb_control_transfer(state, slot, 0x00, USB_SET_CONFIG, config_val as u16, 0, 0, false)
}

// === Configure Endpoints — Linux xhci_endpoint_init, xhci_check_maxpacket ===

/// The xHCI interval of a periodic endpoint (`xhci_get_endpoint_interval`):
/// at high and super speed bInterval is an exponent, at full and low speed
/// a number of frames.
fn xhci_interval(speed: u32, b_interval: u8) -> u32 {
    match speed {
        SPEED_HIGH | SPEED_SUPER | 5 => (b_interval.clamp(1, 16) - 1) as u32,
        _ => {
            // `xhci_parse_frame_interval`: fls(8 * bInterval) - 1, in 3..10.
            let v = 8 * b_interval as u32;
            let fls = 32 - v.leading_zeros();
            fls.saturating_sub(1).clamp(3, 10)
        }
    }
}

/// The input context's slot context: the output one (`xhci_slot_copy`)
/// with Context Entries raised to `last` DCI.
fn input_slot_copy(state: &XhciState, d: usize, last: u8) {
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    for k in 0..4u32 {
        input.w32(ctx + (k * 4) as usize, state.devs[d].dctx.r32(k * 4));
    }
    let cur = (input.r32(ctx) >> 27) & 0x1F;
    let entries = cur.max(last as u32);
    input.w32(ctx, (input.r32(ctx) & !(0x1F << 27)) | (entries << 27));
}

/// Configure interrupt endpoints of `slot` in one command:
/// `(dci, ring, type, wMaxPacketSize, bInterval)` each.
fn cmd_configure_endpoints(state: &mut XhciState, slot: u8, eps: &[(u8, u64, u32, u16, u8)]) -> bool {
    if eps.is_empty() { return true; }
    let Some(d) = dev_idx(state, slot) else { return false };
    let speed = state.devs[d].speed;
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    input.fill(0u32, 0, 4096);

    // Input Control Context: Add Slot plus every endpoint.
    let mut add = 1u32;
    let mut last = 1u8;
    for &(dci, ..) in eps {
        add |= 1 << dci;
        last = last.max(dci);
    }
    input.w32(4u32, add);
    input_slot_copy(state, d, last);

    for &(dci, ring, ep_type, w_max, b_interval) in eps {
        let ep_off = ctx * (dci as usize + 1);
        let maxp = (w_max & 0x7FF) as u32;
        // High-speed periodic endpoints carry extra transactions in bits
        // 12..11 (`xhci_get_endpoint_max_burst`).
        let burst = if speed == SPEED_HIGH { ((w_max >> 11) & 0x3) as u32 } else { 0 };
        let esit = maxp * (burst + 1);
        let interval = xhci_interval(speed, b_interval);
        input.w32(ep_off, (interval << 16) | ((esit >> 16) << 24));
        input.w32(ep_off + 4, (3 << 1) | (ep_type << 3) | (burst << 8) | (maxp << 16));
        input.w32(ep_off + 8, (ring as u32) | 1);
        input.w32(ep_off + 12, (ring >> 32) as u32);
        // Interrupt: average TRB length = max ESIT payload.
        input.w32(ep_off + 16, ((esit & 0xFFFF) << 16) | (esit & 0xFFFF));
    }

    post_command(state, input.phys(), 0, TRB_CONFIGURE_EP | ((slot as u32) << 24));
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

/// Correct EP0's max packet size of `slot` (Evaluate Context).
fn cmd_evaluate_ep0(state: &mut XhciState, slot: u8, max_packet: u16) -> bool {
    let Some(d) = dev_idx(state, slot) else { return false };
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    let dctx = state.devs[d].dctx;
    input.fill(0u32, 0, 4096);
    // Add EP0 only; its context is the output one with a new size.
    input.w32(4u32, 1 << 1);
    for k in 0..(ctx as u32 / 4) {
        input.w32(ctx * 2 + (k * 4) as usize, dctx.r32(ctx as u32 + k * 4));
    }
    let dw1 = (input.r32(ctx * 2 + 4) & 0x0000_FFFF) | ((max_packet as u32) << 16);
    input.w32(ctx * 2 + 4, dw1);

    post_command(state, input.phys(), 0, TRB_EVALUATE_CTX | ((slot as u32) << 24));
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

// === USB-NIC support: bulk endpoints (for the RTL8153 USB-Ethernet driver) ===
//
// A USB-NIC needs bulk IN (RX) + bulk OUT (TX) on top of the control endpoint.
// We address the device, configure its two bulk endpoints, and expose three
// primitives the class driver (rtl8153.rs) builds on: nic_control (register
// access via EP0), nic_bulk_out (TX), nic_bulk_in (RX poll). This is the USB
// transport; the chip logic lives in the class driver.

const EP_TYPE_BULK_OUT: u32 = 2;
const EP_TYPE_BULK_IN:  u32 = 6;
const NIC_BULK_BUF_PAGES: usize = 4;            // 16 KiB == RTL8153 rx_buf_sz
const NIC_BULK_BUF_BYTES: usize = NIC_BULK_BUF_PAGES * 4096;
// Bulk-IN buffers kept simultaneously in flight so the device always has a
// buffer to DMA into (Linux r8152 RTL8152_MAX_RX). An unarmed endpoint between
// completion and re-arm lets the chip's small RX FIFO overflow at gigabit.
// The bulk-IN TR ring uses slots 0..NIC_RX_BUFS with the link at slot
// NIC_RX_BUFS; producer and consumer walk it in lockstep so cycle bits line up.
// 128 deep (2 MiB) absorbs a gigabit burst (~16 ms) until re-arming catches up.
// Bounded by the 256-entry event ring and the one-page bulk-IN ring (<= ~255).
const NIC_RX_BUFS: usize = 128;
// Async bulk-OUT (TX): a small ring of TX buffers so a frame can be posted and
// the call returns without busy-waiting for the USB completion, which would
// otherwise dominate the per-packet cost. Completions are reaped lazily;
// buffers are reused round-robin and a
// buffer is never overwritten while in flight (tx_inflight < NIC_TX_BUFS).
// 2 KiB each covers MTU(1514)+tx_desc(8); 16 deep covers ACK bursts.
const NIC_TX_BUFS: usize = 16;
const NIC_TX_BUF_BYTES: usize = 2048;

/// State owned by the network device only; the controller around it is the
/// shared `XhciState`.
struct NicRings {
    /// Slot and root port of the device (one of several on the controller).
    slot_id: u8,
    port_num: u32,
    port_speed: u32,
    in_ring: DmaRegion,  in_cycle: u32,  in_enq: usize,  in_dci: u8,
    out_ring: DmaRegion, out_cycle: u32, out_enq: usize, out_dci: u8,
    in_buf: DmaRegion,   out_buf: DmaRegion, // out_buf = NIC_TX_BUFS TX buffers
    tx_inflight: usize,                // posted-but-not-completed TX
    tx_next: usize,                    // round-robin TX buffer index
    rx_armed: usize,                   // bulk-IN TRBs currently device-owned
    trb_buf: [usize; NIC_RX_BUFS],     // bulk-IN ring slot -> buffer index
    rx_done_buf: [usize; NIC_RX_BUFS], // FIFO of completed buffers...
    rx_done_len: [usize; NIC_RX_BUFS], // ...and their byte counts
    rx_done_head: usize,
    rx_done_count: usize,
}


// USB-transport-layer profiling (read + reset via nic_take_stats). Lets a
// speed test see whether the bottleneck is the bulk RX path (few bytes /
// many empty polls / shallow ring) or above it (TCP/ACK/poll cadence).
static NIC_RX_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NIC_RX_DELIV: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // calls returning data
// Bulk-IN completion codes, to see why few transfers complete:
//
//   CC_SUCCESS      buffer filled completely (16 KiB)
//   CC_SHORT_PACKET the normal case: the chip had less and closed the transfer
//   other cc        an error; the buffer is re-armed
//   residual        sum of the unfilled part of each buffer
//
// Mostly SHORT with a large residual means the chip sends small chunks and
// each chunk costs a whole transfer.
static NIC_CC_OK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NIC_CC_SHORT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NIC_CC_OTHER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NIC_CC_LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NIC_RESIDUAL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// (ok, short, other, last_other_cc, residual_sum); counters reset to 0.
pub fn nic_take_cc() -> (u64, u64, u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (NIC_CC_OK.swap(0, Relaxed), NIC_CC_SHORT.swap(0, Relaxed),
     NIC_CC_OTHER.swap(0, Relaxed), NIC_CC_LAST.load(Relaxed),
     NIC_RESIDUAL.swap(0, Relaxed))
}
static NIC_RX_EMPTY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // calls returning 0
static NIC_RX_ARMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // sum of in-flight depth at delivery
static NIC_TX_CALLS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NIC_TX_CYC:   core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0); // cycles spent waiting for TX completion

/// Run `f` on the controller that carries the network device.
fn with_nic<R>(f: impl FnOnce(&mut XhciState) -> R) -> Option<R> {
    let mut g = CTRLS.lock();
    g.iter_mut().find(|s| s.nic.is_some()).map(f)
}

/// Negotiated USB link speed of the NIC. Full/Low speed would cap an Ethernet
/// dongle far below gigabit (Full-Speed = 12 Mbit).
pub fn nic_link_speed_str() -> &'static str {
    match with_nic(|s| s.nic.as_ref().map(|n| n.port_speed).unwrap_or(0)) {
        Some(SPEED_SUPER) => "SuperSpeed (5 Gbit)",
        Some(SPEED_HIGH) => "High-Speed (480 Mbit)",
        Some(SPEED_FULL) => "Full-Speed (12 Mbit!)",
        Some(SPEED_LOW) => "Low-Speed (1.5 Mbit!)",
        _ => "unknown",
    }
}

/// (rx_bytes, rx_deliveries, rx_empty_polls, sum_ring_depth, tx_calls, tx_wait_cycles), each reset to 0.
pub fn nic_take_stats() -> (u64, u64, u64, u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (NIC_RX_BYTES.swap(0, Relaxed), NIC_RX_DELIV.swap(0, Relaxed),
     NIC_RX_EMPTY.swap(0, Relaxed), NIC_RX_ARMED.swap(0, Relaxed),
     NIC_TX_CALLS.swap(0, Relaxed), NIC_TX_CYC.swap(0, Relaxed))
}

pub fn nic_attached() -> bool {
    CTRLS.lock().iter().any(|s| s.nic.is_some())
}

/// Read-only scan of every xHCI controller: PCI id, port count, and each
/// connected/USB3 port's protocol + link state. Shows whether USB-C
/// SuperSpeed routes through a separate controller (e.g. a Thunderbolt/Titan
/// Ridge xHCI that is never brought up) while the USB2 half of a device lands
/// on the PCH xHCI. Mem-decode is enabled so BAR reads work; nothing is reset,
/// halted, or addressed.
pub fn scan_all_controllers() {
    let mut n = 0u32;
    for bus in 0u16..=255 {
        for dev_num in 0u8..32 {
            for func in 0u8..8 {
                let addr = pci::PciAddr { bus: bus as u8, device: dev_num, function: func };
                let id = pci::read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 { if func == 0 { break; } continue; }
                let class_reg = pci::read32(addr, 0x08);
                if ((class_reg >> 24) & 0xFF) == 0x0C
                    && ((class_reg >> 16) & 0xFF) == 0x03
                    && ((class_reg >> 8) & 0xFF) == 0x30
                {
                    n += 1;
                    scan_one_controller(addr, (id & 0xFFFF) as u16, ((id >> 16) & 0xFFFF) as u16);
                }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }
    if n == 0 { kprintln!("[npk] usb: no xHCI controllers found"); }
    else { kprintln!("[npk] usb: {} xHCI controller(s) total", n); }
}

fn scan_one_controller(addr: pci::PciAddr, vid: u16, did: u16) {
    // Thunderbolt/USB4 host xHCIs are the usual hiding place for USB-C SS.
    let tb = vid == 0x8086 && matches!(did, 0x1574 | 0x15b5 | 0x15b6 | 0x15c1 | 0x15d4
        | 0x15db | 0x15e8 | 0x15e9 | 0x15ec | 0x15ef | 0x15f0 | 0x1130 | 0x9a13 | 0x9a17);
    kprintln!("[npk] usb: xHCI {:02x}:{:02x}.{} [{:04x}:{:04x}]{}",
        addr.bus, addr.device, addr.function, vid, did,
        if tb { "  <- Thunderbolt/USB4 host" } else { "" });

    let cmd = pci::read16(addr, 0x04);
    pci::write32(addr, 0x04, (cmd | 0x02) as u32); // enable memory-space decode
    let bar0_raw = pci::read32(addr, 0x10);
    let bar0 = if bar0_raw & 0x04 != 0 { pci::read_bar64(addr, 0x10) }
               else { (bar0_raw & 0xFFFF_FFF0) as u64 };
    if bar0 == 0 { kprintln!("    BAR0 unassigned (controller off / D3 — SS path likely dark here)"); return; }

    // SAFETY: BAR0 of this xHCI controller, its register window.
    let mmio = match unsafe { Mmio::map(bar0, BAR0_MAP) } {
        Ok(m) => m,
        Err(_) => { kprintln!("    BAR0 map failed"); return; }
    };

    let caplength = mmio.r8(CAP_CAPLENGTH) as u64;
    let hcsparams1 = mmio.r32(CAP_HCSPARAMS1);
    let max_ports = (hcsparams1 >> 24) & 0xFF;
    if max_ports == 0 || max_ports > 64 { kprintln!("    ports={} (unreadable — not running)", max_ports); return; }
    let oper = mmio.sub(caplength, BAR0_MAP - caplength);
    kprintln!("    ports={}", max_ports);
    for p in 0..max_ports {
        let sc = oper.r32(portsc_off(p));
        let ccs = sc & PORTSC_CCS != 0;
        let ver = port_usb_version(mmio, p);
        if !ccs && ver != 3 { continue; } // show every USB3 port even if empty
        let speed = (sc >> 10) & 0xF;
        let pls = (sc >> 5) & 0xF;
        let sname = match speed {
            SPEED_SUPER => "SuperSpeed", SPEED_HIGH => "High", SPEED_FULL => "Full",
            SPEED_LOW => "Low", 0 => "-", _ => "?",
        };
        kprintln!("    port {} USB{} ccs={} ped={} pls={} speed={}({}) portsc={:#010x}",
            p + 1, ver, ccs as u8, (sc & PORTSC_PED != 0) as u8, pls, speed, sname, sc);
    }
}

/// Warm-reset the USB3 ports of every Thunderbolt/Titan Ridge xHCI and report
/// the before/after link state. A USB3 link stuck in Disabled/Inactive (PLS=4/6)
/// only recovers via a warm reset (WPR, bit 31); the normal port path issues a
/// hot reset (PR, bit 4), which cannot revive a Disabled link. If a port reaches
/// Enabled (PED=1) at speed=4 afterwards, SuperSpeed trained and the device is
/// attachable on this controller. Touches only TB-host USB3 ports.
pub fn tb_warm_reset() {
    let mut hit = 0u32;
    for bus in 0u16..=255 {
        for dev_num in 0u8..32 {
            for func in 0u8..8 {
                let addr = pci::PciAddr { bus: bus as u8, device: dev_num, function: func };
                let id = pci::read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 { if func == 0 { break; } continue; }
                let vid = (id & 0xFFFF) as u16;
                let did = ((id >> 16) & 0xFFFF) as u16;
                let class_reg = pci::read32(addr, 0x08);
                let is_xhci = ((class_reg >> 24) & 0xFF) == 0x0C
                    && ((class_reg >> 16) & 0xFF) == 0x03
                    && ((class_reg >> 8) & 0xFF) == 0x30;
                let tb = vid == 0x8086 && matches!(did, 0x1574 | 0x15b5 | 0x15b6 | 0x15c1
                    | 0x15d4 | 0x15db | 0x15e8 | 0x15e9 | 0x15ec | 0x15ef | 0x15f0
                    | 0x1130 | 0x9a13 | 0x9a17);
                if is_xhci && tb { hit += 1; tb_warm_reset_ctrl(addr, did); }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }
    if hit == 0 { kprintln!("[npk] tbtrain: no Thunderbolt xHCI found"); }
}

fn tb_warm_reset_ctrl(addr: pci::PciAddr, did: u16) {
    let cmd = pci::read16(addr, 0x04);
    pci::write32(addr, 0x04, (cmd | 0x02) as u32);
    let bar0_raw = pci::read32(addr, 0x10);
    let bar0 = if bar0_raw & 0x04 != 0 { pci::read_bar64(addr, 0x10) }
               else { (bar0_raw & 0xFFFF_FFF0) as u64 };
    if bar0 == 0 { kprintln!("[npk] tbtrain: {:04x} BAR0 unassigned", did); return; }
    // SAFETY: BAR0 of this xHCI controller, its register window.
    let mmio = match unsafe { Mmio::map(bar0, BAR0_MAP) } {
        Ok(m) => m,
        Err(_) => { kprintln!("[npk] tbtrain: BAR0 map failed"); return; }
    };
    let caplength = mmio.r8(CAP_CAPLENGTH) as u64;
    let max_ports = (mmio.r32(CAP_HCSPARAMS1) >> 24) & 0xFF;
    if max_ports == 0 || max_ports > 64 { kprintln!("[npk] tbtrain: {:04x} not running", did); return; }
    let oper = mmio.sub(caplength, BAR0_MAP - caplength);

    kprintln!("[npk] tbtrain: TB xHCI [{:04x}] warm-resetting USB3 ports", did);
    for p in 0..max_ports {
        if port_usb_version(mmio, p) != 3 { continue; }
        let off = portsc_off(p);
        let before = oper.r32(off);
        kprintln!("  port {}: before portsc={:#010x} (pls={})", p + 1, before, (before >> 5) & 0xF);

        // Issue warm reset: preserve PP, set WPR (RW1S, self-clearing).
        oper.w32(off, port_neutral(before) | PORTSC_PP | PORTSC_WPR);
        // Wait up to ~500ms for the reset to complete (WRC or PRC), polling ticks.
        let deadline = crate::interrupts::ticks() + 50;
        loop {
            let sc = oper.r32(off);
            if sc & (PORTSC_WRC | PORTSC_PRC) != 0 { break; }
            if crate::interrupts::ticks() >= deadline { break; }
            core::hint::spin_loop();
        }
        // Clear all change bits.
        let sc = oper.r32(off);
        oper.w32(off, port_neutral(sc) | PORTSC_CSC | PORTSC_PEC | PORTSC_WRC
            | PORTSC_PRC | PORTSC_PLC);
        // Settle, then read the resulting state.
        let s2 = crate::interrupts::ticks() + 10;
        while crate::interrupts::ticks() < s2 { core::hint::spin_loop(); }
        let after = oper.r32(off);
        let speed = (after >> 10) & 0xF;
        let sname = match speed {
            SPEED_SUPER => "SuperSpeed", SPEED_HIGH => "High", SPEED_FULL => "Full",
            SPEED_LOW => "Low", 0 => "-", _ => "?",
        };
        kprintln!("  port {}: after  portsc={:#010x} ccs={} ped={} pls={} speed={}({}){}",
            p + 1, after, (after & PORTSC_CCS != 0) as u8, (after & PORTSC_PED != 0) as u8,
            (after >> 5) & 0xF, speed, sname,
            if after & PORTSC_PED != 0 && speed == SPEED_SUPER { "  <<< SuperSpeed LINK UP!" } else { "" });
    }
}

/// On-demand re-dump of the NIC controller's root ports + negotiated link speed.
/// The boot-time scan scrolls past fast; this lets the `nic` command print the
/// same USB2/USB3 protocol + ccs/speed table whenever asked.
pub fn nic_dump_ports() {
    let done = with_nic(|state| {
        dump_nic_ports(state);
        let (speed, port) = state.nic.as_ref()
            .map(|n| (n.port_speed, n.port_num + 1)).unwrap_or((0, 0));
        let link = match speed {
            SPEED_SUPER => "SuperSpeed (5 Gbit)",
            SPEED_HIGH => "High-Speed (480 Mbit)",
            SPEED_FULL => "Full-Speed (12 Mbit!)",
            SPEED_LOW => "Low-Speed (1.5 Mbit!)",
            _ => "unknown",
        };
        kprintln!("[npk] xhci: NIC on port {} — link = {}", port, link);
    });
    if done.is_none() { kprintln!("[npk] xhci: no NIC attached"); }
}

/// USB link speed of the attached NIC as an r8152 coalesce class:
/// 2 = SuperSpeed, 1 = High, 0 = Full/other. Picks the RX aggregation timeout.
pub fn nic_speed_class() -> u8 {
    match with_nic(|s| s.nic.as_ref().map(|n| n.port_speed).unwrap_or(0)) {
        Some(SPEED_SUPER) => 2,
        Some(SPEED_HIGH) => 1,
        _ => 0,
    }
}

/// xHCI major USB revision of root port `port0` (0-based) from the Supported
/// Protocol extended capability (ID=2): 3 = USB3/SuperSpeed-capable, 2 = USB2,
/// 0 = unknown. The same physical USB-C jack appears as two logical ports — one
/// USB2, one USB3 — so this is how we tell whether a SuperSpeed port even exists.
fn port_usb_version(mmio: Mmio, port0: u32) -> u8 {
    let hccparams1 = mmio.r32(CAP_HCCPARAMS1);
    let mut off = ((hccparams1 >> 16) & 0xFFFF) * 4;
    let port_num = port0 + 1; // Supported-Protocol port offset is 1-based
    for _ in 0..64 {
        if off == 0 || off as u64 + 12 > mmio.len() { break; }
        let d0 = mmio.r32(off);
        if d0 & 0xFF == 2 {
            let major = ((d0 >> 24) & 0xFF) as u8;
            let d2 = mmio.r32(off + 8);
            let cpo = d2 & 0xFF;          // compatible port offset (1-based)
            let cpc = (d2 >> 8) & 0xFF;   // compatible port count
            if port_num >= cpo && port_num < cpo + cpc { return major; }
        }
        let next = (d0 >> 8) & 0xFF;
        if next == 0 { break; }
        off += next * 4;
    }
    0
}

/// Dump every populated/connected root port with its protocol + link state.
/// Tells us whether the dongle trained a SuperSpeed link (USB3 port, speed=4)
/// or fell back to the USB2 companion port (speed=3) — the difference between a
/// 480 Mbit ceiling and the 5 Gbit headroom we'd need for true gigabit.
fn dump_nic_ports(x: &XhciState) {
    kprintln!("[npk] xhci: NIC ctrl root-port scan ({} ports):", x.max_ports);
    for p in 0..x.max_ports {
        let sc = x.oper.r32(portsc_off(p));
        let ver = port_usb_version(x.mmio, p);
        let ccs = sc & PORTSC_CCS != 0;
        if !ccs && ver != 3 { continue; } // show all USB3 ports even if empty
        let speed = (sc >> 10) & 0xF;
        let pls = (sc >> 5) & 0xF;
        let sname = match speed {
            SPEED_SUPER => "SuperSpeed", SPEED_HIGH => "High", SPEED_FULL => "Full",
            SPEED_LOW => "Low", 0 => "-", _ => "?",
        };
        kprintln!("  port {} USB{} ccs={} ped={} pls={} speed={}({}) portsc={:#010x}",
            p + 1, ver, ccs as u8, (sc & PORTSC_PED != 0) as u8, pls, speed, sname, sc);
    }
}

/// Set up the network device `vid:pid` among the devices enumeration
/// addressed on this controller, behind a hub or not: its configuration,
/// then its two bulk endpoints. Nothing else on the controller is touched.
fn nic_probe(state: &mut XhciState, vid: u16, pid: u16, ep_in: u8, ep_out: u8) -> bool {
    if state.nic.is_some() { return true; }
    let Some(d) = state.devs.iter().position(|d| {
        d.hub.is_none()
            && d.seen.device.len() >= 12
            && u16::from_le_bytes([d.seen.device[8], d.seen.device[9]]) == vid
            && u16::from_le_bytes([d.seen.device[10], d.seen.device[11]]) == pid
    }) else {
        return false;
    };
    let (slot, speed, root_port) = (state.devs[d].slot, state.devs[d].speed, state.devs[d].place.root_port);
    let path = state.devs[d].seen.port.clone();
    let config_val = state.devs[d].seen.cfg.get(5).copied().unwrap_or(1);
    if !usb_set_config(state, slot, config_val) { return false; }

    let (Some(in_ring), Some(out_ring), Some(in_buf), Some(out_buf)) = (
        alloc_dma(1, "nic bulk-in ring"),
        alloc_dma(1, "nic bulk-out ring"),
        alloc_dma(NIC_RX_BUFS * NIC_BULK_BUF_PAGES, "nic in bufs"),
        alloc_dma((NIC_TX_BUFS * NIC_TX_BUF_BYTES).div_ceil(4096), "nic tx bufs"),
    ) else {
        kprintln!("[npk] xhci: nic DMA alloc failed");
        return false;
    };
    // Bulk-IN uses NIC_RX_BUFS slots; bulk-OUT uses the whole ring.
    write_trb(in_ring, NIC_RX_BUFS, in_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    write_trb(out_ring, NUM_TR_TRBS - 1, out_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));

    let in_dci = ep_in * 2 + 1;   // IN endpoint
    let out_dci = ep_out * 2;     // OUT endpoint
    let bulk_mp = if speed >= SPEED_SUPER { 1024 } else { 512 };
    if !cmd_configure_bulk(state, slot, in_dci, in_ring.phys(), out_dci, out_ring.phys(), bulk_mp) {
        kprintln!("[npk] xhci: nic configure-bulk failed (speed {})", speed);
        return false;
    }

    kprintln!("[npk] xhci: NIC attached on {:02x}:{:02x}.{} port {} (slot {}, bulk in EP{} out EP{}, speed {})",
        state.pci_addr.bus, state.pci_addr.device, state.pci_addr.function,
        path, slot, ep_in, ep_out, speed);

    state.nic = Some(NicRings {
        slot_id: slot, port_num: root_port, port_speed: speed,
        in_ring, in_cycle: 1, in_enq: 0, in_dci,
        out_ring, out_cycle: 1, out_enq: 0, out_dci,
        in_buf, out_buf,
        tx_inflight: 0, tx_next: 0,
        rx_armed: 0, trb_buf: [0; NIC_RX_BUFS],
        rx_done_buf: [0; NIC_RX_BUFS], rx_done_len: [0; NIC_RX_BUFS],
        rx_done_head: 0, rx_done_count: 0,
    });
    // Pre-fill the RX ring so the device can deliver right away.
    for b in 0..NIC_RX_BUFS { nic_arm_rx(state, b); }
    true
}

/// Find and set up `vid:pid` on any controller.
///
/// 1. Gentle: look among the devices every running controller enumerated.
/// 2. Fallback: bring up a controller not in the table and look there.
/// 3. Last resort: reset the known controllers, enumerate again, look.
///
/// The fallbacks exist because without network there is no way to update.
pub fn nic_attach(vid: u16, pid: u16, ep_in: u8, ep_out: u8) -> bool {
    if nic_attached() { return true; }

    {
        let mut g = CTRLS.lock();
        for s in g.iter_mut() {
            if nic_probe(s, vid, pid, ep_in, ep_out) { return true; }
        }
    }

    // Fallback: a controller not in the table.
    for bus in 0u16..=255 {
        for dev_num in 0u8..32 {
            for func in 0u8..8 {
                let addr = pci::PciAddr { bus: bus as u8, device: dev_num, function: func };
                let id = pci::read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 { if func == 0 { break; } continue; }
                let class_reg = pci::read32(addr, 0x08);
                if ((class_reg >> 24) & 0xFF) as u8 == 0x0C
                    && ((class_reg >> 16) & 0xFF) as u8 == 0x03
                    && ((class_reg >> 8) & 0xFF) as u8 == 0x30
                    && !ctrl_known(addr)
                {
                    kprintln!("[npk] xhci: NIC not on any running controller — bringing up {:02x}:{:02x}.{}",
                        addr.bus, addr.device, addr.function);
                    let pci_dev = pci::PciDevice {
                        addr,
                        vendor_id: (id & 0xFFFF) as u16,
                        device_id: ((id >> 16) & 0xFFFF) as u16,
                        bar0: pci::read32(addr, 0x10),
                        irq_line: pci::read8(addr, 0x3C),
                    };
                    if let Some(mut x) = bring_up_controller(pci_dev) {
                        enumerate_root_ports(&mut x);
                        let found = nic_probe(&mut x, vid, pid, ep_in, ep_out);
                        store_ctrl(x);
                        if found { return true; }
                    }
                }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }

    // Last resort: reset the known controllers. `init()` stores every
    // controller, so the previous fallback usually finds none; losing the
    // devices on a controller is better than having no network.
    let addrs: Vec<pci::PciAddr> = CTRLS.lock().iter().map(|s| s.pci_addr).collect();
    for a in addrs {
        kprintln!("[npk] xhci: NIC not found gently on {:02x}:{:02x}.{} — falling back to a \
                   controller RESET (everything addressed there loses its slot)",
            a.bus, a.device, a.function);
        {
            let mut g = CTRLS.lock();
            for s in g.iter_mut().filter(|s| s.pci_addr == a) { hid_forget(s); }
            g.retain(|s| s.pci_addr != a);
        }
        let id = pci::read32(a, 0x00);
        let pci_dev = pci::PciDevice {
            addr: a,
            vendor_id: (id & 0xFFFF) as u16,
            device_id: ((id >> 16) & 0xFFFF) as u16,
            bar0: pci::read32(a, 0x10),
            irq_line: pci::read8(a, 0x3C),
        };
        let Some(mut x) = bring_up_controller(pci_dev) else { continue };
        enumerate_root_ports(&mut x);
        let found = nic_probe(&mut x, vid, pid, ep_in, ep_out);
        store_ctrl(x);
        if found { return true; }
    }
    false
}

/// Whether the table already tracks this controller.
fn ctrl_known(addr: pci::PciAddr) -> bool {
    CTRLS.lock().iter().any(|s| s.pci_addr == addr)
}

/// Configure both bulk endpoints (IN + OUT) of `slot` in one Configure
/// Endpoint command.
fn cmd_configure_bulk(state: &mut XhciState, slot: u8, in_dci: u8, in_ring: u64,
    out_dci: u8, out_ring: u64, max_pkt: u16) -> bool
{
    let Some(d) = dev_idx(state, slot) else { return false };
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    input.fill(0u32, 0, 4096);

    // Input Control Context: add Slot + both endpoints
    input.w32(4u32, 1 | (1u32 << in_dci) | (1u32 << out_dci));
    input_slot_copy(state, d, in_dci.max(out_dci));

    for (dci, ring, ep_type) in [
        (in_dci, in_ring, EP_TYPE_BULK_IN),
        (out_dci, out_ring, EP_TYPE_BULK_OUT),
    ] {
        let ep = ctx * (dci as usize + 1);
        input.w32(ep, 0);
        input.w32(ep + 4, (3 << 1) | (ep_type << 3) | ((max_pkt as u32) << 16));
        input.w32(ep + 8, (ring as u32) | 1);
        input.w32(ep + 12, (ring >> 32) as u32);
        input.w32(ep + 16, max_pkt as u32);
    }

    post_command(state, input.phys(), 0, TRB_CONFIGURE_EP | ((slot as u32) << 24));
    matches!(wait_command_completion(state), Some((cc, _)) if cc == CC_SUCCESS)
}

/// Arm a bulk-IN TRB for RX buffer `buf_idx`.
///
/// The producer walks ring slots 0..NIC_RX_BUFS-1; the link TRB at slot
/// NIC_RX_BUFS wraps it and toggles the cycle bit.
fn nic_arm_rx(state: &mut XhciState, buf_idx: usize) {
    let (sid, dci) = {
        let n = match state.nic.as_mut() { Some(n) => n, None => return };
        let ring_slot = n.in_enq;
        let cyc = n.in_cycle;
        let addr = n.in_buf.phys() + (buf_idx * NIC_BULK_BUF_BYTES) as u64;
        write_trb(n.in_ring, ring_slot, addr, NIC_BULK_BUF_BYTES as u32,
            TRB_NORMAL | TRB_IOC | cyc);
        n.trb_buf[ring_slot] = buf_idx;
        n.in_enq += 1;
        if n.in_enq >= NIC_RX_BUFS {
            write_trb(n.in_ring, NIC_RX_BUFS, n.in_ring.phys(), 0, TRB_LINK | cyc | (1 << 1));
            n.in_cycle ^= 1;
            n.in_enq = 0;
        }
        n.rx_armed += 1;
        (n.slot_id as u32, n.in_dci as u32)
    };
    ring_doorbell(state, sid, dci);
}

/// EP0 control transfer for the NIC. `buf` carries the data stage (copied into
/// the controller's data_buf for OUT, read back for IN). Used for r8152
/// register access.
pub fn nic_control(req_type: u8, request: u8, value: u16, index: u16, buf: &mut [u8], dir_in: bool) -> bool {
    with_nic(|state| {
        let len = buf.len().min(2048) as u16;
        if !dir_in && len > 0 {
            state.data_buf.copy_in(0u32, &buf[..len as usize]);
        }
        let Some(slot) = state.nic.as_ref().map(|n| n.slot_id) else { return false };
        let ok = usb_control_transfer(state, slot, req_type, request, value, index, len, dir_in);
        if ok && dir_in && len > 0 {
            state.data_buf.copy_out(0u32, &mut buf[..len as usize]);
        }
        ok
    }).unwrap_or(false)
}

/// Send a frame, fire and forget.
pub fn nic_bulk_out(data: &[u8]) -> bool {
    with_nic(|state| nic_bulk_out_on(state, data)).unwrap_or(false)
}

fn nic_bulk_out_on(state: &mut XhciState, data: &[u8]) -> bool {
    use core::sync::atomic::Ordering::Relaxed;

    // Drain events: frees completed TX buffers, takes RX completions, and
    // dispatches keyboard and mouse reports on the same event ring.
    drain(state);

    // Backpressure: wait (bounded) only when all TX buffers are in flight;
    // otherwise we would overwrite live DMA.
    let full = |s: &XhciState| s.nic.as_ref().map(|n| n.tx_inflight >= NIC_TX_BUFS).unwrap_or(true);
    if full(state) {
        let t0 = crate::interrupts::rdtsc();
        let deadline = crate::interrupts::ticks() + 100;
        while full(state) {
            if crate::interrupts::ticks() >= deadline { return false; }
            drain(state);
            core::hint::spin_loop();
        }
        NIC_TX_CYC.fetch_add(crate::interrupts::rdtsc().wrapping_sub(t0), Relaxed);
    }

    let len = data.len().min(NIC_TX_BUF_BYTES);
    let (sid, dci) = {
        let n = match state.nic.as_mut() { Some(n) => n, None => return false };
        let bidx = n.tx_next;
        let off = bidx * NIC_TX_BUF_BYTES;
        let addr = n.out_buf.phys() + off as u64;
        // Slot `bidx` is free: tx_inflight < NIC_TX_BUFS means the last user
        // of this round-robin slot has completed.
        n.out_buf.copy_in(off, &data[..len]);

        let idx = n.out_enq;
        let cyc = n.out_cycle;
        write_trb(n.out_ring, idx, addr, len as u32, TRB_NORMAL | TRB_IOC | cyc);
        n.out_enq += 1;
        if n.out_enq >= NUM_TR_TRBS - 1 {
            write_trb(n.out_ring, NUM_TR_TRBS - 1, n.out_ring.phys(), 0, TRB_LINK | cyc | (1 << 1));
            n.out_cycle ^= 1;
            n.out_enq = 0;
        }
        n.tx_next = (bidx + 1) % NIC_TX_BUFS;
        n.tx_inflight += 1;
        (n.slot_id as u32, n.out_dci as u32)
    };
    ring_doorbell(state, sid, dci);
    NIC_TX_CALLS.fetch_add(1, Relaxed);
    true
}

/// Fetch a received frame. Returns 0 if none is pending.
pub fn nic_bulk_in(buf: &mut [u8]) -> usize {
    with_nic(|state| nic_bulk_in_on(state, buf)).unwrap_or(0)
}

fn nic_bulk_in_on(state: &mut XhciState, buf: &mut [u8]) -> usize {
    use core::sync::atomic::Ordering::Relaxed;

    // Drain everything pending; a full event ring stalls the controller.
    drain(state);

    let (b, len, armed, in_buf) = {
        let n = match state.nic.as_mut() { Some(n) => n, None => return 0 };
        if n.rx_done_count == 0 {
            NIC_RX_EMPTY.fetch_add(1, Relaxed);
            return 0;
        }
        let h = n.rx_done_head;
        let b = n.rx_done_buf[h];
        let len = n.rx_done_len[h];
        n.rx_done_head = (h + 1) % NIC_RX_BUFS;
        n.rx_done_count -= 1;
        (b, len, n.rx_armed, n.in_buf)
    };

    let n_copy = len.min(buf.len());
    // RX buffer `b` holds `len` received bytes.
    in_buf.copy_out(b * NIC_BULK_BUF_BYTES, &mut buf[..n_copy]);
    NIC_RX_BYTES.fetch_add(n_copy as u64, Relaxed);
    NIC_RX_DELIV.fetch_add(1, Relaxed);
    NIC_RX_ARMED.fetch_add(armed as u64, Relaxed);
    nic_arm_rx(state, b);   // buffer is free once copied out
    n_copy
}

/// The xHCI MSI-X interrupt: acknowledge like Linux `xhci_irq` (USBSTS.EINT,
/// then the interrupter's IMAN.IP, both write-1-to-clear) and drain every
/// controller's event ring. Only EINT is written back, not the whole status
/// word as Linux does: that would also clear PCD and other bits nothing here
/// reads by interrupt. `try_lock`: if a transfer on core 0 holds the
/// controllers, it drains the ring itself.
pub fn msi_irq() {
    if let Some(mut g) = CTRLS.try_lock() {
        for slot in g.iter_mut() {
            slot.oper.w32(OP_USBSTS, STS_EINT);
            let ir0 = slot.rt.sub(IR0, IR_LEN);
            ir0.w32(0x00u32, ir0.r32(0x00u32) | 0x01);
            drain(slot);
        }
    } else {
        // Core 0 holds the controllers (a transfer). The controller raises
        // nothing new until the ring is drained, and there is no tick to do
        // it. The shell drains it on its next pass (`take_missed_drain`);
        // the handler wakes it.
        MISSED_DRAIN.store(true, Ordering::Release);
    }
}

static MISSED_DRAIN: AtomicBool = AtomicBool::new(false);

/// An interrupt could not drain the rings: the caller must.
pub fn take_missed_drain() -> bool {
    MISSED_DRAIN.swap(false, Ordering::AcqRel)
}

/// A controller came up without MSI-X: its event ring is drained only by
/// polling, and core 0 has no tick to do it.
static NEEDS_POLL: AtomicBool = AtomicBool::new(false);

pub fn needs_poll() -> bool {
    NEEDS_POLL.load(Ordering::Relaxed)
}

/// A USB key is held: its software repeat (`poll_keyboard`) needs the loop.
pub fn repeat_active() -> bool {
    REPEAT_KEY.load(Ordering::Relaxed) != 0
}

/// IRQ-safe drain of all controllers' event rings. Only the drain talks to
/// the hardware; the main thread reads the software rings (KEY_BUF / MOUSE_BUF).
pub fn poll_events_irq() {
    if let Some(mut g) = CTRLS.try_lock() {
        for slot in g.iter_mut() {
            drain(slot);
        }
    }
}

/// An event taken from the event ring.
struct Evt {
    trb_type: u32,
    cc: u32,
    /// For a transfer event: the address of the completed transfer TRB,
    /// the only way to tell which device it belongs to.
    param: u64,
    control: u32,
    /// Bytes not transferred (Transfer Event, bits 23..0 of `status`).
    residual: u32,
}

/// Take the next event and advance the dequeue cursor.
///
/// Does not write ERDP; `flush_erdp` does that once per pass, since an MMIO
/// write per event is expensive.
fn next_event(state: &mut XhciState) -> Option<Evt> {
    let (param, status, control) = read_trb(state.evt_ring, state.evt_dequeue);
    if control & TRB_CYCLE != state.evt_cycle {
        return None;
    }
    state.evt_dequeue += 1;
    if state.evt_dequeue >= NUM_EVT_TRBS {
        state.evt_dequeue = 0;
        state.evt_cycle ^= 1;
    }
    Some(Evt {
        trb_type: control & (0x3F << 10),
        cc: (status >> 24) & 0xFF,
        param,
        control,
        residual: status & 0x00FF_FFFF,
    })
}

/// Write the event ring dequeue pointer and clear Event Handler Busy. Must
/// run at least once per pass that took events, or the controller reports
/// no further events.
fn flush_erdp(state: &XhciState) {
    let erdp = state.evt_ring.phys() + (state.evt_dequeue * 16) as u64;
    state.rt.sub(IR0, IR_LEN).w64_lo_hi(0x18u32, erdp | (1 << 3));
}

/// Whether TRB address `a` lies in the transfer ring starting at `base`.
fn in_ring(a: u64, base: u64) -> bool {
    base != 0 && a >= base && a < base + (NUM_TR_TRBS * 16) as u64
}

/// Dispatch a transfer event to the device that owns its ring; the TRB
/// address identifies the owner.
///
/// Returns `false` if the event belongs to no device here (then it belongs
/// to a synchronous waiter running a control transfer or command).
fn dispatch_transfer(state: &mut XhciState, e: &Evt) -> bool {
    let a = e.param;
    let cc = e.cc;
    let ok = cc == CC_SUCCESS || cc == CC_SHORT_PACKET;

    // The network device: largest rings and most events at runtime.
    if let Some((ir, or)) = state.nic.as_ref().map(|n| (n.in_ring.phys(), n.out_ring.phys())) {
        let on_in = ir != 0 && a >= ir && a < ir + ((NIC_RX_BUFS + 1) * 16) as u64;
        let on_out = or != 0 && a >= or && a < or + (NUM_TR_TRBS * 16) as u64;
        if on_in {
            // Map the TRB address back to its buffer.
            let ring_slot = ((a.wrapping_sub(ir)) / 16) as usize;
            // Count the completion code before processing, including errors
            // that are silently re-armed.
            match cc {
                CC_SUCCESS => { NIC_CC_OK.fetch_add(1, Ordering::Relaxed); }
                CC_SHORT_PACKET => { NIC_CC_SHORT.fetch_add(1, Ordering::Relaxed); }
                other => {
                    NIC_CC_OTHER.fetch_add(1, Ordering::Relaxed);
                    NIC_CC_LAST.store(other as u64, Ordering::Relaxed);
                }
            }
            NIC_RESIDUAL.fetch_add(e.residual as u64, Ordering::Relaxed);
            let mut rearm: Option<usize> = None;
            if ring_slot < NIC_RX_BUFS {
                if let Some(n) = state.nic.as_mut() {
                    let buf_idx = n.trb_buf[ring_slot];
                    if n.rx_armed > 0 { n.rx_armed -= 1; }
                    // Deliver clean completions only. On error the residual
                    // is not a valid byte count, and a full done-queue must
                    // not swallow the buffer; in both cases re-arm at once,
                    // as Linux resubmits a failed URB.
                    if ok && n.rx_done_count < NIC_RX_BUFS {
                        let len = (NIC_BULK_BUF_BYTES as u32).saturating_sub(e.residual) as usize;
                        let t = (n.rx_done_head + n.rx_done_count) % NIC_RX_BUFS;
                        n.rx_done_buf[t] = buf_idx;
                        n.rx_done_len[t] = len;
                        n.rx_done_count += 1;
                    } else {
                        rearm = Some(buf_idx);
                    }
                }
            }
            if let Some(b) = rearm { nic_arm_rx(state, b); }
            return true;
        }
        if on_out {
            if let Some(n) = state.nic.as_mut() {
                if n.tx_inflight > 0 { n.tx_inflight -= 1; }
            }
            return true;
        }
    }

    // HID interfaces: an IN completion is a report, an OUT completion the
    // end of an output report (LEDs).
    for i in 0..state.hid.len() {
        if in_ring(a, state.hid[i].in_ring.phys()) {
            if state.hid[i].running { hid_irq_in(state, i, e); }
            return true;
        }
        if state.hid[i].out.as_ref().is_some_and(|o| in_ring(a, o.ring.phys())) {
            return true;
        }
    }

    // Hub status change endpoints.
    let hub = state.devs.iter()
        .find(|d| d.hub.as_ref().is_some_and(|h| in_ring(a, h.ring.phys())))
        .map(|d| d.slot);
    if let Some(slot) = hub {
        hub_irq(state, slot, e);
        return true;
    }

    false
}

/// Take and dispatch all pending events. The single drain used by every
/// caller (IRQ, tick, main loop, NIC paths).
fn drain(state: &mut XhciState) {
    let mut any = false;
    for _ in 0..NUM_EVT_TRBS {
        let e = match next_event(state) {
            Some(e) => e,
            None => break,
        };
        any = true;
        if e.trb_type == EVT_TRANSFER {
            // An event owned by no device here would belong to a waiting
            // control transfer, which is not running here: it is orphaned.
            // Drop it rather than give it to the wrong device.
            dispatch_transfer(state, &e);
        } else if e.trb_type == EVT_PORT_STATUS {
            note_port_change(state, &e);
        }
        let _ = e.control;
    }
    if any {
        flush_erdp(state);
    }
}

/// Drain events from the main loop. Needed only early in boot, before the
/// interrupt path drains.
pub fn poll_events() {
    if let Some(mut g) = CTRLS.try_lock() {
        for slot in g.iter_mut() {
            drain(slot);
        }
    }
}

fn process_hid_report(modifiers: u8, keys: &[u8], state: &mut KbdState) {
    let shift = (modifiers & 0x22) != 0;  // L/R Shift
    let ctrl = (modifiers & 0x11) != 0;   // L/R Ctrl
    let alt_gr = (modifiers & 0x40) != 0; // Right Alt (AltGr)
    let super_held = (modifiers & 0x88) != 0; // L/R GUI (Super)

    // Update shared modifier state (used by shade compositor)
    crate::keyboard::set_super(super_held);
    crate::keyboard::set_shift(shift);
    crate::keyboard::set_ctrl(ctrl);

    // Use cached layout (IRQ-safe, no allocation)
    let is_de = crate::keyboard::is_de_layout();

    // Find the first non-zero key in the current report for repeat tracking
    let first_key = keys.iter().find(|&&k| k != 0 && k != 1).copied().unwrap_or(0);

    if first_key == 0 {
        // All keys released — stop repeat
        state.repeat_key = 0;
        REPEAT_KEY.store(0, Ordering::Relaxed);
    } else if state.repeat_key != 0 && !keys.contains(&state.repeat_key) {
        // Repeated key was released while another is held — follow current key
        state.repeat_key = first_key;
        state.repeat_shift = shift;
        state.repeat_altgr = alt_gr;
        let now = crate::interrupts::ticks();
        state.repeat_start = now;
        state.repeat_last = now;
        REPEAT_KEY.store(first_key, Ordering::Relaxed);
        REPEAT_SHIFT.store(shift, Ordering::Relaxed);
        REPEAT_ALTGR.store(alt_gr, Ordering::Relaxed);
        REPEAT_START.store(now, Ordering::Relaxed);
        REPEAT_LAST.store(now, Ordering::Relaxed);
    } else if !state.prev().contains(&first_key) {
        // New key pressed — start repeat timer
        state.repeat_key = first_key;
        state.repeat_shift = shift;
        state.repeat_altgr = alt_gr;
        let now = crate::interrupts::ticks();
        state.repeat_start = now;
        state.repeat_last = now;
        REPEAT_KEY.store(first_key, Ordering::Relaxed);
        REPEAT_SHIFT.store(shift, Ordering::Relaxed);
        REPEAT_ALTGR.store(alt_gr, Ordering::Relaxed);
        REPEAT_START.store(now, Ordering::Relaxed);
        REPEAT_LAST.store(now, Ordering::Relaxed);
    }

    for &key in keys.iter() {
        if key == 0 || key == 1 { continue; } // no key / error rollover
        // Only process newly pressed keys
        if state.prev().contains(&key) { continue; }

        // Ctrl+Shift+C → copy the selection (terminal drag-selection or
        // focused Input/TextArea). Routed as a ShadeAction so the actual
        // copy runs in the main loop, not this poll/IRQ context. In a
        // terminal Ctrl+C must stay SIGINT, so copy is the Shift variant.
        if ctrl && shift && key == 0x06 {
            crate::shade::input::push_action_direct(crate::shade::input::ShadeAction::Copy);
            continue;
        }
        // Ctrl+Shift+V → paste the clipboard (terminal input line or focused
        // Input/TextArea). Plain Ctrl+V still reaches widgets as 'v'+ctrl.
        if ctrl && shift && key == 0x19 {
            crate::shade::input::push_action_direct(crate::shade::input::ShadeAction::Paste);
            continue;
        }
        // Ctrl+C mirrors the PS/2 path (see decode_scancode): (1) terminal
        // SIGINT, cancel the running foreground intent (download / OTA);
        // (2) buffer control byte 0x03 so the compositor's clipboard
        // intercept (handle_input_key → handle_clipboard_key) turns it into
        // a copy when a text widget is focused. Elsewhere the loop consumes
        // it as ^C. HID 0x06 = 'c'.
        if ctrl && key == 0x06 {
            crate::intent::request_cancel();
            push_key(0x03);
            continue;
        }

        // Mod+special keys: push shade actions directly (avoids ESC sequence race)
        if super_held && crate::shade::is_active() {
            let handled = match key {
                0x4F => { crate::shade::input::push_action_direct(if ctrl { crate::shade::input::ShadeAction::ResizeRight } else if shift { crate::shade::input::ShadeAction::SwapRight } else { crate::shade::input::ShadeAction::FocusRight }); true }
                0x50 => { crate::shade::input::push_action_direct(if ctrl { crate::shade::input::ShadeAction::ResizeLeft } else if shift { crate::shade::input::ShadeAction::SwapLeft } else { crate::shade::input::ShadeAction::FocusLeft }); true }
                0x51 => { crate::shade::input::push_action_direct(if ctrl { crate::shade::input::ShadeAction::ResizeDown } else if shift { crate::shade::input::ShadeAction::SwapDown } else { crate::shade::input::ShadeAction::FocusDown }); true }
                0x52 => { crate::shade::input::push_action_direct(if ctrl { crate::shade::input::ShadeAction::ResizeUp } else if shift { crate::shade::input::ShadeAction::SwapUp } else { crate::shade::input::ShadeAction::FocusUp }); true }
                0x4B => { crate::shade::input::push_action_direct(crate::shade::input::ShadeAction::ScrollUp); true }
                0x4E => { crate::shade::input::push_action_direct(crate::shade::input::ShadeAction::ScrollDown); true }
                // Digits 1..9 by HID code, not by character: with Shift the
                // layout turns "1" into "+"/"!" (and de_CH's Shift+4 into
                // nothing), so Mod+Shift+N would never match. HID 0x1E = '1'.
                0x1E..=0x26 => { crate::shade::input::push_workspace_key(key - 0x1D, shift); true }
                _ => false,
            };
            if handled { continue; }
        }

        // Arrow keys and special multi-byte sequences (when mod NOT held)
        match key {
            k if nav_seq_final(k).is_some() => {
                let last = nav_seq_final(k).unwrap_or(b'C');
                push_key(0x1B); push_key(b'['); push_key(last);
                continue;
            }
            _ => {}
        }

        let ch = hid_to_char(key, shift, alt_gr, is_de);
        if ch != 0 {
            push_key(ch);
            // A non-ASCII character is several bytes; they must enter the
            // ring back to back.
            while let Some(b) = take_tail() { push_key(b) }
        }
    }
}

/// Remaining bytes of a UTF-8 sequence (see `input::Utf8Tail`). A separate
/// instance because this driver is a separate producer from the PS/2 driver.
static UTF8_TAIL: spin::Mutex<crate::input::Utf8Tail> =
    spin::Mutex::new(crate::input::Utf8Tail::new());

/// `UTF8_TAIL` is taken by the timer ISR (`poll_events_irq` →
/// `process_hid_report`) and by `poll_keyboard` on core 0 with IF=1, so
/// the non-ISR side must mask interrupts, or a tick inside the lock spins
/// forever on it.
fn take_tail() -> Option<u8> {
    crate::interrupts::without_interrupts(|| UTF8_TAIL.lock().take())
}

/// HID code to the first byte of the character; the rest goes to
/// `UTF8_TAIL`. 0 means the key produces no character.
fn hid_to_char(key: u8, shift: bool, alt_gr: bool, is_de: bool) -> u8 {
    let c = hid_to_char_ch(key, shift, alt_gr, is_de);
    if c == '\0' { return 0 }
    crate::interrupts::without_interrupts(|| UTF8_TAIL.lock().split(c))
}

/// Convert HID keycode to a character. `'\0'` for unhandled keys.
fn hid_to_char_ch(key: u8, shift: bool, alt_gr: bool, is_de: bool) -> char {
    // AltGr: special characters (de_CH)
    if alt_gr && is_de {
        if let Some(ch) = altgr_char_de_hid(key) {
            return ch;
        }
    }

    // ISO-Extra key (102-key layout, left of Z): HID 0x64. Plain →
    // `<`, Shift → `>`, AltGr → `\` (latter via altgr_char_de_hid
    // above). The key is outside the 57-entry layout arrays so it
    // gets handled here before the index check.
    if is_de && key == 0x64 {
        return if shift { '>' } else { '<' };
    }
    // A 102-key (ISO) keyboard used with the US layout also has the
    // non-US `\` key at HID 0x64; map it so the key is not silent.
    if !is_de && key == 0x64 {
        return if shift { '|' } else { '\\' };
    }

    if (key as usize) < HID_TO_ASCII.len() {
        if is_de {
            if shift { HID_TO_ASCII_DE_SHIFT[key as usize] }
            else { HID_TO_ASCII_DE[key as usize] }
        } else {
            // The US tables are bytes: they are pure ASCII.
            (if shift { HID_TO_ASCII_SHIFT[key as usize] }
             else { HID_TO_ASCII[key as usize] }) as char
        }
    } else {
        match key {
            0x54 => '/',
            0x55 => '*',
            0x56 => '-',
            0x57 => '+',
            0x58 => '\n', // Numpad Enter
            0x59 => '1',
            0x5A => '2',
            0x5B => '3',
            0x5C => '4',
            0x5D => '5',
            0x5E => '6',
            0x5F => '7',
            0x60 => '8',
            0x61 => '9',
            0x62 => '0',
            0x63 => '.',
            0x4C => '\u{7F}', // Delete
            // Arrow keys are multi-byte — handled separately, not via repeat
            _ => '\0',
        }
    }
}

/// AltGr characters for Swiss German (de_CH) keyboard layout.
/// HID usage codes → ASCII.
fn altgr_char_de_hid(key: u8) -> Option<char> {
    match key {
        0x08 => Some('€'),   // AltGr+e — xkb: AD03 third level EuroSign
        0x1F => Some('@'),   // AltGr+2
        0x20 => Some('#'),   // AltGr+3
        0x24 => Some('|'),   // AltGr+7
        0x2E => Some('~'),   // AltGr+^ (= key)
        0x2F => Some('['),   // AltGr+AD11 ([ key)
        0x30 => Some(']'),   // AltGr+¨ (] key)
        0x34 => Some('{'),   // AltGr+AC11 (' key)
        0x31 => Some('}'),   // AltGr+$ (\ key)
        0x64 => Some('\\'),  // AltGr+< (non-US \)
        _ => None,
    }
}
