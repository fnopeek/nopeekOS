//! The port's GPIO pins, printed as oracle/main_gpio.c prints Linux's: per
//! connector the HPD GPIO opened in interrupt mode and unlocked, its IRQ
//! sources and the DDC service; every translate entry; a busy pin's second
//! open and the HPD pins closed. Unwritten registers read hash(reg).

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use bios_parser2::*;
use dc_types::{dce_environment, dce_version};
use crate::gpio_base::{dal_gpio_close, dal_gpio_get_mode, dal_gpio_lock_pin, dal_gpio_open, dal_gpio_unlock_pin, gpio, gpio_mode};
use crate::gpio_service::{dal_gpio_create_irq, dal_gpio_get_generic_pin_info, dal_gpio_service_create, dal_irq_get_read_request, dal_irq_get_rx_source, dal_irq_get_source, gpio_id};

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
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
    let init = bp_init_data { bios: &adev.bios };
    let bp = dal_bios_parser_create(&adev, &init, dce_version::DCN_VERSION_2_1).expect("bios parser");
    let b = &adev.bios[..];
    let mut gs = dal_gpio_service_create(dce_version::DCN_VERSION_2_1, dce_environment::DCE_ENV_PRODUCTION_DRV)
        .expect("gpio service");

    let mut hpd: [Option<Box<gpio>>; 8] = Default::default();
    for i in 0..8u8 {
        let c = bios_parser_get_connector_id(&bp, b, i);
        if c.id == 0 {
            continue;
        }
        println!("connector {} id {}", i, c.id);
        hpd[i as usize] = link_hpd::link_get_hpd_gpio(&bp, b, c, &gs);
        if let Some(h) = hpd[i as usize].as_deref_mut() {
            println!("  hpd id {} en {}", h.id.map_or(-1, |x| x as i32), h.en);
            let r = dal_gpio_open(&adev, &mut gs, h, gpio_mode::GPIO_MODE_INTERRUPT);
            println!("  open {}", r as u32);
            println!("  unlock {}", dal_gpio_unlock_pin(&mut gs, h) as u32);
            println!("  irq {} rx {} rr {}", dal_irq_get_source(h), dal_irq_get_rx_source(h), dal_irq_get_read_request(h));
        } else {
            println!("  hpd none");
        }
        let di = link_ddc::ddc_service_init_data { id: c, has_link: true, is_dpia_link: false };
        let ddc = link_ddc::link_create_ddc_service(&bp, b, &gs, &di);
        print!("  ddc pin {}", ddc.ddc_pin.is_some() as u32);
        if let Some(p) = ddc.ddc_pin.as_deref() {
            print!(" line {} clock {} hw {} channel {}", p.pin_data.en, p.pin_clock.en, p.hw_info.hw_supported as u32,
                p.hw_info.ddc_channel);
        }
        println!(" internal {}", ddc.flags.IS_INTERNAL_DISPLAY as u32);
    }

    let ids = [
        gpio_id::GPIO_ID_DDC_DATA, gpio_id::GPIO_ID_DDC_CLOCK, gpio_id::GPIO_ID_GENERIC, gpio_id::GPIO_ID_HPD,
        gpio_id::GPIO_ID_GPIO_PAD, gpio_id::GPIO_ID_VIP_PAD, gpio_id::GPIO_ID_SYNC, gpio_id::GPIO_ID_GSL,
    ];
    for id in ids {
        for en in 0..8 {
            let p = dal_gpio_get_generic_pin_info(&gs, Some(id), en);
            println!(
                "pin {} {}: {} {} {} {} {} {} {} {}",
                id as u32, en, p.offset, p.offset_y, p.offset_en, p.offset_mask, p.mask, p.mask_y, p.mask_en, p.mask_mask
            );
        }
    }

    // A busy pin refuses a second open; locking and closing free it.
    for i in 0..8 {
        let Some(h) = hpd[i].as_deref_mut() else { continue };
        let mut again = dal_gpio_create_irq(Some(gpio_id::GPIO_ID_HPD), h.en).expect("irq");
        println!("lock {}", dal_gpio_lock_pin(&mut gs, h) as u32);
        println!("busy open {}", dal_gpio_open(&adev, &mut gs, &mut again, gpio_mode::GPIO_MODE_INPUT) as u32);
        dal_gpio_close(&adev, &mut gs, h);
        let r = dal_gpio_open(&adev, &mut gs, &mut again, gpio_mode::GPIO_MODE_INPUT);
        println!("free open {} mode {}", r as u32, dal_gpio_get_mode(&again) as i32);
        dal_gpio_close(&adev, &mut gs, &mut again);
    }
}
