//! `rtw8822c.c` lines 108-1010: DAC calibration (DACK).
//!
//! Ported, in source order: `rtw8822c_dac_backup_reg` ·
//! `rtw8822c_dac_restore_reg` · `rtw8822c_rf_minmax_cmp` ·
//! `__rtw8822c_dac_iq_sort` · `rtw8822c_dac_iq_sort` ·
//! `rtw8822c_dac_iq_offset` · `rtw8822c_get_path_write_addr` ·
//! `rtw8822c_get_path_read_addr` · `rtw8822c_dac_iq_check` ·
//! `rtw8822c_dac_cal_iq_sample` · `rtw8822c_dac_cal_iq_search` ·
//! `rtw8822c_dac_cal_rf_mode` · `rtw8822c_dac_bb_setting` ·
//! `rtw8822c_dac_cal_adc` · `rtw8822c_dac_cal_step1..4` ·
//! `rtw8822c_dac_cal_backup_vec/_path/_dck/_backup` ·
//! `rtw8822c_dac_cal_restore_dck/_prepare/_wait/_path` ·
//! `__rtw8822c_dac_cal_restore` · `rtw8822c_dac_cal_restore` ·
//! `rtw8822c_rf_dac_cal`.
//!
//! The calibration measures the DC offset of ADC and DAC on both paths and
//! programs the compensation; without it every RX path has a constant
//! error.
//!
//! `dac_cal_restore` only applies on a second bring-up: it restores the
//! result of an earlier run, and `dack_msbk` is zero on the first run,
//! which is where it bails out.
#![allow(dead_code)]

use crate::dm::DmInfo;
use crate::host;
use crate::phy::{read_rf, write_rf_reg_mix, RFREG_MASK, RF_PATH_A, RF_PATH_B};
use crate::regs::*;

/// `rtw_write_rf(..., RFREG_MASK, v)`, the form DACK writes in.
fn wrf(h: i32, path: usize, addr: u32, val: u32) {
    write_rf_reg_mix(h, path, addr, RFREG_MASK, val);
}

/// `struct rtw_backup_info` (main.h), as in `mac.rs`, but all entries here
/// are 4 bytes wide, so no `len` is needed.
#[derive(Clone, Copy, Default)]
struct Backup {
    reg: u32,
    val: u32,
}

/// rtw8822c.c:115-120: the sixteen BB registers DACK changes.
const DACK_ADDRS: [u32; DACK_REG_8822C] = [
    0x180c, 0x1810, 0x410c, 0x4110,
    0x1c3c, 0x1c24, 0x1d70, 0x9b4,
    0x1a00, 0x1a14, 0x1d58, 0x1c38,
    0x1e24, 0x1e28, 0x1860, 0x4160,
];

/// rtw8822c.c:115 — `u32 rf_addr[DACK_RF_8822C] = {0x8f};`
const DACK_RF_ADDRS: [u32; DACK_RF_8822C] = [0x8f];

/// rtw8822c.c:108-135 `rtw8822c_dac_backup_reg`.
///
/// The index `backup_rf[path * i + i]` is Linux's own and is odd: with
/// `DACK_RF_8822C == 1`, `i` is only 0, so both paths use slot 0 and path
/// B overwrites path A; `restore_reg` reads back with the same expression.
/// It is kept unchanged because it is harmless while `DACK_RF_8822C` is 1,
/// and fixing it would change the write order relative to Linux.
fn dac_backup_reg(h: i32) -> ([Backup; DACK_REG_8822C],
                              [Backup; DACK_RF_8822C * DACK_PATH_8822C]) {
    let mut backup = [Backup::default(); DACK_REG_8822C];
    let mut backup_rf = [Backup::default(); DACK_RF_8822C * DACK_PATH_8822C];

    for i in 0..DACK_REG_8822C {
        backup[i].reg = DACK_ADDRS[i];
        backup[i].val = host::r32(h, DACK_ADDRS[i]);
    }

    for path in 0..DACK_PATH_8822C {
        for i in 0..DACK_RF_8822C {
            let reg = DACK_RF_ADDRS[i];
            let val = read_rf(h, path, reg, RFREG_MASK);
            backup_rf[path * i + i].reg = reg;
            backup_rf[path * i + i].val = val;
        }
    }
    (backup, backup_rf)
}

/// rtw8822c.c:137-152 `rtw8822c_dac_restore_reg`
fn dac_restore_reg(h: i32, backup: &[Backup; DACK_REG_8822C],
                   backup_rf: &[Backup; DACK_RF_8822C * DACK_PATH_8822C]) {
    // util.c `rtw_restore_reg`, all 4 bytes here.
    for b in backup.iter() {
        host::w32(h, b.reg, b.val);
    }
    for path in 0..DACK_PATH_8822C {
        for i in 0..DACK_RF_8822C {
            let val = backup_rf[path * i + i].val;
            let reg = backup_rf[path * i + i].reg;
            wrf(h, path, reg, val);
        }
    }
}

/// rtw8822c.c:154-183 `rtw8822c_rf_minmax_cmp`.
///
/// The values are 10-bit two's complement: anything from 0x200 up is
/// negative, so "smaller" is not numeric order.
fn rf_minmax_cmp(value: u32, min: &mut u32, max: &mut u32) {
    if value >= 0x200 {
        if *min >= 0x200 {
            if *min > value {
                *min = value;
            }
        } else {
            *min = value;
        }
        if *max >= 0x200 && *max < value {
            *max = value;
        }
    } else {
        if *min < 0x200 && *min > value {
            *min = value;
        }
        if *max >= 0x200 {
            *max = value;
        } else if *max < value {
            *max = value;
        }
    }
}

