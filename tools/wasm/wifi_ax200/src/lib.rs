//! wifi_ax200 — Intel Wi-Fi 6 AX200 driver (WASM module)
//!
//! iwlwifi-mvm, device family 22000 (gen2). Strict 1:1 port of Linux 6.18.26.
//! Plan: WIFI_AX200.md. Uses the nopeekOS WASM Driver ABI (npk_pci_*,
//! npk_mmio_*, npk_dma_*) — the same ABI proven by the RTL8852BE `wifi` driver.
//!
//! Stage 0a: bind PCI, map BAR0, read HW_REV + HW_RF_ID to confirm the chip.
//! Stage 0b (this file): reset + APM bring-up to MAC-clock-ready, following
//! `_iwl_trans_pcie_start_hw` for family 22000 (non-integrated AX200):
//!   prepare_card_hw → clear_persistence_bit → sw_reset → apm_init(→activate_nic).
//! All register pokes are 1:1 with the Linux source (no guessed values).

#![no_std]

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

mod host;
mod regs;
use regs::*;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// AX200 runtime firmware (unified ucode), embedded like the RTL driver embeds
/// rtw8852b_fw.bin. API 77 = the exact version Linux 6.18.26 requests.
static FW: &[u8] = include_bytes!("../firmware/iwlwifi-cc-a0-77.ucode");

// Little-endian readers over the embedded firmware.
fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
// Little-endian writers into the context-info buffer.
fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
/// u32_encode_bits: place `v` into the field described by `mask`.
fn encode_bits(v: u32, mask: u32) -> u32 {
    (v << mask.trailing_zeros()) & mask
}

/// A coherent DMA allocation: kernel-held physical address + WASM handle.
#[derive(Clone, Copy)]
struct Dma {
    handle: i32,
    phys: u64,
}
impl Dma {
    const NONE: Dma = Dma { handle: -1, phys: 0 };
    fn ok(&self) -> bool { self.handle >= 0 }
}

/// AX200 transport state. Mirrors the bits of `struct iwl_trans_pcie` we use.
struct Ax200 {
    mmio: i32,
    ltr_enabled: bool,
    hw_rev: u32,
    // RX queue DMA (iwl_pcie_alloc_rxq_dma) — addresses go into ctxt_info.
    rxq_bd: Dma,       // RBD ring (__le64 * NUM_RBDS)
    rxq_used_bd: Dma,  // used-BD ring (__le32 * NUM_RBDS)
    rxq_rb_stts: Dma,  // struct iwl_rb_status
    // TX command queue DMA (iwl_pcie_txq_alloc, gen2).
    cmd_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_CMD_QUEUE_SIZE)
    cmd_first_tb: Dma, // first-TB staging buffers
}

impl Ax200 {
    // ── CSR direct register access (BAR0) ────────────────────────
    fn r32(&self, reg: u32) -> u32 { host::mmio_r32(self.mmio, reg) }
    fn w32(&self, reg: u32, val: u32) { host::mmio_w32(self.mmio, reg, val); }
    /// iwl_set_bit: RMW preserving existing bits.
    fn set_bit(&self, reg: u32, bits: u32) { host::mmio_set32(self.mmio, reg, bits); }

    // ── PRPH access through HBUS (iwl_trans_pcie_read/write_prph) ─
    // umac_prph_offset is 0 for AX200, so umac == regular prph.
    fn prph_read(&self, reg: u32) -> u32 {
        self.w32(HBUS_TARG_PRPH_RADDR, (reg & PRPH_MASK) | (3 << 24));
        self.r32(HBUS_TARG_PRPH_RDAT)
    }
    fn prph_write(&self, reg: u32, val: u32) {
        self.w32(HBUS_TARG_PRPH_WADDR, (reg & PRPH_MASK) | (3 << 24));
        self.w32(HBUS_TARG_PRPH_WDAT, val);
    }

