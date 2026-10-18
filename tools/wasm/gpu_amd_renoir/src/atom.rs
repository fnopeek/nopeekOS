//! amdgpu/atom.c (Linux 6.18.55): the ATOM image, parsed. The command
//! table interpreter is `atom_exec`.
//!
//! Linux reads the image through `CU8`/`CU16`/`CU32` without bounds; a read
//! past the end of the image gives 0 here.

use crate::linux::Line;
use crate::regs::*;
use crate::tables::atom_iio_len;
use alloc::vec;
use alloc::vec::Vec;

const STRLEN_N: usize = STRLEN_NORMAL as usize;
const STRLEN_L: usize = STRLEN_LONG as usize;

/// `struct atom_context`, the part the parser fills, plus the scratch area
/// `amdgpu_atomfirmware_allocate_fb_scratch` sets.
pub struct atom_context {
    pub bios: Vec<u8>,
    pub cmd_table: u32,
    pub data_table: u32,
    pub iio: Vec<u16>,
    pub scratch: Vec<u32>,
    pub scratch_size_bytes: i32,
    pub name: [u8; STRLEN_L],
    pub vbios_pn: [u8; STRLEN_L],
    pub version: u32,
    pub vbios_ver_str: [u8; STRLEN_N],
    pub date: [u8; STRLEN_N],
    pub build_num: [u8; STRLEN_N],
    pub reg_block: u16,
    pub io_mode: u32,
    pub data_block: u16,
    pub fb_base: u32,
    pub divmul: [u32; 2],
    pub shift: u8,
    pub io_attr: u16,
    pub cs_equal: bool,
    pub cs_above: bool,
    pub execute_depth: u32,
}

impl atom_context {
    /// `CU8`.
    pub fn cu8(&self, idx: u32) -> u8 {
        self.bios.get(idx as usize).copied().unwrap_or(0)
    }

    /// `CU16`.
    pub fn cu16(&self, idx: u32) -> u16 {
        u16::from_le_bytes([self.cu8(idx), self.cu8(idx + 1)])
    }

    /// `CU32`.
    pub fn cu32(&self, idx: u32) -> u32 {
        u32::from_le_bytes([self.cu8(idx), self.cu8(idx + 1), self.cu8(idx + 2), self.cu8(idx + 3)])
    }

    /// `strncmp(CSTR(idx), s, strlen(s)) == 0`.
    fn cstr_starts_with(&self, idx: u32, s: &[u8]) -> bool {
        s.iter().enumerate().all(|(i, &c)| self.cu8(idx + i as u32) == c)
    }
}

/// The NUL-terminated start of a fixed string field.
pub fn cstr(b: &[u8]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
}

/// `atom_index_iio`.
fn atom_index_iio(ctx: &mut atom_context, mut base: u32) {
    ctx.iio = vec![0u16; 256];
    let end = ctx.bios.len() as u32;
    while ctx.cu8(base) as u32 == ATOM_IIO_START {
        let i = ctx.cu8(base + 1) as usize;
        ctx.iio[i] = (base + 2) as u16;
        base += 2;
        while ctx.cu8(base) as u32 != ATOM_IIO_END {
            let op = ctx.cu8(base) as usize;
            if op >= atom_iio_len.len() || base >= end {
                return;
            }
            base += atom_iio_len[op] as u32;
        }
        base += 3;
    }
}

/// `atom_get_vbios_name`.
fn atom_get_vbios_name(ctx: &mut atom_context) {
    let str_num = ctx.cu8(OFFSET_TO_GET_ATOMBIOS_NUMBER_OF_STRINGS);
    let mut c_ptr;
    if str_num != 0 {
        c_ptr = ctx.cu16(OFFSET_TO_GET_ATOMBIOS_STRING_START) as u32;
    } else {
        // do not know where to find name
        ctx.name[..7].copy_from_slice(b"--N/A--");
        ctx.name[7] = 0;
        return;
    }

    // skip the atombios strings, usually 4
    // 1st is P/N, 2nd is ASIC, 3rd is PCI type, 4th is Memory type
    let end = ctx.bios.len() as u32;
    for _ in 0..str_num {
        while ctx.cu8(c_ptr) != 0 && c_ptr < end {
            c_ptr += 1;
        }
        c_ptr += 1;
    }

    // skip the following 2 chars: 0x0D 0x0A
    c_ptr += 2;

    let mut name_size = 0;
    while name_size < STRLEN_L - 1 && ctx.cu8(c_ptr + name_size as u32) != 0 {
        name_size += 1;
    }
    for i in 0..name_size {
        ctx.name[i] = ctx.cu8(c_ptr + i as u32);
    }
    let mut back = name_size;
    while back > 0 && ctx.name[back - 1] == b' ' {
        back -= 1;
    }
    ctx.name[back] = 0;
}

