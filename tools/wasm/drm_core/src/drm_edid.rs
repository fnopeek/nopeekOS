//! drivers/gpu/drm/drm_edid.c + drm_displayid.c (Linux rpi-6.18): reading an
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
use crate::drm_modes::*;
use crate::i2c::{i2c_msg, i2c_transfer_fn, I2C_M_RD};
use crate::hdmi::*;
use crate::linux::{Line, EINVAL, ENXIO};
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
pub fn drm_do_probe_ddc_edid(i2c_transfer: &mut i2c_transfer_fn, buf: &mut [u8], block: u32) -> i32 {
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
        ret = i2c_transfer(&mut msgs[3 - xfers..]);

        if ret == -ENXIO {
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
pub fn drm_probe_ddc(i2c_transfer: &mut i2c_transfer_fn) -> bool {
    let mut out = [0u8; 1];

    drm_do_probe_ddc_edid(i2c_transfer, &mut out, 0) == 0
}

/// `drm_edid_read_custom`.
pub fn drm_edid_read_custom(connector: &mut drm_connector, read_block: &mut read_block_fn) -> Option<drm_edid> {
    let (edid, size) = _drm_do_get_edid(connector, read_block)?;

    _drm_edid_alloc(edid, size)
}

/// `drm_edid_read_ddc`: read over the adapter, after a probe.
pub fn drm_edid_read_ddc(connector: &mut drm_connector, i2c_transfer: &mut i2c_transfer_fn) -> Option<drm_edid> {
    if connector.force == drm_connector_force::DRM_FORCE_OFF {
        return None;
    }

    if connector.force == drm_connector_force::DRM_FORCE_UNSPECIFIED && !drm_probe_ddc(i2c_transfer) {
        return None;
    }

    // Note: Do *not* call connector updates here.
    drm_edid_read_custom(connector, &mut |buf, block| drm_do_probe_ddc_edid(i2c_transfer, buf, block))
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

/// `edid_quirk_list` as (panel_id, quirks) (regs.rs).
fn edid_quirk_list() -> impl Iterator<Item = (u32, u32)> {
    crate::regs::edid_quirk_list.iter().copied()
}

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
    // Raspberry Pi's tree only: an HDMI sink without EDID still takes RGB.
    if connector.rpi_downstream
        && (connector.connector_type == DRM_MODE_CONNECTOR_HDMIA
            || connector.connector_type == DRM_MODE_CONNECTOR_HDMIB)
    {
        info.color_formats = DRM_COLOR_FORMAT_RGB444;
    } else {
        info.color_formats = 0;
    }
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
/// `parse_cta_vdb`.
fn parse_cta_vdb(connector: &mut drm_connector, db: &[u8]) {
    let info = &mut connector.display_info;
    let len = cea_db_payload_len(db);
    let svds = &db[CEA_DB_DATA..];

    if len == 0 {
        return;
    }

    // Gracefully handle multiple VDBs, however unlikely that is
    for &svd in &svds[..len] {
        let mut vic = svd_to_vic(svd);

        if !drm_valid_cea_vic(vic) {
            vic = 0;
        }

        info.vics.push(vic);
    }
}

/// `update_cta_y420cmdb`: the y420cmdb_map, indexed by VIC position in the
/// VDBs, as y420_cmdb_modes indexed by the VICs themselves.
fn update_cta_y420cmdb(connector: &mut drm_connector, y420cmdb_map: u64) {
    let info = &mut connector.display_info;
    let len = info.vics.len().min(64);

    for i in 0..len {
        let vic = info.vics[i];

        if vic != 0 && y420cmdb_map & (1u64 << i) != 0 {
            bitmap_set(&mut info.hdmi.y420_cmdb_modes, vic);
        }
    }
}

/// `cta_vdb_has_vic`.
fn cta_vdb_has_vic(connector: &drm_connector, vic: u8) -> bool {
    vic != 0 && connector.display_info.vics.contains(&vic)
}

/// `parse_cta_y420vdb`: CTA-861-H YCbCr 4:2:0 Video Data Block.
fn parse_cta_y420vdb(connector: &mut drm_connector, db: &[u8]) {
    let info = &mut connector.display_info;
    let svds = &db[CEA_DB_DATA + 1..];

    for &svd in &svds[..cea_db_payload_len(db).saturating_sub(1)] {
        let vic = svd_to_vic(svd);

        if !drm_valid_cea_vic(vic) {
            continue;
        }

        bitmap_set(&mut info.hdmi.y420_vdb_modes, vic);
        info.color_formats |= DRM_COLOR_FORMAT_YCBCR420;
    }
}

/// `drm_parse_vcdb`.
fn drm_parse_vcdb(connector: &mut drm_connector, db: &[u8]) {
    if db[2] as u32 & EDID_CEA_VCDB_QS != 0 {
        connector.display_info.rgb_quant_range_selectable = true;
    }
}

/// `drm_get_max_frl_rate`: (max_lanes, max_rate_per_lane).
fn drm_get_max_frl_rate(max_frl_rate: u8) -> (u8, u8) {
    match max_frl_rate {
        1 => (3, 3),
        2 => (3, 6),
        3 => (4, 6),
        4 => (4, 8),
        5 => (4, 10),
        6 => (4, 12),
        _ => (0, 0),
    }
}

/// `drm_parse_ycbcr420_deep_color_info`.
fn drm_parse_ycbcr420_deep_color_info(connector: &mut drm_connector, db: &[u8]) {
    let dc_mask = (db[7] as u32 & DRM_EDID_YCBCR420_DC_MASK) as u8;
    connector.display_info.hdmi.y420_dc_modes = dc_mask;
}

/// `drm_parse_dsc_info`.
fn drm_parse_dsc_info(hdmi_dsc: &mut drm_hdmi_dsc_cap, hf_scds: &[u8]) {
    hdmi_dsc.v_1p2 = hf_scds[11] as u32 & DRM_EDID_DSC_1P2 != 0;

    if !hdmi_dsc.v_1p2 {
        return;
    }

    hdmi_dsc.native_420 = hf_scds[11] as u32 & DRM_EDID_DSC_NATIVE_420 != 0;
    hdmi_dsc.all_bpp = hf_scds[11] as u32 & DRM_EDID_DSC_ALL_BPP != 0;

    hdmi_dsc.bpc_supported = if hf_scds[11] as u32 & DRM_EDID_DSC_16BPC != 0 {
        16
    } else if hf_scds[11] as u32 & DRM_EDID_DSC_12BPC != 0 {
        12
    } else if hf_scds[11] as u32 & DRM_EDID_DSC_10BPC != 0 {
        10
    } else {
        // Supports min 8 BPC if DSC 1.2 is supported
        8
    };

    if cea_db_payload_len(hf_scds) >= 12 && hf_scds[12] != 0 {
        let dsc_max_frl_rate = ((hf_scds[12] as u32 & DRM_EDID_DSC_MAX_FRL_RATE_MASK) >> 4) as u8;
        (hdmi_dsc.max_lanes, hdmi_dsc.max_frl_rate_per_lane) = drm_get_max_frl_rate(dsc_max_frl_rate);

        let dsc_max_slices = hf_scds[12] as u32 & DRM_EDID_DSC_MAX_SLICES;

        (hdmi_dsc.max_slices, hdmi_dsc.clk_per_slice) = match dsc_max_slices {
            1 => (1, 340),
            2 => (2, 340),
            3 => (4, 340),
            4 => (8, 340),
            5 => (8, 400),
            6 => (12, 400),
            7 => (16, 400),
            _ => (0, 0),
        };
    }

    if cea_db_payload_len(hf_scds) >= 13 && hf_scds[13] != 0 {
        hdmi_dsc.total_chunk_kbytes = (hf_scds[13] as u32 & DRM_EDID_DSC_TOTAL_CHUNK_KBYTES) as u8;
    }
}

/// `drm_parse_hdmi_forum_scds`: Sink Capability Data Structure.
fn drm_parse_hdmi_forum_scds(connector: &mut drm_connector, hf_scds: &[u8]) {
    let info = &mut connector.display_info;

    info.has_hdmi_infoframe = true;

    if hf_scds[6] & 0x80 != 0 {
        info.hdmi.scdc.supported = true;
        if hf_scds[6] & 0x40 != 0 {
            info.hdmi.scdc.read_request = true;
        }
    }

    // All HDMI 2.0 monitors must support scrambling at rates > 340 MHz.
    // And as per the spec, three factors confirm this:
    // * Availability of a HF-VSDB block in EDID (check)
    // * Non zero Max_TMDS_Char_Rate filed in HF-VSDB (let's check)
    // * SCDC support available (let's check)
    // Lets check it out.

    if hf_scds[5] != 0 {
        let scdc = &mut info.hdmi.scdc;

        // max clock is 5000 KHz times block value
        let max_tmds_clock = hf_scds[5] as i32 * 5000;

        if max_tmds_clock > 340000 {
            info.max_tmds_clock = max_tmds_clock;
        }

        if scdc.supported {
            scdc.scrambling.supported = true;

            // Few sinks support scrambling for clocks < 340M
            if hf_scds[6] & 0x8 != 0 {
                scdc.scrambling.low_rates = true;
            }
        }
    }

    if hf_scds[7] != 0 {
        let max_frl_rate = ((hf_scds[7] as u32 & DRM_EDID_MAX_FRL_RATE_MASK) >> 4) as u8;
        (info.hdmi.max_lanes, info.hdmi.max_frl_rate_per_lane) = drm_get_max_frl_rate(max_frl_rate);
    }

    drm_parse_ycbcr420_deep_color_info(connector, hf_scds);

    if cea_db_payload_len(hf_scds) >= 11 && hf_scds[11] != 0 {
        drm_parse_dsc_info(&mut connector.display_info.hdmi.dsc_cap, hf_scds);
    }
}

/// `drm_parse_hdmi_deep_color_info`.
fn drm_parse_hdmi_deep_color_info(connector: &mut drm_connector, hdmi: &[u8]) {
    let info = &mut connector.display_info;
    let mut dc_bpc = 0;

    // HDMI supports at least 8 bpc
    info.bpc = 8;

    if cea_db_payload_len(hdmi) < 6 {
        return;
    }

    if hdmi[6] as u32 & DRM_EDID_HDMI_DC_30 != 0 {
        dc_bpc = 10;
        info.edid_hdmi_rgb444_dc_modes |= DRM_EDID_HDMI_DC_30 as u8;
    }

    if hdmi[6] as u32 & DRM_EDID_HDMI_DC_36 != 0 {
        dc_bpc = 12;
        info.edid_hdmi_rgb444_dc_modes |= DRM_EDID_HDMI_DC_36 as u8;
    }

    if hdmi[6] as u32 & DRM_EDID_HDMI_DC_48 != 0 {
        dc_bpc = 16;
        info.edid_hdmi_rgb444_dc_modes |= DRM_EDID_HDMI_DC_48 as u8;
    }

    if dc_bpc == 0 {
        return;
    }

    info.bpc = dc_bpc;

    // YCRCB444 is optional according to spec.
    if hdmi[6] as u32 & DRM_EDID_HDMI_DC_Y444 != 0 {
        info.edid_hdmi_ycbcr444_dc_modes = info.edid_hdmi_rgb444_dc_modes;
    }

    // Spec says that if any deep color mode is supported at all,
    // then deep color 36 bit must be supported (only logged).
}

/// `drm_parse_hdmi_vsdb_video`: HDMI Vendor-Specific Data Block.
fn drm_parse_hdmi_vsdb_video(connector: &mut drm_connector, db: &[u8]) {
    let info = &mut connector.display_info;
    let len = cea_db_payload_len(db);

    info.is_hdmi = true;

    info.source_physical_address = (db[4] as u16) << 8 | db[5] as u16;

    if len >= 6 {
        info.dvi_dual = db[6] & 1 != 0;
    }
    if len >= 7 {
        info.max_tmds_clock = db[7] as i32 * 5000;
    }

    // Try to infer whether the sink supports HDMI infoframes.
    //
    // HDMI infoframe support was first added in HDMI 1.4. Assume the sink
    // supports infoframes if HDMI_Video_present is set.
    if len >= 8 && db[8] & (1 << 5) != 0 {
        info.has_hdmi_infoframe = true;
    }

    drm_parse_hdmi_deep_color_info(connector, db);
}

/// `drm_parse_microsoft_vsdb`: the EDID extension for head-mounted and
/// specialized monitors.
fn drm_parse_microsoft_vsdb(connector: &mut drm_connector, db: &[u8]) {
    let version = db[4];
    let desktop_usage = db[5] & (1 << 6) != 0;

    // Version 1 and 2 for HMDs, version 3 flags desktop usage explicitly
    if version == 1 || version == 2 || (version == 3 && !desktop_usage) {
        connector.display_info.non_desktop = true;
    }
}

/// `parse_cta_y420cmdb`: the map of VDB positions that also take 4:2:0.
fn parse_cta_y420cmdb(connector: &mut drm_connector, db: &[u8]) -> u64 {
    let mut map_len = cea_db_payload_len(db).saturating_sub(1);
    let data = &db[CEA_DB_DATA + 1..];
    let mut map: u64 = 0;

    if map_len == 0 {
        // All CEA modes support ycbcr420 sampling also.
        map = u64::MAX;
    } else {
        // This map indicates which of the existing CEA block modes
        // from VDB can support YCBCR420 output too. So if bit=0 is
        // set, first mode from VDB can support YCBCR420 output too.
        // We will parse and keep this map, before parsing VDB itself
        // to avoid going through the same block again and again.
        //
        // Spec is not clear about max possible size of this block.
        // Clamping max bitmap block size at 8 bytes. Every byte can
        // address 8 CEA modes, in this way this map can address
        // 8*8 = first 64 SVDs.
        if map_len > 8 {
            map_len = 8;
        }

        for (i, &b) in data[..map_len].iter().enumerate() {
            map |= (b as u64) << (8 * i);
        }
    }

    if map != 0 {
        connector.display_info.color_formats |= DRM_COLOR_FORMAT_YCBCR420;
    }

    map
}

/// `eotf_supported`.
fn eotf_supported(edid_ext: &[u8]) -> u8 {
    edid_ext[2]
        & ((1 << HDMI_EOTF_TRADITIONAL_GAMMA_SDR)
            | (1 << HDMI_EOTF_TRADITIONAL_GAMMA_HDR)
            | (1 << HDMI_EOTF_SMPTE_ST2084)
            | (1 << HDMI_EOTF_BT_2100_HLG)) as u8
}

/// `hdr_metadata_type`.
fn hdr_metadata_type(edid_ext: &[u8]) -> u8 {
    edid_ext[3] & (1 << HDMI_STATIC_METADATA_TYPE1) as u8
}

/// `drm_calculate_luminance_range`.
fn drm_calculate_luminance_range(connector: &mut drm_connector) {
    let hdr_metadata = connector.display_info.hdr_sink_metadata.hdmi_type1;
    let luminance_range = &mut connector.display_info.luminance_range;
    const pre_computed_values: [u8; 32] = [
        50, 51, 52, 53, 55, 56, 57, 58, 59, 61, 62, 63, 65, 66, 68, 69, 71, 72, 74, 75, 77, 79, 81, 82, 84, 86, 88,
        90, 92, 94, 96, 98,
    ];

    if hdr_metadata.metadata_type as u32 & (1 << HDMI_STATIC_METADATA_TYPE1) == 0 {
        return;
    }

    let max_avg = hdr_metadata.max_fall as u32;
    let min_cll = hdr_metadata.min_cll as u32;

    // From the specification (CTA-861-G), for calculating the maximum
    // luminance we need to use:
    //	Luminance = 50*2**(CV/32)
    // Where CV is a one-byte value. CV = 32*q + r gives
    // 50*(2**q)*(2**(r/32)), with the values of r/32 pre-computed:
    //	(0...32).each {|cv| puts (50*2**(cv/32.0)).round}
    let q = max_avg >> 5;
    let r = max_avg % 32;
    let max = (1u32 << q).wrapping_mul(pre_computed_values[r as usize] as u32);

    // min luminance: maxLum * (CV/255)^2 / 100
    let q = (min_cll + 255 / 2) / 255;
    let min = max.wrapping_mul((q * q + 100 / 2) / 100);

    luminance_range.min_luminance = min;
    luminance_range.max_luminance = max;
}

/// `drm_parse_hdr_metadata_block`.
fn drm_parse_hdr_metadata_block(connector: &mut drm_connector, db: &[u8]) {
    let len = cea_db_payload_len(db);
    let hdr_metadata = &mut connector.display_info.hdr_sink_metadata.hdmi_type1;

    hdr_metadata.eotf = eotf_supported(db);
    hdr_metadata.metadata_type = hdr_metadata_type(db);

    if len >= 4 {
        hdr_metadata.max_cll = db[4] as u16;
    }
    if len >= 5 {
        hdr_metadata.max_fall = db[5] as u16;
    }
    if len >= 6 {
        hdr_metadata.min_cll = db[6] as u16;

        // Calculate only when all values are available
        drm_calculate_luminance_range(connector);
    }
}

/// `drm_parse_hdmi_vsdb_audio`.
fn drm_parse_hdmi_vsdb_audio(connector: &mut drm_connector, db: &[u8]) {
    let len = cea_db_payload_len(db);

    if len >= 6 && db[6] & (1 << 7) != 0 {
        connector.eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |= DRM_ELD_SUPPORTS_AI as u8;
    }

    if len >= 10 && hdmi_vsdb_latency_present(db) {
        connector.latency_present[0] = true;
        connector.video_latency[0] = db[9] as i32;
        connector.audio_latency[0] = db[10] as i32;
    }

    if len >= 12 && hdmi_vsdb_i_latency_present(db) {
        connector.latency_present[1] = true;
        connector.video_latency[1] = db[11] as i32;
        connector.audio_latency[1] = db[12] as i32;
    }
}

/// `drm_parse_cea_ext`.
fn drm_parse_cea_ext(connector: &mut drm_connector, drm_edid: &drm_edid) {
    let info = &mut connector.display_info;
    let mut y420cmdb_map: u64 = 0;

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
            drm_parse_hdmi_vsdb_video(connector, db);
        } else if cea_db_is_hdmi_forum_vsdb(db) || cea_db_is_hdmi_forum_scdb(db) {
            drm_parse_hdmi_forum_scds(connector, db);
        } else if cea_db_is_microsoft_vsdb(db) {
            drm_parse_microsoft_vsdb(connector, db);
        } else if cea_db_is_y420cmdb(db) {
            y420cmdb_map = parse_cta_y420cmdb(connector, db);
        } else if cea_db_is_y420vdb(db) {
            parse_cta_y420vdb(connector, db);
        } else if cea_db_is_vcdb(db) {
            drm_parse_vcdb(connector, db);
        } else if cea_db_is_hdmi_hdr_metadata_block(db) {
            drm_parse_hdr_metadata_block(connector, db);
        } else if cea_db_tag(db) == CTA_DB_VIDEO {
            parse_cta_vdb(connector, db);
        } else if cea_db_tag(db) == CTA_DB_AUDIO {
            connector.display_info.has_audio = true;
        }
    }
    cea_db_iter_end(&mut iter);

    if y420cmdb_map != 0 {
        update_cta_y420cmdb(connector, y420cmdb_map);
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
    // The HDMI VSDBs, parsed for audio once `eld` is no longer borrowed:
    // drm_parse_hdmi_vsdb_audio's bits are or-ed in, in any order.
    let mut hdmi_vsdbs: Vec<Vec<u8>> = Vec::new();

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
                    hdmi_vsdbs.push(db.to_vec());
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

    for db in &hdmi_vsdbs {
        drm_parse_hdmi_vsdb_audio(connector, db);
    }
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

/// `drm_edid_alloc`: a copy of `edid` as a drm_edid.
pub fn drm_edid_alloc(edid: &[u8]) -> Option<drm_edid> {
    if edid.is_empty() || edid.len() < EDID_LENGTH {
        return None;
    }

    _drm_edid_alloc(edid.to_vec(), edid.len())
}

// ── modes ──

/// `struct edid` offsets of the established and standard timings.
const EDID_EST_T1: usize = 35;
const EDID_EST_T2: usize = 36;
const EDID_EST_MFG_RSVD: usize = 37;
const EDID_STD_TIMINGS_OFF: usize = 38;

/// `struct detailed_pixel_timing` byte offsets in a detailed timing.
const PT_HACTIVE_LO: usize = 2;
const PT_HBLANK_LO: usize = 3;
const PT_HACTIVE_HBLANK_HI: usize = 4;
const PT_VACTIVE_LO: usize = 5;
const PT_VBLANK_LO: usize = 6;
const PT_VACTIVE_VBLANK_HI: usize = 7;
const PT_HSYNC_OFFSET_LO: usize = 8;
const PT_HSYNC_PULSE_WIDTH_LO: usize = 9;
const PT_VSYNC_OFFSET_PULSE_WIDTH_LO: usize = 10;
const PT_HSYNC_VSYNC_OFFSET_PULSE_WIDTH_HI: usize = 11;
const PT_WIDTH_MM_LO: usize = 12;
const PT_HEIGHT_MM_LO: usize = 13;
const PT_WIDTH_HEIGHT_MM_HI: usize = 14;
const PT_MISC: usize = 17;

/// `struct detailed_data_monitor_range` offsets in a display descriptor.
const DT_RANGE_FORMULA: usize = DT_DATA + 6;

fn le16(b: &[u8], o: usize) -> u16 {
    b[o] as u16 | (b[o + 1] as u16) << 8
}

/// A byte of a data block, 0 past the EDID's end: the parsers below index
/// by lengths the block declares.
fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// `MODE_SIZE`.
fn MODE_SIZE(m: &drm_display_mode) -> i32 {
    (m.hdisplay as i32).wrapping_mul(m.vdisplay as i32)
}

/// `drm_mode_probed_add`.
fn drm_mode_probed_add(connector: &mut drm_connector, mode: drm_display_mode) {
    connector.probed_modes.push(mode);
}

/// `edid_fixup_preferred`: walk the mode list for connector, clearing the
/// preferred status on existing modes and setting it anew for the right
/// mode ala quirks.
fn edid_fixup_preferred(connector: &mut drm_connector) {
    let mut target_refresh = 0;

    if connector.probed_modes.is_empty() {
        return;
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_PREFER_LARGE_60) {
        target_refresh = 60;
    }
    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_PREFER_LARGE_75) {
        target_refresh = 75;
    }

    let mut preferred = 0;
    for cur in 0..connector.probed_modes.len() {
        connector.probed_modes[cur].r#type &= !(DRM_MODE_TYPE_PREFERRED as u8);

        if cur == preferred {
            continue;
        }

        let cur_mode = &connector.probed_modes[cur];
        // Largest mode is preferred
        if MODE_SIZE(cur_mode) > MODE_SIZE(&connector.probed_modes[preferred]) {
            preferred = cur;
        }

        let cur_vrefresh = drm_mode_vrefresh(cur_mode);
        let preferred_vrefresh = drm_mode_vrefresh(&connector.probed_modes[preferred]);
        // At a given size, try to get closest to target refresh
        if MODE_SIZE(cur_mode) == MODE_SIZE(&connector.probed_modes[preferred])
            && (cur_vrefresh - target_refresh).wrapping_abs() < (preferred_vrefresh - target_refresh).wrapping_abs()
        {
            preferred = cur;
        }
    }

    connector.probed_modes[preferred].r#type |= DRM_MODE_TYPE_PREFERRED as u8;
}

/// `mode_is_rb`.
fn mode_is_rb(mode: &drm_display_mode) -> bool {
    (mode.htotal as i32 - mode.hdisplay as i32 == 160)
        && (mode.hsync_end as i32 - mode.hdisplay as i32 == 80)
        && (mode.hsync_end as i32 - mode.hsync_start as i32 == 32)
        && (mode.vsync_start as i32 - mode.vdisplay as i32 == 3)
}

/// `drm_mode_find_dmt`: a copy of the DMT mode with these parameters.
pub fn drm_mode_find_dmt(hsize: i32, vsize: i32, fresh: i32, rb: bool) -> Option<drm_display_mode> {
    for t in drm_dmt_modes.iter() {
        let ptr = mode_from_table(t);

        if hsize != ptr.hdisplay as i32 {
            continue;
        }
        if vsize != ptr.vdisplay as i32 {
            continue;
        }
        if fresh != drm_mode_vrefresh(&ptr) {
            continue;
        }
        if rb != mode_is_rb(&ptr) {
            continue;
        }

        return Some(ptr);
    }

    None
}

/// `is_detailed_timing_descriptor`.
fn is_detailed_timing_descriptor(descriptor: &[u8]) -> bool {
    le16(descriptor, 0) != 0
}

/// `drm_monitor_supports_rb`: EDID 1.4 defines this explicitly. For EDID
/// 1.3, we guess, badly.
fn drm_monitor_supports_rb(drm_edid: &drm_edid) -> bool {
    if drm_edid.edid[EDID_REVISION] >= 4 {
        let mut ret = false;

        drm_for_each_detailed_block(Some(drm_edid), &mut |descriptor| {
            // is_rb
            if !is_display_descriptor(descriptor, EDID_DETAIL_MONITOR_RANGE as u8) {
                return;
            }

            if descriptor[DT_RANGE_FLAGS] as u32 == DRM_EDID_CVT_SUPPORT_FLAG
                && descriptor[15] as u32 & DRM_EDID_CVT_FLAGS_REDUCED_BLANKING != 0
            {
                ret = true;
            }
        });
        return ret;
    }

    drm_edid_is_digital(Some(drm_edid))
}

/// `find_gtf2`: the last secondary-GTF range descriptor, as 18 bytes.
fn find_gtf2(drm_edid: &drm_edid) -> Option<[u8; DT_SIZE]> {
    let mut res = None;

    drm_for_each_detailed_block(Some(drm_edid), &mut |descriptor| {
        if !is_display_descriptor(descriptor, EDID_DETAIL_MONITOR_RANGE as u8) {
            return;
        }

        if descriptor[DT_RANGE_FLAGS] as u32 == DRM_EDID_SECONDARY_GTF_SUPPORT_FLAG {
            let mut d = [0u8; DT_SIZE];
            d.copy_from_slice(descriptor);
            res = Some(d);
        }
    });

    res
}

/// `drm_gtf2_hbreak`: secondary GTF curve kicks in above some break
/// frequency.
fn drm_gtf2_hbreak(drm_edid: &drm_edid) -> i32 {
    find_gtf2(drm_edid).map_or(0, |d| d[12] as i32 * 2)
}

/// `drm_gtf2_2c`.
fn drm_gtf2_2c(drm_edid: &drm_edid) -> i32 {
    find_gtf2(drm_edid).map_or(0, |d| d[13] as i32)
}

/// `drm_gtf2_m`.
fn drm_gtf2_m(drm_edid: &drm_edid) -> i32 {
    find_gtf2(drm_edid).map_or(0, |d| le16(&d, 14) as i32)
}

/// `drm_gtf2_k`.
fn drm_gtf2_k(drm_edid: &drm_edid) -> i32 {
    find_gtf2(drm_edid).map_or(0, |d| d[16] as i32)
}

/// `drm_gtf2_2j`.
fn drm_gtf2_2j(drm_edid: &drm_edid) -> i32 {
    find_gtf2(drm_edid).map_or(0, |d| d[17] as i32)
}

/// `standard_timing_level`: CVT, GTF2, GTF or DMT. On 1.4, Linux's
/// `get_timing_level` leaves CVT whatever the range descriptors say.
fn standard_timing_level(drm_edid: &drm_edid) -> u32 {
    let revision = drm_edid.edid[EDID_REVISION];

    if revision >= 4 {
        LEVEL_CVT
    } else if revision >= 3 && drm_gtf2_hbreak(drm_edid) != 0 {
        LEVEL_GTF2
    } else if revision >= 2 {
        LEVEL_GTF
    } else {
        LEVEL_DMT
    }
}

/// `bad_std_timing`: 0 is reserved, 1 or 0x20 as both bytes is unused.
fn bad_std_timing(a: u8, b: u8) -> bool {
    (a == 0x00 && b == 0x00) || (a == 0x01 && b == 0x01) || (a == 0x20 && b == 0x20)
}

/// `drm_mode_hsync`.
fn drm_mode_hsync(mode: &drm_display_mode) -> i32 {
    if mode.htotal == 0 {
        return 0;
    }

    let d = mode.htotal as i32;
    if mode.clock > 0 { mode.clock.wrapping_add(d / 2) / d } else { mode.clock.wrapping_sub(d / 2) / d }
}

/// `drm_gtf2_mode`: GTF, or GTF2 above the break frequency.
fn drm_gtf2_mode(drm_edid: &drm_edid, hsize: i32, vsize: i32, vrefresh_rate: i32) -> Option<drm_display_mode> {
    // This is potentially wrong if there's ever a monitor with
    // more than one ranges section, each claiming a different
    // secondary GTF curve. Please don't do that.
    let mode = drm_gtf_mode(hsize, vsize, vrefresh_rate, false, 0)?;

    if drm_mode_hsync(&mode) > drm_gtf2_hbreak(drm_edid) {
        return drm_gtf_mode_complex(hsize, vsize, vrefresh_rate, false, 0, drm_gtf2_m(drm_edid),
                                    drm_gtf2_2c(drm_edid), drm_gtf2_k(drm_edid), drm_gtf2_2j(drm_edid));
    }

    Some(mode)
}

/// `drm_mode_std`: the mode of a standard timing (`t`: hsize, vfreq_aspect).
fn drm_mode_std(connector: &drm_connector, drm_edid: &drm_edid, t: &[u8]) -> Option<drm_display_mode> {
    let (t_hsize, vfreq_aspect) = (t[0], t[1]);
    let aspect_ratio = (vfreq_aspect as u32 & EDID_TIMING_ASPECT_MASK) >> EDID_TIMING_ASPECT_SHIFT;
    let vfreq = (vfreq_aspect as u32 & EDID_TIMING_VFREQ_MASK) >> EDID_TIMING_VFREQ_SHIFT;
    let timing_level = standard_timing_level(drm_edid);

    if bad_std_timing(t_hsize, vfreq_aspect) {
        return None;
    }

    // According to the EDID spec, the hdisplay = hsize * 8 + 248
    let mut hsize = t_hsize as i32 * 8 + 248;
    // vrefresh_rate = vfreq + 60
    let vrefresh_rate = vfreq as i32 + 60;
    // the vdisplay is calculated based on the aspect ratio
    let mut vsize = if aspect_ratio == 0 {
        if drm_edid.edid[EDID_REVISION] < 3 { hsize } else { (hsize * 10) / 16 }
    } else if aspect_ratio == 1 {
        (hsize * 3) / 4
    } else if aspect_ratio == 2 {
        (hsize * 4) / 5
    } else {
        (hsize * 9) / 16
    };

    // HDTV hack, part 1
    if vrefresh_rate == 60 && ((hsize == 1360 && vsize == 765) || (hsize == 1368 && vsize == 769)) {
        hsize = 1366;
        vsize = 768;
    }

    // If this connector already has a mode for this size and refresh
    // rate (because it came from detailed or CVT info), use that
    // instead. This way we don't have to guess at interlace or
    // reduced blanking.
    for m in &connector.probed_modes {
        if m.hdisplay as i32 == hsize && m.vdisplay as i32 == vsize && drm_mode_vrefresh(m) == vrefresh_rate {
            return None;
        }
    }

    // HDTV hack, part 2
    if hsize == 1366 && vsize == 768 && vrefresh_rate == 60 {
        let mut mode = drm_cvt_mode(1366, 768, vrefresh_rate, false, false, false)?;
        mode.hdisplay = 1366;
        mode.hsync_start = mode.hsync_start.wrapping_sub(1);
        mode.hsync_end = mode.hsync_end.wrapping_sub(1);
        return Some(mode);
    }

    // check whether it can be found in default mode table
    if drm_monitor_supports_rb(drm_edid) {
        if let Some(mode) = drm_mode_find_dmt(hsize, vsize, vrefresh_rate, true) {
            return Some(mode);
        }
    }
    if let Some(mode) = drm_mode_find_dmt(hsize, vsize, vrefresh_rate, false) {
        return Some(mode);
    }

    // okay, generate it
    match timing_level {
        LEVEL_GTF => drm_gtf_mode(hsize, vsize, vrefresh_rate, false, 0),
        LEVEL_GTF2 => drm_gtf2_mode(drm_edid, hsize, vsize, vrefresh_rate),
        LEVEL_CVT => drm_cvt_mode(hsize, vsize, vrefresh_rate, false, false, false),
        _ => None,
    }
}

/// `drm_mode_do_interlace_quirk`: EDID is delightfully ambiguous about how
/// interlaced modes are to be encoded. Our internal representation is of
/// frame height, but some HDTV detailed timings are encoded as field
/// height.
fn drm_mode_do_interlace_quirk(mode: &mut drm_display_mode, pt: &[u8]) {
    const cea_interlaced: [(u16, u16); 7] =
        [(1920, 1080), (720, 480), (1440, 480), (2880, 480), (720, 576), (1440, 576), (2880, 576)];

    if pt[PT_MISC] as u32 & DRM_EDID_PT_INTERLACED == 0 {
        return;
    }

    for &(w, h) in &cea_interlaced {
        if mode.hdisplay == w && mode.vdisplay == h / 2 {
            mode.vdisplay = mode.vdisplay.wrapping_mul(2);
            mode.vsync_start = mode.vsync_start.wrapping_mul(2);
            mode.vsync_end = mode.vsync_end.wrapping_mul(2);
            mode.vtotal = mode.vtotal.wrapping_mul(2);
            mode.vtotal |= 1;
        }
    }

    mode.flags |= DRM_MODE_FLAG_INTERLACE;
}

/// `drm_mode_detailed`: the mode of an EDID detailed timing descriptor.
fn drm_mode_detailed(connector: &drm_connector, drm_edid: &drm_edid, timing: &[u8]) -> Option<drm_display_mode> {
    let pt = timing;
    let hactive = (pt[PT_HACTIVE_HBLANK_HI] as u32 & 0xf0) << 4 | pt[PT_HACTIVE_LO] as u32;
    let vactive = (pt[PT_VACTIVE_VBLANK_HI] as u32 & 0xf0) << 4 | pt[PT_VACTIVE_LO] as u32;
    let hblank = (pt[PT_HACTIVE_HBLANK_HI] as u32 & 0xf) << 8 | pt[PT_HBLANK_LO] as u32;
    let vblank = (pt[PT_VACTIVE_VBLANK_HI] as u32 & 0xf) << 8 | pt[PT_VBLANK_LO] as u32;
    let hi = pt[PT_HSYNC_VSYNC_OFFSET_PULSE_WIDTH_HI] as u32;
    let hsync_offset = (hi & 0xc0) << 2 | pt[PT_HSYNC_OFFSET_LO] as u32;
    let hsync_pulse_width = (hi & 0x30) << 4 | pt[PT_HSYNC_PULSE_WIDTH_LO] as u32;
    let vsync_offset = (hi & 0xc) << 2 | pt[PT_VSYNC_OFFSET_PULSE_WIDTH_LO] as u32 >> 4;
    let vsync_pulse_width = (hi & 0x3) << 4 | (pt[PT_VSYNC_OFFSET_PULSE_WIDTH_LO] as u32 & 0xf);

    // ignore tiny modes
    if hactive < 64 || vactive < 64 {
        return None;
    }

    if pt[PT_MISC] as u32 & DRM_EDID_PT_STEREO != 0 {
        // Stereo mode not supported
        return None;
    }

    // it is incorrect if hsync/vsync width is zero
    if hsync_pulse_width == 0 || vsync_pulse_width == 0 {
        return None;
    }

    let mut mode;
    'set_size: {
        if drm_edid_has_internal_quirk(connector, EDID_QUIRK_FORCE_REDUCED_BLANKING) {
            mode = drm_cvt_mode(hactive as i32, vactive as i32, 60, true, false, false)?;
            break 'set_size;
        }

        mode = drm_display_mode::default();

        if drm_edid_has_internal_quirk(connector, EDID_QUIRK_135_CLOCK_TOO_HIGH) {
            mode.clock = 1088 * 10;
        } else {
            mode.clock = le16(timing, 0) as i32 * 10;
        }

        mode.hdisplay = hactive as u16;
        mode.hsync_start = (mode.hdisplay as u32 + hsync_offset) as u16;
        mode.hsync_end = (mode.hsync_start as u32 + hsync_pulse_width) as u16;
        mode.htotal = (mode.hdisplay as u32 + hblank) as u16;

        mode.vdisplay = vactive as u16;
        mode.vsync_start = (mode.vdisplay as u32 + vsync_offset) as u16;
        mode.vsync_end = (mode.vsync_start as u32 + vsync_pulse_width) as u16;
        mode.vtotal = (mode.vdisplay as u32 + vblank) as u16;

        // Some EDIDs have bogus h/vsync_end values
        if mode.hsync_end > mode.htotal {
            mode.hsync_end = mode.htotal;
        }
        if mode.vsync_end > mode.vtotal {
            mode.vsync_end = mode.vtotal;
        }

        drm_mode_do_interlace_quirk(&mut mode, pt);

        if drm_edid_has_internal_quirk(connector, EDID_QUIRK_DETAILED_SYNC_PP) {
            mode.flags |= DRM_MODE_FLAG_PHSYNC | DRM_MODE_FLAG_PVSYNC;
        } else {
            mode.flags |= if pt[PT_MISC] as u32 & DRM_EDID_PT_HSYNC_POSITIVE != 0 {
                DRM_MODE_FLAG_PHSYNC
            } else {
                DRM_MODE_FLAG_NHSYNC
            };
            mode.flags |= if pt[PT_MISC] as u32 & DRM_EDID_PT_VSYNC_POSITIVE != 0 {
                DRM_MODE_FLAG_PVSYNC
            } else {
                DRM_MODE_FLAG_NVSYNC
            };
        }
    }

    // set_size:
    mode.width_mm = pt[PT_WIDTH_MM_LO] as u16 | (pt[PT_WIDTH_HEIGHT_MM_HI] as u16 & 0xf0) << 4;
    mode.height_mm = pt[PT_HEIGHT_MM_LO] as u16 | (pt[PT_WIDTH_HEIGHT_MM_HI] as u16 & 0xf) << 8;

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_DETAILED_IN_CM) {
        mode.width_mm = mode.width_mm.wrapping_mul(10);
        mode.height_mm = mode.height_mm.wrapping_mul(10);
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_DETAILED_USE_MAXIMUM_SIZE) {
        mode.width_mm = drm_edid.edid[EDID_WIDTH_CM] as u16 * 10;
        mode.height_mm = drm_edid.edid[EDID_HEIGHT_CM] as u16 * 10;
    }

    mode.r#type = DRM_MODE_TYPE_DRIVER as u8;
    drm_mode_set_name(&mut mode);

    Some(mode)
}

