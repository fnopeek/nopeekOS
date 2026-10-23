//! drivers/gpu/drm/drm_edid.c + drm_displayid.c (Linux 6.18.55): reading an
//! EDID over DDC block by block, validating it, and what
//! `drm_edid_connector_update` derives from it (display info, ELD, tiles),
//! with the base and extension block, CTA data block and DisplayID
//! iterators under it. An EDID is its bytes; every pointer Linux keeps into
//! it is an offset here. Mode lists are DRM's userspace side and not a part.
//! The CTA data block parsers (HDMI and HDMI Forum VSDB, Microsoft VSDB,
//! YCbCr 4:2:0, VCDB, HDR static metadata, video data blocks) and the
//! DisplayID tile and MSO parsers are named where Linux calls them and not
//! yet ported: they run only on an EDID with such blocks.
#![allow(dead_code)]

use crate::drm_connector::*;
use crate::i2c::{i2c_bus, i2c_msg, i2c_transfer, I2C_M_RD};
use crate::amdgpu_dm_helpers::dm_ctx;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;

pub const EDID_LENGTH: usize = 128;

/// `struct edid` byte offsets.
const EDID_MFG_ID: usize = 8;
const EDID_PROD_CODE: usize = 10;
const EDID_SERIAL: usize = 12;
const EDID_MFG_WEEK: usize = 16;
const EDID_MFG_YEAR: usize = 17;
const EDID_VERSION: usize = 18;
const EDID_REVISION: usize = 19;
const EDID_INPUT: usize = 20;
const EDID_WIDTH_CM: usize = 21;
const EDID_HEIGHT_CM: usize = 22;
const EDID_FEATURES: usize = 24;
const EDID_DETAILED_TIMINGS_OFF: usize = 54;
const EDID_DETAILED_TIMINGS: usize = 4;
const EDID_EXTENSIONS: usize = 126;
const EDID_CHECKSUM: usize = 127;

/// `struct detailed_timing` byte offsets: `pixel_clock`, then
/// `data.other_data` = pad1, type, pad2, data.
const DT_SIZE: usize = 18;
const DT_PAD1: usize = 2;
const DT_TYPE: usize = 3;
const DT_PAD2: usize = 4;
const DT_DATA: usize = 5;
/// `detailed_data_string.str`.
const DT_STR_LEN: usize = 13;
/// `detailed_data_monitor_range`: min_vfreq, max_vfreq, …, flags.
const DT_RANGE_MIN_VFREQ: usize = DT_DATA;
const DT_RANGE_MAX_VFREQ: usize = DT_DATA + 1;
const DT_RANGE_FLAGS: usize = DT_DATA + 5;

/// `struct drm_edid`: the EDID and the size allocated for it.
#[derive(Clone)]
pub struct drm_edid {
    pub edid: Vec<u8>,
    pub size: usize,
}

/// `struct drm_edid_ident`.
pub struct drm_edid_ident {
    pub panel_id: u32,
    pub name: Option<&'static [u8]>,
}

/// `drm_edid_encode_panel_id` (drm_edid.h).
pub const fn drm_edid_encode_panel_id(vend_chr_0: u8, vend_chr_1: u8, vend_chr_2: u8, product_id: u32) -> u32 {
    ((vend_chr_0 as u32 - b'@' as u32) & 0x1f) << 26
        | ((vend_chr_1 as u32 - b'@' as u32) & 0x1f) << 21
        | ((vend_chr_2 as u32 - b'@' as u32) & 0x1f) << 16
        | (product_id & 0xffff)
}

fn edid_product_id(edid: &[u8]) -> u32 {
    edid[EDID_PROD_CODE] as u32 | (edid[EDID_PROD_CODE + 1] as u32) << 8
}

// ── blocks ──

/// `edid_hfeeodb_extension_block_count`.
fn edid_hfeeodb_extension_block_count(edid: &[u8]) -> usize {
    // No extensions according to base block, no HF-EEODB.
    if edid_extension_block_count(edid) == 0 {
        return 0;
    }

    // HF-EEODB is always in the first EDID extension block only
    let cta = &edid[EDID_LENGTH..];
    if edid_block_tag(cta) != CEA_EXT as u8 || cea_revision(cta) < 3 {
        return 0;
    }

    // Need to have the data block collection, and at least 3 bytes.
    if cea_db_collection_size(cta) < 3 {
        return 0;
    }

    // Sinks that include the HF-EEODB in their E-EDID shall include one and
    // only one instance of the HF-EEODB in the E-EDID, occupying bytes 4
    // through 6 of Block 1 of the E-EDID.
    if !cea_db_is_hdmi_forum_eeodb(&cta[4..]) {
        return 0;
    }

    cta[4 + 2] as usize
}

/// `edid_hfeeodb_block_count`.
fn edid_hfeeodb_block_count(edid: &[u8]) -> usize {
    let eeodb = edid_hfeeodb_extension_block_count(edid);

    if eeodb != 0 { eeodb + 1 } else { 0 }
}

/// `edid_extension_block_count`.
fn edid_extension_block_count(edid: &[u8]) -> usize {
    edid[EDID_EXTENSIONS] as usize
}

/// `edid_block_count`.
fn edid_block_count(edid: &[u8]) -> usize {
    edid_extension_block_count(edid) + 1
}

/// `edid_size_by_blocks`.
fn edid_size_by_blocks(num_blocks: usize) -> usize {
    num_blocks * EDID_LENGTH
}

/// `edid_size`.
fn edid_size(edid: &[u8]) -> usize {
    edid_size_by_blocks(edid_block_count(edid))
}

/// `edid_block_data`: the offset of block `index`.
fn edid_block_data(index: usize) -> usize {
    index * EDID_LENGTH
}

/// `edid_extension_block_data`.
fn edid_extension_block_data(index: usize) -> usize {
    edid_block_data(index + 1)
}

/// `__drm_edid_block_count`: the count the EDID gives, may exceed the
/// allocated size.
fn __drm_edid_block_count(drm_edid: &drm_edid) -> usize {
    // Starting point
    let mut num_blocks = edid_block_count(&drm_edid.edid);

    // HF-EEODB override
    if drm_edid.size >= edid_size_by_blocks(2) {
        // Note: HF-EEODB may specify a smaller extension count than the
        // regular one. Unlike in buffer allocation, here we can use it.
        let eeodb = edid_hfeeodb_block_count(&drm_edid.edid);
        if eeodb != 0 {
            num_blocks = eeodb;
        }
    }

    num_blocks
}

/// `drm_edid_block_count`: limited by the allocated size.
fn drm_edid_block_count(drm_edid: &drm_edid) -> usize {
    __drm_edid_block_count(drm_edid).min(drm_edid.size / EDID_LENGTH)
}

/// `drm_edid_extension_block_count`.
fn drm_edid_extension_block_count(drm_edid: &drm_edid) -> isize {
    drm_edid_block_count(drm_edid) as isize - 1
}

/// `drm_edid_legacy_init`: trust the EDID's own size.
fn drm_edid_legacy_init(edid: &[u8]) -> drm_edid {
    drm_edid { edid: edid.to_vec(), size: edid_size(edid) }
}

/// `struct drm_edid_iter`: base and extension blocks.
struct drm_edid_iter<'a> {
    drm_edid: Option<&'a drm_edid>,
    // Current block index.
    index: usize,
}

/// `drm_edid_iter_begin`.
fn drm_edid_iter_begin(drm_edid: Option<&drm_edid>) -> drm_edid_iter<'_> {
    drm_edid_iter { drm_edid, index: 0 }
}

/// `__drm_edid_iter_next`: the next block's offset.
fn __drm_edid_iter_next(iter: &mut drm_edid_iter) -> Option<usize> {
    let drm_edid = iter.drm_edid?;

    if iter.index < drm_edid_block_count(drm_edid) {
        let block = edid_block_data(iter.index);
        iter.index += 1;
        return Some(block);
    }
    None
}

/// `drm_edid_iter_end`.
fn drm_edid_iter_end(iter: &mut drm_edid_iter) {
    iter.drm_edid = None;
    iter.index = 0;
}

const EDID_HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

/// `edid_header_fix`.
fn edid_header_fix(edid: &mut [u8]) {
    edid[..8].copy_from_slice(&EDID_HEADER);
}

/// `drm_edid_header_is_valid`: 8 if the header is perfect, down to 0.
pub fn drm_edid_header_is_valid(edid: &[u8]) -> i32 {
    let mut score = 0;

    for i in 0..EDID_HEADER.len() {
        if edid[i] == EDID_HEADER[i] {
            score += 1;
        }
    }

    score
}

