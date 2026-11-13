//! The VideoCore firmware's property interface on the Raspberry Pi 5:
//! Linux `bcm2835-mailbox.c` behind `raspberrypi.c`, the register and
//! buffer side in `pi5_core::rpi_fw`.
//!
//! Kernel-owned. A module never sends a tag of its own: the interface also
//! allocates and locks VideoCore memory and runs code on the VPU, which
//! sees all RAM. Callers name one operation of this file.

use core::sync::atomic::{AtomicBool, Ordering};
use pi5_core::hw::{Block, Hw};
use pi5_core::rpi_fw as fw;
use spin::Mutex;

use super::dt::Fdt;
use crate::hw::{DmaRegion, Mmio};
use crate::kprintln;

/// `rpi_firmware_transaction`'s `wait_for_completion_timeout(..., 3*HZ)`.
const TIMEOUT_MS: u64 = 3000;
/// The polling step when the caller is not a fiber (boot, before the
/// scheduler), as the mailbox framework's `txpoll_period`.
const POLL_US: u64 = 5000;
/// One page holds every request of this file (`PAGE_ALIGN(size)`).
const BUF_WORDS: usize = 1024;

struct Mbox(Mmio);

impl Hw for Mbox {
    fn read(&mut self, _: Block, off: u64, _: u8) -> u32 { self.0.r32(off) }
    fn write(&mut self, _: Block, off: u64, _: u8, v: u32) { self.0.w32(off, v) }
    fn delay_us(&mut self, us: u64) { crate::time::delay_us(us) }
}

struct Firmware {
    mbox: Mbox,
    /// The property buffer: uncached, below 4 GiB, its bus address equal
    /// to the physical one (no `dma-ranges` on `/soc`).
    buf: DmaRegion,
    irq: crate::irq::Irq,
}

static FW: Mutex<Option<Firmware>> = Mutex::new(None);
/// `transaction_lock`: one request at a time, held across the wait.
static BUSY: AtomicBool = AtomicBool::new(false);

/// `bcm2835_mbox_probe` + `rpi_firmware_probe`: the mailbox, its
/// interrupt, the channel started; then the firmware's revision and hash
/// as Linux prints them. False without a mailbox in the device tree.
pub fn init(fdt: &Fdt) -> bool {
    let Some(node) = fdt.find_compatible(b"brcm,bcm2835-mbox") else { return false };
    if !fdt.is_enabled(&node) { return false; }
    let Some((base, len)) = fdt.reg0(&node) else { return false };
    // GIC SPI cells: type 0, line, flags; level-high on the Pi.
    let (Some(0), Some(line)) = (fdt.prop_cell(&node, b"interrupts", 0), fdt.prop_cell(&node, b"interrupts", 1)) else {
        kprintln!("[npk] firmware: mailbox without an SPI");
        return false;
    };
    // SAFETY: the register window of the mailbox node.
    let Ok(m) = (unsafe { Mmio::map(base, len) }) else { return false };
    let Some(irq) = crate::irq::register_spi(line + 32, false) else {
        kprintln!("[npk] firmware: mailbox interrupt INTID {} refused", line + 32);
        return false;
    };
    let Some(buf) = DmaRegion::alloc_zeroed_aligned_below(1, 1, 1 << 32) else {
        kprintln!("[npk] firmware: no DMA memory below 4 GiB");
        crate::irq::release(irq);
        return false;
    };
    let mut mbox = Mbox(m);
    fw::startup(&mut mbox);
    *FW.lock() = Some(Firmware { mbox, buf, irq });
    print_revision();
    true
}

/// `rpi_firmware_print_firmware_revision` and `_hash`.
fn print_revision() {
    let Some([date]) = property::<1>(fw::RPI_FIRMWARE_GET_FIRMWARE_REVISION, [0]) else {
        kprintln!("[npk] firmware: no answer to GET_FIRMWARE_REVISION");
        return;
    };
    const VARIANTS: [&str; 5] = ["unknown", "start", "start_x", "start_db", "start_cd"];
    let variant = match property::<1>(fw::RPI_FIRMWARE_GET_FIRMWARE_VARIANT, [0]) {
        Some([v]) => VARIANTS.get(v as usize).copied().unwrap_or(VARIANTS[0]),
        None => "cmd unsupported",
    };
    kprintln!("[npk] firmware: from {}, variant {}", crate::time::wall::format(date as u64), variant);
    if let Some(hash) = property::<5>(fw::RPI_FIRMWARE_GET_FIRMWARE_HASH, [0; 5]) {
        kprintln!("[npk] firmware: hash {:08x}{:08x}{:08x}{:08x}{:08x}", hash[0], hash[1], hash[2], hash[3], hash[4]);
    }
}