/// `mode_in_hsync_range`.
fn mode_in_hsync_range(mode: &drm_display_mode, edid: &[u8], t: &[u8]) -> bool {
    let mut hmin = t[7] as i32;
    if edid[EDID_REVISION] >= 4 {
        hmin += if t[4] & 0x04 != 0 { 255 } else { 0 };
    }
    let mut hmax = t[8] as i32;
    if edid[EDID_REVISION] >= 4 {
        hmax += if t[4] & 0x08 != 0 { 255 } else { 0 };
    }
    let hsync = drm_mode_hsync(mode);

    hsync <= hmax && hsync >= hmin
}

/// `mode_in_vsync_range`.
fn mode_in_vsync_range(mode: &drm_display_mode, edid: &[u8], t: &[u8]) -> bool {
    let mut vmin = t[5] as i32;
    if edid[EDID_REVISION] >= 4 {
        vmin += if t[4] & 0x01 != 0 { 255 } else { 0 };
    }
    let mut vmax = t[6] as i32;
    if edid[EDID_REVISION] >= 4 {
        vmax += if t[4] & 0x02 != 0 { 255 } else { 0 };
    }
    let vsync = drm_mode_vrefresh(mode);

    vsync <= vmax && vsync >= vmin
}

/// `range_pixel_clock`.
fn range_pixel_clock(edid: &[u8], t: &[u8]) -> u32 {
    // unspecified
    if t[9] == 0 || t[9] == 255 {
        return 0;
    }

    // 1.4 with CVT support gives us real precision, yay
    if edid[EDID_REVISION] >= 4 && t[10] as u32 == DRM_EDID_CVT_SUPPORT_FLAG {
        return (t[9] as i32 * 10000 - (t[12] as i32 >> 2) * 250) as u32;
    }

    // 1.3 is pathetic, so fuzz up a bit
    t[9] as u32 * 10000 + 5001
}