/// rtw8822c.c:185-196 `__rtw8822c_dac_iq_sort`
fn dac_iq_sort_pair(v1: &mut u32, v2: &mut u32) {
    if (*v1 >= 0x200 && *v2 >= 0x200) || (*v1 < 0x200 && *v2 < 0x200) {
        if *v1 > *v2 {
            core::mem::swap(v1, v2);
        }
    } else if *v1 < 0x200 && *v2 >= 0x200 {
        core::mem::swap(v1, v2);
    }
}

/// rtw8822c.c:198-209 `rtw8822c_dac_iq_sort`: bubble sort over both arrays.
fn dac_iq_sort(iv: &mut [u32; DACK_SN_8822C], qv: &mut [u32; DACK_SN_8822C]) {
    for i in 0..DACK_SN_8822C - 1 {
        for j in 0..DACK_SN_8822C - 1 - i {
            let (a, b) = iv.split_at_mut(j + 1);
            dac_iq_sort_pair(&mut a[j], &mut b[0]);
            let (a, b) = qv.split_at_mut(j + 1);
            dac_iq_sort_pair(&mut a[j], &mut b[0]);
        }
    }
}

/// rtw8822c.c:211-234 `rtw8822c_dac_iq_offset`: the mean of the middle 80
/// of 100 samples, again in 10-bit two's complement.
fn dac_iq_offset(vec: &[u32; DACK_SN_8822C]) -> u32 {
    let mut m = 0u32;
    let mut p = 0u32;
    for &v in vec.iter().take(DACK_SN_8822C - 10).skip(10) {
        if v > 0x200 {
            m += 0x400 - v;
        } else {
            p += v;
        }
    }

    if p > m {
        (p - m) / (DACK_SN_8822C as u32 - 20)
    } else {
        let t = (m - p) / (DACK_SN_8822C as u32 - 20);
        if t != 0 { 0x400 - t } else { 0 }
    }
}

/// rtw8822c.c:236-253 `rtw8822c_get_path_write_addr`
fn path_write_addr(path: usize) -> u32 {
    match path {
        RF_PATH_A => 0x1800,
        RF_PATH_B => 0x4100,
        _ => 0,
    }
}

/// rtw8822c.c:255-272 `rtw8822c_get_path_read_addr`
fn path_read_addr(path: usize) -> u32 {
    match path {
        RF_PATH_A => 0x2800,
        RF_PATH_B => 0x4500,
        _ => 0,
    }
}

/// rtw8822c.c:274-285 `rtw8822c_dac_iq_check`: a sample whose magnitude
/// exceeds 0x64 is an overflow and is discarded.
fn dac_iq_check(value: u32) -> bool {
    !((value >= 0x200 && (0x400 - value) > 0x64) || (value < 0x200 && value > 0x64))
}

/// rtw8822c.c:287-302 `rtw8822c_dac_cal_iq_sample`.
///
/// The cap of 10000 is Linux's; without it the loop hangs if the hardware
/// returns only overflows.
fn dac_cal_iq_sample(h: i32, iv: &mut [u32; DACK_SN_8822C],
                     qv: &mut [u32; DACK_SN_8822C], verbose: bool) {
    let mut i = 0usize;
    let mut cnt = 0u32;
    while i < DACK_SN_8822C && cnt < 10000 {
        cnt += 1;
        let temp = host::r32_mask(h, 0x2dbc, 0x3fffff);
        iv[i] = (temp & 0x3ff000) >> 12;
        qv[i] = temp & 0x3ff;
        if dac_iq_check(iv[i]) && dac_iq_check(qv[i]) {
            i += 1;
        }
    }
    if verbose {
        // Report the raw samples before anything is computed from them: 100
        // identical values indicate a frozen measurement, a real one scatters.
        let (mut imin, mut imax, mut qmin, mut qmax) = (iv[0], iv[0], qv[0], qv[0]);
        for k in 0..DACK_SN_8822C {
            if iv[k] < imin { imin = iv[k]; }
            if iv[k] > imax { imax = iv[k]; }
            if qv[k] < qmin { qmin = qv[k]; }
            if qv[k] > qmax { qmax = qv[k]; }
        }
        host::print("      Rohproben i 0x");
        host::print_hex16(imin as u16);
        host::print("..0x");
        host::print_hex16(imax as u16);
        host::print("  q 0x");
        host::print_hex16(qmin as u16);
        host::print("..0x");
        host::print_hex16(qmax as u16);
        host::print("  (");
        host::print_dec(cnt);
        host::print(" Lesungen fuer 100 gueltige)\n");
    }
}

