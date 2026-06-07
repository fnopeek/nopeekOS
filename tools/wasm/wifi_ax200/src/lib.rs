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

/// Write transfer block `i` (iwl_tfh_tb { __le16 tb_len; __le64 addr }) into a
/// TFD. tbs[] start at offset 2 (after num_tbs); addr is stored unaligned.
fn put_tfh_tb(tfd: &mut [u8], i: usize, len: u16, addr: u64) {
    let o = 2 + i * TFH_TB_LEN;
    tfd[o..o + 2].copy_from_slice(&len.to_le_bytes());
    tfd[o + 2..o + 10].copy_from_slice(&addr.to_le_bytes());
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

/// Classification of a received 802.11 data frame addressed to us.
enum RxKind {
    None,
    /// EAPOL-Key frame (the 4-way) → forward to wifid. `out` holds the frame.
    Eapol(usize),
    /// IP/other data → the kernel IP stack. `out` holds an Ethernet frame.
    Ip(usize),
}

/// A discovered access point (from a scan beacon / probe response).
#[derive(Clone, Copy)]
struct Ap {
    bssid: [u8; 6],
    ssid: [u8; SSID_MAX],
    ssid_len: u8,
    rssi: i8, // dBm
    channel: u8,
    beacon_int: u16, // beacon interval (TU) — for the connect MAC context
    privacy: bool,   // capability Privacy bit (encrypted → needs RSN in assoc-req)
}
impl Ap {
    const EMPTY: Ap = Ap {
        bssid: [0; 6],
        ssid: [0; SSID_MAX],
        ssid_len: 0,
        rssi: 0,
        channel: 0,
        beacon_int: 0,
        privacy: false,
    };
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
    cmd_data: Dma,     // payload buffer for large (NOCOPY) commands → TB1
    cmd_write_ptr: u32, // txq->write_ptr for the command queue
    // RX RB pool (vid v → rb_pool[v-1]) + our read index into the used-BD ring
    // + the free-BD ring write index (for recycling RBs during the scan).
    rb_pool: [Dma; RX_NUM_RBS],
    rxq_read: u32,
    free_bd_write: u32,
    // Firmware error-table SRAM pointers (from the ALIVE notification), for
    // dumping the FW error log when a command/scan produces no response.
    lmac_err_ptr: u32,
    umac_err_ptr: u32,
    // Scan channel list parsed from the NVM_GET_INFO regulatory section: the
    // NVM_CHANNEL_VALID channels with their PHY band, for both 2.4 and 5 GHz.
    scan_chans: [u8; SCAN_MAX_CHANS], // channel numbers
    scan_bands: [u8; SCAN_MAX_CHANS], // PHY_BAND_24 / PHY_BAND_5 per channel
    n_scan_chans: usize,
    // The card's MAC address (from the CSR strap/OTP), for netdev registration.
    mac: [u8; 6],
    // Connect target picked from the scan (strongest AP) — for #3 connect (5a+).
    target_bssid: [u8; 6],
    target_chan: u8,
    target_band: u8, // PHY_BAND_24 / PHY_BAND_5
    target_beacon_int: u16, // beacon interval of the target AP (for MAC context)
    target_ssid: [u8; SSID_MAX],
    target_ssid_len: u8,
    target_privacy: bool, // target is encrypted (WPA2) → assoc-req carries an RSN IE
    target_valid: bool,
    // gen2 management TX queue for the AP station (auth/assoc frames, 5b+).
    mgmt_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_MGMT_QUEUE_SIZE)
    mgmt_first_tb: Dma, // first-TB staging buffers
    mgmt_bc_tbl: Dma,   // byte-count table (FW DMA scheduling)
    mgmt_queue_id: u16, // queue id returned by the firmware
    mgmt_write_ptr: u32,
    // gen2 data TX queue (tid 0) for EAPOL frames during the 4-way (5e+).
    data_tfd: Dma,
    data_first_tb: Dma,
    data_bc_tbl: Dma,
    data_queue_id: u16,
    data_write_ptr: u32,
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
        // Payload buffer for large host commands: their bulk is mapped as a
        // second TB (NOCOPY) instead of being copied into the cmd buffer. One
        // page covers the largest command we build (SCAN_REQ_UMAC ~1.7 KB).
        self.cmd_data = self.alloc_dma(CMD_DATA_BYTES, "cmd.data");
        self.cmd_tfd.ok() && self.cmd_first_tb.ok() && self.cmd_data.ok()
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
    fn rx_restock_and_alive(&mut self) -> Option<Dma> {
        let mut bd = [0u8; RX_NUM_RBS * 8];
        for i in 0..RX_NUM_RBS {
            let rb = self.alloc_dma(RB_SIZE_BYTES, "rb");
            if !rb.ok() {
                return None;
            }
            self.rb_pool[i] = rb;
            // vid = i + 1; page is 4K-aligned so the low bits hold the vid.
            let entry = rb.phys | (i as u64 + 1);
            bd[i * 8..i * 8 + 8].copy_from_slice(&entry.to_le_bytes());
        }
        host::dma_write_buf(self.rxq_bd.handle, 0, &bd);
        host::fence();

        // iwl_pcie_rxq_inc_wr_ptr: write_actual = round_down(write, 8).
        self.free_bd_write = RX_NUM_RBS as u32; // 64 RBs posted at slots 0..63
        let write_actual = self.free_bd_write & !0x7;
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
            return None;
        }
        host::print("[ax200] RX active — closed_rb_num=0x");
        host::print_hex32(closed);
        host::print("\n");

        // The FW reports each filled RB in the used-BD ring (vid). Read used_bd[0]
        // to find which RB holds the first frame (iwl_pcie_get_rxb, < AX210 path).
        let vid = host::dma_r32(self.rxq_used_bd.handle, 0) & RX_VID_MASK;
        if vid == 0 || vid as usize > RX_NUM_RBS {
            host::print("[ax200] bad RX vid\n");
            return None;
        }
        let rb0 = self.rb_pool[vid as usize - 1];
        self.rxq_read = 1; // consumed used_bd[0]

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
        Some(rb0)
    }

    // ── iwl_alive_fn (mvm/fw.c, version >= 6 path) ──────────────
    // Parse the `struct iwl_alive_ntf_v6` the firmware DMA'd into RB[0]'s
    // payload (data[] begins at RX_PKT_DATA_OFF). Confirms the firmware
    // reported OK status and surfaces the LMAC/UMAC version + error-table
    // pointers + sku_id. The sku_id gates the next stage: an all-zero
    // sku_id means PNVM load is skipped entirely (iwl_pnvm_load). Linux's
    // IMR / debug active-region bookkeeping is debug-only and is deferred.
    fn parse_alive_ntf(&mut self, rb0: &Dma) -> bool {
        let mut p = [0u8; 160]; // len_n_flags(4) + hdr(4) + v6(144) = 152
        host::dma_read_buf(rb0.handle, 0, &mut p);
        let s = RX_PKT_DATA_OFF; // alive struct base
        let rd16 = |o: usize| u16::from_le_bytes([p[s + o], p[s + o + 1]]);
        let rd32 = |o: usize| {
            u32::from_le_bytes([p[s + o], p[s + o + 1], p[s + o + 2], p[s + o + 3]])
        };

        let status = rd16(AL_OFF_STATUS);
        let l = AL_OFF_LMAC0; // lmac_data[0]
        let u = AL_OFF_UMAC;
        let sku = AL_OFF_SKU_ID;

        host::print("[ax200] ALIVE status=0x");
        host::print_hex16(status);
        host::print(if status == IWL_ALIVE_STATUS_OK {
            " (OK)\n"
        } else if status == IWL_ALIVE_STATUS_ERR {
            " (ERR!)\n"
        } else {
            " (unknown)\n"
        });

        host::print("[ax200]   LMAC ucode ");
        host::print_hex32(rd32(l + LMAC_OFF_UCODE_MAJOR));
        host::print(".");
        host::print_hex32(rd32(l + LMAC_OFF_UCODE_MINOR));
        host::print(" ver_type=0x");
        host::print_hex32(p[s + l + LMAC_OFF_VER_TYPE] as u32);
        host::print(" subtype=0x");
        host::print_hex32(p[s + l + LMAC_OFF_VER_SUBTYPE] as u32);
        host::print("\n");

        host::print("[ax200]   UMAC ver ");
        host::print_hex32(rd32(u + UMAC_OFF_MAJOR));
        host::print(".");
        host::print_hex32(rd32(u + UMAC_OFF_MINOR));
        host::print("\n");

        let umac_err = rd32(u + UMAC_OFF_ERR_INFO) & !FW_ADDR_CACHE_CONTROL;
        // Stash the error-table SRAM pointers for later error-log dumps.
        self.lmac_err_ptr = rd32(l + LMAC_OFF_ERR_TABLE);
        self.umac_err_ptr = umac_err;
        host::print("[ax200]   err tables: lmac=0x");
        host::print_hex32(rd32(l + LMAC_OFF_ERR_TABLE));
        host::print(" umac=0x");
        host::print_hex32(umac_err);
        host::print("\n");

        let sku0 = rd32(sku);
        let sku1 = rd32(sku + 4);
        let sku2 = rd32(sku + 8);
        host::print("[ax200]   sku_id: 0x");
        host::print_hex32(sku0);
        host::print(" 0x");
        host::print_hex32(sku1);
        host::print(" 0x");
        host::print_hex32(sku2);
        host::print(if sku0 == 0 && sku1 == 0 && sku2 == 0 {
            " (empty → PNVM skipped)\n"
        } else {
            " (PNVM required)\n"
        });

        status == IWL_ALIVE_STATUS_OK
    }

    // ── iwl_pcie_gen2_enqueue_hcmd (tx-gen2.c) ──────────────────
    // Enqueue a host command. The iwl_cmd_header_wide (8 B) + payload is laid
    // out across one or two TBs exactly as the Linux enqueue does:
    //   - The first IWL_FIRST_TB_SIZE (20) bytes of the command (header + the
    //     leading payload bytes) always go into the per-slot first-TB staging
    //     buffer as TB0 (it is the bidirectional-DMA buffer the HW writes back).
    //   - If the command is larger, the remaining payload is mapped as TB1
    //     pointing into the cmd_data buffer (this is the IWL_HCMD_DFL_NOCOPY
    //     path large commands like SCAN_REQ_UMAC use; the byte-count table is
    //     never touched by enqueue_hcmd). The command queue is DQA queue 0;
    //     seq = QUEUE_TO_SEQ(0) | INDEX_TO_SEQ(write_ptr).
    fn send_hcmd(&mut self, group: u8, opcode: u8, payload: &[u8]) {
        // iwl_trans_send_cmd (iwl-trans.c): with wide_cmd_header (always true on
        // gen2, where every command carries the long header), a legacy command
        // with group 0 is promoted to LONG_GROUP via DEF_ID(opcode) = (1<<8) |
        // opcode. The firmware registers these "legacy" commands (TX_ANT 0x98,
        // BT 0x9b, POWER 0x77, MCC 0xc8, MAC_CONTEXT 0x28, …) ONLY under group 1
        // — sending them as group 0 yields a BAD_COMMAND assert. (REPLY_ERROR is
        // the lone exception in Linux; we never send it.)
        let group = if group == 0 { IWL_ALWAYS_LONG_GROUP } else { group };
        let wp = self.cmd_write_ptr;
        let idx = (wp & (IWL_CMD_QUEUE_SIZE as u32 - 1)) as usize;
        let total = CMD_HDR_WIDE_LEN + payload.len();

        // first-TB staging: wide header (8 B) + up to FIRST_TB_HEAD_MAX (12)
        // payload bytes, capped at IWL_FIRST_TB_SIZE (20).
        let head = payload.len().min(FIRST_TB_HEAD_MAX);
        let tb0_len = CMD_HDR_WIDE_LEN + head;
        let mut ftb = [0u8; IWL_FIRST_TB_SIZE];
        ftb[HDRW_OFF_CMD] = opcode;
        ftb[HDRW_OFF_GROUP] = group;
        ftb[HDRW_OFF_SEQ..HDRW_OFF_SEQ + 2].copy_from_slice(&(wp as u16).to_le_bytes());
        ftb[HDRW_OFF_LEN..HDRW_OFF_LEN + 2]
            .copy_from_slice(&(payload.len() as u16).to_le_bytes());
        // reserved (6) + version (7) stay 0.
        ftb[CMD_HDR_WIDE_LEN..tb0_len].copy_from_slice(&payload[..head]);
        let ftb_off = (idx * IWL_FIRST_TB_SIZE_ALIGN) as u32;
        host::dma_write_buf(self.cmd_first_tb.handle, ftb_off, &ftb[..tb0_len]);
        let tb0_phys = self.cmd_first_tb.phys + (idx * IWL_FIRST_TB_SIZE_ALIGN) as u64;

        // Build the TFD: TB0 = staging buffer; TB1 (if any) = the remaining
        // payload mapped from cmd_data. iwl_tfh_tfd: num_tbs @0, then 10-byte
        // TBs {tb_len __le16, addr __le64}.
        let mut tfd = [0u8; TFH_TFD_SIZE];
        put_tfh_tb(&mut tfd, 0, tb0_len as u16, tb0_phys);
        let num_tbs = if total > IWL_FIRST_TB_SIZE {
            // remaining payload → cmd_data, mapped as TB1 (NOCOPY semantics).
            host::dma_write_buf(self.cmd_data.handle, 0, payload);
            let rest = payload.len() - head;
            put_tfh_tb(&mut tfd, 1, rest as u16, self.cmd_data.phys + head as u64);
            2u16
        } else {
            1u16
        };
        tfd[0..2].copy_from_slice(&num_tbs.to_le_bytes());
        let tfd_off = (idx * TFH_TFD_SIZE) as u32;
        host::dma_write_buf(self.cmd_tfd.handle, tfd_off, &tfd);
        host::fence();

        // iwl_txq_inc_wrap then iwl_txq_inc_wr_ptr: bump write_ptr (wrap at 256)
        // and ring the doorbell with the new write_ptr | (queue_id << 16).
        self.cmd_write_ptr = (wp + 1) & (MAX_TFD_QUEUE_SIZE - 1);
        self.w32(HBUS_TARG_WRPTR, self.cmd_write_ptr | (IWL_CMD_QUEUE_ID << 16));

        host::print("[ax200] hcmd sent: group=0x");
        host::print_hex32(group as u32);
        host::print(" cmd=0x");
        host::print_hex32(opcode as u32);
        host::print("\n");
    }

    // Drain newly-closed RBs from the used-BD ring, looking for a frame with the
    // given (cmd, group). Returns the matching RB on success. Mirrors the read-
    // pointer walk of iwl_pcie_rx_handle (mq path): r = closed_rb_num, walk
    // used_bd[read..r], vid → rb_pool[vid-1]. No RB recycling — 64 posted RBs
    // are plenty for the handful of init/NVM frames.
    fn drain_rx_until(&mut self, want_cmd: u8, want_group: u8) -> Option<Dma> {
        host::fence();
        let r = host::dma_r32(self.rxq_rb_stts.handle, 0) & RB_STTS_CLOSED_MASK;
        while self.rxq_read != r {
            let i = self.rxq_read as usize;
            let vid = host::dma_r32(self.rxq_used_bd.handle, (i * 4) as u32) & RX_VID_MASK;
            let mut matched = None;
            if vid >= 1 && vid as usize <= RX_NUM_RBS {
                let rb = self.rb_pool[vid as usize - 1];
                let mut hdr = [0u8; 8];
                host::dma_read_buf(rb.handle, 0, &mut hdr);
                let cmd = hdr[4];
                let grp = hdr[5];
                host::print("[ax200]   RX cmd=0x");
                host::print_hex32(cmd as u32);
                host::print(" group=0x");
                host::print_hex32(grp as u32);
                host::print("\n");
                if cmd == want_cmd && grp == want_group {
                    matched = Some(rb);
                } else {
                    // Recycle non-matched RBs (notifications, echoes — the bulk)
                    // back into the free-BD ring. The driver is now resident, so
                    // the 64-RB pool must be replenished or the firmware runs dry
                    // and can post no further frames (TX completions, beacons).
                    // The matched RB is returned to the caller to read, so it is
                    // NOT recycled here (that would race the firmware writing it).
                    self.recycle_rb(vid);
                    self.flush_free_bd();
                }
            }
            self.rxq_read = (self.rxq_read + 1) & (NUM_RBDS as u32 - 1);
            if matched.is_some() {
                return matched;
            }
        }
        None
    }

    // Poll the RX queue for up to `ms` milliseconds for a (cmd, group) frame.
    fn wait_rx(&mut self, want_cmd: u8, want_group: u8, ms: u32) -> Option<Dma> {
        for _ in 0..ms {
            if let Some(rb) = self.drain_rx_until(want_cmd, want_group) {
                return Some(rb);
            }
            host::sleep_ms(1);
        }
        None
    }

    // ── iwl_run_unified_mvm_ucode post-alive init flow (mvm/fw.c) ──
    // For unified ucode (AX200) the flow after ALIVE is: INIT_EXTENDED_CFG_CMD
    // (declares we will send NVM access) → NVM_ACCESS_COMPLETE → wait for
    // INIT_COMPLETE_NOTIF. PNVM load is skipped (sku_id empty), the external
    // NVM file path is skipped (internal NVM), and iwl_send_phy_cfg_cmd is a
    // no-op for unified ucode. So exactly two host commands, then the notif.
    fn run_init_handshake(&mut self) -> bool {
        // INIT_EXTENDED_CFG_CMD { __le32 init_flags = BIT(IWL_INIT_NVM) }
        self.send_hcmd(SYSTEM_GROUP, INIT_EXTENDED_CFG_CMD, &IWL_INIT_NVM_FLAG.to_le_bytes());
        // NVM_ACCESS_COMPLETE { __le32 reserved = 0 }
        self.send_hcmd(REGULATORY_AND_NVM_GROUP, NVM_ACCESS_COMPLETE, &0u32.to_le_bytes());

        host::print("[ax200] init cmds sent, waiting for INIT_COMPLETE_NOTIF...\n");
        // INIT_COMPLETE_NOTIF is a legacy-group (0) notification.
        if self.wait_rx(INIT_COMPLETE_NOTIF, 0, 2000).is_some() {
            return true;
        }
        host::print("[ax200] INIT_COMPLETE_NOTIF timeout\n");
        false
    }

    // ── iwl_get_nvm (iwl-nvm-parse.c) — read NVM info ──────────────
    // Send NVM_GET_INFO and parse the response: nvm version, reserved-MAC count,
    // MAC SKU caps (bands / 11n / 11ac / 11ax), PHY tx/rx antenna chains, LAR.
    // The MAC address is NOT in this response — it is read from the CSR strap/OTP
    // registers (iwl_set_hw_address_from_csr). The channel profile in the
    // response feeds the scan channel list (Stage 4d).
    fn read_nvm(&mut self) -> bool {
        self.send_hcmd(REGULATORY_AND_NVM_GROUP, NVM_GET_INFO, &0u32.to_le_bytes());
        host::print("[ax200] NVM_GET_INFO sent, waiting for response...\n");
        let rb = match self.wait_rx(NVM_GET_INFO, REGULATORY_AND_NVM_GROUP, 2000) {
            Some(rb) => rb,
            None => {
                host::print("[ax200] NVM_GET_INFO timeout\n");
                return false;
            }
        };

        // Cover the header fields plus the regulatory channel_profile
        // (__le32[51] at payload offset 28 → absolute 8+28 = 36, ending at 240).
        let mut p = [0u8; 256];
        host::dma_read_buf(rb.handle, 0, &mut p);
        let lnf = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        let payload_len = (lnf & FH_FRAME_SIZE_MASK).wrapping_sub(4); // frame - hdr(4)
        let b = RX_PKT_DATA_OFF;
        let rd16 = |o: usize| u16::from_le_bytes([p[b + o], p[b + o + 1]]);
        let rd32 = |o: usize| {
            u32::from_le_bytes([p[b + o], p[b + o + 1], p[b + o + 2], p[b + o + 3]])
        };

        let mac_sku = rd32(NVM_OFF_MAC_SKU);
        host::print("[ax200]   NVM rsp_len=");
        host::print_hex32(payload_len);
        host::print(" version=0x");
        host::print_hex16(rd16(NVM_OFF_VERSION));
        host::print(" n_hw_addrs=");
        host::print_hex32(p[b + NVM_OFF_N_HW_ADDRS] as u32);
        host::print("\n");

        host::print("[ax200]   bands:");
        if mac_sku & NVM_SKU_BAND_24 != 0 { host::print(" 2.4G"); }
        if mac_sku & NVM_SKU_BAND_52 != 0 { host::print(" 5G"); }
        if mac_sku & NVM_SKU_11N != 0 { host::print(" 11n"); }
        if mac_sku & NVM_SKU_11AC != 0 { host::print(" 11ac"); }
        if mac_sku & NVM_SKU_11AX != 0 { host::print(" 11ax"); }
        host::print(" | tx_chains=0x");
        host::print_hex32(rd32(NVM_OFF_TX_CHAINS));
        host::print(" rx_chains=0x");
        host::print_hex32(rd32(NVM_OFF_RX_CHAINS));
        host::print(" lar=0x");
        host::print_hex32(rd32(NVM_OFF_LAR));
        host::print("\n");

        self.read_mac_address();

        // ── Build the scan channel list from the regulatory section ──
        // iwl_init_channel_map (iwl-nvm-parse.c): walk iwl_ext_nvm_channels,
        // read the per-channel __le32 flags from channel_profile, keep the
        // NVM_CHANNEL_VALID channels. 5 GHz channels are gated on the 5.2 band
        // SKU bit. Each channel's PHY band rides in v2.band in the scan command.
        let band_52 = mac_sku & NVM_SKU_BAND_52 != 0;
        let prof = b + NVM_OFF_CHANNEL_PROFILE;
        self.n_scan_chans = 0;
        for idx in 0..NVM_EXT_NUM_CHANNELS {
            let flags = u32::from_le_bytes([
                p[prof + idx * 4],
                p[prof + idx * 4 + 1],
                p[prof + idx * 4 + 2],
                p[prof + idx * 4 + 3],
            ]);
            if flags & NVM_CHANNEL_VALID == 0 {
                continue;
            }
            let is_5ghz = idx >= NVM_NUM_2GHZ;
            if is_5ghz && !band_52 {
                continue;
            }
            let band = if is_5ghz { PHY_BAND_5 } else { PHY_BAND_24 };
            if self.n_scan_chans < SCAN_MAX_CHANS {
                self.scan_chans[self.n_scan_chans] = IWL_EXT_NVM_CHANNELS[idx];
                self.scan_bands[self.n_scan_chans] = band as u8;
                self.n_scan_chans += 1;
            }
        }
        host::print("[ax200]   scan channels: ");
        host::print_dec(self.n_scan_chans as u32);
        host::print(" valid (2.4 + 5 GHz)\n");
        true
    }

    // Drain (and log) whatever the firmware has closed in the RX ring over the
    // next `ms` milliseconds, without matching anything. Used after fire-and-
    // forget commands that produce no response of their own but may surface
    // unrelated notifications. An impossible (cmd, group) makes drain_rx_until
    // log+advance every closed RB and return None.
    fn pump_rx(&mut self, ms: u32) {
        for _ in 0..ms {
            self.drain_rx_until(0xFF, 0xFF);
            host::sleep_ms(1);
        }
    }

    // ── Scan prerequisites from iwl_mvm_up (mvm/fw.c) ──────────────
    // The hard pre-scan config commands. These are fire-and-forget config
    // commands — unlike the init-phase commands they do NOT echo a response, so
    // we just send them and pump the RX ring briefly for diagnostics. (The many
    // best-effort / BIOS-gated commands in iwl_mvm_up — SAR, PPAG, TAS, RFI, BT
    // coex tuning, power, RSS, SF — are deferred like op_mode_nic_config; they
    // aren't needed for a scan to return APs.) Real validation is the scan.
    // The complete mandatory iwl_mvm_up command sequence between ALIVE and the
    // scan, in order — no cherry-picking. Faithful omissions: configure_rxq and
    // rss_cfg are no-ops for a single RX queue (both `return 0` when num_rxqs==1,
    // and ours is 1); the BIOS/ACPI-gated commands (lari_cfg, ppag_init,
    // sar_init, sgom_init, tas_init) send nothing without platform tables, just
    // as Linux on a machine that lacks them (ppag: !approved→0, sgom: !enabled→0,
    // sar: post-config_scan anyway); the remaining post-config_scan tuning is not
    // a scan prerequisite. Best-effort, non-fatal calls (shared_mem_conf,
    // sf_update, tt_tx_backoff, config_ltr) are also skipped — Linux itself
    // continues when they fail. Everything that gates with `goto error` and
    // actually emits a command for our config is here.
    fn run_scan_prereqs(&mut self) {
        // TX_ANT_CONFIGURATION_CMD (legacy group 0): valid tx antennas.
        self.send_hcmd(0, TX_ANT_CONFIGURATION_CMD, &ANT_AB.to_le_bytes());
        self.pump_rx(50);

        self.send_bt_init();      // iwl_mvm_send_bt_init_conf
        self.send_soc_latency();  // iwl_set_soc_latency (SOC_LATENCY_SUPPORT cap)
        self.send_dqa();          // iwl_mvm_send_dqa_cmd (DQA_SUPPORT cap)
        self.send_power();        // iwl_mvm_power_update_device

        // iwl_mvm_init_mcc: set the regulatory domain. With LAR enabled the FW
        // blocks scans until this is done (iwl_mvm_up does it before config_scan).
        self.set_regulatory();

        // iwl_mvm_config_scan → SCAN_CFG_CMD v5 (LONG_GROUP): reduced config —
        // tx/rx antenna chains. bcast_sta_id stays 0 (v5 firmware ignores it).
        let mut cfg = [0u8; SCAN_CFG_LEN];
        cfg[SCAN_CFG_OFF_TX_CHAINS..SCAN_CFG_OFF_TX_CHAINS + 4]
            .copy_from_slice(&ANT_AB.to_le_bytes());
        cfg[SCAN_CFG_OFF_RX_CHAINS..SCAN_CFG_OFF_RX_CHAINS + 4]
            .copy_from_slice(&ANT_AB.to_le_bytes());
        self.send_hcmd(IWL_ALWAYS_LONG_GROUP, SCAN_CFG_CMD, &cfg);
        self.pump_rx(50);
    }

    // ── iwl_mvm_send_bt_init_conf (mvm/coex.c) ────────────────────
    // BT coex config (combo chip shares the antenna). mode = network coex;
    // enabled_modules = SYNC2SCO (IWL_MVM_BT_COEX_SYNC2SCO=1, always) | MPLUT
    // (only if the BT_MPLUT_SUPPORT capability is present) | HIGH_BAND_RET.
    fn send_bt_init(&mut self) {
        let mut modules = BT_COEX_SYNC2SCO_ENABLED | BT_COEX_HIGH_BAND_RET;
        if fw_has_capa(CAPA_BT_MPLUT_SUPPORT) {
            modules |= BT_COEX_MPLUT_ENABLED;
        }
        let mut cmd = [0u8; BT_COEX_CMD_LEN];
        put_u32(&mut cmd, 0, BT_COEX_NW);
        put_u32(&mut cmd, 4, modules);
        self.send_hcmd(0, BT_CONFIG, &cmd);
        self.pump_rx(20);
    }

    // ── iwl_set_soc_latency (fw/init.c) ───────────────────────────
    // SOC config. AX200 is a discrete card (mac_cfg.integrated unset) → flags =
    // DISCRETE, latency = xtal_latency (0). Sent only if the firmware advertises
    // SOC_LATENCY_SUPPORT (the gate in iwl_mvm_up).
    fn send_soc_latency(&mut self) {
        if !fw_has_capa(CAPA_SOC_LATENCY_SUPPORT) {
            return;
        }
        let mut cmd = [0u8; SOC_CONFIG_CMD_LEN];
        put_u32(&mut cmd, 0, SOC_CONFIG_CMD_FLAGS_DISCRETE);
        // latency @ 4 = 0 (AX200 mac_cfg.xtal_latency)
        self.send_hcmd(SYSTEM_GROUP, SOC_CONFIGURATION_CMD, &cmd);
        self.pump_rx(20);
    }

    // ── iwl_mvm_send_dqa_cmd (mvm/fw.c) ───────────────────────────
    // Enable dynamic queue allocation. cmd_queue = IWL_MVM_DQA_CMD_QUEUE (0).
    // Sent only if the firmware advertises DQA_SUPPORT (the gate in iwl_mvm_up).
    fn send_dqa(&mut self) {
        if !fw_has_capa(CAPA_DQA_SUPPORT) {
            return;
        }
        let mut cmd = [0u8; DQA_ENABLE_CMD_LEN];
        put_u32(&mut cmd, 0, IWL_CMD_QUEUE_ID);
        self.send_hcmd(DATA_PATH_GROUP, DQA_ENABLE_CMD, &cmd);
        self.pump_rx(20);
    }

    // ── iwl_mvm_power_update_device (mvm/power.c) ─────────────────
    // Device power table. The default power scheme is BPS (not CAM), so power
    // save is enabled; no other flags apply outside D3.
    fn send_power(&mut self) {
        let mut cmd = [0u8; DEVICE_POWER_CMD_LEN];
        put_u16(&mut cmd, 0, DEVICE_POWER_FLAGS_POWER_SAVE_ENA);
        self.send_hcmd(0, POWER_TABLE_CMD, &cmd);
        self.pump_rx(20);
    }

    // ── iwl_mvm_init_mcc → iwl_mvm_update_mcc (mvm/nvm.c) ──────────
    // Set the firmware regulatory domain. With LAR enabled the firmware refuses
    // to scan until the regdomain is set ("Disallow scans that might crash the
    // FW while the LAR regdomain is not set"). The first update queries the FW's
    // own default — alpha2 "ZZ", source GET_CURRENT — and the FW replies with
    // its chosen MCC + channel profile (CMD_WANT_SKB), after which scans run.
    fn set_regulatory(&mut self) {
        let mut cmd = [0u8; MCC_UPDATE_CMD_LEN];
        put_u16(&mut cmd, MCC_OFF_MCC, MCC_ALPHA2_ZZ);
        cmd[MCC_OFF_SOURCE] = MCC_SOURCE_GET_CURRENT;
        self.send_hcmd(0, MCC_UPDATE_CMD, &cmd);
        host::print("[ax200] MCC_UPDATE_CMD (ZZ / get-current) sent, waiting...\n");

        // The command is promoted to LONG_GROUP (1) in send_hcmd, so its
        // WANT_SKB response echoes group 1 (cf. NVM_GET_INFO echoing its group).
        match self.wait_rx(MCC_UPDATE_CMD, IWL_ALWAYS_LONG_GROUP, 2000) {
            Some(rb) => {
                let mut p = [0u8; 40];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let b = RX_PKT_DATA_OFF;
                let rd32 = |o: usize| {
                    u32::from_le_bytes([p[b + o], p[b + o + 1], p[b + o + 2], p[b + o + 3]])
                };
                let mcc = u16::from_le_bytes([p[b + MCC_RESP_OFF_MCC], p[b + MCC_RESP_OFF_MCC + 1]]);
                let cc = [(mcc >> 8) as u8, mcc as u8];
                host::print("[ax200]   MCC set to '");
                host::print(unsafe { core::str::from_utf8_unchecked(&cc) });
                host::print("' status=0x");
                host::print_hex32(rd32(MCC_RESP_OFF_STATUS));
                host::print(" n_channels=0x");
                host::print_hex32(rd32(MCC_RESP_OFF_N_CHANNELS));
                host::print("\n");
            }
            None => {
                host::print("[ax200]   MCC: no response (scan may stay blocked)\n");
                // No response to a CMD_WANT_SKB command is a strong sign the FW
                // asserted on an earlier command — dump its error log.
                self.dump_fw_error_log();
            }
        }
    }

    // ── iwl_mvm_mac_ctxt_add → iwl_mvm_mac_ctxt_cmd_sta (mvm/mac-ctxt.c) ──
    // Add the firmware MAC context the scan references. mac80211 creates this
    // at add_interface; our driver-initiated scan must add it first or the
    // firmware silently drops the scan (scan_start_mac_or_link_id points at a
    // non-existent context). We model a single unassociated STATION vif:
    // iwl_mvm_mac_ctxt_init assigns the first non-p2p station id 0 / color 0 /
    // TSF A. node_addr is our own MAC (CSR strap, OTP fallback); bssid is
    // broadcast (no BSS yet). is_assoc = 0 makes the firmware forward foreign
    // beacons (MAC_FILTER_IN_BEACON). cck/ofdm_rates are the default mandatory
    // ACK bitmaps iwl_mvm_ack_rates yields for an empty BSSBasicRateSet.
    // protection_flags / qos_flags / ac[] stay 0: a passive scan transmits
    // nothing, so the per-AC EDCA params (populated by mac80211's conf_tx
    // before any real TX) are unused here. Sent fire-and-forget like the other
    // config commands (CMD_SYNC reclaim, no RX notification of its own).
    fn add_mac_context(&mut self) {
        let mut cmd = [0u8; MAC_CTX_CMD_LEN];
        put_u32(&mut cmd, MC_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(0, 0)
        put_u32(&mut cmd, MC_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut cmd, MC_OFF_MAC_TYPE, FW_MAC_TYPE_BSS_STA);
        put_u32(&mut cmd, MC_OFF_TSF_ID, 0); // TSF_ID_A

        let mut mac =
            mac_from_regs(self.r32(CSR_MAC_ADDR0_STRAP), self.r32(CSR_MAC_ADDR1_STRAP));
        if !is_valid_mac(&mac) {
            mac = mac_from_regs(self.r32(CSR_MAC_ADDR0_OTP), self.r32(CSR_MAC_ADDR1_OTP));
        }
        cmd[MC_OFF_NODE_ADDR..MC_OFF_NODE_ADDR + 6].copy_from_slice(&mac);
        for b in &mut cmd[MC_OFF_BSSID_ADDR..MC_OFF_BSSID_ADDR + 6] {
            *b = 0xFF; // eth_broadcast_addr (no bssid_override, no bss_conf.bssid)
        }

        put_u32(&mut cmd, MC_OFF_CCK_RATES, MAC_CCK_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_OFDM_RATES, MAC_OFDM_RATES_DEFAULT);
        // protection_flags / cck_short_preamble / short_slot / qos_flags = 0.
        put_u32(&mut cmd, MC_OFF_FILTER_FLAGS, MAC_FILTER_ACCEPT_GRP | MAC_FILTER_IN_BEACON);
        // union iwl_mac_data_sta: is_assoc = 0 and all timing fields = 0.

        self.send_hcmd(0, MAC_CONTEXT_CMD_OP, &cmd);
        host::print("[ax200] MAC_CONTEXT_CMD (add station ctx id 0) sent\n");
        self.pump_rx(50);
    }

    // ── iwl_set_hw_address_from_csr / iwl_flip_hw_address ──────────
    // Read the 6-byte MAC from the STRAP registers; if the result isn't a valid
    // unicast address, fall back to the OTP registers. Store it for netdev
    // registration and log it.
    fn read_mac_address(&mut self) {
        let mut mac = mac_from_regs(self.r32(CSR_MAC_ADDR0_STRAP), self.r32(CSR_MAC_ADDR1_STRAP));
        if !is_valid_mac(&mac) {
            mac = mac_from_regs(self.r32(CSR_MAC_ADDR0_OTP), self.r32(CSR_MAC_ADDR1_OTP));
        }
        self.mac = mac;
        host::print("[ax200]   MAC address: ");
        Self::print_mac(&mac);
        host::print("\n");
    }

    fn print_mac(mac: &[u8; 6]) {
        for i in 0..6 {
            if i != 0 {
                host::print(":");
            }
            host::print_hex8(mac[i]);
        }
    }

    // ── iwl_pcie_rxmq_restock — recycle one consumed RB ────────────
    // Re-post the RB identified by `vid` into the free-BD ring at the next write
    // slot so the firmware can fill it again. The page is the same; only its
    // ring position changes (bd[slot] = page_dma | vid, gen2 < AX210).
    fn recycle_rb(&mut self, vid: u32) {
        let slot = (self.free_bd_write & (NUM_RBDS as u32 - 1)) as usize;
        let rb = self.rb_pool[vid as usize - 1];
        let entry = rb.phys | vid as u64;
        host::dma_write_buf(self.rxq_bd.handle, (slot * 8) as u32, &entry.to_le_bytes());
        self.free_bd_write += 1;
    }

    // Push the recycled free-BD write index to the HW (round down to 8).
    fn flush_free_bd(&self) {
        host::fence();
        self.w32(RFH_Q0_FRBDCB_WIDX_TRG, self.free_bd_write & !0x7);
    }

    // ── iwl_mvm_scan_umac_v14_and_above (mvm/scan.c, version 15) ────
    // Build a passive regular scan over the 2.4 GHz channels (1..13) and send it
    // as SCAN_REQ_UMAC. Passive (n_ssids = 0 → FORCE_PASSIVE) means no probe
    // request is transmitted, so probe_params stays zeroed; PASS_ALL makes the
    // firmware forward every beacon to the host. All general/channel parameters
    // are filled exactly as the Linux fill helpers do (dwell 10/110, adwell
    // 2/8/10, budget 300, EXT_6 priority, UNASSOC timing = 0, adaptive dwell).
    fn build_scan_cmd(&self, buf: &mut [u8]) {
        // uid @ 0 = 0; ooc_priority + scan_priority = IWL_SCAN_PRIORITY_EXT_6.
        put_u32(buf, SC_OFF_OOC_PRIORITY, SCAN_OOC_PRIORITY_REGULAR);

        // general_params_v11
        put_u16(buf, SC_OFF_GP_FLAGS, SCAN_GP_FLAGS_PASSIVE);
        // scan_start_mac_or_link_id = scan_vif->id (version < 16). Names the FW
        // MAC context added in add_mac_context(); 0 here, but set explicitly.
        buf[SC_OFF_GP_SCAN_START_MAC] = SCAN_VIF_MAC_ID;
        buf[SC_OFF_GP_ACTIVE_DWELL] = IWL_SCAN_DWELL_ACTIVE; // LB
        buf[SC_OFF_GP_ACTIVE_DWELL + 1] = IWL_SCAN_DWELL_ACTIVE; // HB
        buf[SC_OFF_GP_ADWELL_2G] = ADWELL_DEFAULT_LB_N_APS;
        buf[SC_OFF_GP_ADWELL_5G] = ADWELL_DEFAULT_HB_N_APS;
        buf[SC_OFF_GP_ADWELL_SOCIAL] = ADWELL_DEFAULT_N_APS_SOCIAL;
        // flags2 @ 17 = 0
        put_u16(buf, SC_OFF_GP_ADWELL_BUDGET, ADWELL_MAX_BUDGET_FULL);
        // max_out_of_time / suspend_time = 0 (UNASSOC timing)
        put_u32(buf, SC_OFF_GP_SCAN_PRIO, SCAN_OOC_PRIORITY_REGULAR);
        buf[SC_OFF_GP_PASSIVE_DWELL] = IWL_SCAN_DWELL_PASSIVE; // LB
        buf[SC_OFF_GP_PASSIVE_DWELL + 1] = IWL_SCAN_DWELL_PASSIVE; // HB
        // num_of_fragments = 0

        // channel_params_v7 — the NVM_CHANNEL_VALID channels from read_nvm,
        // both bands. Per channel, the band rides in the v2.band BYTE (@ +5);
        // see the band-encoding note below.
        buf[SC_OFF_CP_FLAGS] = SCAN_CHAN_FLAG_ENABLE_CHAN_ORDER;
        buf[SC_OFF_CP_COUNT] = self.n_scan_chans as u8;
        buf[SC_OFF_CP_N_APS_OVERRIDE] = SCAN_N_APS_GO_FRIENDLY;
        buf[SC_OFF_CP_N_APS_OVERRIDE + 1] = SCAN_N_APS_SOCIAL_CHS;
        for i in 0..self.n_scan_chans {
            let o = SC_OFF_CP_CHANNELS + i * SCAN_CH_CFG_LEN;
            // iwl_mvm_umac_scan_cfg_channels_v7, version < 17 (our cmd_ver is 15):
            // cfg.flags holds the directed-scan SSID bitmap (bits 0-19) — 0 for a
            // passive station scan (no SSID, n_aps_flag only for P2P) — and the
            // band rides in the v2.band BYTE (@ +5), NOT in flags bits 30-31.
            // (The v17 path puts band in flags; doing that for v15 left band=0 =
            // PHY_BAND_5/5GHz on 2.4GHz channels → BAD scan params → FW assert.)
            // cfg.flags @ o stays 0 (zeroed buffer).
            buf[o + 4] = self.scan_chans[i]; // channel_num
            buf[o + 5] = self.scan_bands[i]; // v2.band (1 = 2.4 GHz, 0 = 5 GHz)
            buf[o + 6] = 1; // v2.iter_count
            // v2.iter_interval @ o+7 = 0
        }

        // periodic_params: regular scan = one plan, one iteration.
        buf[SC_OFF_PERIODIC_SCHED0_ITER] = 1;
        // probe_params: zeroed (passive scan, no probe request transmitted).
    }

    // ── iwl_mvm_rx_mpdu_mq (mvm/rxmq.c) — parse a scan beacon ─────
    // A REPLY_RX_MPDU_CMD RB holds: [len_n_flags 4][cmd_hdr 4][iwl_rx_mpdu_desc]
    // [802.11 frame]. For family < AX210 the descriptor is IWL_RX_DESC_SIZE_V1
    // (48), so the frame starts at RX_PKT_DATA_OFF + 48. We extract the BSSID
    // (addr3), the SSID (IE 0), the RSSI (max of the two energy chains, negated
    // to dBm), and the channel, de-duplicating by BSSID. Only beacon / probe-
    // response management frames carry these, so other subtypes are skipped.
    fn parse_beacon(rb: &Dma, aps: &mut [Ap], n_aps: &mut usize) {
        let mut buf = [0u8; 384];
        host::dma_read_buf(rb.handle, 0, &mut buf);
        let d = RX_PKT_DATA_OFF; // iwl_rx_mpdu_desc base

        // RSSI: iwl_mvm_get_signal_strength — energy is a positive magnitude,
        // negated to dBm; 0 means "no signal" (S8_MIN). Take the stronger chain.
        let to_dbm = |e: u8| if e != 0 { -(e as i16) } else { -128 };
        let rssi = to_dbm(buf[d + MPDU_OFF_ENERGY_A]).max(to_dbm(buf[d + MPDU_OFF_ENERGY_B])) as i8;
        let channel = buf[d + MPDU_OFF_CHANNEL];

        let f = d + IWL_RX_DESC_SIZE_V1; // 802.11 frame
        // frame_control low byte: type bits 2-3 (0 = management), subtype 4-7.
        let fc = buf[f];
        if fc & 0x0c != 0 {
            return; // not a management frame
        }
        let subtype = (fc >> 4) & 0xf;
        if subtype != DOT11_STYPE_BEACON && subtype != DOT11_STYPE_PROBE_RESP {
            return;
        }

        let mut bssid = [0u8; 6];
        bssid.copy_from_slice(&buf[f + DOT11_OFF_ADDR3..f + DOT11_OFF_ADDR3 + 6]);
        // Beacon interval: fixed param after the 24-byte header + 8-byte timestamp
        // (__le16 TU). Needed for the connect MAC context (iwl_mac_data_sta.bi).
        let bi_off = f + DOT11_HDR_LEN + 8;
        let beacon_int = u16::from_le_bytes([buf[bi_off], buf[bi_off + 1]]);
        // Privacy bit of the capability field → AP is encrypted (needs RSN).
        let privacy = buf[f + DOT11_BEACON_CAP_OFF] & WLAN_CAP_PRIVACY_BIT != 0;
        // De-dup by BSSID; refresh RSSI if we hear a stronger beacon.
        for i in 0..*n_aps {
            if aps[i].bssid == bssid {
                if rssi > aps[i].rssi {
                    aps[i].rssi = rssi;
                }
                return;
            }
        }

        // Walk the information elements for the SSID (element id 0).
        let mut ssid = [0u8; SSID_MAX];
        let mut ssid_len = 0u8;
        let mut p = f + DOT11_OFF_IES;
        while p + 2 <= buf.len() {
            let id = buf[p];
            let len = buf[p + 1] as usize;
            if p + 2 + len > buf.len() {
                break;
            }
            if id == WLAN_EID_SSID {
                let l = len.min(SSID_MAX);
                ssid[..l].copy_from_slice(&buf[p + 2..p + 2 + l]);
                ssid_len = l as u8;
                break;
            }
            p += 2 + len;
        }

        if *n_aps < aps.len() {
            aps[*n_aps] = Ap { bssid, ssid, ssid_len, rssi, channel, beacon_int, privacy };
            *n_aps += 1;
        }
    }

    // Print the collected AP list (SSID, BSSID, RSSI, channel).
    fn print_aps(aps: &[Ap], n_aps: usize) {
        host::print("[ax200] === access points found: ");
        host::print_dec(n_aps as u32);
        host::print(" ===\n");
        for ap in &aps[..n_aps] {
            host::print("[ax200]   ");
            // SSID (printable ASCII; hidden / empty → <hidden>).
            if ap.ssid_len == 0 {
                host::print("<hidden>");
            } else {
                for &b in &ap.ssid[..ap.ssid_len as usize] {
                    let c = if (0x20..0x7f).contains(&b) { b } else { b'.' };
                    host::print(unsafe { core::str::from_utf8_unchecked(core::slice::from_ref(&c)) });
                }
            }
            host::print("  [");
            for i in 0..6 {
                if i != 0 {
                    host::print(":");
                }
                host::print_hex8(ap.bssid[i]);
            }
            host::print("]  ");
            // RSSI in dBm (always negative here).
            host::print("-");
            host::print_dec((-(ap.rssi as i32)) as u32);
            host::print(" dBm  ch ");
            host::print_dec(ap.channel as u32);
            host::print("\n");
        }
    }

    // ── iwl_pcie_rx_handle — service the multi-queue RX ring ──────
    // Drain every RB the firmware has closed since our last read, recycling
    // each one back into the free-BD ring so the firmware never runs dry.
    // `on_frame(cmd, group, &rb)` runs for each frame and returns `false` to
    // stop draining early (a terminal notification). Returns the number of
    // frames seen. Shared by the scan loop and the resident NIC service loop —
    // the one place that walks used_bd → rb_pool → recycle.
    fn service_rx<F: FnMut(u8, u8, &Dma) -> bool>(&mut self, mut on_frame: F) -> u32 {
        host::fence();
        let r = host::dma_r32(self.rxq_rb_stts.handle, 0) & RB_STTS_CLOSED_MASK;
        let mut frames = 0u32;
        let mut recycled = false;
        let mut stop = false;
        while self.rxq_read != r && !stop {
            let i = self.rxq_read as usize;
            let vid = host::dma_r32(self.rxq_used_bd.handle, (i * 4) as u32) & RX_VID_MASK;
            if vid >= 1 && vid as usize <= RX_NUM_RBS {
                let rb = self.rb_pool[vid as usize - 1];
                let mut hdr = [0u8; 8];
                host::dma_read_buf(rb.handle, 0, &mut hdr);
                frames += 1;
                if !on_frame(hdr[4], hdr[5], &rb) {
                    stop = true;
                }
                self.recycle_rb(vid);
                recycled = true;
            }
            self.rxq_read = (self.rxq_read + 1) & (NUM_RBDS as u32 - 1);
        }
        if recycled {
            self.flush_free_bd();
        }
        frames
    }

    // Send the scan and poll the RX ring (npk_sleep yields — never input_wait)
    // until SCAN_COMPLETE_UMAC arrives, parsing every beacon / probe response
    // into the AP list along the way.
    fn run_scan(&mut self) -> bool {
        let mut buf = [0u8; SCAN_CMD_LEN];
        self.build_scan_cmd(&mut buf);
        self.send_hcmd(IWL_ALWAYS_LONG_GROUP, SCAN_REQ_UMAC, &buf);
        host::print("[ax200] SCAN_REQ_UMAC sent (passive, ");
        host::print_dec(self.n_scan_chans as u32);
        host::print(" channels, 2.4 + 5 GHz), scanning...\n");

        let mut frames = 0u32;
        let mut aps = [Ap::EMPTY; MAX_APS];
        let mut n_aps = 0usize;
        let mut completed = false;
        // ~12 s budget: a passive scan of both bands (up to ~40 channels at
        // 110 ms dwell) takes longer than the 2.4-GHz-only scan did.
        for _ in 0..12000 {
            self.service_rx(|cmd, grp, rb| {
                if cmd == SCAN_COMPLETE_UMAC && grp == 0 {
                    completed = true;
                    return false; // stop draining; the scan is done
                }
                // Beacons / probe responses arrive as REPLY_RX_MPDU_CMD
                // (0xc1, LEGACY_GROUP) — parse them into the AP list.
                if cmd == REPLY_RX_MPDU_CMD && grp == 0 {
                    Self::parse_beacon(rb, &mut aps, &mut n_aps);
                }
                frames += 1;
                true
            });
            if completed {
                host::print("[ax200] SCAN_COMPLETE_UMAC received — frames seen: ");
                host::print_dec(frames);
                host::print("\n");
                Self::print_aps(&aps, n_aps);
                // Pick the strongest AP as the connect target (#3 connect).
                let mut best = usize::MAX;
                for i in 0..n_aps {
                    if best == usize::MAX || aps[i].rssi > aps[best].rssi {
                        best = i;
                    }
                }
                if best != usize::MAX {
                    self.target_bssid = aps[best].bssid;
                    self.target_chan = aps[best].channel;
                    // Band by channel: 1..14 = 2.4 GHz, else 5 GHz (iwl_nvm_channels).
                    self.target_band =
                        if aps[best].channel <= 14 { PHY_BAND_24 as u8 } else { PHY_BAND_5_U8 };
                    self.target_beacon_int = aps[best].beacon_int;
                    self.target_ssid = aps[best].ssid;
                    self.target_ssid_len = aps[best].ssid_len;
                    self.target_privacy = aps[best].privacy;
                    self.target_valid = true;
                }
                return true;
            }
            host::sleep_ms(1);
        }
        host::print("[ax200] SCAN_COMPLETE timeout — frames seen: 0x");
        host::print_hex32(frames);
        host::print("\n");
        self.dump_fw_error_log();
        false
    }

    // ── Stage 5a: PHY context + RLC + binding (connect step 1) ────
    // iwl_mvm_phy_ctxt_add + iwl_mvm_phy_send_rlc + iwl_mvm_binding_add_vif
    // (mvm/phy-ctxt.c, mvm/binding.c). Sets the target AP's operating channel
    // (PHY_CONTEXT_CMD v4, 20 MHz), configures the RX chains (RLC_CONFIG_CMD v2 —
    // not offloaded on this FW, cmd_ver=2 < 3), and binds the MAC context (id 0)
    // to the PHY context (id 0) (BINDING_CONTEXT_CMD v2, full struct: CDB binding
    // support, but lmac_id 0 since no CDB). PHY + RLC are fire-and-forget; BINDING
    // returns a status word (CMD_WANT_SKB). All cmd_vers parsed from the FW file.
    fn connect_phy_binding(&mut self) -> bool {
        // PHY_CONTEXT_CMD v4 (action ADD) — 20 MHz on the target channel/band.
        let mut pc = [0u8; PHY_CTX_CMD_LEN];
        put_u32(&mut pc, PC_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(phy 0, color 0)
        put_u32(&mut pc, PC_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut pc, PC_OFF_CI_CHANNEL, self.target_chan as u32);
        pc[PC_OFF_CI_BAND] = self.target_band;
        pc[PC_OFF_CI_WIDTH] = IWL_PHY_CHANNEL_MODE20;
        pc[PC_OFF_CI_CTRL_POS] = 0; // 20 MHz → control channel position 0
        put_u32(&mut pc, PC_OFF_LMAC_ID, IWL_LMAC_24G_INDEX); // no CDB → 0
        self.send_hcmd(0, PHY_CONTEXT_CMD, &pc); // legacy → LONG_GROUP
        host::print("[ax200] PHY_CONTEXT_CMD sent (ch ");
        host::print_dec(self.target_chan as u32);
        host::print(", band ");
        host::print_dec(self.target_band as u32);
        host::print(")\n");
        self.pump_rx(20);

        // RLC_CONFIG_CMD v2 (DATA_PATH_GROUP) — RX chains for the PHY context.
        let mut rlc = [0u8; RLC_CMD_LEN];
        put_u32(&mut rlc, RLC_OFF_PHY_ID, 0);
        put_u32(&mut rlc, RLC_OFF_RX_CHAIN_INFO, RLC_RX_CHAIN_INFO_2X2);
        self.send_hcmd(DATA_PATH_GROUP, RLC_CONFIG_CMD, &rlc);
        host::print("[ax200] RLC_CONFIG_CMD sent\n");
        self.pump_rx(20);

        // BINDING_CONTEXT_CMD v2 (action ADD): MAC ctx 0 ↔ PHY ctx 0.
        let mut bc = [0u8; BINDING_CMD_LEN];
        put_u32(&mut bc, BC_OFF_ID_COLOR, 0); // phy id 0 / color 0
        put_u32(&mut bc, BC_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut bc, BC_OFF_MACS, 0); // macs[0] = MAC ctx id 0
        put_u32(&mut bc, BC_OFF_MACS + 4, FW_CTXT_INVALID); // macs[1]
        put_u32(&mut bc, BC_OFF_MACS + 8, FW_CTXT_INVALID); // macs[2]
        put_u32(&mut bc, BC_OFF_PHY, 0); // phy id 0
        put_u32(&mut bc, BC_OFF_LMAC_ID, IWL_LMAC_24G_INDEX);
        self.send_hcmd(0, BINDING_CONTEXT_CMD, &bc); // legacy → LONG_GROUP
        host::print("[ax200] BINDING_CONTEXT_CMD sent, waiting for status...\n");
        match self.wait_rx(BINDING_CONTEXT_CMD, IWL_ALWAYS_LONG_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let status = le32(&p, RX_PKT_DATA_OFF);
                host::print("[ax200]   binding status=0x");
                host::print_hex32(status);
                host::print("\n");
                host::print("[ax200] Stage 5a OK — PHY context + RLC + binding\n");
                true
            }
            None => {
                host::print("[ax200] Stage 5a FAILED — binding no response\n");
                self.dump_fw_error_log();
                false
            }
        }
    }

    // ── Stage 5b: station (AP peer) + gen2 TX queue (connect step 2) ──
    // iwl_mvm_add_sta + iwl_mvm_tvqm_enable_txq → iwl_trans_txq_alloc (mvm/sta.c,
    // pcie/.../tx-gen2.c). Adds the AP as a LINK station (ADD_STA v12, sta_id 0,
    // minimal flags — HT/rate flags come at assoc) and allocates a dynamic gen2
    // management TX queue for it (for the auth/assoc frames): a TFD ring + first-
    // TB staging + byte-count table, registered with the firmware via
    // SCD_QUEUE_CONFIG_CMD v3 which returns the queue id. No frame is transmitted
    // yet (that is 5c). Both commands return a status (CMD_WANT_SKB).
    fn connect_add_station(&mut self) -> bool {
        // ADD_STA v12 (action ADD) — the AP peer station.
        let mut sc = [0u8; ADD_STA_CMD_LEN];
        sc[AS_OFF_ADD_MODIFY] = 0; // add (not modify)
        put_u16(&mut sc, AS_OFF_TID_DISABLE, TID_DISABLE_AGG_INIT);
        put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(mac 0, 0)
        sc[AS_OFF_ADDR..AS_OFF_ADDR + 6].copy_from_slice(&self.target_bssid);
        sc[AS_OFF_STA_ID] = AP_STA_ID;
        put_u32(&mut sc, AS_OFF_STATION_FLAGS, 0); // refined at assoc (5d)
        put_u32(&mut sc, AS_OFF_STATION_FLAGS_MSK, STA_FLAGS_MSK_ADD);
        sc[AS_OFF_STATION_TYPE] = IWL_STA_LINK;
        self.send_hcmd(0, ADD_STA, &sc); // legacy → LONG_GROUP
        host::print("[ax200] ADD_STA sent (AP peer, sta_id 0), waiting...\n");
        match self.wait_rx(ADD_STA, IWL_ALWAYS_LONG_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let status = le32(&p, RX_PKT_DATA_OFF) & ADD_STA_STATUS_MASK;
                host::print("[ax200]   ADD_STA status=0x");
                host::print_hex32(status);
                host::print("\n");
                if status != ADD_STA_SUCCESS {
                    host::print("[ax200] Stage 5b FAILED — ADD_STA rejected\n");
                    self.dump_fw_error_log();
                    return false;
                }
            }
            None => {
                host::print("[ax200] Stage 5b FAILED — ADD_STA no response\n");
                self.dump_fw_error_log();
                return false;
            }
        }

        // Allocate the management TX queue DMA: TFD ring + first-TB staging +
        // byte-count table (16-slot queue → IWL_MGMT_QUEUE_SIZE).
        self.mgmt_tfd = self.alloc_dma(TFH_TFD_SIZE * IWL_MGMT_QUEUE_SIZE, "mgmt.tfd");
        self.mgmt_first_tb =
            self.alloc_dma(IWL_FIRST_TB_SIZE_ALIGN * IWL_MGMT_QUEUE_SIZE, "mgmt.first_tb");
        self.mgmt_bc_tbl = self.alloc_dma(BC_TBL_BYTES, "mgmt.bc_tbl");
        if !self.mgmt_tfd.ok() || !self.mgmt_first_tb.ok() || !self.mgmt_bc_tbl.ok() {
            host::print("[ax200] Stage 5b FAILED — TX queue DMA alloc\n");
            return false;
        }

        // SCD_QUEUE_CONFIG_CMD v3 (ADD): hand the DMA addresses to the firmware.
        let mut q = [0u8; SCD_CMD_LEN];
        put_u32(&mut q, SQ_OFF_OPERATION, IWL_SCD_QUEUE_ADD);
        put_u32(&mut q, SQ_OFF_STA_MASK, 1 << AP_STA_ID); // BIT(sta_id)
        q[SQ_OFF_TID] = IWL_MGMT_TID;
        put_u32(&mut q, SQ_OFF_FLAGS, 0);
        put_u32(&mut q, SQ_OFF_CB_SIZE, MGMT_QUEUE_CB_SIZE);
        put_u64(&mut q, SQ_OFF_BC_DRAM_ADDR, self.mgmt_bc_tbl.phys);
        put_u64(&mut q, SQ_OFF_TFDQ_DRAM_ADDR, self.mgmt_tfd.phys);
        self.send_hcmd(DATA_PATH_GROUP, SCD_QUEUE_CONFIG_CMD, &q);
        host::print("[ax200] SCD_QUEUE_CONFIG_CMD sent (mgmt tid 15), waiting...\n");
        match self.wait_rx(SCD_QUEUE_CONFIG_CMD, DATA_PATH_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let b = RX_PKT_DATA_OFF;
                self.mgmt_queue_id = u16::from_le_bytes([
                    p[b + SQ_RSP_OFF_QUEUE_NUMBER],
                    p[b + SQ_RSP_OFF_QUEUE_NUMBER + 1],
                ]);
                self.mgmt_write_ptr = u16::from_le_bytes([
                    p[b + SQ_RSP_OFF_WRITE_PTR],
                    p[b + SQ_RSP_OFF_WRITE_PTR + 1],
                ]) as u32;
                host::print("[ax200]   mgmt queue_id=");
                host::print_dec(self.mgmt_queue_id as u32);
                host::print(" write_ptr=");
                host::print_dec(self.mgmt_write_ptr);
                host::print("\n");
                host::print("[ax200] Stage 5b OK — station + TX queue allocated\n");
                true
            }
            None => {
                host::print("[ax200] Stage 5b FAILED — SCD_QUEUE_CONFIG no response\n");
                self.dump_fw_error_log();
                false
            }
        }
    }

    // ── Stage 5b': finish the connect chanctx tail (iwl_mvm_assign_vif_chanctx) ──
    // __iwl_mvm_assign_vif_chanctx does binding → power_update_mac → (quota, only
    // for monitor) and the connect flow then re-sends the MAC context with the
    // target BSSID (iwl_mvm_mac_ctxt_changed on BSS_CHANGED_BSSID). We had skipped
    // this whole tail and went straight to the auth TX with a MAC context still
    // holding the scan-time broadcast BSSID + zero timing — so once session
    // protection put the firmware on-channel and it actually processed the auth,
    // the time-event/scheduler (UMAC) asserted. Send both before the auth.
    fn connect_finish_chanctx(&mut self) {
        // iwl_mvm_power_update_mac → MAC_PM_POWER_TABLE for the bss vif. Power-save
        // disabled path: only id_and_color + keep_alive_seconds, flags = 0.
        let mut pm = [0u8; MAC_POWER_CMD_LEN];
        put_u32(&mut pm, MP_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(0,0)
        put_u16(&mut pm, MP_OFF_KEEP_ALIVE, POWER_KEEP_ALIVE_PERIOD_SEC);
        self.send_hcmd(0, MAC_PM_POWER_TABLE, &pm);
        host::print("[ax200] MAC_PM_POWER_TABLE sent (PS disabled)\n");
        self.pump_rx(20);

        // iwl_mvm_mac_ctxt_changed (MODIFY) with the target AP's BSSID + timing,
        // unassociated branch (is_assoc = 0, MAC_FILTER_IN_BEACON).
        let mut cmd = [0u8; MAC_CTX_CMD_LEN];
        put_u32(&mut cmd, MC_OFF_ID_COLOR, 0);
        put_u32(&mut cmd, MC_OFF_ACTION, FW_CTXT_ACTION_MODIFY);
        put_u32(&mut cmd, MC_OFF_MAC_TYPE, FW_MAC_TYPE_BSS_STA);
        put_u32(&mut cmd, MC_OFF_TSF_ID, 0);
        cmd[MC_OFF_NODE_ADDR..MC_OFF_NODE_ADDR + 6].copy_from_slice(&self.mac);
        cmd[MC_OFF_BSSID_ADDR..MC_OFF_BSSID_ADDR + 6].copy_from_slice(&self.target_bssid);
        put_u32(&mut cmd, MC_OFF_CCK_RATES, MAC_CCK_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_OFDM_RATES, MAC_OFDM_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_FILTER_FLAGS, MAC_FILTER_ACCEPT_GRP | MAC_FILTER_IN_BEACON);
        // iwl_mac_data_sta (unassoc): is_assoc = 0, bi = beacon interval, dtim
        // unknown pre-assoc (→ 0), assoc_id = 0.
        put_u32(&mut cmd, MC_OFF_STA_IS_ASSOC, 0);
        put_u32(&mut cmd, MC_OFF_STA_BI, self.target_beacon_int as u32);
        self.send_hcmd(0, MAC_CONTEXT_CMD_OP, &cmd);
        host::print("[ax200] MAC_CONTEXT_CMD (modify, target BSSID) sent\n");
        self.pump_rx(50);
    }

    // ── gen2 mgmt-frame TX (iwl_txq_gen2_tx + iwl_txq_gen2_build_tx) ──────
    // Transmit one 802.11 management frame on the AP station's queue. Builds a
    // device TX command — short iwl_cmd_header (TX_CMD, group 0) + iwl_tx_cmd_v9
    // (len/flags/host-rate) + the frame — and lays it across the TFD as two TBs
    // (TB0 = first-TB staging with the first 20 bytes, TB1 = the remainder from
    // cmd_data), fills the byte-count table, bumps the write pointer and rings the
    // doorbell. For AX200 (< BZ) mgmt frames use the host rate (IWL_TX_FLAGS_CMD_RATE).
    // Generic gen2 TX onto a given queue (mgmt or data): build the dev TX command
    // (short header + tx_cmd_v9 + frame) across the TFD's two TBs, fill the
    // byte-count table, bump the write pointer and ring the doorbell. Returns the
    // advanced write pointer. (The 802.11 frame is built by the caller.)
    fn tx_raw(&self, qid: u16, wptr: u32, tfd_ring: Dma, first_tb: Dma, bc: Dma, flags: u32, frame: &[u8]) -> u32 {
        let idx = (wptr & (IWL_MGMT_QUEUE_SIZE as u32 - 1)) as usize;
        let mut buf = [0u8; 320];
        buf[0] = TX_CMD;
        buf[1] = 0; // group 0 (short header)
        let seq = (((qid) & 0x1f) << 8) | (idx as u16 & 0xff);
        buf[2..4].copy_from_slice(&seq.to_le_bytes());
        let frame_len = frame.len() as u16;
        put_u16(&mut buf, TXC_OFF_LEN, frame_len);
        put_u16(&mut buf, TXC_OFF_OFFLOAD, 0);
        put_u32(&mut buf, TXC_OFF_FLAGS, flags);
        let rate = if self.target_band == PHY_BAND_24 as u8 {
            RATE_1M_CCK_ANT_A
        } else {
            RATE_6M_OFDM_ANT_A
        };
        put_u32(&mut buf, TXC_OFF_RATE, rate);
        buf[TXC_OFF_FRAME..TXC_OFF_FRAME + frame.len()].copy_from_slice(frame);
        let total = TXC_OFF_FRAME + frame.len();

        host::dma_write_buf(self.cmd_data.handle, 0, &buf[..total]);
        let ftb_off = (idx * IWL_FIRST_TB_SIZE_ALIGN) as u32;
        host::dma_write_buf(first_tb.handle, ftb_off, &buf[..IWL_FIRST_TB_SIZE]);

        let mut tfd = [0u8; TFH_TFD_SIZE];
        put_tfh_tb(&mut tfd, 0, IWL_FIRST_TB_SIZE as u16, first_tb.phys + ftb_off as u64);
        put_tfh_tb(&mut tfd, 1, (total - IWL_FIRST_TB_SIZE) as u16, self.cmd_data.phys + IWL_FIRST_TB_SIZE as u64);
        tfd[0..2].copy_from_slice(&2u16.to_le_bytes()); // num_tbs
        host::dma_write_buf(tfd_ring.handle, (idx * TFH_TFD_SIZE) as u32, &tfd);

        let bc_ent = ((frame_len as u32 + 3) / 4) as u16;
        host::dma_write_buf(bc.handle, (idx * 2) as u32, &bc_ent.to_le_bytes());

        host::fence();
        let next = (wptr + 1) & (MAX_TFD_QUEUE_SIZE - 1);
        self.w32(HBUS_TARG_WRPTR, next | ((qid as u32) << 16));
        next
    }

    fn tx_mgmt_frame(&mut self, frame: &[u8]) {
        self.mgmt_write_ptr = self.tx_raw(
            self.mgmt_queue_id,
            self.mgmt_write_ptr,
            self.mgmt_tfd,
            self.mgmt_first_tb,
            self.mgmt_bc_tbl,
            IWL_TX_FLAGS_ENCRYPT_DIS | IWL_TX_FLAGS_CMD_RATE,
            frame,
        );
    }

    // Transmit a payload as an 802.11 DATA frame on the data queue (toDS:
    // addr1=BSSID, addr2=us, addr3=dst) + LLC/SNAP. `encrypt`=false sets
    // ENCRYPT_DIS (EAPOL during the 4-way); =true lets the firmware encrypt with
    // the installed PTK (IP traffic after AUTHORIZED).
    fn tx_8023(&mut self, dst: [u8; 6], ethertype: u16, payload: &[u8], encrypt: bool) {
        let mut fr = [0u8; 1600];
        fr[0] = DOT11_FC_DATA;
        fr[1] = DOT11_FC1_TODS;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&dst);
        let mut p = DOT11_HDR_LEN;
        fr[p..p + 6].copy_from_slice(&LLC_SNAP_HDR);
        fr[p + 6] = (ethertype >> 8) as u8;
        fr[p + 7] = ethertype as u8;
        p += 8;
        fr[p..p + payload.len()].copy_from_slice(payload);
        p += payload.len();
        let flags = if encrypt {
            IWL_TX_FLAGS_CMD_RATE
        } else {
            IWL_TX_FLAGS_ENCRYPT_DIS | IWL_TX_FLAGS_CMD_RATE
        };
        self.data_write_ptr = self.tx_raw(
            self.data_queue_id,
            self.data_write_ptr,
            self.data_tfd,
            self.data_first_tb,
            self.data_bc_tbl,
            flags,
            &fr[..p],
        );
    }

    // Convert an Ethernet frame from the IP stack ([dst 6][src 6][etype 2][pl])
    // into an encrypted 802.11 data frame and transmit it.
    fn tx_eth(&mut self, eth: &[u8]) {
        if eth.len() < 14 {
            return;
        }
        let mut dst = [0u8; 6];
        dst.copy_from_slice(&eth[0..6]);
        let ethertype = ((eth[12] as u16) << 8) | eth[13] as u16;
        self.tx_8023(dst, ethertype, &eth[14..], true);
    }

    // Allocate a gen2 data TX queue (tid 0) for the AP station, so EAPOL frames
    // have a data path. Same SCD_QUEUE_CONFIG mechanism as the mgmt queue.
    fn alloc_data_queue(&mut self) -> bool {
        self.data_tfd = self.alloc_dma(TFH_TFD_SIZE * IWL_DATA_QUEUE_SIZE, "data.tfd");
        self.data_first_tb =
            self.alloc_dma(IWL_FIRST_TB_SIZE_ALIGN * IWL_DATA_QUEUE_SIZE, "data.first_tb");
        self.data_bc_tbl = self.alloc_dma(BC_TBL_BYTES, "data.bc_tbl");
        if !self.data_tfd.ok() || !self.data_first_tb.ok() || !self.data_bc_tbl.ok() {
            return false;
        }
        let mut q = [0u8; SCD_CMD_LEN];
        put_u32(&mut q, SQ_OFF_OPERATION, IWL_SCD_QUEUE_ADD);
        put_u32(&mut q, SQ_OFF_STA_MASK, 1 << AP_STA_ID);
        q[SQ_OFF_TID] = IWL_DATA_TID;
        put_u32(&mut q, SQ_OFF_CB_SIZE, MGMT_QUEUE_CB_SIZE);
        put_u64(&mut q, SQ_OFF_BC_DRAM_ADDR, self.data_bc_tbl.phys);
        put_u64(&mut q, SQ_OFF_TFDQ_DRAM_ADDR, self.data_tfd.phys);
        self.send_hcmd(DATA_PATH_GROUP, SCD_QUEUE_CONFIG_CMD, &q);
        match self.wait_rx(SCD_QUEUE_CONFIG_CMD, DATA_PATH_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let b = RX_PKT_DATA_OFF;
                self.data_queue_id =
                    u16::from_le_bytes([p[b + SQ_RSP_OFF_QUEUE_NUMBER], p[b + SQ_RSP_OFF_QUEUE_NUMBER + 1]]);
                self.data_write_ptr =
                    u16::from_le_bytes([p[b + SQ_RSP_OFF_WRITE_PTR], p[b + SQ_RSP_OFF_WRITE_PTR + 1]]) as u32;
                host::print("[ax200] data queue_id=");
                host::print_dec(self.data_queue_id as u32);
                host::print("\n");
                true
            }
            None => false,
        }
    }

    // ADD_STA_KEY (0x17, cmd_ver 3) — install a CCMP key the supplicant computed.
    // `group` = GTK (multicast) vs PTK (pairwise). rx_mic/tx_mic/tx_seq stay 0.
    fn install_key(&mut self, group: bool, key_idx: u8, key: &[u8], rsc: &[u8]) {
        let mut cmd = [0u8; ADD_STA_KEY_LEN];
        cmd[KEY_OFF_STA_ID] = AP_STA_ID;
        cmd[KEY_OFF_KEY_OFFSET] = if group { 1 } else { 0 };
        let mut flags = STA_KEY_FLG_CCM | ((key_idx as u16) << STA_KEY_FLG_KEYID_POS);
        if group {
            flags |= STA_KEY_MULTICAST;
        }
        put_u16(&mut cmd, KEY_OFF_KEY_FLAGS, flags);
        let kl = key.len().min(32);
        cmd[KEY_OFF_KEY..KEY_OFF_KEY + kl].copy_from_slice(&key[..kl]);
        let rl = rsc.len().min(16);
        cmd[KEY_OFF_RX_SEQ..KEY_OFF_RX_SEQ + rl].copy_from_slice(&rsc[..rl]);
        self.send_hcmd(0, ADD_STA_KEY_CMD, &cmd); // → LONG_GROUP(1)
        self.pump_rx(20);
        host::print(if group {
            "[ax200] ADD_STA_KEY GTK installed\n"
        } else {
            "[ax200] ADD_STA_KEY PTK installed\n"
        });
    }

    // Extract an 802.11 management frame from an RX buffer if it is addressed to
    // us (addr1 == our MAC). Returns (subtype, first 8 body bytes after the 24-byte
    // header). Same RB layout as parse_beacon: frame @ RX_PKT_DATA_OFF + desc(48).
    fn rx_mgmt_for_us(rb: &Dma, our_mac: &[u8; 6]) -> Option<(u8, [u8; 8])> {
        let mut buf = [0u8; 96];
        host::dma_read_buf(rb.handle, 0, &mut buf);
        let f = RX_PKT_DATA_OFF + IWL_RX_DESC_SIZE_V1; // 56
        let fc = buf[f];
        if fc & 0x0c != 0 {
            return None; // not a management frame
        }
        if buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6] != our_mac[..] {
            return None; // not addressed to us
        }
        let subtype = (fc >> 4) & 0xf;
        let b = f + DOT11_HDR_LEN;
        let mut body = [0u8; 8];
        body.copy_from_slice(&buf[b..b + 8]);
        Some((subtype, body))
    }

    // Classify a received 802.11 DATA frame addressed to us: EAPOL (4-way) vs IP.
    // Finds the LLC/SNAP header at the 802.11 header end or 8 bytes further (an
    // intact CCMP header on a just-decrypted frame). EAPOL → the self-describing
    // EAPOL frame in `out`; IP → an Ethernet frame [dst=us][src=addr3][etype][pl].
    fn rx_classify(rb: &Dma, our_mac: &[u8; 6], out: &mut [u8]) -> RxKind {
        let mut buf = [0u8; 1600];
        host::dma_read_buf(rb.handle, 0, &mut buf);
        let d = RX_PKT_DATA_OFF;
        let mpdu_len = (((buf[d + MPDU_OFF_MPDU_LEN + 1] as usize) << 8)
            | buf[d + MPDU_OFF_MPDU_LEN] as usize)
            .min(buf.len() - d);
        let f = d + IWL_RX_DESC_SIZE_V1; // 56
        let fc = buf[f];
        if fc & 0x0c != DOT11_FC_TYPE_DATA {
            return RxKind::None;
        }
        if buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6] != our_mac[..] {
            return RxKind::None;
        }
        let subtype = (fc >> 4) & 0xf;
        let hdrlen = if subtype & DOT11_STYPE_QOS != 0 { 26 } else { 24 };
        // LLC/SNAP directly after the header, or after an 8-byte CCMP header.
        let llc = if f + hdrlen + 6 <= buf.len() && buf[f + hdrlen..f + hdrlen + 6] == LLC_SNAP_HDR {
            f + hdrlen
        } else if f + hdrlen + 14 <= buf.len() && buf[f + hdrlen + 8..f + hdrlen + 14] == LLC_SNAP_HDR {
            f + hdrlen + 8
        } else {
            return RxKind::None;
        };
        let ethertype = ((buf[llc + 6] as u16) << 8) | buf[llc + 7] as u16;
        let pl = llc + 8; // payload after LLC/SNAP
        if ethertype == ETHERTYPE_EAPOL {
            if pl + 4 > buf.len() {
                return RxKind::None;
            }
            let elen = 4 + (((buf[pl + 2] as usize) << 8) | buf[pl + 3] as usize);
            if pl + elen > buf.len() || elen > out.len() {
                return RxKind::None;
            }
            out[..elen].copy_from_slice(&buf[pl..pl + elen]);
            RxKind::Eapol(elen)
        } else {
            // Ethernet frame for the IP stack: dst = us (addr1), src = addr3 (SA).
            let end = (f + mpdu_len).min(buf.len());
            if end <= pl || 14 + (end - pl) > out.len() {
                return RxKind::None;
            }
            let plen = end - pl;
            out[0..6].copy_from_slice(&buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6]);
            out[6..12].copy_from_slice(&buf[f + DOT11_OFF_ADDR3..f + DOT11_OFF_ADDR3 + 6]);
            out[12] = (ethertype >> 8) as u8;
            out[13] = ethertype as u8;
            out[14..14 + plen].copy_from_slice(&buf[pl..end]);
            RxKind::Ip(14 + plen)
        }
    }

    // Drain the RX ring up to `ms` ms looking for a management frame of the given
    // subtype addressed to us; return its first 8 body bytes. Recycles RBs.
    fn wait_mgmt_response(&mut self, want_subtype: u8, ms: u32) -> Option<[u8; 8]> {
        let our_mac = self.mac;
        let mut found: Option<[u8; 8]> = None;
        for _ in 0..ms {
            self.service_rx(|cmd, grp, rb| {
                if cmd == REPLY_RX_MPDU_CMD && grp == 0 {
                    if let Some((st, body)) = Self::rx_mgmt_for_us(rb, &our_mac) {
                        if st == want_subtype {
                            found = Some(body);
                            return false;
                        }
                    }
                }
                true
            });
            if found.is_some() {
                break;
            }
            host::sleep_ms(1);
        }
        found
    }

    // ── Stage 5c: open-system AUTH (connect step 3) ──────────────────────
    // Session protection (the prepare_tx hook) then an open-system auth request to
    // the target AP, then wait for the AP's auth response (subtype auth, seq 2).
    fn connect_send_auth(&mut self) -> bool {
        // SESSION_PROTECTION_CMD (cmd_ver 1, wait_for_notif=false) — reserves
        // channel time so the firmware actually transmits the unassociated frame.
        let mut sp = [0u8; SP_CMD_LEN];
        put_u32(&mut sp, SP_OFF_ID_COLOR, 0);
        put_u32(&mut sp, SP_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut sp, SP_OFF_CONF_ID, SESSION_PROTECT_CONF_ASSOC);
        put_u32(&mut sp, SP_OFF_DURATION_TU, SP_DURATION_TU);
        self.send_hcmd(MAC_CONF_GROUP, SESSION_PROTECTION_CMD, &sp);
        host::print("[ax200] SESSION_PROTECTION_CMD sent (assoc, 878 TU)\n");
        self.pump_rx(50);

        // 802.11 open-system auth request: DA/BSSID = AP, SA = us, seq 1.
        let mut fr = [0u8; DOT11_HDR_LEN + DOT11_AUTH_BODY_LEN];
        fr[0] = DOT11_FC_AUTH;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&self.target_bssid);
        let b = DOT11_HDR_LEN;
        put_u16(&mut fr, b, DOT11_AUTH_ALG_OPEN);
        put_u16(&mut fr, b + 2, DOT11_AUTH_SEQ_1);
        put_u16(&mut fr, b + 4, 0);
        self.tx_mgmt_frame(&fr);
        host::print("[ax200] AUTH request TX'd (open-system), waiting for response...\n");

        // Auth response body: algorithm(2), seq(2), status(2).
        match self.wait_mgmt_response(DOT11_STYPE_AUTH, 2000) {
            Some(body) => {
                let seq = u16::from_le_bytes([body[2], body[3]]);
                let status = u16::from_le_bytes([body[4], body[5]]);
                if status == DOT11_STATUS_SUCCESS && seq == DOT11_AUTH_SEQ_2 {
                    host::print("[ax200] Stage 5c OK — AUTH accepted (status 0, seq 2)\n");
                    true
                } else {
                    host::print("[ax200] AUTH rejected: status=");
                    host::print_dec(status as u32);
                    host::print(" seq=");
                    host::print_dec(seq as u32);
                    host::print("\n");
                    false
                }
            }
            None => {
                host::print("[ax200] Stage 5c — no AUTH response\n");
                self.dump_fw_error_log();
                false
            }
        }
    }

    // ── Stage 5d: association request → response (connect step 4) ─────────
    // Build an association request (mac80211 ieee80211_send_assoc, legacy IE set)
    // for the target AP, transmit it, and wait for the association response
    // (subtype 1) to read the status code + AID. For an encrypted AP we include a
    // WPA2-PSK-CCMP RSN element so the AP accepts the association (the 4-way
    // handshake / key install that follows lives in wifid — Phase H).
    fn connect_send_assoc(&mut self) -> bool {
        let mut fr = [0u8; 256];
        fr[0] = DOT11_FC_ASSOC_REQ;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&self.target_bssid);
        let mut p = DOT11_HDR_LEN;

        // Fixed fields: capability info + listen interval.
        let mut cap = WLAN_CAP_ESS | WLAN_CAP_SHORT_PREAMBLE;
        if self.target_band == PHY_BAND_24 as u8 {
            cap |= WLAN_CAP_SHORT_SLOT;
        }
        if self.target_privacy {
            cap |= WLAN_CAP_PRIVACY;
        }
        put_u16(&mut fr, p, cap);
        put_u16(&mut fr, p + 2, DOT11_LISTEN_INTERVAL);
        p += 4;

        // SSID element.
        let sl = self.target_ssid_len as usize;
        fr[p] = WLAN_EID_SSID;
        fr[p + 1] = sl as u8;
        fr[p + 2..p + 2 + sl].copy_from_slice(&self.target_ssid[..sl]);
        p += 2 + sl;

        // Supported + extended supported rates (rate byte = Mbps*2, basic bit 0x80).
        let (supp, ext): (&[u8], &[u8]) = if self.target_band == PHY_BAND_24 as u8 {
            (&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24], &[0x30, 0x48, 0x60, 0x6c])
        } else {
            (&[0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c], &[])
        };
        fr[p] = WLAN_EID_SUPP_RATES;
        fr[p + 1] = supp.len() as u8;
        fr[p + 2..p + 2 + supp.len()].copy_from_slice(supp);
        p += 2 + supp.len();
        if !ext.is_empty() {
            fr[p] = WLAN_EID_EXT_SUPP_RATES;
            fr[p + 1] = ext.len() as u8;
            fr[p + 2..p + 2 + ext.len()].copy_from_slice(ext);
            p += 2 + ext.len();
        }

        // RSN element (WPA2-PSK-CCMP) for encrypted APs.
        if self.target_privacy {
            let rsn: [u8; 20] = [
                0x01, 0x00, // version 1
                0x00, 0x0f, 0xac, 0x04, // group cipher: CCMP
                0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, // pairwise: 1 × CCMP
                0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, // AKM: 1 × PSK
                0x00, 0x00, // RSN capabilities
            ];
            fr[p] = WLAN_EID_RSN;
            fr[p + 1] = rsn.len() as u8;
            fr[p + 2..p + 2 + rsn.len()].copy_from_slice(&rsn);
            p += 2 + rsn.len();
        }

        self.tx_mgmt_frame(&fr[..p]);
        host::print("[ax200] ASSOC request TX'd, waiting for response...\n");

        // Assoc response body: capability(2), status_code(2), aid(2).
        match self.wait_mgmt_response(DOT11_STYPE_ASSOC_RESP, 2000) {
            Some(body) => {
                let status =
                    u16::from_le_bytes([body[ASSOC_RESP_OFF_STATUS], body[ASSOC_RESP_OFF_STATUS + 1]]);
                let aid =
                    u16::from_le_bytes([body[ASSOC_RESP_OFF_AID], body[ASSOC_RESP_OFF_AID + 1]]) & 0x3fff;
                if status == DOT11_STATUS_SUCCESS {
                    host::print("[ax200] Stage 5d OK — *** ASSOCIATED *** aid=");
                    host::print_dec(aid as u32);
                    host::print("\n");
                    true
                } else {
                    host::print("[ax200] ASSOC rejected: status=");
                    host::print_dec(status as u32);
                    host::print("\n");
                    false
                }
            }
            None => {
                host::print("[ax200] Stage 5d — no ASSOC response\n");
                false
            }
        }
    }

    // ── Resident NIC service loop ─────────────────────────────────
    // The chip is up and the scan has run; register as a network interface and
    // own the card from here. Same shape as aml.wasm: an infinite loop that
    // does the driver's work and yields via npk_sleep — never returns (the
    // driver holds its DMA + the netdev registration for its lifetime). Frame
    // bridging to the kernel netdev mailboxes (TX poll / RX submit) plugs into
    // this loop once association brings up the data path.
    fn run_netdev(&mut self) -> ! {
        let mac = self.mac;
        if host::netdev_register(&mac) == 0 {
            host::print("[ax200] registered as network interface 'wlan' (");
            Self::print_mac(&mac);
            host::print(")\n");
        } else {
            host::print("[ax200] netdev_register failed\n");
        }
        // The data TX queue was allocated before auth (so its SCD-response wait
        // wouldn't swallow the AP's first EAPOL frame). Tell wifid the connection
        // is ready + the MACs it needs for the PTK, then listen immediately.
        let our_mac = self.mac;
        let mut ready = [0u8; 13];
        ready[0] = EV_READY;
        ready[1..7].copy_from_slice(&self.target_bssid);
        ready[7..13].copy_from_slice(&our_mac);
        host::wifi_send_event(&ready);
        host::print("[ax200] associated — READY sent, listening for EAPOL (4-way)\n");
        let mut rxbuf = [0u8; 1600];
        let mut evt = [0u8; 1700];
        let mut cmd = [0u8; 600];
        let mut txbuf = [0u8; 1514];
        loop {
            // RX: drain + recycle the ring. EAPOL-Key frames → wifid (the 4-way);
            // decrypted IP/other data → the kernel IP stack as Ethernet frames.
            self.service_rx(|c, g, rb| {
                if c == REPLY_RX_MPDU_CMD && g == 0 {
                    match Self::rx_classify(rb, &our_mac, &mut rxbuf) {
                        RxKind::Eapol(n) => {
                            evt[0] = EV_EAPOL_RX;
                            evt[1] = (n & 0xff) as u8;
                            evt[2] = (n >> 8) as u8;
                            evt[3..3 + n].copy_from_slice(&rxbuf[..n]);
                            host::wifi_send_event(&evt[..3 + n]);
                        }
                        RxKind::Ip(n) => host::netdev_submit_rx(&rxbuf[..n]),
                        RxKind::None => {}
                    }
                }
                true
            });
            // TX: send every Ethernet frame the IP stack queued (DHCP, ARP, …).
            loop {
                let n = host::netdev_poll_tx(&mut txbuf);
                if n == 0 {
                    break;
                }
                self.tx_eth(&txbuf[..n]);
            }
            // Control commands from wifid (TX_EAPOL / SET_KEY / AUTHORIZED).
            let clen = host::wifi_poll_cmd(&mut cmd);
            if clen > 0 {
                self.handle_wifi_cmd(&cmd[..clen as usize]);
            }
            host::sleep_ms(20);
        }
    }

    // Dispatch one control command from wifid (the supplicant).
    fn handle_wifi_cmd(&mut self, cmd: &[u8]) {
        match cmd.first().copied() {
            // TX_EAPOL: [op][len u16][frame] → unencrypted EAPOL to the AP.
            Some(CMD_TX_EAPOL) if cmd.len() >= 3 => {
                let len = ((cmd[2] as usize) << 8) | cmd[1] as usize;
                if cmd.len() >= 3 + len {
                    host::print("[ax200] TX_EAPOL (len ");
                    host::print_dec(len as u32);
                    host::print(")\n");
                    let dst = self.target_bssid;
                    self.tx_8023(dst, ETHERTYPE_EAPOL, &cmd[3..3 + len], false);
                }
            }
            // SET_KEY: [op][key_type][key_idx][cipher][key_len][key..][rsc 6].
            Some(CMD_SET_KEY) if cmd.len() >= 5 => {
                let key_type = cmd[1]; // 0=PTK/pairwise 1=GTK/group
                let key_idx = cmd[2];
                let key_len = cmd[4] as usize;
                if cmd.len() >= 5 + key_len + 6 {
                    let key = &cmd[5..5 + key_len];
                    let rsc = &cmd[5 + key_len..5 + key_len + 6];
                    self.install_key(key_type == 1, key_idx, key, rsc);
                }
            }
            // AUTHORIZED: 4-way done → carrier up, IP data path live.
            Some(CMD_AUTHORIZED) => {
                host::netdev_set_link(true);
                host::print("[ax200] *** AUTHORIZED *** — link up, data path live 🎉 (run `dhcp`)\n");
                let mut up = [0u8; 7];
                up[0] = EV_LINK_UP;
                up[1..7].copy_from_slice(&self.target_bssid);
                host::wifi_send_event(&up);
            }
            _ => {}
        }
    }

    // ── iwl_pcie_grab_nic_access + iwl_trans_pcie_read_mem ────────
    // Grab NIC access (so device SRAM is reachable) and read `out.len()` words
    // from device memory at `addr` through the HBUS periphery window (the read
    // data register auto-increments). Used only by the error-log dump.
    fn grab_nic_access(&self) -> bool {
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
        for _ in 0..1500 {
            let gp = self.r32(CSR_GP_CNTRL);
            if gp & (CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY
                | CSR_GP_CNTRL_REG_FLAG_GOING_TO_SLEEP)
                == CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY
            {
                return true;
            }
            for _ in 0..64 {
                core::hint::spin_loop();
            }
        }
        false
    }

    fn read_mem(&self, addr: u32, out: &mut [u32]) {
        self.w32(HBUS_TARG_MEM_RADDR, addr);
        for w in out.iter_mut() {
            *w = self.r32(HBUS_TARG_MEM_RDAT);
        }
    }

    // ── iwl_mvm_dump_nic_error_log (mvm/utils.c) ──────────────────
    // Read the lmac + umac error tables from device SRAM. valid != 0 means the
    // firmware asserted; error_id classifies it and hcmd / last_cmd_id /
    // cmd_header name the command the firmware faulted on.
    fn dump_fw_error_log(&self) {
        if self.lmac_err_ptr == 0 || !self.grab_nic_access() {
            host::print("[ax200] err-log: no NIC access\n");
            return;
        }
        let mut l = [0u32; LERR_WORDS];
        self.read_mem(self.lmac_err_ptr, &mut l);
        host::print("[ax200] LMAC err: valid=0x");
        host::print_hex32(l[LERR_VALID]);
        host::print(" id=0x");
        host::print_hex32(l[LERR_ERROR_ID]);
        host::print(" data1=0x");
        host::print_hex32(l[LERR_DATA1]);
        host::print(" data2=0x");
        host::print_hex32(l[LERR_DATA2]);
        host::print(" data3=0x");
        host::print_hex32(l[LERR_DATA3]);
        host::print(" hcmd=0x");
        host::print_hex32(l[LERR_HCMD]);
        host::print(" last_cmd=0x");
        host::print_hex32(l[LERR_LAST_CMD_ID]);
        host::print("\n");

        if self.umac_err_ptr != 0 {
            let mut u = [0u32; UERR_WORDS];
            self.read_mem(self.umac_err_ptr, &mut u);
            host::print("[ax200] UMAC err: valid=0x");
            host::print_hex32(u[UERR_VALID]);
            host::print(" id=0x");
            host::print_hex32(u[UERR_ERROR_ID]);
            host::print(" data1=0x");
            host::print_hex32(u[UERR_DATA1]);
            host::print(" cmd_hdr=0x");
            host::print_hex32(u[UERR_CMD_HEADER]);
            host::print("\n");
        }
        host::mmio_clr32(self.mmio, CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
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

/// iwl_fw_lookup_cmd_ver (fw/img.c): walk the embedded firmware's
/// IWL_UCODE_TLV_CMD_VERSIONS TLV for the (group, cmd) entry and return its
/// cmd_ver. Group 0 maps to LONG_GROUP (the legacy command space). Returns
/// IWL_FW_CMD_VER_UNKNOWN (99) if absent — callers fall back to a default.
fn fw_cmd_ver(group: u8, cmd: u8) -> u8 {
    let grp = if group == 0 { IWL_ALWAYS_LONG_GROUP } else { group };
    let mut off = FW_TLV_HEADER_LEN;
    while off + 8 <= FW.len() {
        let t = le32(FW, off);
        let l = le32(FW, off + 4) as usize;
        let body = off + 8;
        if body + l > FW.len() {
            break;
        }
        if t == IWL_UCODE_TLV_CMD_VERSIONS {
            let n = l / FW_CMD_VER_ENTRY_LEN;
            for i in 0..n {
                let e = body + i * FW_CMD_VER_ENTRY_LEN;
                if FW[e + 1] == grp && FW[e] == cmd {
                    return FW[e + 2]; // cmd_ver (may be IWL_FW_CMD_VER_UNKNOWN)
                }
            }
        }
        off = body + ((l + 3) & !0x3);
    }
    IWL_FW_CMD_VER_UNKNOWN
}

/// fw_has_capa (iwl-drv.c iwl_set_ucode_capabilities): true if the firmware's
/// IWL_UCODE_TLV_ENABLED_CAPABILITIES TLVs set capability bit `cap`. Each such
/// TLV is { __le32 api_index; __le32 api_capa }; bit `cap` lives in the TLV with
/// api_index == cap/32, at position cap%32 in api_capa.
fn fw_has_capa(cap: u32) -> bool {
    let want_index = cap / 32;
    let want_bit = 1u32 << (cap % 32);
    let mut off = FW_TLV_HEADER_LEN;
    while off + 8 <= FW.len() {
        let t = le32(FW, off);
        let l = le32(FW, off + 4) as usize;
        let body = off + 8;
        if body + l > FW.len() {
            break;
        }
        if t == IWL_UCODE_TLV_ENABLED_CAPABILITIES && l >= 8 {
            if le32(FW, body) == want_index && le32(FW, body + 4) & want_bit != 0 {
                return true;
            }
        }
        off = body + ((l + 3) & !0x3);
    }
    false
}

/// Log one command's firmware version (Stage 4d1 diagnostics).
fn log_cmd_ver(name: &str, group: u8, cmd: u8) {
    host::print("[ax200]   ");
    host::print(name);
    host::print(" v=");
    host::print_hex32(fw_cmd_ver(group, cmd) as u32);
    host::print("\n");
}

/// iwl_flip_hw_address: build the 6-byte MAC from the two CSR registers.
/// addr0 holds bytes [3,2,1,0] (high→low), addr1 holds bytes [4,5] in its low
/// half (byte1, byte0). On a little-endian host iwl_read32 + cpu_to_le32 leaves
/// the register value with byte k at (val >> 8*k).
fn mac_from_regs(addr0: u32, addr1: u32) -> [u8; 6] {
    [
        (addr0 >> 24) as u8,
        (addr0 >> 16) as u8,
        (addr0 >> 8) as u8,
        addr0 as u8,
        (addr1 >> 8) as u8,
        addr1 as u8,
    ]
}

/// is_valid_ether_addr: not multicast (bit 0 of first octet clear) and not all-zero.
fn is_valid_mac(mac: &[u8; 6]) -> bool {
    (mac[0] & 0x1) == 0 && mac.iter().any(|&b| b != 0)
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
    host::print("[ax200] Intel Wi-Fi 6 AX200 driver v0.30.0 — IP data path (DHCP)\n");

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
        cmd_data: Dma::NONE,
        cmd_write_ptr: 0,
        rb_pool: [Dma::NONE; RX_NUM_RBS],
        rxq_read: 0,
        free_bd_write: 0,
        lmac_err_ptr: 0,
        umac_err_ptr: 0,
        scan_chans: [0; SCAN_MAX_CHANS],
        scan_bands: [0; SCAN_MAX_CHANS],
        n_scan_chans: 0,
        mac: [0; 6],
        target_bssid: [0; 6],
        target_chan: 0,
        target_band: 0,
        target_beacon_int: 0,
        target_ssid: [0; SSID_MAX],
        target_ssid_len: 0,
        target_privacy: false,
        target_valid: false,
        mgmt_tfd: Dma::NONE,
        mgmt_first_tb: Dma::NONE,
        mgmt_bc_tbl: Dma::NONE,
        mgmt_queue_id: 0,
        mgmt_write_ptr: 0,
        data_tfd: Dma::NONE,
        data_first_tb: Dma::NONE,
        data_bc_tbl: Dma::NONE,
        data_queue_id: 0,
        data_write_ptr: 0,
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
        match dev.rx_restock_and_alive() {
            Some(rb0) => {
                host::print("[ax200] Stage 3 OK — RX path live, alive notification received\n");

                // ── Stage 4a: parse the ALIVE notification struct ──
                if dev.parse_alive_ntf(&rb0) {
                    host::print("[ax200] Stage 4a OK — firmware ALIVE valid (status OK)\n");

                    // ── Stage 4b: init-flow host commands → INIT_COMPLETE ──
                    if dev.run_init_handshake() {
                        host::print("[ax200] Stage 4b OK — *** INIT_COMPLETE_NOTIF received *** 🎉\n");

                        // ── Stage 4c: read NVM info (caps + MAC address) ──
                        if dev.read_nvm() {
                            host::print("[ax200] Stage 4c OK — NVM info read\n");

                            // ── Stage 4d1: firmware command versions (scan path) ──
                            host::print("[ax200] FW cmd versions (scan path):\n");
                            log_cmd_ver("SCAN_REQ_UMAC ", IWL_ALWAYS_LONG_GROUP, SCAN_REQ_UMAC);
                            log_cmd_ver("SCAN_CFG_CMD  ", IWL_ALWAYS_LONG_GROUP, SCAN_CFG_CMD);
                            log_cmd_ver("ADD_STA       ", IWL_ALWAYS_LONG_GROUP, ADD_STA);
                            log_cmd_ver("PHY_CONTEXT   ", IWL_ALWAYS_LONG_GROUP, PHY_CONTEXT_CMD);
                            log_cmd_ver("MAC_CONTEXT   ", IWL_ALWAYS_LONG_GROUP, MAC_CONTEXT_CMD);
                            log_cmd_ver("TX_ANT_CFG    ", IWL_ALWAYS_LONG_GROUP, TX_ANT_CONFIGURATION_CMD);
                            log_cmd_ver("SCAN_COMPLETE ", IWL_ALWAYS_LONG_GROUP, SCAN_COMPLETE_UMAC);
                            host::print("[ax200] Stage 4d1 OK — cmd versions read\n");

                            // ── Stage 4d2a: scan-config prerequisites ──
                            dev.run_scan_prereqs();
                            host::print("[ax200] Stage 4d2a OK — full iwl_mvm_up pre-scan seq sent\n");

                            // ── Stage 4d2b1b: add the MAC context the scan ──
                            // references (scan_start_mac_or_link_id → ctx id 0).
                            dev.add_mac_context();
                            host::print("[ax200] Stage 4d2b1b OK — MAC context added\n");

                            // ── Stage 4d2b1/2: passive scan → SCAN_COMPLETE,
                            // parse beacons → access points (SSID/BSSID/RSSI). ──
                            if dev.run_scan() {
                                host::print("[ax200] Stage 4d2b2 OK — *** SCAN COMPLETE, APs listed *** 🎉\n");

                                // ── Stage 5a: PHY context + RLC + binding ──
                                // First connect step (no TX yet): set the target
                                // AP's operating channel + bind MAC↔PHY. Target =
                                // strongest AP from the scan.
                                if dev.target_valid && dev.connect_phy_binding()
                                    && dev.connect_add_station()
                                {
                                    // ── Stage 5b': power + MAC context (target BSSID) ──
                                    // The chanctx tail Linux runs before the auth TX.
                                    dev.connect_finish_chanctx();
                                    // Allocate the data TX queue BEFORE auth so its
                                    // SCD-response wait doesn't discard the AP's
                                    // first EAPOL frame, and the post-assoc listen
                                    // can begin immediately.
                                    if !dev.alloc_data_queue() {
                                        host::print("[ax200] data queue alloc FAILED\n");
                                    }
                                    // ── Stage 5c/5d: AUTH → ASSOC mgmt dialog ──
                                    if dev.connect_send_auth() {
                                        dev.connect_send_assoc();
                                    }
                                }

                                // ── Register as a NIC + go resident ──
                                // run_netdev never returns: the driver owns the
                                // card and the `wlan` interface for its lifetime
                                // (same model as aml.wasm).
                                dev.run_netdev();
                            } else {
                                host::print("[ax200] Stage 4d2b1 FAILED — no SCAN_COMPLETE\n");
                            }
                        } else {
                            host::print("[ax200] Stage 4c FAILED — no NVM response\n");
                        }
                    } else {
                        host::print("[ax200] Stage 4b FAILED — no INIT_COMPLETE\n");
                    }
                } else {
                    host::print("[ax200] Stage 4a FAILED — firmware ALIVE not valid\n");
                }
            }
            None => host::print("[ax200] Stage 3 FAILED\n"),
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
