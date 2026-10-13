//! display/intel_bios.c `intel_bios_init` (Linux 6.18.55): the VBT's BDB
//! blocks, general features and definitions, driver features, compression
//! parameters and the child devices with their DDI ports, for a Tiger Lake
//! display (version 12, PCH TGP).
//!
//! Linux keeps the copied blocks and the child devices in kmalloc'd lists;
//! here they live in fixed buffers, and running out of room is an error in
//! the log. Struct layouts, bit-fields and enum values are GCC's (regs.rs).

use crate::intel_opregion;
use crate::linux::{drm_err, say, Line};
use crate::regs::*;
use crate::tables;
use npk_sys::cell::Single;

/// `bdb_blocks[]` in intel_bios.c.
#[derive(Clone, Copy)]
pub struct bdb_block {
    pub section_id: u32,
    pub min_size: u32,
}

/// `struct gmbus_pin`, reduced to whether the pin has a name.
#[derive(Clone, Copy)]
pub struct gmbus_pin {
    pub valid: u32,
}

const CHILD_SIZE: usize = CHILD_DEVICE_CONFIG_SIZE as usize;
const DSC_SIZE: usize = DSC_COMPRESSION_PARAMETERS_ENTRY_SIZE as usize;
const LFP_PTRS_SIZE: usize = BDB_LFP_DATA_PTRS_SIZE as usize;
const LFP_N: usize = BDB_LFP_DATA_PTRS_N as usize;
const N_BDB_BLOCKS: usize = tables::bdb_blocks.len();
const MAX_CHILD_DEVICES: usize = 16;
/// The copied blocks: at most the VBT, plus each block padded to its minimum size.
const BDB_ARENA: usize = intel_opregion::VBT_MAX + 8 * 1024;

/// `struct intel_bios_encoder_data`.
#[derive(Clone, Copy)]
pub struct intel_bios_encoder_data {
    pub child: [u8; CHILD_SIZE],
    pub dsc: Option<[u8; DSC_SIZE]>,
}

impl intel_bios_encoder_data {
    const fn new() -> Self {
        intel_bios_encoder_data { child: [0; CHILD_SIZE], dsc: None }
    }
}

/// The fields of `struct intel_vbt_data` intel_bios_init sets.
#[derive(Clone, Copy)]
pub struct intel_vbt_data {
    pub version: u32,
    pub int_tv_support: u32,
    pub int_crt_support: u32,
    pub int_lvds_support: u32,
    pub lvds_use_ssc: u32,
    pub lvds_ssc_freq: u32,
    pub display_clock_mode: u32,
    pub fdi_rx_polarity_inverted: u32,
    pub orientation: i32,
    pub override_afc_startup: bool,
    pub override_afc_startup_val: u8,
    pub crt_ddc_pin: u32,
    pub display_devices: [intel_bios_encoder_data; MAX_CHILD_DEVICES],
    pub n_display_devices: usize,
}

/// The parts of `struct intel_display` the VBT code reads, and its `vbt`.
pub struct intel_display {
    pub ver: u32,
    pub pch_type: i32,
    pub pipe_mask: u32,
    pub port_mask: u32,
    pub vbt: intel_vbt_data,
}

impl intel_display {
    pub fn new(ver: u32, pch_type: i32, pipe_mask: u32, port_mask: u32) -> Self {
        intel_display {
            ver,
            pch_type,
            pipe_mask,
            port_mask,
            vbt: intel_vbt_data {
                version: 0,
                int_tv_support: 0,
                int_crt_support: 0,
                int_lvds_support: 0,
                lvds_use_ssc: 0,
                lvds_ssc_freq: 0,
                display_clock_mode: 0,
                fdi_rx_polarity_inverted: 0,
                orientation: 0,
                override_afc_startup: false,
                override_afc_startup_val: 0,
                crt_ddc_pin: 0,
                display_devices: [intel_bios_encoder_data::new(); MAX_CHILD_DEVICES],
                n_display_devices: 0,
            },
        }
    }

    fn devices(&self) -> &[intel_bios_encoder_data] {
        &self.vbt.display_devices[..self.vbt.n_display_devices]
    }

    /// `list_add_tail(&devdata->node, &display->vbt.display_devices)`.
    fn add_device(&mut self, devdata: intel_bios_encoder_data) -> bool {
        if self.vbt.n_display_devices == MAX_CHILD_DEVICES {
            drm_err("VBT child device list is full, the rest is dropped");
            return false;
        }
        self.vbt.display_devices[self.vbt.n_display_devices] = devdata;
        self.vbt.n_display_devices += 1;
        true
    }
}

/// `struct bdb_block_entry`: `data` is `arena[start..start + len]`.
#[derive(Clone, Copy)]
struct bdb_block_entry {
    section_id: u32,
    start: usize,
    len: usize,
}

/// `display->vbt.bdb_blocks`.
struct bdb_arena {
    data: [u8; BDB_ARENA],
    used: usize,
    blocks: [bdb_block_entry; N_BDB_BLOCKS],
    n: usize,
}

static BDB: Single<bdb_arena> = Single::new(bdb_arena {
    data: [0; BDB_ARENA],
    used: 0,
    blocks: [bdb_block_entry { section_id: 0, start: 0, len: 0 }; N_BDB_BLOCKS],
    n: 0,
});

fn rd8(b: &[u8], o: usize) -> u32 {
    b.get(o).copied().unwrap_or(0) as u32
}

fn rd16(b: &[u8], o: usize) -> u32 {
    rd8(b, o) | rd8(b, o + 1) << 8
}

fn rd32(b: &[u8], o: usize) -> u32 {
    rd16(b, o) | rd16(b, o + 2) << 16
}

fn wr8(b: &mut [u8], o: usize, v: u32) {
    b[o] = v as u8;
}

fn wr16(b: &mut [u8], o: usize, v: u32) {
    b[o] = v as u8;
    b[o + 1] = (v >> 8) as u8;
}

/// A C bit-field of the struct at `b[o..]`, read little-endian byte by byte.
fn bf(b: &[u8], o: usize, f: Bitfield) -> u32 {
    let n = (f.shift + f.width).div_ceil(8) as usize;
    let mut v = 0u64;
    for i in 0..n {
        v |= (rd8(b, o + f.byte as usize + i) as u64) << (8 * i);
    }
    ((v >> f.shift) & ((1u64 << f.width) - 1)) as u32
}

fn bf_set(b: &mut [u8], o: usize, f: Bitfield, val: u32) {
    let n = (f.shift + f.width).div_ceil(8) as usize;
    let mut v = 0u64;
    for i in 0..n {
        v |= (rd8(b, o + f.byte as usize + i) as u64) << (8 * i);
    }
    let mask = ((1u64 << f.width) - 1) << f.shift;
    v = (v & !mask) | (((val as u64) << f.shift) & mask);
    for i in 0..n {
        b[o + f.byte as usize + i] = (v >> (8 * i)) as u8;
    }
}

fn child(devdata: &intel_bios_encoder_data, f: Bitfield) -> u32 {
    bf(&devdata.child, 0, f)
}

fn letter(c: u32) -> [u8; 1] {
    [c as u8]
}

fn chr(b: &[u8; 1]) -> &str {
    core::str::from_utf8(b).unwrap_or("?")
}