/// rtw8822c.c:304-360 `rtw8822c_dac_cal_iq_search`
fn dac_cal_iq_search(h: i32, iv: &mut [u32; DACK_SN_8822C],
                     qv: &mut [u32; DACK_SN_8822C]) -> (u32, u32) {
    let mut cnt = 0u32;
    loop {
        let mut i_min = iv[0];
        let mut i_max = iv[0];
        let mut q_min = qv[0];
        let mut q_max = qv[0];
        for i in 0..DACK_SN_8822C {
            rf_minmax_cmp(iv[i], &mut i_min, &mut i_max);
            rf_minmax_cmp(qv[i], &mut q_min, &mut q_max);
        }

        let i_delta = if (i_max < 0x200 && i_min < 0x200)
            || (i_max >= 0x200 && i_min >= 0x200)
        {
            i_max - i_min
        } else {
            i_max + (0x400 - i_min)
        };
        let q_delta = if (q_max < 0x200 && q_min < 0x200)
            || (q_max >= 0x200 && q_min >= 0x200)
        {
            q_max - q_min
        } else {
            q_max + (0x400 - q_min)
        };

        dac_iq_sort(iv, qv);

        if i_delta > 5 || q_delta > 5 {
            let temp = host::r32_mask(h, 0x2dbc, 0x3fffff);
            iv[0] = (temp & 0x3ff000) >> 12;
            qv[0] = temp & 0x3ff;
            let temp = host::r32_mask(h, 0x2dbc, 0x3fffff);
            iv[DACK_SN_8822C - 1] = (temp & 0x3ff000) >> 12;
            qv[DACK_SN_8822C - 1] = temp & 0x3ff;
        } else {
            break;
        }

        // `while (cnt++ < 100)`: post-increment, so 101 iterations.
        if cnt >= 100 {
            break;
        }
        cnt += 1;
    }

    (dac_iq_offset(iv), dac_iq_offset(qv))
}

/// rtw8822c.c:362-376 `rtw8822c_dac_cal_rf_mode`.
///
/// The two `rtw_read_rf` at the start exist in Linux only for the debug
/// line after them, but an RF register read is a real bus access on this
/// chip, so dropping them would change the access order.
fn dac_cal_rf_mode(h: i32, verbose: bool) -> (u32, u32) {
    let _rf_a = read_rf(h, RF_PATH_A, 0x0, RFREG_MASK);
    let _rf_b = read_rf(h, RF_PATH_B, 0x0, RFREG_MASK);

    let mut iv = [0u32; DACK_SN_8822C];
    let mut qv = [0u32; DACK_SN_8822C];
    dac_cal_iq_sample(h, &mut iv, &mut qv, verbose);
    dac_cal_iq_search(h, &mut iv, &mut qv)
}

/// rtw8822c.c:378-392 `rtw8822c_dac_bb_setting`
fn dac_bb_setting(h: i32) {
    host::w32_mask(h, 0x1d58, 0xff8, 0x1ff);
    host::w32_mask(h, 0x1a00, 0x3, 0x2);
    host::w32_mask(h, 0x1a14, 0x300, 0x3);
    host::w32(h, 0x1d70, 0x7e7e7e7e);
    host::w32_mask(h, 0x180c, 0x3, 0x0);
    host::w32_mask(h, 0x410c, 0x3, 0x0);
    host::w32(h, 0x1b00, 0x0000_0008);
    host::w8(h, 0x1bcc, 0x3f);
    host::w32(h, 0x1b00, 0x0000_000a);
    host::w8(h, 0x1bcc, 0x3f);
    host::w32_mask(h, 0x1e24, 1 << 31, 0x0);
    host::w32_mask(h, 0x1e28, 0xf, 0x3);
}

/// rtw8822c.c:394-470 `rtw8822c_dac_cal_adc`
fn dac_cal_adc(h: i32, dm: &mut DmInfo, path: usize) -> (u32, u32) {
    let mut adc_ic = 0u32;
    let mut adc_qc = 0u32;

    let base_addr = path_write_addr(path);
    let path_sel = match path {
        RF_PATH_A => 0xa0000u32,
        RF_PATH_B => 0x80000u32,
        _ => return (0, 0),
    };

    // ADCK step1
    host::w32_mask(h, base_addr + 0x30, 1 << 30, 0x0);
    if path == RF_PATH_B {
        host::w32(h, base_addr + 0x30, 0x30db_8041);
    }
    host::w32(h, base_addr + 0x60, 0xf004_0ff0);
    host::w32(h, base_addr + 0x0c, 0xdff0_0220);
    host::w32(h, base_addr + 0x10, 0x02dd_08c4);
    host::w32(h, base_addr + 0x0c, 0x1000_0260);
    wrf(h, RF_PATH_A, 0x0, 0x10000);
    wrf(h, RF_PATH_B, 0x0, 0x10000);

    host::print("    ADCK ");
    host::print(if path == RF_PATH_A { "A" } else { "B" });
    host::print(" Runden (Restversatz |i|/|q|, Abbruch unter 5):\n");
    let mut converged = false;
    for round in 0..10 {
        host::w32(h, 0x1c3c, path_sel + 0x8003);
        host::w32(h, 0x1c24, 0x0001_0002);
        let (mut ic, mut qc) = dac_cal_rf_mode(h, round == 0);

        // compensation value
        if ic != 0x0 {
            ic = 0x400 - ic;
            adc_ic = ic;
        }
        if qc != 0x0 {
            qc = 0x400 - qc;
            adc_qc = qc;
        }
        let temp = (ic & 0x3ff) | ((qc & 0x3ff) << 10);
        host::w32(h, base_addr + 0x68, temp);
        dm.dack_adck[path] = temp;

        // check ADC DC offset
        host::w32(h, 0x1c3c, path_sel + 0x8103);
        let (mut ic, mut qc) = dac_cal_rf_mode(h, false);
        if ic >= 0x200 {
            ic = 0x400 - ic;
        }
        if qc >= 0x200 {
            qc = 0x400 - qc;
        }
        host::print("      -> ");
        host::print_dec(ic);
        host::print("/");
        host::print_dec(qc);
        host::print("  (Ausgleich 0x");
        host::print_hex32(dm.dack_adck[path]);
        host::print(")\n");
        if ic < 5 && qc < 5 {
            converged = true;
            break;
        }
    }
    host::print(if converged {
        "      ADCK konvergiert\n"
    } else {
        "      ADCK NICHT konvergiert\n"
    });

    // ADCK step2
    host::w32(h, 0x1c3c, 0x0000_0003);
    host::w32(h, base_addr + 0x0c, 0x1000_0260);
    host::w32(h, base_addr + 0x10, 0x02d5_08c4);

    // release pull low switch on IQ path
    write_rf_reg_mix(h, path, 0x8f, 1 << 13, 0x1);

    (adc_ic, adc_qc)
}

