//! The port's DC bring-up, printed as oracle/main_dchw.c prints Linux's:
//! amdgpu_dm_init up to dc_hardware_init on the VBIOS, then a DMUB service
//! attached and dc_hardware_init. Unwritten registers read hash / ones /
//! zeros / lit / boot; the VBIOS SMC answers every message. The third
//! argument is the oracle's output, whose `preset` lines are the registers
//! Linux's tables name (the boot image, the HPD levels of `detect`). Then, as amdgpu_dm_init goes on,
//! mmhub_read_system_context and dc_setup_system_context on the oracle's GMC
//! layout, with the GART table a VRAM buffer. Every register write is
//! printed; the DMUB commands are read back off the inbox ring, then the
//! state init_hw leaves. A fourth argument `detect` runs the link loop of
//! amdgpu_dm_initialize_drm_device against the AUX engines and eDP panel
//! of oracle/dp_sink.c, modelled here the same way.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use regs::*;

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

fn ones(_reg: u32) -> u32 {
    !0
}

fn zeros(_reg: u32) -> u32 {
    0
}

/// All ones, but no pipe fused off: every block reads as running.
fn lit(reg: u32) -> u32 {
    if reg == CC_DC_PIPE_DIS { 0 } else { !0 }
}

/// The DP AUX engines and the eDP panel behind the first link's (oracle/
/// dp_sink.c): a request is loaded through AUX_SW_DATA, AUX_SW_GO sends it,
/// the reply comes back through the same index once AUX_SW_DATA_RW is set.
#[derive(Default)]
struct AuxEngine {
    r: [u32; 6],
    sink: bool,
    tx: [u8; 32],
    rx: [u8; 32],
    idx: usize,
    ridx: usize,
    rx_n: usize,
    rmode: bool,
    status: u32,
    i2c_off: usize,
}

struct Sink {
    engines: Vec<AuxEngine>,
    dpcd: Vec<u8>,
    edid: [u8; 128],
}

thread_local! {
    static SINK: std::cell::RefCell<Option<Sink>> = const { std::cell::RefCell::new(None) };
}

const SW_DATA: usize = 0;
const SW_CONTROL: usize = 1;
const INTERRUPT_CONTROL: usize = 2;
const SW_STATUS: usize = 3;
const ARB_CONTROL: usize = 4;
const CONTROL: usize = 5;

fn aux_engine_of(s: &mut Sink, reg: u32) -> Option<(&mut AuxEngine, usize)> {
    s.engines.iter_mut().find_map(|e| e.r.iter().position(|&r| r == reg).map(|w| (e, w)))
}

fn aux_engine_go(m: &mut AuxEngine, dpcd: &mut [u8], edid: &[u8; 128]) {
    let action = (m.tx[0] & 0xf0) as u32;
    let addr = ((m.tx[0] & 0xf) as usize) << 16 | (m.tx[1] as usize) << 8 | m.tx[2] as usize;
    let len = if m.idx > 3 { m.tx[3] as usize + 1 } else { 0 };

    m.rx_n = 1;
    m.rx[0] = 0;
    if !m.sink {
        m.status = DP_AUX0_AUX_SW_STATUS__AUX_SW_DONE_MASK | AUX_SW_STATUS__AUX_SW_RX_TIMEOUT_MASK;
        println!("aux timeout action {:#x} addr {:#x}", action, addr);
        return;
    }
    if action == dc_ddc_types::I2CAUX_TRANSACTION_ACTION_DP_READ {
        m.rx[1..1 + len].copy_from_slice(&dpcd[addr..addr + len]);
        m.rx_n += len;
    } else if action == dc_ddc_types::I2CAUX_TRANSACTION_ACTION_DP_WRITE {
        dpcd[addr..addr + len].copy_from_slice(&m.tx[4..4 + len]);
    } else if addr == 0x50 {
        if action & dc_ddc_types::I2CAUX_TRANSACTION_ACTION_I2C_READ == 0 {
            if len != 0 {
                m.i2c_off = m.tx[4] as usize;
            }
        } else {
            for i in 0..len {
                m.rx[1 + i] = edid[(m.i2c_off + i) & 0x7f];
            }
            m.i2c_off += len;
            m.rx_n += len;
        }
    } else if addr != 0x30 {
        m.rx[0] = dc_ddc_types::AUX_TRANSACTION_REPLY_I2C_OVER_AUX_NACK << 4;
    }
    println!("aux action {:#x} addr {:#x} len {} reply {:#x}", action, addr, len, m.rx[0]);
    m.status = DP_AUX0_AUX_SW_STATUS__AUX_SW_DONE_MASK | (m.rx_n as u32) << DP_AUX0_AUX_SW_STATUS__AUX_SW_REPLY_BYTE_COUNT__SHIFT;
}

