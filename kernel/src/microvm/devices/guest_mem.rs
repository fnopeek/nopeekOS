//! Guest physical memory accessors.
//!
//! The 256 MB guest window is identity-mapped through EPT/NPT to a
//! contiguous host range starting at `host_base`. Within the kernel's
//! 64 GB identity-mapped region (set up at boot), `host_virt =
//! host_phys = host_base + gpa` for any guest physical address inside
//! the window.
//!
//! All accessors bounds-check against the window so a buggy/malicious
//! guest descriptor can't drag us into kernel memory.

#![allow(dead_code)]

const GUEST_RAM_BYTES: u64 = 256 * 1024 * 1024;

#[inline]
fn check(gpa: u64, len: u64) -> bool {
    gpa.checked_add(len).is_some_and(|end| end <= GUEST_RAM_BYTES)
}

pub fn read_u8(host_base: u64, gpa: u64) -> Option<u8> {
    if !check(gpa, 1) { return None; }
    Some(unsafe { core::ptr::read_volatile((host_base + gpa) as *const u8) })
}

pub fn read_u16(host_base: u64, gpa: u64) -> Option<u16> {
    if !check(gpa, 2) { return None; }
    Some(unsafe { core::ptr::read_volatile((host_base + gpa) as *const u16) })
}

pub fn read_u32(host_base: u64, gpa: u64) -> Option<u32> {
    if !check(gpa, 4) { return None; }
    Some(unsafe { core::ptr::read_volatile((host_base + gpa) as *const u32) })
}

pub fn read_u64(host_base: u64, gpa: u64) -> Option<u64> {
    if !check(gpa, 8) { return None; }
    Some(unsafe { core::ptr::read_volatile((host_base + gpa) as *const u64) })
}

pub fn write_u8(host_base: u64, gpa: u64, val: u8) -> bool {
    if !check(gpa, 1) { return false; }
    unsafe { core::ptr::write_volatile((host_base + gpa) as *mut u8, val); }
    true
}

pub fn write_u16(host_base: u64, gpa: u64, val: u16) -> bool {
    if !check(gpa, 2) { return false; }
    unsafe { core::ptr::write_volatile((host_base + gpa) as *mut u16, val); }
    true
}

pub fn write_u32(host_base: u64, gpa: u64, val: u32) -> bool {
    if !check(gpa, 4) { return false; }
    unsafe { core::ptr::write_volatile((host_base + gpa) as *mut u32, val); }
    true
}

pub fn read_bytes(host_base: u64, gpa: u64, dst: &mut [u8]) -> bool {
    if !check(gpa, dst.len() as u64) { return false; }
    unsafe {
        core::ptr::copy_nonoverlapping(
            (host_base + gpa) as *const u8,
            dst.as_mut_ptr(),
            dst.len(),
        );
    }
    true
}

pub fn write_bytes(host_base: u64, gpa: u64, src: &[u8]) -> bool {
    if !check(gpa, src.len() as u64) { return false; }
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr(),
            (host_base + gpa) as *mut u8,
            src.len(),
        );
    }
    true
}
