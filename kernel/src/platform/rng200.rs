//! The BCM2711/BCM2712 hardware random number generator, after Linux
//! `drivers/char/hw_random/iproc-rng200.c` (`bcm2711_rng200_init`,
//! `bcm2711_rng200_read`): a FIFO of 32-bit words behind a warm-up count.

use super::dt::Fdt;
use crate::hw::Mmio;

const RNG_CTRL_OFFSET: u32 = 0x00;
const RNG_CTRL_RNG_RBGEN_MASK: u32 = 0x0000_1FFF;
const RNG_CTRL_RNG_DIV_CTRL_SHIFT: u32 = 13;
const RNG_TOTAL_BIT_COUNT_OFFSET: u32 = 0x0C;
const RNG_TOTAL_BIT_COUNT_THRESHOLD_OFFSET: u32 = 0x10;
const RNG_FIFO_DATA_OFFSET: u32 = 0x20;
const RNG_FIFO_COUNT_OFFSET: u32 = 0x24;
const RNG_FIFO_COUNT_RNG_FIFO_COUNT_MASK: u32 = 0x0000_00FF;
const RNG_FIFO_COUNT_RNG_FIFO_THRESHOLD_SHIFT: u32 = 8;

/// How long a read waits for the warm-up or a word before it gives up.
/// Linux spins without limit; a dead generator must not stop the boot, it
/// leaves the CSPRNG on timing jitter with a warning.
const WAIT_US: u64 = 100_000;

static RNG: spin::Mutex<Option<Mmio>> = spin::Mutex::new(None);

/// Find the generator and start it (`bcm2711_rng200_init`): unless already
/// running, discard the first 0x40000 bits, which are less random, and
/// sample at 1 MHz. False without one.
pub fn init(fdt: &Fdt) -> bool {
    let Some(n) = fdt.find_compatible(b"brcm,bcm2711-rng200") else { return false };
    let Some((base, len)) = fdt.reg0(&n) else { return false };
    // SAFETY: the register window of the RNG node.
    let Ok(m) = (unsafe { Mmio::map(base, len) }) else { return false };
    if m.r32(RNG_CTRL_OFFSET) & RNG_CTRL_RNG_RBGEN_MASK == 0 {
        m.w32(RNG_TOTAL_BIT_COUNT_THRESHOLD_OFFSET, 0x40000);
        m.w32(RNG_FIFO_COUNT_OFFSET, 2 << RNG_FIFO_COUNT_RNG_FIFO_THRESHOLD_SHIFT);
        m.w32(RNG_CTRL_OFFSET, (0x3 << RNG_CTRL_RNG_DIV_CTRL_SHIFT) | RNG_CTRL_RNG_RBGEN_MASK);
    }
    *RNG.lock() = Some(m);
    true
}

/// One 32-bit word from the FIFO once the warm-up has passed
/// (`bcm2711_rng200_read`), or None without a generator or after
/// `WAIT_US` without one.
pub fn read_u32() -> Option<u32> {
    let m = (*RNG.lock())?;
    let deadline = crate::time::uptime_us() + WAIT_US;
    while m.r32(RNG_TOTAL_BIT_COUNT_OFFSET) <= 16 {
        if crate::time::uptime_us() > deadline { return None; }
        core::hint::spin_loop();
    }
    while m.r32(RNG_FIFO_COUNT_OFFSET) & RNG_FIFO_COUNT_RNG_FIFO_COUNT_MASK == 0 {
        if crate::time::uptime_us() > deadline { return None; }
        core::hint::spin_loop();
    }
    Some(m.r32(RNG_FIFO_DATA_OFFSET))
}

/// Two words as one.
pub fn read_u64() -> Option<u64> {
    let lo = read_u32()? as u64;
    let hi = read_u32()? as u64;
    Some(hi << 32 | lo)
}
