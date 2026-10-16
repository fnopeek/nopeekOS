//! drm_edid.c (Linux 6.18.55): reading an EDID over a DP AUX channel's
//! DDC, the monitor-audio check and `drm_edid_to_eld`.
//!
//! What a display server needs from an EDID (modes, colour, HDR, VRR) has
//! no consumer yet and is left out, as are EDID overrides and the HF-EEODB
//! extension count. CTA data blocks are read from CTA extensions; those
//! inside DisplayID extensions are not.

use alloc::vec::Vec;

use crate::intel_dp_aux::{drm_dp_aux, drm_dp_i2c_xfer, i2c_msg};
use crate::regs::*;

const ELEN: usize = EDID_LENGTH as usize;

/// `edid_header`.
const EDID_HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

/// `edid_fixup`: minimum number of valid header bytes.
const EDID_FIXUP: usize = 6;

/// `EDID_DETAILED_TIMINGS`, at `struct edid` offset 54, 18 bytes each.
const EDID_DETAILED_TIMINGS: usize = 4;
const EDID_DETAILED_OFFSET: usize = 54;
const DETAILED_TIMING_SIZE: usize = 18;
/// `struct edid`: `extensions` and `checksum`, `version`, `mfg_id`, `prod_code`.
const EDID_EXTENSIONS: usize = 126;
const EDID_CHECKSUM: usize = 127;
const EDID_VERSION: usize = 18;
const EDID_MFG_ID: usize = 8;
const EDID_PROD_CODE: usize = 10;

/// `enum edid_block_status`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum edid_block_status {
    EDID_BLOCK_OK,
    EDID_BLOCK_READ_FAIL,
    EDID_BLOCK_ZERO,
    EDID_BLOCK_HEADER_CORRUPT,
    EDID_BLOCK_HEADER_REPAIR,
    EDID_BLOCK_HEADER_FIXED,
    EDID_BLOCK_CHECKSUM,
    EDID_BLOCK_VERSION,
}
use edid_block_status::*;

/// A read EDID: base block and extensions, `EDID_LENGTH` each.
pub struct drm_edid {
    pub edid: Vec<u8>,
}

impl drm_edid {
    /// `drm_edid_block_count`.
    fn block_count(&self) -> usize {
        ((self.edid[EDID_EXTENSIONS] as usize) + 1).min(self.edid.len() / ELEN)
    }

    /// `drm_edid_iter_for_each`: every block, the base included.
    fn blocks(&self) -> impl Iterator<Item = &[u8]> {
        self.edid.chunks(ELEN).take(self.block_count())
    }
}

/// `drm_do_probe_ddc_edid` over the AUX channel's I2C adapter.
fn drm_do_probe_ddc_edid(aux: &mut drm_dp_aux, buf: &mut [u8], block: usize) -> bool {
    let mut start = [(block * ELEN) as u8];
    let mut segment = [(block >> 1) as u8];
    let xfers = if segment[0] != 0 { 3 } else { 2 };
    let mut retries = 5;

    // A bit-banged bus can NAK under load; retry the block a few times.
    loop {
        let ret = {
            let mut msgs = [
                i2c_msg { addr: DDC_SEGMENT_ADDR, flags: 0, buf: &mut segment },
                i2c_msg { addr: DDC_ADDR, flags: 0, buf: &mut start },
                i2c_msg { addr: DDC_ADDR, flags: I2C_M_RD, buf: &mut *buf },
            ];
            // Avoid sending the segment addr to not upset non-compliant
            // DDC monitors.
            drm_dp_i2c_xfer(aux, &mut msgs[3 - xfers..])
        };
        if ret == -6 {
            // ENXIO: no adapter, nothing to retry.
            return false;
        }
        if ret == xfers as i32 {
            return true;
        }
        retries -= 1;
        if retries == 0 {
            return false;
        }
    }
}

/// `drm_edid_header_is_valid`.
fn drm_edid_header_is_valid(block: &[u8]) -> usize {
    EDID_HEADER.iter().zip(block).filter(|(a, b)| a == b).count()
}

/// `edid_block_compute_checksum`.
fn edid_block_compute_checksum(block: &[u8]) -> u8 {
    let csum = block[..ELEN - 1].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    0u8.wrapping_sub(csum)
}