/// rtw8822c.c:472-515 `rtw8822c_dac_cal_step1`
fn dac_cal_step1(h: i32, dm: &DmInfo, path: usize) {
    let base_addr = path_write_addr(path);
    let read_addr = path_read_addr(path);

    host::w32(h, base_addr + 0x68, dm.dack_adck[path]);
    host::w32(h, base_addr + 0x0c, 0xdff0_0220);
    if path == RF_PATH_A {
        host::w32(h, base_addr + 0x60, 0xf004_0ff0);
        host::w32(h, 0x1c38, 0xffff_ffff);
    }
    host::w32(h, base_addr + 0x10, 0x02d5_08c5);
    host::w32(h, 0x9b4, 0xdb66_db00);
    host::w32(h, base_addr + 0xb0, 0x0a11_fb88);
    host::w32(h, base_addr + 0xbc, 0x0008_ff81);
    host::w32(h, base_addr + 0xc0, 0x0003_d208);
    host::w32(h, base_addr + 0xcc, 0x0a11_fb88);
    host::w32(h, base_addr + 0xd8, 0x0008_ff81);
    host::w32(h, base_addr + 0xdc, 0x0003_d208);
    host::w32(h, base_addr + 0xb8, 0x6000_0000);
    host::sleep_ms(2);
    host::w32(h, base_addr + 0xbc, 0x000a_ff8d);
    host::sleep_ms(2);
    host::w32(h, base_addr + 0xb0, 0x0a11_fb89);
    host::w32(h, base_addr + 0xcc, 0x0a11_fb89);
    host::sleep_ms(1);
    host::w32(h, base_addr + 0xb8, 0x6200_0000);
    host::w32(h, base_addr + 0xd4, 0x6200_0000);
    host::sleep_ms(20);
    if !crate::fw::check_hw_ready(h, read_addr + 0x08, 0x7fff80, 0xffff)
        || !crate::fw::check_hw_ready(h, read_addr + 0x34, 0x7fff80, 0xffff)
    {
        host::print("[rtl8822ce] failed to wait for dack ready\n");
    }
    host::w32(h, base_addr + 0xb8, 0x0200_0000);
    host::sleep_ms(1);
    host::w32(h, base_addr + 0xbc, 0x0008_ff87);
    host::w32(h, 0x9b4, 0xdb6d_b600);
    host::w32(h, base_addr + 0x10, 0x02d5_08c5);
    host::w32(h, base_addr + 0xbc, 0x0008_ff87);
    host::w32(h, base_addr + 0x60, 0xf000_0000);
}

/// rtw8822c.c:517-564 `rtw8822c_dac_cal_step2`
fn dac_cal_step2(h: i32, path: usize) -> (u32, u32) {
    let base_addr = path_write_addr(path);
    host::w32_mask(h, base_addr + 0xbc, 0xf000_0000, 0x0);
    host::w32_mask(h, base_addr + 0xc0, 0xf, 0x8);
    host::w32_mask(h, base_addr + 0xd8, 0xf000_0000, 0x0);
    host::w32_mask(h, base_addr + 0xdc, 0xf, 0x8);

    host::w32(h, 0x1b00, 0x0000_0008);
    host::w8(h, 0x1bcc, 0x03f);
    host::w32(h, base_addr + 0x0c, 0xdff0_0220);
    host::w32(h, base_addr + 0x10, 0x02d5_08c5);
    host::w32(h, 0x1c3c, 0x0008_8103);

    let (ic_in, qc_in) = dac_cal_rf_mode(h, false);
    let mut ic = ic_in;
    let mut qc = qc_in;

    // compensation value
    if ic != 0x0 {
        ic = 0x400 - ic;
    }
    if qc != 0x0 {
        qc = 0x400 - qc;
    }
    // `0x7f - ic` wraps once the measured offset is large enough:
    // `(0x400 - ic) * 12 / 5` can reach 614, and 0x7f is 127. In C this is a
    // u32 wraparound that later ends as a mismatch via `& 0xf` and
    // `check_hw_ready`. It is written as an explicit wrap so an
    // overflow-checked build does not panic on a bad measurement.
    if ic < 0x300 {
        ic = ic * 2 * 6 / 5;
        ic += 0x80;
    } else {
        ic = (0x400 - ic) * 2 * 6 / 5;
        ic = 0x7fu32.wrapping_sub(ic);
    }
    if qc < 0x300 {
        qc = qc * 2 * 6 / 5;
        qc += 0x80;
    } else {
        qc = (0x400 - qc) * 2 * 6 / 5;
        qc = 0x7fu32.wrapping_sub(qc);
    }

    (ic, qc)
}