/// `edid_fixup` (module parameter, default 6).
const EDID_FIXUP: i32 = 6;

/// `edid_block_compute_checksum`.
fn edid_block_compute_checksum(block: &[u8]) -> u8 {
    let mut csum: u8 = 0;

    for &b in &block[..EDID_LENGTH - 1] {
        csum = csum.wrapping_add(b);
    }

    (0x100 - csum as u32) as u8
}

/// `edid_block_get_checksum`.
fn edid_block_get_checksum(block: &[u8]) -> u8 {
    block[EDID_CHECKSUM]
}

/// `edid_block_tag`.
fn edid_block_tag(block: &[u8]) -> u8 {
    block[0]
}

/// `edid_block_is_zero`.
fn edid_block_is_zero(edid: &[u8]) -> bool {
    edid[..EDID_LENGTH].iter().all(|&b| b == 0)
}

/// `drm_edid_eq`.
fn drm_edid_eq(drm_edid: Option<&drm_edid>, raw_edid: Option<&[u8]>) -> bool {
    let edid1_present = drm_edid.is_some_and(|d| d.size != 0);
    let edid2_present = raw_edid.is_some_and(|r| !r.is_empty());

    if edid1_present != edid2_present {
        return false;
    }

    if edid1_present {
        let (d, r) = (drm_edid.unwrap(), raw_edid.unwrap());
        if d.size != r.len() {
            return false;
        }

        if d.edid[..d.size] != *r {
            return false;
        }
    }

    true
}

/// `enum edid_block_status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum edid_block_status {
    EDID_BLOCK_OK = 0,
    EDID_BLOCK_READ_FAIL,
    EDID_BLOCK_NULL,
    EDID_BLOCK_ZERO,
    EDID_BLOCK_HEADER_CORRUPT,
    EDID_BLOCK_HEADER_REPAIR,
    EDID_BLOCK_HEADER_FIXED,
    EDID_BLOCK_CHECKSUM,
    EDID_BLOCK_VERSION,
}
use edid_block_status::*;

/// `edid_block_check`.
fn edid_block_check(block: &[u8], is_base_block: bool) -> edid_block_status {
    if is_base_block {
        let score = drm_edid_header_is_valid(block);

        if score < EDID_FIXUP.clamp(0, 8) {
            if edid_block_is_zero(block) {
                return EDID_BLOCK_ZERO;
            } else {
                return EDID_BLOCK_HEADER_CORRUPT;
            }
        }

        if score < 8 {
            return EDID_BLOCK_HEADER_REPAIR;
        }
    }

    if edid_block_compute_checksum(block) != edid_block_get_checksum(block) {
        if edid_block_is_zero(block) {
            return EDID_BLOCK_ZERO;
        } else {
            return EDID_BLOCK_CHECKSUM;
        }
    }

    if is_base_block && block[EDID_VERSION] != 1 {
        return EDID_BLOCK_VERSION;
    }

    EDID_BLOCK_OK
}

/// `edid_block_status_valid`.
fn edid_block_status_valid(status: edid_block_status, tag: u8) -> bool {
    status == EDID_BLOCK_OK
        || status == EDID_BLOCK_HEADER_FIXED
        || (status == EDID_BLOCK_CHECKSUM && tag == CEA_EXT as u8)
}

/// `edid_block_valid`.
fn edid_block_valid(block: &[u8], base: bool) -> bool {
    edid_block_status_valid(edid_block_check(block, base), edid_block_tag(block))
}

/// `edid_block_status_print`: pr_notice for what is broken, pr_debug
/// (off) for the rest.
fn edid_block_status_print(status: edid_block_status, block: &[u8], block_num: usize) {
    match status {
        EDID_BLOCK_ZERO => {
            Line::new().s("EDID block ").dec(block_num as u64).s(" is all zeroes").say();
        }
        EDID_BLOCK_HEADER_CORRUPT => Line::new().s("EDID has corrupt header").say(),
        EDID_BLOCK_CHECKSUM => {
            if !edid_block_status_valid(status, edid_block_tag(block)) {
                Line::new().s("EDID block ").dec(block_num as u64).s(" (tag ").hex(edid_block_tag(block) as u32)
                    .s(") checksum is invalid, remainder is ").dec(edid_block_compute_checksum(block) as u64).say();
            }
        }
        EDID_BLOCK_VERSION => {
            Line::new().s("EDID has major version ").dec(block[EDID_VERSION] as u64).s(", instead of 1").say();
        }
        _ => {}
    }
}

/// `drm_edid_block_valid`: validate, and repair, a base or extension block.
fn drm_edid_block_valid(block: &mut [u8], block_num: usize, _print_bad_edid: bool,
                        edid_corrupt: Option<&mut bool>) -> bool {
    let is_base_block = block_num == 0;

    let mut status = edid_block_check(block, is_base_block);
    if status == EDID_BLOCK_HEADER_REPAIR {
        edid_header_fix(block);

        // Retry with fixed header, update status if that worked.
        status = edid_block_check(block, is_base_block);
        if status == EDID_BLOCK_OK {
            status = EDID_BLOCK_HEADER_FIXED;
        }
    }

    if let Some(edid_corrupt) = edid_corrupt {
        // Unknown major version isn't corrupt but we can't use it. Only
        // the base block can reset edid_corrupt to false.
        if is_base_block && (status == EDID_BLOCK_OK || status == EDID_BLOCK_VERSION) {
            *edid_corrupt = false;
        } else if status != EDID_BLOCK_OK {
            *edid_corrupt = true;
        }
    }

    edid_block_status_print(status, block, block_num);

    // Determine whether we can use this block with this status.
    edid_block_status_valid(status, edid_block_tag(block))
}

/// `drm_edid_is_valid`: every block of the EDID (the raw dump of a bad
/// block is debug output).
pub fn drm_edid_is_valid(edid: &mut [u8]) -> bool {
    for i in 0..edid_block_count(edid) {
        let o = edid_block_data(i);
        if !drm_edid_block_valid(&mut edid[o..o + EDID_LENGTH], i, true, None) {
            return false;
        }
    }

    true
}

/// `edid_filter_invalid_blocks`.
fn edid_filter_invalid_blocks(mut edid: Vec<u8>, alloc_size: &mut usize) -> Option<Vec<u8>> {
    let mut valid_blocks = 0;

    // Note: If the EDID uses HF-EEODB, but has invalid blocks, we'll revert
    // back to regular extension count here. We don't want to start
    // modifying the HF-EEODB extension too.
    for i in 0..edid_block_count(&edid) {
        let src = edid_block_data(i);

        if edid_block_valid(&edid[src..src + EDID_LENGTH], i == 0) {
            let dst = edid_block_data(valid_blocks);

            edid.copy_within(src..src + EDID_LENGTH, dst);
            valid_blocks += 1;
        }
    }

    // We already trusted the base block to be valid here...
    if valid_blocks == 0 {
        return None;
    }

    edid[EDID_EXTENSIONS] = (valid_blocks - 1) as u8;
    edid[EDID_CHECKSUM] = edid_block_compute_checksum(&edid);

    *alloc_size = edid_size_by_blocks(valid_blocks);

    edid.truncate(*alloc_size);
    Some(edid)
}

pub const DDC_SEGMENT_ADDR: u16 = 0x30;
pub const DDC_ADDR: u16 = 0x50;

/// `drm_do_probe_ddc_edid`: one block over DDC, the segment pointer only
/// when the block needs it. 0, or -1.
pub fn drm_do_probe_ddc_edid(bus: &mut i2c_bus, ctx: &mut dm_ctx, buf: &mut [u8], block: u32) -> i32 {
    let len = buf.len();
    let mut start = [(block as usize * EDID_LENGTH) as u8];
    let mut segment = [(block >> 1) as u8];
    let xfers = if segment[0] != 0 { 3 } else { 2 };
    let mut retries = 5;
    let mut ret;

    // The core I2C driver will automatically retry the transfer if the
    // adapter reports EAGAIN. However, we find that bit-banging transfers
    // are susceptible to errors under a heavily loaded machine and
    // generate spurious NAKs and timeouts. Retrying the transfer
    // of the individual block a few times seems to overcome this.
    loop {
        let mut msgs = [
            i2c_msg { addr: DDC_SEGMENT_ADDR, flags: 0, len: 1, buf: &mut segment },
            i2c_msg { addr: DDC_ADDR, flags: 0, len: 1, buf: &mut start },
            i2c_msg { addr: DDC_ADDR, flags: I2C_M_RD, len: len as u16, buf: &mut *buf },
        ];

        // Avoid sending the segment addr to not upset non-compliant
        // DDC monitors.
        ret = i2c_transfer(bus, ctx, &mut msgs[3 - xfers..]);

        if ret == -(crate::linux::ENXIO as i32) {
            break;
        }
        retries -= 1;
        if ret == xfers as i32 || retries == 0 {
            break;
        }
    }

    if ret == xfers as i32 { 0 } else { -1 }
}

