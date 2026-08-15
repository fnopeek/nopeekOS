//! AX200 register definitions — verified 1:1 against Linux 6.18.26
//! `drivers/net/wireless/intel/iwlwifi/iwl-csr.h` (CSR_BASE = 0x000).
//!
//! Only offsets verified against the header live here. Bit masks are added
//! per stage as each function is ported (strict 1:1, no guessed values —
//! see memory/feedback_linux_strict.md). BAR0 carries the CSR block.

// ── PCI identity ─────────────────────────────────────────────────
pub const AX200_VENDOR: u16 = 0x8086;
pub const AX200_DEVICE: u16 = 0x2723; // iwl_ax200_mac_cfg, RF = HR, family 22000

// BAR carrying the CSR/PRPH register block (iwlwifi: BAR0).
pub const BAR_CSR: u8 = 0;

// ── CSR registers (iwl-csr.h, CSR_BASE = 0x000) ──────────────────
pub const CSR_HW_IF_CONFIG_REG: u32 = 0x000; // hardware interface config
pub const CSR_INT: u32 = 0x008; // host interrupt status/ack
pub const CSR_INT_MASK: u32 = 0x00C; // host interrupt enable
pub const CSR_FH_INT_STATUS: u32 = 0x010; // busmaster int status/ack
pub const CSR_RESET: u32 = 0x020; // busmaster enable, NMI, etc.
pub const CSR_GP_CNTRL: u32 = 0x024;
pub const CSR_HW_REV: u32 = 0x028;
pub const CSR_FUNC_SCRATCH: u32 = 0x02C; // FW debug scratch
pub const CSR_GIO_REG: u32 = 0x03C;
pub const CSR_UCODE_DRV_GP1: u32 = 0x054;
pub const CSR_MBOX_SET_REG: u32 = 0x088;
pub const CSR_HW_RF_ID: u32 = 0x09C;
pub const CSR_MAC_SHADOW_REG_CTRL: u32 = 0x0A8;
pub const CSR_GIO_CHICKEN_BITS: u32 = 0x100;
pub const CSR_DBG_HPET_MEM_REG: u32 = 0x240;
pub const CSR_DBG_LINK_PWR_MGMT_REG: u32 = 0x250;

// ── CSR bit masks (iwl-csr.h, verified) ──────────────────────────
// HW_IF_CONFIG
pub const CSR_HW_IF_CONFIG_REG_HAP_WAKE: u32 = 0x0008_0000;
pub const CSR_HW_IF_CONFIG_REG_PCI_OWN_SET: u32 = 0x0040_0000;
pub const CSR_HW_IF_CONFIG_REG_WAKE_ME: u32 = 0x0800_0000;
// MBOX_SET
pub const CSR_MBOX_SET_REG_OS_ALIVE: u32 = 0x0000_0020; // BIT(5)
// RESET
pub const CSR_RESET_REG_FLAG_SW_RESET: u32 = 0x0000_0080;
pub const CSR_RESET_LINK_PWR_MGMT_DISABLED: u32 = 0x8000_0000;
// GP_CNTRL
pub const CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY: u32 = 0x0000_0001;
pub const CSR_GP_CNTRL_REG_FLAG_INIT_DONE: u32 = 0x0000_0004;
// GIO_CHICKEN / GIO / DBG_HPET
pub const CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX: u32 = 0x0080_0000;
pub const CSR_GIO_REG_VAL_L0S_DISABLED: u32 = 0x0000_0002;
pub const CSR_DBG_HPET_MEM_REG_VAL: u32 = 0xFFFF_0000;
// HW_RF_ID type (masked compare) — AX200 carries HR
pub const CSR_HW_RF_ID_TYPE_HR: u32 = 0x0010_A000;

// ── PRPH access via HBUS (iwl-csr.h, HBUS_BASE = 0x400) ───────────
pub const HBUS_TARG_PRPH_WADDR: u32 = 0x444;
pub const HBUS_TARG_PRPH_RADDR: u32 = 0x448;
pub const HBUS_TARG_PRPH_WDAT: u32 = 0x44C;
pub const HBUS_TARG_PRPH_RDAT: u32 = 0x450;
// PRPH address mask for family < AX210 (iwl_trans_pcie_prph_msk)
pub const PRPH_MASK: u32 = 0x000F_FFFF;

// ── PRPH registers / bits (iwl-prph.h) ───────────────────────────
pub const HPM_DEBUG: u32 = 0x00A0_3440;
pub const PERSISTENCE_BIT: u32 = 0x0000_1000; // BIT(12)
pub const PREG_PRPH_WPROT_22000: u32 = 0x00A0_4D00;
pub const PREG_WFPM_ACCESS: u32 = 0x0000_1000; // BIT(12)

// ── Poll timeouts (iwl-io.c / trans.c, microseconds) ─────────────
pub const HW_READY_TIMEOUT_US: u32 = 50;
pub const MAC_CLOCK_TIMEOUT_US: u32 = 25_000;

// ── Stage 1: RX/TX rings (iwl-csr.h / iwl-fh.h / fw/api/txq.h) ────
pub const CSR_INT_COALESCING: u32 = 0x004; // 32-usec units, u8 write
pub const IWL_HOST_INT_TIMEOUT_DEF: u8 = 0x40;
// Shadow-register enable mask written to CSR_MAC_SHADOW_REG_CTRL.
pub const CSR_MAC_SHADOW_REG_CTRL_VAL: u32 = 0x800F_FFFF;

// RX ring geometry. AX200 = mq_rx, family 22000 (< AX210), RF = HR.
pub const NUM_RBDS: usize = 256 * 8; // IWL_NUM_RBDS_HE (rf-hr.c)
pub const FREE_BD_SIZE: usize = 8; // __le64 RBD (mq, < AX210)
pub const USED_BD_SIZE: usize = 4; // __le32 (< AX210, < BZ)
pub const RB_STTS_SIZE: usize = 12; // sizeof(struct iwl_rb_status)

// TX command queue geometry (gen2).
pub const IWL_CMD_QUEUE_SIZE: usize = 32; // fw/api/txq.h
pub const TFH_TFD_SIZE: usize = 256; // sizeof(struct iwl_tfh_tfd)
pub const IWL_FIRST_TB_SIZE_ALIGN: usize = 64; // ALIGN(20, 64)

// ── Stage 2: context-info + FW load + ALIVE ──────────────────────
pub const CSR_CTXT_INFO_BA: u32 = 0x040; // 64-bit ctxt_info base address (kick)
pub const CSR_UCODE_DRV_GP1_CLR: u32 = 0x05C;
pub const CSR_UCODE_SW_BIT_RFKILL: u32 = 0x0000_0002;
pub const CSR_UCODE_DRV_GP1_BIT_CMD_BLOCKED: u32 = 0x0000_0004;
pub const CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW: u32 = 0x0800_0000;

// CSR_INT cause bits (iwl-csr.h)
pub const CSR_INT_BIT_ALIVE: u32 = 1 << 0; // uCode initialised
pub const CSR_INT_BIT_FH_RX: u32 = 1 << 31; // Rx DMA / cmd responses

// LTR boot workaround (iwl_pcie_set_ltr, 22000 non-integrated path)
pub const CSR_LTR_LONG_VAL_AD: u32 = 0x0D4;
pub const CSR_LTR_LONG_VAL_AD_NO_SNOOP_REQ: u32 = 0x8000_0000;
pub const CSR_LTR_LONG_VAL_AD_NO_SNOOP_SCALE: u32 = 0x1c00_0000;
pub const CSR_LTR_LONG_VAL_AD_NO_SNOOP_VAL: u32 = 0x03ff_0000;
pub const CSR_LTR_LONG_VAL_AD_SNOOP_REQ: u32 = 0x0000_8000;
pub const CSR_LTR_LONG_VAL_AD_SNOOP_SCALE: u32 = 0x0000_1c00;
pub const CSR_LTR_LONG_VAL_AD_SNOOP_VAL: u32 = 0x0000_03ff;
pub const CSR_LTR_LONG_VAL_AD_SCALE_USEC: u32 = 2;

// PRPH: tell the FW CPU to run (iwl-prph.h)
pub const UREG_CPU_INIT_RUN: u32 = 0x00A0_5C44;

// ── Firmware TLV format (fw/file.h) ──────────────────────────────
pub const FW_TLV_HEADER_LEN: usize = 88; // iwl_tlv_ucode_header
pub const IWL_UCODE_TLV_SEC_RT: u32 = 19; // regular runtime section
pub const CPU1_CPU2_SEPARATOR_SECTION: u32 = 0xFFFF_CCCC;
pub const PAGING_SEPARATOR_SECTION: u32 = 0xAAAA_BBBB;
pub const IWL_MAX_DRAM_ENTRY: usize = 64;

// ── Context-info struct (iwl-context-info.h), packed, 1792 bytes ──
pub const CTXT_INFO_SIZE: usize = 1792;
pub const CI_OFF_MAC_ID: usize = 0; // version.mac_id (u16)
pub const CI_OFF_VERSION: usize = 2; // version.version (u16)
pub const CI_OFF_SIZE: usize = 4; // version.size (u16, DWs)
pub const CI_OFF_CONTROL_FLAGS: usize = 8; // control.control_flags (u32)
pub const CI_OFF_FREE_RBD: usize = 24; // rbd_cfg.free_rbd_addr (u64)
pub const CI_OFF_USED_RBD: usize = 32; // rbd_cfg.used_rbd_addr (u64)
pub const CI_OFF_STATUS_WR: usize = 40; // rbd_cfg.status_wr_ptr (u64)
pub const CI_OFF_CMD_QUEUE_ADDR: usize = 48; // hcmd_cfg.cmd_queue_addr (u64)
pub const CI_OFF_CMD_QUEUE_SIZE: usize = 56; // hcmd_cfg.cmd_queue_size (u8)
pub const CI_OFF_UMAC_IMG: usize = 192; // dram.umac_img[64] (u64 each)
pub const CI_OFF_LMAC_IMG: usize = 704; // dram.lmac_img[64]
pub const CI_OFF_VIRTUAL_IMG: usize = 1216; // dram.virtual_img[64]