/// `port_name`.
fn port_name(port: i32) -> [u8; 1] {
    letter((b'A' as i32 + port) as u32)
}

/// `_get_blocksize`, with `base` at the block ID.
fn _get_blocksize(b: &[u8], base: usize) -> u32 {
    // The MIPI Sequence Block v3+ has a separate size field.
    if rd8(b, base) == BDB_MIPI_SEQUENCE && rd8(b, base + 3) >= 3 {
        rd32(b, base + 4)
    } else {
        rd16(b, base + 1)
    }
}

/// `get_blocksize`, with `data` just past the block ID and size.
fn get_blocksize(b: &[u8], data: usize) -> u32 {
    _get_blocksize(b, data - 3)
}

/// `find_raw_section`: offset of the block data in `bdb`.
fn find_raw_section(bdb: &[u8], section_id: u32) -> Option<usize> {
    let mut index = rd16(bdb, BDB_HEADER_HEADER_SIZE as usize) as u64;
    let total = rd16(bdb, BDB_HEADER_BDB_SIZE as usize) as u64;

    while index + 3 < total {
        let current_id = rd8(bdb, index as usize);
        let current_size = _get_blocksize(bdb, index as usize) as u64;
        index += 3;

        if index + current_size > total {
            return None;
        }
        if current_id == section_id {
            return Some(index as usize);
        }
        index += current_size;
    }
    None
}

/// `raw_block_offset`.
fn raw_block_offset(bdb: &[u8], section_id: u32) -> u32 {
    find_raw_section(bdb, section_id).map_or(0, |o| o as u32)
}

/// `bdb_find_section`: offset of the copied block's data in the arena.
fn bdb_find_section(a: &bdb_arena, section_id: u32) -> Option<usize> {
    a.blocks[..a.n].iter().find(|e| e.section_id == section_id).map(|e| e.start + 3)
}

/// The first `N` bytes of a copied block; the copy is at least the block's
/// minimum size, so a struct of that size is always in range.
fn bdb_copy_section<const N: usize>(section_id: u32) -> Option<([u8; N], u32)> {
    BDB.with(|a| {
        let off = bdb_find_section(a, section_id)?;
        let mut s = [0u8; N];
        s.copy_from_slice(&a.data[off..off + N]);
        Some((s, get_blocksize(&a.data, off)))
    })
}

// struct lfp_data_ptr_table fields of `ptrs->ptr[i].<table>`.
fn lfp_ptr(i: usize, table: u64) -> usize {
    BDB_LFP_DATA_PTRS_PTR as usize + i * LFP_DATA_PTR_SIZE as usize + table as usize
}

fn tbl_offset(ptrs: &[u8], t: usize) -> i32 {
    rd16(ptrs, t + LFP_DATA_PTR_TABLE_OFFSET as usize) as i32
}

fn tbl_size(ptrs: &[u8], t: usize) -> i32 {
    rd8(ptrs, t + LFP_DATA_PTR_TABLE_TABLE_SIZE as usize) as i32
}

fn set_tbl_offset(ptrs: &mut [u8], t: usize, v: i32) {
    wr16(ptrs, t + LFP_DATA_PTR_TABLE_OFFSET as usize, v as u32);
}

fn set_tbl_size(ptrs: &mut [u8], t: usize, v: i32) {
    wr8(ptrs, t + LFP_DATA_PTR_TABLE_TABLE_SIZE as usize, v as u32);
}

const FP: u64 = LFP_DATA_PTR_FP_TIMING;
const DVO: u64 = LFP_DATA_PTR_DVO_TIMING;
const PNP: u64 = LFP_DATA_PTR_PANEL_PNP_ID;
const PANEL_NAME: usize = BDB_LFP_DATA_PTRS_PANEL_NAME as usize;

/// `lfp_data_min_size`.
fn lfp_data_min_size(a: &bdb_arena) -> usize {
    let Some(p) = bdb_find_section(a, BDB_LFP_DATA_PTRS) else {
        return 0;
    };
    let ptrs = &a.data[p..];
    let mut size = BDB_LFP_DATA_SIZE as usize;
    if tbl_size(ptrs, PANEL_NAME) != 0 {
        size = size.max(tbl_offset(ptrs, PANEL_NAME) as usize + BDB_LFP_DATA_TAIL_SIZE as usize);
    }
    size
}

/// `validate_lfp_data_ptrs`.
fn validate_lfp_data_ptrs(bdb: &[u8], ptrs: &[u8]) -> bool {
    let Some(data_block) = find_raw_section(bdb, BDB_LFP_DATA) else {
        return false;
    };
    let data_block_size = get_blocksize(bdb, data_block) as i32;
    if data_block_size == 0 {
        return false;
    }

    // always 3 indicating the presence of fp_timing+dvo_timing+panel_pnp_id
    if rd8(ptrs, BDB_LFP_DATA_PTRS_NUM_ENTRIES as usize) != 3 {
        return false;
    }

    let mut fp_timing_size = tbl_size(ptrs, lfp_ptr(0, FP));
    let dvo_timing_size = tbl_size(ptrs, lfp_ptr(0, DVO));
    let panel_pnp_id_size = tbl_size(ptrs, lfp_ptr(0, PNP));
    let panel_name_size = tbl_size(ptrs, PANEL_NAME);

    // fp_timing has variable size
    if fp_timing_size < 32
        || dvo_timing_size != BDB_EDID_DTD_SIZE as i32
        || panel_pnp_id_size != BDB_EDID_PNP_ID_SIZE as i32
    {
        return false;
    }

    // panel_name is not present in old VBTs
    if panel_name_size != 0 && panel_name_size != BDB_EDID_PRODUCT_NAME_SIZE as i32 {
        return false;
    }

    let off = |i: usize, t: u64| tbl_offset(ptrs, lfp_ptr(i, t));
    let lfp_data_size = off(1, FP) - off(0, FP);
    if LFP_N as i32 * lfp_data_size > data_block_size {
        return false;
    }

    // make sure the table entries have uniform size
    for i in 1..LFP_N {
        if tbl_size(ptrs, lfp_ptr(i, FP)) != fp_timing_size
            || tbl_size(ptrs, lfp_ptr(i, DVO)) != dvo_timing_size
            || tbl_size(ptrs, lfp_ptr(i, PNP)) != panel_pnp_id_size
        {
            return false;
        }
        if off(i, FP) - off(i - 1, FP) != lfp_data_size
            || off(i, DVO) - off(i - 1, DVO) != lfp_data_size
            || off(i, PNP) - off(i - 1, PNP) != lfp_data_size
        {
            return false;
        }
    }

    // Real VBTs (except vlv/chv) have 6 unaccounted bytes in the fp_timing
    // table, with the 0xffff terminator inside them.
    if fp_timing_size + 6 + dvo_timing_size + panel_pnp_id_size == lfp_data_size {
        fp_timing_size += 6;
    }

    if fp_timing_size + dvo_timing_size + panel_pnp_id_size != lfp_data_size {
        return false;
    }

    if off(0, FP) + fp_timing_size != off(0, DVO)
        || off(0, DVO) + dvo_timing_size != off(0, PNP)
        || off(0, PNP) + panel_pnp_id_size != lfp_data_size
    {
        return false;
    }

    // make sure the tables fit inside the data block
    for i in 0..LFP_N {
        if off(i, FP) + fp_timing_size > data_block_size
            || off(i, DVO) + dvo_timing_size > data_block_size
            || off(i, PNP) + panel_pnp_id_size > data_block_size
        {
            return false;
        }
    }

    if tbl_offset(ptrs, PANEL_NAME) + LFP_N as i32 * panel_name_size > data_block_size {
        return false;
    }

    // make sure fp_timing terminators are present at expected locations
    for i in 0..LFP_N {
        let t = data_block as i32 + off(i, FP) + fp_timing_size - 2;
        if rd16(bdb, t as usize) != 0xffff {
            return false;
        }
    }

    true
}

