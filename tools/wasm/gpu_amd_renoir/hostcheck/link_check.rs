//! The port's links, printed as oracle/main_link.c prints Linux's:
//! create_links with one virtual link on the VBIOS, every register write,
//! the pool's encoder slots, then every link and its encoder. Unwritten
//! registers read hash(reg).

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use bios_parser2::{bp_init_data, dal_bios_parser_create};
use dc_types::{dce_environment, dce_version, graphics_object_id};

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

fn gid(n: &str, g: graphics_object_id) {
    println!("{} id {} enum {} type {}", n, g.id, g.enum_id as u32, g.r#type as u32);
}

fn main() {
    let bios = std::fs::read(std::env::args().nth(1).expect("vbios.bin")).expect("read");
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
    npk_sys::fake_card_unset(hash, (!0 - 1, !0 - 2));
    npk_sys::fake_card_echo(true);

    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0x100000 * 4);
    adev.bios = bios;
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());

    let mut dc = Box::new(dc::dc::default());
    let init = bp_init_data { bios: &adev.bios };
    dc.ctx.dc_bios = dal_bios_parser_create(&adev, &init, dce_version::DCN_VERSION_2_1).expect("bios parser");
    dc.ctx.gpio_service =
        gpio_service::dal_gpio_service_create(dce_version::DCN_VERSION_2_1, dce_environment::DCE_ENV_PRODUCTION_DRV);
    dc.caps.is_apu = true;
    dc.res_pool = Some(Box::new(dc_resource::resource_pool::default()));

    let ok = dc::create_links(&adev, &mut dc, 1);
    let pool = dc.res_pool.as_deref().unwrap();
    println!(
        "create_links {} link_count {} internal {} dig {} edp {} lowest_dpia {}",
        ok as u32, dc.link_count, dc.caps.num_of_internal_disp, pool.dig_link_enc_count, dc.ctx.dc_edp_id_count,
        dc.lowest_dpia_link_index
    );
    for (i, e) in pool.link_encoders.iter().enumerate() {
        println!("link_encoders {} {}", i, e.map_or(-1, |l| l as i32));
    }
    for (l, k) in dc.links.iter().enumerate() {
        print!(
            "link {} index {} ep {} internal {} signal {:#x}",
            l, k.link_index, k.ep_type, k.is_internal_display as u32, k.connector_signal
        );
        gid("", k.link_id);
        print!(
            "  irq {} {} {} hpd_gpio {}",
            k.irq_source_hpd, k.irq_source_hpd_rx, k.irq_source_read_request, k.hpd_gpio.is_some() as u32
        );
        if let Some(h) = k.hpd_gpio.as_deref() {
            print!(" id {} en {} mode {}", h.id.map_or(-1, |x| x as i32), h.en, h.mode as i32);
        }
        print!(
            "\n  ddc {} hw_inst {} hpd_src {} eng {} enc_hw_inst {} panel {}",
            k.ddc.is_some() as u32, k.ddc_hw_inst, k.hpd_src as i32, k.eng_id as i32, k.link_enc_hw_inst,
            k.panel_cntl.is_some() as u32
        );
        if let Some(p) = k.panel_cntl.as_deref() {
            print!(" inst {} pwrseq {}", p.inst, p.pwrseq_inst);
        }
        println!(
            "\n  tag acpi {} type {} enum {} raw {} map {} caps {} drive {} {} psr {} {}",
            k.device_tag.acpi_device, k.device_tag.dev_id.device_type as u32, k.device_tag.dev_id.enum_id,
            k.device_tag.dev_id.raw_device_tag, k.ddi_channel_mapping, k.chip_caps,
            k.bios_forced_drive_settings.VOLTAGE_SWING, k.bios_forced_drive_settings.PRE_EMPHASIS,
            k.psr_settings.psr_vtotal_control_support as u32, k.psr_settings.psr_version as i32
        );
        if let Some(e) = k.link_enc.as_ref().map(|e| e.base()) {
            print!(
                "  enc output {:#x} engine {} transmitter {} hpd {} flags {:#x} color {} clock {} 420 {} {} fec {}",
                e.output_signals, e.preferred_engine as i32, e.transmitter as i32, e.hpd_source as i32,
                e.features.flags, e.features.max_hdmi_deep_color, e.features.max_hdmi_pixel_clock,
                e.features.hdmi_ycbcr420_supported as u32, e.features.dp_ycbcr420_supported as u32,
                e.features.fec_supported as u32
            );
            gid(" id", e.id);
            gid("  connector", e.connector);
        }
    }
}