// control_flags fields (iwl_context_info_flags)
pub const IWL_CTXT_INFO_TFD_FORMAT_LONG: u32 = 0x0100;
pub const IWL_CTXT_INFO_RB_CB_SIZE_SHIFT: u32 = 4; // mask 0x00f0
pub const IWL_CTXT_INFO_RB_SIZE_SHIFT: u32 = 9; // mask 0x1e00
pub const IWL_CTXT_INFO_RB_SIZE_4K: u32 = 0x4; // default rx_buf_size
pub const CMD_QUEUE_CB_SIZE: u8 = 2; // TFD_QUEUE_CB_SIZE(32) = ilog2(32)-3

// ── Stage 3: RX restock + ALIVE notification ─────────────────────
// RFH free-RBD write-pointer trigger (direct MMIO in BAR0, gen2 < BZ).
pub const RFH_Q0_FRBDCB_WIDX_TRG: u32 = 0x1C80;
// RB pool size: enough to receive the alive notification (kept small to
// stay under MAX_DMA_ALLOCS; no npk_dma_free in the ABI). Each RB = 1 page.
pub const RX_NUM_RBS: usize = 64;
pub const RB_SIZE_BYTES: usize = 4096; // IWL_AMSDU_4K
// rb_stts.closed_rb_num producer index mask.
pub const RB_STTS_CLOSED_MASK: u32 = 0x0FFF;
// UCODE_ALIVE_NTFY command id (group 0).
pub const UCODE_ALIVE_NTFY: u8 = 0x01;

// ── Stage 4a: ALIVE notification struct (fw/api/alive.h) ─────────
// The firmware DMAs a `struct iwl_alive_ntf_v6` (144 bytes) as the payload
// of an iwl_rx_packet. data[] starts after len_n_flags(4) + iwl_cmd_header(4).
pub const RX_PKT_DATA_OFF: usize = 8;
pub const IWL_ALIVE_STATUS_OK: u16 = 0xCAFE;
pub const IWL_ALIVE_STATUS_ERR: u16 = 0xDEAD;
// error_info_addr carries cache-control bits that must be masked off
// (fw/img.h: FW_ADDR_CACHE_CONTROL).
pub const FW_ADDR_CACHE_CONTROL: u32 = 0xC000_0000;
// Field offsets within iwl_alive_ntf_v6 (relative to the struct base):
//   __le16 status; __le16 flags; iwl_lmac_alive lmac_data[2];
//   iwl_umac_alive umac_data; iwl_sku_id sku_id; iwl_imr_alive_info imr;
pub const AL_OFF_STATUS: usize = 0; // __le16
pub const AL_OFF_LMAC0: usize = 4; // iwl_lmac_alive[0] (48 bytes)
pub const AL_OFF_UMAC: usize = 100; // 4 + 2*48
pub const AL_OFF_SKU_ID: usize = 116; // 4 + 2*48 + 16
// Within iwl_lmac_alive (48 bytes):
pub const LMAC_OFF_UCODE_MAJOR: usize = 0; // __le32
pub const LMAC_OFF_UCODE_MINOR: usize = 4; // __le32
pub const LMAC_OFF_VER_SUBTYPE: usize = 8; // u8
pub const LMAC_OFF_VER_TYPE: usize = 9; // u8
pub const LMAC_OFF_ERR_TABLE: usize = 16; // dbg_ptrs.error_event_table_ptr
// Within iwl_umac_alive (16 bytes):
pub const UMAC_OFF_MAJOR: usize = 0; // __le32
pub const UMAC_OFF_MINOR: usize = 4; // __le32
pub const UMAC_OFF_ERR_INFO: usize = 8; // dbg_ptrs.error_info_addr

// ── Stage 4b: host-command enqueue + init handshake ──────────────
// TX command-queue write-pointer doorbell (iwl-csr.h, HBUS_BASE 0x400 + 0x60).
pub const HBUS_TARG_WRPTR: u32 = 0x460;
// Command queue id = IWL_MVM_DQA_CMD_QUEUE (BUILD_BUG_ON forces it to 0).
pub const IWL_CMD_QUEUE_ID: u32 = 0;
// iwl_cmd_header_wide (fw/api/cmdhdr.h): cmd, group_id, __le16 sequence,
// __le16 length, reserved, version = 8 bytes.
pub const CMD_HDR_WIDE_LEN: usize = 8;
// IWL_FIRST_TB_SIZE (iwl-trans.h) — minimum bidirectional-DMA copy size; our
// whole init command fits within it (single TB).
pub const IWL_FIRST_TB_SIZE: usize = 20;
pub const HDRW_OFF_CMD: usize = 0;
pub const HDRW_OFF_GROUP: usize = 1;
pub const HDRW_OFF_SEQ: usize = 2; // __le16
pub const HDRW_OFF_LEN: usize = 4; // __le16 (payload length)
// iwl_txq_inc_wrap wraps at max_tfd_queue_size (TFD_QUEUE_SIZE_MAX = 256).
pub const MAX_TFD_QUEUE_SIZE: u32 = 256;
// iwl_tfh_tb: tb_len(__le16) + addr(__le64, unaligned) = 10 bytes.
pub const TFH_TB_LEN: usize = 10;
// Max payload bytes that ride in the first-TB staging buffer alongside the
// 8-byte wide header (IWL_FIRST_TB_SIZE - CMD_HDR_WIDE_LEN = 20 - 8).
pub const FIRST_TB_HEAD_MAX: usize = IWL_FIRST_TB_SIZE - CMD_HDR_WIDE_LEN;
// cmd_data buffer: holds the bulk of a large host command (mapped as TB1).
// One page covers SCAN_REQ_UMAC (~1.7 KB).
pub const CMD_DATA_BYTES: usize = 4096;

// Command groups (fw/api/commands.h iwl_mvm_command_groups).
pub const LONG_GROUP: u8 = 0x1;
pub const SYSTEM_GROUP: u8 = 0x2;
pub const REGULATORY_AND_NVM_GROUP: u8 = 0xc;
// Init-flow command ids.
pub const INIT_EXTENDED_CFG_CMD: u8 = 0x03; // SYSTEM_GROUP (fw/api/commands.h)
pub const NVM_ACCESS_COMPLETE: u8 = 0x00; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)
pub const INIT_COMPLETE_NOTIF: u8 = 0x04; // legacy group 0 (commands.h)
// iwl_init_extended_cfg_cmd.init_flags = BIT(IWL_INIT_NVM); IWL_INIT_NVM = 1.
pub const IWL_INIT_NVM_FLAG: u32 = 1 << 1;

// RX used_bd entry is a bare __le32 for family < AX210; low 12 bits = VID.
pub const RX_VID_MASK: u32 = 0x0FFF;

// ── Stage 4c: NVM_GET_INFO (read channels/MAC/SKU) ───────────────
pub const NVM_GET_INFO: u8 = 0x02; // REGULATORY_AND_NVM_GROUP (nvm-reg.h)
// iwl_rx_packet frame-size field (iwl-trans.h FH_RSCSR_FRAME_SIZE_MSK).
pub const FH_FRAME_SIZE_MASK: u32 = 0x0000_3FFF;
// MAC-address CSR registers. CSR_ADDR_BASE = base->mac_addr_from_csr = 0x380
// for family 22000 (cfg/22000.c). STRAP is the OEM-fused address; if invalid,
// fall back to the OTP address (iwl_set_hw_address_from_csr).
pub const CSR_MAC_ADDR0_STRAP: u32 = 0x388;
pub const CSR_MAC_ADDR1_STRAP: u32 = 0x38C;
pub const CSR_MAC_ADDR0_OTP: u32 = 0x380;
pub const CSR_MAC_ADDR1_OTP: u32 = 0x384;
// iwl_nvm_get_info_rsp field offsets (within the response payload). The
// general/mac_sku/phy_sku part is identical in the v3 and v4 responses.
pub const NVM_OFF_FLAGS: usize = 0; // general.flags __le32
pub const NVM_OFF_VERSION: usize = 4; // general.nvm_version __le16
pub const NVM_OFF_N_HW_ADDRS: usize = 7; // general.n_hw_addrs u8
pub const NVM_OFF_MAC_SKU: usize = 8; // mac_sku.mac_sku_flags __le32
pub const NVM_OFF_TX_CHAINS: usize = 12; // phy_sku.tx_chains __le32
pub const NVM_OFF_RX_CHAINS: usize = 16; // phy_sku.rx_chains __le32
pub const NVM_OFF_LAR: usize = 20; // regulatory.lar_enabled __le32
// mac_sku_flags bits (enum iwl_nvm_mac_sku_flags).
pub const NVM_SKU_BAND_24: u32 = 1 << 0;
pub const NVM_SKU_BAND_52: u32 = 1 << 1;
pub const NVM_SKU_11N: u32 = 1 << 2;
pub const NVM_SKU_11AC: u32 = 1 << 3;
pub const NVM_SKU_11AX: u32 = 1 << 4;