/// `fixup_lfp_data_ptrs`: make the data table offsets relative to the data block.
fn fixup_lfp_data_ptrs(bdb: &[u8], ptrs: &mut [u8]) -> bool {
    let offset = raw_block_offset(bdb, BDB_LFP_DATA) as i32;

    for i in 0..LFP_N {
        for t in [FP, DVO, PNP] {
            if tbl_offset(ptrs, lfp_ptr(i, t)) < offset {
                return false;
            }
        }
        for t in [FP, DVO, PNP] {
            let v = tbl_offset(ptrs, lfp_ptr(i, t));
            set_tbl_offset(ptrs, lfp_ptr(i, t), v - offset);
        }
    }

    if tbl_size(ptrs, PANEL_NAME) != 0 {
        if tbl_offset(ptrs, PANEL_NAME) < offset {
            return false;
        }
        let v = tbl_offset(ptrs, PANEL_NAME);
        set_tbl_offset(ptrs, PANEL_NAME, v - offset);
    }

    validate_lfp_data_ptrs(bdb, ptrs)
}

/// `make_lfp_data_ptr`.
fn make_lfp_data_ptr(ptrs: &mut [u8], table: usize, table_size: i32, total_size: i32) -> i32 {
    if total_size < table_size {
        return total_size;
    }
    set_tbl_size(ptrs, table, table_size);
    set_tbl_offset(ptrs, table, total_size - table_size);
    total_size - table_size
}

/// `next_lfp_data_ptr`.
fn next_lfp_data_ptr(ptrs: &mut [u8], next: usize, prev: usize, size: i32) {
    let (ts, o) = (tbl_size(ptrs, prev), tbl_offset(ptrs, prev));
    set_tbl_size(ptrs, next, ts);
    set_tbl_offset(ptrs, next, o + size);
}

/// `generate_lfp_data_ptrs`: a block 41 with its 3-byte header.
fn generate_lfp_data_ptrs(version: u32, bdb: &[u8]) -> Option<[u8; 3 + LFP_PTRS_SIZE]> {
    // The hardcoded fp_timing_size is only valid for modernish VBTs.
    if version < 155 {
        return None;
    }
    let fp_timing_size = 38;

    let block = find_raw_section(bdb, BDB_LFP_DATA)?;

    say("Generating LFP data table pointers");

    let block_size = get_blocksize(bdb, block) as i32;

    let mut size = fp_timing_size + BDB_EDID_DTD_SIZE as i32 + BDB_EDID_PNP_ID_SIZE as i32;
    if size * LFP_N as i32 > block_size {
        return None;
    }

    let mut ptrs_block = [0u8; 3 + LFP_PTRS_SIZE];
    wr8(&mut ptrs_block, 0, BDB_LFP_DATA_PTRS);
    wr16(&mut ptrs_block, 1, LFP_PTRS_SIZE as u32);
    let ptrs = &mut ptrs_block[3..];

    size = make_lfp_data_ptr(ptrs, lfp_ptr(0, PNP), BDB_EDID_PNP_ID_SIZE as i32, size);
    size = make_lfp_data_ptr(ptrs, lfp_ptr(0, DVO), BDB_EDID_DTD_SIZE as i32, size);
    size = make_lfp_data_ptr(ptrs, lfp_ptr(0, FP), fp_timing_size, size);

    let ne = BDB_LFP_DATA_PTRS_NUM_ENTRIES as usize;
    for t in [FP, DVO, PNP] {
        if tbl_size(ptrs, lfp_ptr(0, t)) != 0 {
            ptrs[ne] += 1;
        }
    }

    if size != 0 || ptrs[ne] != 3 {
        return None;
    }

    size = fp_timing_size + BDB_EDID_DTD_SIZE as i32 + BDB_EDID_PNP_ID_SIZE as i32;
    for i in 1..LFP_N {
        for t in [FP, DVO, PNP] {
            next_lfp_data_ptr(ptrs, lfp_ptr(i, t), lfp_ptr(i - 1, t), size);
        }
    }

    let table_size = BDB_EDID_PRODUCT_NAME_SIZE as i32;
    if LFP_N as i32 * (size + table_size) <= block_size {
        set_tbl_size(ptrs, PANEL_NAME, table_size);
        set_tbl_offset(ptrs, PANEL_NAME, size * LFP_N as i32);
    }

    let offset = block as i32;
    for i in 0..LFP_N {
        for t in [FP, DVO, PNP] {
            let v = tbl_offset(ptrs, lfp_ptr(i, t));
            set_tbl_offset(ptrs, lfp_ptr(i, t), v + offset);
        }
    }
    if tbl_size(ptrs, PANEL_NAME) != 0 {
        let v = tbl_offset(ptrs, PANEL_NAME);
        set_tbl_offset(ptrs, PANEL_NAME, v + offset);
    }

    Some(ptrs_block)
}

/// `init_bdb_block`.
fn init_bdb_block(a: &mut bdb_arena, version: u32, bdb: &[u8], section_id: u32, min_size: usize) {
    let block = find_raw_section(bdb, section_id);

    // Modern VBTs lack the LFP data table pointers block, make one up
    let temp_block = if block.is_none() && section_id == BDB_LFP_DATA_PTRS {
        generate_lfp_data_ptrs(version, bdb)
    } else {
        None
    };
    let (src, block): (&[u8], usize) = match (block, &temp_block) {
        (Some(b), _) => (bdb, b),
        (None, Some(t)) => (&t[..], 3),
        (None, None) => return,
    };

    if min_size == 0 {
        Line::new().s("error: Block ").dec(section_id as u64).s(" min_size is zero").say();
    }

    let mut block_size = get_blocksize(src, block) as usize;

    // Version number and new block size are part of the header for MIPI
    // sequence block v3+.
    if section_id == BDB_MIPI_SEQUENCE && rd8(src, block) >= 3 {
        block_size += 5;
    }

    let len = min_size.max(block_size) + 3;
    if a.n == a.blocks.len() || len > a.data.len() - a.used {
        Line::new().s("error: no room to copy BDB block ").dec(section_id as u64)
            .s(" (").dec(len as u64).s(" bytes)").say();
        return;
    }
    let start = a.used;
    let entry = &mut a.data[start..start + len];
    entry.fill(0);
    let from = block - 3;
    let n = (block_size + 3).min(src.len().saturating_sub(from));
    entry[..n].copy_from_slice(&src[from..from + n]);

    if section_id == BDB_LFP_DATA_PTRS && !fixup_lfp_data_ptrs(bdb, &mut entry[3..]) {
        drm_err("VBT has malformed LFP data table pointers");
        return;
    }

    a.blocks[a.n] = bdb_block_entry { section_id, start, len };
    a.n += 1;
    a.used += len;
}