/// `edid_block_check`.
fn edid_block_check(block: &[u8], is_base_block: bool) -> edid_block_status {
    let zero = block.iter().all(|&b| b == 0);
    if is_base_block {
        let score = drm_edid_header_is_valid(block);
        if score < EDID_FIXUP {
            return if zero { EDID_BLOCK_ZERO } else { EDID_BLOCK_HEADER_CORRUPT };
        }
        if score < 8 {
            return EDID_BLOCK_HEADER_REPAIR;
        }
    }
    if edid_block_compute_checksum(block) != block[EDID_CHECKSUM] {
        return if zero { EDID_BLOCK_ZERO } else { EDID_BLOCK_CHECKSUM };
    }
    if is_base_block && block[EDID_VERSION] != 1 {
        return EDID_BLOCK_VERSION;
    }
    EDID_BLOCK_OK
}

/// `edid_block_status_valid`.
fn edid_block_status_valid(status: edid_block_status, tag: u8) -> bool {
    status == EDID_BLOCK_OK || status == EDID_BLOCK_HEADER_FIXED
        || (status == EDID_BLOCK_CHECKSUM && tag as u32 == CEA_EXT)
}

/// `edid_block_read`.
fn edid_block_read(aux: &mut drm_dp_aux, block: &mut [u8], block_num: usize) -> edid_block_status {
    let is_base_block = block_num == 0;
    let mut status = EDID_BLOCK_READ_FAIL;
    for attempt in 0..4 {
        if !drm_do_probe_ddc_edid(aux, block, block_num) {
            return EDID_BLOCK_READ_FAIL;
        }
        status = edid_block_check(block, is_base_block);
        if status == EDID_BLOCK_HEADER_REPAIR {
            block[..8].copy_from_slice(&EDID_HEADER);
            // Retry with fixed header, update status if that worked.
            status = edid_block_check(block, is_base_block);
            if status == EDID_BLOCK_OK {
                status = EDID_BLOCK_HEADER_FIXED;
            }
        }
        if edid_block_status_valid(status, block[0]) {
            break;
        }
        // Fail early for unrepairable base block all zeros.
        if attempt == 0 && is_base_block && status == EDID_BLOCK_ZERO {
            break;
        }
    }
    status
}

/// `drm_edid_read_ddc` → `_drm_do_get_edid`, with the invalid extension
/// blocks filtered out (`edid_filter_invalid_blocks`).
pub fn drm_edid_read_ddc(aux: &mut drm_dp_aux) -> Option<drm_edid> {
    let mut edid = alloc::vec![0u8; ELEN];
    let status = edid_block_read(aux, &mut edid, 0);
    if !edid_block_status_valid(status, edid[0]) {
        crate::linux::Line::new().s("EDID: base block not valid (").dec(status as u64).s(")").say();
        return None;
    }

    let num_blocks = edid[EDID_EXTENSIONS] as usize + 1;
    edid.resize(num_blocks * ELEN, 0);
    let mut keep = alloc::vec![true; num_blocks];
    for i in 1..num_blocks {
        let status = edid_block_read(aux, &mut edid[i * ELEN..(i + 1) * ELEN], i);
        if !edid_block_status_valid(status, edid[i * ELEN]) {
            if status == EDID_BLOCK_READ_FAIL {
                return None;
            }
            keep[i] = false;
        }
    }
    if keep.iter().any(|k| !k) {
        let mut out = Vec::with_capacity(edid.len());
        for (i, k) in keep.iter().enumerate() {
            if *k {
                out.extend_from_slice(&edid[i * ELEN..(i + 1) * ELEN]);
            }
        }
        let valid = out.len() / ELEN;
        out[EDID_EXTENSIONS] = (valid - 1) as u8;
        // The base block's checksum follows the new extension count.
        out[EDID_CHECKSUM] = edid_block_compute_checksum(&out[..ELEN]);
        edid = out;
    }
    Some(drm_edid { edid })
}

/// `struct cea_db` access: tag and payload length.
fn cea_db_tag(db: &[u8]) -> u32 {
    (db[0] >> 5) as u32
}

fn cea_db_payload_len(db: &[u8]) -> usize {
    (db[0] & 0x1f) as usize
}

/// `cea_db_collection_size`.
fn cea_db_collection_size(cta: &[u8]) -> usize {
    let d = cta[2] as usize;
    if !(4..=127).contains(&d) { 0 } else { d - 4 }
}

/// `cea_db_iter_for_each` over the CTA extensions (revision 3 and up):
/// each data block, header byte first.
fn cea_dbs(drm_edid: &drm_edid) -> Vec<&[u8]> {
    let mut out = Vec::new();
    for ext in drm_edid.blocks() {
        // Only support CTA Extension revision 3+.
        if ext[0] as u32 != CEA_EXT || ext[1] < 3 {
            continue;
        }
        let size = cea_db_collection_size(ext);
        if size == 0 {
            continue;
        }
        let end = 4 + size;
        let mut index = 4;
        while index + 1 <= end && index + 1 + cea_db_payload_len(&ext[index..]) <= end {
            let len = cea_db_payload_len(&ext[index..]);
            out.push(&ext[index..index + 1 + len]);
            index += 1 + len;
        }
    }
    out
}

