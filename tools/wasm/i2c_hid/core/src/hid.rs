//! HID over I2C — das Protokoll auf dem Bus.
//!
//! Portiert aus Linux 6.18.26 `drivers/hid/i2c-hid/i2c-hid-core.c`. Die
//! Reihenfolge ist die des Originals, und die Wartezeiten sind seine:
//! sie stehen dort nicht in der Spezifikation, sondern sind gemessen.

use crate::dw_i2c::{self, Bus, Error, Msg};
use alloc::{format, string::String, vec, vec::Vec};

/// `struct i2c_hid_desc` (i2c-hid-core.c) — 30 Bytes, little-endian.
#[derive(Clone, Copy, Debug, Default)]
pub struct HidDesc {
    pub desc_length: u16,
    pub bcd_version: u16,
    pub report_desc_length: u16,
    pub report_desc_register: u16,
    pub input_register: u16,
    pub max_input_length: u16,
    pub output_register: u16,
    pub max_output_length: u16,
    pub command_register: u16,
    pub data_register: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_id: u16,
}

fn u16le(b: &[u8], off: usize) -> u16 {
    if off + 2 > b.len() { return 0; }
    (b[off] as u16) | ((b[off + 1] as u16) << 8)
}

impl HidDesc {
    pub fn parse(b: &[u8]) -> HidDesc {
        HidDesc {
            desc_length: u16le(b, 0),
            bcd_version: u16le(b, 2),
            report_desc_length: u16le(b, 4),
            report_desc_register: u16le(b, 6),
            input_register: u16le(b, 8),
            max_input_length: u16le(b, 10),
            output_register: u16le(b, 12),
            max_output_length: u16le(b, 14),
            command_register: u16le(b, 16),
            data_register: u16le(b, 18),
            vendor_id: u16le(b, 20),
            product_id: u16le(b, 22),
            version_id: u16le(b, 24),
        }
    }

    /// `i2c_hid_fetch_hid_descriptor` prueft genau diese zwei Dinge.
    ///
    /// **Beides ist eine echte Probe, keine Formalitaet:** ein Geraet, das
    /// gar nicht antwortet, liefert lauter Nullen oder lauter Einsen — und
    /// beides faellt hier durch.
    pub fn valid(&self) -> Result<(), String> {
        if self.bcd_version != 0x0100 {
            return Err(format!("bcdVersion {:#06x}, expected 0x0100", self.bcd_version));
        }
        if self.desc_length != 30 {
            return Err(format!("wHIDDescLength {}, expected 30", self.desc_length));
        }
        Ok(())
    }

    pub fn describe(&self) -> String {
        format!(
            "hid: vid={:#06x} pid={:#06x} ver={:#06x} · input reg {:#06x} max {} · \
             report desc reg {:#06x} len {} · cmd {:#06x} data {:#06x}",
            self.vendor_id, self.product_id, self.version_id,
            self.input_register, self.max_input_length,
            self.report_desc_register, self.report_desc_length,
            self.command_register, self.data_register
        )
    }
}

/// `i2c_hid_probe_address` — ein Byte lesen, und bei Fehlschlag nach
/// 400 µs noch einmal.
///
/// Manche STM- und Weida-Geraete brauchen nach einer steigenden Taktflanke
/// diese Zeit, um aus dem Tiefschlaf zu kommen; der erste Versuch schlaegt
/// dann fehl. Steht im Original mit genau dieser Begruendung.
pub fn probe_address(bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16) -> Result<(), Error> {
    let mut one = [0u8; 1];
    {
        let mut msgs = [Msg::Read(&mut one)];
        if dw_i2c::xfer(bus, dw, addr, &mut msgs).is_ok() {
            return Ok(());
        }
    }
    bus.udelay(500);
    let mut msgs = [Msg::Read(&mut one)];
    dw_i2c::xfer(bus, dw, addr, &mut msgs)
}

/// `i2c_hid_read_register` — zwei Bytes Registeradresse schreiben, dann
/// `len` Bytes lesen. Ein einziger Transfer mit Restart dazwischen.
pub fn read_register(
    bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16, reg: u16, out: &mut [u8],
) -> Result<(), Error> {
    let cmd = [(reg & 0xFF) as u8, (reg >> 8) as u8];
    let mut msgs = [Msg::Write(&cmd), Msg::Read(out)];
    dw_i2c::xfer(bus, dw, addr, &mut msgs)
}

