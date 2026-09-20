//! `rtw8822c.c`, Zeilen 108-1010 — die DAC-Kalibrierung (DACK).
//!
//! Portiert, in Quellreihenfolge: `rtw8822c_dac_backup_reg` ·
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
//! **Was die Kalibrierung tut:** sie misst den Gleichspannungsversatz von
//! ADC und DAC beider Pfade und traegt den Ausgleich in die Hardware ein.
//! Ohne sie steht auf jedem Empfangspfad ein konstanter Fehler.
//!
//! **`dac_cal_restore` greift bei uns nie.** Es rettet das Ergebnis eines
//! FRUEHEREN Laufs ueber ein Aus- und Wiedereinschalten — `dack_msbk` ist
//! beim ersten Lauf null, und die Funktion steigt genau daran aus. Portiert
//! ist sie trotzdem vollstaendig: sobald der Treiber den Chip ein zweites
//! Mal anwirft, spart sie die ganze Messung.
#![allow(dead_code)]

use crate::dm::DmInfo;
use crate::host;
use crate::phy::{read_rf, write_rf_reg_mix, RFREG_MASK, RF_PATH_A, RF_PATH_B};
use crate::regs::*;

/// `rtw_write_rf(..., RFREG_MASK, v)` — die Form, in der die DACK schreibt.
fn wrf(h: i32, path: usize, addr: u32, val: u32) {
    write_rf_reg_mix(h, path, addr, RFREG_MASK, val);
}

/// `struct rtw_backup_info` (main.h), wie in `mac.rs` — aber hier sind alle
/// Eintraege 4 Byte breit, also braucht es kein `len`.
#[derive(Clone, Copy, Default)]
struct Backup {
    reg: u32,
    val: u32,
}

/// rtw8822c.c:115-120 — die sechzehn BB-Register, die die DACK verstellt.
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
/// **Der Index `backup_rf[path * i + i]` ist Linux' eigener und er ist
/// schraeg** — mit `DACK_RF_8822C == 1` laeuft `i` nur ueber 0, also ist
/// `path * 0 + 0` fuer BEIDE Pfade die 0. Der zweite Pfad ueberschreibt den
/// ersten, und `restore_reg` liest mit demselben Ausdruck zurueck. Der
/// Ausdruck steht hier unveraendert: er ist harmlos, solange
/// `DACK_RF_8822C` 1 ist, und ihn stillschweigend zu „reparieren" hiesse,
/// eine andere Reihenfolge zu schreiben als Linux fuehrt.
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
    // util.c `rtw_restore_reg`, hier durchgehend 4 Byte.
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
/// Die Werte sind Zehn-Bit-Zweierkomplement: alles ab 0x200 ist negativ.
/// Deshalb ist „kleiner" hier nicht die Zahlenordnung, und deshalb sieht
/// diese Funktion so aus, wie sie aussieht.
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

/// rtw8822c.c:198-209 `rtw8822c_dac_iq_sort` — Bubblesort ueber beide Felder.
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

/// rtw8822c.c:211-234 `rtw8822c_dac_iq_offset` — der Mittelwert der
/// mittleren 80 von 100 Proben, wieder im Zehn-Bit-Zweierkomplement.
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

/// rtw8822c.c:274-285 `rtw8822c_dac_iq_check` — eine Probe, deren Betrag
/// ueber 0x64 liegt, ist ein Ueberlauf und wird verworfen.
fn dac_iq_check(value: u32) -> bool {
    !((value >= 0x200 && (0x400 - value) > 0x64) || (value < 0x200 && value > 0x64))
}

/// rtw8822c.c:287-302 `rtw8822c_dac_cal_iq_sample`.
///
/// Der Deckel von 10000 ist Linux'; ohne ihn haengt die Schleife, wenn die
/// Hardware nur Ueberlaeufe liefert.
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
        // Die ROHEN Proben, bevor irgendetwas daraus gerechnet wird. Ein
        // Feld aus 100 gleichen Zahlen ist eine eingefrorene Messung; eine
        // echte streut ([[feedback_dump_the_raw_input_before_debugging_the_interpretation]]).
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

        // `while (cnt++ < 100)` — nachgestellte Erhoehung, also 101 Runden.
        if cnt >= 100 {
            break;
        }
        cnt += 1;
    }

    (dac_iq_offset(iv), dac_iq_offset(qv))
}

/// rtw8822c.c:362-376 `rtw8822c_dac_cal_rf_mode`.
///
/// Die zwei `rtw_read_rf` am Anfang stehen in Linux nur fuer die Debugzeile
/// dahinter — aber ein Lesezugriff auf ein RF-Register ist bei diesem Chip
/// ein echter Buszugriff, und weglassen hiesse, die Reihenfolge zu aendern.
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
    // `0x7f - ic` LAEUFT UM, sobald der gemessene Versatz gross genug ist:
    // `(0x400 - ic) * 12 / 5` kann bis 614 werden, und 0x7f ist 127. In C
    // ist das ein u32-Umlauf, der danach von `& 0xf` und `check_hw_ready`
    // ohnehin als „passt nicht" endet. Hier steht es ausdruecklich als
    // Umlauf, weil ein Rust-Bau mit Ueberlaufpruefung sonst PANISCH endet —
    // und ein Treiber, der an einer Messung stirbt, ist schlimmer als einer,
    // der dieselbe Fehlermeldung wie Linux ausgibt.
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
/// Gibt zurueck: die zwei Werte fuer die Abbruchpruefung (`ic`, `qc` als
/// BETRAG) und die zwei Rohwerte fuer die Debugzeile (`i_out`, `q_out`).
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
/// **Die Indizes von Pfad B sind vertauscht gegenueber Pfad A** — bei A
/// steht `[0][0] [0][1] [1][0] [1][1]`, bei B `[0][0] [1][0] [0][1] [1][1]`.
/// Das steht so in Linux, und `restore_dck` liest in DERSELBEN Verdrehung
/// zurueck, also hebt es sich auf. Geradegezogen waere es eine Abweichung.
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
    // Die Tabellen schreiben auf RF 0x3e als EINZIGES Register, das danach
    // niemand mehr anfasst, verschiedene Werte je Pfad: A=0x3, B=0x20
    // (rtw8822c_table.c, gegen den Bedingungslaeufer nachgerechnet). Lesen
    // beide Pfade dasselbe, ist der Pfadzugriff selbst falsch — und dann
    // ist jede Aussage ueber "Pfad B" wertlos.
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

    // Linux gibt diese vier Zahlen als Debugzeilen aus. Sie sind die einzige
    // Auskunft darueber, ob die Kalibrierung konvergiert ist — ein `ic`/`qc`
    // unter 5 heisst ja.
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

/// Die zehn Runden aus `rtw8822c_rf_dac_cal`, fuer EINEN Pfad.
///
/// In Linux steht diese Schleife zweimal ausgeschrieben da und sagt NICHTS
/// darueber, wie sie ausgegangen ist — sie laeuft zehnmal und geht weiter.
/// Hier meldet sie jede Runde, und zwar den Wert, an dem der Abbruch haengt
/// (`ic`/`qc` aus step3, der BETRAG des restlichen Versatzes). Der erste
/// Geraetelauf sagte fuer Pfad B 36/40 statt unter 5 — und aus einer
/// Endzahl allein ist nicht zu sehen, ob es schwingt, feststeht oder
/// langsam faellt ([[feedback_dump_the_raw_input_before_debugging_the_interpretation]]).
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
