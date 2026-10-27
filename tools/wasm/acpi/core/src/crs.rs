//! `_CRS`/`_PRS` resource templates — the binary form, decoded.
//!
//! Layouts 1:1 from ACPICA `drivers/acpi/acpica/amlresrc.h`, where the
//! field names can be looked up. Only the descriptors a driver here needs
//! are unpacked; everything else is skipped by its length, so an unknown
//! entry does not shift the rest of the template.
//!
//! A template is a sequence of small or large descriptors:
//!
//! * small — byte 0 bit 7 = 0, length in bits 2..0, total `1 + len`
//! * large — byte 0 bit 7 = 1, then a `u16` length, total `3 + len`
//!
//! It ends with the small tag `0x79`.

use alloc::{string::String, vec::Vec};

/// Descriptor type bytes (ACPI 6.5 §6.4.3) as they appear in the buffer.
pub const TAG_MEMORY32_FIXED: u8 = 0x86;
pub const TAG_EXTENDED_IRQ: u8 = 0x89;
pub const TAG_GPIO: u8 = 0x8C;
pub const TAG_SERIAL_BUS: u8 = 0x8E;
pub const TAG_END: u8 = 0x79;

/// Serial bus `type` field (AML_RESOURCE_SERIAL_COMMON.type).
pub const SERIAL_TYPE_I2C: u8 = 1;

/// GPIO `connection_type`.
pub const GPIO_CONN_INTERRUPT: u8 = 0;
pub const GPIO_CONN_IO: u8 = 1;

#[derive(Clone, Debug)]
pub enum Resource {
    /// `Memory32Fixed` — how an FCH I2C controller declares its register
    /// block.
    Memory32Fixed { address: u32, length: u32 },
    /// `Interrupt (...)` — the GSI(s).
    ExtendedIrq { flags: u8, interrupts: Vec<u32> },
    /// `I2cSerialBus (...)` — slave address, bus speed and the ACPI path of
    /// the controller (`resource_source`).
    I2cSerialBus {
        slave_address: u16,
        connection_speed: u32,
        /// Bit 0 of `flags`: 1 = consumer, 0 = producer.
        flags: u8,
        type_specific_flags: u16,
        source: String,
    },
    /// `GpioInt` / `GpioIo` — pin list and the path of the GPIO controller.
    Gpio {
        connection_type: u8,
        flags: u16,
        int_flags: u16,
        pins: Vec<u16>,
        source: String,
    },
    /// Anything else: only the tag, so a report stays complete.
    Other { tag: u8, len: usize },
}

fn u16le(b: &[u8], off: usize) -> u16 {
    if off + 2 > b.len() {
        return 0;
    }
    (b[off] as u16) | ((b[off + 1] as u16) << 8)
}

fn u32le(b: &[u8], off: usize) -> u32 {
    if off + 4 > b.len() {
        return 0;
    }
    (b[off] as u32)
        | ((b[off + 1] as u32) << 8)
        | ((b[off + 2] as u32) << 16)
        | ((b[off + 3] as u32) << 24)
}

/// A NUL-terminated string from `off`, at most up to `end`.
///
/// ACPICA reads `resource_source` the same way: the rest of the descriptor
/// after the fixed fields, up to the NUL. Without a NUL the rest counts.
fn source_str(b: &[u8], off: usize, end: usize) -> String {
    if off >= end || off >= b.len() {
        return String::new();
    }
    let end = end.min(b.len());
    let mut s = String::new();
    for &c in &b[off..end] {
        if c == 0 {
            break;
        }
        s.push(c as char);
    }
    s
}