/// `i2c_hid_fetch_hid_descriptor`.
pub fn fetch_descriptor(
    bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16, desc_reg: u16,
) -> Result<HidDesc, String> {
    let mut raw = [0u8; 30];
    read_register(bus, dw, addr, desc_reg, &mut raw)
        .map_err(|e| format!("descriptor read failed: {e:?}"))?;
    let d = HidDesc::parse(&raw);
    d.valid()?;
    Ok(d)
}

/// Opcodes aus dem HID-over-I2C-Protokoll (i2c-hid-core.c).
pub const OPCODE_RESET: u8 = 0x01;
pub const OPCODE_SET_REPORT: u8 = 0x03;
pub const OPCODE_SET_POWER: u8 = 0x08;
pub const REPORT_TYPE_FEATURE: u8 = 0x03;
pub const PWR_ON: u8 = 0x00;
pub const PWR_SLEEP: u8 = 0x01;

/// `i2c_hid_encode_command`.
fn encode_command(buf: &mut Vec<u8>, opcode: u8, report_type: u8, report_id: u8) {
    if report_id < 0x0F {
        buf.push((report_type << 4) | report_id);
        buf.push(opcode);
    } else {
        buf.push((report_type << 4) | 0x0F);
        buf.push(opcode);
        buf.push(report_id);
    }
}

/// `i2c_hid_set_power`.
///
/// **Die 60 ms danach stehen nicht in der Spezifikation.** Der Kommentar
/// im Original sagt es woertlich: nach PWR_ON soll das GERAET den Takt
/// dehnen, aber Windows wartet 1 ms, Goodix-Geraete brauchen 60 — und
/// mehrere Geraete arbeiten ohne diese Pause nicht richtig. Gemessen, nicht
/// hergeleitet, und deshalb uebernommen.
pub fn set_power(
    bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16, d: &HidDesc, state: u8,
) -> Result<(), Error> {
    let mut cmd: Vec<u8> = vec![
        (d.command_register & 0xFF) as u8,
        (d.command_register >> 8) as u8,
    ];
    encode_command(&mut cmd, OPCODE_SET_POWER, 0, state);

    let mut r = {
        let mut msgs = [Msg::Write(&cmd)];
        dw_i2c::xfer(bus, dw, addr, &mut msgs)
    };
    if r.is_err() && state == PWR_ON {
        // Dieselbe 400-µs-Geschichte wie bei `probe_address`.
        bus.udelay(500);
        let mut msgs = [Msg::Write(&cmd)];
        r = dw_i2c::xfer(bus, dw, addr, &mut msgs);
    }
    if r.is_ok() && state == PWR_ON {
        bus.udelay(60_000);
    }
    r
}

/// `i2c_hid_set_or_send_report` mit `do_set = true` — ein FEATURE-Bericht
/// an das Geraet.
///
/// Gebraucht fuer genau eine Sache, aber eine wichtige: den „Device Mode"
/// eines Praezisions-Touchpads auf 3 zu stellen. Ohne diesen Schalter
/// meldet es sich wie eine Maus und liefert gar keine Mehrfingerdaten —
/// Zweifinger-Scrollen ist dann nicht schwer, sondern unmoeglich.
///
/// Die Form ist die des Originals: Befehlsregister, SET_REPORT mit Typ
/// und Berichtsnummer, dann die Adresse des DATENregisters, dann der
/// Bericht mit seiner Laenge davor (`i2c_hid_format_report`).
pub fn set_report(
    bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16, d: &HidDesc,
    report_type: u8, report_id: u8, data: &[u8],
) -> Result<(), Error> {
    let mut cmd: Vec<u8> = vec![
        (d.command_register & 0xFF) as u8,
        (d.command_register >> 8) as u8,
    ];
    encode_command(&mut cmd, OPCODE_SET_REPORT, report_type, report_id);
    cmd.push((d.data_register & 0xFF) as u8);
    cmd.push((d.data_register >> 8) as u8);

    // `i2c_hid_format_report`: Laenge zuerst, dann — wenn es eine gibt —
    // die Berichtsnummer, dann die Daten. Die Laenge zaehlt sich SELBST
    // mit.
    let mut body: Vec<u8> = vec![0, 0];
    if report_id != 0 { body.push(report_id); }
    body.extend_from_slice(data);
    let n = body.len() as u16;
    body[0] = (n & 0xFF) as u8;
    body[1] = (n >> 8) as u8;
    cmd.extend_from_slice(&body);

    let mut msgs = [Msg::Write(&cmd)];
    dw_i2c::xfer(bus, dw, addr, &mut msgs)
}