    /// iwl_poll_bit: spin until (reg & mask) == mask or timeout (microseconds).
    /// IWL_POLL_INTERVAL is 10us in Linux; we lack a us timer, so we pace with
    /// a short busy-spin (small timeouts) or a yielding 1ms sleep (large ones).
    fn poll_bit(&self, reg: u32, mask: u32, timeout_us: u32) -> bool {
        let mut waited = 0u32;
        loop {
            if self.r32(reg) & mask == mask { return true; }
            if waited >= timeout_us { return false; }
            if timeout_us >= 1000 {
                host::sleep_ms(1);
                waited += 1000;
            } else {
                for _ in 0..64 { core::hint::spin_loop(); }
                waited += 10;
            }
        }
    }

    // ── iwl_pcie_set_hw_ready / prepare_card_hw (trans.c) ────────
    fn set_hw_ready(&self) -> bool {
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_PCI_OWN_SET);
        let ready = self.poll_bit(
            CSR_HW_IF_CONFIG_REG,
            CSR_HW_IF_CONFIG_REG_PCI_OWN_SET,
            HW_READY_TIMEOUT_US,
        );
        if ready {
            self.set_bit(CSR_MBOX_SET_REG, CSR_MBOX_SET_REG_OS_ALIVE);
        }
        ready
    }

    /// Returns true once the card is ready (owned by us, not AMT/ME).
    fn prepare_card_hw(&self) -> bool {
        if self.set_hw_ready() {
            return true;
        }
        self.set_bit(CSR_DBG_LINK_PWR_MGMT_REG, CSR_RESET_LINK_PWR_MGMT_DISABLED);
        host::sleep_ms(2); // usleep_range(1000, 2000)

        for _ in 0..10 {
            // Prepare conditions to check again: wake the management bus.
            self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_WAKE_ME);
            // Inner do-while: up to 150ms (Linux uses 200us steps; we use 1ms).
            let mut t = 0u32;
            loop {
                if self.set_hw_ready() {
                    return true;
                }
                // No iwl_mei (CSME) in our world — that branch is skipped.
                host::sleep_ms(1);
                t += 1000;
                if t >= 150_000 {
                    break;
                }
            }
            host::sleep_ms(25);
        }
        false
    }

    // ── iwl_trans_pcie_sw_reset (trans.c) ───────────────────────
    // AX200 family 22000 < BZ → CSR_RESET path, usleep_range(5000, 6000).
    fn sw_reset(&self, retake_ownership: bool) -> bool {
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        host::sleep_ms(6);
        if retake_ownership {
            self.prepare_card_hw()
        } else {
            true
        }
    }

    // ── iwl_trans_pcie_clear_persistence_bit (trans.c) ──────────
    // Family 22000 → wprot = PREG_PRPH_WPROT_22000. Returns false only on
    // the unrecoverable -EPERM path.
    fn clear_persistence_bit(&self) -> bool {
        let hpm = self.prph_read(HPM_DEBUG);
        if !is_hw_error_value(hpm) && (hpm & PERSISTENCE_BIT) != 0 {
            let wprot_val = self.prph_read(PREG_PRPH_WPROT_22000);
            if wprot_val & PREG_WFPM_ACCESS != 0 {
                host::print("[ax200] error: can not clear persistence bit\n");
                return false;
            }
            self.prph_write(HPM_DEBUG, hpm & !PERSISTENCE_BIT);
        }
        true
    }

    // ── iwl_pcie_apm_config (trans.c) ───────────────────────────
    // L0s is unstable on these devices: always set L0S_DISABLED. Then cache
    // ASPM / LTR capability for later (set_pwr / set_ltr in Stage 2).
    fn apm_config(&mut self) {
        self.set_bit(CSR_GIO_REG, CSR_GIO_REG_VAL_L0S_DISABLED);

        let cap = pcie_find_cap(PCI_CAP_ID_EXP);
        if cap != 0 {
            let _lctl = pci_read16(cap + PCI_EXP_LNKCTL);
            // pm_support = !(lctl & ASPM_L0S) — not needed until power mgmt.
            let cap2 = pci_read16(cap + PCI_EXP_DEVCTL2);
            self.ltr_enabled = cap2 & PCI_EXP_DEVCTL2_LTR_EN != 0;
        }
        host::print("[ax200] apm_config: LTR ");
        host::print(if self.ltr_enabled { "enabled\n" } else { "disabled\n" });
    }

    // ── iwl_pcie_gen1_2_activate_nic (trans.c) ──────────────────
    // bisr_workaround = 1 for AX200: mdelay(2) before, udelay(200) after.
    // Family < BZ → INIT_DONE, poll MAC_CLOCK_READY.
    fn activate_nic(&self) -> bool {
        host::sleep_ms(2); // bisr_workaround: TOP FSM settle

        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_INIT_DONE);

        let ok = self.poll_bit(
            CSR_GP_CNTRL,
            CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY,
            MAC_CLOCK_TIMEOUT_US,
        );
        if !ok {
            host::print("[ax200] failed to wake NIC (MAC clock not ready)\n");
        }

        for _ in 0..1024 { core::hint::spin_loop(); } // bisr_workaround: udelay(200)
        ok
    }

    // ── iwl_pcie_apm_init (trans.c, gen1 — the start_hw path) ────
    // Family 22000: DIS_L0S_EXIT_TIMER (family < 8000) skipped; no pll_cfg;
    // no host_interrupt_operation_mode.
    fn apm_init(&mut self) -> bool {
        // Disable L0s without affecting L1 (ICH bug W/A).
        self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);
        // FH wait threshold to maximum (HW error during stress W/A).
        self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);
        // HAP INTA: wake the PCIe link L1a -> L0s.
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_HAP_WAKE);

        self.apm_config();
        // pll_cfg: AX200 base has none → no CSR_ANA_PLL_CFG.
        self.activate_nic()
    }

    // ── _iwl_trans_pcie_start_hw (trans.c) ──────────────────────
    fn start_hw(&mut self) -> bool {
        if !self.prepare_card_hw() {
            host::print("[ax200] error while preparing HW (AMT owns the device?)\n");
            return false;
        }
        host::print("[ax200] card prepared (ownership taken)\n");

        if !self.clear_persistence_bit() {
            return false;
        }

        if !self.sw_reset(true) {
            host::print("[ax200] sw_reset: card not ready after reset\n");
            return false;
        }
        host::print("[ax200] sw_reset done\n");

        // force_power_gating: family==22000 && integrated. AX200 is a discrete
        // M.2 card (not integrated) → skipped.

        if !self.apm_init() {
            return false;
        }
        host::print("[ax200] apm_init done — MAC clock ready\n");
        true
    }

    // ── iwl_pcie_gen2_apm_init (trans-gen2.c) ───────────────────
    // Same register effect as apm_init for family 22000 (gen1's DIS_L0S /
    // pll branches are already conditioned out). nic_init re-runs it.
    fn gen2_apm_init(&mut self) -> bool {
        self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);
        self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_HAP_WAKE);
        self.apm_config();
        self.activate_nic()
        // STATUS_DEVICE_ENABLED is host-side bookkeeping, not a register.
    }

    /// Allocate a coherent DMA buffer of at least `bytes`. npk_dma_alloc
    /// zeroes the pages and guarantees contiguous + below 4 GB, matching
    /// Linux' dma_alloc_coherent + the "no 4 GB boundary cross" requirement.
    fn alloc_dma(&self, bytes: usize, name: &str) -> Dma {
        let pages = ((bytes + 4095) / 4096) as u16;
        let handle = host::dma_alloc(pages);
        if handle < 0 {
            host::print("[ax200] DMA alloc failed: ");
            host::print(name);
            host::print("\n");
            return Dma::NONE;
        }
        Dma { handle, phys: host::dma_phys(handle) }
    }

    // ── iwl_pcie_gen2_rx_init (rx.c) ────────────────────────────
    // gen2 does NOT configure the RFH (firmware does it at alive) and the RB
    // page pool is filled at restock (alive). So here: set the int-coalescing
    // timer and allocate the ctxt_info-referenced RX rings. num_rxqs = 1.
    fn gen2_rx_init(&mut self) -> bool {
        host::mmio_w8(self.mmio, CSR_INT_COALESCING, IWL_HOST_INT_TIMEOUT_DEF);

        self.rxq_bd = self.alloc_dma(FREE_BD_SIZE * NUM_RBDS, "rxq.bd");
        self.rxq_used_bd = self.alloc_dma(USED_BD_SIZE * NUM_RBDS, "rxq.used_bd");
        self.rxq_rb_stts = self.alloc_dma(RB_STTS_SIZE, "rxq.rb_stts");

        self.rxq_bd.ok() && self.rxq_used_bd.ok() && self.rxq_rb_stts.ok()
    }

    // ── iwl_txq_gen2_init (tx-gen2.c) — command queue ───────────
    // iwl_pcie_txq_alloc allocates the TFD ring + first-TB staging buffers.
    // dma_alloc zeroing leaves every TFD with num_tbs=0 (set-invalid-gen2).
    // The byte-count table is not in the gen2 cmd-queue path and is not
    // referenced by ctxt_info, so it is not allocated here.
    fn txq_gen2_init(&mut self) -> bool {
        let slots = IWL_CMD_QUEUE_SIZE; // max(IWL_CMD_QUEUE_SIZE, min_txq_size=0)
        self.cmd_tfd = self.alloc_dma(TFH_TFD_SIZE * slots, "cmd.tfd");
        self.cmd_first_tb = self.alloc_dma(IWL_FIRST_TB_SIZE_ALIGN * slots, "cmd.first_tb");
        self.cmd_tfd.ok() && self.cmd_first_tb.ok()
    }

    // ── iwl_pcie_gen2_nic_init (trans-gen2.c) ───────────────────
    fn nic_init(&mut self) -> bool {
        if !self.gen2_apm_init() {
            host::print("[ax200] nic_init: gen2_apm_init failed\n");
            return false;
        }
        // iwl_op_mode_nic_config (mvm): DEFERRED. It is the op-mode/NVM layer
        // (radio-stepping CSR bits), not the PCIe transport, and is not needed
        // for the firmware CPU to reach ALIVE. Lands with the mvm port.

        if !self.gen2_rx_init() {
            return false;
        }
        if !self.txq_gen2_init() {
            return false;
        }

        // enable shadow regs in HW
        self.set_bit(CSR_MAC_SHADOW_REG_CTRL, CSR_MAC_SHADOW_REG_CTRL_VAL);
        true
    }

    // ── start_fw prologue (trans-gen2.c, the bits before nic_init) ──
    // disable interrupts, check RF-kill, clear the RF-kill handshake so the
    // firmware doesn't think the radio is killed, then clear pending ints.
    fn prepare_for_fw_load(&self) {
        // iwl_disable_interrupts
        self.w32(CSR_INT_MASK, 0);
        self.w32(CSR_INT, 0xFFFF_FFFF);
        self.w32(CSR_FH_INT_STATUS, 0xFFFF_FFFF);

        // iwl_pcie_check_hw_rf_kill: bit clear == radio killed.
        let gp = self.r32(CSR_GP_CNTRL);
        if gp & CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW == 0 {
            host::print("[ax200] WARNING: HW RF-kill asserted — firmware may not boot\n");
        }

        // make sure rfkill handshake bits are cleared
        self.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_SW_BIT_RFKILL);
        self.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_DRV_GP1_BIT_CMD_BLOCKED);
        self.w32(CSR_INT, 0xFFFF_FFFF);
    }

    // ── iwl_pcie_set_ltr (trans-gen2.c, 22000 non-integrated) ───
    fn set_ltr(&self) {
        let ltr = CSR_LTR_LONG_VAL_AD_NO_SNOOP_REQ
            | encode_bits(CSR_LTR_LONG_VAL_AD_SCALE_USEC, CSR_LTR_LONG_VAL_AD_NO_SNOOP_SCALE)
            | encode_bits(250, CSR_LTR_LONG_VAL_AD_NO_SNOOP_VAL)
            | CSR_LTR_LONG_VAL_AD_SNOOP_REQ
            | encode_bits(CSR_LTR_LONG_VAL_AD_SCALE_USEC, CSR_LTR_LONG_VAL_AD_SNOOP_SCALE)
            | encode_bits(250, CSR_LTR_LONG_VAL_AD_SNOOP_VAL);
        self.w32(CSR_LTR_LONG_VAL_AD, ltr);
    }

    // ── iwl_pcie_init_fw_sec (ctxt-info.c) ──────────────────────
    // Walk the SEC_RT TLVs in order; the CPU1_CPU2 / PAGING separators split
    // them into lmac / umac / paging. DMA each section's data and record its
    // physical address into the matching ctxt_dram image array. The firmware
    // reads these chunks itself; init_fw_sec ignores the per-section offset.
    fn init_fw_sec(
        &self,
        lmac: &mut [u64; IWL_MAX_DRAM_ENTRY],
        umac: &mut [u64; IWL_MAX_DRAM_ENTRY],
        virt: &mut [u64; IWL_MAX_DRAM_ENTRY],
    ) -> (usize, usize, usize) {
        let (mut lc, mut uc, mut vc) = (0usize, 0usize, 0usize);
        let mut region = 0u8; // 0 = lmac, 1 = umac, 2 = paging
        let mut off = FW_TLV_HEADER_LEN;

        while off + 8 <= FW.len() {
            let t = le32(FW, off);
            let l = le32(FW, off + 4) as usize;
            let body = off + 8;
            if body + l > FW.len() {
                break;
            }
            if t == IWL_UCODE_TLV_SEC_RT {
                let sec_off = le32(FW, body);
                if sec_off == CPU1_CPU2_SEPARATOR_SECTION {
                    region = 1;
                } else if sec_off == PAGING_SEPARATOR_SECTION {
                    region = 2;
                } else {
                    let data = &FW[body + 4..body + l];
                    let dma = self.alloc_dma(data.len(), "fw.sec");
                    if !dma.ok() {
                        return (lc, uc, vc); // caller checks counts
                    }
                    host::dma_write_buf(dma.handle, 0, data);
                    match region {
                        0 if lc < IWL_MAX_DRAM_ENTRY => { lmac[lc] = dma.phys; lc += 1; }
                        1 if uc < IWL_MAX_DRAM_ENTRY => { umac[uc] = dma.phys; uc += 1; }
                        2 if vc < IWL_MAX_DRAM_ENTRY => { virt[vc] = dma.phys; vc += 1; }
                        _ => {}
                    }
                }
            }
            off = body + ((l + 3) & !0x3);
        }
        (lc, uc, vc)
    }

    // ── iwl_pcie_ctxt_info_init + the start_fw tail → ALIVE ─────
    fn load_firmware(&mut self) -> bool {
        self.prepare_for_fw_load();

        // init_fw_sec: DMA the firmware sections.
        let mut lmac = [0u64; IWL_MAX_DRAM_ENTRY];
        let mut umac = [0u64; IWL_MAX_DRAM_ENTRY];
        let mut virt = [0u64; IWL_MAX_DRAM_ENTRY];
        let (lc, uc, vc) = self.init_fw_sec(&mut lmac, &mut umac, &mut virt);
        if lc == 0 || uc == 0 {
            host::print("[ax200] FW section load failed\n");
            return false;
        }
        host::print("[ax200] FW sections loaded: lmac=");
        host::print_hex32(lc as u32);
        host::print(" umac=");
        host::print_hex32(uc as u32);
        host::print(" paging=");
        host::print_hex32(vc as u32);
        host::print("\n");

        // Build the context-info structure (zeroed buffer + filled fields).
        let mut ci = [0u8; CTXT_INFO_SIZE];
        put_u16(&mut ci, CI_OFF_MAC_ID, self.hw_rev as u16);
        put_u16(&mut ci, CI_OFF_VERSION, 0);
        put_u16(&mut ci, CI_OFF_SIZE, (CTXT_INFO_SIZE / 4) as u16);

        let cb_size = (NUM_RBDS as u32).trailing_zeros(); // RX_QUEUE_CB_SIZE = ilog2
        let control_flags = IWL_CTXT_INFO_TFD_FORMAT_LONG
            | (cb_size << IWL_CTXT_INFO_RB_CB_SIZE_SHIFT)
            | (IWL_CTXT_INFO_RB_SIZE_4K << IWL_CTXT_INFO_RB_SIZE_SHIFT);
        put_u32(&mut ci, CI_OFF_CONTROL_FLAGS, control_flags);

        put_u64(&mut ci, CI_OFF_FREE_RBD, self.rxq_bd.phys);
        put_u64(&mut ci, CI_OFF_USED_RBD, self.rxq_used_bd.phys);
        put_u64(&mut ci, CI_OFF_STATUS_WR, self.rxq_rb_stts.phys);
        put_u64(&mut ci, CI_OFF_CMD_QUEUE_ADDR, self.cmd_tfd.phys);
        ci[CI_OFF_CMD_QUEUE_SIZE] = CMD_QUEUE_CB_SIZE;

        for i in 0..lc { put_u64(&mut ci, CI_OFF_LMAC_IMG + i * 8, lmac[i]); }
        for i in 0..uc { put_u64(&mut ci, CI_OFF_UMAC_IMG + i * 8, umac[i]); }
        for i in 0..vc { put_u64(&mut ci, CI_OFF_VIRTUAL_IMG + i * 8, virt[i]); }

        let ci_dma = self.alloc_dma(CTXT_INFO_SIZE, "ctxt_info");
        if !ci_dma.ok() {
            return false;
        }
        host::dma_write_buf(ci_dma.handle, 0, &ci);

        // iwl_enable_fw_load_int_ctx_info (non-MSI-X): the early ALIVE comes as
        // CSR_INT_BIT_ALIVE; FH_RX is for the later (Stage 3) ALIVE notification.
        self.w32(CSR_INT_MASK, CSR_INT_BIT_ALIVE | CSR_INT_BIT_FH_RX);

        // kick FW self-load: write the ctxt_info physical address.
        host::mmio_w64(self.mmio, CSR_CTXT_INFO_BA, ci_dma.phys);

        self.set_ltr();

        // tell the FW CPU to run (family < AX210 → regular PRPH).
        self.prph_write(UREG_CPU_INIT_RUN, 1);

        // Poll CSR_INT for the early ALIVE interrupt (no MSI-X, no notif_wait).
        host::print("[ax200] FW kicked, waiting for ALIVE...\n");
        if self.poll_bit(CSR_INT, CSR_INT_BIT_ALIVE, 2_000_000) {
            return true;
        }
        let intr = self.r32(CSR_INT);
        host::print("[ax200] ALIVE timeout — CSR_INT=0x");
        host::print_hex32(intr);
        host::print("\n");
        false
    }

    // ── iwl_pcie_rxmq_restock + read the ALIVE notification ─────
    // After the early ALIVE interrupt the firmware has configured the RFH, so
    // we may restock: allocate the RB pool, write each buffer into the free-RBD
    // ring (bd[i] = page_dma | vid, gen2 < AX210), then bump the HW write
    // pointer. The firmware then DMAs UCODE_ALIVE_NTFY into the first RB and
    // advances rb_stts.closed_rb_num, which we poll.
    fn rx_restock_and_alive(&mut self) -> bool {
        let mut bd = [0u8; RX_NUM_RBS * 8];
        let mut rb0 = Dma::NONE;
        for i in 0..RX_NUM_RBS {
            let rb = self.alloc_dma(RB_SIZE_BYTES, "rb");
            if !rb.ok() {
                return false;
            }
            if i == 0 {
                rb0 = rb;
            }
            // vid = i + 1; page is 4K-aligned so the low bits hold the vid.
            let entry = rb.phys | (i as u64 + 1);
            bd[i * 8..i * 8 + 8].copy_from_slice(&entry.to_le_bytes());
        }
        host::dma_write_buf(self.rxq_bd.handle, 0, &bd);
        host::fence();

        // iwl_pcie_rxq_inc_wr_ptr: write_actual = round_down(write, 8).
        let write_actual = (RX_NUM_RBS as u32) & !0x7;
        self.w32(RFH_Q0_FRBDCB_WIDX_TRG, write_actual);

        host::print("[ax200] RX restocked, waiting for alive notification...\n");
        let mut closed = 0u32;
        for _ in 0..1000 {
            host::fence();
            closed = host::dma_r32(self.rxq_rb_stts.handle, 0) & RB_STTS_CLOSED_MASK;
            if closed != 0 {
                break;
            }
            host::sleep_ms(1);
        }
        if closed == 0 {
            host::print("[ax200] no RX — alive notification timeout\n");
            return false;
        }
        host::print("[ax200] RX active — closed_rb_num=0x");
        host::print_hex32(closed);
        host::print("\n");

        // Dump the first RB header (iwl_rx_packet: len_n_flags, cmd, group_id).
        let mut hdr = [0u8; 8];
        host::dma_read_buf(rb0.handle, 0, &mut hdr);
        let len_n_flags = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        host::print("[ax200] RB[0] len_n_flags=0x");
        host::print_hex32(len_n_flags);
        host::print(" cmd=0x");
        host::print_hex32(hdr[4] as u32);
        host::print(" group=0x");
        host::print_hex32(hdr[5] as u32);
        host::print("\n");
        if hdr[4] == UCODE_ALIVE_NTFY && hdr[5] == 0 {
            host::print("[ax200] → UCODE_ALIVE_NTFY confirmed\n");
        }
        true
    }

    // Halt the firmware's DMA engines before the driver returns. The kernel
    // frees our DMA buffers on return; a still-running chip must not DMA into
    // them afterwards. sw_reset (CSR_RESET) stops the device.
    fn stop(&self) {
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        host::sleep_ms(6);
    }
}