/// `init_bdb_blocks`. The per-block "Found BDB block" lines become one.
fn init_bdb_blocks(version: u32, bdb: &[u8]) {
    BDB.with(|a| {
        a.used = 0;
        a.n = 0;
        for b in tables::bdb_blocks.iter() {
            let section_id = b.section_id;
            let mut min_size = b.min_size as usize;

            if section_id == BDB_LFP_DATA {
                min_size = lfp_data_min_size(a);
            }

            init_bdb_block(a, version, bdb, section_id, min_size);
        }
        let mut l = Line::new();
        l.s("Found BDB blocks");
        for e in &a.blocks[..a.n] {
            l.s(" ").dec(e.section_id as u64).s("/").dec((e.len - 3) as u64);
        }
        l.say();
    });
}

/// `intel_bios_ssc_frequency`: DISPLAY_VER 12 takes the default branch.
fn intel_bios_ssc_frequency(_display: &intel_display, alternate: bool) -> u32 {
    if alternate { SSC_FREQ_TGL_ALTERNATE } else { SSC_FREQ_TGL_DEFAULT }
}

/// `HAS_DDI`: the display info flag of `tgl_desc`.
fn HAS_DDI(_display: &intel_display) -> bool {
    TGL_DISPLAY_HAS_DDI != 0
}

/// `HAS_PCH_SPLIT`.
fn HAS_PCH_SPLIT(display: &intel_display) -> bool {
    display.pch_type != PCH_NONE as i32
}

/// `parse_general_features`.
fn parse_general_features(display: &mut intel_display) {
    const SIZE: usize = BDB_GENERAL_FEATURES_SIZE as usize;
    let Some((general, _)) = bdb_copy_section::<SIZE>(BDB_GENERAL_FEATURES) else {
        return;
    };
    let g = |f| bf(&general, 0, f);

    display.vbt.int_tv_support = g(GENERAL_INT_TV_SUPPORT);
    // int_crt_support can't be trusted on earlier platforms
    if display.vbt.version >= 155 && HAS_DDI(display) {
        display.vbt.int_crt_support = g(GENERAL_INT_CRT_SUPPORT);
    }
    display.vbt.lvds_use_ssc = g(GENERAL_ENABLE_SSC);
    display.vbt.lvds_ssc_freq = intel_bios_ssc_frequency(display, g(GENERAL_SSC_FREQ) != 0);
    display.vbt.display_clock_mode = g(GENERAL_DISPLAY_CLOCK_MODE);
    display.vbt.fdi_rx_polarity_inverted = g(GENERAL_FDI_RX_POLARITY_INVERTED);
    if display.vbt.version >= 181 {
        display.vbt.orientation = if g(GENERAL_ROTATE_180) != 0 {
            DRM_MODE_PANEL_ORIENTATION_BOTTOM_UP as i32
        } else {
            DRM_MODE_PANEL_ORIENTATION_NORMAL as i32
        };
    } else {
        display.vbt.orientation = DRM_MODE_PANEL_ORIENTATION_UNKNOWN;
    }

    let afc = g(GENERAL_AFC_STARTUP_CONFIG);
    if display.vbt.version >= 249 && afc != 0 {
        display.vbt.override_afc_startup = true;
        display.vbt.override_afc_startup_val = if afc == 1 { 0 } else { 7 };
    }

    let v = &display.vbt;
    Line::new().s("BDB_GENERAL_FEATURES int_tv_support ").dec(v.int_tv_support as u64)
        .s(" int_crt_support ").dec(v.int_crt_support as u64)
        .s(" lvds_use_ssc ").dec(v.lvds_use_ssc as u64)
        .s(" lvds_ssc_freq ").dec(v.lvds_ssc_freq as u64)
        .s(" display_clock_mode ").dec(v.display_clock_mode as u64)
        .s(" fdi_rx_polarity_inverted ").dec(v.fdi_rx_polarity_inverted as u64).say();
}

/// `parse_sdvo_device_mapping`: SDVO exists on display versions 3 to 7 only.
fn parse_sdvo_device_mapping(display: &intel_display) {
    if !(3..=7).contains(&display.ver) {
        say("Skipping SDVO device mapping");
        return;
    }
    drm_err("SDVO device mapping is not ported");
}

/// `parse_driver_features`.
fn parse_driver_features(display: &mut intel_display) {
    const SIZE: usize = BDB_DRIVER_FEATURES_SIZE as usize;
    let Some((driver, _)) = bdb_copy_section::<SIZE>(BDB_DRIVER_FEATURES) else {
        return;
    };

    // DISPLAY_VER >= 5. BDB_DRIVER_FEATURE_INT_SDVO_LVDS counts as eDP, as
    // real world VBTs use it.
    if bf(&driver, 0, DRIVER_LVDS_CONFIG) != BDB_DRIVER_FEATURE_INT_LVDS {
        display.vbt.int_lvds_support = 0;
    }
}

/// `parse_compression_parameters`.
fn parse_compression_parameters(display: &mut intel_display) {
    const SIZE: usize = BDB_COMPRESSION_PARAMETERS_SIZE as usize;
    if display.vbt.version < 198 {
        return;
    }

    let params = bdb_copy_section::<SIZE>(BDB_COMPRESSION_PARAMETERS);
    if let Some((p, block_size)) = &params {
        // Sanity checks
        if rd16(p, BDB_COMPRESSION_PARAMETERS_ENTRY_SIZE as usize) != DSC_SIZE as u32 {
            say("VBT: unsupported compression param entry size");
            return;
        }
        if (*block_size as u16 as usize) < SIZE {
            say("VBT: expected 16 compression param entries");
            return;
        }
    }

    for devdata in display.vbt.display_devices[..display.vbt.n_display_devices].iter_mut() {
        if child(devdata, CHILD_COMPRESSION_ENABLE) == 0 {
            continue;
        }
        let Some((p, _)) = &params else {
            say("VBT: compression params not available");
            continue;
        };
        if child(devdata, CHILD_COMPRESSION_METHOD_CPS) != 0 {
            say("VBT: CPS compression not supported");
            continue;
        }

        let index = child(devdata, CHILD_COMPRESSION_STRUCTURE_INDEX) as usize;
        let o = BDB_COMPRESSION_PARAMETERS_DATA as usize + index * DSC_SIZE;
        let mut dsc = [0u8; DSC_SIZE];
        dsc.copy_from_slice(&p[o..o + DSC_SIZE]);
        devdata.dsc = Some(dsc);
    }
}

/// `translate_iboost`.
fn translate_iboost(val: u32) -> u32 {
    let mapping = [IBOOST_0, IBOOST_1, IBOOST_2];
    if val as usize >= mapping.len() {
        Line::new().s("Unsupported I_boost value found in VBT (").dec(val as u64)
            .s("), display may not work properly").say();
        return 0;
    }
    mapping[val as usize]
}

/// `map_ddc_pin`: PCH TGP on Tiger Lake is below PCH_DG1, not RKL, display
/// version 12 and at least PCH_ICP, so `icp_ddc_pin_map`.
fn map_ddc_pin(_display: &intel_display, vbt_pin: u32) -> u32 {
    let ddc_pin_map = &tables::icp_ddc_pin_map;
    for (i, &p) in ddc_pin_map.iter().enumerate() {
        if p as u32 == vbt_pin {
            return i as u32;
        }
    }
    Line::new().s("Ignoring alternate pin: VBT claims DDC pin ").dec(vbt_pin as u64)
        .s(", which is not valid for this platform").say();
    0
}