/// `cea_db_is_hdmi_vsdb`: the HDMI 1.4 vendor block (OUI 00-0C-03).
fn cea_db_is_hdmi_vsdb(db: &[u8]) -> bool {
    cea_db_tag(db) == CTA_DB_VENDOR && cea_db_payload_len(db) >= 5
        && db[1] == 0x03 && db[2] == 0x0c && db[3] == 0x00
}

/// `is_display_descriptor`.
fn is_display_descriptor(d: &[u8], type_: u32) -> bool {
    d[0] == 0 && d[1] == 0 && d[2] == 0 && d[3] as u32 == type_
}

/// `drm_for_each_detailed_block`: the base block's four, then those of each
/// CTA extension. VTB extensions are not walked.
fn detailed_blocks(drm_edid: &drm_edid) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let base = &drm_edid.edid[..ELEN];
    for i in 0..EDID_DETAILED_TIMINGS {
        let o = EDID_DETAILED_OFFSET + i * DETAILED_TIMING_SIZE;
        out.push(&base[o..o + DETAILED_TIMING_SIZE]);
    }
    for ext in drm_edid.blocks() {
        if ext[0] as u32 != CEA_EXT {
            continue;
        }
        // cea_for_each_detailed_block
        let d = ext[2] as usize;
        if !(4..=127).contains(&d) {
            continue;
        }
        let n = (127 - d) / DETAILED_TIMING_SIZE;
        for i in 0..n {
            let o = d + DETAILED_TIMING_SIZE * i;
            out.push(&ext[o..o + DETAILED_TIMING_SIZE]);
        }
    }
    out
}

/// `get_monitor_name`: the last monitor name descriptor, up to 13 bytes.
fn get_monitor_name(drm_edid: &drm_edid, name: &mut [u8]) -> usize {
    let mut edid_name: Option<&[u8]> = None;
    for d in detailed_blocks(drm_edid) {
        if is_display_descriptor(d, EDID_DETAIL_MONITOR_NAME) {
            edid_name = Some(&d[5..18]);
        }
    }
    let mut mnl = 0;
    if let Some(s) = edid_name {
        while mnl < 13 {
            if s[mnl] == 0x0a {
                break;
            }
            name[mnl] = s[mnl];
            mnl += 1;
        }
    }
    mnl
}

/// `drm_edid_get_monitor_name`, as text.
pub fn monitor_name(drm_edid: &drm_edid, out: &mut [u8; 13]) -> usize {
    get_monitor_name(drm_edid, out)
}

/// `info->cea_rev` (`drm_parse_cea_ext`): the revision of the first CTA
/// extension.
fn cea_rev(drm_edid: &drm_edid) -> u8 {
    drm_edid.blocks().find(|e| e[0] as u32 == CEA_EXT).map(|e| e[1]).unwrap_or(0)
}

/// `_drm_detect_monitor_audio`.
pub fn drm_detect_monitor_audio(drm_edid: &drm_edid) -> bool {
    if drm_edid.blocks().any(|e| e[0] as u32 == CEA_EXT && e[3] as u32 & EDID_BASIC_AUDIO != 0) {
        return true;
    }
    cea_dbs(drm_edid).iter().any(|db| cea_db_tag(db) == CTA_DB_AUDIO)
}

/// `drm_parse_hdmi_vsdb_audio`: the HDMI VSDB's latency fields
/// (`connector->latency_present`, `video_latency`, `audio_latency`).
#[derive(Clone, Copy, Default)]
pub struct av_latency {
    pub latency_present: [bool; 2],
    pub video_latency: [i32; 2],
    pub audio_latency: [i32; 2],
}

/// `hdmi_vsdb_latency_present`, `hdmi_vsdb_i_latency_present`.
fn hdmi_vsdb_latency_present(db: &[u8]) -> bool {
    db[8] & (1 << 7) != 0
}

fn hdmi_vsdb_i_latency_present(db: &[u8]) -> bool {
    hdmi_vsdb_latency_present(db) && db[8] & (1 << 6) != 0
}