/// rtw8822c.c:566-641 `rtw8822c_dac_cal_step3`.
///
/// Returns the two values for the termination check (`ic`, `qc` as
/// magnitudes) and the two raw values for the debug line (`i_out`, `q_out`).
fn dac_cal_step3(h: i32, path: usize, adc_ic: u32, adc_qc: u32,
                 ic_in: u32, qc_in: u32) -> (u32, u32, u32, u32) {
    let base_addr = path_write_addr(path);
    let read_addr = path_read_addr(path);
    let ic = ic_in;
    let qc = qc_in;

    host::w32(h, base_addr + 0x0c, 0xdff0_0220);
    host::w32(h, base_addr + 0x10, 0x02d5_08c5);
    host::w32(h, 0x9b4, 0xdb66_db00);
    host::w32(h, base_addr + 0xb0, 0x0a11_fb88);
    host::w32(h, base_addr + 0xbc, 0xc008_ff81);
    host::w32(h, base_addr + 0xc0, 0x0003_d208);
    host::w32_mask(h, base_addr + 0xbc, 0xf000_0000, ic & 0xf);
    host::w32_mask(h, base_addr + 0xc0, 0xf, (ic & 0xf0) >> 4);
    host::w32(h, base_addr + 0xcc, 0x0a11_fb88);
    host::w32(h, base_addr + 0xd8, 0xe008_ff81);
    host::w32(h, base_addr + 0xdc, 0x0003_d208);
    host::w32_mask(h, base_addr + 0xd8, 0xf000_0000, qc & 0xf);
    host::w32_mask(h, base_addr + 0xdc, 0xf, (qc & 0xf0) >> 4);
    host::w32(h, base_addr + 0xb8, 0x6000_0000);
    host::sleep_ms(2);
    host::w32_mask(h, base_addr + 0xbc, 0xe, 0x6);
    host::sleep_ms(2);
    host::w32(h, base_addr + 0xb0, 0x0a11_fb89);
    host::w32(h, base_addr + 0xcc, 0x0a11_fb89);
    host::sleep_ms(1);
    host::w32(h, base_addr + 0xb8, 0x6200_0000);
    host::w32(h, base_addr + 0xd4, 0x6200_0000);
    host::sleep_ms(20);
    if !crate::fw::check_hw_ready(h, read_addr + 0x24, 0x07f8_0000, ic)
        || !crate::fw::check_hw_ready(h, read_addr + 0x50, 0x07f8_0000, qc)
    {
        host::print("[rtl8822ce] failed to write IQ vector to hardware\n");
    }
    host::w32(h, base_addr + 0xb8, 0x0200_0000);
    host::sleep_ms(1);
    host::w32_mask(h, base_addr + 0xbc, 0xe, 0x3);
    host::w32(h, 0x9b4, 0xdb6d_b600);

    // check DAC DC offset
    let temp = ((adc_ic + 0x10) & 0x3ff) | (((adc_qc + 0x10) & 0x3ff) << 10);
    host::w32(h, base_addr + 0x68, temp);
    host::w32(h, base_addr + 0x10, 0x02d5_08c5);
    host::w32(h, base_addr + 0x60, 0xf000_0000);
    let (mut ic, mut qc) = dac_cal_rf_mode(h, false);

    ic = if ic >= 0x10 { ic - 0x10 } else { 0x400 - (0x10 - ic) };
    qc = if qc >= 0x10 { qc - 0x10 } else { 0x400 - (0x10 - qc) };

    let i_out = ic;
    let q_out = qc;

    if ic >= 0x200 {
        ic = 0x400 - ic;
    }
    if qc >= 0x200 {
        qc = 0x400 - qc;
    }

    (ic, qc, i_out, q_out)
}

/// rtw8822c.c:643-651 `rtw8822c_dac_cal_step4`
fn dac_cal_step4(h: i32, path: usize) {
    let base_addr = path_write_addr(path);
    host::w32(h, base_addr + 0x68, 0x0);
    host::w32(h, base_addr + 0x10, 0x02d5_08c4);
    host::w32_mask(h, base_addr + 0xbc, 0x1, 0x0);
    host::w32_mask(h, base_addr + 0x30, 1 << 30, 0x1);
}

/// rtw8822c.c:653-668 `rtw8822c_dac_cal_backup_vec`
fn dac_cal_backup_vec(h: i32, dm: &mut DmInfo, path: usize, vec: usize,
                      w_addr: u32, r_addr: u32) {
    if vec >= 2 {
        return;
    }
    for i in 0..DACK_MSBK_BACKUP_NUM {
        host::w32_mask(h, w_addr, 0xf000_0000, i as u32);
        let val = host::r32_mask(h, r_addr, 0x7fc_0000) as u16;
        dm.dack_msbk[path][vec][i] = val;
    }
}

/// rtw8822c.c:670-688 `rtw8822c_dac_cal_backup_path`
fn dac_cal_backup_path(h: i32, dm: &mut DmInfo, path: usize) {
    const W_OFF: u32 = 0x1c;
    const R_OFF: u32 = 0x2c;
    if path >= 2 {
        return;
    }
    // backup I vector
    let w_addr = path_write_addr(path) + 0xb0;
    let r_addr = path_read_addr(path) + 0x10;
    dac_cal_backup_vec(h, dm, path, 0, w_addr, r_addr);

    // backup Q vector
    let w_addr = path_write_addr(path) + 0xb0 + W_OFF;
    let r_addr = path_read_addr(path) + 0x10 + R_OFF;
    dac_cal_backup_vec(h, dm, path, 1, w_addr, r_addr);
}

