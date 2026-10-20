//! A guest's frame as radeonsi imports a dma-buf without a modifier
//! (si_texture_from_winsys_buffer), and the sampled-image descriptor (T#)
//! the compositor's import.comp reads it through. Mesa's common code for
//! the part a guest frame takes, on Renoir: ac_surface_apply_bo_metadata,
//! ac_compute_surface, ac_surface_apply_umd_metadata (ac_surface.c),
//! ac_build_texture_descriptor, ac_set_mutable_tex_desc_fields
//! (ac_descriptors.c), for B8G8R8A8_UNORM, 2D, one level, one sample.
//! hostcheck/import_check.rs holds every field against Mesa's own run of
//! these steps (shaders/imgdesc.c). Not a Linux file.
//!
//! Mesa leaves the bounds of the DCC to the consumer's own VM. The
//! compositor's VM holds other buffers, so `check` refuses a surface whose
//! image or DCC would reach past the guest's object.

use crate::regs::*;

/// `ATI_VENDOR_ID`.
const ATI_VENDOR_ID: u32 = 0x1002;

/// `AddrSwizzleMode` on GFX9: the ones a 32-bit colour surface is laid out
/// in and this port models.
pub const ADDR_SW_LINEAR: u32 = 0;
pub const ADDR_SW_4KB_S_X: u32 = 21;
pub const ADDR_SW_4KB_D_X: u32 = 22;
pub const ADDR_SW_64KB_Z_X: u32 = 24;
pub const ADDR_SW_64KB_S_X: u32 = 25;
pub const ADDR_SW_64KB_D_X: u32 = 26;
pub const ADDR_SW_64KB_R_X: u32 = 27;

/// `ADDR_INVALIDPARAMS`: addrlib's refusal of a displayable surface in a
/// swizzle mode the display cannot read.
pub const ADDR_INVALIDPARAMS: i32 = 3;

/// `SQ_IMG_RSRC_WORD*` fields (registers/gfx9.json).
const S_008F14_BASE_ADDRESS_HI: (u32, u32) = (0, 8);
const S_008F14_DATA_FORMAT: (u32, u32) = (20, 6);
const S_008F14_NUM_FORMAT: (u32, u32) = (26, 4);
const S_008F18_WIDTH: (u32, u32) = (0, 14);
const S_008F18_HEIGHT: (u32, u32) = (14, 14);
const S_008F18_PERF_MOD: (u32, u32) = (28, 3);
const S_008F1C_DST_SEL_X: (u32, u32) = (0, 3);
const S_008F1C_DST_SEL_Y: (u32, u32) = (3, 3);
const S_008F1C_DST_SEL_Z: (u32, u32) = (6, 3);
const S_008F1C_DST_SEL_W: (u32, u32) = (9, 3);
const S_008F1C_LAST_LEVEL: (u32, u32) = (16, 4);
const S_008F1C_SW_MODE: (u32, u32) = (20, 5);
const S_008F1C_TYPE: (u32, u32) = (28, 4);
const S_008F20_PITCH: (u32, u32) = (13, 16);
const S_008F20_BC_SWIZZLE: (u32, u32) = (29, 3);
const S_008F24_META_DATA_ADDRESS: (u32, u32) = (17, 8);
const S_008F24_META_PIPE_ALIGNED: (u32, u32) = (26, 1);
const S_008F24_META_RB_ALIGNED: (u32, u32) = (27, 1);
const S_008F28_COMPRESSION_EN: (u32, u32) = (21, 1);
const S_008F28_ALPHA_IS_ON_MSB: (u32, u32) = (22, 1);

const V_008F14_IMG_DATA_FORMAT_8_8_8_8: u32 = 10;
const V_008F14_IMG_NUM_FORMAT_UNORM: u32 = 0;
const V_008F1C_SQ_SEL_X: u32 = 4;
const V_008F1C_SQ_SEL_Y: u32 = 5;
const V_008F1C_SQ_SEL_Z: u32 = 6;
const V_008F1C_SQ_SEL_W: u32 = 7;
const V_008F1C_SQ_RSRC_IMG_2D: u32 = 9;
const V_008F1C_SQ_RSRC_IMG_2D_MSAA: u32 = 14;
const V_008F1C_SQ_RSRC_IMG_2D_MSAA_ARRAY: u32 = 15;
const V_008F20_BC_SWIZZLE_ZYXW: u32 = 4;