/// `connector_bad_edid`: the real checksum of the last block for DP
/// compliance; the dump of the blocks is debug output.
fn connector_bad_edid(connector: &mut drm_connector, edid: &[u8], num_blocks: usize) {
    // 0x7e in the EDID is the number of extension blocks. The EDID
    // is 1 (base block) + num_ext_blocks big. That means we can think
    // of 0x7e in the EDID of the _index_ of the last block in the
    // combined chunk of memory.
    let last_block = edid[EDID_EXTENSIONS] as usize;

    // Calculate real checksum for the last edid extension block data
    if last_block < num_blocks {
        let o = edid_block_data(last_block);
        connector.real_edid_checksum = edid_block_compute_checksum(&edid[o..o + EDID_LENGTH]);
    }

    connector.bad_edid_counter += 1;
}

/// `drm_edid_override_get`: no debugfs override and no firmware EDID
/// (`CONFIG_DRM_LOAD_EDID_FIRMWARE` is off) in the module.
fn drm_edid_override_get(_connector: &drm_connector) -> Option<drm_edid> {
    None
}

/// `read_block_fn`: block `block` into `buf` (EDID_LENGTH bytes), 0 on
/// success.
pub type read_block_fn<'a> = dyn FnMut(&mut [u8], u32) -> i32 + 'a;

/// `edid_block_read`.
fn edid_block_read(block: &mut [u8], block_num: u32, read_block: &mut read_block_fn) -> edid_block_status {
    let is_base_block = block_num == 0;
    let mut status = EDID_BLOCK_READ_FAIL;

    for r#try in 0..4 {
        if read_block(block, block_num) != 0 {
            return EDID_BLOCK_READ_FAIL;
        }

        status = edid_block_check(block, is_base_block);
        if status == EDID_BLOCK_HEADER_REPAIR {
            edid_header_fix(block);

            // Retry with fixed header, update status if that worked.
            status = edid_block_check(block, is_base_block);
            if status == EDID_BLOCK_OK {
                status = EDID_BLOCK_HEADER_FIXED;
            }
        }

        if edid_block_status_valid(status, edid_block_tag(block)) {
            break;
        }

        // Fail early for unrepairable base block all zeros.
        if r#try == 0 && is_base_block && status == EDID_BLOCK_ZERO {
            break;
        }
    }

    status
}

/// `_drm_do_get_edid`: the EDID and its allocated size, or None.
fn _drm_do_get_edid(connector: &mut drm_connector, read_block: &mut read_block_fn) -> Option<(Vec<u8>, usize)> {
    let mut invalid_blocks = 0;
    let mut alloc_size = EDID_LENGTH;

    if let Some(o) = drm_edid_override_get(connector) {
        return Some((o.edid[..o.size].to_vec(), o.size));
    }

    let mut edid = vec![0u8; alloc_size];

    let status = edid_block_read(&mut edid, 0, read_block);

    edid_block_status_print(status, &edid, 0);

    if status == EDID_BLOCK_READ_FAIL {
        return None;
    }

    // FIXME: Clarify what a corrupt EDID actually means.
    connector.edid_corrupt = !(status == EDID_BLOCK_OK || status == EDID_BLOCK_VERSION);

    if !edid_block_status_valid(status, edid_block_tag(&edid)) {
        if status == EDID_BLOCK_ZERO {
            connector.null_edid_counter += 1;
        }

        connector_bad_edid(connector, &edid, 1);
        return None;
    }

    if edid_extension_block_count(&edid) == 0 {
        return Some((edid, alloc_size));
    }

    alloc_size = edid_size(&edid);
    edid.resize(alloc_size, 0);

    let mut num_blocks = edid_block_count(&edid);
    let mut i = 1;
    while i < num_blocks {
        let o = edid_block_data(i);

        let status = edid_block_read(&mut edid[o..o + EDID_LENGTH], i as u32, read_block);

        edid_block_status_print(status, &edid[o..o + EDID_LENGTH], i);

        if !edid_block_status_valid(status, edid_block_tag(&edid[o..])) {
            if status == EDID_BLOCK_READ_FAIL {
                return None;
            }
            invalid_blocks += 1;
        } else if i == 1 {
            // If the first EDID extension is a CTA extension, and
            // the first Data Block is HF-EEODB, override the
            // extension block count.
            //
            // Note: HF-EEODB could specify a smaller extension
            // count too, but we can't risk allocating a smaller
            // amount.
            let eeodb = edid_hfeeodb_block_count(&edid);

            if eeodb > num_blocks {
                num_blocks = eeodb;
                alloc_size = edid_size_by_blocks(num_blocks);
                edid.resize(alloc_size, 0);
            }
        }
        i += 1;
    }

    if invalid_blocks != 0 {
        connector_bad_edid(connector, &edid, num_blocks);

        edid = edid_filter_invalid_blocks(edid, &mut alloc_size)?;
    }

    Some((edid, alloc_size))
}

/// `drm_edid_raw`: the raw EDID, unless its own count would overrun it.
pub fn drm_edid_raw(drm_edid: Option<&drm_edid>) -> Option<&[u8]> {
    let drm_edid = drm_edid?;
    if drm_edid.size == 0 {
        return None;
    }

    // Do not return pointers where relying on EDID extension count would
    // lead to buffer overflow.
    if edid_size(&drm_edid.edid) > drm_edid.size {
        return None;
    }

    Some(&drm_edid.edid)
}

/// `_drm_edid_alloc`: the container, without duplicating the data.
fn _drm_edid_alloc(edid: Vec<u8>, size: usize) -> Option<drm_edid> {
    if size == 0 || size < EDID_LENGTH {
        return None;
    }

    Some(drm_edid { edid, size })
}

/// `drm_probe_ddc`: is something answering on the DDC?
pub fn drm_probe_ddc(bus: &mut i2c_bus, ctx: &mut dm_ctx) -> bool {
    let mut out = [0u8; 1];

    drm_do_probe_ddc_edid(bus, ctx, &mut out, 0) == 0
}

/// `drm_edid_read_custom`.
pub fn drm_edid_read_custom(connector: &mut drm_connector, read_block: &mut read_block_fn) -> Option<drm_edid> {
    let (edid, size) = _drm_do_get_edid(connector, read_block)?;

    _drm_edid_alloc(edid, size)
}

/// `drm_edid_read_ddc`: read over the adapter, after a probe.
pub fn drm_edid_read_ddc(connector: &mut drm_connector, bus: &mut i2c_bus, ctx: &mut dm_ctx) -> Option<drm_edid> {
    if connector.force == drm_connector_force::DRM_FORCE_OFF {
        return None;
    }

    if connector.force == drm_connector_force::DRM_FORCE_UNSPECIFIED && !drm_probe_ddc(bus, ctx) {
        return None;
    }

    // Note: Do *not* call connector updates here.
    drm_edid_read_custom(connector, &mut |buf, block| drm_do_probe_ddc_edid(bus, ctx, buf, block))
}

/// `drm_edid_get_panel_id`.
pub fn drm_edid_get_panel_id(drm_edid: &drm_edid) -> u32 {
    let edid = &drm_edid.edid;

    if drm_edid.size < EDID_LENGTH {
        return 0;
    }

    // We represent the ID as a 32-bit number so it can easily be compared
    // with "==".
    //
    // NOTE that we deal with endianness differently for the top half
    // of this ID than for the bottom half. The bottom half (the product
    // id) gets decoded as little endian by the EDID_PRODUCT_ID because
    // that's how everyone seems to interpret it. The top half (the mfg_id)
    // gets stored as big endian because that makes
    // drm_edid_encode_panel_id() and drm_edid_decode_panel_id() easier
    // to write (it's easier to extract the ASCII). It doesn't really
    // matter, though, as long as the number here is unique.
    (edid[EDID_MFG_ID] as u32) << 24 | (edid[EDID_MFG_ID + 1] as u32) << 16 | edid_product_id(edid)
}

// ── detailed blocks ──

/// `detailed_cb`: called with the 18 bytes of one detailed timing.
type detailed_cb<'a> = dyn FnMut(&[u8]) + 'a;

/// `is_display_descriptor`.
fn is_display_descriptor(descriptor: &[u8], r#type: u8) -> bool {
    descriptor[0] == 0 && descriptor[1] == 0 && descriptor[DT_PAD1] == 0 && descriptor[DT_TYPE] == r#type
}

