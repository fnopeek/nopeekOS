//! xHCI USB Host Controller Driver
//!
//! HID devices after Linux usbhid (keyboards, mice, receivers that are
//! both) and one USB network device. No hubs.

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
    dcbaa: DmaRegion,
    cmd_ring: DmaRegion,
    cmd_cycle: u32,
    cmd_enqueue: usize,
    evt_ring: DmaRegion,
    evt_cycle: u32,
    evt_dequeue: usize,
    evt_seg_table: DmaRegion,
    input_ctx: DmaRegion,
    device_ctx: DmaRegion,
    ep0_ring: DmaRegion,
    ep0_cycle: u32,
    ep0_enqueue: usize,
    data_buf: DmaRegion, // general-purpose DMA buffer (4KB)
    /// The device the EP0 set above currently belongs to.
    slot_id: u8,
    port_speed: u32,
    /// USB devices with HID interfaces, and the interfaces.
    hid_devs: Vec<HidUsbDev>,
    hid: Vec<HidIntf>,
    /// Pages unplugged HID devices gave back, for the next one.
    free_pages: Vec<DmaRegion>,
    /// Ports with a Port Status Change Event not yet handled (`hotplug`).
    port_changes: [u64; 4],
    /// Completion code of the last command or control transfer, for the
    /// log when enumeration fails; 0 = it timed out.
    last_cc: u32,
    /// DMA buffers for a network device on this controller.
    nic_device_ctx: DmaRegion,
    nic_ep0_ring: DmaRegion,
    /// The network device, if one is attached here. It shares this state
    /// and its event ring dequeue pointer; a second state on the same ring
    /// would steal events.
    nic: Option<NicRings>,
}

/// Maximum number of xHCI controllers tracked. Laptops commonly have two,
/// desktops sometimes three.
const MAX_CTRLS: usize = 4;

/// One state per controller. Devices of different kinds (keyboard, mouse,
/// NIC) may sit on any controller, and several on the same one.
static CTRLS: spin::Mutex<[Option<XhciState>; MAX_CTRLS]> =
    spin::Mutex::new([None, None, None, None]);