/// `mode_in_range`.
fn mode_in_range(mode: &drm_display_mode, drm_edid: &drm_edid, timing: &[u8]) -> bool {
    let edid = &drm_edid.edid;
    let t = timing;

    if !mode_in_hsync_range(mode, edid, t) {
        return false;
    }

    if !mode_in_vsync_range(mode, edid, t) {
        return false;
    }

    let max_clock = range_pixel_clock(edid, t);
    if max_clock != 0 && mode.clock as u32 > max_clock {
        return false;
    }

    // 1.4 max horizontal check
    if edid[EDID_REVISION] >= 4
        && t[10] as u32 == DRM_EDID_CVT_SUPPORT_FLAG
        && t[13] != 0
        && mode.hdisplay as i32 > 8 * (t[13] as i32 + 256 * (t[12] as i32 & 0x3))
    {
        return false;
    }

    if mode_is_rb(mode) && !drm_monitor_supports_rb(drm_edid) {
        return false;
    }

    true
}

/// `valid_inferred_mode`.
fn valid_inferred_mode(connector: &drm_connector, mode: &drm_display_mode) -> bool {
    let mut ok = false;

    for m in &connector.probed_modes {
        if mode.hdisplay == m.hdisplay && mode.vdisplay == m.vdisplay && drm_mode_vrefresh(mode) == drm_mode_vrefresh(m) {
            return false; // duplicated
        }
        if mode.hdisplay <= m.hdisplay && mode.vdisplay <= m.vdisplay {
            ok = true;
        }
    }
    ok
}