/// `cea_for_each_detailed_block`.
fn cea_for_each_detailed_block(ext: &[u8], cb: &mut detailed_cb) {
    let d = ext[0x02] as usize;

    if !(4..=127).contains(&d) {
        return;
    }

    let n = (127 - d) / 18;
    for i in 0..n {
        cb(&ext[d + 18 * i..d + 18 * i + DT_SIZE]);
    }
}

/// `vtb_for_each_detailed_block`.
fn vtb_for_each_detailed_block(ext: &[u8], cb: &mut detailed_cb) {
    let n = (ext[0x02] as usize).min(6);

    if ext[0x01] != 1 {
        return; // unknown version
    }

    for i in 0..n {
        cb(&ext[5 + 18 * i..5 + 18 * i + DT_SIZE]);
    }
}

/// `drm_for_each_detailed_block`.
fn drm_for_each_detailed_block(drm_edid: Option<&drm_edid>, cb: &mut detailed_cb) {
    let Some(d) = drm_edid else { return };

    for i in 0..EDID_DETAILED_TIMINGS {
        let o = EDID_DETAILED_TIMINGS_OFF + DT_SIZE * i;
        cb(&d.edid[o..o + DT_SIZE]);
    }

    let mut edid_iter = drm_edid_iter_begin(Some(d));
    while let Some(ext) = __drm_edid_iter_next(&mut edid_iter) {
        let ext = &d.edid[ext..ext + EDID_LENGTH];
        match ext[0] as u32 {
            CEA_EXT => cea_for_each_detailed_block(ext, cb),
            VTB_EXT => vtb_for_each_detailed_block(ext, cb),
            _ => {}
        }
    }
    drm_edid_iter_end(&mut edid_iter);
}

/// `isspace` (lib/ctype.c): space, \t \n \v \f \r.
fn isspace(c: u8) -> bool {
    c == b' ' || (0x09..=0x0d).contains(&c)
}

/// `match_identity`.
fn match_identity(timing: &[u8], ident: &drm_edid_ident, matched: &mut bool) {
    let name = ident.name.unwrap();
    let name_len = name.len();
    let desc = &timing[DT_DATA..DT_DATA + DT_STR_LEN];
    let desc_len = DT_STR_LEN;

    if name_len > desc_len
        || !(is_display_descriptor(timing, EDID_DETAIL_MONITOR_NAME as u8)
            || is_display_descriptor(timing, EDID_DETAIL_MONITOR_STRING as u8))
    {
        return;
    }

    if desc[..name_len] != *name {
        return;
    }

    for &c in &desc[name_len..desc_len] {
        if c == b'\n' {
            break;
        }
        // Allow white space before EDID string terminator.
        if !isspace(c) {
            return;
        }
    }

    *matched = true;
}

/// `drm_edid_match`.
pub fn drm_edid_match(drm_edid: Option<&drm_edid>, ident: &drm_edid_ident) -> bool {
    let Some(d) = drm_edid else { return false };
    if drm_edid_get_panel_id(d) != ident.panel_id {
        return false;
    }

    // Match with name only if it's not NULL.
    if ident.name.is_some() {
        let mut matched = false;

        drm_for_each_detailed_block(drm_edid, &mut |t| match_identity(t, ident, &mut matched));

        return matched;
    }

    true
}

/// `monitor_name` + `get_monitor_name`: up to 13 bytes of the name.
fn get_monitor_name(drm_edid: Option<&drm_edid>, name: &mut [u8]) -> usize {
    let mut edid_name: Option<[u8; DT_STR_LEN]> = None;

    if drm_edid.is_none() {
        return 0;
    }

    drm_for_each_detailed_block(drm_edid, &mut |timing| {
        if !is_display_descriptor(timing, EDID_DETAIL_MONITOR_NAME as u8) {
            return;
        }

        edid_name = Some(timing[DT_DATA..DT_DATA + DT_STR_LEN].try_into().unwrap());
    });
    let mut mnl = 0;
    if let Some(edid_name) = edid_name {
        while mnl < 13 {
            if edid_name[mnl] == 0x0a {
                break;
            }

            name[mnl] = edid_name[mnl];
            mnl += 1;
        }
    }

    mnl
}

/// `drm_edid_get_monitor_name`: NUL-terminated in `name`.
pub fn drm_edid_get_monitor_name(edid: Option<&[u8]>, name: &mut [u8]) {
    let bufsize = name.len();
    let mut name_length = 0;

    if bufsize == 0 {
        return;
    }

    if let Some(edid) = edid {
        let mut buf = [0u8; 13];
        let d = drm_edid { edid: edid.to_vec(), size: edid_size(edid) };

        name_length = get_monitor_name(Some(&d), &mut buf).min(bufsize - 1);
        name[..name_length].copy_from_slice(&buf[..name_length]);
    }

    name[name_length] = 0;
}

// ── quirks ──

/// `enum drm_edid_internal_quirk`, after `DRM_EDID_QUIRK_NUM`.
const EDID_QUIRK_PREFER_LARGE_60: u32 = LD_DRM_EDID_QUIRK_NUM;
const EDID_QUIRK_135_CLOCK_TOO_HIGH: u32 = LD_DRM_EDID_QUIRK_NUM + 1;
const EDID_QUIRK_PREFER_LARGE_75: u32 = LD_DRM_EDID_QUIRK_NUM + 2;
const EDID_QUIRK_DETAILED_IN_CM: u32 = LD_DRM_EDID_QUIRK_NUM + 3;
const EDID_QUIRK_DETAILED_USE_MAXIMUM_SIZE: u32 = LD_DRM_EDID_QUIRK_NUM + 4;
const EDID_QUIRK_DETAILED_SYNC_PP: u32 = LD_DRM_EDID_QUIRK_NUM + 5;
const EDID_QUIRK_FORCE_REDUCED_BLANKING: u32 = LD_DRM_EDID_QUIRK_NUM + 6;
const EDID_QUIRK_FORCE_8BPC: u32 = LD_DRM_EDID_QUIRK_NUM + 7;
const EDID_QUIRK_FORCE_12BPC: u32 = LD_DRM_EDID_QUIRK_NUM + 8;
const EDID_QUIRK_FORCE_6BPC: u32 = LD_DRM_EDID_QUIRK_NUM + 9;
const EDID_QUIRK_FORCE_10BPC: u32 = LD_DRM_EDID_QUIRK_NUM + 10;
const EDID_QUIRK_NON_DESKTOP: u32 = LD_DRM_EDID_QUIRK_NUM + 11;
const EDID_QUIRK_CAP_DSC_15BPP: u32 = LD_DRM_EDID_QUIRK_NUM + 12;

/// `edid_get_quirks`, over Linux's `edid_quirk_list` (tables.rs; every
/// entry matches by panel id alone: { panel_id, name = NULL, quirks }).
fn edid_get_quirks(drm_edid: &drm_edid) -> u32 {
    for q in edid_quirk_list() {
        if drm_edid_match(Some(drm_edid), &drm_edid_ident { panel_id: q.0, name: None }) {
            return q.1;
        }
    }

    0
}

/// `edid_quirk_list` as (panel_id, quirks): the static as GCC lays it
/// out, a u32 panel id, the name pointer, then the quirks.
fn edid_quirk_list() -> impl Iterator<Item = (u32, u32)> {
    let t = &crate::tables::edid_quirk_list;
    t.chunks(EDID_QUIRK_WORDS).map(|e| (e[0], e[EDID_QUIRK_WORDS - 2]))
}

/// The words of one `struct edid_quirk` (24 bytes).
const EDID_QUIRK_WORDS: usize = 6;

/// `drm_edid_has_internal_quirk`.
fn drm_edid_has_internal_quirk(connector: &drm_connector, quirk: u32) -> bool {
    connector.display_info.quirks & (1 << quirk) != 0
}

// ── CTA data blocks ──

/// `cea_revision`.
fn cea_revision(cea: &[u8]) -> u8 {
    // FIXME is this correct for the DispID variant?
    // The DispID spec doesn't really specify whether
    // this is the revision of the CEA extension or
    // the DispID CEA data block. And the only value
    // given as an example is 0.
    cea[1]
}

/// `cea_db_tag`.
fn cea_db_tag(db: &[u8]) -> u32 {
    (db[0] >> 5) as u32
}

/// `cea_db_payload_len`.
fn cea_db_payload_len(db: &[u8]) -> usize {
    (db[0] & 0x1f) as usize
}

/// `cea_db_data`: the payload offset in the block.
const CEA_DB_DATA: usize = 1;

/// `oui`.
fn oui(first: u8, second: u8, third: u8) -> u32 {
    (first as u32) << 16 | (second as u32) << 8 | third as u32
}

