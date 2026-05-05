//! Virtqueue + virtio-blk request servicing.
//!
//! virtio 1.x split-virtqueue layout (per spec §2.7):
//!
//! ```text
//!   desc:     array of vring_desc[queue_size]                    16 B each
//!   avail:    flags(2) | idx(2) | ring[size](2 each) | used_event(2)
//!   used:     flags(2) | idx(2) | elem[size](id u32 + len u32)   8 B each
//! ```
//!
//! virtio-blk request shape (3+ descriptors per request):
//!   [0]   driver-readable header  (16 B: type + reserved + sector)
//!   [1..] data buffer(s)          (read or write per VRING_DESC_F_WRITE)
//!   [n]   driver-writable status  (1 B)

#![allow(dead_code)]

use super::guest_mem;

// vring_desc flags
pub const VRING_DESC_F_NEXT:     u16 = 1;
pub const VRING_DESC_F_WRITE:    u16 = 2;
pub const VRING_DESC_F_INDIRECT: u16 = 4;

// virtio-blk request types
const VIRTIO_BLK_T_IN:     u32 = 0;
const VIRTIO_BLK_T_OUT:    u32 = 1;
const VIRTIO_BLK_T_FLUSH:  u32 = 4;
const VIRTIO_BLK_T_GET_ID: u32 = 8;

// Status return codes (1 byte at the tail descriptor)
const VIRTIO_BLK_S_OK:     u8 = 0;
const VIRTIO_BLK_S_IOERR:  u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

const SECTOR_SIZE: u64 = 512;

#[derive(Clone, Copy)]
pub struct Desc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

pub fn read_desc(host_base: u64, table: u64, idx: u16, queue_size: u16) -> Option<Desc> {
    if idx >= queue_size { return None; }
    let off = table + (idx as u64) * 16;
    Some(Desc {
        addr: guest_mem::read_u64(host_base, off)?,
        len: guest_mem::read_u32(host_base, off + 8)?,
        flags: guest_mem::read_u16(host_base, off + 12)?,
        next: guest_mem::read_u16(host_base, off + 14)?,
    })
}

/// Read the current driver-side avail-ring head index.
pub fn avail_idx(host_base: u64, avail_gpa: u64) -> Option<u16> {
    guest_mem::read_u16(host_base, avail_gpa + 2)
}

/// Read the descriptor head index for a given slot in the avail-ring.
pub fn avail_ring(host_base: u64, avail_gpa: u64, queue_size: u16, slot: u16) -> Option<u16> {
    let i = (slot % queue_size) as u64;
    guest_mem::read_u16(host_base, avail_gpa + 4 + i * 2)
}

/// Push a (head, len) pair to the used ring and bump used.idx with a
/// release-fence so the descriptor writes settle first.
pub fn used_push(host_base: u64, used_gpa: u64, queue_size: u16, used_idx: &mut u16, head: u16, len: u32) {
    let slot = (*used_idx % queue_size) as u64;
    let elem = used_gpa + 4 + slot * 8;
    guest_mem::write_u32(host_base, elem,     head as u32);
    guest_mem::write_u32(host_base, elem + 4, len);
    *used_idx = used_idx.wrapping_add(1);
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    guest_mem::write_u16(host_base, used_gpa + 2, *used_idx);
}

/// Walk the available ring from `last_avail_idx` to the current head
/// and service each request. Returns true if any request completed
/// (caller should set ISR + inject IRQ).
pub fn service_blk_queue(
    host_base: u64,
    desc_table: u64,
    avail: u64,
    used: u64,
    queue_size: u16,
    last_avail_idx: &mut u16,
    used_idx: &mut u16,
    backing: &mut [u8],
) -> bool {
    // avail.flags @ +0 (ignored — VIRTIO_F_RING_EVENT_IDX off)
    // avail.idx   @ +2
    let avail_idx = match guest_mem::read_u16(host_base, avail + 2) {
        Some(v) => v,
        None => return false,
    };
    if avail_idx == *last_avail_idx {
        return false;
    }

    let mut serviced_any = false;

    while *last_avail_idx != avail_idx {
        // ring[i] @ avail + 4 + (i % size) * 2
        let ring_slot = (*last_avail_idx % queue_size) as u64;
        let head_idx = match guest_mem::read_u16(host_base, avail + 4 + ring_slot * 2) {
            Some(v) => v,
            None => break,
        };

        let total_written = service_one_request(
            host_base, desc_table, head_idx, queue_size, backing,
        );

        // used.elem[used_idx % size] = (head_idx, total_written)
        let used_slot = (*used_idx % queue_size) as u64;
        let used_elem = used + 4 + used_slot * 8;
        guest_mem::write_u32(host_base, used_elem,     head_idx as u32);
        guest_mem::write_u32(host_base, used_elem + 4, total_written);

        *used_idx = used_idx.wrapping_add(1);
        *last_avail_idx = last_avail_idx.wrapping_add(1);
        serviced_any = true;
    }

    if serviced_any {
        // Memory ordering: descriptor writes must be visible before
        // the used.idx update. On x86 this is implicit (writes are
        // ordered) but we add a compiler fence for clarity.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        // used.idx @ +2
        guest_mem::write_u16(host_base, used + 2, *used_idx);
    }

    serviced_any
}