// NVM_GET_INFO v4 regulatory section: per-channel flags. lar_enabled @20,
// n_channels @24, then channel_profile (__le32 per channel) @28 within the
// response payload (iwl_nvm_get_info_rsp, REGULATORY_NVM_GET_INFO_RSP_API_S_VER_4).
pub const NVM_OFF_CHANNEL_PROFILE: usize = 28;
// iwl_nvm_channel_flags (iwl-nvm-parse.h): bit 0 = usable for this SKU/geo.
pub const NVM_CHANNEL_VALID: u32 = 1 << 0;
// AX200 is RF-HR → cfg.nvm_type = IWL_NVM_EXT, not uhb → iwl_ext_nvm_channels:
// 14 × 2.4 GHz then 37 × 5 GHz (iwl_nl80211_band_from_channel_idx splits at 14).
pub const NVM_NUM_2GHZ: usize = 14;
pub const NVM_EXT_NUM_CHANNELS: usize = 51;
pub const IWL_EXT_NVM_CHANNELS: [u8; NVM_EXT_NUM_CHANNELS] = [
    // 2.4 GHz
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
    // 5 GHz
    36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92,
    96, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
    149, 153, 157, 161, 165, 169, 173, 177, 181,
];

// ── Stage 4d: scan setup ─────────────────────────────────────────
// Firmware command-version TLV (fw/file.h): array of iwl_fw_cmd_version
// { u8 cmd; u8 group; u8 cmd_ver; u8 notif_ver; }. iwl_fw_lookup_cmd_ver maps
// (group ?: LONG_GROUP, cmd) → cmd_ver; this picks the versioned struct layouts
// the scan path needs (SCAN_REQ_UMAC etc. range over v1..v17).
pub const IWL_UCODE_TLV_CMD_VERSIONS: u32 = 48;
pub const IWL_FW_CMD_VER_UNKNOWN: u8 = 99;
pub const IWL_ALWAYS_LONG_GROUP: u8 = 1;
pub const FW_CMD_VER_ENTRY_LEN: usize = 4; // sizeof(iwl_fw_cmd_version)
// Command ids (fw/api/commands.h) for the scan path.
pub const PHY_CONTEXT_CMD: u8 = 0x08;
pub const SCAN_CFG_CMD: u8 = 0x0c;
pub const SCAN_REQ_UMAC: u8 = 0x0d;
pub const SCAN_COMPLETE_UMAC: u8 = 0x0f;
pub const ADD_STA: u8 = 0x18;
pub const MAC_CONTEXT_CMD: u8 = 0x28;
pub const TX_ANT_CONFIGURATION_CMD: u8 = 0x98;
pub const SCAN_ITERATION_COMPLETE_UMAC: u8 = 0xb5;
// Valid antenna mask (chains A+B). iwl_mvm_get_valid_tx_ant / iwl_mvm_scan_rx_ant
// resolve to fw->valid_tx/rx_ant & nvm = 0x3 on this 2×2 AX200 (Stage 4c caps).
pub const ANT_AB: u32 = 0x3;

// ── Stage 4d2b: SCAN_REQ_UMAC v17 (passive regular scan) ─────────
// Total size of iwl_scan_req_umac_v17 (uid 4 + ooc 4 + general 36 + channel
// 4+67*8 + periodic 12 + probe 1344). probe_params stays zeroed (passive scan
// transmits no probe request, so its preq is unused).
pub const SCAN_CMD_LEN: usize = 1940;
// iwl_scan_req_umac_v17 field offsets:
pub const SC_OFF_OOC_PRIORITY: usize = 4; // uid @ 0 stays 0
// general_params_v11 @ 8 (36 bytes):
pub const SC_OFF_GP_FLAGS: usize = 8; // __le16
pub const SC_OFF_GP_ACTIVE_DWELL: usize = 12; // u8[2] (LB, HB)
pub const SC_OFF_GP_ADWELL_2G: usize = 14;
pub const SC_OFF_GP_ADWELL_5G: usize = 15;
pub const SC_OFF_GP_ADWELL_SOCIAL: usize = 16;
pub const SC_OFF_GP_FLAGS2: usize = 17; // 0 (non-cdb, regular)
pub const SC_OFF_GP_ADWELL_BUDGET: usize = 18; // __le16
// max_out_of_time[2] @ 20, suspend_time[2] @ 28 — all 0 for UNASSOC type.
pub const SC_OFF_GP_SCAN_PRIO: usize = 36; // __le32
pub const SC_OFF_GP_PASSIVE_DWELL: usize = 40; // u8[2]
// num_of_fragments[2] @ 42 — 0 (not fragmented).
// channel_params_v7 @ 44:
pub const SC_OFF_CP_FLAGS: usize = 44; // u8
pub const SC_OFF_CP_COUNT: usize = 45; // u8
pub const SC_OFF_CP_N_APS_OVERRIDE: usize = 46; // u8[2]
pub const SC_OFF_CP_CHANNELS: usize = 48; // iwl_scan_channel_cfg_umac[67]
pub const SCAN_CH_CFG_LEN: usize = 8; // {__le32 flags, u8 channel_num, union[3]}
// periodic_params_v1 @ 584: schedule[2] (4 B each), delay, reserved.
pub const SC_OFF_PERIODIC_SCHED0_ITER: usize = 584 + 2; // schedule[0].iter_count

// Values (mvm/scan.c):
pub const SCAN_OOC_PRIORITY_REGULAR: u32 = 6; // IWL_SCAN_PRIORITY_EXT_6
// gen flags v2: FORCE_PASSIVE BIT(11) | PASS_ALL BIT(1) | ADAPTIVE_DWELL BIT(7).
pub const SCAN_GP_FLAGS_PASSIVE: u16 = (1 << 11) | (1 << 1) | (1 << 7);
pub const IWL_SCAN_DWELL_ACTIVE: u8 = 10;
pub const IWL_SCAN_DWELL_PASSIVE: u8 = 110;
pub const ADWELL_DEFAULT_LB_N_APS: u8 = 2;
pub const ADWELL_DEFAULT_HB_N_APS: u8 = 8;
pub const ADWELL_DEFAULT_N_APS_SOCIAL: u8 = 10;
pub const ADWELL_MAX_BUDGET_FULL: u16 = 300;
pub const SCAN_N_APS_GO_FRIENDLY: u8 = 10;
pub const SCAN_N_APS_SOCIAL_CHS: u8 = 2;
pub const SCAN_CHAN_FLAG_ENABLE_CHAN_ORDER: u8 = 1 << 5; // BIT(5)
pub const PHY_BAND_24: u32 = 1; // phy-ctxt.h
pub const PHY_BAND_5: u32 = 0; // phy-ctxt.h
pub const CHAN_CFG_FLAGS_BAND_POS: u32 = 30;
// Upper bound on channels we put in one scan: command holds SCAN_MAX_NUM_CHANS_V3.
pub const SCAN_MAX_CHANS: usize = 67;
// iwl_scan_config (SCAN_CFG v5, fw/api/scan.h): enable_cam_mode, enable_promisc,
// bcast_sta_id, reserved (all u8 → 0; v5 FW ignores bcast_sta_id), then
// __le32 tx_chains, __le32 rx_chains. 12 bytes total.
pub const SCAN_CFG_LEN: usize = 12;
pub const SCAN_CFG_OFF_TX_CHAINS: usize = 4;
pub const SCAN_CFG_OFF_RX_CHAINS: usize = 8;
// general_params.scan_start_mac_or_link_id (u8 @ general+3 → cmd offset 11).
// It must name the firmware MAC context the scan runs on. For SCAN_REQ_UMAC
// version < 16 this is scan_vif->id (mvm/scan.c iwl_mvm_scan_umac_fill_general_
// p_v12); our station vif gets MAC id 0 (see Stage 4d2b1b below).
pub const SC_OFF_GP_SCAN_START_MAC: usize = 11;

// ── Stage 4d2b1b: MAC context (iwl_mac_ctx_cmd, fw/api/mac.h) ─────
// A scan references a firmware MAC context (scan_start_mac_or_link_id).
// mac80211 creates it on add_interface (iwl_mvm_mac_ctxt_add); our driver-
// initiated scan must add it first. NOTE: no aux station / TX queue is added
// — for ADD_STA cmd_ver >= 12 (this firmware) iwl_mvm_has_new_station_api is
// true, so iwl_mvm_up does NOT call iwl_mvm_add_aux_sta; the firmware uses an
// internal aux station for scan activity (mvm/fw.c:1459, mvm/mvm.h:1509).
// MAC_CONTEXT_CMD is opcode 0x28 in the legacy group (group 0).
pub const MAC_CONTEXT_CMD_OP: u8 = 0x28;
// sizeof(struct iwl_mac_ctx_cmd): 100-byte common+qos+ac[AC_NUM+1=5] + a
// 48-byte union (largest member p2p_sta). We fill only the 44-byte .sta.
pub const MAC_CTX_CMD_LEN: usize = 148;
pub const FW_CTXT_ACTION_ADD: u32 = 1; // enum iwl_ctxt_action (INVALID=0,ADD=1)
pub const FW_CTXT_ACTION_MODIFY: u32 = 2; // enum iwl_ctxt_action (MODIFY=2)
pub const FW_MAC_TYPE_BSS_STA: u32 = 5; // enum iwl_mac_types
pub const SCAN_VIF_MAC_ID: u8 = 0; // ctxt_init id for the 1st non-p2p station
// iwl_mac_ctx_cmd field offsets (packed):
pub const MC_OFF_ID_COLOR: usize = 0; // __le32 FW_CMD_ID_AND_COLOR(0,0)=0
pub const MC_OFF_ACTION: usize = 4; // __le32
pub const MC_OFF_MAC_TYPE: usize = 8; // __le32
pub const MC_OFF_TSF_ID: usize = 12; // __le32 (TSF_ID_A = 0)
pub const MC_OFF_NODE_ADDR: usize = 16; // u8[6] (+ __le16 reserved @ 22)
pub const MC_OFF_BSSID_ADDR: usize = 24; // u8[6] (+ __le16 reserved @ 30)
pub const MC_OFF_CCK_RATES: usize = 32; // __le32
pub const MC_OFF_OFDM_RATES: usize = 36; // __le32
pub const MC_OFF_PROT_FLAGS: usize = 40; // __le32 (0, unassociated)
pub const MC_OFF_FILTER_FLAGS: usize = 52; // __le32
// cck_short_preamble @ 44, short_slot @ 48, qos_flags @ 56, ac[5] @ 60 — all 0.
// union iwl_mac_data_sta @ 100 (after qos_flags @56 + ac[AC_NUM+1=5]*8 = 40).
// For the connect MODIFY (iwl_mvm_mac_ctxt_cmd_sta, unassoc branch) we set the
// timing fields the firmware needs to schedule the auth/assoc on the target BSS.
pub const MC_OFF_STA_IS_ASSOC: usize = 100; // __le32 (0 = not yet associated)
pub const MC_OFF_STA_DTIM_TIME: usize = 104; // __le32 (device time of next DTIM)
pub const MC_OFF_STA_DTIM_TSF: usize = 108;  // __le64 (AP TSF of next DTIM)
pub const MC_OFF_STA_BI: usize = 116;        // __le32 beacon interval (TU)
pub const MC_OFF_STA_DTIM_INTERVAL: usize = 124; // __le32 (bi * dtim_period)
pub const MC_OFF_STA_LISTEN_INTERVAL: usize = 132; // __le32
pub const MC_OFF_STA_ASSOC_ID: usize = 136;  // __le32 (aid; 0 before assoc)
pub const MC_OFF_STA_BEACON_ARRIVE: usize = 140; // __le32 assoc_beacon_arrive_time
// filter flags (enum iwl_mac_filter_flags): accept multicast + foreign beacons.
pub const MAC_FILTER_ACCEPT_GRP: u32 = 1 << 2;
pub const MAC_FILTER_IN_BEACON: u32 = 1 << 6;
// Default basic-rate ACK bitmaps for an empty BSSBasicRateSet (iwl_mvm_ack_rates
// with basic_rates == 0): mandatory CCK 1/2/5.5/11 = 0x0F, OFDM 6/12/24 = 0x15.
pub const MAC_CCK_RATES_DEFAULT: u32 = 0x0F;
pub const MAC_OFDM_RATES_DEFAULT: u32 = 0x15;

