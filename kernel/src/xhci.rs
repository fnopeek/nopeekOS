//! xHCI USB Host Controller Driver
//!
//! Minimal implementation for USB HID boot-protocol keyboards.
//! Polling model, single device, no hubs.

use crate::{kprintln, pci, paging, memory};
use crate::paging::PageFlags;
use core::sync::atomic::{AtomicBool, Ordering, fence};

// === MMIO helpers ===

fn r32(base: u64, off: u32) -> u32 {
    // SAFETY: MMIO read from mapped, uncacheable region
    unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) }
}
fn w32(base: u64, off: u32, val: u32) {
    // SAFETY: MMIO write to mapped, uncacheable region
    unsafe { core::ptr::write_volatile((base + off as u64) as *mut u32, val); }
}
fn r8(base: u64, off: u32) -> u8 {
    // SAFETY: MMIO read
    unsafe { core::ptr::read_volatile((base + off as u64) as *const u8) }
}
fn w64(base: u64, off: u32, val: u64) {
    // SAFETY: 64-bit MMIO write (low then high)
    w32(base, off, val as u32);
    w32(base, off + 4, (val >> 32) as u32);
}

// === Capability register offsets (from BAR0) ===
const CAP_CAPLENGTH:  u32 = 0x00;
const CAP_HCSPARAMS1: u32 = 0x04;
const CAP_HCSPARAMS2: u32 = 0x08;
const CAP_HCCPARAMS1: u32 = 0x10;
const CAP_DBOFF:      u32 = 0x14;
const CAP_RTSOFF:     u32 = 0x18;

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

// USBSTS bits
const STS_HCH: u32 = 1 << 0;  // HC Halted
const STS_CNR: u32 = 1 << 11; // Controller Not Ready

// PORTSC bits
const PORTSC_CCS:   u32 = 1 << 0;  // Current Connect Status
const PORTSC_PED:   u32 = 1 << 1;  // Port Enabled
const PORTSC_PR:    u32 = 1 << 4;  // Port Reset
#[allow(dead_code)]
const PORTSC_PLS_MASK: u32 = 0xF << 5; // Port Link State
const PORTSC_PP:    u32 = 1 << 9;  // Port Power
#[allow(dead_code)]
const PORTSC_SPEED_MASK: u32 = 0xF << 10;
const PORTSC_PRC:   u32 = 1 << 21; // Port Reset Change
const PORTSC_CSC:   u32 = 1 << 17; // Connect Status Change
const PORTSC_PEC:   u32 = 1 << 18; // Port Enabled Change
const PORTSC_WRC:   u32 = 1 << 19; // Warm Port Reset Change
const PORTSC_OCC:   u32 = 1 << 20; // Over-current Change
const PORTSC_PLC:   u32 = 1 << 22; // Port Link State Change
const PORTSC_CEC:   u32 = 1 << 23; // Config Error Change
// RW1C bits mask — write 0 to these to avoid accidentally clearing them
const PORTSC_RW1C: u32 = PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC;

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
const TRB_ENABLE_SLOT:    u32 = 9 << 10;
const TRB_ADDRESS_DEVICE: u32 = 11 << 10;
const TRB_CONFIGURE_EP:   u32 = 12 << 10;
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
#[allow(dead_code)]
const EVT_PORT_STATUS:   u32 = 34 << 10;

// Completion codes
const CC_SUCCESS:        u32 = 1;
const CC_SHORT_PACKET:   u32 = 13;

// Endpoint types in endpoint context
const EP_TYPE_CONTROL:      u32 = 4;
const EP_TYPE_INTERRUPT_IN:  u32 = 7;

// USB request types
const USB_GET_DESCRIPTOR: u8 = 6;
const USB_SET_CONFIG:     u8 = 9;
const USB_SET_PROTOCOL:   u8 = 0x0B;
const USB_SET_IDLE:       u8 = 0x0A;

// Descriptor types
const DESC_DEVICE:        u16 = 0x0100;
const DESC_CONFIG:        u16 = 0x0200;

const NUM_CMD_TRBS: usize = 32;
const NUM_EVT_TRBS: usize = 32;
const NUM_TR_TRBS:  usize = 32;

// HID usage code to ASCII table (boot protocol, US layout base)
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

// Swiss German layout: remap HID usage codes
// Key differences: z↔y swap, number row shifted chars, special chars
static HID_TO_ASCII_DE: [u8; 57] = [
    0, 0, 0, 0,
    b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h',
    b'i', b'j', b'k', b'l', b'm', b'n', b'o', b'p',
    b'q', b'r', b's', b't', b'u', b'v', b'w', b'x',
    b'z', b'y',                                        // z/y swapped
    b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0',
    b'\n', 0x1B, 0x08, b'\t', b' ',
    b'\'', b'^', b'[', b']', b'$',
    0, b';', b'\'', b'<', b',', b'.', b'-',
];

static HID_TO_ASCII_DE_SHIFT: [u8; 57] = [
    0, 0, 0, 0,
    b'A', b'B', b'C', b'D', b'E', b'F', b'G', b'H',
    b'I', b'J', b'K', b'L', b'M', b'N', b'O', b'P',
    b'Q', b'R', b'S', b'T', b'U', b'V', b'W', b'X',
    b'Z', b'Y',
    b'+', b'"', b'*', 0,    b'%', b'&', b'/', b'(', b')', b'=',  // Shift+4=ç→0, Shift+7=/
    b'\n', 0x1B, 0x08, b'\t', b' ',
    b'?', b'`', b'{', b'}', b'!',
    0, b':', b'"', b'>', b';', b':', b'_',
];

// Key buffer (shared with keyboard.rs via poll_keyboard)
const KEY_BUF_SIZE: usize = 32;
static mut KEY_BUF: [u8; KEY_BUF_SIZE] = [0; KEY_BUF_SIZE];
static mut KEY_HEAD: usize = 0;
static mut KEY_TAIL: usize = 0;