/// `dvo_port_type`.
fn dvo_port_type(dvo_port: u32) -> u32 {
    match dvo_port {
        DVO_PORT_HDMIA | DVO_PORT_HDMIB | DVO_PORT_HDMIC | DVO_PORT_HDMID | DVO_PORT_HDMIE
        | DVO_PORT_HDMIF | DVO_PORT_HDMIG | DVO_PORT_HDMIH | DVO_PORT_HDMII => DVO_PORT_HDMIA,
        DVO_PORT_DPA | DVO_PORT_DPB | DVO_PORT_DPC | DVO_PORT_DPD | DVO_PORT_DPE | DVO_PORT_DPF
        | DVO_PORT_DPG | DVO_PORT_DPH | DVO_PORT_DPI => DVO_PORT_DPA,
        DVO_PORT_MIPIA | DVO_PORT_MIPIB | DVO_PORT_MIPIC | DVO_PORT_MIPID => DVO_PORT_MIPIA,
        _ => dvo_port,
    }
}

/// `__dvo_port_to_port`.
fn __dvo_port_to_port(port_mapping: &[[i32; 3]], dvo_port: u32) -> i32 {
    for (port, dvo) in port_mapping.iter().enumerate() {
        for &d in dvo {
            if d == -1 {
                break;
            }
            if dvo_port as i32 == d {
                return port as i32;
            }
        }
    }
    PORT_NONE
}

/// `port_mapping` of `dvo_port_to_port`: each DDI port can have more than
/// one value in the "DVO Port" field.
const PORT_MAPPING: [[i32; 3]; PORT_I as usize + 1] = {
    let mut m = [[0; 3]; PORT_I as usize + 1];
    m[PORT_A as usize] = [DVO_PORT_HDMIA as i32, DVO_PORT_DPA as i32, -1];
    m[PORT_B as usize] = [DVO_PORT_HDMIB as i32, DVO_PORT_DPB as i32, -1];
    m[PORT_C as usize] = [DVO_PORT_HDMIC as i32, DVO_PORT_DPC as i32, -1];
    m[PORT_D as usize] = [DVO_PORT_HDMID as i32, DVO_PORT_DPD as i32, -1];
    m[PORT_E as usize] = [DVO_PORT_HDMIE as i32, DVO_PORT_DPE as i32, DVO_PORT_CRT as i32];
    m[PORT_F as usize] = [DVO_PORT_HDMIF as i32, DVO_PORT_DPF as i32, -1];
    m[PORT_G as usize] = [DVO_PORT_HDMIG as i32, DVO_PORT_DPG as i32, -1];
    m[PORT_H as usize] = [DVO_PORT_HDMIH as i32, DVO_PORT_DPH as i32, -1];
    m[PORT_I as usize] = [DVO_PORT_HDMII as i32, DVO_PORT_DPI as i32, -1];
    m
};

/// `dvo_port_to_port`: display version 12, not ADL-S, DG1 or RKL, so the
/// plain `port_mapping`.
fn dvo_port_to_port(_display: &intel_display, dvo_port: u32) -> i32 {
    __dvo_port_to_port(&PORT_MAPPING, dvo_port)
}

/// `dsi_dvo_port_to_port`.
fn dsi_dvo_port_to_port(display: &intel_display, dvo_port: u32) -> i32 {
    match dvo_port {
        DVO_PORT_MIPIA => PORT_A as i32,
        DVO_PORT_MIPIC => {
            if display.ver >= 11 { PORT_B as i32 } else { PORT_C as i32 }
        }
        _ => PORT_NONE,
    }
}

/// `intel_bios_encoder_port`.
pub fn intel_bios_encoder_port(display: &intel_display, devdata: &intel_bios_encoder_data) -> i32 {
    let dvo_port = child(devdata, CHILD_DVO_PORT);
    let mut port = dvo_port_to_port(display, dvo_port);
    if port == PORT_NONE && display.ver >= 11 {
        port = dsi_dvo_port_to_port(display, dvo_port);
    }
    port
}

/// `parse_bdb_230_dp_max_link_rate`.
fn parse_bdb_230_dp_max_link_rate(vbt_max_link_rate: u32) -> u32 {
    match vbt_max_link_rate {
        BDB_230_VBT_DP_MAX_LINK_RATE_UHBR20 => DP_RATE_230_UHBR20,
        BDB_230_VBT_DP_MAX_LINK_RATE_UHBR13P5 => DP_RATE_230_UHBR13P5,
        BDB_230_VBT_DP_MAX_LINK_RATE_UHBR10 => DP_RATE_230_UHBR10,
        BDB_230_VBT_DP_MAX_LINK_RATE_HBR3 => DP_RATE_230_HBR3,
        BDB_230_VBT_DP_MAX_LINK_RATE_HBR2 => DP_RATE_230_HBR2,
        BDB_230_VBT_DP_MAX_LINK_RATE_HBR => DP_RATE_230_HBR,
        BDB_230_VBT_DP_MAX_LINK_RATE_LBR => DP_RATE_230_LBR,
        _ => 0,
    }
}

/// `parse_bdb_216_dp_max_link_rate`.
fn parse_bdb_216_dp_max_link_rate(vbt_max_link_rate: u32) -> u32 {
    match vbt_max_link_rate {
        BDB_216_VBT_DP_MAX_LINK_RATE_HBR2 => DP_RATE_216_HBR2,
        BDB_216_VBT_DP_MAX_LINK_RATE_HBR => DP_RATE_216_HBR,
        BDB_216_VBT_DP_MAX_LINK_RATE_LBR => DP_RATE_216_LBR,
        _ => DP_RATE_216_HBR3,
    }
}

/// `intel_bios_dp_max_link_rate`.
pub fn intel_bios_dp_max_link_rate(display: &intel_display, devdata: &intel_bios_encoder_data) -> u32 {
    if display.vbt.version < 216 {
        return 0;
    }
    let rate = child(devdata, CHILD_DP_MAX_LINK_RATE);
    if display.vbt.version >= 230 {
        parse_bdb_230_dp_max_link_rate(rate)
    } else {
        parse_bdb_216_dp_max_link_rate(rate)
    }
}

/// `intel_bios_dp_max_lane_count`.
pub fn intel_bios_dp_max_lane_count(display: &intel_display, devdata: &intel_bios_encoder_data) -> u32 {
    if display.vbt.version < 244 {
        return 0;
    }
    child(devdata, CHILD_DP_MAX_LANE_COUNT) + 1
}

/// `sanitize_device_type`: only port A before display version 12.
fn sanitize_device_type(display: &intel_display, devdata: &mut intel_bios_encoder_data, port: i32) {
    if port != PORT_A as i32 || display.ver >= 12 {
        return;
    }
    if !intel_bios_encoder_supports_dvi(devdata) {
        return;
    }
    let is_hdmi = intel_bios_encoder_supports_hdmi(devdata);
    Line::new().s("VBT claims port A supports DVI").s(if is_hdmi { "/HDMI" } else { "" })
        .s(", ignoring").say();

    let t = child(devdata, CHILD_DEVICE_TYPE);
    let t = (t & !DEVICE_TYPE_TMDS_DVI_SIGNALING) | DEVICE_TYPE_NOT_HDMI_OUTPUT;
    bf_set(&mut devdata.child, 0, CHILD_DEVICE_TYPE, t);
}