fn set(f: (u32, u32), v: u32) -> u32 {
    (v & ((1u32 << f.1) - 1)) << f.0
}

fn get(f: (u32, u32), w: u32) -> u32 {
    (w >> f.0) & ((1u32 << f.1) - 1)
}

/// The fields of `struct radeon_surf` the descriptor is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct radeon_surf {
    pub swizzle_mode: u32,
    pub scanout: bool,
    pub epitch: u32,
    pub surf_pitch: u32,
    pub surf_height: u32,
    pub surf_size: u64,
    pub surf_alignment_log2: u32,
    pub meta_offset: u64,
    pub meta_size: u32,
    pub display_dcc_size: u32,
    pub meta_alignment_log2: u32,
    pub pipe_aligned: bool,
    pub rb_aligned: bool,
    pub num_meta_levels: u32,
    pub tile_swizzle: u32,
}

/// Why a frame is not imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum refusal {
    /// A layout this port does not model.
    unmodelled(&'static str),
    /// addrlib's own refusal (`ac_compute_surface`).
    surface(i32),
    /// `ac_surface_apply_umd_metadata` returned false.
    metadata,
    /// The image or its DCC is not inside the object.
    bounds(&'static str),
}

/// `ac_get_umd_metadata_word1`.
pub fn ac_get_umd_metadata_word1(pci_id: u16) -> u32 {
    (ATI_VENDOR_ID << 16) | pci_id as u32
}

/// `ac_surface_apply_bo_metadata` on GFX9: swizzle mode and scanout.
pub fn ac_surface_apply_bo_metadata(tiling_flags: u64) -> (u32, bool) {
    let swizzle = (tiling_flags >> AMDGPU_TILING_SWIZZLE_MODE_SHIFT) as u32 & AMDGPU_TILING_SWIZZLE_MODE_MASK;
    (swizzle, (tiling_flags >> AMDGPU_TILING_SCANOUT_SHIFT) & 1 != 0)
}

/// `ac_compute_surface` of an imported colour surface (`RADEON_SURF_IMPORTED |
/// SHAREABLE`, the swizzle mode kept) as si_init_surface asks for it: what
/// addrlib's Gfx9Lib computes for 32 bits per pixel, one level, one
/// sample. The image is whole blocks of the swizzle mode (4 KiB: 32x32,
/// 64 KiB: 128x128 pixels). The DCC Renoir lays out has one byte per 8x8
/// pixels over the image padded to 512x512, in whole blocks of the swizzle
/// mode; it is computed pipe- and RB-aligned, and the displayable copy
/// takes the same size.
pub fn ac_compute_surface(width: u32, height: u32, swizzle_mode: u32, scanout: bool) -> Result<radeon_surf, refusal> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(refusal::unmodelled("size"));
    }
    let (block, align_log2) = match swizzle_mode {
        ADDR_SW_4KB_S_X | ADDR_SW_4KB_D_X => (32u32, 12u32),
        ADDR_SW_64KB_Z_X | ADDR_SW_64KB_S_X | ADDR_SW_64KB_D_X | ADDR_SW_64KB_R_X => (128, 16),
        _ => return Err(refusal::unmodelled("swizzle mode")),
    };
    // get_display_flag: a scanout surface of 32 bits per pixel is
    // displayable, and Renoir's display reads only 64KB_S_X.
    if scanout && swizzle_mode != ADDR_SW_64KB_S_X {
        return Err(refusal::surface(ADDR_INVALIDPARAMS));
    }
    let surf_pitch = width.next_multiple_of(block);
    let surf_height = height.next_multiple_of(block);
    let align = 1u64 << align_log2;
    let dcc = (surf_pitch.next_multiple_of(512) as u64 * surf_height.next_multiple_of(512) as u64 / 64)
        .next_multiple_of(align) as u32;
    Ok(radeon_surf {
        swizzle_mode,
        scanout,
        epitch: surf_pitch - 1,
        surf_pitch,
        surf_height,
        surf_size: surf_pitch as u64 * surf_height as u64 * 4,
        surf_alignment_log2: align_log2,
        meta_offset: 0,
        meta_size: dcc,
        display_dcc_size: dcc,
        meta_alignment_log2: align_log2,
        pipe_aligned: true,
        rb_aligned: true,
        num_meta_levels: 1,
        tile_swizzle: 0,
    })
}