/// Store a running controller: replaces the entry with the same PCI
/// address, else takes the first free slot.
fn store_ctrl(state: XhciState) -> bool {
    let addr = state.pci_addr;
    let mut g = CTRLS.lock();
    for i in 0..MAX_CTRLS {
        if g[i].as_ref().map(|s| s.pci_addr) == Some(addr) {
            g[i] = Some(state);
            return true;
        }
    }
    for i in 0..MAX_CTRLS {
        if g[i].is_none() {
            g[i] = Some(state);
            return true;
        }
    }
    kprintln!("[npk] xhci: more than {} controllers — {:02x}:{:02x}.{} not tracked",
        MAX_CTRLS, addr.bus, addr.device, addr.function);
    false
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
/// Returns the running state with no device enumerated yet. `max_slots_en`
/// caps how many device slots the HC will accept (Address Device).
fn bring_up_controller(dev: pci::PciDevice, max_slots_en: u32) -> Option<XhciState> {

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

    // Allocate DMA structures (all page-aligned, zeroed). The EP0 set
    // serves enumeration; a HID device gets its own set when accepted, a
    // network device the nic_* set.
    let (Some(dcbaa), Some(cmd_ring), Some(evt_ring), Some(evt_seg_table),
         Some(input_ctx), Some(device_ctx), Some(ep0_ring),
         Some(data_buf), Some(nic_device_ctx), Some(nic_ep0_ring)) = (
        alloc_dma(1, "DCBAA"),
        alloc_dma(1, "cmd ring"),
        alloc_dma(1, "evt ring"),
        alloc_dma(1, "evt seg table"),
        alloc_dma(1, "input ctx"),
        alloc_dma(1, "device ctx"),
        alloc_dma(1, "EP0 ring"),
        alloc_dma(1, "data buf"),
        alloc_dma(1, "nic dev ctx"),
        alloc_dma(1, "nic EP0"),
    ) else {
        kprintln!("[npk] xhci: DMA alloc failed");
        return None;
    };

    // Set up Link TRBs at end of rings (wrap back to start)
    write_trb(cmd_ring, NUM_CMD_TRBS - 1, cmd_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1)); // Toggle Cycle
    write_trb(ep0_ring, NUM_TR_TRBS - 1, ep0_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    write_trb(nic_ep0_ring, NUM_TR_TRBS - 1, nic_ep0_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));

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
    oper.w32(OP_CONFIG, max_slots_en); // MaxSlotsEn
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
        mmio, oper, rt, db, ctx_size, max_ports,
        dcbaa, cmd_ring, cmd_cycle: 1, cmd_enqueue: 0,
        evt_ring, evt_cycle: 1, evt_dequeue: 0, evt_seg_table,
        input_ctx, device_ctx,
        ep0_ring, ep0_cycle: 1, ep0_enqueue: 0,
        data_buf, slot_id: 0, port_speed: 0,
        hid_devs: Vec::new(), hid: Vec::new(), free_pages: Vec::new(), port_changes: [0; 4], last_cc: 0,
        nic_device_ctx, nic_ep0_ring, nic: None,
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

/// Initialize one xHCI controller and every HID device on it.
fn init_controller(dev: pci::PciDevice) -> bool {
    let mut state = match bring_up_controller(dev, 16) {
        Some(s) => s,
        None => return false,
    };

    let mut seen = [0u64; 4];
    for p in 0..state.max_ports {
        if state.oper.r32(portsc_off(p)) & PORTSC_CCS == 0 { continue; }
        crate::kdebug!("[npk] xhci: trying port {}", p + 1);
        seen[(p / 64) as usize] |= 1 << (p % 64);
        probe_hid_port(&mut state, p);
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

/// Whether `init` found a USB pointer. Enumeration finds every HID
/// interface at once, mice included.
pub fn init_mouse() -> bool {
    USB_POINTERS.load(Ordering::Relaxed) > 0
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

/// A USB device with HID interfaces (Linux `usb_device`). Its device
/// context and EP0 ring stay registered for the slot until it is unplugged;
/// once enumerated, nothing here sends it a control transfer.
struct HidUsbDev {
    slot_id: u8,
    port: u32,
    device_ctx: DmaRegion,
    ep0_ring: DmaRegion,
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
    /// Index in `XhciState::hid_devs`.
    dev: usize,
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
        SPEED_SUPER => 512,
        _ => 64,
    }
}

/// Address the device on `port` with the EP0 set in `state`, then read
/// bMaxPacketSize0 and correct EP0 if it differs (Linux `hub_port_init`
/// with the new scheme: Address Device, GET_DESCRIPTOR 8 bytes,
/// `usb_ep0_reinit` → Evaluate Context).
fn address_device(state: &mut XhciState, port: u32) -> bool {
    let mps0 = initial_mps0(state.port_speed);
    if !cmd_address_device(state, port, mps0) { return false; }
    if !usb_get_descriptor(state, DESC_DEVICE, 8) { return false; }
    let b = state.data_buf.r8(7u32);
    let real = if state.port_speed >= SPEED_SUPER { 1u16 << b.min(15) } else { b as u16 };
    if real != 0 && real != mps0 {
        crate::kdebug!("[npk] xhci: port {} ep0 max packet {} -> {}", port + 1, mps0, real);
        return cmd_evaluate_ep0(state, real);
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

/// Reset, address and read the configuration of the device on `port`:
/// `(slot, device context, EP0 ring, configuration descriptor)`. On failure
/// the slot is released and the pages go back to the pool.
fn usb_port_init(state: &mut XhciState, port: u32) -> Option<(u8, DmaRegion, DmaRegion, Vec<u8>)> {
    let dctx = take_page(state, "hid dev ctx")?;
    let Some(ep0) = take_ring(state, "hid EP0") else {
        state.free_pages.push(dctx);
        return None;
    };
    state.device_ctx = dctx;
    state.ep0_ring = ep0;
    state.ep0_cycle = 1;
    state.ep0_enqueue = 0;
    let give_back = |state: &mut XhciState| {
        state.free_pages.push(dctx);
        state.free_pages.push(ep0);
    };

    if !reset_port(state, port) {
        kprintln!("[npk] xhci: port {} reset failed", port + 1);
        give_back(state);
        return None;
    }
    state.port_speed = (state.oper.r32(portsc_off(port)) >> 10) & 0xF;
    let Some(slot) = cmd_enable_slot(state) else {
        kprintln!("[npk] xhci: enable slot failed");
        give_back(state);
        return None;
    };
    state.slot_id = slot;
    state.dcbaa.w64(slot as usize * 8, dctx.phys());

    let cfg = (|| {
        if !address_device(state, port) {
            port_failed(state, port, "address device");
            return None;
        }
        // Configuration descriptor: 9 bytes for the total length, then all.
        if !usb_get_descriptor(state, DESC_CONFIG, 9) {
            port_failed(state, port, "configuration descriptor");
            return None;
        }
        let total = u16::from_le_bytes([state.data_buf.r8(2u32), state.data_buf.r8(3u32)]) as usize;
        let fetch = total.min(4096);
        if !usb_get_descriptor(state, DESC_CONFIG, fetch as u16) {
            port_failed(state, port, "configuration descriptor");
            return None;
        }
        let mut cfg = alloc::vec![0u8; fetch];
        state.data_buf.copy_out(0u32, &mut cfg);
        Some(cfg)
    })();
    match cfg {
        Some(cfg) => Some((slot, dctx, ep0, cfg)),
        None => {
            usb_release_slot(state, slot);
            give_back(state);
            None
        }
    }
}

/// Which step of enumeration failed, with what the controller said.
fn port_failed(state: &XhciState, port: u32, step: &str) {
    let sc = state.oper.r32(portsc_off(port));
    let speed = match state.port_speed {
        SPEED_LOW => "low", SPEED_FULL => "full", SPEED_HIGH => "high", SPEED_SUPER => "super", _ => "?",
    };
    if state.last_cc == 0 {
        kprintln!("[npk] xhci: port {} {} timed out (USB{}, {} speed, portsc {:#010x})",
            port + 1, step, port_usb_version(state.mmio, port), speed, sc);
    } else {
        kprintln!("[npk] xhci: port {} {} failed, completion code {} (USB{}, {} speed, portsc {:#010x})",
            port + 1, step, state.last_cc, port_usb_version(state.mmio, port), speed, sc);
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

/// Look for HID interfaces on `port`; set up every one found.
fn probe_hid_port(state: &mut XhciState, port: u32) {
    let mut init = None;
    for attempt in 0..PORT_INIT_TRIES {
        if attempt > 0 {
            kprintln!("[npk] xhci: port {} retrying ({}/{})", port + 1, attempt + 1, PORT_INIT_TRIES);
        }
        init = usb_port_init(state, port);
        if init.is_some() { break; }
        if state.oper.r32(portsc_off(port)) & PORTSC_CCS == 0 { return; }
    }
    let Some((slot, dctx, ep0, cfg)) = init else {
        kprintln!("[npk] xhci: port {}: unable to enumerate USB device, port disabled", port + 1);
        hub_port_disable(state, port);
        return;
    };
    let config_val = cfg.get(5).copied().unwrap_or(1);

    let descs = parse_hid_interfaces(&cfg);
    if descs.is_empty() {
        crate::kdebug!("[npk] xhci: port {} has no HID interface, releasing slot {}", port + 1, slot);
        usb_release_slot(state, slot);
        state.free_pages.push(dctx);
        state.free_pages.push(ep0);
        return;
    }

    // The device is ours: it keeps this context and EP0 ring.
    let dev_idx = state.hid_devs.len();
    state.hid_devs.push(HidUsbDev { slot_id: slot, port, device_ctx: dctx, ep0_ring: ep0 });

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
            dev: dev_idx, ifnum: d.ifnum, keyboard: false, pointer: false,
            in_dci, in_ring, in_cycle: 1, in_enqueue: 0, out, buf,
            insize: 0, report: Vec::new(), hid: hid_core::Device::default(),
            input: hid_core::input::Input::default(), kbd: KbdState::new(),
            error_count: 0, running: false,
        });
    }
    if !cmd_configure_endpoints(state, &eps) {
        kprintln!("[npk] xhci: port {} configure endpoints failed", port + 1);
        return;
    }
    if !usb_set_config(state, config_val) {
        kprintln!("[npk] xhci: port {} set configuration failed", port + 1);
        return;
    }

    for (n, d) in descs.iter().enumerate() {
        let i = first + n;
        if i >= state.hid.len() { break; }
        if !usbhid_parse(state, i, d) { continue; }
        usbhid_start(state, i, d);
    }
}

/// The device on `port` is gone (`usb_disconnect`): its interfaces stop,
/// its slot is released, its pages go back to the pool.
fn hid_disconnect(state: &mut XhciState, d: usize) {
    let dev = &state.hid_devs[d];
    let (slot, port, dctx, ep0) = (dev.slot_id, dev.port, dev.device_ctx, dev.ep0_ring);
    let mut pages = Vec::new();
    for h in state.hid.iter_mut().filter(|h| h.dev == d) {
        hid_stop(h);
        pages.push(h.in_ring);
        pages.push(h.buf);
        if let Some(o) = h.out.as_ref() { pages.push(o.ring); }
    }
    state.hid.retain(|h| h.dev != d);
    for h in state.hid.iter_mut() {
        if h.dev > d { h.dev -= 1; }
    }
    state.hid_devs.remove(d);
    usb_release_slot(state, slot);
    state.free_pages.extend(pages);
    state.free_pages.push(dctx);
    state.free_pages.push(ep0);
    kprintln!("[npk] xhci: USB device on port {} gone", port + 1);
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
fn hub_port_debounce(oper: Mmio, port: u32) -> Option<bool> {
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
/// is enumerated, one that went is torn down.
fn hotplug_pass() {
    if !HOTPLUG.swap(false, Ordering::AcqRel) { return; }
    // Which ports, and whether each saw a connect change. Read and cleared
    // under the lock; the debounce waits outside it, so the interrupt keeps
    // draining every other device meanwhile.
    let mut work: Vec<(pci::PciAddr, Mmio, u32, bool)> = Vec::new();
    {
        let mut g = CTRLS.lock();
        for s in g.iter_mut().flatten() {
            let nic_port = s.nic.as_ref().map(|n| n.port_num);
            for w in 0..4 {
                let mut bits = core::mem::take(&mut s.port_changes[w]);
                while bits != 0 {
                    let p = (w * 64) as u32 + bits.trailing_zeros();
                    bits &= bits - 1;
                    if p >= s.max_ports || Some(p) == nic_port { continue; }
                    let off = portsc_off(p);
                    let sc = s.oper.r32(off);
                    if sc & PORTSC_CHANGES != 0 {
                        s.oper.w32(off, port_neutral(sc) | (sc & PORTSC_CHANGES));
                    }
                    let gone = sc & PORTSC_CCS == 0 && s.hid_devs.iter().any(|d| d.port == p);
                    if sc & PORTSC_CSC != 0 || gone {
                        work.push((s.pci_addr, s.oper, p, sc & PORTSC_CSC != 0));
                    }
                }
            }
        }
    }
    for (addr, oper, port, changed) in work {
        let connected = if changed {
            hub_port_debounce(oper, port).unwrap_or(false)
        } else {
            false
        };
        let mut g = CTRLS.lock();
        let Some(s) = g.iter_mut().flatten().find(|s| s.pci_addr == addr) else { continue };
        if let Some(d) = s.hid_devs.iter().position(|d| d.port == port) {
            hid_disconnect(s, d);
        }
        if connected {
            kprintln!("[npk] xhci: device connected on port {}", port + 1);
            probe_hid_port(s, port);
        }
    }
}

/// `usbhid_parse`: SET_IDLE, then the report descriptor, parsed.
fn usbhid_parse(state: &mut XhciState, i: usize, d: &HidIntfDesc) -> bool {
    let ifnum = d.ifnum as u16;
    let rsize = d.report_len;
    if rsize == 0 || rsize > HID_MAX_DESCRIPTOR_SIZE {
        kprintln!("[npk] xhci: hid interface {}: weird size of report descriptor ({})", ifnum, rsize);
        return false;
    }
    let _ = usb_control_transfer(state, 0x21, HID_REQ_SET_IDLE, 0, ifnum, 0, false);

    // `hid_get_class_descriptor`: up to four tries.
    let mut ok = false;
    for _ in 0..4 {
        state.data_buf.fill(0u32, 0, rsize as u64);
        if usb_control_transfer(state, 0x81, USB_GET_DESCRIPTOR, (HID_DT_REPORT as u16) << 8,
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
    let dv = &state.hid_devs[state.hid[i].dev];
    kprintln!("[npk] xhci: USB HID on {:02x}:{:02x}.{} port {} interface {}{}{}",
        state.pci_addr.bus, state.pci_addr.device, state.pci_addr.function,
        dv.port + 1, d.ifnum,
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
        let slot = state.hid_devs[h.dev].slot_id as u32;
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
        state.data_buf.copy_in(0u32, &report);
        let _ = usb_control_transfer(state, 0x21, HID_REQ_SET_REPORT, (2u16 << 8) | id as u16,
            ifnum, report.len() as u16, false);
    }
}

/// Queue the next IN transfer of interface `i` (`hid_start_in`).
fn schedule_hid_in(state: &mut XhciState, i: usize) {
    let slot = state.hid_devs[state.hid[i].dev].slot_id as u32;
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
        if state.hid[i].error_count >= 3 {
            state.hid[i].error_count = 0;
            let port = state.hid_devs[state.hid[i].dev].port;
            if state.oper.r32(portsc_off(port)) & PORTSC_CCS == 0 {
                // Unplugged: do not re-queue.
                hid_stop(&mut state.hid[i]);
                return;
            }
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

#[allow(dead_code)]
fn find_connected_port(state: &XhciState) -> Option<u32> {
    for p in 0..state.max_ports {
        let sc = state.oper.r32(portsc_off(p));
        if sc & PORTSC_CCS != 0 {
            return Some(p);
        }
    }
    None
}

/// Read USB string descriptor `idx` into `out` as ASCII (best effort:
/// UTF-16LE low bytes, printable only). Returns the byte count written.
fn usb_get_string(state: &mut XhciState, idx: u8, out: &mut [u8]) -> usize {
    if idx == 0 { return 0; }
    // wValue = (STRING desc type 0x03 << 8) | index, wIndex = langid 0x0409.
    if !usb_control_transfer(state, 0x80, USB_GET_DESCRIPTOR,
        0x0300 | idx as u16, 0x0409, 255, true) {
        return 0;
    }
    let blen = state.data_buf.r8(0u32) as usize;
    if blen < 2 || state.data_buf.r8(1u32) != 0x03 { return 0; }
    let mut n = 0usize;
    let mut i = 2usize;
    while i + 1 < blen && n < out.len() {
        let lo = state.data_buf.r8(i as u32);
        let hi = state.data_buf.r8(i as u32 + 1);
        if hi == 0 && (0x20..0x7F).contains(&lo) { out[n] = lo; n += 1; }
        i += 2;
    }
    n
}

/// Enumerate every USB device on every xHCI controller and print a table
/// (controller BDF, port, VID:PID, identifying class, product name). Read-only
/// probing: each device is slot-enabled, addressed, queried, then released.
/// Brings each controller up fresh, so a USB keyboard/mouse on a probed
/// controller may need re-plugging afterwards.
pub fn list_devices() {
    let mut total = 0u32;
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
                    let pci_dev = pci::PciDevice {
                        addr,
                        vendor_id: (id & 0xFFFF) as u16,
                        device_id: ((id >> 16) & 0xFFFF) as u16,
                        bar0: pci::read32(addr, 0x10),
                        irq_line: pci::read8(addr, 0x3C),
                    };
                    total += enumerate_controller(pci_dev);
                }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }
    kprintln!();
    kprintln!("  {} USB device(s) found", total);
    kprintln!();
}

/// Bring up one controller, address each connected device, print its identity.
/// Returns how many devices were reported.
fn enumerate_controller(dev: pci::PciDevice) -> u32 {
    let (cbus, cdev, cfunc) = (dev.addr.bus, dev.addr.device, dev.addr.function);
    let mut state = match bring_up_controller(dev, 16) {
        Some(s) => s,
        None => return 0,
    };
    let mut found = 0u32;

    for p in 0..state.max_ports {
        let sc = state.oper.r32(portsc_off(p));
        if sc & PORTSC_CCS == 0 { continue; }
        state.port_speed = (sc >> 10) & 0xF;

        // USB3 (SuperSpeed) root ports are link-trained and Enabled (PED) at
        // connect; a USB2-style Port Reset on an already-enabled port confuses
        // it and the following Address Device fails. Only PR-reset ports that
        // aren't enabled yet (USB2 low/full/high speed).
        if sc & PORTSC_PED == 0 {
            if !reset_port(&state, p) {
                kprintln!("  port {} reset failed", p + 1);
                continue;
            }
            state.port_speed = (state.oper.r32(portsc_off(p)) >> 10) & 0xF;
        }

        let slot_id = match cmd_enable_slot(&mut state) {
            Some(s) => s,
            None => { kprintln!("  port {} enable-slot failed", p + 1); continue; }
        };
        state.slot_id = slot_id;
        state.dcbaa.w64(slot_id as usize * 8, state.device_ctx.phys());

        reset_ep0_ring(&mut state);
        if !address_device(&mut state, p) {
            kprintln!("  port {} address-device failed (speed {})", p + 1, state.port_speed);
            cmd_disable_slot(&mut state, slot_id);
            continue;
        }

        // Device descriptor (18 bytes): VID/PID, device class, iProduct.
        if !usb_get_descriptor(&mut state, DESC_DEVICE, 18) {
            kprintln!("  port {} device-descriptor failed", p + 1);
            cmd_disable_slot(&mut state, slot_id);
            continue;
        }
        let vid = u16::from_le_bytes([state.data_buf.r8(8u32), state.data_buf.r8(9u32)]);
        let pid = u16::from_le_bytes([state.data_buf.r8(10u32), state.data_buf.r8(11u32)]);
        let dclass = state.data_buf.r8(4u32);
        let dsub = state.data_buf.r8(5u32);
        let i_product = state.data_buf.r8(15u32);

        // First interface descriptor → its class. Devices whose device-class is
        // 0 (per-interface) or 0xFF (vendor) defer the real class here — most
        // USB-NICs do exactly this.
        let (mut iclass, mut isub, mut iproto) = (0u8, 0u8, 0u8);
        if usb_get_descriptor(&mut state, DESC_CONFIG, 9) {
            let total_len = u16::from_le_bytes([state.data_buf.r8(2u32), state.data_buf.r8(3u32)]) as usize;
            let fetch = total_len.min(256) as u16;
            if usb_get_descriptor(&mut state, DESC_CONFIG, fetch) {
                let mut pos = 0usize;
                while pos + 2 <= fetch as usize {
                    let blen = state.data_buf.r8(pos as u32) as usize;
                    if blen == 0 { break; }
                    if state.data_buf.r8(pos as u32 + 1) == 0x04 {
                        iclass = state.data_buf.r8(pos as u32 + 5);
                        isub = state.data_buf.r8(pos as u32 + 6);
                        iproto = state.data_buf.r8(pos as u32 + 7);
                        break;
                    }
                    pos += blen;
                }
            }
        }

        let mut name = [0u8; 40];
        let nlen = usb_get_string(&mut state, i_product, &mut name);

        let vendor = dclass == 0x00 || dclass == 0xFF;
        let eff_class = if vendor { iclass } else { dclass };
        let eff_sub = if vendor { isub } else { dsub };

        kprintln!("  {:02x}:{:02x}.{} port {}  {:04x}:{:04x}  {}",
            cbus, cdev, cfunc, p + 1, vid, pid,
            usb_class_name(eff_class, eff_sub, iproto));
        let known = usb_vendor_product(vid, pid);
        if !known.is_empty() {
            kprintln!("              {}", known);
        } else if nlen > 0 {
            kprintln!("              {}", core::str::from_utf8(&name[..nlen]).unwrap_or(""));
        }

        found += 1;
        cmd_disable_slot(&mut state, slot_id);
    }
    found
}

fn usb_class_name(class: u8, sub: u8, proto: u8) -> &'static str {
    match (class, sub, proto) {
        (0x01, _, _) => "Audio",
        (0x02, _, _) => "Communications (CDC)",
        (0x03, 0x01, 0x01) => "HID keyboard",
        (0x03, 0x01, 0x02) => "HID mouse",
        (0x03, _, _) => "HID",
        (0x08, _, _) => "Mass storage",
        (0x09, _, _) => "USB hub",
        (0x0A, _, _) => "CDC data (Ethernet)",
        (0x0E, _, _) => "Video",
        (0xE0, 0x01, 0x03) => "Wireless (RNDIS)",
        (0xE0, _, _) => "Wireless",
        (0xFF, _, _) => "Vendor-specific",
        _ => "Unknown class",
    }
}

/// Known USB chips relevant to the driver catalog (NIC dongles first).
fn usb_vendor_product(vid: u16, pid: u16) -> &'static str {
    match (vid, pid) {
        (0x0bda, 0x8153) => "Realtek RTL8153 USB GbE (r8152)",
        (0x0bda, 0x8152) => "Realtek RTL8152 USB FE (r8152)",
        (0x0b95, 0x1790) => "ASIX AX88179 USB GbE",
        (0x0b95, 0x178a) => "ASIX AX88179A USB GbE",
        (0x0bda, _) => "Realtek",
        (0x0b95, _) => "ASIX",
        _ => "",
    }
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

#[allow(dead_code)]
fn cmd_disable_slot(state: &mut XhciState, slot_id: u8) {
    let slot_field = (slot_id as u32) << 24;
    post_command(state, 0, 0, TRB_DISABLE_SLOT | slot_field);
    let _ = wait_command_completion(state); // best effort
    // Clear DCBAA entry
    state.dcbaa.w64(slot_id as usize * 8, 0);
}

#[allow(dead_code)]
fn reset_ep0_ring(state: &mut XhciState) {
    state.ep0_ring.fill(0u32, 0, 4096);
    write_trb(state.ep0_ring, NUM_TR_TRBS - 1, state.ep0_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    state.ep0_cycle = 1;
    state.ep0_enqueue = 0;
}

fn cmd_address_device(state: &mut XhciState, port: u32, max_packet: u16) -> bool {
    let ctx = state.ctx_size;
    let input = state.input_ctx;

    input.fill(0u32, 0, 4096);

    // Input Control Context: Add Slot (bit 0) + EP0 (bit 1)
    input.w32(4u32, 0x03); // Add flags at offset 4

    // Slot Context (at input + ctx_size * 1)
    let slot_off = ctx;
    let route_speed_entries = (1 << 27) | // Context Entries = 1
        ((state.port_speed as u32) << 20); // Speed
    input.w32(slot_off, route_speed_entries);
    // Dword 1: Root Hub Port Number (1-based)
    input.w32(slot_off + 4, ((port + 1) as u32) << 16);

    // EP0 Context (at input + ctx_size * 2)
    let ep0_off = ctx * 2;
    let ep_type_mps = (EP_TYPE_CONTROL << 3) | (3 << 1); // CErr=3, EP Type=Control
    let mps_field = (max_packet as u32) << 16;
    let ring = state.ep0_ring.phys();
    // Dword 1: CErr + EP Type
    input.w32(ep0_off + 4, ep_type_mps | mps_field);
    // Dword 2-3: TR Dequeue Pointer (with DCS=1)
    input.w32(ep0_off + 8, (ring as u32) | 1);
    input.w32(ep0_off + 12, (ring >> 32) as u32);
    // Dword 4: Average TRB Length
    input.w32(ep0_off + 16, 8);

    // Post Address Device Command
    let slot_field = (state.slot_id as u32) << 24;
    post_command(state, input.phys(), 0, TRB_ADDRESS_DEVICE | slot_field);
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

// === USB Control Transfers ===

fn usb_control_transfer(state: &mut XhciState, bm_request: u8, b_request: u8,
    w_value: u16, w_index: u16, w_length: u16, dir_in: bool) -> bool
{
    // The whole TD must fit before the Link TRB in the last slot. If it
    // does not, the rest of the ring becomes No-Op TDs and the TD starts
    // at the top after the link; otherwise its Data or Status stage would
    // land on the Link TRB and past the ring.
    let needed = if w_length > 0 { 3 } else { 2 };
    if state.ep0_enqueue + needed > NUM_TR_TRBS - 1 {
        let c = state.ep0_cycle;
        for i in state.ep0_enqueue..NUM_TR_TRBS - 1 {
            write_trb(state.ep0_ring, i, 0, 0, TRB_TR_NOOP | c);
        }
        write_trb(state.ep0_ring, NUM_TR_TRBS - 1, state.ep0_ring.phys(), 0, TRB_LINK | c | (1 << 1));
        state.ep0_cycle ^= 1;
        state.ep0_enqueue = 0;
    }

    let ep0 = &mut state.ep0_enqueue;
    let cycle = state.ep0_cycle;

    // Setup Stage TRB
    let setup_lo = bm_request as u32 | ((b_request as u32) << 8)
        | ((w_value as u32) << 16);
    let setup_hi = w_index as u32 | ((w_length as u32) << 16);
    let setup_param = setup_lo as u64 | ((setup_hi as u64) << 32);
    let trt = if w_length == 0 { TRB_TRT_NO } else if dir_in { TRB_TRT_IN } else { 0x02 << 16 };
    write_trb(state.ep0_ring, *ep0, setup_param, 8, TRB_SETUP_STAGE | TRB_IDT | trt | cycle);
    *ep0 += 1;

    // Data Stage TRB (if needed)
    if w_length > 0 {
        let dir_bit = if dir_in { TRB_DIR_IN } else { 0 };
        write_trb(state.ep0_ring, *ep0, state.data_buf.phys(), w_length as u32, TRB_DATA_STAGE | dir_bit | cycle);
        *ep0 += 1;
    }

    // Status Stage TRB
    let status_dir = if w_length > 0 && dir_in { 0 } else { TRB_DIR_IN };
    write_trb(state.ep0_ring, *ep0, 0, 0, TRB_STATUS_STAGE | TRB_IOC | status_dir | cycle);
    *ep0 += 1;

    // Wrap check
    if *ep0 >= NUM_TR_TRBS - 1 {
        let link_ctrl = TRB_LINK | cycle | (1 << 1);
        write_trb(state.ep0_ring, NUM_TR_TRBS - 1, state.ep0_ring.phys(), 0, link_ctrl);
        state.ep0_cycle ^= 1;
        *ep0 = 0;
    }

    // Ring doorbell for slot, target EP0 (DCI=1)
    ring_doorbell(state, state.slot_id as u32, 1);

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
        if in_ring(e.param, state.ep0_ring.phys()) {
            state.last_cc = e.cc;
            return ok;
        }
        if dispatch_transfer(state, &e) {
            continue;
        }
        // Belongs to no known device: treat it as ours, so a setup whose
        // event cannot be attributed does not run into the timeout.
        return ok;
    }
}

fn usb_get_descriptor(state: &mut XhciState, desc_type: u16, length: u16) -> bool {
    state.data_buf.fill(0u32, 0, length as u64);
    usb_control_transfer(state, 0x80, USB_GET_DESCRIPTOR, desc_type, 0, length, true)
}

fn usb_set_config(state: &mut XhciState, config_val: u8) -> bool {
    usb_control_transfer(state, 0x00, USB_SET_CONFIG, config_val as u16, 0, 0, false)
}



// === Configure Endpoints — Linux xhci_endpoint_init, xhci_check_maxpacket ===

/// The xHCI interval of a periodic endpoint (`xhci_get_endpoint_interval`):
/// at high and super speed bInterval is an exponent, at full and low speed
/// a number of frames.
fn xhci_interval(speed: u32, b_interval: u8) -> u32 {
    match speed {
        SPEED_HIGH | SPEED_SUPER => (b_interval.clamp(1, 16) - 1) as u32,
        _ => {
            // `xhci_parse_frame_interval`: fls(8 * bInterval) - 1, in 3..10.
            let v = 8 * b_interval as u32;
            let fls = 32 - v.leading_zeros();
            fls.saturating_sub(1).clamp(3, 10)
        }
    }
}

/// Configure interrupt endpoints of the current slot in one command:
/// `(dci, ring, type, wMaxPacketSize, bInterval)` each.
fn cmd_configure_endpoints(state: &mut XhciState, eps: &[(u8, u64, u32, u16, u8)]) -> bool {
    if eps.is_empty() { return true; }
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

    // Slot Context: the output one (`xhci_slot_copy`), with Context Entries
    // raised to the last endpoint.
    for k in 0..4u32 {
        input.w32(ctx + (k * 4) as usize, state.device_ctx.r32(k * 4));
    }
    let dw0 = (input.r32(ctx) & !(0x1F << 27)) | ((last as u32) << 27);
    input.w32(ctx, dw0);

    for &(dci, ring, ep_type, w_max, b_interval) in eps {
        let ep_off = ctx * (dci as usize + 1);
        let maxp = (w_max & 0x7FF) as u32;
        // High-speed periodic endpoints carry extra transactions in bits
        // 12..11 (`xhci_get_endpoint_max_burst`).
        let burst = if state.port_speed == SPEED_HIGH { ((w_max >> 11) & 0x3) as u32 } else { 0 };
        let esit = maxp * (burst + 1);
        let interval = xhci_interval(state.port_speed, b_interval);
        input.w32(ep_off, (interval << 16) | ((esit >> 16) << 24));
        input.w32(ep_off + 4, (3 << 1) | (ep_type << 3) | (burst << 8) | (maxp << 16));
        input.w32(ep_off + 8, (ring as u32) | 1);
        input.w32(ep_off + 12, (ring >> 32) as u32);
        // Interrupt: average TRB length = max ESIT payload.
        input.w32(ep_off + 16, ((esit & 0xFFFF) << 16) | (esit & 0xFFFF));
    }

    let slot_field = (state.slot_id as u32) << 24;
    post_command(state, input.phys(), 0, TRB_CONFIGURE_EP | slot_field);
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

/// Correct EP0's max packet size of the current slot (Evaluate Context).
fn cmd_evaluate_ep0(state: &mut XhciState, max_packet: u16) -> bool {
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    input.fill(0u32, 0, 4096);
    // Add EP0 only; its context is the output one with a new size.
    input.w32(4u32, 1 << 1);
    for k in 0..(ctx as u32 / 4) {
        input.w32(ctx * 2 + (k * 4) as usize, state.device_ctx.r32(ctx as u32 + k * 4));
    }
    let dw1 = (input.r32(ctx * 2 + 4) & 0x0000_FFFF) | ((max_packet as u32) << 16);
    input.w32(ctx * 2 + 4, dw1);

    let slot_field = (state.slot_id as u32) << 24;
    post_command(state, input.phys(), 0, TRB_EVALUATE_CTX | slot_field);
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
    /// Slot and port of the device (one of several on the controller).
    slot_id: u8,
    port_num: u32,
    port_speed: u32,
    /// Own EP0 state (runtime control transfers, e.g. link status).
    ep0_cycle: u32,
    ep0_enqueue: usize,
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
    for i in 0..MAX_CTRLS {
        if let Some(s) = g[i].as_mut() {
            if s.nic.is_some() { return Some(f(s)); }
        }
    }
    None
}

/// Switch the EP0 set to the network device, run `f`, switch back.
///
/// `usb_control_transfer` works on the set currently in the state, which is
/// the one enumeration left there. The NIC needs this at runtime because
/// `rtl8153` reads the link status through control transfers.
fn with_nic_ep0<R>(state: &mut XhciState, f: impl FnOnce(&mut XhciState) -> R) -> R {
    let saved = (state.slot_id, state.ep0_ring, state.ep0_cycle,
                 state.ep0_enqueue, state.device_ctx, state.port_speed);
    if let Some(n) = state.nic.as_ref() {
        state.slot_id = n.slot_id;
        state.ep0_cycle = n.ep0_cycle;
        state.ep0_enqueue = n.ep0_enqueue;
        state.port_speed = n.port_speed;
    }
    state.ep0_ring = state.nic_ep0_ring;
    state.device_ctx = state.nic_device_ctx;
    let r = f(state);
    if let Some(n) = state.nic.as_mut() {
        n.ep0_cycle = state.ep0_cycle;
        n.ep0_enqueue = state.ep0_enqueue;
    }
    state.slot_id = saved.0;
    state.ep0_ring = saved.1;
    state.ep0_cycle = saved.2;
    state.ep0_enqueue = saved.3;
    state.device_ctx = saved.4;
    state.port_speed = saved.5;
    r
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
    let g = CTRLS.lock();
    g.iter().flatten().any(|s| s.nic.is_some())
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

/// Find and set up the network device on one running controller without
/// resetting it, so every other device keeps its slot (a controller reset
/// via `bring_up_controller` would drop them all).
///
/// The keyboard and mouse ports are skipped: a port reset there would drop
/// a device that is already addressed.
fn nic_probe(state: &mut XhciState, vid: u16, pid: u16, ep_in: u8, ep_out: u8) -> bool {
    if state.nic.is_some() { return true; }
    dump_nic_ports(state);

    // Save the current EP0 set.
    let saved = (state.slot_id, state.ep0_ring, state.ep0_cycle,
                 state.ep0_enqueue, state.device_ctx, state.port_speed);
    let mut found = false;

    for p in 0..state.max_ports {
        if state.hid_devs.iter().any(|d| d.port == p) { continue; }
        if state.oper.r32(portsc_off(p)) & PORTSC_CCS == 0 { continue; }

        if !reset_port(state, p) { continue; }
        state.port_speed = (state.oper.r32(portsc_off(p)) >> 10) & 0xF;

        // Own fresh set for this device.
        state.nic_ep0_ring.fill(0u32, 0, 4096);
        state.nic_device_ctx.fill(0u32, 0, 4096);
        write_trb(state.nic_ep0_ring, NUM_TR_TRBS - 1, state.nic_ep0_ring.phys(), 0,
            TRB_LINK | TRB_CYCLE | (1 << 1));
        state.ep0_ring = state.nic_ep0_ring;
        state.device_ctx = state.nic_device_ctx;
        state.ep0_cycle = 1;
        state.ep0_enqueue = 0;

        let slot = match cmd_enable_slot(state) { Some(s) => s, None => continue };
        state.slot_id = slot;
        state.dcbaa.w64(slot as usize * 8, state.device_ctx.phys());

        if !address_device(state, p) { cmd_disable_slot(state, slot); continue; }
        if !usb_get_descriptor(state, DESC_DEVICE, 18) { cmd_disable_slot(state, slot); continue; }
        let dvid = u16::from_le_bytes([state.data_buf.r8(8u32), state.data_buf.r8(9u32)]);
        let dpid = u16::from_le_bytes([state.data_buf.r8(10u32), state.data_buf.r8(11u32)]);
        if dvid != vid || dpid != pid {
            // Not ours: release the slot, or the next attempt on this port
            // fails.
            cmd_disable_slot(state, slot);
            continue;
        }

        if !usb_get_descriptor(state, DESC_CONFIG, 9) { cmd_disable_slot(state, slot); break; }
        let config_val = state.data_buf.r8(5u32);
        if !usb_set_config(state, config_val) { cmd_disable_slot(state, slot); break; }

        let (Some(in_ring), Some(out_ring), Some(in_buf), Some(out_buf)) = (
            alloc_dma(1, "nic bulk-in ring"),
            alloc_dma(1, "nic bulk-out ring"),
            alloc_dma(NIC_RX_BUFS * NIC_BULK_BUF_PAGES, "nic in bufs"),
            alloc_dma((NIC_TX_BUFS * NIC_TX_BUF_BYTES).div_ceil(4096), "nic tx bufs"),
        ) else {
            kprintln!("[npk] xhci: nic DMA alloc failed");
            cmd_disable_slot(state, slot);
            break;
        };
        // Bulk-IN uses NIC_RX_BUFS slots; bulk-OUT uses the whole ring.
        write_trb(in_ring, NIC_RX_BUFS, in_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
        write_trb(out_ring, NUM_TR_TRBS - 1, out_ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));

        let in_dci = ep_in * 2 + 1;   // IN endpoint
        let out_dci = ep_out * 2;     // OUT endpoint
        let bulk_mp = if state.port_speed == SPEED_SUPER { 1024 } else { 512 };
        if !cmd_configure_bulk(state, in_dci, in_ring.phys(), out_dci, out_ring.phys(), bulk_mp) {
            kprintln!("[npk] xhci: nic configure-bulk failed (speed {})", state.port_speed);
            cmd_disable_slot(state, slot);
            break;
        }

        kprintln!("[npk] xhci: NIC attached on {:02x}:{:02x}.{} port {} (slot {}, bulk in EP{} out EP{}, speed {})",
            state.pci_addr.bus, state.pci_addr.device, state.pci_addr.function,
            p + 1, slot, ep_in, ep_out, state.port_speed);

        state.nic = Some(NicRings {
            slot_id: slot, port_num: p, port_speed: state.port_speed,
            ep0_cycle: state.ep0_cycle, ep0_enqueue: state.ep0_enqueue,
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
        found = true;
        break;
    }

    // Restore the keyboard's EP0 set.
    state.slot_id = saved.0;
    state.ep0_ring = saved.1;
    state.ep0_cycle = saved.2;
    state.ep0_enqueue = saved.3;
    state.device_ctx = saved.4;
    state.port_speed = saved.5;
    found
}

/// Find and set up `vid:pid` on any controller.
///
/// 1. Gentle: probe every running controller; nothing is reset.
/// 2. Fallback: bring up (reset) a controller not in the table, store it,
///    and probe it the same way.
/// 3. Last resort: reset the known controllers and probe again.
///
/// The fallbacks exist because without network there is no way to update.
pub fn nic_attach(vid: u16, pid: u16, ep_in: u8, ep_out: u8) -> bool {
    if nic_attached() { return true; }

    {
        let mut g = CTRLS.lock();
        for i in 0..MAX_CTRLS {
            if let Some(s) = g[i].as_mut() {
                if nic_probe(s, vid, pid, ep_in, ep_out) { return true; }
            }
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
                    if let Some(x) = bring_up_controller(pci_dev, 16) {
                        store_ctrl(x);
                        let mut g = CTRLS.lock();
                        for i in 0..MAX_CTRLS {
                            let hit = g[i].as_ref().map(|s| s.pci_addr == addr).unwrap_or(false);
                            if !hit { continue; }
                            if let Some(s) = g[i].as_mut() {
                                if nic_probe(s, vid, pid, ep_in, ep_out) { return true; }
                            }
                        }
                    }
                }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }

    // Last resort: reset the known controllers. `init()` stores every
    // controller, so the previous fallback usually finds none; losing the
    // devices on a controller is better than having no network.
    let mut addrs: [Option<pci::PciAddr>; MAX_CTRLS] = [None; MAX_CTRLS];
    {
        let g = CTRLS.lock();
        for i in 0..MAX_CTRLS {
            addrs[i] = g[i].as_ref().map(|s| s.pci_addr);
        }
    }
    for a in addrs.into_iter().flatten() {
        kprintln!("[npk] xhci: NIC not found gently on {:02x}:{:02x}.{} — falling back to a \
                   controller RESET (everything addressed there loses its slot)",
            a.bus, a.device, a.function);
        {
            let mut g = CTRLS.lock();
            for i in 0..MAX_CTRLS {
                if g[i].as_ref().map(|s| s.pci_addr) == Some(a) {
                    if let Some(s) = g[i].as_mut() { hid_forget(s); }
                    g[i] = None;
                }
            }
        }
        let id = pci::read32(a, 0x00);
        let pci_dev = pci::PciDevice {
            addr: a,
            vendor_id: (id & 0xFFFF) as u16,
            device_id: ((id >> 16) & 0xFFFF) as u16,
            bar0: pci::read32(a, 0x10),
            irq_line: pci::read8(a, 0x3C),
        };
        let x = match bring_up_controller(pci_dev, 16) { Some(x) => x, None => continue };
        store_ctrl(x);
        let mut g = CTRLS.lock();
        for i in 0..MAX_CTRLS {
            if g[i].as_ref().map(|s| s.pci_addr) != Some(a) { continue; }
            if let Some(s) = g[i].as_mut() {
                if nic_probe(s, vid, pid, ep_in, ep_out) { return true; }
            }
        }
    }
    false
}

/// Whether the table already tracks this controller.
fn ctrl_known(addr: pci::PciAddr) -> bool {
    let g = CTRLS.lock();
    g.iter().flatten().any(|s| s.pci_addr == addr)
}



/// Configure both bulk endpoints (IN + OUT) in one Configure Endpoint command.
fn cmd_configure_bulk(state: &mut XhciState, in_dci: u8, in_ring: u64,
    out_dci: u8, out_ring: u64, max_pkt: u16) -> bool
{
    let ctx = state.ctx_size;
    let input = state.input_ctx;
    input.fill(0u32, 0, 4096);

    let max_dci = in_dci.max(out_dci);
    // Input Control Context: add Slot + both endpoints
    input.w32(4u32, 1 | (1u32 << in_dci) | (1u32 << out_dci));
    // Slot context: Context Entries = highest DCI, plus speed
    let slot_off = ctx;
    input.w32(slot_off, ((max_dci as u32) << 27) | ((state.port_speed as u32) << 20));

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

    let slot_field = (state.slot_id as u32) << 24;
    post_command(state, input.phys(), 0, TRB_CONFIGURE_EP | slot_field);
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
        let ok = with_nic_ep0(state, |s|
            usb_control_transfer(s, req_type, request, value, index, len, dir_in));
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
        for slot in g.iter_mut().flatten() {
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
        for slot in g.iter_mut().flatten() {
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
        for slot in g.iter_mut().flatten() {
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
