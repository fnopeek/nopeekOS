/* The DP AUX engines of the register file, and an eDP panel behind the
 * engine of the panel's DDC. A request is the bytes loaded through
 * AUX_SW_DATA (a write with AUX_SW_AUTOINCREMENT_DISABLE loads the index,
 * every data write stores and advances it); AUX_SW_GO sends it and sets
 * AUX_SW_DONE with the reply in the same buffer, which reads advance
 * through once AUX_SW_DATA_RW is set. GO and DONE_ACK clear themselves,
 * the arbiter always grants the software, a reset is done as it is asked.
 * The other engines time out. Included by main_dchw.c, ahead of rd/wr. */
#define AUX_ENGINES 6

struct aux_engine_model {
	const struct dce110_aux_registers *r;
	bool sink;
	u8 tx[32], rx[32];
	unsigned int idx, ridx, rx_n;
	bool rmode;
	u32 status;
	unsigned int i2c_off;
};
static struct aux_engine_model aux_engines[AUX_ENGINES];
static int n_aux_engines;
static u8 sink_dpcd[0x100000];
static u8 sink_edid[128];

static struct aux_engine_model *aux_engine_of(u32 reg, int *which)
{
	for (int i = 0; i < n_aux_engines; i++) {
		const struct dce110_aux_registers *r = aux_engines[i].r;
		int w = reg == r->AUX_SW_DATA ? 1 : reg == r->AUX_SW_CONTROL ? 2 : reg == r->AUX_INTERRUPT_CONTROL ? 3 :
			reg == r->AUX_SW_STATUS ? 4 : reg == r->AUX_ARB_CONTROL ? 5 : reg == r->AUX_CONTROL ? 6 : 0;
		if (w) {
			*which = w;
			return &aux_engines[i];
		}
	}
	return NULL;
}

/* The sink: native DPCD reads and writes, I2C over AUX to the EDID at 0x50
 * (a write sets the offset), the segment pointer at 0x30 acknowledged,
 * every other I2C address NACKed. */
static void aux_engine_go(struct aux_engine_model *m)
{
	u8 action = m->tx[0] & 0xf0;
	u32 addr = (m->tx[0] & 0xf) << 16 | m->tx[1] << 8 | m->tx[2];
	u32 len = m->idx > 3 ? m->tx[3] + 1 : 0;

	m->rx_n = 1;
	m->rx[0] = 0;
	if (!m->sink) {
		m->status = DP_AUX1_AUX_SW_STATUS__AUX_SW_DONE_MASK | DP_AUX1_AUX_SW_STATUS__AUX_SW_RX_TIMEOUT_MASK;
		printf("aux timeout action 0x%x addr 0x%x\n", action, addr);
		return;
	}
	if (action == I2CAUX_TRANSACTION_ACTION_DP_READ) {
		memcpy(m->rx + 1, sink_dpcd + addr, len);
		m->rx_n += len;
	} else if (action == I2CAUX_TRANSACTION_ACTION_DP_WRITE) {
		memcpy(sink_dpcd + addr, m->tx + 4, len);
	} else if (addr == 0x50) {
		if (!(action & I2CAUX_TRANSACTION_ACTION_I2C_READ)) {
			if (len)
				m->i2c_off = m->tx[4];
		} else {
			for (u32 i = 0; i < len; i++)
				m->rx[1 + i] = sink_edid[(m->i2c_off + i) & 0x7f];
			m->i2c_off += len;
			m->rx_n += len;
		}
	} else if (addr != 0x30) {
		m->rx[0] = AUX_TRANSACTION_REPLY_I2C_OVER_AUX_NACK << 4;
	}
	printf("aux action 0x%x addr 0x%x len %u reply 0x%x\n", action, addr, len, m->rx[0]);
	m->status = DP_AUX1_AUX_SW_STATUS__AUX_SW_DONE_MASK | m->rx_n << DP_AUX1_AUX_SW_STATUS__AUX_SW_REPLY_BYTE_COUNT__SHIFT;
}