/// `cea_db_is_extended_tag`.
fn cea_db_is_extended_tag(db: &[u8], tag: u32) -> bool {
    cea_db_tag(db) == CTA_DB_EXTENDED_TAG && cea_db_payload_len(db) >= 1 && db[1] as u32 == tag
}

/// `cea_db_is_vendor`.
fn cea_db_is_vendor(db: &[u8], vendor_oui: u32) -> bool {
    let data = &db[CEA_DB_DATA..];

    cea_db_tag(db) == CTA_DB_VENDOR && cea_db_payload_len(db) >= 3 && oui(data[2], data[1], data[0]) == vendor_oui
}

/// `cea_db_is_hdmi_vsdb`.
fn cea_db_is_hdmi_vsdb(db: &[u8]) -> bool {
    cea_db_is_vendor(db, HDMI_IEEE_OUI) && cea_db_payload_len(db) >= 5
}

/// `cea_db_is_hdmi_forum_vsdb`.
fn cea_db_is_hdmi_forum_vsdb(db: &[u8]) -> bool {
    cea_db_is_vendor(db, HDMI_FORUM_IEEE_OUI) && cea_db_payload_len(db) >= 7
}

/// `cea_db_is_hdmi_forum_eeodb`.
fn cea_db_is_hdmi_forum_eeodb(db: &[u8]) -> bool {
    cea_db_is_extended_tag(db, CTA_EXT_DB_HF_EEODB) && cea_db_payload_len(db) >= 2
}

/// `cea_db_is_microsoft_vsdb`.
fn cea_db_is_microsoft_vsdb(db: &[u8]) -> bool {
    cea_db_is_vendor(db, LD_MICROSOFT_IEEE_OUI) && cea_db_payload_len(db) == 21
}

/// `cea_db_is_vcdb`.
fn cea_db_is_vcdb(db: &[u8]) -> bool {
    cea_db_is_extended_tag(db, CTA_EXT_DB_VIDEO_CAP) && cea_db_payload_len(db) == 2
}

/// `cea_db_is_hdmi_forum_scdb`.
fn cea_db_is_hdmi_forum_scdb(db: &[u8]) -> bool {
    cea_db_is_extended_tag(db, CTA_EXT_DB_HF_SCDB) && cea_db_payload_len(db) >= 7
}

/// `cea_db_is_y420cmdb`.
fn cea_db_is_y420cmdb(db: &[u8]) -> bool {
    cea_db_is_extended_tag(db, CTA_EXT_DB_420_VIDEO_CAP_MAP)
}

/// `cea_db_is_y420vdb`.
fn cea_db_is_y420vdb(db: &[u8]) -> bool {
    cea_db_is_extended_tag(db, CTA_EXT_DB_420_VIDEO_DATA)
}

/// `cea_db_is_hdmi_hdr_metadata_block`.
fn cea_db_is_hdmi_hdr_metadata_block(db: &[u8]) -> bool {
    cea_db_is_extended_tag(db, CTA_EXT_DB_HDR_STATIC_METADATA) && cea_db_payload_len(db) >= 3
}

/// `cea_db_collection_size`.
fn cea_db_collection_size(cta: &[u8]) -> usize {
    let d = cta[2] as usize;

    if !(4..=127).contains(&d) {
        return 0;
    }

    d - 4
}

/// `struct cea_db_iter`: the current collection is an offset into the EDID.
struct cea_db_iter<'a> {
    edid: &'a [u8],
    edid_iter: drm_edid_iter<'a>,
    displayid_iter: displayid_iter<'a>,
    // Current Data Block Collection.
    collection: Option<usize>,
    // Current Data Block index in current collection.
    index: usize,
    // End index in current collection.
    end: usize,
}

/// `cea_db_iter_edid_begin`.
fn cea_db_iter_edid_begin(drm_edid: Option<&drm_edid>) -> cea_db_iter<'_> {
    cea_db_iter {
        edid: drm_edid.map_or(&[][..], |d| &d.edid[..]),
        edid_iter: drm_edid_iter_begin(drm_edid),
        displayid_iter: displayid_iter_edid_begin(drm_edid),
        collection: None,
        index: 0,
        end: 0,
    }
}

/// `__cea_db_iter_current_block`: the block's offset.
fn __cea_db_iter_current_block(iter: &cea_db_iter) -> Option<usize> {
    let c = iter.collection?;

    let db = c + iter.index;

    if iter.index + 1 <= iter.end && iter.index + 1 + cea_db_payload_len(&iter.edid[db..]) <= iter.end {
        return Some(db);
    }

    None
}

/// `__cea_db_iter_edid_next`: the next CTA extension, revision 3 and up.
fn __cea_db_iter_edid_next(iter: &mut cea_db_iter) -> Option<usize> {
    while let Some(o) = __drm_edid_iter_next(&mut iter.edid_iter) {
        let ext = &iter.edid[o..o + EDID_LENGTH];

        // Only support CTA Extension revision 3+
        if ext[0] as u32 != CEA_EXT || cea_revision(ext) < 3 {
            continue;
        }

        let size = cea_db_collection_size(ext);
        if size == 0 {
            continue;
        }

        iter.index = 4;
        iter.end = iter.index + size;

        return Some(o);
    }

    None
}

/// `__cea_db_iter_displayid_next`: the next DisplayID CTA block.
fn __cea_db_iter_displayid_next(iter: &mut cea_db_iter) -> Option<usize> {
    while let Some(block) = __displayid_iter_next(&mut iter.displayid_iter) {
        if iter.edid[block] as u32 != DATA_BLOCK_CTA {
            continue;
        }

        // The displayid iterator has already verified the block bounds
        // in displayid_iter_block().
        iter.index = DISPLAYID_BLOCK_SIZE;
        iter.end = iter.index + iter.edid[block + 2] as usize;

        return Some(block);
    }

    None
}

/// `__cea_db_iter_next`.
fn __cea_db_iter_next(iter: &mut cea_db_iter) -> Option<usize> {
    if iter.collection.is_some() {
        // Current collection should always be valid.
        let Some(db) = __cea_db_iter_current_block(iter) else {
            iter.collection = None;
            return None;
        };

        // Next block in CTA Data Block Collection
        iter.index += 1 + cea_db_payload_len(&iter.edid[db..]);

        if let Some(db) = __cea_db_iter_current_block(iter) {
            return Some(db);
        }
    }

    loop {
        // Find the next CTA Data Block Collection. First iterate all
        // the EDID CTA Extensions, then all the DisplayID CTA blocks.
        //
        // Per DisplayID v1.3 Appendix B: DisplayID as an EDID
        // Extension, it's recommended that DisplayID extensions are
        // exposed after all of the CTA Extensions.
        iter.collection = __cea_db_iter_edid_next(iter);
        if iter.collection.is_none() {
            iter.collection = __cea_db_iter_displayid_next(iter);
        }

        iter.collection?;

        if let Some(db) = __cea_db_iter_current_block(iter) {
            return Some(db);
        }
    }
}

/// `cea_db_iter_end`.
fn cea_db_iter_end(iter: &mut cea_db_iter) {
    displayid_iter_end(&mut iter.displayid_iter);
    drm_edid_iter_end(&mut iter.edid_iter);
    iter.collection = None;
    iter.index = 0;
    iter.end = 0;
}

/// `struct cea_sad` (drm_edid.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct cea_sad {
    pub format: u8,
    pub channels: u8,
    pub freq: u8,
    pub byte2: u8,
}

/// `drm_edid_cta_sad_set`.
fn drm_edid_cta_sad_set(sad: &[u8]) -> cea_sad {
    cea_sad { format: (sad[0] & 0x78) >> 3, channels: sad[0] & 0x07, freq: sad[1] & 0x7f, byte2: sad[2] }
}

/// `_drm_edid_to_sad`: the SADs of the first Audio Data Block.
fn _drm_edid_to_sad(drm_edid: Option<&drm_edid>, psads: &mut Vec<cea_sad>) -> i32 {
    let mut count = 0;

    let mut iter = cea_db_iter_edid_begin(drm_edid);
    while let Some(db) = __cea_db_iter_next(&mut iter) {
        let db = &iter.edid[db..];
        if cea_db_tag(db) == CTA_DB_AUDIO {
            count = cea_db_payload_len(db) / 3; // SAD is 3B
            *psads = (0..count).map(|i| drm_edid_cta_sad_set(&db[1 + i * 3..])).collect();
            break;
        }
    }
    cea_db_iter_end(&mut iter);

    count as i32
}