/// `ac_surface_zero_dcc_fields` of a colour surface.
fn ac_surface_zero_dcc_fields(surf: &mut radeon_surf) {
    surf.meta_offset = 0;
}

/// `ac_surface_apply_umd_metadata` on GFX9, for a surface at offset 0 and
/// one sample: the DCC radeonsi recorded in the descriptor it exported.
/// `metadata` holds the words AMDGPU_GEM_METADATA returns.
pub fn ac_surface_apply_umd_metadata(word1: u32, surf: &mut radeon_surf, num_mipmap_levels: u32,
                                     metadata: &[u32]) -> bool {
    if metadata.len() < 10 || metadata[0] == 0 || metadata[1] != word1 {
        ac_surface_zero_dcc_fields(surf);
        return true;
    }
    let desc = &metadata[2..10];
    let desc_last_level = get(S_008F1C_LAST_LEVEL, desc[3]);
    let ty = get(S_008F1C_TYPE, desc[3]);
    if ty == V_008F1C_SQ_RSRC_IMG_2D_MSAA || ty == V_008F1C_SQ_RSRC_IMG_2D_MSAA_ARRAY {
        if desc_last_level != 0 {
            return false;
        }
    } else if desc_last_level != num_mipmap_levels - 1 {
        return false;
    }
    if get(S_008F28_COMPRESSION_EN, desc[6]) != 0 {
        surf.meta_offset = (desc[7] as u64) << 8 | (get(S_008F24_META_DATA_ADDRESS, desc[5]) as u64) << 40;
        surf.pipe_aligned = get(S_008F24_META_PIPE_ALIGNED, desc[5]) != 0;
        surf.rb_aligned = get(S_008F24_META_RB_ALIGNED, desc[5]) != 0;
        surf.num_meta_levels = desc_last_level + 1;
    } else {
        ac_surface_zero_dcc_fields(surf);
    }
    true
}

/// `vi_dcc_enabled` at level 0.
pub fn dcc_enabled(surf: &radeon_surf) -> bool {
    surf.meta_offset != 0 && surf.num_meta_levels > 0
}

/// The surface of a guest frame from its object's tiling flags and UMD
/// metadata, as radeonsi imports it.
pub fn import(pci_id: u16, width: u32, height: u32, tiling_flags: u64, metadata: &[u32])
              -> Result<radeon_surf, refusal> {
    let (swizzle, scanout) = ac_surface_apply_bo_metadata(tiling_flags);
    let mut surf = ac_compute_surface(width, height, swizzle, scanout)?;
    if !ac_surface_apply_umd_metadata(ac_get_umd_metadata_word1(pci_id), &mut surf, 1, metadata) {
        return Err(refusal::metadata);
    }
    Ok(surf)
}

/// The texture unit reads `surf` from an object of `bo_size` bytes and
/// nothing past it: the image (radeonsi's own check), and the DCC at the
/// offset the guest named, at the alignment it was laid out with. Mixed
/// pipe and RB alignment is a layout Mesa never makes.
pub fn check(surf: &radeon_surf, bo_size: u64) -> Result<(), refusal> {
    if surf.surf_size > bo_size {
        return Err(refusal::bounds("image"));
    }
    if surf.tile_swizzle != 0 {
        return Err(refusal::unmodelled("tile swizzle"));
    }
    if dcc_enabled(surf) {
        if surf.pipe_aligned != surf.rb_aligned {
            return Err(refusal::unmodelled("mixed DCC alignment"));
        }
        let end = surf.meta_offset.checked_add(surf.meta_size.max(surf.display_dcc_size) as u64);
        if surf.meta_offset % (1u64 << surf.meta_alignment_log2) != 0
            || surf.meta_offset < surf.surf_size
            || end.is_none_or(|e| e > bo_size) {
            return Err(refusal::bounds("DCC"));
        }
    }
    Ok(())
}