/// `atom_get_vbios_date`.
fn atom_get_vbios_date(ctx: &mut atom_context) {
    let d = |i: u32| ctx.cu8(OFFSET_TO_VBIOS_DATE + i);
    let date = [
        b'2', b'0', d(6), d(7), b'/', d(0), d(1), b'/', d(3), d(4), b' ', d(9), d(10), d(11), d(12), d(13), 0,
    ];
    ctx.date[..date.len()].copy_from_slice(&date);
}

/// `atom_find_str_in_rom`: where `s` (cut at `maxlen`) starts in the image,
/// searched from `start` through `end`.
fn atom_find_str_in_rom(ctx: &atom_context, s: &[u8], mut start: u32, end: u32, maxlen: usize) -> Option<u32> {
    let str_len = s.len().min(maxlen);
    while start <= end {
        let mut str_off = 0;
        while str_off < str_len {
            if s[str_off] != ctx.cu8(start + str_off as u32) {
                break;
            }
            str_off += 1;
        }
        if str_off == str_len {
            return Some(start);
        }
        start += 1;
    }
    None
}

/// `atom_get_vbios_pn`.
fn atom_get_vbios_pn(ctx: &mut atom_context) {
    let mut vbios_str = if ctx.cu8(OFFSET_TO_GET_ATOMBIOS_NUMBER_OF_STRINGS) != 0 {
        Some(ctx.cu16(OFFSET_TO_GET_ATOMBIOS_STRING_START) as u32)
    } else {
        Some(OFFSET_TO_VBIOS_PART_NUMBER)
    };

    if vbios_str.is_some_and(|p| ctx.cu8(p) == 0) {
        // Linux steps a NULL result by sizeof(BIOS_ATOM_PREFIX) - 1 and reads
        // from that address; here no part number is taken.
        vbios_str = atom_find_str_in_rom(ctx, BIOS_ATOM_PREFIX, 3, 1024, 64);
    }
    if let Some(p) = vbios_str {
        let p = if ctx.cu8(p) == 0 { p + 1 } else { p };
        let mut count = 0;
        while count < BIOS_STRING_LENGTH as usize {
            let c = ctx.cu8(p + count as u32);
            if !(b' '..=b'z').contains(&c) {
                break;
            }
            ctx.vbios_pn[count] = c;
            count += 1;
        }
        ctx.vbios_pn[count] = 0;
    }

    Line::new().s("ATOM BIOS: ").bytes(cstr(&ctx.vbios_pn)).say();
}

/// `atom_get_vbios_version`.
fn atom_get_vbios_version(ctx: &mut atom_context) {
    // Search from strings offset if it's present
    let mut start = ctx.cu16(OFFSET_TO_GET_ATOMBIOS_STRING_START) as u32;
    // Search till atom rom header start point
    let mut end = ctx.cu16(OFFSET_TO_ATOM_ROM_HEADER_POINTER) as u32;
    // Use hardcoded offsets, if the offsets are not populated
    if end <= start {
        start = 3;
        end = 1024;
    }
    // find anchor ATOMBIOSBK-AMD
    match atom_find_str_in_rom(ctx, BIOS_VERSION_PREFIX, start, end, 64) {
        Some(p) => {
            // skip ATOMBIOSBK-AMD VER
            let p = p + 18;
            for i in 0..STRLEN_N {
                ctx.vbios_ver_str[i] = ctx.cu8(p + i as u32);
            }
        }
        None => ctx.vbios_ver_str[0] = 0,
    }
}

