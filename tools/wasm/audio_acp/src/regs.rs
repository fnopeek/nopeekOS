//! `rn_chip_offset_byte.h` and `rn_acp3x.h` (Linux 6.18.60), the
//! registers the PDM path uses, as offsets into BAR 0 (`rn_readl`
//! subtracts `ACP_PHY_BASE_ADDRESS`).

pub const ACP_PHY_BASE_ADDRESS: u32 = 0x1240000;
pub const ACP_REG_END: u32 = 0x1250200;

const fn r(a: u32) -> u32 { a - ACP_PHY_BASE_ADDRESS }

pub const ACPAXI2AXI_ATU_PAGE_SIZE_GRP_1: u32 = r(0x1240C00);
pub const ACPAXI2AXI_ATU_BASE_ADDR_GRP_1: u32 = r(0x1240C04);
pub const ACPAXI2AXI_ATU_CTRL: u32 = r(0x1240C40);
pub const ACP_SOFT_RESET: u32 = r(0x1241000);
pub const ACP_CONTROL: u32 = r(0x1241004);
pub const ACP_EXTERNAL_INTR_ENB: u32 = r(0x1241800);
pub const ACP_EXTERNAL_INTR_CNTL: u32 = r(0x1241804);
pub const ACP_EXTERNAL_INTR_STAT: u32 = r(0x1241808);
pub const ACP_PGFSM_CONTROL: u32 = r(0x124141C);
pub const ACP_PGFSM_STATUS: u32 = r(0x1241420);
pub const ACP_CLKMUX_SEL: u32 = r(0x1241424);
pub const ACP_SCRATCH_REG_0: u32 = r(0x1250000);
pub const ACP_WOV_PDM_ENABLE: u32 = r(0x1242C04);
pub const ACP_WOV_PDM_DMA_ENABLE: u32 = r(0x1242C08);
pub const ACP_WOV_RX_RINGBUFADDR: u32 = r(0x1242C0C);
pub const ACP_WOV_RX_RINGBUFSIZE: u32 = r(0x1242C10);
pub const ACP_WOV_RX_LINEARPOSITIONCNTR_HIGH: u32 = r(0x1242C18);
pub const ACP_WOV_RX_LINEARPOSITIONCNTR_LOW: u32 = r(0x1242C1C);
pub const ACP_WOV_RX_INTR_WATERMARK_SIZE: u32 = r(0x1242C20);
pub const ACP_WOV_PDM_FIFO_FLUSH: u32 = r(0x1242C24);
pub const ACP_WOV_PDM_NO_OF_CHANNELS: u32 = r(0x1242C28);
pub const ACP_WOV_PDM_DECIMATION_FACTOR: u32 = r(0x1242C2C);
pub const ACP_WOV_MISC_CTRL: u32 = r(0x1242C5C);
pub const ACP_WOV_CLK_CTRL: u32 = r(0x1242C60);

pub const ACP_DEVICE_ID: u32 = 0x15E2;
pub const ACP_POWER_ON_IN_PROGRESS: u32 = 0x01;
pub const ACP_SOFT_RESET_SOFTRESET_AUDDONE_MASK: u32 = 0x00010001;
pub const ACP_PGFSM_CNTL_POWER_ON_MASK: u32 = 0x01;
pub const ACP_PGFSM_STATUS_MASK: u32 = 0x03;
pub const ACP_ERROR_MASK: u32 = 0x20000000;
pub const ACP_EXT_INTR_STAT_CLEAR_MASK: u32 = 0xFFFFFFFF;
pub const PDM_DMA_STAT: u32 = 0x10;
pub const PDM_DMA_INTR_MASK: u32 = 0x10000;
pub const PDM_DECIMATION_FACTOR: u32 = 0x2;
pub const ACP_PDM_CLK_FREQ_MASK: u32 = 0x07;
/// `ACP_WOV_GAIN_CONTROL`, `GENMASK(4, 3)`.
pub const ACP_WOV_GAIN_CONTROL: u32 = 0x18;
pub const ACP_PDM_ENABLE: u32 = 0x01;
pub const ACP_PDM_DISABLE: u32 = 0x00;
pub const ACP_PDM_DMA_EN_STATUS: u32 = 0x02;
pub const DELAY_US: u32 = 5;
pub const ACP_COUNTER: u32 = 20000;
pub const ACP_SRAM_PTE_OFFSET: u32 = 0x02050000;
pub const PAGE_SIZE_4K_ENABLE: u32 = 0x2;
pub const MEM_WINDOW_START: u32 = 0x4000000;
pub const CAPTURE_MAX_NUM_PERIODS: u32 = 4;
pub const CAPTURE_MAX_PERIOD_SIZE: u32 = 8192;