/// The engine's side of a write the card already stored.
fn aux_engine_write(reg: u32, val: u32) {
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        let Some(s) = s.as_mut() else { return };
        let Sink { engines, dpcd, edid } = s;
        let Some((m, w)) = engines.iter_mut().find_map(|e| e.r.iter().position(|&r| r == reg).map(|w| (e, w)))
        else {
            return;
        };
        match w {
            SW_DATA => {
                if val & DP_AUX0_AUX_SW_DATA__AUX_SW_AUTOINCREMENT_DISABLE_MASK != 0 {
                    m.idx = ((val & DP_AUX0_AUX_SW_DATA__AUX_SW_INDEX_MASK) >> DP_AUX0_AUX_SW_DATA__AUX_SW_INDEX__SHIFT) as usize;
                }
                m.rmode = val & DP_AUX0_AUX_SW_DATA__AUX_SW_DATA_RW_MASK != 0;
                if m.rmode {
                    m.ridx = m.idx;
                } else {
                    m.tx[m.idx & 31] = ((val & DP_AUX0_AUX_SW_DATA__AUX_SW_DATA_MASK) >> DP_AUX0_AUX_SW_DATA__AUX_SW_DATA__SHIFT) as u8;
                    m.idx += 1;
                }
            }
            SW_CONTROL if val & DP_AUX0_AUX_SW_CONTROL__AUX_SW_GO_MASK != 0 => {
                npk_sys::fake_set_reg(reg, val & !DP_AUX0_AUX_SW_CONTROL__AUX_SW_GO_MASK);
                aux_engine_go(m, dpcd, edid);
            }
            INTERRUPT_CONTROL if val & DP_AUX0_AUX_INTERRUPT_CONTROL__AUX_SW_DONE_ACK_MASK != 0 => {
                m.status &= !DP_AUX0_AUX_SW_STATUS__AUX_SW_DONE_MASK;
                npk_sys::fake_set_reg(reg, val & !DP_AUX0_AUX_INTERRUPT_CONTROL__AUX_SW_DONE_ACK_MASK);
            }
            _ => {}
        }
    });
}

fn aux_engine_read(reg: u32) -> Option<u32> {
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        let (m, w) = aux_engine_of(s.as_mut()?, reg)?;
        let v = npk_sys::fake_reg(reg);
        Some(match w {
            SW_DATA if m.rmode => {
                let b = m.rx[m.ridx & 31];
                m.ridx += 1;
                (v & !DP_AUX0_AUX_SW_DATA__AUX_SW_DATA_MASK) | (b as u32) << DP_AUX0_AUX_SW_DATA__AUX_SW_DATA__SHIFT
            }
            SW_STATUS => m.status,
            ARB_CONTROL => {
                (v & !DP_AUX0_AUX_ARB_CONTROL__AUX_REG_RW_CNTL_STATUS_MASK)
                    | 1 << DP_AUX0_AUX_ARB_CONTROL__AUX_REG_RW_CNTL_STATUS__SHIFT
            }
            CONTROL => {
                (v & !DP_AUX0_AUX_CONTROL__AUX_RESET_DONE_MASK)
                    | if v & DP_AUX0_AUX_CONTROL__AUX_RESET_MASK != 0 { DP_AUX0_AUX_CONTROL__AUX_RESET_DONE_MASK } else { 0 }
            }
            _ => v,
        })
    })
}

/// An eDP 1.4 panel, 1920x1080 over two HBR lanes, backlight on PWM, no PSR
/// (oracle/dp_sink.c, byte for byte).
const PANEL_EDID: [u8; 128] = [
    0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x30, 0xe4, 0x2c, 0x05, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x1d, 0x01, 0x04, 0xa5, 0x22, 0x13, 0x78, 0x02, 0xa0, 0xf5, 0x95, 0x5b, 0x59, 0x93, 0x27,
    0x1d, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x3c, 0x37, 0x80, 0xde, 0x70, 0x38, 0x14, 0x40, 0x3c, 0x20,
    0x36, 0x00, 0x58, 0xc2, 0x10, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xfe, 0x00, 0x4c,
    0x47, 0x20, 0x44, 0x69, 0x73, 0x70, 0x6c, 0x61, 0x79, 0x0a, 0x20, 0x20, 0x00, 0x00, 0x00, 0xfe,
    0x00, 0x4c, 0x50, 0x31, 0x35, 0x36, 0x57, 0x46, 0x47, 0x2d, 0x53, 0x50, 0x42, 0x32, 0x00, 0x00,
];