/// `drm_dmt_modes_for_range`.
fn drm_dmt_modes_for_range(connector: &mut drm_connector, drm_edid: &drm_edid, timing: &[u8]) -> i32 {
    let mut modes = 0;

    for t in drm_dmt_modes.iter() {
        let m = mode_from_table(t);
        if mode_in_range(&m, drm_edid, timing) && valid_inferred_mode(connector, &m) {
            drm_mode_probed_add(connector, m);
            modes += 1;
        }
    }

    modes
}

/// `drm_mode_fixup_1366x768`: fix up 1366x768 mode from 1368x768; GFT/CVT
/// can't express 1366 width which isn't dividable by 8.
pub fn drm_mode_fixup_1366x768(mode: &mut drm_display_mode) {
    if mode.hdisplay == 1368 && mode.vdisplay == 768 {
        mode.hdisplay = 1366;
        mode.hsync_start = mode.hsync_start.wrapping_sub(1);
        mode.hsync_end = mode.hsync_end.wrapping_sub(1);
        drm_mode_set_name(mode);
    }
}

/// `drm_gtf_modes_for_range`, `drm_gtf2_modes_for_range`,
/// `drm_cvt_modes_for_range`: the extra modes by one formula.
fn formula_modes_for_range(connector: &mut drm_connector, drm_edid: &drm_edid, timing: &[u8],
                           make: &dyn Fn(i32, i32, i32) -> Option<drm_display_mode>) -> i32 {
    let mut modes = 0;

    for m in extra_modes.iter() {
        let Some(mut newmode) = make(m.0 as i32, m.1 as i32, m.2 as i32) else { return modes };

        drm_mode_fixup_1366x768(&mut newmode);
        if !mode_in_range(&newmode, drm_edid, timing) || !valid_inferred_mode(connector, &newmode) {
            continue;
        }

        drm_mode_probed_add(connector, newmode);
        modes += 1;
    }

    modes
}

/// `do_inferred_modes` + `add_inferred_modes`.
fn add_inferred_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let mut modes = 0;

    if drm_edid.edid[EDID_REVISION] < 1 {
        return 0;
    }

    drm_for_each_detailed_block(Some(drm_edid), &mut |timing| {
        if !is_display_descriptor(timing, EDID_DETAIL_MONITOR_RANGE as u8) {
            return;
        }

        modes += drm_dmt_modes_for_range(connector, drm_edid, timing);

        if drm_edid.edid[EDID_REVISION] < 2 {
            return; // GTF not defined yet
        }

        match timing[DT_RANGE_FLAGS] as u32 {
            DRM_EDID_SECONDARY_GTF_SUPPORT_FLAG => {
                modes += formula_modes_for_range(connector, drm_edid, timing,
                                                 &|w, h, r| drm_gtf2_mode(drm_edid, w, h, r));
            }
            DRM_EDID_DEFAULT_GTF_SUPPORT_FLAG => {
                modes += formula_modes_for_range(connector, drm_edid, timing,
                                                 &|w, h, r| drm_gtf_mode(w, h, r, false, 0));
            }
            DRM_EDID_CVT_SUPPORT_FLAG => {
                if drm_edid.edid[EDID_REVISION] < 4 {
                    return;
                }
                let rb = drm_monitor_supports_rb(drm_edid);
                modes += formula_modes_for_range(connector, drm_edid, timing,
                                                 &|w, h, r| drm_cvt_mode(w, h, r, rb, false, false));
            }
            _ => {}
        }
    });

    modes
}

/// `drm_est3_modes`.
fn drm_est3_modes(connector: &mut drm_connector, timing: &[u8]) -> i32 {
    let mut modes = 0;
    let est = &timing[6..];

    for i in 0..6 {
        for j in (0..8).rev() {
            let m = (i * 8) + (7 - j);
            if m >= est3_modes.len() {
                break;
            }
            if est[i] & (1 << j) != 0 {
                let e = est3_modes[m];
                if let Some(mode) = drm_mode_find_dmt(e.0 as i32, e.1 as i32, e.2 as i32, e.3 != 0) {
                    drm_mode_probed_add(connector, mode);
                    modes += 1;
                }
            }
        }
    }

    modes
}

