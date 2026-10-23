//! display/dc/link/protocols/link_dpcd.c (Linux 6.18.55): DPCD reads and
//! writes of a link through the DM, split at the mandatory partitions and
//! widened to the blocks that must be read in one transaction. The two
//! tables are Linux's statics (tables.rs), start/end pairs.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_dm_helpers::{dm_helpers_dp_read_dpcd, dm_helpers_dp_write_dpcd};
use crate::dc::dc;
use crate::dc_types::dc_status;
use alloc::vec;
use alloc::vec::Vec;

/// `END_ADDRESS` / `ADDRESS_RANGE_SIZE`.
fn end_address(start: u32, size: u32) -> u32 {
    start.wrapping_add(size).wrapping_sub(1)
}

fn address_range_size(start: u32, end: u32) -> u32 {
    end.wrapping_sub(start).wrapping_add(1)
}

/// `struct dpcd_address_range`.
#[derive(Clone, Copy)]
struct dpcd_address_range {
    start: u32,
    end: u32,
}

fn mandatory_dpcd_partitions(i: usize) -> dpcd_address_range {
    let t = &crate::tables::mandatory_dpcd_partitions;
    dpcd_address_range { start: t[2 * i], end: t[2 * i + 1] }
}

fn mandatory_dpcd_blocks() -> impl Iterator<Item = dpcd_address_range> {
    crate::tables::mandatory_dpcd_blocks.chunks(2).map(|p| dpcd_address_range { start: p[0], end: p[1] })
}

/// `internal_link_read_dpcd`.
fn internal_link_read_dpcd(adev: &amdgpu_device, dc: &mut dc, li: usize, address: u32, data: &mut [u8]) -> dc_status {
    if !dc.links[li].aux_access_disabled && !dm_helpers_dp_read_dpcd(adev, dc, li, address, data) {
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    dc_status::DC_OK
}

/// `internal_link_write_dpcd`.
fn internal_link_write_dpcd(adev: &amdgpu_device, dc: &mut dc, li: usize, address: u32, data: &[u8]) -> dc_status {
    if !dc.links[li].aux_access_disabled && !dm_helpers_dp_write_dpcd(adev, dc, li, address, data) {
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    dc_status::DC_OK
}

/// `do_addresses_intersect_with_range`.
fn do_addresses_intersect_with_range(range: &dpcd_address_range, start_address: u32, end_address: u32) -> bool {
    start_address <= range.end && end_address >= range.start
}

/// `dpcd_get_next_partition_size`.
fn dpcd_get_next_partition_size(address: u32, size: u32) -> u32 {
    let end = end_address(address, size);
    let mut partition_iterator = 0;

    // find current partition
    // this loop spins forever if partition map above is not surjective
    while !do_addresses_intersect_with_range(&mandatory_dpcd_partitions(partition_iterator), address, end) {
        partition_iterator += 1;
    }
    if end < mandatory_dpcd_partitions(partition_iterator).end {
        return size;
    }
    address_range_size(address, mandatory_dpcd_partitions(partition_iterator).end)
}

/// `dpcd_extend_address_range`: the widened address and size, and the
/// buffer the read goes into when it was widened (None: the caller's).
fn dpcd_extend_address_range(in_address: u32, in_size: u32) -> (u32, u32, Option<Vec<u8>>) {
    let end = end_address(in_address, in_size);
    let mut new_addr_range = dpcd_address_range { start: in_address, end };

    for addr_range in mandatory_dpcd_blocks() {
        if addr_range.start <= in_address && addr_range.end >= in_address {
            new_addr_range.start = addr_range.start;
        }

        if addr_range.start <= end && addr_range.end >= end {
            new_addr_range.end = addr_range.end;
        }
    }
    if new_addr_range.start != in_address || new_addr_range.end != end {
        let out_size = address_range_size(new_addr_range.start, new_addr_range.end);
        return (new_addr_range.start, out_size, Some(vec![0; out_size as usize]));
    }
    (in_address, in_size, None)
}

/// `dpcd_reduce_address_range`. Linux copies the caller's buffer into the
/// widened one before freeing it, so a widened read never reaches the
/// caller; the port does the same.
fn dpcd_reduce_address_range(extended_address: u32, extended_data: Option<Vec<u8>>, reduced_address: u32,
                             reduced_data: &[u8]) {
    let offset = (reduced_address - extended_address) as usize;

    // If the address is same, address was not extended.
    // So we do not need to free any memory.
    // The data is in original buffer(reduced_data).
    let Some(mut extended_data) = extended_data else { return };

    extended_data[offset..offset + reduced_data.len()].copy_from_slice(reduced_data);
}

/// `core_link_read_dpcd`.
pub fn core_link_read_dpcd(adev: &amdgpu_device, dc: &mut dc, li: usize, address: u32, data: &mut [u8]) -> dc_status {
    let size = data.len() as u32;
    let mut status = dc_status::DC_ERROR_UNEXPECTED;
    let mut data_index = 0u32;

    let (extended_address, extended_size, mut widened) = dpcd_extend_address_range(address, size);
    let mut partitioned_address = extended_address;
    // size of the remaining partitioned address space
    let mut size_left_to_read = extended_size;
    while size_left_to_read != 0 {
        // size of the next partition to be read from
        let partition_size = dpcd_get_next_partition_size(partitioned_address, size_left_to_read);
        let range = data_index as usize..(data_index + partition_size) as usize;
        let buf = match widened.as_deref_mut() {
            Some(w) => &mut w[range],
            None => &mut data[range],
        };
        status = internal_link_read_dpcd(adev, dc, li, partitioned_address, buf);
        if status != dc_status::DC_OK {
            break;
        }
        partitioned_address += partition_size;
        data_index += partition_size;
        size_left_to_read -= partition_size;
    }
    dpcd_reduce_address_range(extended_address, widened, address, data);
    status
}

/// `core_link_write_dpcd`.
pub fn core_link_write_dpcd(adev: &amdgpu_device, dc: &mut dc, li: usize, mut address: u32, data: &[u8]) -> dc_status {
    let mut size = data.len() as u32;
    let mut data_index = 0u32;
    let mut status = dc_status::DC_ERROR_UNEXPECTED;

    while size != 0 {
        let partition_size = dpcd_get_next_partition_size(address, size);
        status = internal_link_write_dpcd(adev, dc, li, address,
            &data[data_index as usize..(data_index + partition_size) as usize]);
        if status != dc_status::DC_OK {
            break;
        }
        address += partition_size;
        data_index += partition_size;
        size -= partition_size;
    }
    status
}