/// rtw8822c.c:690-712 `rtw8822c_dac_cal_backup_dck`.
///
/// Path B's indices are swapped relative to path A: A uses
/// `[0][0] [0][1] [1][0] [1][1]`, B `[0][0] [1][0] [0][1] [1][1]`. That is
/// Linux, and `restore_dck` reads back with the same swap, so it cancels.
fn dac_cal_backup_dck(h: i32, dm: &mut DmInfo) {
    dm.dack_dck[RF_PATH_A][0][0] = host::r32_mask(h, REG_DCKA_I_0, 0xf000_0000) as u8;
    dm.dack_dck[RF_PATH_A][0][1] = host::r32_mask(h, REG_DCKA_I_1, 0xf) as u8;
    dm.dack_dck[RF_PATH_A][1][0] = host::r32_mask(h, REG_DCKA_Q_0, 0xf000_0000) as u8;
    dm.dack_dck[RF_PATH_A][1][1] = host::r32_mask(h, REG_DCKA_Q_1, 0xf) as u8;

    dm.dack_dck[RF_PATH_B][0][0] = host::r32_mask(h, REG_DCKB_I_0, 0xf000_0000) as u8;
    dm.dack_dck[RF_PATH_B][1][0] = host::r32_mask(h, REG_DCKB_I_1, 0xf) as u8;
    dm.dack_dck[RF_PATH_B][0][1] = host::r32_mask(h, REG_DCKB_Q_0, 0xf000_0000) as u8;
    dm.dack_dck[RF_PATH_B][1][1] = host::r32_mask(h, REG_DCKB_Q_1, 0xf) as u8;
}

/// rtw8822c.c:714-742 `rtw8822c_dac_cal_backup`
fn dac_cal_backup(h: i32, dm: &mut DmInfo) {
    let temp = [host::r32(h, 0x1860), host::r32(h, 0x4160), host::r32(h, 0x9b4)];

    // set clock
    host::w32(h, 0x9b4, 0xdb66_db00);

    // backup path-A I/Q
    host::clr32(h, 0x1830, 1 << 30);
    host::w32_mask(h, 0x1860, 0xfc00_0000, 0x3c);
    dac_cal_backup_path(h, dm, RF_PATH_A);

    // backup path-B I/Q
    host::clr32(h, 0x4130, 1 << 30);
    host::w32_mask(h, 0x4160, 0xfc00_0000, 0x3c);
    dac_cal_backup_path(h, dm, RF_PATH_B);

    dac_cal_backup_dck(h, dm);
    host::set32(h, 0x1830, 1 << 30);
    host::set32(h, 0x4130, 1 << 30);

    host::w32(h, 0x1860, temp[0]);
    host::w32(h, 0x4160, temp[1]);
    host::w32(h, 0x9b4, temp[2]);
}

/// rtw8822c.c:744-772 `rtw8822c_dac_cal_restore_dck`
fn dac_cal_restore_dck(h: i32, dm: &DmInfo) {
    host::set32(h, REG_DCKA_I_0, 1 << 19);
    host::w32_mask(h, REG_DCKA_I_0, 0xf000_0000, dm.dack_dck[RF_PATH_A][0][0] as u32);
    host::w32_mask(h, REG_DCKA_I_1, 0xf, dm.dack_dck[RF_PATH_A][0][1] as u32);

    host::set32(h, REG_DCKA_Q_0, 1 << 19);
    host::w32_mask(h, REG_DCKA_Q_0, 0xf000_0000, dm.dack_dck[RF_PATH_A][1][0] as u32);
    host::w32_mask(h, REG_DCKA_Q_1, 0xf, dm.dack_dck[RF_PATH_A][1][1] as u32);

    host::set32(h, REG_DCKB_I_0, 1 << 19);
    host::w32_mask(h, REG_DCKB_I_0, 0xf000_0000, dm.dack_dck[RF_PATH_B][0][0] as u32);
    host::w32_mask(h, REG_DCKB_I_1, 0xf, dm.dack_dck[RF_PATH_B][0][1] as u32);

    host::set32(h, REG_DCKB_Q_0, 1 << 19);
    host::w32_mask(h, REG_DCKB_Q_0, 0xf000_0000, dm.dack_dck[RF_PATH_B][1][0] as u32);
    host::w32_mask(h, REG_DCKB_Q_1, 0xf, dm.dack_dck[RF_PATH_B][1][1] as u32);
}

/// rtw8822c.c:774-828 `rtw8822c_dac_cal_restore_prepare`
fn dac_cal_restore_prepare(h: i32, dm: &DmInfo) {
    host::w32(h, 0x9b4, 0xdb66_db00);

    host::w32_mask(h, 0x18b0, 1 << 27, 0x0);
    host::w32_mask(h, 0x18cc, 1 << 27, 0x0);
    host::w32_mask(h, 0x41b0, 1 << 27, 0x0);
    host::w32_mask(h, 0x41cc, 1 << 27, 0x0);

    host::w32_mask(h, 0x1830, 1 << 30, 0x0);
    host::w32_mask(h, 0x1860, 0xfc00_0000, 0x3c);
    host::w32_mask(h, 0x18b4, 1 << 0, 0x1);
    host::w32_mask(h, 0x18d0, 1 << 0, 0x1);

    host::w32_mask(h, 0x4130, 1 << 30, 0x0);
    host::w32_mask(h, 0x4160, 0xfc00_0000, 0x3c);
    host::w32_mask(h, 0x41b4, 1 << 0, 0x1);
    host::w32_mask(h, 0x41d0, 1 << 0, 0x1);

    host::w32_mask(h, 0x18b0, 0xf00, 0x0);
    host::w32_mask(h, 0x18c0, 1 << 14, 0x0);
    host::w32_mask(h, 0x18cc, 0xf00, 0x0);
    host::w32_mask(h, 0x18dc, 1 << 14, 0x0);

    host::w32_mask(h, 0x18b0, 1 << 0, 0x0);
    host::w32_mask(h, 0x18cc, 1 << 0, 0x0);
    host::w32_mask(h, 0x18b0, 1 << 0, 0x1);
    host::w32_mask(h, 0x18cc, 1 << 0, 0x1);

    dac_cal_restore_dck(h, dm);

    host::w32_mask(h, 0x18c0, 0x38000, 0x7);
    host::w32_mask(h, 0x18dc, 0x38000, 0x7);
    host::w32_mask(h, 0x41c0, 0x38000, 0x7);
    host::w32_mask(h, 0x41dc, 0x38000, 0x7);

    host::w32_mask(h, 0x18b8, (1 << 26) | (1 << 25), 0x1);
    host::w32_mask(h, 0x18d4, (1 << 26) | (1 << 25), 0x1);

    host::w32_mask(h, 0x41b0, 0xf00, 0x0);
    host::w32_mask(h, 0x41c0, 1 << 14, 0x0);
    host::w32_mask(h, 0x41cc, 0xf00, 0x0);
    host::w32_mask(h, 0x41dc, 1 << 14, 0x0);

    host::w32_mask(h, 0x41b0, 1 << 0, 0x0);
    host::w32_mask(h, 0x41cc, 1 << 0, 0x0);
    host::w32_mask(h, 0x41b0, 1 << 0, 0x1);
    host::w32_mask(h, 0x41cc, 1 << 0, 0x1);

    host::w32_mask(h, 0x41b8, (1 << 26) | (1 << 25), 0x1);
    host::w32_mask(h, 0x41d4, (1 << 26) | (1 << 25), 0x1);
}