/// `rpi_firmware_property` for a tag whose value buffer is `N` words:
/// `value` in, the firmware's answer out. None on a firmware error, a
/// timeout or without a mailbox.
pub fn property<const N: usize>(tag: u32, value: [u32; N]) -> Option<[u32; N]> {
    while BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        if crate::smp::fiber::current_stack_lo().is_none() || !crate::smp::fiber::yield_sleep(1) {
            core::hint::spin_loop();
        }
    }
    let r = property_locked(tag, value);
    BUSY.store(false, Ordering::Release);
    r
}

fn property_locked<const N: usize>(tag: u32, value: [u32; N]) -> Option<[u32; N]> {
    let mut words = [0u32; BUF_WORDS];
    let n = fw::build_property(&mut words, tag, &value)?;
    let (irq, bus) = {
        let g = FW.lock();
        let f = g.as_ref()?;
        for (i, w) in words[..n].iter().enumerate() {
            f.buf.w32(i * 4, *w);
        }
        (f.irq, f.buf.bus() as u32)
    };
    // The buffer is uncached; the write below orders it (writel's wmb).
    if !transaction(irq, fw::MBOX_CHAN_PROPERTY, bus) {
        kprintln!("[npk] firmware: transaction {:#010x} timeout", tag);
        return None;
    }
    let g = FW.lock();
    let f = g.as_ref()?;
    for (i, w) in words[..n].iter_mut().enumerate() {
        *w = f.buf.r32(i * 4);
    }
    match fw::property_answer(&words[..n], N) {
        Some(a) => {
            let mut out = [0u32; N];
            out.copy_from_slice(a);
            Some(out)
        }
        None => {
            kprintln!("[npk] firmware: request {:#010x} returned status {:#010x}", tag, words[1]);
            None
        }
    }
}

/// `rpi_firmware_transaction`: the message to the firmware, then its
/// answer within `TIMEOUT_MS`. A fiber parks on the interrupt; a caller
/// outside one (boot) polls mailbox 0.
fn transaction(irq: crate::irq::Irq, chan: u32, data: u32) -> bool {
    let msg = fw::mbox_msg(chan, data);
    let deadline = crate::time::uptime_us() + TIMEOUT_MS * 1000;
    let in_fiber = crate::smp::fiber::current_stack_lo().is_some();
    // The mailbox framework sends once the previous message left
    // (`last_tx_done`).
    loop {
        let done = FW.lock().as_mut().is_some_and(|f| fw::last_tx_done(&mut f.mbox));
        if done { break; }
        if crate::time::uptime_us() > deadline { return false; }
        crate::time::delay_us(POLL_US.min(100));
    }
    let mut since = crate::irq::arm(irq);
    if let Some(f) = FW.lock().as_mut() {
        fw::send_data(&mut f.mbox, msg);
    }
    loop {
        // `bcm2835_mbox_irq`: every answer waiting completes the client
        // (`response_callback` ignores the word).
        let mut got = false;
        if let Some(f) = FW.lock().as_mut() {
            fw::drain(&mut f.mbox, |_| got = true);
        }
        if got { return true; }
        let now = crate::time::uptime_us();
        if now > deadline { return false; }
        if in_fiber {
            let left_ms = (deadline - now).div_ceil(1000);
            crate::irq::wait(irq, since, left_ms);
            since = crate::irq::arm(irq);
        } else {
            crate::time::delay_us(POLL_US.min(deadline - now));
        }
    }
}

/// `rpi_firmware_clk_rate_request`'s ops for clock `id`.
pub fn clock_rate(id: u32) -> Option<u32> {
    property::<2>(fw::RPI_FIRMWARE_GET_CLOCK_RATE, [id, 0]).map(|a| a[1])
}

pub fn clock_max_rate(id: u32) -> Option<u32> {
    property::<2>(fw::RPI_FIRMWARE_GET_MAX_CLOCK_RATE, [id, 0]).map(|a| a[1])
}

pub fn clock_min_rate(id: u32) -> Option<u32> {
    property::<2>(fw::RPI_FIRMWARE_GET_MIN_CLOCK_RATE, [id, 0]).map(|a| a[1])
}

pub fn clock_measured(id: u32) -> Option<u32> {
    property::<2>(fw::RPI_FIRMWARE_GET_CLOCK_MEASURED, [id, 0]).map(|a| a[1])
}

/// `raspberrypi_fw_is_prepared`'s state word: bit 0 on, bit 1 absent.
pub fn clock_state(id: u32) -> Option<u32> {
    property::<2>(fw::RPI_FIRMWARE_GET_CLOCK_STATE, [id, 0]).map(|a| a[1])
}

/// `rpi_firmware_notify_reboot` without reboot flags: the firmware is told
/// before a restart or power-off, as Linux's reboot notifier and
/// `rpi_firmware_shutdown` do.
pub fn notify_reboot() {
    if FW.lock().is_some() {
        let _ = property::<0>(fw::RPI_FIRMWARE_NOTIFY_REBOOT, []);
    }
}