fn drm_parse_hdmi_vsdb_audio(db: &[u8], eld: &mut [u8], lat: &mut av_latency) {
    let len = cea_db_payload_len(db);
    if len >= 6 && db[6] & (1 << 7) != 0 {
        eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |= DRM_ELD_SUPPORTS_AI as u8;
    }
    if len >= 10 && hdmi_vsdb_latency_present(db) {
        lat.latency_present[0] = true;
        lat.video_latency[0] = db[9] as i32;
        lat.audio_latency[0] = db[10] as i32;
    }
    if len >= 12 && hdmi_vsdb_i_latency_present(db) {
        lat.latency_present[1] = true;
        lat.video_latency[1] = db[11] as i32;
        lat.audio_latency[1] = db[12] as i32;
    }
}

/// `drm_eld_calc_baseline_block_size`.
fn drm_eld_calc_baseline_block_size(eld: &[u8]) -> usize {
    let mnl = (eld[DRM_ELD_CEA_EDID_VER_MNL as usize] as u32 & DRM_ELD_MNL_MASK) >> DRM_ELD_MNL_SHIFT;
    let sad_count = (eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] as u32 & DRM_ELD_SAD_COUNT_MASK) >> DRM_ELD_SAD_COUNT_SHIFT;
    (DRM_ELD_MONITOR_NAME_STRING - DRM_ELD_HEADER_BLOCK_SIZE + mnl + sad_count * 3) as usize
}

/// `drm_eld_size`.
pub fn drm_eld_size(eld: &[u8]) -> usize {
    DRM_ELD_HEADER_BLOCK_SIZE as usize + eld[DRM_ELD_BASELINE_ELD_LEN as usize] as usize * 4
}

/// `drm_edid_to_eld`. The HDCP and Port_ID fields are the graphics
/// driver's to fill in.
pub fn drm_edid_to_eld(drm_edid: &drm_edid, is_dp: bool, eld: &mut [u8; MAX_ELD_BYTES as usize],
                       lat: &mut av_latency) -> usize {
    // clear_eld
    eld.fill(0);
    *lat = av_latency::default();

    let name_at = DRM_ELD_MONITOR_NAME_STRING as usize;
    let mnl = get_monitor_name(drm_edid, &mut eld[name_at..name_at + 13]);

    eld[DRM_ELD_CEA_EDID_VER_MNL as usize] = cea_rev(drm_edid) << DRM_ELD_CEA_EDID_VER_SHIFT;
    eld[DRM_ELD_CEA_EDID_VER_MNL as usize] |= mnl as u8;

    eld[DRM_ELD_VER as usize] = DRM_ELD_VER_CEA861D as u8;

    eld[DRM_ELD_MANUFACTURER_NAME0 as usize] = drm_edid.edid[EDID_MFG_ID];
    eld[DRM_ELD_MANUFACTURER_NAME1 as usize] = drm_edid.edid[EDID_MFG_ID + 1];
    eld[DRM_ELD_PRODUCT_CODE0 as usize] = drm_edid.edid[EDID_PROD_CODE];
    eld[DRM_ELD_PRODUCT_CODE1 as usize] = drm_edid.edid[EDID_PROD_CODE + 1];

    let mut total_sad_count = 0usize;
    for db in cea_dbs(drm_edid) {
        let data = &db[1..];
        let len = cea_db_payload_len(db);
        match cea_db_tag(db) {
            CTA_DB_AUDIO => {
                // Audio Data Block, contains SADs.
                let sad_count = (len / 3).min(15 - total_sad_count);
                if sad_count >= 1 {
                    // DRM_ELD_CEA_SAD(mnl, total_sad_count)
                    let at = 20 + mnl + 3 * total_sad_count;
                    eld[at..at + sad_count * 3].copy_from_slice(&data[..sad_count * 3]);
                }
                total_sad_count += sad_count;
            }
            CTA_DB_SPEAKER => {
                // Speaker Allocation Data Block.
                if len >= 1 {
                    eld[DRM_ELD_SPEAKER as usize] = data[0];
                }
            }
            CTA_DB_VENDOR => {
                // HDMI Vendor-Specific Data Block.
                if cea_db_is_hdmi_vsdb(db) {
                    drm_parse_hdmi_vsdb_audio(db, &mut eld[..], lat);
                }
            }
            _ => {}
        }
    }

    eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |= (total_sad_count << DRM_ELD_SAD_COUNT_SHIFT) as u8;
    eld[DRM_ELD_SAD_COUNT_CONN_TYPE as usize] |=
        if is_dp { DRM_ELD_CONN_TYPE_DP as u8 } else { DRM_ELD_CONN_TYPE_HDMI as u8 };

    eld[DRM_ELD_BASELINE_ELD_LEN as usize] = drm_eld_calc_baseline_block_size(eld).div_ceil(4) as u8;
    total_sad_count
}