fn push_key(k: u8) {
    // SAFETY: single-core, no concurrent access
    unsafe {
        let next = (KEY_HEAD + 1) % KEY_BUF_SIZE;
        if next != KEY_TAIL {
            KEY_BUF[KEY_HEAD] = k;
            KEY_HEAD = next;
        }
    }
}

/// Poll for a key from the USB keyboard. Called from keyboard.rs.
pub fn poll_keyboard() -> Option<u8> {
    if !AVAILABLE.load(Ordering::Relaxed) { return None; }
    poll_events();

    // Check key buffer first (newly pressed keys)
    // SAFETY: single-core
    unsafe {
        if KEY_HEAD != KEY_TAIL {
            let k = KEY_BUF[KEY_TAIL];
            KEY_TAIL = (KEY_TAIL + 1) % KEY_BUF_SIZE;
            return Some(k);
        }
    }

    // Timer-based key repeat for held keys
    let mut state_lock = STATE.lock();
    if let Some(ref mut state) = *state_lock {
        if state.repeat_key != 0 {
            let now = crate::interrupts::ticks();
            let held_ms = (now - state.repeat_start) * 10; // ticks are ~10ms each (100Hz)
            let since_last = (now - state.repeat_last) * 10;

            // Initial delay 500ms, then repeat every 50ms
            if held_ms >= 500 && since_last >= 50 {
                state.repeat_last = now;
                let is_de = match crate::config::get("keyboard") {
                    Some(ref s) if s == "us" => false,
                    _ => true,
                };
                let ch = hid_to_char(state.repeat_key, state.repeat_shift, state.repeat_altgr, is_de);
                if ch != 0 {
                    return Some(ch);
                }
            }
        }
    }

    None
}

static AVAILABLE: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn is_available() -> bool { AVAILABLE.load(Ordering::Relaxed) }

#[allow(dead_code)]
struct XhciState {
    mmio: u64,
    oper: u64,          // operational registers base
    rt: u64,            // runtime registers base
    db: u64,            // doorbell array base
    ctx_size: usize,    // 32 or 64
    max_ports: u32,
    // DMA regions (physical = virtual, identity-mapped)
    dcbaa: u64,
    cmd_ring: u64,
    cmd_cycle: u32,
    cmd_enqueue: usize,
    evt_ring: u64,
    evt_cycle: u32,
    evt_dequeue: usize,
    evt_seg_table: u64,
    input_ctx: u64,
    device_ctx: u64,
    ep0_ring: u64,
    ep0_cycle: u32,
    ep0_enqueue: usize,
    intr_ring: u64,
    intr_cycle: u32,
    intr_enqueue: usize,
    data_buf: u64,      // general-purpose DMA buffer (4KB)
    slot_id: u8,
    port_speed: u32,
    intr_ep_dci: u8,     // DCI of interrupt IN endpoint
    prev_keys: [u8; 6],  // previous HID report keys
    repeat_key: u8,      // key currently held for repeat
    repeat_shift: bool,  // shift state when repeat started
    repeat_altgr: bool,  // altgr state when repeat started
    repeat_start: u64,   // tick when key was first pressed
    repeat_last: u64,    // tick when last repeat was emitted
    port_num: u32,       // connected port number
    error_count: u32,    // consecutive transfer errors
}

static STATE: spin::Mutex<Option<XhciState>> = spin::Mutex::new(None);

/// Initialize xHCI controller and enumerate USB keyboard.
/// Tries all xHCI controllers until one with a connected device is found.
pub fn init() -> bool {
    // Find all xHCI controllers (class 0C:03:30) and try each
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
                    if init_controller(pci_dev) { return true; }
                }
                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }
    false
}