// ── Stage 4d2a': regulatory domain (MCC_UPDATE_CMD, fw/api/nvm-reg.h) ──
// With LAR enabled (our NVM reported lar=1) the firmware refuses to scan until
// the regulatory domain is set: "Disallow scans that might crash the FW while
// the LAR regdomain is not set" (mvm/nvm.c iwl_mvm_init_mcc). iwl_mvm_up calls
// init_mcc right before config_scan. The initial update queries the FW's own
// default with alpha2 "ZZ" and source GET_CURRENT (mvm/mac80211.c
// iwl_mvm_get_current_regdomain → iwl_mvm_update_mcc); the FW replies with its
// chosen MCC + channel profile, after which scans are allowed. MCC_UPDATE_CMD
// is opcode 0xc8 in the legacy group and carries CMD_WANT_SKB (it responds).
pub const MCC_UPDATE_CMD: u8 = 0xc8;
// struct iwl_mcc_update_cmd: __le16 mcc, u8 source_id, u8 reserved, __le32 key,
// u8 reserved2[20] = 28 bytes.
pub const MCC_UPDATE_CMD_LEN: usize = 28;
pub const MCC_OFF_MCC: usize = 0; // __le16 (alpha2[0] << 8 | alpha2[1])
pub const MCC_OFF_SOURCE: usize = 2; // u8 source_id
pub const MCC_ALPHA2_ZZ: u16 = ((b'Z' as u16) << 8) | (b'Z' as u16); // 0x5A5A
pub const MCC_SOURCE_GET_CURRENT: u8 = 0x10; // enum iwl_mcc_source
// iwl_mcc_update_resp_v8 payload offsets (relative to the rx packet data[]):
// __le32 status, __le16 mcc, ... __le32 n_channels @ 20.
pub const MCC_RESP_OFF_STATUS: usize = 0;
pub const MCC_RESP_OFF_MCC: usize = 4;
pub const MCC_RESP_OFF_N_CHANNELS: usize = 20;

// ── Stage 4d2b1d: the rest of the mandatory iwl_mvm_up pre-scan cmds ──
// The full sequence iwl_mvm_up runs between ALIVE and config_scan, faithfully:
// TX_ANT → BT coex → SOC latency → DQA → device power → MCC → SCAN_CFG.
// configure_rxq + rss_cfg are no-ops for num_rxqs==1 (both return early); the
// BIOS/ACPI-gated cmds (lari/ppag/sar/sgom/tas) send nothing without platform
// tables (e.g. ppag !approved → return 0, sgom !enabled → return 0), exactly
// as Linux on a machine without them; the post-config_scan tuning isn't a scan
// prerequisite. The cap-gated ones below check the firmware capability bitmap.
pub const DATA_PATH_GROUP: u8 = 0x5; // fw/api/commands.h (SYSTEM_GROUP = 0x2 above)

// iwl_mvm_send_bt_init_conf (mvm/coex.c): BT_CONFIG, legacy group, struct
// iwl_bt_coex_cmd { __le32 mode; __le32 enabled_modules } = 8 bytes.
pub const BT_CONFIG: u8 = 0x9b;
pub const BT_COEX_CMD_LEN: usize = 8;
pub const BT_COEX_NW: u32 = 0x1; // enum: mode = network-coexistence
pub const BT_COEX_SYNC2SCO_ENABLED: u32 = 1 << 2; // IWL_MVM_BT_COEX_SYNC2SCO=1 → always
pub const BT_COEX_MPLUT_ENABLED: u32 = 1 << 0; // only if BT_MPLUT_SUPPORT cap
pub const BT_COEX_HIGH_BAND_RET: u32 = 1 << 4; // always

// iwl_set_soc_latency (fw/init.c): SOC_CONFIGURATION_CMD in SYSTEM_GROUP, struct
// iwl_soc_configuration_cmd { __le32 flags; __le32 latency } = 8 bytes. AX200 is
// a discrete card (mac_cfg.integrated unset) → flags = DISCRETE, latency = 0.
// Gated by the SOC_LATENCY_SUPPORT capability.
pub const SOC_CONFIGURATION_CMD: u8 = 0x01;
pub const SOC_CONFIG_CMD_LEN: usize = 8;
pub const SOC_CONFIG_CMD_FLAGS_DISCRETE: u32 = 1 << 0;

// iwl_mvm_send_dqa_cmd (mvm/fw.c): DQA_ENABLE_CMD in DATA_PATH_GROUP, struct
// iwl_dqa_enable_cmd { __le32 cmd_queue } = 4 bytes. cmd_queue = the command
// queue id (IWL_MVM_DQA_CMD_QUEUE = 0 = IWL_CMD_QUEUE_ID). Gated by DQA_SUPPORT.
pub const DQA_ENABLE_CMD: u8 = 0x0;
pub const DQA_ENABLE_CMD_LEN: usize = 4;

// iwl_mvm_power_update_device (mvm/power.c): POWER_TABLE_CMD, legacy group,
// struct iwl_device_power_cmd { __le16 flags; __le16 reserved } = 4 bytes. The
// default power scheme is BPS (not CAM) so power-save is enabled.
pub const POWER_TABLE_CMD: u8 = 0x77;
pub const DEVICE_POWER_CMD_LEN: usize = 4;
pub const DEVICE_POWER_FLAGS_POWER_SAVE_ENA: u16 = 1 << 0;

// FW capability bitmap (IWL_UCODE_TLV_ENABLED_CAPABILITIES = 30, fw/file.h).
// Each such TLV is struct iwl_ucode_capa { __le32 api_index; __le32 api_capa };
// capability bit N is set iff a TLV has api_index == N/32 and bit (N%32) in
// api_capa (iwl-drv.c iwl_set_ucode_capabilities). Bit numbers from fw/file.h.
pub const IWL_UCODE_TLV_ENABLED_CAPABILITIES: u32 = 30;
pub const CAPA_DQA_SUPPORT: u32 = 12;
pub const CAPA_SOC_LATENCY_SUPPORT: u32 = 37;
pub const CAPA_BT_MPLUT_SUPPORT: u32 = 67;

// ── FW error-log dump (iwl_mvm_dump_nic_error_log) ───────────────
// Read the firmware's lmac/umac error tables from device SRAM via the HBUS
// periphery-memory window. If valid != 0 the firmware asserted; error_id +
// the last hcmd/cmd_header reveal which command faulted it.
pub const HBUS_TARG_MEM_RADDR: u32 = 0x40C; // HBUS_BASE(0x400)+0x0C
pub const HBUS_TARG_MEM_RDAT: u32 = 0x41C; // auto-incrementing read data
// CSR_GP_CNTRL: grab NIC access to read SRAM (iwl_pcie_grab_nic_access).
pub const CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ: u32 = 0x0000_0008;
pub const CSR_GP_CNTRL_REG_FLAG_GOING_TO_SLEEP: u32 = 0x0000_0010;
// iwl_error_event_table (lmac) / iwl_umac_error_event_table u32 field indices.
pub const LERR_VALID: usize = 0;
pub const LERR_ERROR_ID: usize = 1;
pub const LERR_DATA1: usize = 7;
pub const LERR_DATA2: usize = 8;
pub const LERR_DATA3: usize = 9;
pub const LERR_HCMD: usize = 23;
pub const LERR_LAST_CMD_ID: usize = 29;
pub const LERR_WORDS: usize = 32; // enough to reach last_cmd_id
pub const UERR_VALID: usize = 0;
pub const UERR_ERROR_ID: usize = 1;
pub const UERR_DATA1: usize = 6;
pub const UERR_CMD_HEADER: usize = 13;
pub const UERR_WORDS: usize = 15;