fn panel_sink(panel_engine: usize) -> Sink {
    let mut dpcd = vec![0u8; 0x100000];
    dpcd[DP_DPCD_REV as usize] = 0x14;
    dpcd[DP_MAX_LINK_RATE as usize] = 0x0a; // DP_LINK_BW_2_7
    dpcd[DP_MAX_LANE_COUNT as usize] = 2 | 0x80; // DP_ENHANCED_FRAME_CAP
    dpcd[DP_MAX_DOWNSPREAD as usize] = 0x01; // DP_MAX_DOWNSPREAD_0_5
    dpcd[DP_EDP_CONFIGURATION_CAP as usize] = 0x01 | 0x02; // alternate scrambler reset, framing change
    dpcd[DP_EDP_DPCD_REV as usize] = 0x03; // DP_EDP_14
    dpcd[DP_SET_POWER as usize] = DP_SET_POWER_D0 as u8;
    let mut edid = PANEL_EDID;
    let sum = edid[..127].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    edid[127] = sum.wrapping_neg();
    let engines = dce_aux::AUX_REGS.iter().enumerate()
        .map(|(i, r)| AuxEngine {
            r: [r.AUX_SW_DATA, r.AUX_SW_CONTROL, r.AUX_INTERRUPT_CONTROL, r.AUX_SW_STATUS, r.AUX_ARB_CONTROL,
                r.AUX_CONTROL],
            sink: i == panel_engine,
            ..Default::default()
        })
        .collect();
    Sink { engines, dpcd, edid }
}

/// The VBIOS SMC answers every message OK with the SMU version; the DMCUB
/// consumes the inbox ring as soon as its write pointer moves.
fn device(reg: u32, v: u32) {
    aux_engine_write(reg, v);
    if reg == RN_VBIOS_MP1_SMN_C2PMSG_67 {
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_83, 0x373700);
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_91, 1);
    }
    if reg == DMUB_DMCUB_INBOX1_WPTR {
        npk_sys::fake_set_reg(DMUB_DMCUB_INBOX1_RPTR, v);
    }
}