static bool aux_engine_write(u32 reg, u32 val)
{
	int w;
	struct aux_engine_model *m = aux_engine_of(reg, &w);

	if (!m)
		return false;
	if (w == 1) {
		if (val & DP_AUX1_AUX_SW_DATA__AUX_SW_AUTOINCREMENT_DISABLE_MASK)
			m->idx = (val & DP_AUX1_AUX_SW_DATA__AUX_SW_INDEX_MASK) >> DP_AUX1_AUX_SW_DATA__AUX_SW_INDEX__SHIFT;
		m->rmode = val & DP_AUX1_AUX_SW_DATA__AUX_SW_DATA_RW_MASK;
		if (m->rmode)
			m->ridx = m->idx;
		else
			m->tx[m->idx++ & 31] = (val & DP_AUX1_AUX_SW_DATA__AUX_SW_DATA_MASK) >> DP_AUX1_AUX_SW_DATA__AUX_SW_DATA__SHIFT;
	} else if (w == 2 && (val & DP_AUX1_AUX_SW_CONTROL__AUX_SW_GO_MASK)) {
		set(reg, val & ~DP_AUX1_AUX_SW_CONTROL__AUX_SW_GO_MASK);
		aux_engine_go(m);
		return true;
	} else if (w == 3 && (val & DP_AUX1_AUX_INTERRUPT_CONTROL__AUX_SW_DONE_ACK_MASK)) {
		m->status &= ~DP_AUX1_AUX_SW_STATUS__AUX_SW_DONE_MASK;
		set(reg, val & ~DP_AUX1_AUX_INTERRUPT_CONTROL__AUX_SW_DONE_ACK_MASK);
		return true;
	}
	set(reg, val);
	return true;
}

static bool aux_engine_read(u32 reg, u32 *val)
{
	int w;
	struct aux_engine_model *m = aux_engine_of(reg, &w);

	if (!m)
		return false;
	int i = find(reg);
	u32 v = i < 0 ? 0 : reg_val[i];
	if (w == 1 && m->rmode)
		v = (v & ~DP_AUX1_AUX_SW_DATA__AUX_SW_DATA_MASK) |
		    m->rx[m->ridx++ & 31] << DP_AUX1_AUX_SW_DATA__AUX_SW_DATA__SHIFT;
	else if (w == 4)
		v = m->status;
	else if (w == 5)
		v = (v & ~DP_AUX1_AUX_ARB_CONTROL__AUX_REG_RW_CNTL_STATUS_MASK) |
		    1 << DP_AUX1_AUX_ARB_CONTROL__AUX_REG_RW_CNTL_STATUS__SHIFT;
	else if (w == 6)
		v = (v & ~DP_AUX1_AUX_CONTROL__AUX_RESET_DONE_MASK) |
		    (v & DP_AUX1_AUX_CONTROL__AUX_RESET_MASK ? DP_AUX1_AUX_CONTROL__AUX_RESET_DONE_MASK : 0);
	*val = v;
	return true;
}

/* An eDP 1.4 panel, 1920x1080 over two HBR lanes, backlight on PWM, no PSR,
 * and its EDID (base block only, checksum filled in). */
static const u8 panel_edid[128] = {
	0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x30, 0xe4, 0x2c, 0x05, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x1d, 0x01, 0x04, 0xa5, 0x22, 0x13, 0x78, 0x02, 0xa0, 0xf5, 0x95, 0x5b, 0x59, 0x93, 0x27,
	0x1d, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
	0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x3c, 0x37, 0x80, 0xde, 0x70, 0x38, 0x14, 0x40, 0x3c, 0x20,
	0x36, 0x00, 0x58, 0xc2, 0x10, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xfe, 0x00, 0x4c,
	0x47, 0x20, 0x44, 0x69, 0x73, 0x70, 0x6c, 0x61, 0x79, 0x0a, 0x20, 0x20, 0x00, 0x00, 0x00, 0xfe,
	0x00, 0x4c, 0x50, 0x31, 0x35, 0x36, 0x57, 0x46, 0x47, 0x2d, 0x53, 0x50, 0x42, 0x32, 0x00, 0x00,
};

static void panel_sink_init(void)
{
	u8 sum = 0;

	sink_dpcd[DP_DPCD_REV] = 0x14;
	sink_dpcd[DP_MAX_LINK_RATE] = DP_LINK_BW_2_7;
	sink_dpcd[DP_MAX_LANE_COUNT] = 2 | DP_ENHANCED_FRAME_CAP;
	sink_dpcd[DP_MAX_DOWNSPREAD] = DP_MAX_DOWNSPREAD_0_5;
	sink_dpcd[DP_EDP_CONFIGURATION_CAP] = DP_ALTERNATE_SCRAMBLER_RESET_CAP | DP_FRAMING_CHANGE_CAP;
	sink_dpcd[DP_EDP_DPCD_REV] = DP_EDP_14;
	sink_dpcd[DP_SET_POWER] = DP_SET_POWER_D0;
	memcpy(sink_edid, panel_edid, 128);
	for (int i = 0; i < 127; i++)
		sum += sink_edid[i];
	sink_edid[127] = -sum;
}