/// `drm_edid_to_sad`.
pub fn drm_edid_to_sad(edid: &[u8], sads: &mut Vec<cea_sad>) -> i32 {
    let d = drm_edid_legacy_init(edid);
    _drm_edid_to_sad(Some(&d), sads)
}

/// `_drm_edid_to_speaker_allocation`: the Speaker Allocation Data Block.
fn _drm_edid_to_speaker_allocation(drm_edid: Option<&drm_edid>, sadb: &mut Vec<u8>) -> i32 {
    let mut count = 0;

    let mut iter = cea_db_iter_edid_begin(drm_edid);
    while let Some(db) = __cea_db_iter_next(&mut iter) {
        let db = &iter.edid[db..];
        if cea_db_tag(db) == CTA_DB_SPEAKER && cea_db_payload_len(db) == 3 {
            *sadb = db[1..1 + cea_db_payload_len(db)].to_vec();
            count = cea_db_payload_len(db);
            break;
        }
    }
    cea_db_iter_end(&mut iter);

    count as i32
}

/// `drm_edid_to_speaker_allocation`.
pub fn drm_edid_to_speaker_allocation(edid: &[u8], sadb: &mut Vec<u8>) -> i32 {
    let d = drm_edid_legacy_init(edid);
    _drm_edid_to_speaker_allocation(Some(&d), sadb)
}

/// `drm_edid_find_extension`: the next extension block tagged `ext_id`.
pub fn drm_edid_find_extension(drm_edid: Option<&drm_edid>, ext_id: u32, ext_index: &mut isize) -> Option<usize> {
    let d = drm_edid?;
    // No EDID or EDID extensions
    if drm_edid_extension_block_count(d) <= 0 {
        return None;
    }

    // Find CEA extension
    let mut i = *ext_index;
    while i < drm_edid_extension_block_count(d) {
        let edid_ext = edid_extension_block_data(i as usize);
        if edid_block_tag(&d.edid[edid_ext..]) as u32 == ext_id {
            break;
        }
        i += 1;
    }

    if i >= drm_edid_extension_block_count(d) {
        return None;
    }

    *ext_index = i + 1;

    Some(edid_extension_block_data(i as usize))
}

// ── DisplayID (drm_displayid.c) ──

/// `struct displayid_header` (4 bytes: rev, bytes, prod_id, ext_count) and
/// `struct displayid_block` (3 bytes: tag, rev, num_bytes).
const DISPLAYID_HEADER_SIZE: usize = 4;
const DISPLAYID_BLOCK_SIZE: usize = 3;

/// `struct displayid_iter`: section and blocks as offsets into the EDID.
pub struct displayid_iter<'a> {
    drm_edid: Option<&'a drm_edid>,
    section: Option<usize>,
    length: usize,
    idx: usize,
    ext_index: isize,
    version: u8,
    primary_use: u8,
    quirks: u8,
}

/// `QUIRK_IGNORE_CHECKSUM` and the one quirk of drm_displayid.c.
const QUIRK_IGNORE_CHECKSUM: u8 = 0;
const DISPLAYID_QUIRKS: [(drm_edid_ident, u8); 1] = [(
    drm_edid_ident { panel_id: drm_edid_encode_panel_id(b'C', b'S', b'O', 5142), name: Some(b"MNE007ZA1-5") },
    1 << QUIRK_IGNORE_CHECKSUM,
)];

/// `get_quirks` (drm_displayid.c).
fn get_quirks(drm_edid: Option<&drm_edid>) -> u8 {
    for (ident, quirks) in &DISPLAYID_QUIRKS {
        if drm_edid_match(drm_edid, ident) {
            return *quirks;
        }
    }

    0
}

/// `displayid_get_header`: the header's offset in the section, if it fits.
fn displayid_get_header(length: usize, index: usize) -> Option<usize> {
    if DISPLAYID_HEADER_SIZE as isize > length as isize - index as isize {
        return None;
    }

    Some(index)
}

/// `validate_displayid`.
fn validate_displayid(displayid: &[u8], length: usize, idx: usize, ignore_checksum: bool) -> Option<usize> {
    let base = displayid_get_header(length, idx)?;

    // +1 for DispID checksum
    let dispid_length = DISPLAYID_HEADER_SIZE + displayid[base + 1] as usize + 1;
    if dispid_length as isize > length as isize - idx as isize {
        return None;
    }

    let mut csum: u8 = 0;
    for i in 0..dispid_length {
        csum = csum.wrapping_add(displayid[idx + i]);
    }
    if csum != 0 {
        Line::new().s("DisplayID checksum invalid, remainder is ").dec(csum as u64)
            .s(if ignore_checksum { " (ignoring)" } else { "" }).say();

        if !ignore_checksum {
            return None;
        }
    }

    Some(base)
}

/// `find_next_displayid_extension`.
fn find_next_displayid_extension(iter: &mut displayid_iter) -> Option<usize> {
    let ignore_checksum = iter.quirks & (1 << QUIRK_IGNORE_CHECKSUM) != 0;

    let displayid = drm_edid_find_extension(iter.drm_edid, DISPLAYID_EXT, &mut iter.ext_index)?;
    let ext = &iter.drm_edid.unwrap().edid[displayid..displayid + EDID_LENGTH];

    // EDID extensions block checksum isn't for us
    iter.length = EDID_LENGTH - 1;
    iter.idx = 1;

    let base = validate_displayid(ext, iter.length, iter.idx, ignore_checksum)?;

    iter.length = iter.idx + DISPLAYID_HEADER_SIZE + ext[base + 1] as usize;

    Some(displayid)
}

/// `displayid_iter_edid_begin`.
pub fn displayid_iter_edid_begin(drm_edid: Option<&drm_edid>) -> displayid_iter<'_> {
    displayid_iter {
        drm_edid,
        section: None,
        length: 0,
        idx: 0,
        ext_index: 0,
        version: 0,
        primary_use: 0,
        quirks: get_quirks(drm_edid),
    }
}

/// `displayid_iter_block`: the block's offset in the EDID.
fn displayid_iter_block(iter: &displayid_iter) -> Option<usize> {
    let s = iter.section?;
    let edid = &iter.drm_edid?.edid;

    let block = s + iter.idx;

    if iter.idx + DISPLAYID_BLOCK_SIZE <= iter.length
        && iter.idx + DISPLAYID_BLOCK_SIZE + edid[block + 2] as usize <= iter.length
    {
        return Some(block);
    }

    None
}

/// `__displayid_iter_next`.
pub fn __displayid_iter_next(iter: &mut displayid_iter) -> Option<usize> {
    let edid = &iter.drm_edid?.edid;

    if iter.section.is_some() {
        // current block should always be valid
        let Some(block) = displayid_iter_block(iter) else {
            iter.section = None;
            iter.drm_edid = None;
            return None;
        };

        // next block in section
        iter.idx += DISPLAYID_BLOCK_SIZE + edid[block + 2] as usize;

        if let Some(block) = displayid_iter_block(iter) {
            return Some(block);
        }
    }

    loop {
        // The first section we encounter is the base section
        let base_section = iter.section.is_none();

        iter.section = find_next_displayid_extension(iter);
        let Some(section) = iter.section else {
            iter.drm_edid = None;
            return None;
        };

        // Save the structure version and primary use case.
        if base_section {
            if let Some(base) = displayid_get_header(iter.length, iter.idx) {
                iter.version = edid[section + base];
                iter.primary_use = edid[section + base + 2];
            }
        }

        iter.idx += DISPLAYID_HEADER_SIZE;

        if let Some(block) = displayid_iter_block(iter) {
            return Some(block);
        }
    }
}

/// `displayid_iter_end`.
pub fn displayid_iter_end(iter: &mut displayid_iter) {
    iter.drm_edid = None;
    iter.section = None;
    iter.length = 0;
    iter.idx = 0;
    iter.ext_index = 0;
    iter.version = 0;
    iter.primary_use = 0;
    iter.quirks = 0;
}

/// `displayid_version`.
fn displayid_version(iter: &displayid_iter) -> u8 {
    iter.version
}

/// `displayid_primary_use`.
fn displayid_primary_use(iter: &displayid_iter) -> u8 {
    iter.primary_use
}

// ── display info ──