/// Walk one descriptor chain, perform the I/O, write the status byte.
/// Returns the number of bytes written into the guest's buffers (used
/// in the used-ring `len` field — virtio-blk convention is data bytes
/// + 1 for the status byte on a successful read).
fn service_one_request(
    host_base: u64,
    desc_table: u64,
    head_idx: u16,
    queue_size: u16,
    backing: &mut [u8],
) -> u32 {
    // [0] header
    let head = match read_desc(host_base, desc_table, head_idx, queue_size) {
        Some(d) => d,
        None => return 0,
    };
    if head.len < 16 || head.flags & VRING_DESC_F_WRITE != 0 {
        return 0; // malformed
    }
    let req_type = match guest_mem::read_u32(host_base, head.addr) {
        Some(v) => v,
        None => return 0,
    };
    let sector = match guest_mem::read_u64(host_base, head.addr + 8) {
        Some(v) => v,
        None => return 0,
    };

    if head.flags & VRING_DESC_F_NEXT == 0 {
        return 0; // header alone — invalid
    }

    // Walk descriptors after the header. The chain ends with a 1-byte
    // writable descriptor for status.
    let mut idx = head.next;
    let mut bytes_written: u32 = 0;
    let mut status_addr: u64 = 0;
    let mut status: u8 = VIRTIO_BLK_S_OK;
    let mut sector_off: u64 = sector * SECTOR_SIZE;

    loop {
        let d = match read_desc(host_base, desc_table, idx, queue_size) {
            Some(v) => v,
            None => { status = VIRTIO_BLK_S_IOERR; break; }
        };
        let has_next = d.flags & VRING_DESC_F_NEXT != 0;
        let writable = d.flags & VRING_DESC_F_WRITE != 0;

        if !has_next {
            // Tail descriptor — must be 1-byte writable status.
            if !writable || d.len < 1 {
                // No status slot — best-effort, just bail.
                return bytes_written;
            }
            status_addr = d.addr;
            break;
        }

        // Middle descriptor — data buffer.
        match req_type {
            VIRTIO_BLK_T_IN => {
                // Device writes data into guest memory.
                if !writable { status = VIRTIO_BLK_S_IOERR; }
                else {
                    let n = d.len as usize;
                    let end = sector_off as usize + n;
                    if end > backing.len() {
                        status = VIRTIO_BLK_S_IOERR;
                    } else {
                        guest_mem::write_bytes(host_base, d.addr, &backing[sector_off as usize..end]);
                        bytes_written = bytes_written.saturating_add(n as u32);
                        sector_off += n as u64;
                    }
                }
            }
            VIRTIO_BLK_T_OUT => {
                if writable { status = VIRTIO_BLK_S_IOERR; }
                else {
                    let n = d.len as usize;
                    let end = sector_off as usize + n;
                    if end > backing.len() {
                        status = VIRTIO_BLK_S_IOERR;
                    } else {
                        guest_mem::read_bytes(host_base, d.addr, &mut backing[sector_off as usize..end]);
                        sector_off += n as u64;
                    }
                }
            }
            VIRTIO_BLK_T_GET_ID => {
                // Device writes a 20-byte ASCII serial number.
                if writable {
                    const ID: &[u8; 20] = b"nopeek-microvm-blk0\0";
                    let n = (d.len as usize).min(ID.len());
                    guest_mem::write_bytes(host_base, d.addr, &ID[..n]);
                    bytes_written = bytes_written.saturating_add(n as u32);
                }
            }
            VIRTIO_BLK_T_FLUSH => {
                // No-op — backing is in-RAM.
            }
            _ => { status = VIRTIO_BLK_S_UNSUPP; }
        }

        idx = d.next;
    }

    if status_addr != 0 {
        guest_mem::write_u8(host_base, status_addr, status);
        // Spec convention: include the status byte in the bytes-written
        // count for read responses. Linux's blk layer relies on this.
        bytes_written = bytes_written.saturating_add(1);
    }

    bytes_written
}