/// `add_established_modes`: the established timings bitmap of the base
/// block, then the established timings III descriptors.
fn add_established_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let edid = &drm_edid.edid;
    let est_bits: u32 =
        edid[EDID_EST_T1] as u32 | (edid[EDID_EST_T2] as u32) << 8 | (edid[EDID_EST_MFG_RSVD] as u32 & 0x80) << 9;
    let mut modes = 0;

    for i in 0..=EDID_EST_TIMINGS as usize {
        if est_bits & (1 << i) != 0 {
            drm_mode_probed_add(connector, mode_from_table(&edid_est_modes[i]));
            modes += 1;
        }
    }

    let mut closure_modes = 0;
    if edid[EDID_REVISION] >= 1 {
        drm_for_each_detailed_block(Some(drm_edid), &mut |timing| {
            // do_established_modes
            if !is_display_descriptor(timing, EDID_DETAIL_EST_TIMINGS as u8) {
                return;
            }

            closure_modes += drm_est3_modes(connector, timing);
        });
    }

    modes + closure_modes
}

/// `add_standard_modes`: the standard timings of the base block, then
/// those of standard timing descriptors.
fn add_standard_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let mut modes = 0;

    for i in 0..EDID_STD_TIMINGS as usize {
        let o = EDID_STD_TIMINGS_OFF + 2 * i;
        if let Some(newmode) = drm_mode_std(connector, drm_edid, &drm_edid.edid[o..o + 2]) {
            drm_mode_probed_add(connector, newmode);
            modes += 1;
        }
    }

    let mut closure_modes = 0;
    if drm_edid.edid[EDID_REVISION] >= 1 {
        drm_for_each_detailed_block(Some(drm_edid), &mut |timing| {
            // do_standard_modes
            if !is_display_descriptor(timing, EDID_DETAIL_STD_MODES as u8) {
                return;
            }

            for i in 0..6 {
                let o = DT_DATA + 2 * i;
                if let Some(newmode) = drm_mode_std(connector, drm_edid, &timing[o..o + 2]) {
                    drm_mode_probed_add(connector, newmode);
                    closure_modes += 1;
                }
            }
        });
    }

    // XXX should also look for standard codes in VTB blocks

    modes + closure_modes
}

/// `drm_cvt_modes`: the four 3-byte CVT codes of a descriptor (Linux's
/// `cvt[i]` starts at the descriptor's data, byte 5).
fn drm_cvt_modes(connector: &mut drm_connector, timing: &[u8]) -> i32 {
    let mut modes = 0;
    const rates: [i32; 5] = [60, 85, 75, 60, 50];

    for i in 0..4 {
        let code = &timing[DT_DATA + 3 * i..DT_DATA + 3 * i + 3];

        if code == [0, 0, 0] {
            continue;
        }

        let height = (code[0] as i32 + ((code[1] as i32 & 0xf0) << 4) + 1) * 2;
        let width = match code[1] & 0x0c {
            0x04 => height * 16 / 9,
            0x08 => height * 16 / 10,
            0x0c => height * 15 / 9,
            _ => height * 4 / 3,
        };

        for j in 1..5 {
            if code[2] & (1 << j) != 0 {
                if let Some(newmode) = drm_cvt_mode(width, height, rates[j], j == 0, false, false) {
                    drm_mode_probed_add(connector, newmode);
                    modes += 1;
                }
            }
        }
    }

    modes
}

/// `add_cvt_modes`.
fn add_cvt_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let mut modes = 0;

    if drm_edid.edid[EDID_REVISION] >= 3 {
        drm_for_each_detailed_block(Some(drm_edid), &mut |timing| {
            // do_cvt_mode
            if !is_display_descriptor(timing, EDID_DETAIL_CVT_3BYTE as u8) {
                return;
            }

            modes += drm_cvt_modes(connector, timing);
        });
    }

    // XXX should also look for CVT codes in VTB blocks

    modes
}

/// `add_detailed_modes`: the detailed timings, the first preferred.
fn add_detailed_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let mut modes = 0;
    let mut preferred = if drm_edid.edid[EDID_REVISION] >= 4 {
        true // first detailed timing is always preferred
    } else {
        drm_edid.edid[EDID_FEATURES] as u32 & DRM_EDID_FEATURE_PREFERRED_TIMING != 0
    };

    drm_for_each_detailed_block(Some(drm_edid), &mut |timing| {
        // do_detailed_mode
        if !is_detailed_timing_descriptor(timing) {
            return;
        }

        let Some(mut newmode) = drm_mode_detailed(connector, drm_edid, timing) else { return };

        if preferred {
            newmode.r#type |= DRM_MODE_TYPE_PREFERRED as u8;
        }

        // Detailed modes are limited to 10kHz pixel clock resolution,
        // so fix up anything that looks like CEA/HDMI mode, but the clock
        // is just slightly off.
        fixup_detailed_cea_mode_clock(&mut newmode);

        drm_mode_probed_add(connector, newmode);
        modes += 1;
        preferred = false;
    });

    modes
}

/// `drm_edid_has_cta_extension`: a CTA extension or a DisplayID CTA data
/// block.
fn drm_edid_has_cta_extension(drm_edid: &drm_edid) -> bool {
    let mut found = false;

    // Look for a top level CEA extension block
    let mut edid_iter = drm_edid_iter_begin(Some(drm_edid));
    while let Some(ext) = __drm_edid_iter_next(&mut edid_iter) {
        if drm_edid.edid[ext] as u32 == CEA_EXT {
            found = true;
            break;
        }
    }
    drm_edid_iter_end(&mut edid_iter);

    if found {
        return true;
    }

    // CEA blocks can also be found embedded in a DisplayID block
    let mut iter = displayid_iter_edid_begin(Some(drm_edid));
    while let Some(block) = __displayid_iter_next(&mut iter) {
        if drm_edid.edid[block] as u32 == DATA_BLOCK_CTA {
            found = true;
            break;
        }
    }
    displayid_iter_end(&mut iter);

    found
}

/// `cea_mode_for_vic`.
fn cea_mode_for_vic(vic: u8) -> Option<drm_display_mode> {
    let vic = vic as usize;

    if vic >= 1 && vic < 1 + edid_cea_modes_1.len() {
        return Some(mode_from_table(&edid_cea_modes_1[vic - 1]));
    }
    if vic >= 193 && vic < 193 + edid_cea_modes_193.len() {
        return Some(mode_from_table(&edid_cea_modes_193[vic - 193]));
    }
    None
}

/// `cea_num_vics`.
fn cea_num_vics() -> u8 {
    (193 + edid_cea_modes_193.len()) as u8
}

/// `cea_next_vic`.
fn cea_next_vic(vic: u8) -> u8 {
    let vic = vic + 1;
    if vic as usize == 1 + edid_cea_modes_1.len() { 193 } else { vic }
}

/// `cea_mode_alternate_clock`: the alternate clock for the CEA mode
/// (60Hz vs. 59.94Hz etc.).
fn cea_mode_alternate_clock(cea_mode: &drm_display_mode) -> u32 {
    let mut clock = cea_mode.clock as u32;

    if drm_mode_vrefresh(cea_mode) % 6 != 0 {
        return clock;
    }

    // edid_cea_modes contains the 59.94Hz
    // variant for 240 and 480 line modes,
    // and the 60Hz variant otherwise.
    if cea_mode.vdisplay == 240 || cea_mode.vdisplay == 480 {
        clock = ((clock as u64 * 1001 + 500) / 1000) as u32;
    } else {
        clock = ((clock as u64 * 1000 + 500) / 1001) as u32;
    }

    clock
}

/// `cea_mode_alternate_timings`: for certain VICs the spec allows the
/// vertical front porch to vary by one or two lines; cea_modes[] stores
/// the variant with the shortest one.
fn cea_mode_alternate_timings(vic: u8, mode: &mut drm_display_mode) -> bool {
    if ((vic == 8 || vic == 9 || vic == 12 || vic == 13) && mode.vtotal < 263)
        || ((vic == 23 || vic == 24 || vic == 27 || vic == 28) && mode.vtotal < 314)
    {
        mode.vsync_start += 1;
        mode.vsync_end += 1;
        mode.vtotal += 1;

        return true;
    }

    false
}

/// `abs(int - unsigned int) > unsigned int`, in the kernel's `abs`.
fn clock_off(clock: i32, other: u32, tolerance: u32) -> bool {
    clock.wrapping_sub(other as i32).wrapping_abs() as u32 > tolerance
}

/// `drm_match_cea_mode_clock_tolerance`.
fn drm_match_cea_mode_clock_tolerance(to_match: &drm_display_mode, clock_tolerance: u32) -> u8 {
    let mut match_flags = DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_FLAGS;

    if to_match.clock == 0 {
        return 0;
    }

    if to_match.picture_aspect_ratio != 0 {
        match_flags |= DRM_MODE_MATCH_ASPECT_RATIO;
    }

    let mut vic = 1;
    while vic < cea_num_vics() {
        let mut cea_mode = cea_mode_for_vic(vic).unwrap_or_default();

        // Check both 60Hz and 59.94Hz
        let clock1 = cea_mode.clock as u32;
        let clock2 = cea_mode_alternate_clock(&cea_mode);

        if !(clock_off(to_match.clock, clock1, clock_tolerance) && clock_off(to_match.clock, clock2, clock_tolerance)) {
            loop {
                if drm_mode_match(to_match, &cea_mode, match_flags) {
                    return vic;
                }
                if !cea_mode_alternate_timings(vic, &mut cea_mode) {
                    break;
                }
            }
        }
        vic = cea_next_vic(vic);
    }

    0
}

/// `drm_match_cea_mode`: the CEA Video ID (VIC) of the mode or 0 if it
/// isn't a CEA-861 mode.
pub fn drm_match_cea_mode(to_match: &drm_display_mode) -> u8 {
    let mut match_flags = DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_FLAGS;

    if to_match.clock == 0 {
        return 0;
    }

    if to_match.picture_aspect_ratio != 0 {
        match_flags |= DRM_MODE_MATCH_ASPECT_RATIO;
    }

    let mut vic = 1;
    while vic < cea_num_vics() {
        let mut cea_mode = cea_mode_for_vic(vic).unwrap_or_default();

        // Check both 60Hz and 59.94Hz
        let clock1 = cea_mode.clock;
        let clock2 = cea_mode_alternate_clock(&cea_mode) as i32;

        if KHZ2PICOS(to_match.clock) == KHZ2PICOS(clock1) || KHZ2PICOS(to_match.clock) == KHZ2PICOS(clock2) {
            loop {
                if drm_mode_match(to_match, &cea_mode, match_flags) {
                    return vic;
                }
                if !cea_mode_alternate_timings(vic, &mut cea_mode) {
                    break;
                }
            }
        }
        vic = cea_next_vic(vic);
    }

    0
}

/// `drm_valid_cea_vic`.
fn drm_valid_cea_vic(vic: u8) -> bool {
    cea_mode_for_vic(vic).is_some()
}

/// `drm_get_cea_aspect_ratio`.
fn drm_get_cea_aspect_ratio(video_code: u8) -> u32 {
    cea_mode_for_vic(video_code).map_or(HDMI_PICTURE_ASPECT_NONE, |m| m.picture_aspect_ratio)
}

/// `edid_4k_modes[vic]`.
fn hdmi_mode_for_vic(vic: u8) -> drm_display_mode {
    mode_from_table(&edid_4k_modes[vic as usize])
}