/// Split a resource template into its descriptors.
///
/// Stops at the end tag, at a zero length (or the loop would never end) and
/// at the end of the buffer.
pub fn parse(buf: &[u8]) -> Vec<Resource> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let tag = buf[i];
        if tag & 0x80 == 0 {
            // Small descriptor: length in the low three bits.
            let len = (tag & 0x07) as usize;
            if (tag >> 3) & 0x0F == (TAG_END >> 3) & 0x0F {
                break; // End tag: small type 0x0F
            }
            out.push(Resource::Other { tag, len });
            i += 1 + len;
            continue;
        }
        // Large descriptor.
        if i + 3 > buf.len() {
            break;
        }
        let len = u16le(buf, i + 1) as usize;
        let end = i + 3 + len;
        if len == 0 || end > buf.len() {
            break;
        }
        let d = &buf[i..end];
        match tag {
            TAG_MEMORY32_FIXED => out.push(Resource::Memory32Fixed {
                address: u32le(d, 4),
                length: u32le(d, 8),
            }),
            TAG_EXTENDED_IRQ => {
                let flags = if d.len() > 3 { d[3] } else { 0 };
                let count = if d.len() > 4 { d[4] as usize } else { 0 };
                let mut ints = Vec::new();
                for n in 0..count {
                    let off = 5 + n * 4;
                    if off + 4 > d.len() {
                        break;
                    }
                    ints.push(u32le(d, off));
                }
                out.push(Resource::ExtendedIrq { flags, interrupts: ints });
            }
            TAG_GPIO => {
                // amlresrc.h struct aml_resource_gpio
                let connection_type = if d.len() > 4 { d[4] } else { 0 };
                let flags = u16le(d, 5);
                let int_flags = u16le(d, 7);
                let pin_table_offset = u16le(d, 14) as usize;
                let res_source_offset = u16le(d, 17) as usize;
                let vendor_offset = u16le(d, 19) as usize;
                // The pin list runs up to the source string (or, if absent,
                // up to the vendor data).
                let pin_end = if res_source_offset > pin_table_offset {
                    res_source_offset
                } else if vendor_offset > pin_table_offset {
                    vendor_offset
                } else {
                    d.len()
                };
                let mut pins = Vec::new();
                let mut p = pin_table_offset;
                while p + 2 <= pin_end.min(d.len()) {
                    pins.push(u16le(d, p));
                    p += 2;
                }
                let src_end = if vendor_offset > res_source_offset {
                    vendor_offset
                } else {
                    d.len()
                };
                out.push(Resource::Gpio {
                    connection_type,
                    flags,
                    int_flags,
                    pins,
                    source: source_str(d, res_source_offset, src_end),
                });
            }
            TAG_SERIAL_BUS => {
                // AML_RESOURCE_SERIAL_COMMON from offset 3.
                let bus_type = if d.len() > 5 { d[5] } else { 0 };
                let flags = if d.len() > 6 { d[6] } else { 0 };
                let type_specific_flags = u16le(d, 7);
                let type_data_length = u16le(d, 10) as usize;
                if bus_type == SERIAL_TYPE_I2C {
                    // aml_resource_i2c_serialbus: connection_speed @12, slave @16.
                    // The source string follows the type-specific data, whose
                    // length is in `type_data_length`.
                    let src_off = 12 + type_data_length;
                    out.push(Resource::I2cSerialBus {
                        slave_address: u16le(d, 16),
                        connection_speed: u32le(d, 12),
                        flags,
                        type_specific_flags,
                        source: source_str(d, src_off, d.len()),
                    });
                } else {
                    out.push(Resource::Other { tag, len });
                }
            }
            _ => out.push(Resource::Other { tag, len }),
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A hand-built template: Memory32Fixed + Extended IRQ + end. The values
    /// come from the structure definition; the test checks the decoder, not
    /// firmware.
    #[test]
    fn memory_and_irq() {
        let mut b = vec![
            0x86, 0x09, 0x00, // Memory32Fixed, len 9
            0x01, // flags (writable)
            0x00, 0x20, 0xDC, 0xFE, // address 0xFEDC2000
            0x00, 0x10, 0x00, 0x00, // length 0x1000
        ];
        b.extend_from_slice(&[
            0x89, 0x06, 0x00, // Extended IRQ, len 6
            0x03, // flags
            0x01, // count
            0x0A, 0x00, 0x00, 0x00, // GSI 10
        ]);
        b.extend_from_slice(&[0x79, 0x00]);

        let r = parse(&b);
        assert_eq!(r.len(), 2);
        match &r[0] {
            Resource::Memory32Fixed { address, length } => {
                assert_eq!(*address, 0xFEDC_2000);
                assert_eq!(*length, 0x1000);
            }
            _ => panic!("expected Memory32Fixed"),
        }
        match &r[1] {
            Resource::ExtendedIrq { interrupts, .. } => assert_eq!(interrupts, &[10]),
            _ => panic!("expected ExtendedIrq"),
        }
    }

    /// I2cSerialBus with a source string — the case controller lookup
    /// depends on.
    #[test]
    fn i2c_serial_bus_with_source() {
        let src = b"\\_SB.I2CB\0";
        let type_data_length = 6usize; // connection_speed + slave_address
        let body_len = 9 + type_data_length + src.len(); // from revision_id
        let mut b = vec![TAG_SERIAL_BUS];
        b.push((body_len & 0xFF) as u8);
        b.push((body_len >> 8) as u8);
        b.push(0x01); // revision_id
        b.push(0x00); // res_source_index
        b.push(SERIAL_TYPE_I2C);
        b.push(0x00); // flags
        b.extend_from_slice(&[0x00, 0x00]); // type_specific_flags
        b.push(0x01); // type_revision_id
        b.extend_from_slice(&[(type_data_length as u8), 0x00]);
        b.extend_from_slice(&400_000u32.to_le_bytes());
        b.extend_from_slice(&0x0015u16.to_le_bytes());
        b.extend_from_slice(src);
        b.extend_from_slice(&[0x79, 0x00]);

        let r = parse(&b);
        assert_eq!(r.len(), 1);
        match &r[0] {
            Resource::I2cSerialBus { slave_address, connection_speed, source, .. } => {
                assert_eq!(*slave_address, 0x15);
                assert_eq!(*connection_speed, 400_000);
                assert_eq!(source, "\\_SB.I2CB");
            }
            _ => panic!("expected I2cSerialBus"),
        }
    }

    /// An unknown large descriptor must not shift the rest.
    #[test]
    fn unknown_descriptor_is_skipped_by_length() {
        let mut b = vec![0x8F, 0x04, 0x00, 1, 2, 3, 4];
        b.extend_from_slice(&[
            0x86, 0x09, 0x00, 0x01, 0x00, 0x00, 0x00, 0xF0, 0x00, 0x10, 0x00, 0x00,
        ]);
        b.extend_from_slice(&[0x79, 0x00]);
        let r = parse(&b);
        assert_eq!(r.len(), 2);
        assert!(matches!(r[0], Resource::Other { tag: 0x8F, .. }));
        assert!(matches!(r[1], Resource::Memory32Fixed { address: 0xF000_0000, .. }));
    }
}