/// `drm_reset_display_info`.
fn drm_reset_display_info(connector: &mut drm_connector) {
    let info = &mut connector.display_info;

    info.width_mm = 0;
    info.height_mm = 0;

    info.bpc = 0;
    info.color_formats = 0;
    info.cea_rev = 0;
    info.max_tmds_clock = 0;
    info.dvi_dual = false;
    info.is_hdmi = false;
    info.has_audio = false;
    info.has_hdmi_infoframe = false;
    info.rgb_quant_range_selectable = false;
    info.hdmi = drm_hdmi_info::default();
    info.hdr_sink_metadata = hdr_sink_metadata::default();

    info.edid_hdmi_rgb444_dc_modes = 0;
    info.edid_hdmi_ycbcr444_dc_modes = 0;

    info.non_desktop = false;
    info.monitor_range = drm_monitor_range_info::default();
    info.luminance_range = drm_luminance_range_info::default();

    info.mso_stream_count = 0;
    info.mso_pixel_overlap = 0;
    info.max_dsc_bpp = 0;

    info.vics = Vec::new();

    info.quirks = 0;

    info.source_physical_address = LD_CEC_PHYS_ADDR_INVALID as u16;
}

/// `clear_eld`.
fn clear_eld(connector: &mut drm_connector) {
    connector.eld = [0; MAX_ELD_BYTES];

    connector.latency_present[0] = false;
    connector.latency_present[1] = false;
    connector.video_latency[0] = 0;
    connector.audio_latency[0] = 0;
    connector.video_latency[1] = 0;
    connector.audio_latency[1] = 0;
}

/// `get_monitor_range` + `drm_get_monitor_range`: the VRR refresh range of
/// a "range limits only" descriptor, on an EDID 1.4 with continuous
/// frequencies.
fn drm_get_monitor_range(connector: &mut drm_connector, drm_edid: &drm_edid) {
    if drm_edid.edid[EDID_REVISION] < 4 {
        return;
    }

    if drm_edid.edid[EDID_FEATURES] as u32 & DRM_EDID_FEATURE_CONTINUOUS_FREQ == 0 {
        return;
    }

    let revision = drm_edid.edid[EDID_REVISION];
    let monitor_range = &mut connector.display_info.monitor_range;
    drm_for_each_detailed_block(Some(drm_edid), &mut |timing| {
        if !is_display_descriptor(timing, EDID_DETAIL_MONITOR_RANGE as u8) {
            return;
        }

        // These limits are used to determine the VRR refresh
        // rate range. Only the "range limits only" variant
        // of the range descriptor seems to guarantee that
        // any and all timings are accepted by the sink, as
        // opposed to just timings conforming to the indicated
        // formula (GTF/GTF2/CVT). Thus other variants of the
        // range descriptor are not accepted here.
        if timing[DT_RANGE_FLAGS] as u32 != DRM_EDID_RANGE_LIMITS_ONLY_FLAG {
            return;
        }

        monitor_range.min_vfreq = timing[DT_RANGE_MIN_VFREQ] as u16;
        monitor_range.max_vfreq = timing[DT_RANGE_MAX_VFREQ] as u16;

        if revision >= 4 {
            if timing[DT_PAD2] as u32 & DRM_EDID_RANGE_OFFSET_MIN_VFREQ != 0 {
                monitor_range.min_vfreq += 255;
            }
            if timing[DT_PAD2] as u32 & DRM_EDID_RANGE_OFFSET_MAX_VFREQ != 0 {
                monitor_range.max_vfreq += 255;
            }
        }
    });
}

/// `drm_edid_is_digital`.
pub fn drm_edid_is_digital(drm_edid: Option<&drm_edid>) -> bool {
    drm_edid.is_some_and(|d| d.edid[EDID_INPUT] as u32 & DRM_EDID_INPUT_DIGITAL != 0)
}

/// A parser of a block this EDID has, not yet ported.
fn not_ported(name: &str) {
    Line::new().s("drm_edid: ").s(name).s(" not ported").say();
}

/// `drm_parse_cea_ext`.
fn drm_parse_cea_ext(connector: &mut drm_connector, drm_edid: &drm_edid) {
    let info = &mut connector.display_info;
    let y420cmdb_map: u64 = 0;

    let mut edid_iter = drm_edid_iter_begin(Some(drm_edid));
    while let Some(o) = __drm_edid_iter_next(&mut edid_iter) {
        let edid_ext = &drm_edid.edid[o..o + EDID_LENGTH];
        if edid_ext[0] as u32 != CEA_EXT {
            continue;
        }

        if info.cea_rev == 0 {
            info.cea_rev = edid_ext[1];
        }

        // The existence of a CTA extension should imply RGB support
        info.color_formats = DRM_COLOR_FORMAT_RGB444;
        if edid_ext[3] as u32 & EDID_CEA_YCRCB444 != 0 {
            info.color_formats |= DRM_COLOR_FORMAT_YCBCR444;
        }
        if edid_ext[3] as u32 & EDID_CEA_YCRCB422 != 0 {
            info.color_formats |= DRM_COLOR_FORMAT_YCBCR422;
        }
        if edid_ext[3] as u32 & EDID_BASIC_AUDIO != 0 {
            info.has_audio = true;
        }
    }
    drm_edid_iter_end(&mut edid_iter);

    let mut iter = cea_db_iter_edid_begin(Some(drm_edid));
    while let Some(db) = __cea_db_iter_next(&mut iter) {
        let db = &iter.edid[db..];

        if cea_db_is_hdmi_vsdb(db) {
            not_ported("drm_parse_hdmi_vsdb_video");
        } else if cea_db_is_hdmi_forum_vsdb(db) || cea_db_is_hdmi_forum_scdb(db) {
            not_ported("drm_parse_hdmi_forum_scds");
        } else if cea_db_is_microsoft_vsdb(db) {
            not_ported("drm_parse_microsoft_vsdb");
        } else if cea_db_is_y420cmdb(db) {
            not_ported("parse_cta_y420cmdb");
        } else if cea_db_is_y420vdb(db) {
            not_ported("parse_cta_y420vdb");
        } else if cea_db_is_vcdb(db) {
            not_ported("drm_parse_vcdb");
        } else if cea_db_is_hdmi_hdr_metadata_block(db) {
            not_ported("drm_parse_hdr_metadata_block");
        } else if cea_db_tag(db) == CTA_DB_VIDEO {
            not_ported("parse_cta_vdb");
        } else if cea_db_tag(db) == CTA_DB_AUDIO {
            connector.display_info.has_audio = true;
        }
    }
    cea_db_iter_end(&mut iter);

    if y420cmdb_map != 0 {
        not_ported("update_cta_y420cmdb");
    }
}

/// `update_displayid_info`: non-desktop from the base section.
fn update_displayid_info(connector: &mut drm_connector, drm_edid: &drm_edid) {
    let info = &mut connector.display_info;

    let mut iter = displayid_iter_edid_begin(Some(drm_edid));
    if __displayid_iter_next(&mut iter).is_some() {
        if displayid_version(&iter) as u32 == DISPLAY_ID_STRUCTURE_VER_20
            && (displayid_primary_use(&iter) as u32 == PRIMARY_USE_HEAD_MOUNTED_VR
                || displayid_primary_use(&iter) as u32 == PRIMARY_USE_HEAD_MOUNTED_AR)
        {
            info.non_desktop = true;
        }

        // We're only interested in the base section here, no need to
        // iterate further.
    }
    displayid_iter_end(&mut iter);
}

/// `drm_update_mso`.
fn drm_update_mso(_connector: &mut drm_connector, drm_edid: &drm_edid) {
    let mut iter = displayid_iter_edid_begin(Some(drm_edid));
    while let Some(block) = __displayid_iter_next(&mut iter) {
        if drm_edid.edid[block] as u32 == DATA_BLOCK_2_VENDOR_SPECIFIC {
            not_ported("drm_parse_vesa_mso_data");
        }
    }
    displayid_iter_end(&mut iter);
}