/// `hdmi_mode_alternate_clock`: the alternate clock for HDMI modes (those
/// from the HDMI vendor specific block).
fn hdmi_mode_alternate_clock(hdmi_mode: &drm_display_mode) -> u32 {
    cea_mode_alternate_clock(hdmi_mode)
}

/// `drm_match_hdmi_mode_clock_tolerance`.
fn drm_match_hdmi_mode_clock_tolerance(to_match: &drm_display_mode, clock_tolerance: u32) -> u8 {
    let mut match_flags = DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_FLAGS;

    if to_match.clock == 0 {
        return 0;
    }

    if to_match.picture_aspect_ratio != 0 {
        match_flags |= DRM_MODE_MATCH_ASPECT_RATIO;
    }

    for vic in 1..edid_4k_modes.len() as u8 {
        let hdmi_mode = hdmi_mode_for_vic(vic);

        // Make sure to also match alternate clocks
        let clock1 = hdmi_mode.clock as u32;
        let clock2 = hdmi_mode_alternate_clock(&hdmi_mode);

        if clock_off(to_match.clock, clock1, clock_tolerance) && clock_off(to_match.clock, clock2, clock_tolerance) {
            continue;
        }

        if drm_mode_match(to_match, &hdmi_mode, match_flags) {
            return vic;
        }
    }

    0
}

/// `drm_match_hdmi_mode`: the HDMI Video ID of a mode of the HDMI vendor
/// specific block, or 0.
fn drm_match_hdmi_mode(to_match: &drm_display_mode) -> u8 {
    let mut match_flags = DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_FLAGS;

    if to_match.clock == 0 {
        return 0;
    }

    if to_match.picture_aspect_ratio != 0 {
        match_flags |= DRM_MODE_MATCH_ASPECT_RATIO;
    }

    for vic in 1..edid_4k_modes.len() as u8 {
        let hdmi_mode = hdmi_mode_for_vic(vic);

        // Make sure to also match alternate clocks
        let clock1 = hdmi_mode.clock;
        let clock2 = hdmi_mode_alternate_clock(&hdmi_mode) as i32;

        if (KHZ2PICOS(to_match.clock) == KHZ2PICOS(clock1) || KHZ2PICOS(to_match.clock) == KHZ2PICOS(clock2))
            && drm_mode_match(to_match, &hdmi_mode, match_flags)
        {
            return vic;
        }
    }
    0
}

/// `drm_valid_hdmi_vic`.
fn drm_valid_hdmi_vic(vic: u8) -> bool {
    vic > 0 && (vic as usize) < edid_4k_modes.len()
}

/// `add_alternate_cea_modes`: for each probed CEA or HDMI mode, the
/// variant with the other clock (60Hz vs. 59.94Hz).
fn add_alternate_cea_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let mut list = Vec::new();

    // Don't add CTA modes if the CTA extension block is missing
    if !drm_edid_has_cta_extension(drm_edid) {
        return 0;
    }

    // Go through all probed modes and create a new mode
    // with the alternate clock for certain CEA modes.
    for mode in &connector.probed_modes {
        let mut vic = drm_match_cea_mode(mode);
        let (cea_mode, clock2) = if drm_valid_cea_vic(vic) {
            let m = cea_mode_for_vic(vic).unwrap_or_default();
            let c = cea_mode_alternate_clock(&m);
            (m, c)
        } else {
            vic = drm_match_hdmi_mode(mode);
            if !drm_valid_hdmi_vic(vic) {
                continue;
            }
            let m = hdmi_mode_for_vic(vic);
            let c = hdmi_mode_alternate_clock(&m);
            (m, c)
        };

        let clock1 = cea_mode.clock as u32;

        if clock1 == clock2 {
            continue;
        }

        if mode.clock as u32 != clock1 && mode.clock as u32 != clock2 {
            continue;
        }

        let mut newmode = cea_mode;

        // Carry over the stereo flags
        newmode.flags |= mode.flags & DRM_MODE_FLAG_3D_MASK;

        // The current mode could be either variant. Make
        // sure to pick the "other" clock for the new mode.
        if mode.clock as u32 != clock1 {
            newmode.clock = clock1 as i32;
        } else {
            newmode.clock = clock2 as i32;
        }

        list.push(newmode);
    }

    let modes = list.len() as i32;
    connector.probed_modes.append(&mut list);

    modes
}

/// `svd_to_vic`: 0-6 bit vic, 7th bit native mode indicator.
fn svd_to_vic(svd: u8) -> u8 {
    if (1..=64).contains(&svd) || (129..=192).contains(&svd) {
        return svd & 127;
    }

    svd
}

/// `drm_display_mode_from_vic_index`: the mode of the 0-based vic_index'th
/// VIC across all CTA VDBs in the EDID.
fn drm_display_mode_from_vic_index(connector: &drm_connector, vic_index: i32) -> Option<drm_display_mode> {
    let info = &connector.display_info;

    let vic = *info.vics.get(usize::try_from(vic_index).ok()?)?;
    if vic == 0 {
        return None;
    }

    drm_display_mode_from_cea_vic(vic)
}

/// `do_y420vdb_modes`: the modes of a YCbCr 4:2:0 Video Data Block, which
/// the sink takes in 4:2:0 only.
fn do_y420vdb_modes(connector: &mut drm_connector, svds: &[u8]) -> i32 {
    let mut modes = 0;

    for &svd in svds {
        let vic = svd_to_vic(svd);

        let Some(newmode) = cea_mode_for_vic(vic) else { continue };
        drm_mode_probed_add(connector, newmode);
        modes += 1;
    }

    modes
}

/// `drm_display_mode_from_cea_vic`: a new mode matching the CEA VIC.
pub fn drm_display_mode_from_cea_vic(video_code: u8) -> Option<drm_display_mode> {
    cea_mode_for_vic(video_code)
}

/// `add_cta_vdb_modes`: the modes of the VICs parse_cta_vdb() found.
fn add_cta_vdb_modes(connector: &mut drm_connector) -> i32 {
    let mut modes = 0;

    for i in 0..connector.display_info.vics.len() {
        if let Some(mode) = drm_display_mode_from_vic_index(connector, i as i32) {
            drm_mode_probed_add(connector, mode);
            modes += 1;
        }
    }

    modes
}

/// `stereo_mandatory_modes`: width, height, vrefresh, flags.
const stereo_mandatory_modes: [(u16, u16, i32, u32); 8] = [
    (1920, 1080, 24, DRM_MODE_FLAG_3D_TOP_AND_BOTTOM),
    (1920, 1080, 24, DRM_MODE_FLAG_3D_FRAME_PACKING),
    (1920, 1080, 50, DRM_MODE_FLAG_INTERLACE | DRM_MODE_FLAG_3D_SIDE_BY_SIDE_HALF),
    (1920, 1080, 60, DRM_MODE_FLAG_INTERLACE | DRM_MODE_FLAG_3D_SIDE_BY_SIDE_HALF),
    (1280, 720, 50, DRM_MODE_FLAG_3D_TOP_AND_BOTTOM),
    (1280, 720, 50, DRM_MODE_FLAG_3D_FRAME_PACKING),
    (1280, 720, 60, DRM_MODE_FLAG_3D_TOP_AND_BOTTOM),
    (1280, 720, 60, DRM_MODE_FLAG_3D_FRAME_PACKING),
];

/// `stereo_match_mandatory`.
fn stereo_match_mandatory(mode: &drm_display_mode, stereo_mode: &(u16, u16, i32, u32)) -> bool {
    let interlaced = mode.flags & DRM_MODE_FLAG_INTERLACE;

    mode.hdisplay == stereo_mode.0
        && mode.vdisplay == stereo_mode.1
        && interlaced == (stereo_mode.3 & DRM_MODE_FLAG_INTERLACE)
        && drm_mode_vrefresh(mode) == stereo_mode.2
}

/// `add_hdmi_mandatory_stereo_modes`.
fn add_hdmi_mandatory_stereo_modes(connector: &mut drm_connector) -> i32 {
    let mut stereo_modes = Vec::new();

    for mode in &connector.probed_modes {
        for mandatory in &stereo_mandatory_modes {
            if !stereo_match_mandatory(mode, mandatory) {
                continue;
            }

            let mut new_mode = mode.clone();
            new_mode.flags |= mandatory.3;
            stereo_modes.push(new_mode);
        }
    }

    let modes = stereo_modes.len() as i32;
    connector.probed_modes.append(&mut stereo_modes);

    modes
}

/// `add_hdmi_mode`.
fn add_hdmi_mode(connector: &mut drm_connector, vic: u8) -> i32 {
    if !drm_valid_hdmi_vic(vic) {
        Line::new().s("[CONNECTOR:").bytes(connector.name.as_bytes()).s("] Unknown HDMI VIC: ").dec(vic as u64).say();
        return 0;
    }

    drm_mode_probed_add(connector, hdmi_mode_for_vic(vic));

    1
}

/// `add_3d_struct_modes`.
fn add_3d_struct_modes(connector: &mut drm_connector, structure: u16, vic_index: i32) -> i32 {
    let mut modes = 0;

    for (bit, flag) in [
        (0, DRM_MODE_FLAG_3D_FRAME_PACKING),
        (6, DRM_MODE_FLAG_3D_TOP_AND_BOTTOM),
        (8, DRM_MODE_FLAG_3D_SIDE_BY_SIDE_HALF),
    ] {
        if structure & (1 << bit) != 0 {
            if let Some(mut newmode) = drm_display_mode_from_vic_index(connector, vic_index) {
                newmode.flags |= flag;
                drm_mode_probed_add(connector, newmode);
                modes += 1;
            }
        }
    }

    modes
}

/// `hdmi_vsdb_latency_present`.
fn hdmi_vsdb_latency_present(db: &[u8]) -> bool {
    at(db, 8) & (1 << 7) != 0
}

/// `hdmi_vsdb_i_latency_present`.
fn hdmi_vsdb_i_latency_present(db: &[u8]) -> bool {
    hdmi_vsdb_latency_present(db) && at(db, 8) & (1 << 6) != 0
}

/// `hdmi_vsdb_latency_length`.
fn hdmi_vsdb_latency_length(db: &[u8]) -> i32 {
    if hdmi_vsdb_i_latency_present(db) {
        4
    } else if hdmi_vsdb_latency_present(db) {
        2
    } else {
        0
    }
}