/// `sanitize_hdmi_level_shift`: the clamp is for Broadwell only.
fn sanitize_hdmi_level_shift(_display: &intel_display, devdata: &intel_bios_encoder_data, _port: i32) {
    if !intel_bios_encoder_supports_dvi(devdata) {
        return;
    }
}

fn device_type(devdata: &intel_bios_encoder_data) -> u32 {
    child(devdata, CHILD_DEVICE_TYPE)
}

fn intel_bios_encoder_supports_crt(devdata: &intel_bios_encoder_data) -> bool {
    device_type(devdata) & DEVICE_TYPE_ANALOG_OUTPUT != 0
}

pub fn intel_bios_encoder_supports_dvi(devdata: &intel_bios_encoder_data) -> bool {
    device_type(devdata) & DEVICE_TYPE_TMDS_DVI_SIGNALING != 0
}

pub fn intel_bios_encoder_supports_hdmi(devdata: &intel_bios_encoder_data) -> bool {
    intel_bios_encoder_supports_dvi(devdata) && device_type(devdata) & DEVICE_TYPE_NOT_HDMI_OUTPUT == 0
}

pub fn intel_bios_encoder_supports_dp(devdata: &intel_bios_encoder_data) -> bool {
    device_type(devdata) & DEVICE_TYPE_DISPLAYPORT_OUTPUT != 0
}

pub fn intel_bios_encoder_supports_edp(devdata: &intel_bios_encoder_data) -> bool {
    intel_bios_encoder_supports_dp(devdata) && device_type(devdata) & DEVICE_TYPE_INTERNAL_CONNECTOR != 0
}

pub fn intel_bios_encoder_supports_dsi(devdata: &intel_bios_encoder_data) -> bool {
    device_type(devdata) & DEVICE_TYPE_MIPI_OUTPUT != 0
}

/// `intel_bios_encoder_is_lspcon`: HAS_LSPCON is display versions 9 and 10.
pub fn intel_bios_encoder_is_lspcon(display: &intel_display, devdata: &intel_bios_encoder_data) -> bool {
    (9..=10).contains(&display.ver) && child(devdata, CHILD_LSPCON) != 0
}

/// `intel_bios_hdmi_level_shift`: an index in the HDMI/DVI DDI buffer
/// translation table, or -1.
pub fn intel_bios_hdmi_level_shift(display: &intel_display, devdata: &intel_bios_encoder_data) -> i32 {
    if display.vbt.version < 158 || display.ver >= 14 {
        return -1;
    }
    child(devdata, CHILD_HDMI_LEVEL_SHIFTER_VALUE) as i32
}

/// `intel_bios_hdmi_max_tmds_clock`.
pub fn intel_bios_hdmi_max_tmds_clock(display: &intel_display, devdata: &intel_bios_encoder_data) -> u32 {
    if display.vbt.version < 204 {
        return 0;
    }
    match child(devdata, CHILD_HDMI_MAX_DATA_RATE) {
        HDMI_MAX_DATA_RATE_PLATFORM => 0,
        HDMI_MAX_DATA_RATE_594 => 594000,
        HDMI_MAX_DATA_RATE_340 => 340000,
        HDMI_MAX_DATA_RATE_300 => 300000,
        HDMI_MAX_DATA_RATE_297 => 297000,
        HDMI_MAX_DATA_RATE_165 => 165000,
        v => {
            Line::new().s("error: MISSING_CASE hdmi_max_data_rate ").dec(v as u64).say();
            0
        }
    }
}

/// `is_port_valid`: the port F check is for Ice Lake only.
fn is_port_valid(_display: &intel_display, _port: i32) -> bool {
    true
}

/// `intel_bios_encoder_supports_dp_dual_mode`.
pub fn intel_bios_encoder_supports_dp_dual_mode(devdata: &intel_bios_encoder_data) -> bool {
    if !intel_bios_encoder_supports_dp(devdata) || !intel_bios_encoder_supports_hdmi(devdata) {
        return false;
    }
    let dvo_port = child(devdata, CHILD_DVO_PORT);
    if dvo_port_type(dvo_port) == DVO_PORT_DPA {
        return true;
    }
    // Only accept a HDMI dvo_port as DP++ if it has an AUX channel
    dvo_port_type(dvo_port) == DVO_PORT_HDMIA && child(devdata, CHILD_AUX_CHANNEL) != 0
}

/// `map_aux_ch`: display version 12, not ADL-S, DG1 or RKL, so `direct_aux_ch_map`.
fn map_aux_ch(_display: &intel_display, aux_channel: u32) -> i32 {
    let aux_ch_map = &tables::direct_aux_ch_map;
    for (i, &a) in aux_ch_map.iter().enumerate() {
        if a as u32 == aux_channel {
            return i as i32;
        }
    }
    Line::new().s("Ignoring alternate AUX CH: VBT claims AUX ").hex(aux_channel)
        .s(", which is not valid for this platform").say();
    AUX_CH_NONE
}

/// `intel_bios_dp_aux_ch`.
pub fn intel_bios_dp_aux_ch(display: &intel_display, devdata: &intel_bios_encoder_data) -> i32 {
    let aux_channel = child(devdata, CHILD_AUX_CHANNEL);
    if aux_channel == 0 {
        return AUX_CH_NONE;
    }
    map_aux_ch(display, aux_channel)
}

/// `intel_bios_dp_boost_level`.
pub fn intel_bios_dp_boost_level(display: &intel_display, devdata: &intel_bios_encoder_data) -> u32 {
    if display.vbt.version < 196 || child(devdata, CHILD_IBOOST) == 0 {
        return 0;
    }
    translate_iboost(child(devdata, CHILD_DP_IBOOST_LEVEL))
}

/// `intel_bios_hdmi_boost_level`.
pub fn intel_bios_hdmi_boost_level(display: &intel_display, devdata: &intel_bios_encoder_data) -> u32 {
    if display.vbt.version < 196 || child(devdata, CHILD_IBOOST) == 0 {
        return 0;
    }
    translate_iboost(child(devdata, CHILD_HDMI_IBOOST_LEVEL))
}

/// `intel_bios_hdmi_ddc_pin`.
pub fn intel_bios_hdmi_ddc_pin(display: &intel_display, devdata: &intel_bios_encoder_data) -> u32 {
    let ddc_pin = child(devdata, CHILD_DDC_PIN);
    if ddc_pin == 0 {
        return 0;
    }
    map_ddc_pin(display, ddc_pin)
}

pub fn intel_bios_encoder_supports_typec_usb(display: &intel_display, devdata: &intel_bios_encoder_data) -> bool {
    display.vbt.version >= 195 && child(devdata, CHILD_DP_USB_TYPE_C) != 0
}

pub fn intel_bios_encoder_supports_tbt(display: &intel_display, devdata: &intel_bios_encoder_data) -> bool {
    display.vbt.version >= 209 && child(devdata, CHILD_TBT) != 0
}

pub fn intel_bios_encoder_lane_reversal(devdata: &intel_bios_encoder_data) -> bool {
    child(devdata, CHILD_LANE_REVERSAL) != 0
}

