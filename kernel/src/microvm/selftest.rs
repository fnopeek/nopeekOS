//! A guest booted from a bundle the loader left in RAM (`-initrd` under
//! QEMU), on a machine without a system disk: the microVM's bring-up on
//! Arm before images come from npkFS.
//!
//! Bundle: "NPKVMT01", then the image, initramfs and command line lengths
//! (u64 little endian), then the three in that order.

use crate::platform::dt::Fdt;

const MAGIC: &[u8; 8] = b"NPKVMT01";

/// `/chosen/linux,initrd-start` and `-end`, one or two cells each.
pub fn initrd(fdt: &Fdt) -> Option<(u64, u64)> {
    let chosen = fdt.find_path(b"/chosen")?;
    let read = |name: &[u8]| -> Option<u64> {
        let (o, len) = fdt.prop(&chosen, name)?;
        match len {
            4 => Some(fdt.cell_u32(o) as u64),
            8 => Some((fdt.cell_u32(o) as u64) << 32 | fdt.cell_u32(o + 4) as u64),
            _ => None,
        }
    };
    let (s, e) = (read(b"linux,initrd-start")?, read(b"linux,initrd-end")?);
    (e > s).then_some((s, e))
}

/// Boot the bundle at `[start, end)` and wait until the guest is gone.
pub fn run(start: u64, end: u64) -> ! {
    // SAFETY: the loader's initrd, reserved from the frame allocator
    // (`holes`) and covered by the identity map.
    let b = unsafe { core::slice::from_raw_parts(start as *const u8, (end - start) as usize) };
    let le = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8])) as usize;
    if b.len() < 32 || &b[..8] != MAGIC {
        crate::kprintln!("[microvm] the initrd is no guest bundle");
        crate::arch::halt_forever();
    }
    let (ni, nr, nc) = (le(8), le(16), le(24));
    if 32 + ni + nr + nc > b.len() {
        crate::kprintln!("[microvm] guest bundle truncated");
        crate::arch::halt_forever();
    }
    let image = &b[32..32 + ni];
    let rd = &b[32 + ni..32 + ni + nr];
    let cmd = &b[32 + ni + nr..32 + ni + nr + nc];
    crate::kprintln!("[microvm] guest bundle: Image {} KB, initramfs {} KB, cmdline {:?}",
        ni / 1024, nr / 1024, core::str::from_utf8(cmd).unwrap_or("?"));
    crate::microvm::boottime::start();
    if let Err(e) = crate::microvm::vm_open(image, cmd, (nr > 0).then_some(rd), b"") {
        crate::kprintln!("[microvm] launch FAILED: {}", e);
        crate::arch::halt_forever();
    }
    let t0 = crate::time::counter();
    while crate::microvm::vm_busy() {
        crate::microvm::vm_poll_slice();
        crate::arch::aarch64::cpu::wait_for_interrupt();
    }
    let ms = crate::time::counter().wrapping_sub(t0) / (crate::time::counter_hz() / 1000).max(1);
    crate::kprintln!("[microvm] guest gone after {} ms", ms);
    crate::arch::halt_forever()
}
