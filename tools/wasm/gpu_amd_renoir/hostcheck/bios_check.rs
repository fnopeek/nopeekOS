//! The port's DC VBIOS parser on a VBIOS image, printed as
//! oracle/main_bios.c prints Linux's bios_parser2_construct.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use bios_parser2::{bp_init_data, dal_bios_parser_create};
use dc_types::{dce_version, graphics_object_id, i2c_reg_info};

fn gid(n: &str, g: graphics_object_id) {
    println!("{} id {} enum {} type {}", n, g.id, g.enum_id as u32, g.r#type as u32);
}

fn retimer(dp: u32, slv: u8, num: u8, r: &[i2c_reg_info], num6: u8, r6: &[i2c_reg_info]) {
    println!("dp{} slv {} num {} num6 {}", dp, slv, num, num6);
    for (i, x) in r.iter().enumerate() {
        println!("dp{} reg {} {} {}", dp, i, x.i2c_reg_index, x.i2c_reg_val);
    }
    for (i, x) in r6.iter().enumerate() {
        println!("dp{} reg6 {} {} {}", dp, i, x.i2c_reg_index, x.i2c_reg_val);
    }
}

fn name<T: core::fmt::Debug>(v: Option<T>) -> String {
    v.map_or("NULL".into(), |v| format!("{:?}", v))
}

fn main() {
    let bios = std::fs::read(std::env::args().nth(1).expect("vbios.bin")).expect("read");
    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0);
    adev.bios = bios;
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());

    let init = bp_init_data { bios: &adev.bios };
    let bp = dal_bios_parser_create(&adev, &init, dce_version::DCN_VERSION_2_1);
    println!("construct {}", bp.is_some() as u32);
    let Some(bp) = bp else { return };
    println!("bios_size {}", bp.base.bios_size);
    println!("master_data_tbl {:x}", bp.master_data_tbl);
    let t = bp.object_info_tbl.tbl.unwrap_or(0);
    println!(
        "object_info_tbl_offset {:x} rev {}.{} v1_4 {:x} v1_5 {:x}",
        bp.object_info_tbl_offset, bp.object_info_tbl.revision.major, bp.object_info_tbl.revision.minor, t, t
    );
    let c = &bp.cmd_tbl;
    println!("dig_encoder_control {}", name(c.dig_encoder_control));
    println!("transmitter_control {}", name(c.transmitter_control));
    println!("set_pixel_clock {}", name(c.set_pixel_clock));
    println!("set_crtc_timing {}", name(c.set_crtc_timing));
    println!("enable_crtc {}", name(c.enable_crtc));
    println!("external_encoder_control {}", name(c.external_encoder_control));
    println!("enable_disp_power_gating {}", name(c.enable_disp_power_gating));
    println!("set_dce_clock {}", name(c.set_dce_clock));
    println!("get_smu_clock_info {}", name(c.get_smu_clock_info));
    println!("enable_lvtma_control {}", name(c.enable_lvtma_control));
    println!(
        "cmd_helper {}",
        match bp.cmd_helper {
            Some(command_table2::command_table_helper_id::dal_cmd_tbl_helper_dce112_get_table2) => "&helper_dce112_2",
            None => "NULL",
        }
    );

    let fw = &bp.base.fw_info;
    println!("fw_info_valid {}", bp.base.fw_info_valid as u32);
    println!(
        "pll {} {} {} {} {}",
        fw.pll_info.crystal_frequency,
        fw.pll_info.min_input_pxl_clk_pll_frequency,
        fw.pll_info.max_input_pxl_clk_pll_frequency,
        fw.pll_info.min_output_pxl_clk_pll_frequency,
        fw.pll_info.max_output_pxl_clk_pll_frequency
    );
    println!("feature {} {}", fw.feature.memory_clk_ss_percentage, fw.feature.engine_clk_ss_percentage);
    println!(
        "fw {} {} {} {} {} {} {} {} {} {} {}",
        fw.default_display_engine_pll_frequency,
        fw.external_clock_source_frequency_for_dp,
        fw.smu_gpu_pll_output_freq,
        fw.min_allowed_bl_level,
        fw.remote_display_config,
        fw.default_memory_clk,
        fw.default_engine_clk,
        fw.dp_phy_ref_clk,
        fw.i2c_engine_ref_clk,
        fw.oem_i2c_present as u32,
        fw.oem_i2c_obj_id
    );
    println!("vram_info {} {}", bp.base.vram_info.num_chans, bp.base.vram_info.dram_channel_width_bytes);

    println!("integrated_info {}", bp.base.integrated_info.is_some() as u32);
    let Some(ii) = bp.base.integrated_info.as_deref() else { return };
    for (i, v) in ii.disp_clk_voltage.iter().enumerate() {
        println!("disp_clk_voltage {} {} {}", i, v.voltage_index, v.max_supported_clk);
    }
    for (i, p) in ii.ext_disp_conn_info.path.iter().enumerate() {
        println!(
            "path {} tag {} acpi {} aux {} hpd {} map {} caps {}",
            i, p.device_tag, p.device_acpi_enum, p.ext_aux_ddc_lut_index, p.ext_hpd_pin_lut_index,
            p.channel_mapping, p.caps
        );
        gid("  connector", p.device_connector_id);
        gid("  encoder", p.ext_encoder_obj_id);
    }
    print!("guid");
    for g in ii.ext_disp_conn_info.gu_id {
        print!(" {}", g);
    }
    println!();
    println!(
        "checksum {} fixdpvoltageswing {}",
        ii.ext_disp_conn_info.checksum, ii.ext_disp_conn_info.fixdpvoltageswing
    );
    println!(
        "mem {} {} gpu_cap {} system_config {} cpu_cap {}",
        ii.memory_type, ii.ma_channel_number, ii.gpu_cap_info, ii.system_config, ii.cpu_cap_info
    );
    println!(
        "ss lvds {} {} hdmi {} {} dvi {} {} dp_ss_control {} lvds_misc {}",
        ii.lvds_ss_percentage,
        ii.lvds_sspread_rate_in_10hz,
        ii.hdmi_ss_percentage,
        ii.hdmi_sspread_rate_in_10hz,
        ii.dvi_ss_percentage,
        ii.dvi_sspread_rate_in_10_hz,
        ii.dp_ss_control,
        ii.lvds_misc
    );
    retimer(0, ii.dp0_ext_hdmi_slv_addr, ii.dp0_ext_hdmi_reg_num, &ii.dp0_ext_hdmi_reg_settings,
        ii.dp0_ext_hdmi_6g_reg_num, &ii.dp0_ext_hdmi_6g_reg_settings);
    retimer(1, ii.dp1_ext_hdmi_slv_addr, ii.dp1_ext_hdmi_reg_num, &ii.dp1_ext_hdmi_reg_settings,
        ii.dp1_ext_hdmi_6g_reg_num, &ii.dp1_ext_hdmi_6g_reg_settings);
    retimer(2, ii.dp2_ext_hdmi_slv_addr, ii.dp2_ext_hdmi_reg_num, &ii.dp2_ext_hdmi_reg_settings,
        ii.dp2_ext_hdmi_6g_reg_num, &ii.dp2_ext_hdmi_6g_reg_settings);
    retimer(3, ii.dp3_ext_hdmi_slv_addr, ii.dp3_ext_hdmi_reg_num, &ii.dp3_ext_hdmi_reg_settings,
        ii.dp3_ext_hdmi_6g_reg_num, &ii.dp3_ext_hdmi_6g_reg_settings);
    let e = &ii.edp1_info;
    println!(
        "edp1 {} {} {} {} {} {} {} {}",
        e.edp_backlight_pwm_hz, e.edp_ss_percentage, e.edp_ss_rate_10hz, e.edp_pwr_on_off_delay,
        e.edp_pwr_on_vary_bl_to_blon, e.edp_pwr_down_bloff_to_vary_bloff, e.edp_panel_bpc, e.edp_bootup_bl_level
    );
    println!("gpuclk_ss {} {}", ii.gpuclk_ss_percentage, ii.gpuclk_ss_type);
}