/// rtw8822c.c:830-845 `rtw8822c_dac_cal_restore_wait`
fn dac_cal_restore_wait(h: i32, target_addr: u32, toggle_addr: u32) -> bool {
    let mut cnt = 0u32;
    loop {
        host::w32_mask(h, toggle_addr, (1 << 26) | (1 << 25), 0x0);
        host::w32_mask(h, toggle_addr, (1 << 26) | (1 << 25), 0x2);
        if host::r32_mask(h, target_addr, 0xf) == 0x6 {
            return true;
        }
        if cnt >= 100 {
            return false;
        }
        cnt += 1;
    }
}

/// rtw8822c.c:847-891 `rtw8822c_dac_cal_restore_path`
fn dac_cal_restore_path(h: i32, dm: &DmInfo, path: usize) -> bool {
    const W_OFF: u32 = 0x1c;
    const R_OFF: u32 = 0x2c;

    let w_i = path_write_addr(path) + 0xb0;
    let r_i = path_read_addr(path) + 0x08;
    let w_q = path_write_addr(path) + 0xb0 + W_OFF;
    let r_q = path_read_addr(path) + 0x08 + R_OFF;

    if !dac_cal_restore_wait(h, r_i, w_i + 0x8) {
        return false;
    }
    for i in 0..DACK_MSBK_BACKUP_NUM {
        host::w32_mask(h, w_i + 0x4, 1 << 2, 0x0);
        let value = dm.dack_msbk[path][0][i] as u32;
        host::w32_mask(h, w_i + 0x4, 0xff8, value);
        host::w32_mask(h, w_i, 0xf000_0000, i as u32);
        host::w32_mask(h, w_i + 0x4, 1 << 2, 0x1);
    }
    host::w32_mask(h, w_i + 0x4, 1 << 2, 0x0);

    if !dac_cal_restore_wait(h, r_q, w_q + 0x8) {
        return false;
    }
    for i in 0..DACK_MSBK_BACKUP_NUM {
        host::w32_mask(h, w_q + 0x4, 1 << 2, 0x0);
        let value = dm.dack_msbk[path][1][i] as u32;
        host::w32_mask(h, w_q + 0x4, 0xff8, value);
        host::w32_mask(h, w_q, 0xf000_0000, i as u32);
        host::w32_mask(h, w_q + 0x4, 1 << 2, 0x1);
    }
    host::w32_mask(h, w_q + 0x4, 1 << 2, 0x0);

    host::w32_mask(h, w_i + 0x8, (1 << 26) | (1 << 25), 0x0);
    host::w32_mask(h, w_q + 0x8, (1 << 26) | (1 << 25), 0x0);
    host::w32_mask(h, w_i + 0x4, 1 << 0, 0x0);
    host::w32_mask(h, w_q + 0x4, 1 << 0, 0x0);

    true
}

/// rtw8822c.c:893-902 `__rtw8822c_dac_cal_restore`
fn dac_cal_restore_both(h: i32, dm: &DmInfo) -> bool {
    dac_cal_restore_path(h, dm, RF_PATH_A) && dac_cal_restore_path(h, dm, RF_PATH_B)
}

/// rtw8822c.c:904-941 `rtw8822c_dac_cal_restore`
fn dac_cal_restore(h: i32, dm: &DmInfo) -> bool {
    // sample the first element for both path's IQ vector
    if dm.dack_msbk[RF_PATH_A][0][0] == 0
        && dm.dack_msbk[RF_PATH_A][1][0] == 0
        && dm.dack_msbk[RF_PATH_B][0][0] == 0
        && dm.dack_msbk[RF_PATH_B][1][0] == 0
    {
        return false;
    }

    let temp = [host::r32(h, 0x1860), host::r32(h, 0x4160), host::r32(h, 0x9b4)];

    dac_cal_restore_prepare(h, dm);
    if !crate::fw::check_hw_ready(h, 0x2808, 0x7fff80, 0xffff)
        || !crate::fw::check_hw_ready(h, 0x2834, 0x7fff80, 0xffff)
        || !crate::fw::check_hw_ready(h, 0x4508, 0x7fff80, 0xffff)
        || !crate::fw::check_hw_ready(h, 0x4534, 0x7fff80, 0xffff)
    {
        return false;
    }

    if !dac_cal_restore_both(h, dm) {
        host::print("[rtl8822ce] failed to restore dack vectors\n");
        return false;
    }

    host::w32_mask(h, 0x1830, 1 << 30, 0x1);
    host::w32_mask(h, 0x4130, 1 << 30, 0x1);
    host::w32(h, 0x1860, temp[0]);
    host::w32(h, 0x4160, temp[1]);
    host::w32_mask(h, 0x18b0, 1 << 27, 0x1);
    host::w32_mask(h, 0x18cc, 1 << 27, 0x1);
    host::w32_mask(h, 0x41b0, 1 << 27, 0x1);
    host::w32_mask(h, 0x41cc, 1 << 27, 0x1);
    host::w32(h, 0x9b4, temp[2]);

    true
}