/// `do_hdmi_vsdb_modes`: the modes of the HDMI Vendor Specific data block
/// (`len`: its payload, db[1..=len]), with the stereo 3D modes.
fn do_hdmi_vsdb_modes(connector: &mut drm_connector, db: &[u8], len: u8) -> i32 {
    let len = len as i32;
    let mut modes = 0;
    let mut offset: i32 = 0;
    let mut multi_present = 0;
    let b = |i: i32| at(db, i as usize);

    if len < 8 {
        return modes;
    }

    // no HDMI_Video_Present
    if b(8) & (1 << 5) == 0 {
        return modes;
    }

    offset += hdmi_vsdb_latency_length(db);

    // the declared length is not long enough for the 2 first bytes
    // of additional video format capabilities
    if len < 8 + offset + 2 {
        return modes;
    }

    // 3D_Present
    offset += 1;
    if b(8 + offset) & (1 << 7) != 0 {
        modes += add_hdmi_mandatory_stereo_modes(connector);

        // 3D_Multi_present
        multi_present = (b(8 + offset) & 0x60) >> 5;
    }

    offset += 1;
    let vic_len = (b(8 + offset) >> 5) as i32;
    let hdmi_3d_len = (b(8 + offset) & 0x1f) as i32;

    let mut i = 0;
    while i < vic_len && len >= 9 + offset + i {
        let vic = b(9 + offset + i);
        modes += add_hdmi_mode(connector, vic);
        i += 1;
    }
    offset += 1 + vic_len;

    let multi_len = match multi_present {
        1 => 2,
        2 => 4,
        _ => 0,
    };

    if len < 8 + offset + hdmi_3d_len - 1 {
        return modes;
    }

    if hdmi_3d_len < multi_len {
        return modes;
    }

    if multi_present == 1 || multi_present == 2 {
        // 3D_Structure_ALL
        let structure_all = (b(8 + offset) as u16) << 8 | b(9 + offset) as u16;

        // check if 3D_MASK is present
        let mask: u16 = if multi_present == 2 { (b(10 + offset) as u16) << 8 | b(11 + offset) as u16 } else { 0xffff };

        for i in 0..16 {
            if mask & (1 << i) != 0 {
                modes += add_3d_struct_modes(connector, structure_all, i);
            }
        }
    }

    offset += multi_len;

    let mut i = 0;
    while i < hdmi_3d_len - multi_len {
        let mut newflag = 0;

        let detail_present = (b(8 + offset + i) & 0x0f) > 7;

        if detail_present && i + 1 == hdmi_3d_len - multi_len {
            break;
        }

        // 2D_VIC_order_X
        let vic_index = (b(8 + offset + i) >> 4) as i32;

        // 3D_Structure_X
        match b(8 + offset + i) & 0x0f {
            0 => newflag = DRM_MODE_FLAG_3D_FRAME_PACKING,
            6 => newflag = DRM_MODE_FLAG_3D_TOP_AND_BOTTOM,
            8 => {
                // 3D_Detail_X: 0 and 1 both include horizontal sub-sampling
                if (b(9 + offset + i) >> 4) <= 1 {
                    newflag = DRM_MODE_FLAG_3D_SIDE_BY_SIDE_HALF;
                }
            }
            _ => {}
        }

        if newflag != 0 {
            if let Some(mut newmode) = drm_display_mode_from_vic_index(connector, vic_index) {
                newmode.flags |= newflag;
                drm_mode_probed_add(connector, newmode);
                modes += 1;
            }
        }

        if detail_present {
            i += 1;
        }
        i += 1;
    }

    modes
}

/// `add_cea_modes`: the VDB modes, then those of the HDMI VSDB and the
/// YCbCr 4:2:0 VDB.
fn add_cea_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    // CTA VDB block VICs parsed earlier
    let mut modes = add_cta_vdb_modes(connector);

    let mut iter = cea_db_iter_edid_begin(Some(drm_edid));
    while let Some(db) = __cea_db_iter_next(&mut iter) {
        let db = &iter.edid[db..];
        if cea_db_is_hdmi_vsdb(db) {
            modes += do_hdmi_vsdb_modes(connector, db, cea_db_payload_len(db) as u8);
        } else if cea_db_is_y420vdb(db) {
            let n = cea_db_payload_len(db).saturating_sub(1);
            let vdb420 = &db[CEA_DB_DATA + 1..CEA_DB_DATA + 1 + n];

            // Add 4:2:0(only) modes present in EDID
            modes += do_y420vdb_modes(connector, vdb420);
        }
    }
    cea_db_iter_end(&mut iter);

    modes
}

/// `fixup_detailed_cea_mode_clock`: allow 5kHz clock difference either
/// way to account for the 10kHz clock resolution limit of detailed
/// timings.
fn fixup_detailed_cea_mode_clock(mode: &mut drm_display_mode) {
    let (clock1, clock2);

    let vic = drm_match_cea_mode_clock_tolerance(mode, 5);
    if drm_valid_cea_vic(vic) {
        let cea_mode = cea_mode_for_vic(vic).unwrap_or_default();
        clock1 = cea_mode.clock;
        clock2 = cea_mode_alternate_clock(&cea_mode) as i32;
    } else {
        let vic = drm_match_hdmi_mode_clock_tolerance(mode, 5);
        if drm_valid_hdmi_vic(vic) {
            let cea_mode = hdmi_mode_for_vic(vic);
            clock1 = cea_mode.clock;
            clock2 = hdmi_mode_alternate_clock(&cea_mode) as i32;
        } else {
            return;
        }
    }

    // pick whichever is closest
    let clock = if mode.clock.wrapping_sub(clock1).wrapping_abs() < mode.clock.wrapping_sub(clock2).wrapping_abs() {
        clock1
    } else {
        clock2
    };

    if mode.clock == clock {
        return;
    }

    mode.clock = clock;
}

/// `drm_mode_displayid_detailed`: a DisplayID type I or VII detailed
/// timing (20 bytes).
fn drm_mode_displayid_detailed(timings: &[u8], type_7: bool) -> drm_display_mode {
    let pixel_clock = (timings[0] as u32 | (timings[1] as u32) << 8 | (timings[2] as u32) << 16) + 1;
    let hactive = le16(timings, 4) as u32 + 1;
    let hblank = le16(timings, 6) as u32 + 1;
    let hsync = (le16(timings, 8) as u32 & 0x7fff) + 1;
    let hsync_width = le16(timings, 10) as u32 + 1;
    let vactive = le16(timings, 12) as u32 + 1;
    let vblank = le16(timings, 14) as u32 + 1;
    let vsync = (le16(timings, 16) as u32 & 0x7fff) + 1;
    let vsync_width = le16(timings, 18) as u32 + 1;
    let hsync_positive = le16(timings, 8) & (1 << 15) != 0;
    let vsync_positive = le16(timings, 16) & (1 << 15) != 0;

    let mut mode = drm_display_mode::default();

    // resolution is kHz for type VII, and 10 kHz for type I
    mode.clock = if type_7 { pixel_clock as i32 } else { (pixel_clock * 10) as i32 };
    mode.hdisplay = hactive as u16;
    mode.hsync_start = (mode.hdisplay as u32 + hsync) as u16;
    mode.hsync_end = (mode.hsync_start as u32 + hsync_width) as u16;
    mode.htotal = (mode.hdisplay as u32 + hblank) as u16;

    mode.vdisplay = vactive as u16;
    mode.vsync_start = (mode.vdisplay as u32 + vsync) as u16;
    mode.vsync_end = (mode.vsync_start as u32 + vsync_width) as u16;
    mode.vtotal = (mode.vdisplay as u32 + vblank) as u16;

    mode.flags = 0;
    mode.flags |= if hsync_positive { DRM_MODE_FLAG_PHSYNC } else { DRM_MODE_FLAG_NHSYNC };
    mode.flags |= if vsync_positive { DRM_MODE_FLAG_PVSYNC } else { DRM_MODE_FLAG_NVSYNC };
    mode.r#type = DRM_MODE_TYPE_DRIVER as u8;

    if timings[3] & 0x80 != 0 {
        mode.r#type |= DRM_MODE_TYPE_PREFERRED as u8;
    }
    drm_mode_set_name(&mut mode);

    mode
}

/// `add_displayid_detailed_1_modes`.
fn add_displayid_detailed_1_modes(connector: &mut drm_connector, edid: &[u8], block: usize) -> i32 {
    let mut num_modes = 0;
    let type_7 = edid[block] as u32 == DATA_BLOCK_2_TYPE_7_DETAILED_TIMING;
    let num_bytes = edid[block + 2] as usize;
    // blocks must be multiple of 20 bytes length
    if num_bytes % 20 != 0 {
        return 0;
    }

    let num_timings = num_bytes / 20;
    for i in 0..num_timings {
        let o = block + DISPLAYID_BLOCK_SIZE + 20 * i;
        let Some(timings) = edid.get(o..o + 20) else { break };

        let newmode = drm_mode_displayid_detailed(timings, type_7);
        drm_mode_probed_add(connector, newmode);
        num_modes += 1;
    }
    num_modes
}

/// `drm_mode_displayid_formula`: a DisplayID type IX or X formula timing
/// (6 bytes).
fn drm_mode_displayid_formula(timings: &[u8], _type_10: bool) -> Option<drm_display_mode> {
    let hactive = le16(timings, 1).wrapping_add(1);
    let vactive = le16(timings, 3).wrapping_add(1);
    let timing_formula = timings[0] & 0x7;

    // TODO: support RB-v2 & RB-v3
    if timing_formula > 1 {
        return None;
    }

    // TODO: support video-optimized refresh rate (only logged)

    let mut mode = drm_cvt_mode(hactive as i32, vactive as i32, timings[5] as i32 + 1, timing_formula == 1, false,
                                false)?;

    // TODO: interpret S3D flags

    mode.r#type = DRM_MODE_TYPE_DRIVER as u8;
    drm_mode_set_name(&mut mode);

    Some(mode)
}

/// `add_displayid_formula_modes`.
fn add_displayid_formula_modes(connector: &mut drm_connector, edid: &[u8], block: usize) -> i32 {
    let mut num_modes = 0;
    let type_10 = edid[block] as u32 == DATA_BLOCK_2_TYPE_10_FORMULA_TIMING;
    let timing_size = 6 + ((edid[block + 1] as usize & 0x70) >> 4);
    let num_bytes = edid[block + 2] as usize;

    // extended blocks are not supported yet
    if timing_size != 6 {
        return 0;
    }

    if num_bytes % timing_size != 0 {
        return 0;
    }

    let num_timings = num_bytes / timing_size;
    for i in 0..num_timings {
        let o = block + DISPLAYID_BLOCK_SIZE + timing_size * i;
        let Some(timings) = edid.get(o..o + 6) else { break };

        let Some(newmode) = drm_mode_displayid_formula(timings, type_10) else { continue };

        drm_mode_probed_add(connector, newmode);
        num_modes += 1;
    }
    num_modes
}

/// `add_displayid_detailed_modes`.
fn add_displayid_detailed_modes(connector: &mut drm_connector, drm_edid: &drm_edid) -> i32 {
    let mut num_modes = 0;

    let mut iter = displayid_iter_edid_begin(Some(drm_edid));
    while let Some(block) = __displayid_iter_next(&mut iter) {
        let tag = drm_edid.edid[block] as u32;
        if tag == DATA_BLOCK_TYPE_1_DETAILED_TIMING || tag == DATA_BLOCK_2_TYPE_7_DETAILED_TIMING {
            num_modes += add_displayid_detailed_1_modes(connector, &drm_edid.edid, block);
        } else if tag == DATA_BLOCK_2_TYPE_9_FORMULA_TIMING || tag == DATA_BLOCK_2_TYPE_10_FORMULA_TIMING {
            num_modes += add_displayid_formula_modes(connector, &drm_edid.edid, block);
        }
    }
    displayid_iter_end(&mut iter);

    num_modes
}

/// `_drm_edid_connector_add_modes`.
fn _drm_edid_connector_add_modes(connector: &mut drm_connector, drm_edid: Option<&drm_edid>) -> i32 {
    let mut num_modes = 0;

    let Some(drm_edid) = drm_edid else { return 0 };

    // EDID spec says modes should be preferred in this order:
    // - preferred detailed mode
    // - other detailed modes from base block
    // - detailed modes from extension blocks
    // - CVT 3-byte code modes
    // - standard timing codes
    // - established timing codes
    // - modes inferred from GTF or CVT range information
    //
    // We get this pretty much right.
    //
    // XXX order for additional mode types in extension blocks?
    num_modes += add_detailed_modes(connector, drm_edid);
    num_modes += add_cvt_modes(connector, drm_edid);
    num_modes += add_standard_modes(connector, drm_edid);
    num_modes += add_established_modes(connector, drm_edid);
    num_modes += add_cea_modes(connector, drm_edid);
    num_modes += add_alternate_cea_modes(connector, drm_edid);
    num_modes += add_displayid_detailed_modes(connector, drm_edid);
    if drm_edid.edid[EDID_FEATURES] as u32 & DRM_EDID_FEATURE_CONTINUOUS_FREQ != 0 {
        num_modes += add_inferred_modes(connector, drm_edid);
    }

    if drm_edid_has_internal_quirk(connector, EDID_QUIRK_PREFER_LARGE_60)
        || drm_edid_has_internal_quirk(connector, EDID_QUIRK_PREFER_LARGE_75)
    {
        edid_fixup_preferred(connector);
    }

    num_modes
}