/// The alignment the address of `surf` must have: its image's and its
/// DCC's (`alignment_log2`).
pub fn alignment(surf: &radeon_surf) -> u64 {
    let log2 = if dcc_enabled(surf) { surf.surf_alignment_log2.max(surf.meta_alignment_log2) }
               else { surf.surf_alignment_log2 };
    1u64 << log2
}

/// `ac_border_color_swizzle` of B8G8R8A8: alpha last, X third.
const BC_SWIZZLE: u32 = V_008F20_BC_SWIZZLE_ZYXW;

/// `si_make_texture_descriptor` (ac_build_gfx6_texture_descriptor) and
/// `si_set_mutable_tex_desc_fields` (ac_set_mutable_tex_desc_fields) of a
/// 2D B8G8R8A8_UNORM view of the whole surface at `va`.
pub fn descriptor(surf: &radeon_surf, width: u32, height: u32, va: u64) -> [u32; 8] {
    debug_assert!(va % alignment(surf) == 0);
    let dcc = dcc_enabled(surf);
    let mut desc = [0u32; 8];
    // ac_build_gfx6_texture_descriptor.
    desc[1] = set(S_008F14_DATA_FORMAT, V_008F14_IMG_DATA_FORMAT_8_8_8_8)
        | set(S_008F14_NUM_FORMAT, V_008F14_IMG_NUM_FORMAT_UNORM);
    desc[2] = set(S_008F18_WIDTH, width - 1) | set(S_008F18_HEIGHT, height - 1) | set(S_008F18_PERF_MOD, 4);
    // The format's swizzle ZYXW, composed with the identity.
    desc[3] = set(S_008F1C_DST_SEL_X, V_008F1C_SQ_SEL_Z)
        | set(S_008F1C_DST_SEL_Y, V_008F1C_SQ_SEL_Y)
        | set(S_008F1C_DST_SEL_Z, V_008F1C_SQ_SEL_X)
        | set(S_008F1C_DST_SEL_W, V_008F1C_SQ_SEL_W)
        | set(S_008F1C_TYPE, V_008F1C_SQ_RSRC_IMG_2D);
    desc[4] = set(S_008F20_BC_SWIZZLE, BC_SWIZZLE);
    if dcc {
        // ac_alpha_is_on_msb: B8G8R8A8 is SWAP_ALT, not reversed.
        desc[6] = set(S_008F28_ALPHA_IS_ON_MSB, 1);
    }
    // ac_set_mutable_tex_desc_fields.
    desc[0] = (va >> 8) as u32;
    desc[1] |= set(S_008F14_BASE_ADDRESS_HI, (va >> 40) as u32);
    let mut meta_va = 0;
    if dcc {
        meta_va = va + surf.meta_offset;
        let dcc_tile_swizzle = (surf.tile_swizzle << 8) as u64 & ((1u64 << surf.meta_alignment_log2) - 1);
        meta_va |= dcc_tile_swizzle;
    }
    desc[0] |= surf.tile_swizzle;
    desc[3] |= set(S_008F1C_SW_MODE, surf.swizzle_mode);
    desc[4] |= set(S_008F20_PITCH, surf.epitch);
    if meta_va != 0 {
        desc[5] |= set(S_008F24_META_DATA_ADDRESS, (meta_va >> 40) as u32)
            | set(S_008F24_META_PIPE_ALIGNED, surf.pipe_aligned as u32)
            | set(S_008F24_META_RB_ALIGNED, surf.rb_aligned as u32);
        desc[6] |= set(S_008F28_COMPRESSION_EN, 1);
        desc[7] = (meta_va >> 8) as u32;
    }
    // si_set_mutable_tex_desc_fields on GFX9: the pitch again, epitch.
    desc[4] &= !set(S_008F20_PITCH, !0);
    desc[4] |= set(S_008F20_PITCH, surf.epitch);
    desc
}