/// `atom_get_vbios_build`.
fn atom_get_vbios_build(ctx: &mut atom_context) {
    let base = ctx.cu16(ATOM_ROM_TABLE_PTR) as u32;
    let atom_rom_hdr = base;
    let mut s = ctx.cu16(base + ATOM_ROM_CFG_PTR) as u32;
    // Skip config string
    while s < atom_rom_hdr {
        let c = ctx.cu8(s);
        s += 1;
        if c == 0 {
            break;
        }
    }
    // Skip change list string
    while s < atom_rom_hdr {
        let c = ctx.cu8(s);
        s += 1;
        if c == 0 {
            break;
        }
    }
    let len = (atom_rom_hdr.saturating_sub(s) as usize).min(STRLEN_N);
    if len != 0 {
        // strscpy(build_num, str, len)
        let mut i = 0;
        while i + 1 < len && ctx.cu8(s + i as u32) != 0 {
            ctx.build_num[i] = ctx.cu8(s + i as u32);
            i += 1;
        }
        ctx.build_num[i] = 0;
    }
}

/// `amdgpu_atom_parse`.
pub fn amdgpu_atom_parse(bios: Vec<u8>) -> Option<atom_context> {
    let mut ctx = atom_context {
        bios,
        cmd_table: 0,
        data_table: 0,
        iio: Vec::new(),
        scratch: Vec::new(),
        scratch_size_bytes: 0,
        name: [0; STRLEN_L],
        vbios_pn: [0; STRLEN_L],
        version: 0,
        vbios_ver_str: [0; STRLEN_N],
        date: [0; STRLEN_N],
        build_num: [0; STRLEN_N],
        reg_block: 0,
        io_mode: 0,
        data_block: 0,
        fb_base: 0,
        divmul: [0; 2],
        shift: 0,
        io_attr: 0,
        cs_equal: false,
        cs_above: false,
        execute_depth: 0,
    };

    if ctx.cu16(0) as u32 != ATOM_BIOS_MAGIC {
        Line::new().s("Invalid BIOS magic").say();
        return None;
    }
    if !ctx.cstr_starts_with(ATOM_ATI_MAGIC_PTR, ATOM_ATI_MAGIC) {
        Line::new().s("Invalid ATI magic").say();
        return None;
    }

    let base = ctx.cu16(ATOM_ROM_TABLE_PTR) as u32;
    if !ctx.cstr_starts_with(base + ATOM_ROM_MAGIC_PTR, ATOM_ROM_MAGIC) {
        Line::new().s("Invalid ATOM magic").say();
        return None;
    }

    ctx.cmd_table = ctx.cu16(base + ATOM_ROM_CMD_PTR) as u32;
    ctx.data_table = ctx.cu16(base + ATOM_ROM_DATA_PTR) as u32;
    let iio = ctx.cu16(ctx.data_table + ATOM_DATA_IIO_PTR) as u32 + 4;
    atom_index_iio(&mut ctx, iio);

    let master = ctx.cu16(base + ROM_HEADER_MASTER_DATA_TABLE) as u32;
    if master != 0 {
        let fw_info = ctx.cu16(master + MASTER_DATA_FIRMWARE_INFO) as u32;
        if fw_info != 0 {
            ctx.version = ctx.cu32(fw_info + FIRMWARE_INFO_REVISION);
        }
    }

    atom_get_vbios_name(&mut ctx);
    atom_get_vbios_pn(&mut ctx);
    atom_get_vbios_date(&mut ctx);
    atom_get_vbios_version(&mut ctx);
    atom_get_vbios_build(&mut ctx);

    Some(ctx)
}

/// One table header: `size`, `frev`, `crev`, and where the table starts.
pub struct atom_table_header {
    pub size: u16,
    pub frev: u8,
    pub crev: u8,
    pub data_start: u16,
}

/// `amdgpu_atom_parse_data_header`.
pub fn amdgpu_atom_parse_data_header(ctx: &atom_context, index: u32) -> Option<atom_table_header> {
    let offset = index * 2 + 4;
    let idx = ctx.cu16(ctx.data_table + offset) as u32;
    if idx == 0 {
        return None;
    }
    Some(atom_table_header { size: ctx.cu16(idx), frev: ctx.cu8(idx + 2), crev: ctx.cu8(idx + 3), data_start: idx as u16 })
}

/// `amdgpu_atom_parse_cmd_header`: `frev`, `crev`.
pub fn amdgpu_atom_parse_cmd_header(ctx: &atom_context, index: u32) -> Option<(u8, u8)> {
    let offset = index * 2 + 4;
    let idx = ctx.cu16(ctx.cmd_table + offset) as u32;
    if idx == 0 {
        return None;
    }
    Some((ctx.cu8(idx + 2), ctx.cu8(idx + 3)))
}