/// `update_display_info`.
fn update_display_info(connector: &mut drm_connector, drm_edid: Option<&drm_edid>) {
    drm_reset_display_info(connector);
    clear_eld(connector);

    let Some(drm_edid) = drm_edid else { return };

    let edid = &drm_edid.edid;

    connector.display_info.quirks = edid_get_quirks(drm_edid);

    connector.display_info.width_mm = edid[EDID_WIDTH_CM] as u32 * 10;
    connector.display_info.height_mm = edid[EDID_HEIGHT_CM] as u32 * 10;

    drm_get_monitor_range(connector, drm_edid);

    'out: {
        if edid[EDID_REVISION] < 3 {
            break 'out;
        }

        if !drm_edid_is_digital(Some(drm_edid)) {
            break 'out;
        }

        connector.display_info.color_formats |= DRM_COLOR_FORMAT_RGB444;
        drm_parse_cea_ext(connector, drm_edid);

        update_displayid_info(connector, drm_edid);

        let info = &mut connector.display_info;
        // Digital sink with "DFP 1.x compliant TMDS" according to EDID 1.3?
        //
        // For such displays, the DFP spec 1.0, section 3.10 "EDID support"
        // tells us to assume 8 bpc color depth if the EDID doesn't have
        // extensions which tell otherwise.
        if info.bpc == 0 && edid[EDID_REVISION] == 3 && edid[EDID_INPUT] as u32 & DRM_EDID_DIGITAL_DFP_1_X != 0 {
            info.bpc = 8;
        }

        // Only defined for 1.4 with digital displays
        if edid[EDID_REVISION] < 4 {
            break 'out;
        }

        info.bpc = match edid[EDID_INPUT] as u32 & DRM_EDID_DIGITAL_DEPTH_MASK {
            DRM_EDID_DIGITAL_DEPTH_6 => 6,
            DRM_EDID_DIGITAL_DEPTH_8 => 8,
            DRM_EDID_DIGITAL_DEPTH_10 => 10,
            DRM_EDID_DIGITAL_DEPTH_12 => 12,
            DRM_EDID_DIGITAL_DEPTH_14 => 14,
            DRM_EDID_DIGITAL_DEPTH_16 => 16,
            _ => 0,
        };

        if edid[EDID_FEATURES] as u32 & DRM_EDID_FEATURE_RGB_YCRCB444 != 0 {
            info.color_formats |= DRM_COLOR_FORMAT_YCBCR444;
        }
        if edid[EDID_FEATURES] as u32 & DRM_EDID_FEATURE_RGB_YCRCB422 != 0 {
            info.color_formats |= DRM_COLOR_FORMAT_YCBCR422;
        }

        drm_update_mso(connector, drm_edid);
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_NON_DESKTOP) {
        connector.display_info.non_desktop = true;
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_CAP_DSC_15BPP) {
        connector.display_info.max_dsc_bpp = 15;
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_FORCE_6BPC) {
        connector.display_info.bpc = 6;
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_FORCE_8BPC) {
        connector.display_info.bpc = 8;
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_FORCE_10BPC) {
        connector.display_info.bpc = 10;
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_FORCE_12BPC) {
        connector.display_info.bpc = 12;
    }

    // Depends on info->cea_rev set by drm_parse_cea_ext() above
    drm_edid_to_eld(connector, Some(drm_edid));
}

/// `drm_eld_mnl` / `drm_eld_sad_count` / `drm_eld_calc_baseline_block_size`
/// (drm_eld.h).
fn drm_eld_mnl(eld: &[u8]) -> usize {
    ((eld[DRM_ELD_CEA_EDID_VER_MNL as usize] as u32 & DRM_ELD_MNL_MASK) >> DRM_ELD_MNL_SHIFT) as usize
}

fn drm_eld_sad_count(eld: &[u8]) -> usize {
    ((eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] as u32 & DRM_ELD_SAD_COUNT_MASK) >> DRM_ELD_SAD_COUNT_SHIFT) as usize
}

fn drm_eld_calc_baseline_block_size(eld: &[u8]) -> usize {
    DRM_ELD_MONITOR_NAME_STRING as usize - DRM_ELD_HEADER_BLOCK_SIZE as usize + drm_eld_mnl(eld)
        + drm_eld_sad_count(eld) * 3
}

/// `drm_edid_to_eld`: the ELD the connector carries for audio.
fn drm_edid_to_eld(connector: &mut drm_connector, drm_edid: Option<&drm_edid>) {
    let Some(d) = drm_edid else { return };
    let cea_rev = connector.display_info.cea_rev;
    let connector_type = connector.connector_type;
    let eld = &mut connector.eld;
    let mut total_sad_count = 0;

    let mnl = get_monitor_name(drm_edid, &mut eld[DRM_ELD_MONITOR_NAME_STRING as usize..]);

    eld[DRM_ELD_CEA_EDID_VER_MNL as usize] = (cea_rev as u32) .wrapping_shl(DRM_ELD_CEA_EDID_VER_SHIFT) as u8;
    eld[DRM_ELD_CEA_EDID_VER_MNL as usize] |= mnl as u8;

    eld[DRM_ELD_VER as usize] = DRM_ELD_VER_CEA861D as u8;

    eld[DRM_ELD_MANUFACTURER_NAME0 as usize] = d.edid[EDID_MFG_ID];
    eld[DRM_ELD_MANUFACTURER_NAME1 as usize] = d.edid[EDID_MFG_ID + 1];
    eld[DRM_ELD_PRODUCT_CODE0 as usize] = d.edid[EDID_PROD_CODE];
    eld[DRM_ELD_PRODUCT_CODE1 as usize] = d.edid[EDID_PROD_CODE + 1];

    let mut iter = cea_db_iter_edid_begin(drm_edid);
    while let Some(db) = __cea_db_iter_next(&mut iter) {
        let db = &iter.edid[db..];
        let data = &db[CEA_DB_DATA..];
        let len = cea_db_payload_len(db);

        match cea_db_tag(db) {
            CTA_DB_AUDIO => {
                // Audio Data Block, contains SADs
                let sad_count = (len / 3).min(15 - total_sad_count);
                if sad_count >= 1 {
                    let o = DRM_ELD_CEA_SAD(mnl, total_sad_count);
                    eld[o..o + sad_count * 3].copy_from_slice(&data[..sad_count * 3]);
                }
                total_sad_count += sad_count;
            }
            CTA_DB_SPEAKER => {
                // Speaker Allocation Data Block
                if len >= 1 {
                    eld[DRM_ELD_SPEAKER as usize] = data[0];
                }
            }
            CTA_DB_VENDOR => {
                // HDMI Vendor-Specific Data Block
                if cea_db_is_hdmi_vsdb(db) {
                    not_ported("drm_parse_hdmi_vsdb_audio");
                }
            }
            _ => {}
        }
    }
    cea_db_iter_end(&mut iter);

    eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |= (total_sad_count << DRM_ELD_SAD_COUNT_SHIFT) as u8;

    if connector_type == DRM_MODE_CONNECTOR_DisplayPort || connector_type == DRM_MODE_CONNECTOR_eDP {
        eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |= DRM_ELD_CONN_TYPE_DP as u8;
    } else {
        eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |= DRM_ELD_CONN_TYPE_HDMI as u8;
    }

    eld[DRM_ELD_BASELINE_ELD_LEN as usize] = drm_eld_calc_baseline_block_size(&eld[..]).div_ceil(4) as u8;
}

/// `DRM_ELD_CEA_SAD` (drm_eld.h).
#[allow(non_snake_case)]
fn DRM_ELD_CEA_SAD(mnl: usize, sad: usize) -> usize {
    20 + mnl + 3 * sad
}

/// `displayid_is_tiled_block`.
fn displayid_is_tiled_block(iter: &displayid_iter, tag: u32) -> bool {
    ((displayid_version(iter) as u32) < DISPLAY_ID_STRUCTURE_VER_20 && tag == DATA_BLOCK_TILED_DISPLAY)
        || (displayid_version(iter) as u32 == DISPLAY_ID_STRUCTURE_VER_20 && tag == DATA_BLOCK_2_TILED_DISPLAY_TOPOLOGY)
}

/// `_drm_update_tile_info`.
fn _drm_update_tile_info(connector: &mut drm_connector, drm_edid: Option<&drm_edid>) {
    connector.has_tile = false;

    let mut iter = displayid_iter_edid_begin(drm_edid);
    while let Some(block) = __displayid_iter_next(&mut iter) {
        let edid = &drm_edid.unwrap().edid;
        if displayid_is_tiled_block(&iter, edid[block] as u32) {
            not_ported("drm_parse_tiled_block");
        }
    }
    displayid_iter_end(&mut iter);

    if !connector.has_tile && connector.tile_group.is_some() {
        // drm_mode_put_tile_group: the connector's reference only.
        connector.tile_group = None;
    }
}

/// `_drm_edid_connector_property_update`: the EDID blob (the property's
/// payload) and the epoch it counts; setting the EDID, non-desktop and tile
/// properties is DRM's userspace interface and not a part.
fn _drm_edid_connector_property_update(connector: &mut drm_connector, drm_edid: Option<&drm_edid>) -> i32 {
    if let Some(old_edid) = connector.edid_blob.as_deref() {
        if !drm_edid_eq(drm_edid, Some(old_edid)) {
            connector.epoch_counter += 1;
        }
    }

    // drm_property_replace_global_blob
    connector.edid_blob = drm_edid.filter(|d| d.size != 0).map(|d| d.edid[..d.size].to_vec());

    0
}

/// `drm_edid_connector_update`.
pub fn drm_edid_connector_update(connector: &mut drm_connector, drm_edid: Option<&drm_edid>) -> i32 {
    update_display_info(connector, drm_edid);

    _drm_update_tile_info(connector, drm_edid);

    _drm_edid_connector_property_update(connector, drm_edid)
}