/// rtw8822c.c:943-1008 `rtw8822c_rf_dac_cal`
pub fn rf_dac_cal(h: i32, dm: &mut DmInfo) -> bool {
    // The tables write different values per path to RF 0x3e, a register
    // nothing touches afterwards: A=0x3, B=0x20 (rtw8822c_table.c, checked
    // against the condition walker). If both paths read the same, path
    // addressing itself is broken.
    host::print("    RF 0x3e (Tabelle: A=0x3, B=0x20):  A=0x");
    host::print_hex32(read_rf(h, RF_PATH_A, 0x3e, RFREG_MASK));
    host::print("  B=0x");
    host::print_hex32(read_rf(h, RF_PATH_B, 0x3e, RFREG_MASK));
    host::print("\n");

    if dac_cal_restore(h, dm) {
        host::print("    DACK aus dem Zwischenspeicher wiederhergestellt\n");
        return true;
    }

    // not able to restore, do it
    let (backup, backup_rf) = dac_backup_reg(h);
    dac_bb_setting(h);

    let mut ic_a = 0u32;
    let mut qc_a = 0u32;
    let mut ic_b = 0u32;
    let mut qc_b = 0u32;
    let mut i_a = 0u32;
    let mut q_a = 0u32;
    let mut i_b = 0u32;
    let mut q_b = 0u32;

    // path-A
    let (adc_ic_a, adc_qc_a) = dac_cal_adc(h, dm, RF_PATH_A);
    let conv_a = dac_cal_loop(h, dm, RF_PATH_A, adc_ic_a, adc_qc_a,
                              &mut ic_a, &mut qc_a, &mut i_a, &mut q_a);
    dac_cal_step4(h, RF_PATH_A);

    // path-B
    let (adc_ic_b, adc_qc_b) = dac_cal_adc(h, dm, RF_PATH_B);
    let conv_b = dac_cal_loop(h, dm, RF_PATH_B, adc_ic_b, adc_qc_b,
                              &mut ic_b, &mut qc_b, &mut i_b, &mut q_b);
    dac_cal_step4(h, RF_PATH_B);

    host::w32(h, 0x1b00, 0x0000_0008);
    host::w32_mask(h, 0x4130, 1 << 30, 0x1);
    host::w8(h, 0x1bcc, 0x0);
    host::w32(h, 0x1b00, 0x0000_000a);
    host::w8(h, 0x1bcc, 0x0);

    dac_restore_reg(h, &backup, &backup_rf);

    // backup results to restore, saving a lot of time
    dac_cal_backup(h, dm);

    // Linux prints these four numbers as debug lines. They are the only
    // indication whether the calibration converged; `ic`/`qc` below 5 means
    // yes.
    host::print("    DACK A: ic=0x");
    host::print_hex32(ic_a);
    host::print(" qc=0x");
    host::print_hex32(qc_a);
    host::print(" i=0x");
    host::print_hex32(i_a);
    host::print(" q=0x");
    host::print_hex32(q_a);
    host::print("\n    DACK B: ic=0x");
    host::print_hex32(ic_b);
    host::print(" qc=0x");
    host::print_hex32(qc_b);
    host::print(" i=0x");
    host::print_hex32(i_b);
    host::print(" q=0x");
    host::print_hex32(q_b);
    host::print("\n");

    conv_a && conv_b
}

/// The ten iterations from `rtw8822c_rf_dac_cal` for one path.
///
/// Linux writes this loop out twice and reports nothing about its outcome.
/// Here every iteration reports the value the termination depends on
/// (`ic`/`qc` from step3, the magnitude of the residual offset), so it is
/// visible whether it oscillates, is stuck, or converges slowly.
#[allow(clippy::too_many_arguments)]
fn dac_cal_loop(h: i32, dm: &DmInfo, path: usize, adc_ic: u32, adc_qc: u32,
                ic_out: &mut u32, qc_out: &mut u32,
                i_out: &mut u32, q_out: &mut u32) -> bool {
    host::print("    DACK ");
    host::print(if path == RF_PATH_A { "A" } else { "B" });
    host::print(" Runden (Restversatz |i|/|q|, Abbruch unter 5):");
    let mut converged = false;
    for _ in 0..10 {
        dac_cal_step1(h, dm, path);
        let (ic, qc) = dac_cal_step2(h, path);
        *ic_out = ic;
        *qc_out = qc;
        let (ic, qc, io, qo) = dac_cal_step3(h, path, adc_ic, adc_qc, ic, qc);
        *i_out = io;
        *q_out = qo;
        host::print(" ");
        host::print_dec(ic);
        host::print("/");
        host::print_dec(qc);
        if ic < 5 && qc < 5 {
            converged = true;
            break;
        }
    }
    host::print(if converged { "  -> konvergiert\n" } else { "  -> NICHT konvergiert\n" });
    converged
}