/// `drm_edid_connector_add_modes`: the modes of the EDID the connector
/// was last updated with (drm_edid_connector_update), added to its probed
/// modes. The number added.
pub fn drm_edid_connector_add_modes(connector: &mut drm_connector) -> i32 {
    let drm_edid = connector.edid_blob.as_deref().and_then(drm_edid_alloc);

    _drm_edid_connector_add_modes(connector, drm_edid.as_ref())
}

/// `drm_edid_override_connector_update`: the modes of an override or
/// firmware EDID; there is none (drm_edid_override_get).
pub fn drm_edid_override_connector_update(connector: &mut drm_connector) -> i32 {
    let mut num_modes = 0;

    if let Some(over) = drm_edid_override_get(connector) {
        if drm_edid_connector_update(connector, Some(&over)) == 0 {
            num_modes = drm_edid_connector_add_modes(connector);
        }
    }

    num_modes
}

/// `drm_add_modes_noedid`: the DMT modes up to hdisplay x vdisplay and
/// 61 Hz, for a connector without EDID.
pub fn drm_add_modes_noedid(connector: &mut drm_connector, hdisplay: u32, vdisplay: u32) -> i32 {
    let mut num_modes = 0;

    for t in drm_dmt_modes.iter() {
        let ptr = mode_from_table(t);

        if hdisplay != 0 && vdisplay != 0 {
            // Only when two are valid, they will be used to check
            // whether the mode should be added to the mode list of
            // the connector.
            if ptr.hdisplay as u32 > hdisplay || ptr.vdisplay as u32 > vdisplay {
                continue;
            }
        }
        if drm_mode_vrefresh(&ptr) > 61 {
            continue;
        }
        drm_mode_probed_add(connector, ptr);
        num_modes += 1;
    }
    num_modes
}

/// `drm_connector_update_edid_property`.
pub fn drm_connector_update_edid_property(connector: &mut drm_connector, edid: Option<&[u8]>) -> i32 {
    let drm_edid = edid.map(drm_edid_legacy_init);
    drm_edid_connector_update(connector, drm_edid.as_ref())
}

/// `is_hdmi2_sink`.
fn is_hdmi2_sink(connector: &drm_connector) -> bool {
    connector.display_info.hdmi.scdc.supported
        || connector.display_info.color_formats & DRM_COLOR_FORMAT_YCBCR420 != 0
}

/// `drm_get_hdmi_aspect_ratio`.
fn drm_get_hdmi_aspect_ratio(video_code: u8) -> u32 {
    edid_4k_modes[video_code as usize].13
}

/// `drm_mode_hdmi_vic`.
fn drm_mode_hdmi_vic(connector: &drm_connector, mode: &drm_display_mode) -> u8 {
    if !connector.display_info.has_hdmi_infoframe {
        return 0;
    }

    // No HDMI VIC when signalling 3D video format
    if mode.flags & DRM_MODE_FLAG_3D_MASK != 0 {
        return 0;
    }

    drm_match_hdmi_mode(mode)
}

/// `drm_mode_cea_vic`.
fn drm_mode_cea_vic(connector: &drm_connector, mode: &drm_display_mode) -> u8 {
    // HDMI spec says if a mode is found in HDMI 1.4b 4K modes
    // we should send its VIC in vendor infoframes, else send the
    // VIC in AVI infoframes. Lets check if this mode is present in
    // HDMI 1.4b 4K modes
    if drm_mode_hdmi_vic(connector, mode) != 0 {
        return 0;
    }

    drm_match_cea_mode(mode)
}

/// `vic_for_avi_infoframe`: a VIC above 64 is sent only to an HDMI 2.0
/// sink or one that lists it.
fn vic_for_avi_infoframe(connector: &drm_connector, vic: u8) -> u8 {
    if !is_hdmi2_sink(connector) && vic > 64 && !cta_vdb_has_vic(connector, vic) {
        return 0;
    }

    vic
}

/// `drm_hdmi_avi_infoframe_from_display_mode`.
pub fn drm_hdmi_avi_infoframe_from_display_mode(frame: &mut hdmi_avi_infoframe, connector: &drm_connector,
                                                mode: &drm_display_mode) -> i32 {
    hdmi_avi_infoframe_init(frame);

    if mode.flags & DRM_MODE_FLAG_DBLCLK != 0 {
        frame.pixel_repeat = 1;
    }

    let vic = drm_mode_cea_vic(connector, mode);
    let hdmi_vic = drm_mode_hdmi_vic(connector, mode);

    frame.picture_aspect = HDMI_PICTURE_ASPECT_NONE;

    // As some drivers don't support atomic, we can't use connector state.
    // So just initialize the frame with default values, just the same way
    // as it's done with other properties here.
    frame.content_type = HDMI_CONTENT_TYPE_GRAPHICS;
    frame.itc = false;

    // Populate picture aspect ratio from either
    // user input (if specified) or from the CEA/HDMI mode lists.
    let mut picture_aspect = mode.picture_aspect_ratio;
    if picture_aspect == HDMI_PICTURE_ASPECT_NONE {
        if vic != 0 {
            picture_aspect = drm_get_cea_aspect_ratio(vic);
        } else if hdmi_vic != 0 {
            picture_aspect = drm_get_hdmi_aspect_ratio(hdmi_vic);
        }
    }

    // The infoframe can't convey anything but none, 4:3
    // and 16:9, so if the user has asked for anything else
    // we can only satisfy it by specifying the right VIC.
    if picture_aspect > HDMI_PICTURE_ASPECT_16_9 {
        if vic != 0 {
            if picture_aspect != drm_get_cea_aspect_ratio(vic) {
                return -EINVAL;
            }
        } else if hdmi_vic != 0 {
            if picture_aspect != drm_get_hdmi_aspect_ratio(hdmi_vic) {
                return -EINVAL;
            }
        } else {
            return -EINVAL;
        }

        picture_aspect = HDMI_PICTURE_ASPECT_NONE;
    }

    frame.video_code = vic_for_avi_infoframe(connector, vic);
    frame.picture_aspect = picture_aspect;
    frame.active_aspect = HDMI_ACTIVE_ASPECT_PICTURE;
    frame.scan_mode = HDMI_SCAN_MODE_UNDERSCAN;

    0
}

/// `drm_default_rgb_quant_range`: all CEA modes other than VIC 1 use
/// limited quantization range.
pub fn drm_default_rgb_quant_range(mode: &drm_display_mode) -> u32 {
    if drm_match_cea_mode(mode) > 1 {
        HDMI_QUANTIZATION_RANGE_LIMITED
    } else {
        HDMI_QUANTIZATION_RANGE_FULL
    }
}

/// `drm_hdmi_avi_infoframe_quant_range`.
pub fn drm_hdmi_avi_infoframe_quant_range(frame: &mut hdmi_avi_infoframe, connector: &drm_connector,
                                          mode: &drm_display_mode, rgb_quant_range: u32) {
    let info = &connector.display_info;

    // CEA-861:
    // "A Source shall not send a non-zero Q value that does not correspond
    //  to the default RGB Quantization Range for the transmitted Picture
    //  unless the Sink indicates support for the Q bit in a Video
    //  Capabilities Data Block."
    //
    // HDMI 2.0 recommends sending non-zero Q when it does match the
    // default RGB quantization range for the mode, even when QS=0.
    if info.rgb_quant_range_selectable || rgb_quant_range == drm_default_rgb_quant_range(mode) {
        frame.quantization_range = rgb_quant_range;
    } else {
        frame.quantization_range = HDMI_QUANTIZATION_RANGE_DEFAULT;
    }

    // CEA-861-F:
    // "When transmitting any RGB colorimetry, the Source should set the
    //  YQ-field to match the RGB Quantization Range being transmitted
    //  (e.g., when Limited Range RGB, set YQ=0 or when Full Range RGB,
    //  set YQ=1) and the Sink shall ignore the YQ-field."
    //
    // Unfortunate certain sinks (eg. VIZ Model 67/E261VA) get confused
    // by non-zero YQ when receiving RGB. There doesn't seem to be any
    // good way to tell which version of CEA-861 the sink supports, so
    // we limit non-zero YQ to HDMI 2.0 sinks only as HDMI 2.0 is based
    // on CEA-861-F.
    if !is_hdmi2_sink(connector) || rgb_quant_range == HDMI_QUANTIZATION_RANGE_LIMITED {
        frame.ycc_quantization_range = HDMI_YCC_QUANTIZATION_RANGE_LIMITED;
    } else {
        frame.ycc_quantization_range = HDMI_YCC_QUANTIZATION_RANGE_FULL;
    }
}

/// `s3d_structure_from_display_mode`.
fn s3d_structure_from_display_mode(mode: &drm_display_mode) -> i32 {
    let layout = mode.flags & DRM_MODE_FLAG_3D_MASK;

    (match layout {
        DRM_MODE_FLAG_3D_FRAME_PACKING => HDMI_3D_STRUCTURE_FRAME_PACKING,
        DRM_MODE_FLAG_3D_FIELD_ALTERNATIVE => HDMI_3D_STRUCTURE_FIELD_ALTERNATIVE,
        DRM_MODE_FLAG_3D_LINE_ALTERNATIVE => HDMI_3D_STRUCTURE_LINE_ALTERNATIVE,
        DRM_MODE_FLAG_3D_SIDE_BY_SIDE_FULL => HDMI_3D_STRUCTURE_SIDE_BY_SIDE_FULL,
        DRM_MODE_FLAG_3D_L_DEPTH => HDMI_3D_STRUCTURE_L_DEPTH,
        DRM_MODE_FLAG_3D_L_DEPTH_GFX_GFX_DEPTH => HDMI_3D_STRUCTURE_L_DEPTH_GFX_GFX_DEPTH,
        DRM_MODE_FLAG_3D_TOP_AND_BOTTOM => HDMI_3D_STRUCTURE_TOP_AND_BOTTOM,
        DRM_MODE_FLAG_3D_SIDE_BY_SIDE_HALF => HDMI_3D_STRUCTURE_SIDE_BY_SIDE_HALF,
        _ => HDMI_3D_STRUCTURE_INVALID,
    }) as i32
}

/// `drm_hdmi_vendor_infoframe_from_display_mode`.
pub fn drm_hdmi_vendor_infoframe_from_display_mode(frame: &mut hdmi_vendor_infoframe, connector: &drm_connector,
                                                   mode: &drm_display_mode) -> i32 {
    if !connector.display_info.has_hdmi_infoframe {
        return -EINVAL;
    }

    let err = hdmi_vendor_infoframe_init(frame);
    if err < 0 {
        return err;
    }

    // Even if it's not absolutely necessary to send the infoframe
    // (ie.vic==0 and s3d_struct==0) we will still send it if we
    // know that the sink can handle it. This is based on a
    // suggestion in HDMI 2.0 Appendix F. Apparently some sinks
    // have trouble realizing that they should switch from 3D to 2D
    // mode if the source simply stops sending the infoframe when
    // it wants to switch from 3D to 2D.
    frame.vic = drm_mode_hdmi_vic(connector, mode);
    frame.s3d_struct = s3d_structure_from_display_mode(mode);

    0
}