// ── Stage 4d2b2: beacon / RX MPDU parse (fw/api/rx.h, mvm/rxmq.c) ──
// REPLY_RX_MPDU_CMD (0xc1, LEGACY_GROUP) carries an iwl_rx_mpdu_desc followed by
// the 802.11 frame. For family < AX210 the descriptor is IWL_RX_DESC_SIZE_V1 =
// offsetofend(struct iwl_rx_mpdu_desc, v1) = 20 (common head) + 28 (v1) = 48.
pub const REPLY_RX_MPDU_CMD: u8 = 0xc1;
pub const IWL_RX_DESC_SIZE_V1: usize = 48;
// Field offsets within iwl_rx_mpdu_desc, relative to pkt->data (RX_PKT_DATA_OFF).
pub const MPDU_OFF_MPDU_LEN: usize = 0; // __le16
pub const MPDU_OFF_MAC_FLAGS1: usize = 2; // u8
pub const MPDU_OFF_MAC_FLAGS2: usize = 3; // u8
pub const MPDU_OFF_STATUS: usize = 12; // __le32 (enum iwl_rx_mpdu_status)
pub const MPDU_OFF_RATE_N_FLAGS: usize = 28; // v1.rate_n_flags (union @20 + 8)
pub const MPDU_OFF_ENERGY_A: usize = 32; // v1.energy_a (union @20 + v1 offset 12)
pub const MPDU_OFF_ENERGY_B: usize = 33; // v1.energy_b
pub const MPDU_OFF_CHANNEL: usize = 34; // v1.channel
pub const MPDU_OFF_GP2_ON_AIR: usize = 36; // v1.gp2_on_air_rise __le32
pub const MPDU_OFF_TSF_ON_AIR: usize = 40; // v1.tsf_on_air_rise __le64
// mac_flags1: bits 7:4 = (MIC+CRC length / 2) the RADA may not have stripped.
// iwl_mvm_create_skb: mic_crc_len = u8_get_bits(mac_flags1, 0xf0) << 1.
pub const MFLG1_MIC_CRC_LEN_MASK: u8 = 0xf0;
// mac_flags2: the firmware DWORD-aligns the payload by inserting 2 bytes AFTER
// the IV when (802.11 header + IV) is not a multiple of 4 — which is exactly the
// QoS-header + CCMP case (26 + 8 = 34). Missing this shifts every payload by 2.
pub const MFLG2_PAD: u8 = 0x20;
// iwl_rx_mpdu_status: which cipher the firmware decrypted with (→ IV length).
pub const RX_STATUS_SEC_MASK: u32 = 0x7 << 8;
pub const RX_STATUS_SEC_CCM: u32 = 0x2 << 8;
pub const IEEE80211_CCMP_HDR_LEN: usize = 8;
// 802.11 management frame (ieee80211_hdr + beacon/probe-response body).
pub const DOT11_HDR_LEN: usize = 24; // fc(2)+dur(2)+addr1/2/3(18)+seq(2)
pub const DOT11_OFF_ADDR1: usize = 4;  // DA (receiver)
pub const DOT11_OFF_ADDR2: usize = 10; // SA (transmitter)
pub const DOT11_OFF_ADDR3: usize = 16; // BSSID
pub const DOT11_OFF_SEQ: usize = 22;   // seq_ctrl __le16 (frag 0-3 | seq_num 4-15)
pub const DOT11_BEACON_FIXED: usize = 12; // timestamp(8)+beacon_int(2)+capab(2)
pub const DOT11_OFF_IES: usize = DOT11_HDR_LEN + DOT11_BEACON_FIXED; // 36
pub const WLAN_EID_SSID: u8 = 0;
// Management-frame subtypes (frame_control bits 4-7) we collect APs from.
pub const DOT11_STYPE_BEACON: u8 = 8;
pub const DOT11_STYPE_PROBE_RESP: u8 = 5;
pub const MAX_APS: usize = 64; // both bands → more networks than 2.4 GHz alone
pub const SSID_MAX: usize = 32;

// ── Stage 5a: connect — PHY context + RLC + binding ──────────────
// All cmd_vers parsed from the FW file: PHY_CONTEXT v4, RLC_CONFIG v2,
// BINDING v2 (BINDING_CDB_SUPPORT=yes → full struct), CDB_SUPPORT=no → lmac_id 0.
pub const PHY_BAND_5_U8: u8 = 0; // PHY_BAND_5
pub const IWL_PHY_CHANNEL_MODE20: u8 = 0;
pub const IWL_LMAC_24G_INDEX: u32 = 0; // no CDB → lmac_id always 0
pub const FW_CTXT_INVALID: u32 = 0xffff_ffff;

// PHY_CONTEXT_CMD v4 (fw/api/phy-ctxt.h, struct iwl_phy_context_cmd, 32 B).
// PHY_CONTEXT_CMD = 0x08 already defined above (scan section).
pub const PHY_CTX_CMD_LEN: usize = 32;
pub const PC_OFF_ID_COLOR: usize = 0;     // __le32 FW_CMD_ID_AND_COLOR(phy_id=0,0)=0
pub const PC_OFF_ACTION: usize = 4;       // __le32 FW_CTXT_ACTION_ADD
pub const PC_OFF_CI_CHANNEL: usize = 8;   // ci.channel __le32
pub const PC_OFF_CI_BAND: usize = 12;     // ci.band u8 (PHY_BAND_24/5)
pub const PC_OFF_CI_WIDTH: usize = 13;    // ci.width u8 (MODE20)
pub const PC_OFF_CI_CTRL_POS: usize = 14; // ci.ctrl_pos u8 (0 for 20 MHz)
pub const PC_OFF_LMAC_ID: usize = 16;     // __le32 (ci.reserved @15)
// rxchain_info @20 (reserved in v4 → 0), dsp_cfg_flags @24,
// secondary_ctrl_chnl_loc @28, reserved[3] @29 — all 0.

// RLC_CONFIG_CMD v2 (DATA_PATH_GROUP/0x08, struct iwl_rlc_config_cmd, 32 B).
// Required: RLC cmd_ver=2 (< 3 = not offloaded), so the driver sends rx_chain_info.
pub const RLC_CONFIG_CMD: u8 = 0x08;
pub const RLC_CMD_LEN: usize = 32;
pub const RLC_OFF_PHY_ID: usize = 0;        // __le32 phy_id
pub const RLC_OFF_RX_CHAIN_INFO: usize = 4; // rlc.rx_chain_info __le32
// rlc.reserved @8; sad{chain_a@12,chain_b@16,mac_id@20,reserved@24}=0; flags@28; rsv@29.
// iwl_mvm_phy_ctxt_set_rxchain: valid_rx_ant<<PHY_RX_CHAIN_VALID_POS(1) |
// idle_cnt<<CNT_POS(10) | active_cnt<<MIMO_CNT_POS(12). 2x2 + diversity → idle=active=2.
pub const RLC_RX_CHAIN_INFO_2X2: u32 = (0x3 << 1) | (2 << 10) | (2 << 12); // 0x2806

// BINDING_CONTEXT_CMD v2 (0x2b, struct iwl_binding_cmd, 28 B; CDB → full w/ lmac_id).
pub const BINDING_CONTEXT_CMD: u8 = 0x2b;
pub const BINDING_CMD_LEN: usize = 28;
pub const BC_OFF_ID_COLOR: usize = 0; // FW_CMD_ID_AND_COLOR(phy_id=0,0)=0
pub const BC_OFF_ACTION: usize = 4;   // FW_CTXT_ACTION_ADD
pub const BC_OFF_MACS: usize = 8;     // __le32 macs[3] (MAX_MACS_IN_BINDING)
pub const BC_OFF_PHY: usize = 20;     // __le32 phy = FW_CMD_ID_AND_COLOR(phy_id=0,0)
pub const BC_OFF_LMAC_ID: usize = 24; // __le32 lmac_id

// ── Stage 5b: connect — station (AP peer) + gen2 TX queue ─────────
// ADD_STA v12 (fw/api/sta.h, struct iwl_mvm_add_sta_cmd, 48 B). ADD_STA = 0x18
// already defined above. Status response (ADD_STA_SUCCESS = 0x1).
pub const ADD_STA_CMD_LEN: usize = 48;
pub const AS_OFF_ADD_MODIFY: usize = 0;   // u8 (0 = add)
pub const AS_OFF_TID_DISABLE: usize = 2;  // __le16 tid_disable_tx
pub const AS_OFF_MAC_ID_COLOR: usize = 4; // __le32 FW_CMD_ID_AND_COLOR(0,0)=0
pub const AS_OFF_ADDR: usize = 8;         // u8[6] AP BSSID
pub const AS_OFF_STA_ID: usize = 16;      // u8
pub const AS_OFF_STATION_FLAGS: usize = 20;     // __le32
pub const AS_OFF_STATION_FLAGS_MSK: usize = 24; // __le32
pub const AS_OFF_STATION_TYPE: usize = 35;      // u8
pub const AS_OFF_ASSOC_ID: usize = 36;          // __le16 (set from IEEE80211_STA_ASSOC on)
pub const IWL_STA_LINK: u8 = 0;           // enum iwl_sta_type
pub const ADD_STA_SUCCESS: u32 = 0x1;
pub const ADD_STA_STATUS_MASK: u32 = 0xff; // IWL_ADD_STA_STATUS_MASK
pub const TID_DISABLE_AGG_INIT: u16 = 0xffff; // "No aggs at first" (sta.c:1779)
// station_flags_msk for the add: FAT_EN(3<<26) | MIMO_EN(3<<28) | RTS_MIMO_PROT(BIT17).
pub const STA_FLAGS_MSK_ADD: u32 = (3 << 26) | (3 << 28) | (1 << 17); // 0x3C020000
// enum iwl_sta_flags (fw/api/sta.h). Applied at assoc once the peer's HT caps
// are known (iwl_mvm_sta_send_to_fw): channel width, spatial streams, and the
// A-MPDU limits the AP advertised. Without them the firmware keeps the station
// at its "just added" defaults and TLC has nothing to scale into.
pub const STA_FLG_FAT_EN_20MHZ: u32 = 0 << 26;
pub const STA_FLG_MIMO_EN_SISO: u32 = 0 << 28;
pub const STA_FLG_MIMO_EN_MIMO2: u32 = 1 << 28;
pub const STA_FLG_MAX_AGG_SIZE_SHIFT: u32 = 19;
pub const STA_FLG_MAX_AGG_SIZE_MSK: u32 = 0xf << 19;
pub const STA_FLG_AGG_MPDU_DENS_SHIFT: u32 = 23;
pub const STA_FLG_AGG_MPDU_DENS_MSK: u32 = 7 << 23;
pub const AP_STA_ID: u8 = 0;              // first free station table index

