// The device as the host checks fake it, include!d by them.

const VRAM_MB: u32 = 512;
const BDF: (u8, u8, u8) = (4, 0, 0);

fn put16(b: &mut [u8], at: u32, v: u16) {
    b[at as usize..at as usize + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: u32, v: u32) {
    b[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
}

/// The fake card, ACPI and firmware of the device, and its `adev` with BAR5
/// as `_start` maps it; the arguments are ip_discovery.bin, vbios.bin and
/// the firmware directory.
fn fake_renoir() -> amdgpu_device::amdgpu_device {
    let disc = std::fs::read(std::env::args().nth(1).expect("ip_discovery.bin")).expect("read");
    let bios = std::fs::read(std::env::args().nth(2).expect("vbios.bin")).expect("read");
    npk_sys::fake_firmware_dir(&std::env::args().nth(3).expect("firmware dir"));

    let vram_size = (VRAM_MB as u64) << 20;
    let mut regs = std::collections::HashMap::new();
    regs.insert(mmRCC_CONFIG_MEMSIZE, VRAM_MB);
    regs.insert(mmMP0_SMN_C2PMSG_33, 0x8000_0000);
    // BIOS scratch 7 (atomfirmware bios_scratch_reg_startaddr 0x4c + 7) as
    // a posted APU leaves it; the card run reads the real one.
    regs.insert(0x4c + 7, ATOM_S7_ASIC_INIT_COMPLETE_MASK);
    let mut vram = disc;
    vram.resize(DISCOVERY_TMR_SIZE as usize, 0);
    npk_sys::fake_card(npk_sys::FakeCard {
        regs,
        mm_index: mmMM_INDEX,
        mm_index_hi: mmMM_INDEX_HI,
        mm_data: mmMM_DATA,
        vram_base: vram_size - DISCOVERY_TMR_OFFSET as u64,
        vram,
        // The SMU mailbox (MP1 segment 0 + C2PMSG_66 / C2PMSG_90): a
        // message written is answered with SMU_RESP_OK.
        on_write: vec![
            (0x16000 + mmMP1_SMN_C2PMSG_66, 0x16000 + mmMP1_SMN_C2PMSG_90, SMU_RESP_OK),
            // The PSP (MP0 segment 0): a ring command on C2PMSG_64 is
            // answered with the response flag.
            (0x16000 + MP0_mmMP0_SMN_C2PMSG_64, 0x16000 + MP0_mmMP0_SMN_C2PMSG_64, MBOX_TOS_RESP_FLAG),
        ],
        writes: Vec::new(),
        psp_wptr: Some(0x16000 + MP0_mmMP0_SMN_C2PMSG_67),
    });

    let mut vfct = vec![0u8; VFCT_SIZE as usize];
    put32(&mut vfct, VFCT_VBIOS_IMAGE_OFFSET, VFCT_SIZE);
    let vhdr = vfct.len() as u32;
    vfct.resize((vhdr + GOP_VBIOS_CONTENT_OFFSET) as usize, 0);
    put32(&mut vfct, vhdr + VHDR_PCI_BUS, BDF.0 as u32);
    put32(&mut vfct, vhdr + VHDR_PCI_DEVICE, BDF.1 as u32);
    put32(&mut vfct, vhdr + VHDR_PCI_FUNCTION, BDF.2 as u32);
    put16(&mut vfct, vhdr + VHDR_VENDOR_ID, 0x1002);
    put16(&mut vfct, vhdr + VHDR_DEVICE_ID, 0x164c);
    put32(&mut vfct, vhdr + VHDR_IMAGE_LENGTH, bios.len() as u32);
    vfct.extend_from_slice(&bios);
    let len = vfct.len() as u32;
    put32(&mut vfct, ACPI_TABLE_HEADER_LENGTH, len);
    npk_sys::fake_acpi(*b"VFCT", vfct);

    let mut adev = amdgpu_device::amdgpu_device::new(0x164c, 0xc1, npk_sys::Mmio(0), 512 << 10);
    adev.rmmio_base = 0xfd40_0000;
    adev.subsystem_vendor = 0x17aa;
    adev.subsystem_device = 0x3806;
    adev
}
