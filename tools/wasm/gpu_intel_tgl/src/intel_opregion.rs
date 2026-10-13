//! display/intel_opregion.c `intel_opregion_setup` with its SWSCI mailbox,
//! and `intel_bios_is_valid_vbt` from display/intel_bios.c (Linux 6.18.55).
//!
//! The OpRegion is ACPI NVS memory the firmware shares with the driver; the
//! card's ASLS config register holds its address. Linux maps it write-back;
//! `npk_mmio_map_phys` maps it uncached, which changes speed, not contents.
//! The VBT is copied out into module memory for parsing.

use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables;
use npk_sys::cell::Single;
use npk_sys::Mmio;

/// The largest VBT the module keeps (RVDA may point outside the 8 KB
/// OpRegion).
pub const VBT_MAX: usize = 64 * 1024;

static VBT_BUF: Single<[u8; VBT_MAX]> = Single::new([0; VBT_MAX]);

/// `struct intel_opregion`, as far as setup fills it.
pub struct intel_opregion {
    pub base: Mmio,
    /// ASLS within its page; every offset below is relative to ASLS.
    pub skew: u64,
    pub asls: u32,
    pub major: u8,
    pub minor: u8,
    pub revision: u8,
    pub mboxes: u32,
    pub has_acpi: bool,
    pub has_swsci: bool,
    pub has_asle: bool,
    pub has_asle_ext: bool,
    pub swsci_gbda_sub_functions: u32,
    pub swsci_sbcb_sub_functions: u32,
    pub vbt_len: usize,
    pub vbt_source: &'static str,
}

fn rd8(m: Mmio, off: u64) -> u8 {
    m.r8(off as u32)
}

/// A little-endian field of `n` bytes at any alignment (the OpRegion
/// structures are packed).
fn rd_le(m: Mmio, off: u64, n: u32) -> u64 {
    (0..n).fold(0u64, |v, i| v | (rd8(m, off + i as u64) as u64) << (8 * i))
}

fn wr32(m: Mmio, off: u64, v: u32) {
    m.w32(off as u32, v);
}

impl intel_opregion {
    fn swsci_off(&self, field: u64) -> u64 {
        self.skew + OPREGION_SWSCI_OFFSET as u64 + field
    }

    /// `check_swsci_function`.
    fn check_swsci_function(&self, function: u32) -> Result<(), i32> {
        if !self.has_swsci {
            return Err(-(ENODEV as i32));
        }
        let main_function = (function & SWSCI_SCIC_MAIN_FUNCTION_MASK) >> SWSCI_SCIC_MAIN_FUNCTION_SHIFT;
        let sub_function = (function & SWSCI_SCIC_SUB_FUNCTION_MASK) >> SWSCI_SCIC_SUB_FUNCTION_SHIFT;
        if main_function == SWSCI_SBCB {
            if self.swsci_sbcb_sub_functions & (1 << sub_function) == 0 {
                return Err(-(EINVAL as i32));
            }
        } else if main_function == SWSCI_GBDA && self.swsci_gbda_sub_functions & (1 << sub_function) == 0 {
            return Err(-(EINVAL as i32));
        }
        Ok(())
    }

    /// `swsci`: post a request into the mailbox and raise the SCI with a
    /// word write to the card's SWSCI config register.
    fn swsci(&self, function: u32, parm: u32, parm_out: Option<&mut u32>) -> Result<(), i32> {
        self.check_swsci_function(function)?;
        let m = self.base;

        let mut dslp = rd_le(m, self.swsci_off(OPREGION_SWSCI_DSLP), 4) as u32;
        if dslp == 0 {
            dslp = 50;
        } else if dslp > MAX_DSLP {
            Line::new().s("ACPI BIOS requests an excessive sleep of ").dec(dslp as u64)
                .s(" ms, using ").dec(MAX_DSLP as u64).s(" ms instead").say();
            dslp = MAX_DSLP;
        }

        let mut scic = rd_le(m, self.swsci_off(OPREGION_SWSCI_SCIC), 4) as u32;
        if scic & SWSCI_SCIC_INDICATOR != 0 {
            return Err(-(EBUSY as i32));
        }
        scic = function | SWSCI_SCIC_INDICATOR;
        wr32(m, self.swsci_off(OPREGION_SWSCI_PARM), parm);
        wr32(m, self.swsci_off(OPREGION_SWSCI_SCIC), scic);

        // Ensure the SCI event is selected and the trigger is cleared.
        let mut swsci_val = (npk_sys::pci_read_config(SWSCI as i32 & !3) as u32 >> ((SWSCI & 2) * 8)) as u16;
        if swsci_val & SWSCI_SCISEL as u16 == 0 || swsci_val & SWSCI_GSSCIE as u16 != 0 {
            swsci_val |= SWSCI_SCISEL as u16;
            swsci_val &= !(SWSCI_GSSCIE as u16);
            npk_sys::pci_write_config16(SWSCI as i32, swsci_val as i32);
        }
        // The event trigger tells the BIOS to check the mail.
        swsci_val |= SWSCI_GSSCIE as u16;
        npk_sys::pci_write_config16(SWSCI as i32, swsci_val as i32);

        // poll_timeout_us(..., 1000, dslp * 1000, false)
        let polled = linux::wait_for_ms(dslp as u64, || {
            scic = rd_le(m, self.swsci_off(OPREGION_SWSCI_SCIC), 4) as u32;
            scic & SWSCI_SCIC_INDICATOR == 0
        });
        if polled.is_err() {
            linux::say("SWSCI request timed out");
            return Err(-(ETIMEDOUT as i32));
        }
        let status = (scic & SWSCI_SCIC_EXIT_STATUS_MASK) >> SWSCI_SCIC_EXIT_STATUS_SHIFT;
        if status != SWSCI_SCIC_EXIT_STATUS_SUCCESS {
            Line::new().s("SWSCI request error ").dec(status as u64).say();
            return Err(-(EIO as i32));
        }
        if let Some(out) = parm_out {
            *out = rd_le(m, self.swsci_off(OPREGION_SWSCI_PARM), 4) as u32;
        }
        Ok(())
    }