// SCD_QUEUE_CONFIG_CMD v3 (DATA_PATH_GROUP/0x17, struct iwl_scd_queue_cfg_cmd
// ADD union, 36 B). gen2 dynamic TX queue allocation.
pub const SCD_QUEUE_CONFIG_CMD: u8 = 0x17;
pub const SCD_CMD_LEN: usize = 36;
pub const IWL_SCD_QUEUE_ADD: u32 = 0;
pub const SQ_OFF_OPERATION: usize = 0;   // __le32
pub const SQ_OFF_STA_MASK: usize = 4;    // __le32 BIT(sta_id)
pub const SQ_OFF_TID: usize = 8;         // u8
pub const SQ_OFF_FLAGS: usize = 12;      // __le32 (0 for v3 ADD)
pub const SQ_OFF_CB_SIZE: usize = 16;    // __le32 TFD_QUEUE_CB_SIZE(size)=ilog2(size)-3
pub const SQ_OFF_BC_DRAM_ADDR: usize = 20; // __le64
pub const SQ_OFF_TFDQ_DRAM_ADDR: usize = 28; // __le64
// iwl_tx_queue_cfg_rsp: queue_number __le16 @0, flags @2, write_pointer @4.
pub const SQ_RSP_OFF_QUEUE_NUMBER: usize = 0;
pub const SQ_RSP_OFF_WRITE_PTR: usize = 4;
// MGMT queue (auth/assoc TX): tid = IWL_MGMT_TID, size = IWL_MGMT_QUEUE_SIZE
// (22000 has no min_txq_size → max(16,0)=16). cb_size = ilog2(16)-3 = 1.
pub const IWL_MGMT_TID: u8 = 15;
pub const IWL_MGMT_QUEUE_SIZE: usize = 16;
pub const MGMT_QUEUE_CB_SIZE: u32 = 1;
// bc table (gen2, non-AX210): iwl_bc_tbl_entry(__le16) * TFD_QUEUE_BC_SIZE(256+64).
pub const TFD_QUEUE_BC_SIZE: usize = 256 + 64;
pub const BC_TBL_BYTES: usize = TFD_QUEUE_BC_SIZE * 2; // 640

// ── Rate scaling (TLC offload) — TLC_MNG_CONFIG_CMD (DATA_PATH_GROUP/0x0F) ──
// struct iwl_tlc_config_cmd_v4 (fw/api/rs.h, TLC_MNG_CONFIG_CMD_API_S_VER_4),
// 28 bytes. Our FW reports cmd_ver=4. Without this command the firmware never
// rate-scales and every data frame goes out at the fixed host rate (1 Mbit CCK,
// tx_raw); with it the firmware picks the best per-frame rate from the station's
// advertised rate set (iwl_mvm_rs_fw_rate_init, mvm/rs-fw.c).
pub const TLC_MNG_CONFIG_CMD: u8 = 0x0F;
pub const TLC_CMD_LEN: usize = 28;
pub const TLC_OFF_STA_ID: usize = 0;       // u8
pub const TLC_OFF_MAX_CH_WIDTH: usize = 4; // u8 (enum iwl_tlc_mng_cfg_cw)
pub const TLC_OFF_MODE: usize = 5;         // u8 (enum iwl_tlc_mng_cfg_mode)
pub const TLC_OFF_CHAINS: usize = 6;       // u8 (chain A|B mask, = ANT_AB)
pub const TLC_OFF_SGI: usize = 7;          // u8 sgi_ch_width_supp
pub const TLC_OFF_FLAGS: usize = 8;        // __le16
pub const TLC_OFF_NON_HT_RATES: usize = 10; // __le16
pub const TLC_OFF_HT_RATES: usize = 12;    // __le16[2][3] (12 B, 0 for legacy)
pub const TLC_OFF_MAX_MPDU: usize = 24;    // __le16
pub const TLC_OFF_MAX_TXOP: usize = 26;    // __le16
pub const TLC_CH_WIDTH_20MHZ: u8 = 0;      // IWL_TLC_MNG_CH_WIDTH_20MHZ
pub const TLC_MODE_NON_HT: u8 = 0;         // IWL_TLC_MNG_MODE_NON_HT
pub const TLC_MODE_HT: u8 = 1;             // IWL_TLC_MNG_MODE_HT
// ht_rates is __le16[IWL_TLC_NSS_MAX=2][IWL_TLC_MCS_PER_BW_NUM_V4=3]; HT only
// ever fills the [nss][IWL_TLC_MCS_PER_BW_80=0] slot (rs_fw_set_supp_rates).
pub const TLC_OFF_HT_RATES_NSS1: usize = TLC_OFF_HT_RATES;     // [0][0]
pub const TLC_OFF_HT_RATES_NSS2: usize = TLC_OFF_HT_RATES + 6; // [1][0]
// enum iwl_tlc_mng_cfg_flags
pub const TLC_FLAGS_STBC: u16 = 1 << 0;
pub const TLC_FLAGS_LDPC: u16 = 1 << 1;
// TLC_MNG_UPDATE_NOTIF (DATA_PATH_GROUP/0xF7, notif_ver 3): the firmware reports
// the rate it settled on. This is the ONLY way to see the negotiated air rate
// from the host — without it we cannot tell 1 Mbit CCK from MCS 15.
// struct iwl_tlc_update_notif: sta_id u8, reserved[3], flags __le32, rate __le32.
pub const TLC_MNG_UPDATE_NOTIF: u8 = 0xF7;
pub const TLC_NOTIF_OFF_RATE: usize = 8;   // __le32 rate_n_flags
// rate_n_flags v2 field extraction (same format as the TX command, see below).
pub const RATE_MCS_MOD_TYPE_POS: u32 = 8;
pub const RATE_MCS_MOD_TYPE_MSK: u32 = 0x7 << 8;
pub const RATE_MCS_MOD_TYPE_CCK: u32 = 0 << 8;
pub const RATE_MCS_MOD_TYPE_LEGACY_OFDM: u32 = 1 << 8;
pub const RATE_MCS_MOD_TYPE_HT: u32 = 2 << 8;
pub const RATE_MCS_MOD_TYPE_VHT: u32 = 3 << 8;
pub const RATE_MCS_MOD_TYPE_HE: u32 = 4 << 8;
pub const RATE_MCS_CODE_MSK: u32 = 0x1f;   // MCS index / legacy rate index
pub const RATE_MCS_NSS_MSK: u32 = 0x20;    // 0 = 1 stream, 1 = 2 streams
// …but only in the v3 format. This firmware reports v2 (TX_CMD cmd_ver 9 < 11),
// where the SAME field sits one bit lower. Everything else is identical —
// iwl_v3_rate_from_v2_v3 does nothing but move this one bit. Read with the v3
// mask, a v2 word turns "MCS 7, 2 streams" into "MCS 23, 1 stream": a rate that
// does not exist, and no Mbit figure at all.
pub const RATE_MCS_NSS_MSK_V2: u32 = 0x10;
// fw_rates_ver: TX_CMD cmd_ver >= 11 means the firmware talks v3 (iwl_mvm_has_rate_v3).
pub const TX_CMD_VER_RATE_V3: u8 = 11;
pub const RATE_MCS_CHAN_WIDTH_POS: u32 = 11;
pub const RATE_MCS_CHAN_WIDTH_MSK: u32 = 0x7 << 11;
// Supported legacy-rate bitmap: BIT(hw_value) per rate (IWL_RATE_*_INDEX). Full
// 2.4 GHz 11g = CCK 1/2/5.5/11 (bits 0-3) + OFDM 6..54 (bits 4-11) = 0x0FFF;
// 5 GHz is OFDM-only (no CCK) = bits 4-11 = 0x0FF0.
pub const TLC_NON_HT_RATES_24: u16 = 0x0FFF;
pub const TLC_NON_HT_RATES_5: u16 = 0x0FF0;

