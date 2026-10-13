//! display/intel_bw.c (Linux 6.18.55): `intel_bw_init_hw` for Tiger Lake,
//! i.e. `tgl_get_bw_info` with `tgl_sa_info`: the QGV points from PCODE
//! and the derated bandwidth per plane group.

use crate::i915_drv::drm_i915_private;
use crate::intel_pcode;
use crate::linux::Line;
use crate::regs::*;

const NUM_GROUPS: usize = BW_NUM_GROUPS as usize;
const NUM_QGV: usize = I915_NUM_QGV_POINTS as usize;
const NUM_PSF: usize = I915_NUM_PSF_GV_POINTS as usize;

/// `struct intel_bw_info`.
#[derive(Clone, Copy, Default)]
pub struct intel_bw_info {
    pub deratedbw: [u32; NUM_QGV],
    pub psf_bw: [u32; NUM_PSF],
    pub peakbw: [u32; NUM_QGV],
    pub num_qgv_points: u8,
    pub num_psf_gv_points: u8,
    pub num_planes: u8,
}

/// `display->bw`.
#[derive(Clone, Copy, Default)]
pub struct intel_display_bw {
    pub max: [intel_bw_info; NUM_GROUPS],
}

/// `struct intel_qgv_point`.
#[derive(Clone, Copy, Default)]
struct intel_qgv_point {
    dclk: u16,
    t_rp: u16,
    t_rdpre: u16,
    t_rc: u16,
    t_ras: u16,
    t_rcd: u16,
}

/// `struct intel_qgv_info`.
#[derive(Clone, Copy, Default)]
struct intel_qgv_info {
    points: [intel_qgv_point; NUM_QGV],
    psf_points: [u8; NUM_PSF],
    num_points: u8,
    num_psf_points: u8,
    t_bl: u8,
    max_numchannels: u8,
    channel_width: u8,
    deinterleave: u8,
}

fn div_round_up(n: i64, d: i64) -> i64 {
    (n + d - 1) / d
}

fn div_round_closest(n: i64, d: i64) -> i64 {
    (n + d / 2) / d
}

/// `icl_pcode_read_qgv_point_info` (display version 12).
fn icl_pcode_read_qgv_point_info(i915: &mut drm_i915_private, sp: &mut intel_qgv_point,
                                 point: u32) -> Result<(), i32> {
    let mut val = 0u32;
    let mut val2 = 0u32;
    intel_pcode::snb_pcode_read(i915, ICL_PCODE_MEM_SUBSYSYSTEM_INFO | ICL_PCODE_MEM_SS_READ_QGV_POINT_INFO(point),
                                &mut val, Some(&mut val2))?;
    let dclk = (val & 0xffff) as i64;
    sp.dclk = div_round_up(16667 * dclk + 500, 1000) as u16;
    sp.t_rp = ((val & 0xff0000) >> 16) as u16;
    sp.t_rcd = ((val & 0xff000000) >> 24) as u16;
    sp.t_rdpre = (val2 & 0xff) as u16;
    sp.t_ras = ((val2 & 0xff00) >> 8) as u16;
    sp.t_rc = sp.t_rp + sp.t_ras;
    Ok(())
}

/// `icl_get_qgv_points` (display version 12, not RKL).
fn icl_get_qgv_points(i915: &mut drm_i915_private, qi: &mut intel_qgv_info, is_y_tile: bool) -> Result<(), i32> {
    let dram_info = i915.dram_info;
    qi.num_points = dram_info.num_qgv_points as u8;
    qi.num_psf_points = dram_info.num_psf_gv_points as u8;

    match dram_info.ty {
        INTEL_DRAM_DDR4 => {
            qi.t_bl = if is_y_tile { 8 } else { 4 };
            qi.max_numchannels = 2;
            qi.channel_width = 64;
            qi.deinterleave = if is_y_tile { 1 } else { 2 };
        }
        INTEL_DRAM_DDR5 => {
            qi.t_bl = if is_y_tile { 16 } else { 8 };
            qi.max_numchannels = 4;
            qi.channel_width = 32;
            qi.deinterleave = if is_y_tile { 1 } else { 2 };
        }
        INTEL_DRAM_LPDDR4 | INTEL_DRAM_LPDDR5 => {
            qi.t_bl = 16;
            qi.max_numchannels = 8;
            qi.channel_width = 16;
            qi.deinterleave = if is_y_tile { 2 } else { 4 };
        }
        _ => {
            qi.t_bl = 16;
            qi.max_numchannels = 1;
        }
    }

    if qi.num_points as usize > NUM_QGV {
        crate::linux::say("warning: more QGV points than the table holds");
        qi.num_points = NUM_QGV as u8;
    }

    for i in 0..qi.num_points as usize {
        let mut sp = qi.points[i];
        if let Err(e) = icl_pcode_read_qgv_point_info(i915, &mut sp, i as u32) {
            Line::new().s("Could not read QGV ").dec(i as u64).s(" info").say();
            return Err(e);
        }
        qi.points[i] = sp;
        Line::new().s("QGV ").dec(i as u64).s(": DCLK=").dec(sp.dclk as u64).s(" tRP=").dec(sp.t_rp as u64)
            .s(" tRDPRE=").dec(sp.t_rdpre as u64).s(" tRAS=").dec(sp.t_ras as u64)
            .s(" tRCD=").dec(sp.t_rcd as u64).s(" tRC=").dec(sp.t_rc as u64).say();
    }

    if qi.num_psf_points > 0 {
        // adls_pcode_read_psf_gv_point_info is not ported; PCH TGP reports
        // no PSF points.
        Line::new().s("Failed to read PSF point data; PSF points will not be considered in bandwidth calculations.").say();
        qi.num_psf_points = 0;
    }
    Ok(())
}