    /// `swsci_setup`.
    fn swsci_setup(&mut self) {
        let mut requested_callbacks = false;
        let mut tmp = 0u32;
        self.swsci_gbda_sub_functions = 1;
        self.swsci_sbcb_sub_functions = 1;

        if self.swsci(SWSCI_GBDA_SUPPORTED_CALLS, 0, Some(&mut tmp)).is_ok() {
            tmp <<= 1;
            self.swsci_gbda_sub_functions |= tmp;
        }
        if self.swsci(SWSCI_GBDA_REQUESTED_CALLBACKS, 0, Some(&mut tmp)).is_ok() {
            self.swsci_sbcb_sub_functions |= tmp;
            requested_callbacks = true;
        }
        if self.swsci(SWSCI_SBCB_SUPPORTED_CALLBACKS, 0, Some(&mut tmp)).is_ok() {
            let low = tmp & 0x7ff;
            let high = tmp & !0xfff;
            tmp = (high << 4) | (low << 1) | 1;
            if requested_callbacks {
                let req = self.swsci_sbcb_sub_functions;
                if req & tmp != req {
                    Line::new().s("SWSCI BIOS requested (").hex(req)
                        .s(") SBCB callbacks that are not supported (").hex(tmp).s(")").say();
                }
            } else {
                self.swsci_sbcb_sub_functions |= tmp;
            }
        }
        Line::new().s("SWSCI GBDA callbacks ").hex(self.swsci_gbda_sub_functions)
            .s(", SBCB callbacks ").hex(self.swsci_sbcb_sub_functions).say();
    }
}

/// `intel_bios_is_valid_vbt`, on a VBT copied into module memory.
pub fn intel_bios_is_valid_vbt(buf: &[u8]) -> bool {
    let le16 = |o: usize| u16::from_le_bytes([buf[o], buf[o + 1]]) as usize;
    let le32 = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]) as usize;
    let mut size = buf.len();
    if VBT_HEADER_SIZE as usize > size {
        return false;
    }
    let sig_len = VBT_SIGNATURE_LEN as usize;
    if buf[..sig_len] != tables::vbt_signature[..sig_len] {
        return false;
    }
    let vbt_size = le16(VBT_HEADER_VBT_SIZE as usize);
    if vbt_size > size {
        return false;
    }
    size = vbt_size;
    let bdb_offset = le32(VBT_HEADER_BDB_OFFSET as usize);
    // range_overflows_t(size_t, start, len, max): start >= max || len > max - start
    let overflows = |start: usize, len: usize| start >= size || len > size - start;
    if overflows(bdb_offset, BDB_HEADER_SIZE as usize) {
        return false;
    }
    let bdb_size = le16(bdb_offset + BDB_HEADER_BDB_SIZE as usize);
    !overflows(bdb_offset, bdb_size)
}

/// Copy `len` bytes of physical memory at `phys` into the VBT buffer and
/// check them. The buffer is the VBT from here on.
fn take_vbt(phys: u64, len: usize) -> Option<usize> {
    if len == 0 || len > VBT_MAX {
        return None;
    }
    let page = phys & !0xfff;
    let skew = (phys - page) as usize;
    let pages = (skew + len).div_ceil(4096);
    let m = Mmio::map_phys(page, pages as i32)?;
    VBT_BUF.with(|buf| {
        for i in 0..len {
            buf[i] = m.r8((skew + i) as u32);
        }
        intel_bios_is_valid_vbt(&buf[..len]).then_some(len)
    })
}