// ── Stage 5c: gen2 TX data path (send an 802.11 frame) ───────────
// TX_CMD = 0x1c. The device TX command is a SHORT 4-byte iwl_cmd_header
// (cmd, group_id=0, sequence) followed by iwl_tx_cmd_v9 (TX_CMD cmd_ver=9) and
// the 802.11 frame. Unlike host commands, TX uses the short header (group 0).
pub const TX_CMD: u8 = 0x1c;
// iwl_tx_cmd_v9 (fw/api/tx.h): len/offload/flags/dram_info/rate_n_flags = 20 B,
// then the 802.11 header+body. Field offsets relative to the dev_cmd start
// (after the 4-byte cmd header), i.e. tx_cmd begins at dev_cmd offset 4.
pub const TXC_HDR_LEN: usize = 4;       // iwl_cmd_header (short)
pub const TXC_OFF_LEN: usize = 4;       // tx_cmd_v9.len __le16 (full 802.11 frame len)
pub const TXC_OFF_OFFLOAD: usize = 6;   // tx_cmd_v9.offload_assist __le16
pub const TXC_OFF_FLAGS: usize = 8;     // tx_cmd_v9.flags __le32
pub const TXC_OFF_DRAM: usize = 12;     // tx_cmd_v9.dram_info (8 B, 0 = no key)
pub const TXC_OFF_RATE: usize = 20;     // tx_cmd_v9.rate_n_flags __le32
pub const TXC_OFF_FRAME: usize = 24;    // 802.11 frame (hdr[]) starts here
// iwl_tx_flags (NEW gen2 set — NOT the gen1 TX_CMD_FLG_*).
pub const IWL_TX_FLAGS_CMD_RATE: u32 = 1 << 0; // use rate_n_flags from the cmd
pub const IWL_TX_FLAGS_ENCRYPT_DIS: u32 = 1 << 1; // unencrypted
// offload_assist (enum iwl_tx_offload_assist_flags_pos). iwl_mvm_tx_csum fills
// MH_SIZE for EVERY frame — the 802.11 header length in 2-byte words — and sets
// PAD when that length is not a multiple of 4, in which case the transport
// inserts 2 bytes between header and payload to DWORD-align the payload. That
// is exactly the QoS case (26 bytes); getting it wrong misplaces the CCMP IV.
pub const TX_CMD_OFFLD_MH_SIZE_POS: u16 = 8;
pub const TX_CMD_OFFLD_PAD: u16 = 1 << 13;
// rate_n_flags (fw_rates_ver=2: TX_CMD cmd_ver 9 ≥8 <11). Modern format:
// MOD_TYPE @ bit8 (CCK=0, LEGACY_OFDM=1<<8), legacy rate in bits 0-2, ANT @ bit14.
// 1 Mbps CCK ant A = 0x4000; 6 Mbps OFDM ant A = 0x4100 (iwl_v3_rate_to_v2_v3).
pub const RATE_1M_CCK_ANT_A: u32 = 0x0000_4000;
pub const RATE_6M_OFDM_ANT_A: u32 = 0x0000_4100;
// 802.11 management auth frame (open system): 24-byte header + 6-byte body.
pub const DOT11_FC_AUTH: u8 = 0xB0; // frame_control byte0: type mgmt(0), subtype auth(11)
pub const DOT11_AUTH_BODY_LEN: usize = 6; // algorithm(2) + seq(2) + status(2)
pub const DOT11_AUTH_ALG_OPEN: u16 = 0;
pub const DOT11_AUTH_SEQ_1: u16 = 1;
pub const DOT11_AUTH_SEQ_2: u16 = 2;  // AP's auth response
pub const DOT11_STATUS_SUCCESS: u16 = 0;

// ── Stage 5d: association request / response ──────────────────────
pub const DOT11_FC_ASSOC_REQ: u8 = 0x00; // mgmt(0), subtype assoc-req(0)
pub const DOT11_STYPE_AUTH: u8 = 11;     // subtype of an auth frame
pub const DOT11_STYPE_ASSOC_RESP: u8 = 1; // subtype of an assoc response
pub const DOT11_STYPE_DISASSOC: u8 = 10; // mgmt subtype disassociation
pub const DOT11_STYPE_DEAUTH: u8 = 12;   // mgmt subtype deauthentication (mesh steering kick)
// assoc-req fixed fields: capability(2) + listen_interval(2), then IEs.
// assoc-resp body: capability(2) + status_code(2) + aid(2), then IEs.
pub const ASSOC_RESP_OFF_STATUS: usize = 2; // within the 802.11 body
pub const ASSOC_RESP_OFF_AID: usize = 4;
pub const DOT11_LISTEN_INTERVAL: u16 = 10;
// ieee80211 capability bits (assoc-req).
pub const WLAN_CAP_ESS: u16 = 1 << 0;
pub const WLAN_CAP_PRIVACY: u16 = 1 << 4;
pub const WLAN_CAP_SHORT_PREAMBLE: u16 = 1 << 5;
pub const WLAN_CAP_SHORT_SLOT: u16 = 1 << 10;
// information element ids.
pub const WLAN_EID_SUPP_RATES: u8 = 1;
pub const WLAN_EID_TIM: u8 = 5;
pub const WLAN_EID_HT_CAPABILITY: u8 = 45;
pub const WLAN_EID_RSN: u8 = 48;
pub const WLAN_EID_EXT_SUPP_RATES: u8 = 50;
pub const WLAN_EID_VENDOR_SPECIFIC: u8 = 221;

// ── HT (802.11n) capability element — include/linux/ieee80211.h ──────
// struct ieee80211_ht_cap, 26 bytes: cap_info(2) ampdu_params(1) mcs(16)
// extended_ht_cap(2) tx_BF_cap(4) antenna_selection(1). Without this element in
// the assoc request the AP treats us as an 802.11a/g station: 54 Mbit ceiling,
// no aggregation, and the firmware's TLC has nothing above OFDM to scale into.
pub const HT_CAP_IE_LEN: usize = 26;
pub const HT_OFF_CAP_INFO: usize = 0;   // __le16
pub const HT_OFF_AMPDU_PARAMS: usize = 2; // u8
pub const HT_OFF_MCS_RX_MASK: usize = 3;  // u8[10] (mcs.rx_mask)
pub const HT_OFF_MCS_TX_PARAMS: usize = 3 + 12; // mcs.tx_params (after rx_mask+rx_highest)
pub const IEEE80211_HT_CAP_LDPC_CODING: u16 = 0x0001;
pub const IEEE80211_HT_CAP_SM_PS_DISABLED: u16 = 3 << 2; // WLAN_HT_CAP_SM_PS_DISABLED
pub const IEEE80211_HT_CAP_SGI_20: u16 = 0x0020;
pub const IEEE80211_HT_CAP_RX_STBC_1: u16 = 1 << 8;  // 1 spatial stream
pub const IEEE80211_HT_CAP_RX_STBC: u16 = 0x0300;
pub const IEEE80211_HT_MCS_TX_DEFINED: u8 = 0x01;
pub const IEEE80211_HT_AMPDU_PARM_FACTOR: u8 = 0x03;
pub const IEEE80211_HT_AMPDU_PARM_DENSITY: u8 = 0x1C;
pub const IEEE80211_HT_AMPDU_PARM_DENSITY_SHIFT: u8 = 2;
// iwl_init_ht_hw_capab: the AX200 advertises 64 KB A-MPDU / 4 us MPDU density.
pub const HT_AMPDU_FACTOR_64K: u8 = 3;
pub const HT_AMPDU_DENSITY_4US: u8 = 5; // IEEE80211_HT_MPDU_DENSITY_4

// WMM Information Element (WFA vendor-specific 221, OUI 00:50:F2 type 2
// subtype 0 version 1, then a QoS-Info byte). An HT station is by definition a
// QoS station; APs that see HT without WMM may refuse to use HT rates.
pub const WMM_INFO_IE: [u8; 9] = [
    WLAN_EID_VENDOR_SPECIFIC, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00,
];

// QoS data frames: subtype bit 3 set (0x88 with the data type) and a 2-byte QoS
// control field after addr3/seq, so the header is 26 instead of 24 bytes.
pub const DOT11_FC_QOS_DATA: u8 = 0x88; // type data(2), subtype 8 (QoS data)
pub const DOT11_QOS_HDR_LEN: usize = 26;

// Block-Ack action frames (802.11 category 3). We are not an aggregating
// receiver yet, so an ADDBA request from the AP is answered with an explicit
// decline — leaving it unanswered makes some APs hold the TID in limbo.
pub const DOT11_STYPE_ACTION: u8 = 13;
pub const WLAN_CATEGORY_BACK: u8 = 3;
pub const WLAN_ACTION_ADDBA_REQ: u8 = 0;
pub const WLAN_ACTION_ADDBA_RESP: u8 = 1;
pub const WLAN_STATUS_REQUEST_DECLINED: u16 = 37;
// privacy bit in a beacon's capability field (offset hdr+8+2 = 34).
pub const DOT11_BEACON_CAP_OFF: usize = DOT11_HDR_LEN + 8 + 2; // 34
pub const WLAN_CAP_PRIVACY_BIT: u8 = 0x10;

// ── Stage 5c: session protection (prepare_tx hook before auth) ────
// mac80211 calls drv_mgd_prepare_tx → iwl_mvm_mac_mgd_prepare_tx →
// iwl_mvm_protect_assoc → iwl_mvm_schedule_session_protection right before the
// auth frame. It reserves channel time for the auth/assoc exchange; without it
// the unassociated STA gets no airtime and the FW holds the frame back.
// FW has CAPA_SESSION_PROT_CMD (bit 54) → the SESSION_PROTECTION_CMD path.
pub const MAC_CONF_GROUP: u8 = 0x3; // fw/api/commands.h
pub const SESSION_PROTECTION_CMD: u8 = 0x5; // MAC_CONF_GROUP, cmd_ver=1 (fw/api/mac-cfg.h)
// struct iwl_session_prot_cmd (fw/api/time-event.h), 24 B, all __le32.
pub const SP_CMD_LEN: usize = 24;
pub const SP_OFF_ID_COLOR: usize = 0;  // mac id (cmd_ver 1 → mvmvif->id = 0)
pub const SP_OFF_ACTION: usize = 4;    // FW_CTXT_ACTION_ADD
pub const SP_OFF_CONF_ID: usize = 8;   // SESSION_PROTECT_CONF_ASSOC = 0
pub const SP_OFF_DURATION_TU: usize = 12; // MSEC_TO_TU(900) = 900*1000/1024 = 878
// repetition_count @16, interval @20 — not used, 0.
pub const SESSION_PROTECT_CONF_ASSOC: u32 = 0; // first enum value
pub const SP_DURATION_TU: u32 = 878; // schedule_session_protection passes 900 ms