fn init_controller(dev: pci::PciDevice) -> bool {

    kprintln!("[npk] xhci: PCI {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        dev.addr.bus, dev.addr.device, dev.addr.function,
        dev.vendor_id, dev.device_id);

    pci::enable_bus_master(dev.addr);
    let cmd = pci::read16(dev.addr, 0x04);
    pci::write32(dev.addr, 0x04, (cmd | 0x06) as u32);

    // Bridge bus mastering
    if dev.addr.bus > 0 {
        for d in 0u8..32 {
            for f in 0u8..8 {
                let ba = pci::PciAddr { bus: 0, device: d, function: f };
                let bid = pci::read32(ba, 0x00);
                if bid == 0xFFFF_FFFF || bid == 0 { if f == 0 { break; } continue; }
                if pci::read8(ba, 0x0E) & 0x7F == 1 {
                    let sec = pci::read8(ba, 0x19);
                    let sub_bus = pci::read8(ba, 0x1A);
                    if dev.addr.bus >= sec && dev.addr.bus <= sub_bus {
                        pci::enable_bus_master(ba);
                    }
                }
                if f == 0 && pci::read8(ba, 0x0E) & 0x80 == 0 { break; }
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
    if bar0 == 0 { kprintln!("[npk] xhci: BAR0 is zero"); return false; }

    // Map BAR0 (64KB)
    let map_size = 64 * 1024u64;
    for off in (0..map_size).step_by(4096) {
        match paging::map_page(bar0 + off, bar0 + off,
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_CACHE) {
            Ok(()) | Err(paging::PagingError::AlreadyMapped) => {}
            Err(e) => { kprintln!("[npk] xhci: map failed: {:?}", e); return false; }
        }
    }

    let mmio = bar0;

    // Read capability registers
    let caplength = r8(mmio, CAP_CAPLENGTH) as u32;
    let hcsparams1 = r32(mmio, CAP_HCSPARAMS1);
    let hcsparams2 = r32(mmio, CAP_HCSPARAMS2);
    let hccparams1 = r32(mmio, CAP_HCCPARAMS1);
    let dboff = r32(mmio, CAP_DBOFF) & 0xFFFF_FFFC;
    let rtsoff = r32(mmio, CAP_RTSOFF) & 0xFFFF_FFE0;

    let max_slots = hcsparams1 & 0xFF;
    let max_ports = (hcsparams1 >> 24) & 0xFF;
    let ctx_size: usize = if hccparams1 & 0x04 != 0 { 64 } else { 32 };

    let oper = mmio + caplength as u64;
    let rt = mmio + rtsoff as u64;
    let db = mmio + dboff as u64;

    kprintln!("[npk] xhci: ports={} slots={} ctx={}B", max_ports, max_slots, ctx_size);

    // BIOS/OS handoff via extended capabilities
    let xecp_off = ((hccparams1 >> 16) & 0xFFFF) as u32 * 4;
    if xecp_off > 0 {
        bios_handoff(mmio, xecp_off);
    }

    // Halt controller
    let cmd_val = r32(oper, OP_USBCMD);
    w32(oper, OP_USBCMD, cmd_val & !CMD_RUN);
    if !wait_for(oper, OP_USBSTS, STS_HCH, STS_HCH) {
        kprintln!("[npk] xhci: halt timeout");
        return false;
    }

    // Reset controller
    w32(oper, OP_USBCMD, CMD_HCRST);
    if !wait_for(oper, OP_USBCMD, CMD_HCRST, 0) {
        kprintln!("[npk] xhci: reset timeout (CMD)");
        return false;
    }
    if !wait_for(oper, OP_USBSTS, STS_CNR, 0) {
        kprintln!("[npk] xhci: reset timeout (CNR)");
        return false;
    }

    // Allocate DMA structures (all page-aligned, zeroed)
    let dcbaa = alloc_dma(1, "DCBAA");
    let cmd_ring = alloc_dma(1, "cmd ring");
    let evt_ring = alloc_dma(1, "evt ring");
    let evt_seg_table = alloc_dma(1, "evt seg table");
    let input_ctx = alloc_dma(1, "input ctx");
    let device_ctx = alloc_dma(1, "device ctx");
    let ep0_ring = alloc_dma(1, "EP0 ring");
    let intr_ring = alloc_dma(1, "intr ring");
    let data_buf = alloc_dma(1, "data buf");

    if dcbaa == 0 || cmd_ring == 0 || evt_ring == 0 || evt_seg_table == 0
        || input_ctx == 0 || device_ctx == 0 || ep0_ring == 0 || intr_ring == 0
        || data_buf == 0 {
        kprintln!("[npk] xhci: DMA alloc failed");
        return false;
    }

    // Set up Link TRBs at end of rings (wrap back to start)
    write_trb(cmd_ring, NUM_CMD_TRBS - 1, cmd_ring, 0, TRB_LINK | TRB_CYCLE | (1 << 1)); // Toggle Cycle
    write_trb(ep0_ring, NUM_TR_TRBS - 1, ep0_ring, 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    write_trb(intr_ring, NUM_TR_TRBS - 1, intr_ring, 0, TRB_LINK | TRB_CYCLE | (1 << 1));

    // Set up Event Ring Segment Table (1 entry)
    // SAFETY: writing to DMA-allocated, zeroed memory
    unsafe {
        let seg = evt_seg_table as *mut u64;
        core::ptr::write_volatile(seg, evt_ring);           // ring base address
        core::ptr::write_volatile(seg.add(1),
            NUM_EVT_TRBS as u64);                            // ring size
    }

    // Scratchpad buffers
    let sp_hi = (hcsparams2 >> 21) & 0x1F;
    let sp_lo = (hcsparams2 >> 27) & 0x1F;
    let num_scratchpad = ((sp_hi << 5) | sp_lo) as usize;
    if num_scratchpad > 0 {
        let sp_array = alloc_dma(1, "scratchpad array");
        if sp_array == 0 { kprintln!("[npk] xhci: scratchpad alloc failed"); return false; }
        for i in 0..num_scratchpad {
            let page = alloc_dma(1, "scratchpad page");
            if page == 0 { kprintln!("[npk] xhci: scratchpad page alloc failed"); return false; }
            // SAFETY: writing to DMA array
            unsafe { core::ptr::write_volatile((sp_array as *mut u64).add(i), page); }
        }
        // DCBAA[0] = scratchpad array pointer
        // SAFETY: writing to DMA array
        unsafe { core::ptr::write_volatile(dcbaa as *mut u64, sp_array); }
        kprintln!("[npk] xhci: {} scratchpad buffers", num_scratchpad);
    }

    // Program controller
    w32(oper, OP_CONFIG, 1); // MaxSlotsEn = 1
    w64(oper, OP_DCBAAP, dcbaa);
    w64(oper, OP_CRCR, cmd_ring | 1); // cycle bit = 1

    // Program Event Ring (interrupter 0)
    let ir0 = rt + 0x20; // interrupter 0 offset
    w32(ir0, 0x08, 1);              // ERSTSZ = 1 segment
    w64(ir0, 0x18, evt_ring);       // ERDP
    w64(ir0, 0x10, evt_seg_table);  // ERSTBA (write AFTER ERSTSZ)
    // Enable interrupter (for event ring to work, even in polling mode)
    w32(ir0, 0x00, r32(ir0, 0x00) | 0x02); // IMAN.IE = 1

    // Start controller
    w32(oper, OP_USBCMD, CMD_RUN);
    if !wait_for(oper, OP_USBSTS, STS_HCH, 0) {
        kprintln!("[npk] xhci: start failed");
        return false;
    }
    kprintln!("[npk] xhci: controller running");

    let mut state = XhciState {
        mmio, oper, rt, db, ctx_size, max_ports,
        dcbaa, cmd_ring, cmd_cycle: 1, cmd_enqueue: 0,
        evt_ring, evt_cycle: 1, evt_dequeue: 0, evt_seg_table,
        input_ctx, device_ctx,
        ep0_ring, ep0_cycle: 1, ep0_enqueue: 0,
        intr_ring, intr_cycle: 1, intr_enqueue: 0,
        data_buf, slot_id: 0, port_speed: 0,
        intr_ep_dci: 0, prev_keys: [0; 6],
        repeat_key: 0, repeat_shift: false, repeat_altgr: false, repeat_start: 0, repeat_last: 0,
        port_num: 0, error_count: 0,
    };

    // Power on all ports
    for p in 0..max_ports {
        let off = portsc_off(p);
        let sc = r32(oper, off);
        if sc & PORTSC_PP == 0 {
            w32(oper, off, (sc & !PORTSC_RW1C) | PORTSC_PP);
        }
    }

    // Wait for device attachment + link training (USB spec: up to ~500ms)
    // Quick scan: check immediately, then wait up to 500ms
    let mut found_port = None;
    for _ in 0..5u32 {
        crate::interrupts::delay_ms(100);
        for p in 0..max_ports {
            if r32(oper, portsc_off(p)) & PORTSC_CCS != 0 {
                found_port = Some(p);
                break;
            }
        }
        if found_port.is_some() { break; }
    }

    let port = match found_port {
        Some(p) => p,
        None => return false, // silently skip — try next controller
    };
    state.port_num = port;
    kprintln!("[npk] xhci: device on port {}", port + 1);

    // Reset port
    if !reset_port(&state, port) {
        kprintln!("[npk] xhci: port reset failed");
        return false;
    }
    state.port_speed = (r32(state.oper, portsc_off(port)) >> 10) & 0xF;
    kprintln!("[npk] xhci: port speed = {}", state.port_speed);

    // Enable Slot
    let slot_id = match cmd_enable_slot(&mut state) {
        Some(s) => s,
        None => { kprintln!("[npk] xhci: enable slot failed"); return false; }
    };
    state.slot_id = slot_id;
    kprintln!("[npk] xhci: slot {}", slot_id);

    // Set DCBAA entry for this slot
    // SAFETY: writing to DMA array
    unsafe {
        core::ptr::write_volatile(
            (state.dcbaa as *mut u64).add(slot_id as usize),
            state.device_ctx
        );
    }

    // Address Device
    let max_packet = match state.port_speed {
        SPEED_LOW => 8u16,
        SPEED_FULL => 8,
        SPEED_HIGH => 64,
        SPEED_SUPER => 512,
        _ => 64,
    };
    if !cmd_address_device(&mut state, port, max_packet) {
        kprintln!("[npk] xhci: address device failed");
        return false;
    }
    kprintln!("[npk] xhci: device addressed");

    // Get Device Descriptor (8 bytes to get bMaxPacketSize0)
    if !usb_get_descriptor(&mut state, DESC_DEVICE, 8) {
        kprintln!("[npk] xhci: get device desc failed");
        return false;
    }
    let real_max_packet = r8(state.data_buf, 7) as u16;
    if real_max_packet > 0 && real_max_packet != max_packet {
        // Would need Evaluate Context to update — skip for now, usually matches
    }

    // Get full Device Descriptor (18 bytes)
    if !usb_get_descriptor(&mut state, DESC_DEVICE, 18) {
        kprintln!("[npk] xhci: get full device desc failed");
        return false;
    }
    let dev_class = r8(state.data_buf, 4);
    kprintln!("[npk] xhci: device class={:#04x}", dev_class);

    // Get Configuration Descriptor (9 bytes first to get total length)
    if !usb_get_descriptor(&mut state, DESC_CONFIG, 9) {
        kprintln!("[npk] xhci: get config desc failed");
        return false;
    }
    let total_len = u16::from_le_bytes([
        r8(state.data_buf, 2), r8(state.data_buf, 3)
    ]) as usize;
    let config_val = r8(state.data_buf, 5);

    // Get full Configuration Descriptor
    let fetch_len = total_len.min(512) as u16;
    if !usb_get_descriptor(&mut state, DESC_CONFIG, fetch_len) {
        kprintln!("[npk] xhci: get full config desc failed");
        return false;
    }

    // Parse for HID keyboard interface + interrupt IN endpoint
    let (kbd_iface, intr_ep, intr_max_pkt, intr_interval) =
        match find_keyboard_endpoint(&state, fetch_len as usize) {
            Some(v) => v,
            None => { kprintln!("[npk] xhci: no keyboard interface found"); return false; }
        };
    kprintln!("[npk] xhci: keyboard iface={} ep={:#04x} maxpkt={} interval={}",
        kbd_iface, intr_ep, intr_max_pkt, intr_interval);

    // Set Configuration
    if !usb_set_config(&mut state, config_val) {
        kprintln!("[npk] xhci: set config failed");
        return false;
    }

    // Set Protocol = Boot Protocol (0)
    if !usb_set_protocol(&mut state, kbd_iface, 0) {
        kprintln!("[npk] xhci: set protocol failed");
        // Non-fatal, some keyboards default to boot protocol
    }

    // Set Idle (rate=0)
    let _ = usb_set_idle(&mut state, kbd_iface);

    // Configure Endpoint (interrupt IN)
    let ep_dci = (intr_ep & 0x0F) * 2 + 1; // DCI for IN endpoint
    state.intr_ep_dci = ep_dci;
    kprintln!("[npk] xhci: configuring endpoint DCI={}", ep_dci);
    if !cmd_configure_endpoint(&mut state, ep_dci, intr_max_pkt, intr_interval) {
        kprintln!("[npk] xhci: configure endpoint failed");
        return false;
    }
    kprintln!("[npk] xhci: endpoint configured, scheduling transfer");

    // Schedule first interrupt transfer
    schedule_interrupt_transfer(&mut state);
    kprintln!("[npk] xhci: USB keyboard online");
    AVAILABLE.store(true, Ordering::Relaxed);
    *STATE.lock() = Some(state);
    true
}

// === Helper functions ===

fn alloc_dma(pages: usize, _name: &str) -> u64 {
    match memory::allocate_contiguous(pages) {
        Some(a) => {
            // SAFETY: zeroing allocated DMA memory
            unsafe { core::ptr::write_bytes(a as *mut u8, 0, pages * 4096); }
            a
        }
        None => 0,
    }
}

fn wait_for(base: u64, reg: u32, mask: u32, expected: u32) -> bool {
    // Tick-based timeout (500ms) — CPU-speed independent
    let deadline = crate::interrupts::ticks() + 50; // 50 ticks = 500ms at 100Hz
    loop {
        if r32(base, reg) & mask == expected { return true; }
        if crate::interrupts::ticks() >= deadline { return false; }
        core::hint::spin_loop();
    }
}

fn portsc_off(port: u32) -> u32 {
    0x400 + port * 0x10
}

fn write_trb(ring: u64, idx: usize, param: u64, status: u32, control: u32) {
    let addr = ring + (idx * 16) as u64;
    // SAFETY: writing to DMA-allocated memory
    unsafe {
        core::ptr::write_volatile(addr as *mut u32, param as u32);
        core::ptr::write_volatile((addr + 4) as *mut u32, (param >> 32) as u32);
        core::ptr::write_volatile((addr + 8) as *mut u32, status);
        fence(Ordering::SeqCst);
        core::ptr::write_volatile((addr + 12) as *mut u32, control);
    }
}

fn read_trb(ring: u64, idx: usize) -> (u64, u32, u32) {
    let addr = ring + (idx * 16) as u64;
    // SAFETY: reading from DMA memory
    unsafe {
        let lo = core::ptr::read_volatile(addr as *const u32) as u64;
        let hi = core::ptr::read_volatile((addr + 4) as *const u32) as u64;
        let status = core::ptr::read_volatile((addr + 8) as *const u32);
        let control = core::ptr::read_volatile((addr + 12) as *const u32);
        (lo | (hi << 32), status, control)
    }
}

fn ring_doorbell(state: &XhciState, slot: u32, target: u32) {
    fence(Ordering::SeqCst);
    w32(state.db, slot * 4, target);
}

fn bios_handoff(mmio: u64, mut off: u32) {
    // Walk extended capability list to find USB Legacy Support (ID=1)
    for _ in 0..100 {
        let cap = r32(mmio, off);
        let id = cap & 0xFF;
        if id == 1 {
            // Found USB Legacy Support capability
            // Set OS Owned Semaphore (bit 24)
            w32(mmio, off, cap | (1 << 24));
            // Wait for BIOS Owned Semaphore (bit 16) to clear (1s timeout)
            let deadline = crate::interrupts::ticks() + 100;
            while r32(mmio, off) & (1 << 16) != 0 {
                if crate::interrupts::ticks() >= deadline { break; }
                core::hint::spin_loop();
            }
            // Disable SMI (clear USBLEGCTLSTS enable bits)
            let ctl_off = off + 4;
            w32(mmio, ctl_off, r32(mmio, ctl_off) & 0x0000_001F); // keep RO/RW1C, clear enables
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
        let sc = r32(state.oper, portsc_off(p));
        if sc & PORTSC_CCS != 0 {
            return Some(p);
        }
    }
    None
}

fn reset_port(state: &XhciState, port: u32) -> bool {
    let off = portsc_off(port);
    // Read PORTSC, preserve non-RW1C bits, set Port Reset
    let sc = r32(state.oper, off);
    w32(state.oper, off, (sc & !PORTSC_RW1C) | PORTSC_PR);

    // Wait for reset complete (500ms timeout)
    let deadline = crate::interrupts::ticks() + 50;
    loop {
        let sc = r32(state.oper, off);
        if sc & PORTSC_PED != 0 { return true; }  // Port Enabled = reset done
        if sc & PORTSC_PRC != 0 {
            w32(state.oper, off, (sc & !PORTSC_RW1C) | PORTSC_PRC);
            if sc & PORTSC_PED != 0 { return true; }
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
        write_trb(state.cmd_ring, NUM_CMD_TRBS - 1, state.cmd_ring, 0, link_ctrl);
        state.cmd_cycle ^= 1;
        state.cmd_enqueue = 0;
    }
    ring_doorbell(state, 0, 0); // HC doorbell
}

fn wait_command_completion(state: &mut XhciState) -> Option<(u32, u32)> {
    // Poll event ring for command completion (1s timeout)
    let deadline = crate::interrupts::ticks() + 100;
    loop {
        if crate::interrupts::ticks() >= deadline { return None; }
        let (_param, status, control) = read_trb(state.evt_ring, state.evt_dequeue);
        let cycle = control & TRB_CYCLE;
        if cycle != state.evt_cycle { core::hint::spin_loop(); continue; }

        let trb_type = control & (0x3F << 10);
        let cc = (status >> 24) & 0xFF;

        // Advance dequeue
        state.evt_dequeue += 1;
        if state.evt_dequeue >= NUM_EVT_TRBS {
            state.evt_dequeue = 0;
            state.evt_cycle ^= 1;
        }
        // Update ERDP
        let erdp = state.evt_ring + (state.evt_dequeue * 16) as u64;
        let ir0 = state.rt + 0x20;
        w64(ir0, 0x18, erdp | (1 << 3)); // EHB bit

        if trb_type == EVT_CMD_COMPLETE {
            let slot = (control >> 24) & 0xFF;
            return Some((cc, slot));
        }
        // Consume other events (port status change etc.)
    }
}

fn cmd_enable_slot(state: &mut XhciState) -> Option<u8> {
    post_command(state, 0, 0, TRB_ENABLE_SLOT);
    let (cc, slot) = wait_command_completion(state)?;
    if cc != CC_SUCCESS { return None; }
    Some(slot as u8)
}

fn cmd_address_device(state: &mut XhciState, port: u32, max_packet: u16) -> bool {
    let ctx = state.ctx_size;
    let input = state.input_ctx;

    // SAFETY: writing to DMA-allocated input context
    unsafe { core::ptr::write_bytes(input as *mut u8, 0, 4096); }

    // Input Control Context: Add Slot (bit 0) + EP0 (bit 1)
    // SAFETY: writing to DMA memory
    unsafe {
        core::ptr::write_volatile((input + 4) as *mut u32, 0x03); // Add flags at offset 4
    }

    // Slot Context (at input + ctx_size * 1)
    let slot_off = input + ctx as u64;
    let route_speed_entries = (1 << 27) | // Context Entries = 1
        ((state.port_speed as u32) << 20); // Speed
    // SAFETY: writing slot context
    unsafe {
        core::ptr::write_volatile(slot_off as *mut u32, route_speed_entries);
        // Dword 1: Root Hub Port Number (1-based)
        core::ptr::write_volatile((slot_off + 4) as *mut u32, ((port + 1) as u32) << 16);
    }

    // EP0 Context (at input + ctx_size * 2)
    let ep0_off = input + (ctx * 2) as u64;
    let ep_type_mps = (EP_TYPE_CONTROL << 3) | (3 << 1); // CErr=3, EP Type=Control
    let mps_field = (max_packet as u32) << 16;
    // SAFETY: writing EP0 context
    unsafe {
        // Dword 1: CErr + EP Type
        core::ptr::write_volatile((ep0_off + 4) as *mut u32, ep_type_mps | mps_field);
        // Dword 2-3: TR Dequeue Pointer (with DCS=1)
        core::ptr::write_volatile((ep0_off + 8) as *mut u32, (state.ep0_ring as u32) | 1);
        core::ptr::write_volatile((ep0_off + 12) as *mut u32, (state.ep0_ring >> 32) as u32);
        // Dword 4: Average TRB Length
        core::ptr::write_volatile((ep0_off + 16) as *mut u32, 8);
    }

    // Post Address Device Command
    let slot_field = (state.slot_id as u32) << 24;
    post_command(state, input, 0, TRB_ADDRESS_DEVICE | slot_field);
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

// === USB Control Transfers ===

fn usb_control_transfer(state: &mut XhciState, bm_request: u8, b_request: u8,
    w_value: u16, w_index: u16, w_length: u16, dir_in: bool) -> bool
{
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
        write_trb(state.ep0_ring, *ep0, state.data_buf, w_length as u32, TRB_DATA_STAGE | dir_bit | cycle);
        *ep0 += 1;
    }

    // Status Stage TRB
    let status_dir = if w_length > 0 && dir_in { 0 } else { TRB_DIR_IN };
    write_trb(state.ep0_ring, *ep0, 0, 0, TRB_STATUS_STAGE | TRB_IOC | status_dir | cycle);
    *ep0 += 1;

    // Wrap check
    if *ep0 >= NUM_TR_TRBS - 1 {
        let link_ctrl = TRB_LINK | cycle | (1 << 1);
        write_trb(state.ep0_ring, NUM_TR_TRBS - 1, state.ep0_ring, 0, link_ctrl);
        state.ep0_cycle ^= 1;
        *ep0 = 0;
    }

    // Ring doorbell for slot, target EP0 (DCI=1)
    ring_doorbell(state, state.slot_id as u32, 1);

    // Wait for transfer completion (1s timeout)
    let deadline = crate::interrupts::ticks() + 100;
    loop {
        if crate::interrupts::ticks() >= deadline { return false; }
        let (_param, status, control) = read_trb(state.evt_ring, state.evt_dequeue);
        if control & TRB_CYCLE != state.evt_cycle { core::hint::spin_loop(); continue; }

        state.evt_dequeue += 1;
        if state.evt_dequeue >= NUM_EVT_TRBS {
            state.evt_dequeue = 0;
            state.evt_cycle ^= 1;
        }
        let erdp = state.evt_ring + (state.evt_dequeue * 16) as u64;
        let ir0 = state.rt + 0x20;
        w64(ir0, 0x18, erdp | (1 << 3));

        let trb_type = control & (0x3F << 10);
        let cc = (status >> 24) & 0xFF;

        if trb_type == EVT_TRANSFER {
            return cc == CC_SUCCESS || cc == CC_SHORT_PACKET;
        }
        if trb_type == EVT_CMD_COMPLETE {
            // Unexpected command completion — consume and continue
        }
    }
}

fn usb_get_descriptor(state: &mut XhciState, desc_type: u16, length: u16) -> bool {
    // SAFETY: zeroing DMA buffer
    unsafe { core::ptr::write_bytes(state.data_buf as *mut u8, 0, length as usize); }
    usb_control_transfer(state, 0x80, USB_GET_DESCRIPTOR, desc_type, 0, length, true)
}

fn usb_set_config(state: &mut XhciState, config_val: u8) -> bool {
    usb_control_transfer(state, 0x00, USB_SET_CONFIG, config_val as u16, 0, 0, false)
}

fn usb_set_protocol(state: &mut XhciState, iface: u8, protocol: u8) -> bool {
    usb_control_transfer(state, 0x21, USB_SET_PROTOCOL, protocol as u16, iface as u16, 0, false)
}

fn usb_set_idle(state: &mut XhciState, iface: u8) -> bool {
    usb_control_transfer(state, 0x21, USB_SET_IDLE, 0, iface as u16, 0, false)
}

// === Descriptor parsing ===

fn find_keyboard_endpoint(state: &XhciState, total_len: usize) -> Option<(u8, u8, u16, u8)> {
    let buf = state.data_buf;
    let mut pos = 0usize;
    let mut in_kbd_iface = false;
    let mut kbd_iface = 0u8;

    while pos + 1 < total_len {
        let len = r8(buf, pos as u32) as usize;
        let dtype = r8(buf, (pos + 1) as u32);
        if len < 2 { break; }

        // Interface descriptor (type 4)
        if dtype == 4 && len >= 9 {
            let iface_class = r8(buf, (pos + 5) as u32);
            let iface_subclass = r8(buf, (pos + 6) as u32);
            let iface_protocol = r8(buf, (pos + 7) as u32);
            // HID class=3, boot subclass=1, keyboard protocol=1
            in_kbd_iface = iface_class == 3 && iface_subclass == 1 && iface_protocol == 1;
            if in_kbd_iface {
                kbd_iface = r8(buf, (pos + 2) as u32);
            }
        }

        // Endpoint descriptor (type 5)
        if dtype == 5 && len >= 7 && in_kbd_iface {
            let ep_addr = r8(buf, (pos + 2) as u32);
            let ep_attr = r8(buf, (pos + 3) as u32);
            let max_pkt = u16::from_le_bytes([
                r8(buf, (pos + 4) as u32), r8(buf, (pos + 5) as u32)
            ]);
            let interval = r8(buf, (pos + 6) as u32);
            // Interrupt IN endpoint: attr bits [1:0] = 3 (interrupt), addr bit 7 = IN
            if (ep_attr & 0x03) == 3 && (ep_addr & 0x80) != 0 {
                return Some((kbd_iface, ep_addr, max_pkt, interval));
            }
        }

        pos += len;
    }
    None
}

// === Configure Interrupt Endpoint ===

fn cmd_configure_endpoint(state: &mut XhciState, ep_dci: u8, max_pkt: u16, interval: u8) -> bool {
    let ctx = state.ctx_size;
    let input = state.input_ctx;

    // SAFETY: zeroing input context
    unsafe { core::ptr::write_bytes(input as *mut u8, 0, 4096); }

    // Input Control Context: Add Slot (bit 0) + the endpoint (bit ep_dci)
    // SAFETY: writing to DMA memory
    unsafe {
        core::ptr::write_volatile((input + 4) as *mut u32, 1 | (1u32 << ep_dci));
    }

    // Slot Context: Context Entries = last valid endpoint index = ep_dci
    let slot_off = input + ctx as u64;
    let slot_dw0 = ((ep_dci as u32) << 27) | ((state.port_speed as u32) << 20);
    // SAFETY: writing slot context
    unsafe {
        core::ptr::write_volatile(slot_off as *mut u32, slot_dw0);
    }

    // Endpoint Context (at input + ctx_size * (ep_dci + 1))
    let ep_off = input + (ctx * (ep_dci as usize + 1)) as u64;

    // Compute interval for xHCI (different from USB bInterval)
    let xhci_interval = match state.port_speed {
        SPEED_HIGH | SPEED_SUPER => {
            if interval > 0 { interval - 1 } else { 0 }
        }
        _ => {
            // FS/LS: convert ms to 125us frames
            let mut val = 0u8;
            let mut ms = interval as u32;
            while ms > 1 { ms >>= 1; val += 1; }
            val + 3
        }
    };

    // Dword 0: Interval + mult=0 + LSA=0
    // Dword 1: CErr=3, EP Type=Interrupt IN (7), MaxPacketSize
    // SAFETY: writing endpoint context
    unsafe {
        core::ptr::write_volatile(ep_off as *mut u32, (xhci_interval as u32) << 16);
        core::ptr::write_volatile((ep_off + 4) as *mut u32,
            (3 << 1) | (EP_TYPE_INTERRUPT_IN << 3) | ((max_pkt as u32) << 16));
        // TR Dequeue Pointer with DCS=1
        core::ptr::write_volatile((ep_off + 8) as *mut u32, (state.intr_ring as u32) | 1);
        core::ptr::write_volatile((ep_off + 12) as *mut u32, (state.intr_ring >> 32) as u32);
        // Average TRB Length
        core::ptr::write_volatile((ep_off + 16) as *mut u32, 8);
    }

    let slot_field = (state.slot_id as u32) << 24;
    post_command(state, input, 0, TRB_CONFIGURE_EP | slot_field);
    match wait_command_completion(state) {
        Some((cc, _)) => cc == CC_SUCCESS,
        None => false,
    }
}

// === Interrupt Transfer (Keyboard Polling) ===

fn schedule_interrupt_transfer(state: &mut XhciState) {
    let idx = state.intr_enqueue;
    let cycle = state.intr_cycle;
    // Normal TRB: 8 bytes from data_buf+2048 (separate from control xfer buf)
    let buf = state.data_buf + 2048;
    write_trb(state.intr_ring, idx, buf, 8, TRB_NORMAL | TRB_IOC | cycle);
    state.intr_enqueue += 1;
    if state.intr_enqueue >= NUM_TR_TRBS - 1 {
        let link = TRB_LINK | cycle | (1 << 1);
        write_trb(state.intr_ring, NUM_TR_TRBS - 1, state.intr_ring, 0, link);
        state.intr_cycle ^= 1;
        state.intr_enqueue = 0;
    }
    ring_doorbell(state, state.slot_id as u32, state.intr_ep_dci as u32);
}

fn poll_events() {
    let mut lock = STATE.lock();
    let state = match lock.as_mut() {
        Some(s) => s,
        None => return,
    };

    // Check event ring for completions
    for _ in 0..8 {
        let (_param, status, control) = read_trb(state.evt_ring, state.evt_dequeue);
        if control & TRB_CYCLE != state.evt_cycle { break; }

        let trb_type = control & (0x3F << 10);
        let cc = (status >> 24) & 0xFF;

        state.evt_dequeue += 1;
        if state.evt_dequeue >= NUM_EVT_TRBS {
            state.evt_dequeue = 0;
            state.evt_cycle ^= 1;
        }
        let erdp = state.evt_ring + (state.evt_dequeue * 16) as u64;
        let ir0 = state.rt + 0x20;
        w64(ir0, 0x18, erdp | (1 << 3));

        if trb_type == EVT_TRANSFER {
            if cc != CC_SUCCESS && cc != CC_SHORT_PACKET {
                state.error_count += 1;
                if state.error_count >= 3 {
                    // Check if device is still connected
                    let portsc = r32(state.oper, portsc_off(state.port_num));
                    if portsc & PORTSC_CCS == 0 {
                        crate::kprintln!("[npk] xhci: device disconnected");
                        AVAILABLE.store(false, Ordering::Relaxed);
                        return;
                    }
                    state.error_count = 0; // Port still connected, reset counter
                }
                schedule_interrupt_transfer(state);
                continue;
            }
            state.error_count = 0;

            // Read HID boot protocol report from buffer
            let buf = state.data_buf + 2048;
            let modifiers = r8(buf, 0);
            let mut keys = [0u8; 6];
            for i in 0..6 {
                keys[i] = r8(buf, (2 + i) as u32);
            }
            process_hid_report(modifiers, &keys, state);
            state.prev_keys = keys;

            // Re-schedule next transfer
            schedule_interrupt_transfer(state);
        }
    }
}

fn process_hid_report(modifiers: u8, keys: &[u8; 6], state: &mut XhciState) {
    let shift = (modifiers & 0x22) != 0;  // L/R Shift
    let _ctrl = (modifiers & 0x11) != 0;  // L/R Ctrl
    let alt_gr = (modifiers & 0x40) != 0; // Right Alt (AltGr)
    let super_held = (modifiers & 0x88) != 0; // L/R GUI (Super)

    // Update shared modifier state (used by shade compositor)
    crate::keyboard::set_super(super_held);
    crate::keyboard::set_shift(shift);

    // Determine layout
    let is_de = match crate::config::get("keyboard") {
        Some(ref s) if s == "us" => false,
        _ => true, // default de_CH
    };

    // Find the first non-zero key in the current report for repeat tracking
    let first_key = keys.iter().find(|&&k| k != 0 && k != 1).copied().unwrap_or(0);

    if first_key == 0 {
        // All keys released — stop repeat
        state.repeat_key = 0;
    } else if !state.prev_keys.contains(&first_key) {
        // New key pressed — start repeat timer
        state.repeat_key = first_key;
        state.repeat_shift = shift;
        state.repeat_altgr = alt_gr;
        state.repeat_start = crate::interrupts::ticks();
        state.repeat_last = state.repeat_start;
    }
    // If same key still held, repeat_key stays set and poll_keyboard() handles timing

    for &key in keys.iter() {
        if key == 0 || key == 1 { continue; } // no key / error rollover
        // Only process newly pressed keys
        if state.prev_keys.contains(&key) { continue; }

        // Arrow keys and special multi-byte sequences
        match key {
            0x4F => { push_key(0x1B); push_key(b'['); push_key(b'C'); continue; } // Right
            0x50 => { push_key(0x1B); push_key(b'['); push_key(b'D'); continue; } // Left
            0x51 => { push_key(0x1B); push_key(b'['); push_key(b'B'); continue; } // Down
            0x52 => { push_key(0x1B); push_key(b'['); push_key(b'A'); continue; } // Up
            0x4A => { push_key(0x1B); push_key(b'['); push_key(b'H'); continue; } // Home
            0x4D => { push_key(0x1B); push_key(b'['); push_key(b'F'); continue; } // End
            0x4B => { push_key(0x1B); push_key(b'['); push_key(b'5'); continue; } // PgUp
            0x4E => { push_key(0x1B); push_key(b'['); push_key(b'6'); continue; } // PgDn
            _ => {}
        }

        let ch = hid_to_char(key, shift, alt_gr, is_de);
        if ch != 0 {
            push_key(ch);
        }
    }
}

/// Convert HID keycode to ASCII character. Returns 0 for unhandled keys.
fn hid_to_char(key: u8, shift: bool, alt_gr: bool, is_de: bool) -> u8 {
    // AltGr: special characters (de_CH)
    if alt_gr && is_de {
        if let Some(ch) = altgr_char_de_hid(key) {
            return ch;
        }
    }

    if (key as usize) < HID_TO_ASCII.len() {
        if is_de {
            if shift { HID_TO_ASCII_DE_SHIFT[key as usize] }
            else { HID_TO_ASCII_DE[key as usize] }
        } else {
            if shift { HID_TO_ASCII_SHIFT[key as usize] }
            else { HID_TO_ASCII[key as usize] }
        }
    } else {
        match key {
            0x54 => b'/',
            0x55 => b'*',
            0x56 => b'-',
            0x57 => b'+',
            0x58 => b'\n', // Numpad Enter
            0x59 => b'1',
            0x5A => b'2',
            0x5B => b'3',
            0x5C => b'4',
            0x5D => b'5',
            0x5E => b'6',
            0x5F => b'7',
            0x60 => b'8',
            0x61 => b'9',
            0x62 => b'0',
            0x63 => b'.',
            0x4C => 0x7F, // Delete
            // Arrow keys are multi-byte — handled separately, not via repeat
            _ => 0,
        }
    }
}

/// AltGr characters for Swiss German (de_CH) keyboard layout.
/// HID usage codes → ASCII.
fn altgr_char_de_hid(key: u8) -> Option<u8> {
    match key {
        0x1F => Some(b'@'),   // AltGr+2
        0x20 => Some(b'#'),   // AltGr+3
        0x24 => Some(b'|'),   // AltGr+7
        0x2E => Some(b'~'),   // AltGr+^ (= key)
        0x2F => Some(b'['),   // AltGr+ü ([ key)
        0x30 => Some(b']'),   // AltGr+¨ (] key)
        0x34 => Some(b'{'),   // AltGr+ä (' key)
        0x31 => Some(b'}'),   // AltGr+$ (\ key)
        0x64 => Some(b'\\'),  // AltGr+< (non-US \)
        _ => None,
    }
}