/// The VBT the setup found, if any.
pub fn vbt<R>(len: usize, f: impl FnOnce(&[u8]) -> R) -> R {
    VBT_BUF.with(|buf| f(&buf[..len]))
}

/// `intel_opregion_setup`. The ASLE interrupt work and the PMIC/ACPI
/// notifier hooks belong to later stages.
pub fn intel_opregion_setup() -> Option<intel_opregion> {
    let asls = npk_sys::pci_read_config(ASLS as i32) as u32;
    if asls == 0 {
        linux::say("ACPI OpRegion not supported!");
        return None;
    }
    let base = Mmio::map_phys(asls as u64 & !0xfff, ((OPREGION_SIZE as u64 + (asls as u64 & 0xfff)).div_ceil(4096)) as i32)?;
    // The mapping is page-aligned; ASLS may not be.
    let skew = asls as u64 & 0xfff;
    let at = |o: u64| o + skew;

    let sig: [u8; 16] = core::array::from_fn(|i| rd8(base, at(i as u64)));
    if sig[..] != OPREGION_SIGNATURE[..16] {
        linux::say("opregion signature mismatch");
        return None;
    }
    let mut op = intel_opregion {
        base,
        skew,
        asls,
        major: rd8(base, at(OPREGION_HDR_OVER_MAJOR)),
        minor: rd8(base, at(OPREGION_HDR_OVER_MINOR)),
        revision: rd8(base, at(OPREGION_HDR_OVER_REVISION)),
        mboxes: rd_le(base, at(OPREGION_HDR_MBOXES), 4) as u32,
        has_acpi: false,
        has_swsci: false,
        has_asle: false,
        has_asle_ext: false,
        swsci_gbda_sub_functions: 0,
        swsci_sbcb_sub_functions: 0,
        vbt_len: 0,
        vbt_source: "",
    };
    Line::new().s("ACPI OpRegion version ").dec(op.major as u64).s(".").dec(op.minor as u64)
        .s(".").dec(op.revision as u64).s(", mailboxes ").hex(op.mboxes).say();

    if op.mboxes & MBOX_ACPI as u32 != 0 {
        op.has_acpi = true;
        // The driver handles monitor hotplug itself; no ACPI notifications.
        wr32(base, at(OPREGION_ACPI_OFFSET as u64 + OPREGION_ACPI_CHPD), 1);
    }
    if op.mboxes & MBOX_SWSCI as u32 != 0 {
        if op.major >= 3 {
            linux::drm_err("SWSCI Mailbox #2 present for opregion v3.x, ignoring");
        } else {
            op.has_swsci = true;
            op.swsci_setup();
        }
    }
    if op.mboxes & MBOX_ASLE as u32 != 0 {
        op.has_asle = true;
        wr32(base, at(OPREGION_ASLE_OFFSET as u64 + OPREGION_ASLE_ARDY), ASLE_ARDY_NOT_READY);
    }
    if op.mboxes & MBOX_ASLE_EXT as u32 != 0 {
        op.has_asle_ext = true;
    }

    // intel_no_opregion_vbt lists one Lenovo ThinkCentre only.
    if op.major >= 2 && op.has_asle {
        let asle = OPREGION_ASLE_OFFSET as u64;
        let mut rvda = rd_le(base, at(asle + OPREGION_ASLE_RVDA), 8);
        let rvds = rd_le(base, at(asle + OPREGION_ASLE_RVDS), 4) as usize;
        if rvda != 0 && rvds != 0 {
            if op.major > 2 || op.minor >= 1 {
                if rvda < OPREGION_SIZE as u64 {
                    linux::say("warning: RVDA points into the OpRegion");
                }
                rvda += asls as u64;
            }
            if let Some(len) = take_vbt(rvda, rvds) {
                op.vbt_len = len;
                op.vbt_source = "ACPI OpRegion (RVDA)";
                return Some(op);
            }
            linux::say("Invalid VBT in ACPI OpRegion (RVDA)");
        }
    }

    let mut vbt_size = if op.mboxes & MBOX_ASLE_EXT as u32 != 0 { OPREGION_ASLE_EXT_OFFSET } else { OPREGION_SIZE } as usize;
    vbt_size -= OPREGION_VBT_OFFSET as usize;
    if let Some(len) = take_vbt(asls as u64 + OPREGION_VBT_OFFSET as u64, vbt_size) {
        op.vbt_len = len;
        op.vbt_source = "ACPI OpRegion (Mailbox #4)";
    } else {
        linux::say("Invalid VBT in ACPI OpRegion (Mailbox #4)");
    }
    Some(op)
}