// ── Connect tail: per-MAC power (iwl_mvm_power_update_mac) ────────
// __iwl_mvm_assign_vif_chanctx sends power before quotas ("Power state must be
// updated before quotas"). iwl_mvm_power_send_cmd → MAC_PM_POWER_TABLE (0xa9,
// legacy group → promoted to LONG_GROUP). struct iwl_mac_power_cmd, 40 B. We
// model the power-save-disabled path (iwl_mvm_power_build_cmd early-return):
// only id_and_color + keep_alive_seconds, flags = 0 (no PS — we don't sleep).
pub const MAC_PM_POWER_TABLE: u8 = 0xa9;
pub const MAC_POWER_CMD_LEN: usize = 40;
pub const MP_OFF_ID_COLOR: usize = 0;     // __le32 FW_CMD_ID_AND_COLOR(0,0)=0
pub const MP_OFF_FLAGS: usize = 4;        // __le16 (0 = PS disabled)
pub const MP_OFF_KEEP_ALIVE: usize = 6;   // __le16 keep_alive_seconds
pub const POWER_KEEP_ALIVE_PERIOD_SEC: u16 = 25; // max(3*dtim*bi, this); dtim=0 → 25

// ── Stage 5e: EAPOL transport + WiFi-class control channel ───────
// After association the AP runs the WPA2 4-way handshake; its EAPOL-Key frames
// arrive as 802.11 DATA frames (LLC/SNAP, ethertype 0x888E). We demux them off
// the RX ring and forward to wifid (the supplicant) over the control channel.
pub const ETHERTYPE_EAPOL: u16 = 0x888E;
pub const LLC_SNAP_HDR: [u8; 6] = [0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00];
pub const DOT11_FC_TYPE_DATA: u8 = 0x08; // fc byte0 & 0x0c == data
pub const DOT11_STYPE_QOS: u8 = 0x08;    // subtype bit → +2-byte QoS control
// Control-channel wire ops (docs/spec/WIFI_CLASS_ABI.md). downlink = manager→driver.
pub const CMD_SET_KEY: u8 = 0x04;
pub const CMD_TX_EAPOL: u8 = 0x05;
pub const CMD_ASSOCIATED: u8 = 0x07;
pub const CMD_AUTHORIZED: u8 = 0x08;
// uplink = driver→manager.
pub const EV_EAPOL_RX: u8 = 0x84;
pub const EV_LINK_UP: u8 = 0x85;
pub const EV_LINK_DOWN: u8 = 0x86;
pub const EV_READY: u8 = 0x83;

// ── Stage 5e: data TX queue (tid 0) + key install ────────────────
pub const IWL_DATA_TID: u8 = 0;
// Data TX queue depth. 16 was too shallow once rate scaling lifted throughput
// from 1 Mbit to tens of Mbit: under a burst the firmware's read pointer lags
// (TX retries) and the ring overflows. 64 gives headroom over the kernel's
// 16-slot TX mailbox while staying well within the gen2 max (256). The TFD slot
// index = write_ptr & (size-1); cb_size = ilog2(size)-3 (must match the size).
pub const IWL_DATA_QUEUE_SIZE: usize = 64;

// BQL-style anti-bufferbloat cap: never let more than this many data frames sit
// in-flight in the FW queue, even though the ring holds 64. Each in-flight frame
// = ~1.5 KB of airtime a latency-sensitive packet (a ping) must wait behind, so
// the full 63-deep queue is tens of ms of bufferbloat. ~1 BDP at the current
// achieved rate keeps throughput while slashing latency-under-load. Raise once
// HT/A-MPDU lifts the air rate (then the BDP grows). Must stay < QUEUE_SIZE-1.
pub const TX_INFLIGHT_MAX: u32 = 16;
pub const DATA_QUEUE_CB_SIZE: u32 = 3; // TFD_QUEUE_CB_SIZE(64) = ilog2(64)-3
// Per-slot TX staging stride. Each in-flight TFD's TB1 must point at its OWN
// payload region, or back-to-back frames clobber each other's data before the
// firmware DMAs it (the bug behind "dies under load once the rate went up").
// One full dev-cmd + 802.11 data frame (24 + ~1532) fits in 2 KiB.
pub const TX_PAYLOAD_STRIDE: usize = 2048;
// ADD_STA_KEY (0x17 LEGACY → LONG_GROUP, cmd_ver 3). struct iwl_mvm_add_sta_key_cmd
// = common(52) + rx_mic(8) + tx_mic(8) + tx_seq(8) = 76 B. CCMP: mic/seq all 0.
pub const ADD_STA_KEY_CMD: u8 = 0x17;
pub const ADD_STA_KEY_LEN: usize = 76;
pub const KEY_OFF_STA_ID: usize = 0;       // u8
pub const KEY_OFF_KEY_OFFSET: usize = 1;   // u8 (FW key-table slot)
pub const KEY_OFF_KEY_FLAGS: usize = 2;    // __le16
pub const KEY_OFF_KEY: usize = 4;          // u8[32]
pub const KEY_OFF_RX_SEQ: usize = 36;      // u8[16] (rx_secur_seq_cnt / RSC)
// iwl_sta_key_flag: CCMP encryption + key id + group/MFP.
pub const STA_KEY_FLG_CCM: u16 = 2 << 0;
pub const STA_KEY_FLG_KEYID_POS: u16 = 8;
pub const STA_KEY_MULTICAST: u16 = 1 << 14;
// 802.11 data frame: frame_control byte0 = data(type 2), byte1 toDS for STA→AP.
pub const DOT11_FC_DATA: u8 = 0x08; // type data, subtype 0
pub const DOT11_FC1_TODS: u8 = 0x01;

// ── TX completion (struct iwl_tx_resp, fw/api/tx.h) ──────────────
// The new-tx-api variant, which is what a gen2 device sends
// (iwl_mvm_get_agg_status → &((struct iwl_tx_resp *)tx_resp)->status). 44 bytes;
// offsets relative to pkt->data (RX_PKT_DATA_OFF). This is the only place the
// host learns what an on-air transmission actually cost: how many times it was
// retried, and how many microseconds of airtime it burned.
pub const TX_RESP_LEN: usize = 44;
pub const TXR_OFF_FRAME_COUNT: usize = 0;  // u8
pub const TXR_OFF_FAILURE_RTS: usize = 2;  // u8
pub const TXR_OFF_FAILURE_FRAME: usize = 3; // u8 — retries; count = this + 1
pub const TXR_OFF_INITIAL_RATE: usize = 4; // __le32 rate_n_flags
pub const TXR_OFF_MEDIA_TIME: usize = 8;   // __le16 wireless_media_time (us)
pub const TXR_OFF_BYTE_CNT: usize = 30;    // __le16
pub const TXR_OFF_STATUS: usize = 40;      // __le16 status.status
pub const TX_STATUS_MSK: u16 = 0x00ff;
pub const TX_STATUS_SUCCESS: u16 = 0x01;
pub const TX_STATUS_DIRECT_DONE: u16 = 0x02;
// rate_n_flags v2, remaining fields we decode for the report (fw/api/rs.h).
pub const RATE_MCS_SGI_MSK: u32 = 1 << 20;
pub const RATE_MCS_LDPC_MSK: u32 = 1 << 16;

// ── Connect policy (sys/config/wifi_ssid, sys/config/wifi_band) ──
pub const BAND_PREF_AUTO: u8 = 0; // prefer 5 GHz when it is strong enough
pub const BAND_PREF_5: u8 = 1;    // 5 GHz only (fall back if none)
pub const BAND_PREF_24: u8 = 2;   // 2.4 GHz only (fall back if none)
// Above this RSSI a 5 GHz AP is worth taking over a louder 2.4 GHz one: the
// band is uncongested and, in a repeater mesh, usually the router itself rather
// than a node whose backhaul halves the throughput. wpa_supplicant's band
// preference works the same way — a signal floor, not a pure RSSI contest.
pub const BAND_PREF_5_MIN_RSSI: i8 = -60;
// …and never more than this far below the best 2.4 GHz AP of the same network.
// A floor alone is not enough: 5 GHz at -64 dBm clears any sane floor while a
// 2.4 GHz radio of the same mesh sits at -46, and the wider band cannot make up
// an 18 dB deficit. Measured case: at -64 not even a beacon of the chosen BSS
// arrived, so the association completed and then nothing else ever did.
pub const BAND_PREF_5_MAX_PENALTY_DB: i16 = 12;
pub const PICK_STRONGEST: u8 = 0;
pub const PICK_5G_PREFERRED: u8 = 1;
pub const PICK_BAND_FORCED: u8 = 2;
pub const PICK_SSID_FILTERED: u8 = 3;

// Status snapshot published via npk_driver_report. One screen of text.
pub const REPORT_CAP: usize = 1600;
// How often the snapshot is refreshed. 1 s is short enough to show a speed test
// live and long enough that formatting never lands in the hot path.
pub const REPORT_PERIOD_MS: u64 = 1000;

// ── PCIe capability layout (apm_config: ASPM / LTR detect) ───────
pub const PCI_CAP_PTR: u8 = 0x34; // first capability pointer
pub const PCI_CAP_ID_EXP: u8 = 0x10; // PCI Express capability
pub const PCI_EXP_LNKCTL: u8 = 0x10; // offset within PCIe cap
pub const PCI_EXP_DEVCTL2: u8 = 0x28; // offset within PCIe cap
pub const PCI_EXP_LNKCTL_ASPM_L0S: u16 = 0x0001;
pub const PCI_EXP_DEVCTL2_LTR_EN: u16 = 0x0400;

// How long to wait for the AP to start the 4-way after associating before
// treating the association as dead and reconnecting. An AP normally sends msg1
// within milliseconds; 8 s leaves room for a retransmit round without leaving a
// silently-dead link up for minutes.
pub const HANDSHAKE_TIMEOUT_MS: u64 = 8000;
pub const PICK_5G_TOO_WEAK: u8 = 4;