pub fn intel_bios_encoder_hpd_invert(devdata: &intel_bios_encoder_data) -> bool {
    child(devdata, CHILD_HPD_INVERT) != 0
}

/// `aux_ch_name` of intel_dp_aux.c, display version 12.
fn aux_ch_name(l: &mut Line, aux_ch: i32) {
    if aux_ch >= AUX_CH_USBC1 as i32 {
        l.s("USBC").s(chr(&letter((b'1' as i32 + aux_ch - AUX_CH_USBC1 as i32) as u32)));
    } else {
        l.s(chr(&letter((b'A' as i32 + aux_ch) as u32)));
    }
}

/// `print_ddi_port`, with the AUX channel and DDC pin on a line of their own.
fn print_ddi_port(display: &intel_display, devdata: &intel_bios_encoder_data) {
    let port = intel_bios_encoder_port(display, devdata);
    if port == PORT_NONE {
        return;
    }
    let p = port_name(port);
    let p = chr(&p);

    let b = |v: bool| v as u64;
    Line::new().s("Port ").s(p).s(" VBT info: CRT:").dec(b(intel_bios_encoder_supports_crt(devdata)))
        .s(" DVI:").dec(b(intel_bios_encoder_supports_dvi(devdata)))
        .s(" HDMI:").dec(b(intel_bios_encoder_supports_hdmi(devdata)))
        .s(" DP:").dec(b(intel_bios_encoder_supports_dp(devdata)))
        .s(" eDP:").dec(b(intel_bios_encoder_supports_edp(devdata)))
        .s(" DSI:").dec(b(intel_bios_encoder_supports_dsi(devdata)))
        .s(" DP++:").dec(b(intel_bios_encoder_supports_dp_dual_mode(devdata)))
        .s(" LSPCON:").dec(b(intel_bios_encoder_is_lspcon(display, devdata)))
        .s(" USB-Type-C:").dec(b(intel_bios_encoder_supports_typec_usb(display, devdata)))
        .s(" TBT:").dec(b(intel_bios_encoder_supports_tbt(display, devdata)))
        .s(" DSC:").dec(b(devdata.dsc.is_some())).say();

    let mut l = Line::new();
    l.s("Port ").s(p).s(" VBT AUX CH ");
    let aux_ch = intel_bios_dp_aux_ch(display, devdata);
    if aux_ch == AUX_CH_NONE {
        l.s("none");
    } else {
        aux_ch_name(&mut l, aux_ch);
    }
    l.s(", DDC pin ").dec(intel_bios_hdmi_ddc_pin(display, devdata) as u64).say();

    let hdmi_level_shift = intel_bios_hdmi_level_shift(display, devdata);
    if hdmi_level_shift >= 0 {
        Line::new().s("Port ").s(p).s(" VBT HDMI level shift: ").dec(hdmi_level_shift as u64).say();
    }

    let max_tmds_clock = intel_bios_hdmi_max_tmds_clock(display, devdata);
    if max_tmds_clock != 0 {
        Line::new().s("Port ").s(p).s(" VBT HDMI max TMDS clock: ").dec(max_tmds_clock as u64)
            .s(" kHz").say();
    }

    // I_boost config for SKL and above
    let dp_boost_level = intel_bios_dp_boost_level(display, devdata);
    if dp_boost_level != 0 {
        Line::new().s("Port ").s(p).s(" VBT (e)DP boost level: ").dec(dp_boost_level as u64).say();
    }

    let hdmi_boost_level = intel_bios_hdmi_boost_level(display, devdata);
    if hdmi_boost_level != 0 {
        Line::new().s("Port ").s(p).s(" VBT HDMI boost level: ").dec(hdmi_boost_level as u64).say();
    }

    let dp_max_link_rate = intel_bios_dp_max_link_rate(display, devdata);
    if dp_max_link_rate != 0 {
        Line::new().s("Port ").s(p).s(" VBT DP max link rate: ").dec(dp_max_link_rate as u64).say();
    }

    // VBT vswing/preemph tables are not implemented, in Linux either.
    if child(devdata, CHILD_USE_VBT_VSWING) != 0 {
        Line::new().s("error: Port ").s(p).s(" asks to use VBT vswing/preemph tables").say();
    }
}

/// `parse_ddi_port`.
fn parse_ddi_port(display: &intel_display, devdata: &mut intel_bios_encoder_data) {
    let port = intel_bios_encoder_port(display, devdata);
    if port == PORT_NONE {
        return;
    }

    if !is_port_valid(display, port) {
        Line::new().s("VBT reports port ").s(chr(&port_name(port)))
            .s(" as supported, but that can't be true: skipping").say();
        return;
    }

    sanitize_device_type(display, devdata, port);
    sanitize_hdmi_level_shift(display, devdata, port);
}

/// `parse_ddi_ports`: has_ddi_port_info is display version 5 and up.
fn parse_ddi_ports(display: &mut intel_display) {
    for i in 0..display.vbt.n_display_devices {
        let mut devdata = display.vbt.display_devices[i];
        parse_ddi_port(display, &mut devdata);
        display.vbt.display_devices[i] = devdata;
    }

    for devdata in display.devices() {
        print_ddi_port(display, devdata);
    }
}

/// `child_device_expected_size`.
fn child_device_expected_size(version: u32) -> i32 {
    if version > 263 {
        -(ENOENT as i32)
    } else if version >= 263 {
        44
    } else if version >= 256 {
        40
    } else if version >= 216 {
        39
    } else if version >= 196 {
        38
    } else if version >= 195 {
        37
    } else if version >= 111 {
        LEGACY_CHILD_DEVICE_CONFIG_SIZE as i32
    } else if version >= 106 {
        27
    } else {
        22
    }
}

/// `child_device_size_valid`.
fn child_device_size_valid(display: &intel_display, size: i32) -> bool {
    let mut expected_size = child_device_expected_size(display.vbt.version);
    if expected_size < 0 {
        expected_size = CHILD_SIZE as i32;
        Line::new().s("Expected child device config size for VBT version ").dec(display.vbt.version as u64)
            .s(" not known; assuming ").dec(expected_size as u64).say();
    }

    // Flag an error for unexpected size, but continue anyway.
    if size != expected_size {
        Line::new().s("error: Unexpected child device config size ").dec(size as u64)
            .s(" (expected ").dec(expected_size as u64).s(" for VBT version ")
            .dec(display.vbt.version as u64).s(")").say();
    }

    // The legacy sized child device config is the minimum we need.
    if size < LEGACY_CHILD_DEVICE_CONFIG_SIZE as i32 {
        Line::new().s("Child device config size ").dec(size as u64).s(" is too small.").say();
        return false;
    }

    true
}

/// `intel_gmbus_is_valid_pin` of intel_gmbus.c: `get_gmbus_pin` picks
/// `gmbus_pins_icp` for PCH TGP (at least PCH_ICP, below PCH_DG1).
fn intel_gmbus_is_valid_pin(_display: &intel_display, pin: u32) -> bool {
    let pins = &tables::gmbus_pins_icp;
    (pin as usize) < pins.len() && pins[pin as usize].valid != 0
}