/// Log a DMA allocation's physical address (Stage 1 diagnostics).
fn log_dma(name: &str, d: &Dma) {
    host::print(name);
    host::print(": phys 0x");
    host::print_hex64(d.phys);
    host::print("\n");
}

/// iwl_trans_is_hw_error_value (iwl-trans.h).
fn is_hw_error_value(val: u32) -> bool {
    (val & !0xf) == 0xa5a5_a5a0 || (val & !0xf) == 0x5a5a_5a50
}

/// Read a 16-bit value from PCI config space (dword read + extract).
fn pci_read16(off: u8) -> u16 {
    let dword = host::pci_read_config(off & !0x3);
    ((dword >> ((off & 0x3) * 8)) & 0xFFFF) as u16
}

/// Walk the PCI capability list for the given capability ID. Returns the
/// config-space offset of the capability, or 0 if absent.
fn pcie_find_cap(id: u8) -> u8 {
    let mut ptr = (host::pci_read_config(PCI_CAP_PTR as u8) & 0xFC) as u8;
    let mut guard = 0;
    while ptr != 0 && guard < 48 {
        let hdr = host::pci_read_config(ptr);
        if (hdr & 0xFF) as u8 == id {
            return ptr;
        }
        ptr = ((hdr >> 8) & 0xFC) as u8;
        guard += 1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[ax200] Intel Wi-Fi 6 AX200 driver v0.5.0 — Stage 3 (RX + alive ntfy)\n");

    // ── Stage 0a: bind, bus master, map BAR0, identity ───────────
    let rc = host::pci_bind(AX200_VENDOR, AX200_DEVICE);
    if rc != 0 {
        host::print("[ax200] PCI bind failed (");
        match rc {
            -1 => host::print("not found"),
            -2 => host::print("denied"),
            _ => host::print("unknown error"),
        }
        host::print(")\n");
        return;
    }
    host::print("[ax200] PCI bind OK\n");

    host::pci_enable_bus_master();

    let mmio = host::mmio_map_bar(BAR_CSR, 16);
    if mmio < 0 {
        host::print("[ax200] BAR0 map failed\n");
        return;
    }
    host::print("[ax200] BAR0 mapped\n");

    let mut dev = Ax200 {
        mmio,
        ltr_enabled: false,
        hw_rev: 0,
        rxq_bd: Dma::NONE,
        rxq_used_bd: Dma::NONE,
        rxq_rb_stts: Dma::NONE,
        cmd_tfd: Dma::NONE,
        cmd_first_tb: Dma::NONE,
    };

    let hw_rev = dev.r32(CSR_HW_REV);
    dev.hw_rev = hw_rev;
    let rf_id = dev.r32(CSR_HW_RF_ID);
    host::log_reg("CSR_HW_REV", hw_rev);
    host::log_reg("CSR_HW_RF_ID", rf_id);

    if hw_rev == 0xFFFF_FFFF || hw_rev == 0 {
        host::print("[ax200] HW_REV looks wrong — chip not responding?\n");
        return;
    }
    // RF type lives in the high nibble pattern; HR is the AX200's radio.
    if rf_id & 0x0FFF_F000 == CSR_HW_RF_ID_TYPE_HR & 0x0FFF_F000 {
        host::print("[ax200] RF type = HR (matches AX200) — Stage 0a OK\n");
    } else {
        host::print("[ax200] RF type unexpected (continuing anyway)\n");
    }

    // ── Stage 0b: reset + APM bring-up ───────────────────────────
    if !dev.start_hw() {
        host::print("[ax200] Stage 0b FAILED\n");
        return;
    }
    host::print("[ax200] Stage 0b OK — chip powered up, PRPH accessible\n");

    // ── Stage 1: RX/TX rings + command queue ─────────────────────
    if !dev.nic_init() {
        host::print("[ax200] Stage 1 FAILED\n");
        return;
    }
    host::print("[ax200] Stage 1 OK — rings allocated:\n");
    log_dma("  rxq.bd      ", &dev.rxq_bd);
    log_dma("  rxq.used_bd ", &dev.rxq_used_bd);
    log_dma("  rxq.rb_stts ", &dev.rxq_rb_stts);
    log_dma("  cmd.tfd     ", &dev.cmd_tfd);
    log_dma("  cmd.first_tb", &dev.cmd_first_tb);

    // ── Stage 2: context-info + FW self-load + ALIVE ─────────────
    if dev.load_firmware() {
        host::print("[ax200] Stage 2 OK — *** FIRMWARE ALIVE *** 🎉\n");

        // ── Stage 3: RX restock + read the ALIVE notification ────
        if dev.rx_restock_and_alive() {
            host::print("[ax200] Stage 3 OK — RX path live, alive notification received\n");
        } else {
            host::print("[ax200] Stage 3 FAILED\n");
        }
    } else {
        host::print("[ax200] Stage 2 FAILED (no ALIVE)\n");
    }

    // Halt the chip before returning: the kernel frees our DMA buffers on
    // return and a still-running firmware must not DMA into them afterwards.
    // (Probe-stage driver — we return rather than idle; npk_input_wait HLTs
    // without yielding, which would pin the core. A persistent yielding
    // run-loop arrives with Stage 4+.)
    dev.stop();
}