fn print_range(n: &str, i: u32, r: &dm_pp_smu::pp_smu_wm_set_range) {
    println!(
        "  {} {} inst {} type {} fill {} {} drain {} {}",
        n, i, r.wm_inst, r.wm_type, r.min_fill_clk_mhz, r.max_fill_clk_mhz, r.min_drain_clk_mhz, r.max_drain_clk_mhz
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let bios = std::fs::read(args.next().expect("vbios.bin")).expect("read");
    let mode = args.next().unwrap_or_default();
    let rule: fn(u32) -> u32 = match mode.as_str() {
        "ones" => ones,
        "zeros" => zeros,
        "lit" | "boot" => lit,
        _ => hash,
    };
    let oracle_out = args.next();
    let detect = args.next().as_deref() == Some("detect");
    let presets: Vec<(u32, u32)> = if let Some(path) = oracle_out {
        let text = std::fs::read_to_string(path).expect("read");
        text.lines()
            .filter_map(|l| l.strip_prefix("preset reg 0x"))
            .map(|l| {
                let (r, v) = l.split_once(" = 0x").expect("preset line");
                (u32::from_str_radix(r, 16).unwrap(), u32::from_str_radix(v, 16).unwrap())
            })
            .collect()
    } else {
        Vec::new()
    };

    npk_sys::fake_card(npk_sys::FakeCard {
        regs: Default::default(),
        mm_index: !0,
        mm_index_hi: !0,
        mm_data: !0,
        vram_base: 0,
        vram: Vec::new(),
        on_write: Vec::new(),
        writes: Vec::new(),
        psp_wptr: None,
    });
    npk_sys::fake_card_unset(rule, (!0 - 1, !0 - 2));
    npk_sys::fake_write_hook(device);
    npk_sys::fake_read_hook(aux_engine_read);
    npk_sys::fake_card_echo_skip(&[DMUB_DMCUB_INBOX1_WPTR]);
    npk_sys::quiet(true);

    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0x100000 * 4);
    adev.subsystem_vendor = 0x17aa;
    adev.subsystem_device = 0x3f1a;
    adev.asic_type = 24;
    adev.flags = 0x20000;
    adev.dm.dmcub_fw_version = 0x0101002b;
    adev.ip_versions[DCE_HWIP as usize][0] = 0x02010000;
    adev.family = 142;
    adev.external_rev_id = 0x91;
    adev.gmc.vram_width = 64;
    adev.dm.cgs_device = true;
    adev.reg_offset[DCE_HWIP as usize][0] = vec![0x12, 0xc0, 0x34c0, 0x9000, 0x2403c00];
    adev.reg_offset[NBIO_HWIP as usize][0] = vec![0, 0x14, 0xd20, 0x10400, 0x241b000];
    adev.reg_offset[CLK_HWIP as usize][0] = vec![0x16c00, 0x16e00, 0x17000, 0x17200, 0x1b000];
    adev.bios = bios;
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());
    adev.smu.pm_enabled = true;
    adev.apu_flags = AMD_APU_IS_RENOIR;
    adev.gmc.vram_start = 0xf4_0000_0000;
    adev.gmc.fb_start = 0xf4_0000_0000;
    adev.gmc.fb_end = 0xf4_0000_0000 + 0x2000_0000 - 1;
    adev.gmc.gart_start = 0;
    adev.gmc.gart_end = 0x4000_0000 - 1;
    adev.gmc.agp_start = 0xf8_0000_0000;
    adev.gmc.agp_end = 0xffff_ffff_ffff;
    adev.vm_manager.vram_base_offset = 0x6000_0000;
    adev.mman.bos.push(Some(amdgpu_object::amdgpu_bo {
        tbo: ttm::ttm_buffer_object {
            resource: Some(ttm::ttm_resource { start: 0x1ff00, size: 0x100000, mem_type: TTM_PL_VRAM,
                ..Default::default() }),
            ..Default::default()
        },
        flags: 0,
        preferred_domains: 0,
        allowed_domains: 0,
        placements: Vec::new(),
        xcp_id: 0,
        kptr: None,
        ref_count: 1,
        parent: None,
        vm_bo: None,
        _resv: Default::default(),
        resv: None,
        deleted: false,
        entries: Vec::new(),
        client: None,
        pool: None,
        tiling_flags: 0,
        metadata_flags: 0,
        metadata: Vec::new(),
    }));
    adev.gart.bo = Some(adev.mman.bos.len() - 1);

    npk_sys::fake_card_echo(true);
    println!("amdgpu_dm_init {}", amdgpu_dm::amdgpu_dm_init(&mut adev));
    let mut dc = adev.dm.dc.take().expect("dc");
    for &(r, v) in &presets {
        println!("preset reg {:#x} = {:#x}", r, v);
        npk_sys::fake_set_reg(r, v);
    }

    let ring = npk_sys::Dma::alloc((DMUB_RB_SIZE / 4096) as i32).expect("dma");
    let mut dmub = Box::new(dmub_srv::dmub_srv::default());
    dmub_srv::dmub_srv_create(&mut dmub, DMUB_ASIC_DCN21, 0);
    dmub.hw_init = true;
    dmub.power_state = DMUB_POWER_STATE_D0;
    dmub.cpu = Some(amdgpu_object::amdgpu_kptr::gtt(ring.handle));
    dmub.inbox1.rb.capacity = DMUB_RB_SIZE;
    dc.ctx.dmub_srv = dc_dmub_srv::dc_dmub_srv_create(dmub);

    println!("dc_hardware_init");
    dc::dc_hardware_init(&adev, &mut dc);

    println!("dc_setup_system_context");
    if adev.flags & AMD_IS_APU != 0 && adev.asic_type >= CHIP_CARRIZO {
        let pa_config = amdgpu_dm::mmhub_read_system_context(&adev);
        let a = &pa_config.system_aperture;
        println!(
            "pa_config aperture {:#x} {:#x} fb {:#x} {:#x} {:#x} agp {:#x} {:#x} {:#x}",
            a.start_addr, a.end_addr, a.fb_base, a.fb_top, a.fb_offset, a.agp_base, a.agp_bot, a.agp_top
        );
        let g = &pa_config.gart_config;
        println!(
            "pa_config gart {:#x} {:#x} {:#x} hvm {}",
            g.page_table_start_addr, g.page_table_end_addr, g.page_table_base_addr, pa_config.is_hvm_enabled as u32
        );

        println!("num_vmids {}", dc::dc_setup_system_context(&adev, &mut dc, &pa_config));
        println!(
            "riommu_active {} vm_pa_config valid {}",
            dc.res_pool.as_deref().unwrap().hubbub.as_deref().unwrap().base.riommu_active as u32,
            dc.vm_pa_config.valid as u32
        );
    }
    // amdgpu_dm_initialize_drm_device's loop over the links, on the eDP
    // panel with every other HPD low (the presets above).
    if detect {
        let panel_engine = dc.links[0].ddc_hw_inst as usize;
        SINK.with(|s| *s.borrow_mut() = Some(panel_sink(panel_engine)));
        for l in 0..dc.link_count as usize {
            if dc.links[l].connector_signal == LF_SIGNAL_TYPE_VIRTUAL {
                continue;
            }
            amdgpu_dm_helpers::amdgpu_dm_connector_init(&mut dc, l);
            println!("detect link {}", l);
            let mut new_connection_type = dc_connection_none;
            if !link_detection::dc_link_detect_connection_type(&adev, &mut dc, l, &mut new_connection_type) {
                println!("KMS: Failed to detect connector");
            }
            println!("dc_link_detect {}",
                link_detection::dc_link_detect(&adev, &mut dc, l, link_detection::dc_detect_reason::DETECT_REASON_BOOT)
                    as u32);
        }
        for (l, link) in dc.links.iter().enumerate() {
            let sink = link.local_sink.as_deref();
            print!("link {} type {} signal {}", l, link.r#type, sink.map_or(-1, |s| s.sink_signal as i32));
            println!(" cap lanes {} rate {} spread {} verified lanes {} rate {}", link.reported_link_cap.lane_count,
                link.reported_link_cap.link_rate, link.reported_link_cap.link_spread,
                link.verified_link_cap.lane_count, link.verified_link_cap.link_rate);
            let Some(sink) = sink else { continue };
            let c = &sink.edid_caps;
            let name_len = c.display_name.iter().position(|&b| b == 0).unwrap_or(c.display_name.len());
            println!("  sink edid {} mfg {:#x} prod {:#x} serial {:#x} week {} year {} name '{}' hdmi {} audio {} speakers {:#x}",
                sink.dc_edid.length, c.manufacturer_id, c.product_id, c.serial_number, c.manufacture_week,
                c.manufacture_year, String::from_utf8_lossy(&c.display_name[..name_len]), c.edid_hdmi as u32,
                c.audio_mode_count, c.speaker_flags);
            println!("  dpcd rev {:#x} lanes {:#x} spread {:#x} ext_caps {:#x} sink_count {} branch {}",
                link.dpcd_caps.dpcd_rev, link.dpcd_caps.max_ln_count, link.dpcd_caps.max_down_spread,
                link.dpcd_sink_ext_caps, link.dpcd_caps.sink_count, link.dpcd_caps.is_branch_dev as u32);
        }
    }
    npk_sys::fake_card_echo(false);

    let wrpt = dc.ctx.dmub_srv.as_deref().unwrap().dmub.as_deref().unwrap().inbox1.rb.wrpt;
    let mem = npk_sys::window_bytes(ring.handle);
    for slot in mem[..wrpt as usize].chunks(DMUB_RB_CMD_SIZE as usize) {
        print!("dmub cmd");
        for b in slot {
            print!(" {:02x}", b);
        }
        println!();
    }

    let ranges = &dc.clk_mgr.as_deref().unwrap().base.ranges;
    println!("wm_ranges reader {} writer {}", ranges.num_reader_wm_sets, ranges.num_writer_wm_sets);
    for i in 0..ranges.num_reader_wm_sets {
        print_range("reader", i, &ranges.reader_wm_sets[i as usize]);
    }
    for i in 0..ranges.num_writer_wm_sets {
        print_range("writer", i, &ranges.writer_wm_sets[i as usize]);
    }
    let pool = dc.res_pool.as_deref().unwrap();
    println!(
        "ref_clocks xtalin {} dccg {} dchub {}",
        pool.ref_clocks.xtalin_clock_inKhz, pool.ref_clocks.dccg_ref_clock_inKhz, pool.ref_clocks.dchub_ref_clock_inKhz
    );
    for (l, k) in dc.links.iter().enumerate() {
        println!(
            "link {} sink_present {} active {} fec {}",
            l, k.edp_sink_present as u32, k.link_status.link_active as u32, k.fec_state
        );
    }
}