/// `parse_general_definitions`.
fn parse_general_definitions(display: &mut intel_display) {
    BDB.with(|a| {
        let Some(defs) = bdb_find_section(a, BDB_GENERAL_DEFINITIONS) else {
            say("No general definition block is found, no devices defined.");
            return;
        };
        let d = &a.data;

        let block_size = get_blocksize(d, defs) as u16 as usize;
        if block_size < BDB_GENERAL_DEFINITIONS_SIZE as usize {
            Line::new().s("General definitions block too small (").dec(block_size as u64).s(")").say();
            return;
        }

        let bus_pin = rd8(d, defs + BDB_GENERAL_DEFINITIONS_CRT_DDC_GMBUS_PIN as usize);
        Line::new().s("crt_ddc_bus_pin: ").dec(bus_pin as u64).say();
        if intel_gmbus_is_valid_pin(display, bus_pin) {
            display.vbt.crt_ddc_pin = bus_pin;
        }

        let child_dev_size = rd8(d, defs + BDB_GENERAL_DEFINITIONS_CHILD_DEV_SIZE as usize) as usize;
        if !child_device_size_valid(display, child_dev_size as i32) {
            return;
        }

        // get the number of child device
        let child_device_num = (block_size - BDB_GENERAL_DEFINITIONS_SIZE as usize) / child_dev_size;

        for i in 0..child_device_num {
            let child_ptr = defs + BDB_GENERAL_DEFINITIONS_DEVICES as usize + i * child_dev_size;
            let device_type = bf(d, child_ptr, CHILD_DEVICE_TYPE);
            if device_type == 0 {
                continue;
            }

            Line::new().s("Found VBT child device with type ").hex(device_type).say();

            // Copy as much as we know and is available; reads depend on the version.
            let mut devdata = intel_bios_encoder_data::new();
            let n = child_dev_size.min(CHILD_SIZE);
            devdata.child[..n].copy_from_slice(&d[child_ptr..child_ptr + n]);

            if !display.add_device(devdata) {
                break;
            }
        }

        if display.vbt.n_display_devices == 0 {
            say("no child dev is parsed from VBT");
        }
    });
}

/// `init_vbt_defaults`: common defaults which may be overridden by VBT.
fn init_vbt_defaults(display: &mut intel_display) {
    display.vbt.crt_ddc_pin = GMBUS_PIN_VGADDC;

    // general features
    display.vbt.int_tv_support = 1;
    display.vbt.int_crt_support = 1;

    // driver features
    display.vbt.int_lvds_support = 1;

    // Default to using SSC
    display.vbt.lvds_use_ssc = 1;
    // Core/SandyBridge/IvyBridge use alternative (120MHz) reference clock for LVDS.
    display.vbt.lvds_ssc_freq = intel_bios_ssc_frequency(display, !HAS_PCH_SPLIT(display));
    Line::new().s("Set default to SSC at ").dec(display.vbt.lvds_ssc_freq as u64).s(" kHz").say();
}

/// `intel_port_to_phy` of intel_display.c: display version 12, not ADL-S,
/// DG1, RKL, JSL or EHL, so the last branch.
fn intel_port_to_phy(_display: &intel_display, port: u32) -> u32 {
    PHY_A + port - PORT_A
}

/// `intel_phy_is_tc` of intel_display.c: Tiger Lake, integrated.
fn intel_phy_is_tc(_display: &intel_display, phy: u32) -> bool {
    (PHY_D..=PHY_I).contains(&phy)
}

/// `init_vbt_missing_defaults`: defaults to initialize only if there is no VBT.
fn init_vbt_missing_defaults(display: &mut intel_display) {
    let ports = display.port_mask;

    if !HAS_DDI(display) {
        return;
    }

    for port in PORT_A..I915_MAX_PORTS {
        if ports & (1 << port) == 0 {
            continue;
        }
        let phy = intel_port_to_phy(display, port);

        // VBT has the TypeC mode (native,TBT/USB) and we don't want to detect it.
        if intel_phy_is_tc(display, phy) {
            continue;
        }

        // Create fake child device config
        let mut devdata = intel_bios_encoder_data::new();
        let c = &mut devdata.child;

        let dvo_port = if port == PORT_F {
            DVO_PORT_HDMIF
        } else if port == PORT_E {
            DVO_PORT_HDMIE
        } else {
            DVO_PORT_HDMIA + port
        };
        bf_set(c, 0, CHILD_DVO_PORT, dvo_port);

        let mut device_type = 0;
        if port != PORT_A && port != PORT_E {
            device_type |= DEVICE_TYPE_TMDS_DVI_SIGNALING;
        }
        if port != PORT_E {
            device_type |= DEVICE_TYPE_DISPLAYPORT_OUTPUT;
        }
        if port == PORT_A {
            device_type |= DEVICE_TYPE_INTERNAL_CONNECTOR;
        }
        bf_set(c, 0, CHILD_DEVICE_TYPE, device_type);

        if !display.add_device(devdata) {
            break;
        }

        Line::new().s("Generating default VBT child device with type ").hex(device_type)
            .s(" on port ").s(chr(&port_name(port as i32))).say();
    }

    // Bypass some minimum baseline VBT version checks
    display.vbt.version = 155;
}

/// `intel_bios_get_vbt`: the OpRegion's copy. There is no `vbt_firmware`
/// parameter, a dGPU's SPI flash does not apply, and the PCI ROM needs a
/// kernel call this module does not have.
fn intel_bios_get_vbt(opregion: Option<&intel_opregion::intel_opregion>) -> Option<usize> {
    let len = opregion.map_or(0, |op| op.vbt_len);
    if len == 0 {
        say("VBT in the PCI ROM is not reachable from a module");
        return None;
    }
    Some(len)
}

/// `intel_bios_init`: find the VBT and initialize settings from it, or
/// defaults if there is none.
pub fn intel_bios_init(display: &mut intel_display, opregion: Option<&intel_opregion::intel_opregion>) {
    display.vbt.n_display_devices = 0;

    // HAS_DISPLAY
    if display.pipe_mask == 0 {
        say("Skipping VBT init due to disabled display.");
        return;
    }

    init_vbt_defaults(display);

    let vbt = intel_bios_get_vbt(opregion);

    if let Some(len) = vbt {
        intel_opregion::vbt(len, |vbt| {
            let bdb_offset = rd32(vbt, VBT_HEADER_BDB_OFFSET as usize) as usize;
            let bdb = &vbt[bdb_offset..];
            display.vbt.version = rd16(bdb, BDB_HEADER_VERSION as usize);

            let sig = &vbt[..VBT_HEADER_SIGNATURE_SIZE as usize];
            let sig_len = sig.iter().position(|&c| c == 0).unwrap_or(sig.len());
            Line::new().s("VBT signature \"").s(core::str::from_utf8(&sig[..sig_len]).unwrap_or("?"))
                .s("\", BDB version ").dec(display.vbt.version as u64).say();

            init_bdb_blocks(display.vbt.version, bdb);
        });

        // Grab useful general definitions
        parse_general_features(display);
        parse_general_definitions(display);
        parse_driver_features(display);

        // Depends on child device list
        parse_compression_parameters(display);
    } else {
        say("Failed to find VBIOS tables (VBT)");
        init_vbt_missing_defaults(display);
    }

    // Further processing on pre-parsed or generated child device data
    parse_sdvo_device_mapping(display);
    parse_ddi_ports(display);
}