/// `i2c_hid_start_hwreset` + `i2c_hid_finish_hwreset`.
///
/// Der Reset meldet sich zurueck, indem das Geraet ein Eingaberegister mit
/// LAENGE NULL bereitstellt. Linux wartet darauf ueber den Interrupt; wir
/// lesen das Register, bis die Null kommt oder die Zeit ablaeuft.
pub fn reset(
    bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16, d: &HidDesc,
) -> Result<(), String> {
    set_power(bus, dw, addr, d, PWR_ON).map_err(|e| format!("power on failed: {e:?}"))?;

    let mut cmd: Vec<u8> = vec![
        (d.command_register & 0xFF) as u8,
        (d.command_register >> 8) as u8,
    ];
    encode_command(&mut cmd, OPCODE_RESET, 0, 0);
    {
        let mut msgs = [Msg::Write(&cmd)];
        dw_i2c::xfer(bus, dw, addr, &mut msgs)
            .map_err(|e| format!("reset command failed: {e:?}"))?;
    }

    // Auf die Null-Laenge warten (Linux: 1 s).
    let deadline = bus.now_us() + 1_000_000;
    loop {
        let mut len = [0u8; 2];
        let mut msgs = [Msg::Read(&mut len)];
        if dw_i2c::xfer(bus, dw, addr, &mut msgs).is_ok() {
            let n = (len[0] as u16) | ((len[1] as u16) << 8);
            if n == 0 { break; }
        }
        if bus.now_us() >= deadline {
            // Linux warnt hier nur und macht weiter.
            bus.note("i2c-hid: device did not ack reset within 1000 ms");
            break;
        }
        bus.udelay(1000);
    }

    // „At least some SIS devices need this after reset."
    set_power(bus, dw, addr, d, PWR_ON).map_err(|e| format!("power on after reset: {e:?}"))?;
    Ok(())
}

/// `i2c_hid_get_input` — ein Eingabebericht, wenn einer anliegt.
///
/// Gelesen wird die in `wMaxInputLength` angesagte Groesse; die ersten
/// zwei Bytes sind die tatsaechliche Laenge. **Null heisst „nichts da"**
/// (oder: ein Reset ist fertig), und das ist der Normalfall beim Pollen.
pub fn get_input<'a>(
    bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16, d: &HidDesc, buf: &'a mut [u8],
) -> Result<Option<&'a [u8]>, Error> {
    let want = (d.max_input_length as usize).min(buf.len());
    if want < 2 { return Ok(None); }
    {
        let mut msgs = [Msg::Read(&mut buf[..want])];
        dw_i2c::xfer(bus, dw, addr, &mut msgs)?;
    }
    let n = ((buf[0] as usize) | ((buf[1] as usize) << 8)) as usize;
    if n == 0 { return Ok(None); }
    // 0xFFFF ist ein bekannter Muellwert (I2C_HID_QUIRK_BOGUS_IRQ).
    if n == 0xFFFF || n > want || n < 2 { return Ok(None); }
    Ok(Some(&buf[2..n]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Deskriptor, wie ein Elan-Touchpad ihn liefert — und die beiden
    /// Proben, die Linux daran macht.
    #[test]
    fn descriptor_validation() {
        let mut raw = [0u8; 30];
        raw[0] = 30; // wHIDDescLength
        raw[2] = 0x00; raw[3] = 0x01; // bcdVersion 1.00
        raw[8] = 0x03; // wInputRegister
        let d = HidDesc::parse(&raw);
        assert!(d.valid().is_ok());
        assert_eq!(d.input_register, 3);

        // Ein Geraet, das nicht antwortet, liefert lauter Nullen …
        assert!(HidDesc::parse(&[0u8; 30]).valid().is_err());
        // … oder lauter Einsen.
        assert!(HidDesc::parse(&[0xFFu8; 30]).valid().is_err());
    }

    /// `i2c_hid_encode_command`: ab Report-ID 15 wird sie ein drittes Byte.
    #[test]
    fn command_encoding() {
        let mut b = Vec::new();
        encode_command(&mut b, OPCODE_SET_POWER, 0, PWR_ON);
        assert_eq!(b, vec![0x00, 0x08]);

        let mut b = Vec::new();
        encode_command(&mut b, OPCODE_RESET, 0, 0);
        assert_eq!(b, vec![0x00, 0x01]);

        let mut b = Vec::new();
        encode_command(&mut b, 0x02, 3, 0x20);
        assert_eq!(b, vec![0x3F, 0x02, 0x20]);
    }
}