/// `adl_calc_psf_bw`.
fn adl_calc_psf_bw(clk: i64) -> i64 {
    div_round_closest(64 * clk * 100, 6)
}

/// `icl_sagv_max_dclk`.
fn icl_sagv_max_dclk(qi: &intel_qgv_info) -> i64 {
    let mut dclk = 0u16;
    for i in 0..qi.num_points as usize {
        dclk = dclk.max(qi.points[i].dclk);
    }
    dclk as i64
}

/// `tgl_get_bw_info` with `tgl_sa_info`.
fn tgl_get_bw_info(i915: &mut drm_i915_private) -> Result<(), i32> {
    let dram_info = i915.dram_info;
    let mut qi = intel_qgv_info::default();
    // Assume Y tiling may be used.
    let is_y_tile = true;
    let mut num_channels = (dram_info.num_channels as u8).max(1) as i64;
    let ipqdepthpch = 16i64;

    if let Err(e) = icl_get_qgv_points(i915, &mut qi, is_y_tile) {
        Line::new().s("Failed to get memory subsystem information, ignoring bandwidth limits").say();
        return Err(e);
    }

    if dram_info.ty == INTEL_DRAM_LPDDR4 || dram_info.ty == INTEL_DRAM_LPDDR5 {
        num_channels *= 2;
    }

    if qi.deinterleave == 0 {
        qi.deinterleave = div_round_up(num_channels, if is_y_tile { 4 } else { 2 }) as u8;
    }
    if num_channels < qi.max_numchannels as i64 {
        qi.deinterleave = div_round_up(qi.deinterleave as i64, 2).max(1) as u8;
    }
    if num_channels > qi.max_numchannels as i64 {
        Line::new().s("Number of channels exceeds max number of channels.").say();
    }
    if qi.max_numchannels != 0 {
        num_channels = (num_channels as u8).min(qi.max_numchannels) as i64;
    }

    let dclk_max = icl_sagv_max_dclk(&qi);
    let peakbw = num_channels * div_round_up(qi.channel_width as i64, 8) * dclk_max;
    let maxdebw = (TGL_SA_DEPROGBWLIMIT as i64 * 1000).min(peakbw * DEPROGBWPCLIMIT as i64 / 100);
    let ipqdepth = ipqdepthpch.min(TGL_SA_DISPLAYRTIDS as i64 / num_channels);
    // clperchgroup = 4kpagespermempage * clperchperblock,
    // clperchperblock = 8 / num_channels * interleave
    let clperchgroup = 4 * div_round_up(8, num_channels) * qi.deinterleave as i64;

    for i in 0..NUM_GROUPS {
        let clpchgroup = (TGL_SA_DEBURST as i64 * qi.deinterleave as i64 / num_channels) << i;

        if i < NUM_GROUPS - 1 {
            i915.display.bw.max[i + 1].num_planes = if clpchgroup < clperchgroup {
                ((ipqdepth - clpchgroup) / clpchgroup + 1) as u8
            } else {
                0
            };
        }

        let bi = &mut i915.display.bw.max[i];
        bi.num_qgv_points = qi.num_points;
        bi.num_psf_gv_points = qi.num_psf_points;

        for j in 0..qi.num_points as usize {
            let sp = &qi.points[j];
            // Max row cycle time
            let ct = (sp.t_rc as i64).max(sp.t_rp as i64 + sp.t_rcd as i64
                + (clpchgroup - 1) * qi.t_bl as i64 + sp.t_rdpre as i64);
            let bw = div_round_up(sp.dclk as i64 * clpchgroup * 32 * num_channels, ct);

            bi.deratedbw[j] = maxdebw.min(bw * (100 - TGL_SA_DERATING as i64) / 100) as u32;
            bi.peakbw[j] = div_round_closest(sp.dclk as i64 * num_channels * qi.channel_width as i64, 8) as u32;

            Line::new().s("BW").dec(i as u64).s(" / QGV ").dec(j as u64).s(": num_planes=")
                .dec(bi.num_planes as u64).s(" deratedbw=").dec(bi.deratedbw[j] as u64)
                .s(" peakbw: ").dec(bi.peakbw[j] as u64).say();
        }

        for j in 0..qi.num_psf_points as usize {
            bi.psf_bw[j] = adl_calc_psf_bw(qi.psf_points[j] as i64) as u32;
        }
    }

    // With SAGV disabled in the BIOS there is one point, and PCODE refuses
    // to restrict it.
    i915.display.sagv_status = if qi.num_points == 1 { I915_SAGV_NOT_CONTROLLED } else { I915_SAGV_ENABLED };
    Ok(())
}

/// `intel_bw_init_hw` (display version 12, not RKL/ADL).
pub fn intel_bw_init_hw(i915: &mut drm_i915_private) {
    if !i915.display.has_display {
        return;
    }
    let _ = tgl_get_bw_info(i915);
}
