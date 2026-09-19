//! Der HID-Report-Deskriptor — was ein Byte im Bericht bedeutet.
//!
//! Portiert aus Linux 6.18.26 `drivers/hid/hid-core.c`
//! (`hid_parser_main`/`_global`/`_local`, `hid_add_field`), auf das
//! eingedampft, was ein Zeigergeraet braucht: je Bericht eine Liste von
//! Feldern mit Bitversatz, Breite und Usage.
//!
//! **Warum ueberhaupt parsen?** Weil sonst jeder Treiber fuer genau ein
//! Modell gilt. Die Elan im IdeaPad, die Wacom daneben und das naechste
//! Geraet legen ihre Bytes verschieden — im Deskriptor steht, wie.

use alloc::{format, string::String, vec::Vec};

// Usage Pages, die uns angehen.
pub const PAGE_GENERIC_DESKTOP: u16 = 0x01;
pub const PAGE_BUTTON: u16 = 0x09;
pub const PAGE_DIGITIZER: u16 = 0x0D;

// Usages (Generic Desktop)
pub const USAGE_X: u16 = 0x30;
pub const USAGE_Y: u16 = 0x31;
pub const USAGE_WHEEL: u16 = 0x38;

// Usages (Digitizer)
pub const USAGE_TIP_SWITCH: u16 = 0x42;
pub const USAGE_CONTACT_ID: u16 = 0x51;
/// „Device Mode" im Feature-Bericht: 0 = Maus-Kompatibilitaet,
/// 3 = Praezisions-Touchpad. Ohne diesen Schalter liefert ein Touchpad
/// gar keine Mehrfingerdaten — es TUT so, als waere es eine Maus.
pub const USAGE_INPUT_MODE: u16 = 0x52;
pub const USAGE_CONTACT_COUNT: u16 = 0x54;

/// Zu welchem Berichtstyp ein Feld gehoert.
///
/// Report-IDs sind je Typ eigenstaendig: derselbe Bericht 3 kann als
/// Eingabe und als Feature ganz verschieden aussehen, und beide haben
/// ihre eigenen Bitversaetze.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind { Input, Output, Feature }

/// Ein Feld in einem Eingabebericht.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub kind: Kind,
    pub report_id: u8,
    pub usage_page: u16,
    pub usage: u16,
    /// Bitversatz IM BERICHT, ohne das Report-ID-Byte.
    pub bit_offset: u32,
    pub bit_size: u32,
    pub logical_min: i32,
    pub logical_max: i32,
    /// Bit 2 des Input-Items: 0 = absolut, 1 = relativ.
    pub relative: bool,
    /// Bit 0: 1 = Konstante (Fuellbits), fuer uns uninteressant.
    pub constant: bool,
}

/// Das Ergebnis: alle Eingabefelder, in Deskriptor-Reihenfolge.
#[derive(Default)]
pub struct ReportMap {
    pub fields: Vec<Field>,
    /// Hat der Deskriptor ueberhaupt Report-IDs benutzt? Wenn nicht,
    /// traegt der Bericht kein ID-Byte.
    pub uses_ids: bool,
    /// Die GANZE Laenge je Bericht, in Bit.
    ///
    /// Aus den Feldern allein ist sie NICHT herleitbar: Fuellbits sind
    /// keine Felder, stehen aber im Bericht. Und wer einen Feature-Bericht
    /// zu kurz schickt, bekommt auf dem Bus ein ACK und trotzdem keine
    /// Wirkung — genau daran ist der Umschalter in den Praezisionsmodus
    /// gescheitert (Elan: `Report Size 16`, wir schickten ein Byte).
    lens: Vec<(Kind, u8, u32)>,
}

impl ReportMap {
    /// Wie lang ist dieser Bericht, in BYTES, ohne das Report-ID-Byte?
    pub fn report_bytes(&self, kind: Kind, id: u8) -> usize {
        self.lens.iter()
            .find(|(k, i, _)| *k == kind && *i == id)
            .map(|(_, _, bits)| ((*bits as usize) + 7) / 8)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Default)]
struct Global {
    usage_page: u16,
    logical_min: i32,
    logical_max: i32,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

/// Einen Report-Deskriptor zerlegen.
///
/// Kurze Items: `bSize` in Bit 1..0, `bType` in 3..2, `bTag` in 7..4.
/// `bSize == 3` heisst VIER Bytes, nicht drei — die Stelle, an der sich
/// ein selbstgeschriebener Parser als erstes vertut.
pub fn parse(desc: &[u8]) -> ReportMap {
    let mut out = ReportMap::default();
    let mut g = Global::default();
    // Ein Stapel fuer Push/Pop (Tag 0xA4/0xB4) — selten, aber wenn er
    // fehlt, verrutscht alles danach.
    let mut stack: Vec<Global> = Vec::new();
    let mut usages: Vec<u16> = Vec::new();
    let mut usage_min: Option<u16> = None;
    // Bitversatz je Report-ID.
    // Je Typ eigene Versaetze — siehe [`Kind`].
    let mut off_in: [u32; 256] = [0; 256];
    let mut off_out: [u32; 256] = [0; 256];
    let mut off_feat: [u32; 256] = [0; 256];

    let mut i = 0usize;
    while i < desc.len() {
        let b0 = desc[i];
        if b0 == 0xFE {
            // Long item: Laenge in Byte 1.
            let len = *desc.get(i + 1).unwrap_or(&0) as usize;
            i += 3 + len;
            continue;
        }
        let size = match b0 & 0x03 { 0 => 0, 1 => 1, 2 => 2, _ => 4 };
        let ty = (b0 >> 2) & 0x03;
        let tag = (b0 >> 4) & 0x0F;
        if i + 1 + size > desc.len() { break; }
        let mut val: u32 = 0;
        for k in 0..size {
            val |= (desc[i + 1 + k] as u32) << (8 * k);
        }
        // Vorzeichenbehaftet fuer logical min/max.
        let sval: i32 = match size {
            1 => (val as u8) as i8 as i32,
            2 => (val as u16) as i16 as i32,
            4 => val as i32,
            _ => 0,
        };
        i += 1 + size;

        match ty {
            // ── Main ───────────────────────────────────────────────
            0 => match tag {
                0x8 | 0x9 | 0xB => {
                    // Input / Output / Feature — dieselbe Buchfuehrung,
                    // nur ein anderer Topf.
                    let kind = match tag {
                        0x8 => Kind::Input,
                        0x9 => Kind::Output,
                        _ => Kind::Feature,
                    };
                    let constant = val & 0x01 != 0;
                    let relative = val & 0x04 != 0;
                    let off = match kind {
                        Kind::Input => &mut off_in[g.report_id as usize],
                        Kind::Output => &mut off_out[g.report_id as usize],
                        Kind::Feature => &mut off_feat[g.report_id as usize],
                    };
                    // Ein Block, dessen Elemente ALLE DIESELBE Usage
                    // tragen, braucht keinen Eintrag je Element.
                    //
                    // Linux speichert Anzahl und Groesse einmal je Feld;
                    // ich lege je Element einen an, und ein
                    // Hersteller-Feature mit `Report Count (0x488)` macht
                    // daraus 1160 Eintraege — auf Florians Touchpad kamen
                    // so 1583 Felder aus 381 Bytes zusammen. Wo die Usages
                    // sich unterscheiden (Kontaktpunkte, Tastenreihen ueber
                    // `Usage Minimum`), bleibt es bei einem Eintrag je
                    // Element; sonst genuegt einer, und der Versatz
                    // springt ueber den ganzen Block.
                    let distinct = usages.len() > 1 || usage_min.is_some();
                    if !distinct && g.report_count > 1 {
                        out.fields.push(Field {
                            kind,
                            report_id: g.report_id,
                            usage_page: g.usage_page,
                            usage: usages.last().copied().unwrap_or(0),
                            bit_offset: *off,
                            bit_size: g.report_size,
                            logical_min: g.logical_min,
                            logical_max: g.logical_max,
                            relative,
                            constant,
                        });
                        *off += g.report_size * g.report_count;
                        usages.clear();
                        usage_min = None;
                        continue;
                    }
                    for n in 0..g.report_count {
                        let usage = if (n as usize) < usages.len() {
                            usages[n as usize]
                        } else if let Some(m) = usage_min {
                            m.saturating_add(n as u16)
                        } else {
                            usages.last().copied().unwrap_or(0)
                        };
                        // Fuellbits erzeugen KEIN Feld.
                        //
                        // Sie werden nie gelesen (`find` filtert sie
                        // ohnehin), aber sie kosten: ein Herstellerblock
                        // mit `Report Count (0x488)` legte 1160 Eintraege
                        // an. Auf Florians Touchpad kamen so 1634 Felder
                        // aus 381 Bytes zusammen. Der Versatz muss
                        // trotzdem weiterlaufen — sonst verrutscht alles
                        // danach.
                        if constant {
                            *off += g.report_size;
                            continue;
                        }
                        out.fields.push(Field {
                            kind,
                            report_id: g.report_id,
                            usage_page: g.usage_page,
                            usage,
                            bit_offset: *off,
                            bit_size: g.report_size,
                            logical_min: g.logical_min,
                            logical_max: g.logical_max,
                            relative,
                            constant,
                        });
                        *off += g.report_size;
                    }
                    usages.clear();
                    usage_min = None;
                }
                0xA | 0xC => {
                    // Collection / End Collection
                    usages.clear();
                    usage_min = None;
                }
                _ => { usages.clear(); usage_min = None; }
            },
            // ── Global ─────────────────────────────────────────────
            1 => match tag {
                0x0 => g.usage_page = val as u16,
                0x1 => g.logical_min = sval,
                0x2 => g.logical_max = sval,
                0x7 => g.report_size = val,
                0x8 => { g.report_id = val as u8; out.uses_ids = true; }
                0x9 => g.report_count = val,
                0xA => stack.push(g),
                0xB => { if let Some(p) = stack.pop() { g = p; } }
                _ => {}
            },
            // ── Local ──────────────────────────────────────────────
            2 => match tag {
                0x0 => usages.push(val as u16),
                0x1 => usage_min = Some(val as u16),
                0x2 => {} // Usage Maximum: die Spanne ergibt sich aus min + n
                _ => {}
            },
            _ => {}
        }
    }
    for id in 0..256usize {
        for (kind, off) in [
            (Kind::Input, off_in[id]),
            (Kind::Output, off_out[id]),
            (Kind::Feature, off_feat[id]),
        ] {
            if off > 0 { out.lens.push((kind, id as u8, off)); }
        }
    }
    out
}

impl ReportMap {
    /// Das erste EINGABEfeld mit dieser Usage in diesem Bericht.
    pub fn find(&self, report_id: u8, page: u16, usage: u16) -> Option<&Field> {
        self.find_all(Kind::Input, report_id, page, usage).into_iter().next()
    }

    /// ALLE Felder dieser Usage — ein Mehrfinger-Touchpad fuehrt X und Y
    /// je Kontaktpunkt, also mehrfach im selben Bericht.
    pub fn find_all(&self, kind: Kind, report_id: u8, page: u16, usage: u16) -> Vec<&Field> {
        self.fields.iter().filter(|f| {
            f.kind == kind && f.report_id == report_id
                && f.usage_page == page && f.usage == usage && !f.constant
        }).collect()
    }

    /// Ein FEATURE-Feld mit dieser Usage, irgendwo — samt seiner
    /// Berichtsnummer.
    pub fn find_feature(&self, page: u16, usage: u16) -> Option<&Field> {
        self.fields.iter().find(|f| {
            f.kind == Kind::Feature && f.usage_page == page && f.usage == usage
        })
    }

    /// Alle Report-IDs, die Eingabefelder tragen.
    pub fn report_ids(&self) -> Vec<u8> {
        let mut ids: Vec<u8> = Vec::new();
        for f in &self.fields {
            if f.kind == Kind::Input && !ids.contains(&f.report_id) { ids.push(f.report_id); }
        }
        ids
    }

    /// Ein Bericht, der sich als ZEIGER auswerten laesst: X und Y darin.
    pub fn pointer_report(&self) -> Option<u8> {
        self.report_ids().into_iter().find(|&id| {
            self.find(id, PAGE_GENERIC_DESKTOP, USAGE_X).is_some()
                && self.find(id, PAGE_GENERIC_DESKTOP, USAGE_Y).is_some()
        })
    }

    /// Ein Bericht, der KONTAKTPUNKTE traegt — also ein echter
    /// Touchpad-Bericht und keine Maus-Nachahmung.
    pub fn touchpad_report(&self) -> Option<u8> {
        self.report_ids().into_iter().find(|&id| {
            self.find(id, PAGE_DIGITIZER, USAGE_TIP_SWITCH).is_some()
                && self.find(id, PAGE_GENERIC_DESKTOP, USAGE_X).is_some()
        })
    }

    /// Wieviele Kontaktpunkte dieser Bericht fuehrt.
    ///
    /// **Das ist nicht die Zahl der Finger, die das Geraet erkennt.** Ein
    /// Praezisions-Touchpad, dessen Bericht nur einen Platz hat, schickt
    /// MEHRERE Berichte je Bild — `Contact Count` im ersten sagt, wieviele
    /// insgesamt kommen. Florians Elan macht genau das.
    pub fn contact_slots(&self, report_id: u8) -> usize {
        self.find_all(Kind::Input, report_id, PAGE_DIGITIZER, USAGE_TIP_SWITCH).len()
    }

    pub fn describe(&self) -> String {
        let mut s = format!("{} field(s), report ids {:?}", self.fields.len(), self.report_ids());
        if let Some(id) = self.pointer_report() {
            let x = self.find(id, PAGE_GENERIC_DESKTOP, USAGE_X).unwrap();
            s.push_str(&format!(
                " · pointer in report {id}: X at bit {} ({} bit, {}), range {}..{}",
                x.bit_offset, x.bit_size,
                if x.relative { "relative" } else { "absolute" },
                x.logical_min, x.logical_max));
        }
        s
    }
}

/// Ein Feld aus einem Bericht herausziehen — vorzeichenrichtig.
///
/// `data` ist der Bericht OHNE Report-ID-Byte.
pub fn extract(data: &[u8], f: &Field) -> i32 {
    if f.bit_size == 0 || f.bit_size > 32 { return 0; }
    let mut v: u32 = 0;
    for k in 0..f.bit_size {
        let bit = f.bit_offset + k;
        let byte = (bit / 8) as usize;
        if byte >= data.len() { break; }
        if (data[byte] >> (bit % 8)) & 1 != 0 {
            v |= 1 << k;
        }
    }
    // Ein Feld mit negativem Minimum ist vorzeichenbehaftet.
    if f.logical_min < 0 && f.bit_size < 32 {
        let sign = 1u32 << (f.bit_size - 1);
        if v & sign != 0 {
            return (v | !((1u32 << f.bit_size) - 1)) as i32;
        }
    }
    v as i32
}

/// Einen Wert IN einen Bericht schreiben — das Gegenstueck zu
/// [`extract`]. Bits ausserhalb des Puffers fallen weg.
pub fn insert(data: &mut [u8], f: &Field, value: i32) {
    if f.bit_size == 0 || f.bit_size > 32 { return; }
    let v = value as u32;
    for k in 0..f.bit_size {
        let bit = f.bit_offset + k;
        let byte = (bit / 8) as usize;
        if byte >= data.len() { break; }
        let mask = 1u8 << (bit % 8);
        if (v >> k) & 1 != 0 { data[byte] |= mask; } else { data[byte] &= !mask; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Der Boot-Maus-Deskriptor aus der HID-Spezifikation (Appendix E.10).
    /// Drei Knoepfe, fuenf Fuellbits, X und Y als relative Bytes.
    const BOOT_MOUSE: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x02, // Usage (Mouse)
        0xA1, 0x01, // Collection (Application)
        0x09, 0x01, //   Usage (Pointer)
        0xA1, 0x00, //   Collection (Physical)
        0x05, 0x09, //     Usage Page (Button)
        0x19, 0x01, //     Usage Minimum (1)
        0x29, 0x03, //     Usage Maximum (3)
        0x15, 0x00, //     Logical Minimum (0)
        0x25, 0x01, //     Logical Maximum (1)
        0x95, 0x03, //     Report Count (3)
        0x75, 0x01, //     Report Size (1)
        0x81, 0x02, //     Input (Data,Var,Abs)
        0x95, 0x01, //     Report Count (1)
        0x75, 0x05, //     Report Size (5)
        0x81, 0x01, //     Input (Cnst)
        0x05, 0x01, //     Usage Page (Generic Desktop)
        0x09, 0x30, //     Usage (X)
        0x09, 0x31, //     Usage (Y)
        0x15, 0x81, //     Logical Minimum (-127)
        0x25, 0x7F, //     Logical Maximum (127)
        0x75, 0x08, //     Report Size (8)
        0x95, 0x02, //     Report Count (2)
        0x81, 0x06, //     Input (Data,Var,Rel)
        0xC0,       //   End Collection
        0xC0,       // End Collection
    ];

    #[test]
    fn boot_mouse_layout() {
        let m = parse(BOOT_MOUSE);
        assert!(!m.uses_ids, "der Boot-Maus-Deskriptor hat keine Report-IDs");
        let id = m.pointer_report().expect("X und Y gefunden");
        assert_eq!(id, 0);

        let x = m.find(0, PAGE_GENERIC_DESKTOP, USAGE_X).unwrap();
        let y = m.find(0, PAGE_GENERIC_DESKTOP, USAGE_Y).unwrap();
        // 3 Knopfbits + 5 Fuellbits = Byte 1, dann X, dann Y.
        assert_eq!(x.bit_offset, 8);
        assert_eq!(y.bit_offset, 16);
        assert_eq!(x.bit_size, 8);
        assert!(x.relative);
        assert_eq!(x.logical_min, -127);

        // Knoepfe: Usage Minimum 1 laeuft ueber die drei Felder hoch.
        let b1 = m.find(0, PAGE_BUTTON, 1).expect("Knopf 1");
        let b3 = m.find(0, PAGE_BUTTON, 3).expect("Knopf 3");
        assert_eq!(b1.bit_offset, 0);
        assert_eq!(b3.bit_offset, 2);
    }

    /// Ein Bericht: linke Taste, 5 nach rechts, 3 nach oben.
    #[test]
    fn boot_mouse_values() {
        let m = parse(BOOT_MOUSE);
        let data = [0x01u8, 5, (-3i8) as u8];
        let x = m.find(0, PAGE_GENERIC_DESKTOP, USAGE_X).unwrap();
        let y = m.find(0, PAGE_GENERIC_DESKTOP, USAGE_Y).unwrap();
        let b1 = m.find(0, PAGE_BUTTON, 1).unwrap();
        assert_eq!(extract(&data, x), 5);
        assert_eq!(extract(&data, y), -3, "negativ, weil logical_min < 0");
        assert_eq!(extract(&data, b1), 1);
    }

    /// Der ECHTE Deskriptor von Florians Touchpad (Elan ELAN06FA,
    /// `vid=0x04f3 pid=0x31ad`, 381 Bytes, vom Geraet gelesen).
    ///
    /// Er haelt genau das fest, was uns eine Fehlersuche gekostet hat:
    /// Bericht 4 fuehrt **einen** Kontaktplatz und ein `Contact Count`.
    /// Wer daraus schliesst, das Pad erkenne nur einen Finger, sucht den
    /// Fehler danach an der falschen Stelle.
    #[test]
    fn elan_touchpad_report_layout() {
        let d = include_bytes!("../testdata/elan06fa.bin");
        let m = parse(d);
        assert_eq!(m.contact_slots(4), 1, "EIN Platz — das Geraet sendet mehrere Berichte");

        let tip = m.find(4, PAGE_DIGITIZER, USAGE_TIP_SWITCH).expect("tip switch");
        assert_eq!((tip.bit_offset, tip.bit_size), (1, 1));

        let x = m.find(4, PAGE_GENERIC_DESKTOP, USAGE_X).expect("X");
        assert_eq!((x.bit_offset, x.bit_size), (8, 16));
        assert_eq!(x.logical_max, 3679);
        assert!(!x.relative, "ein Touchpad meldet ORTE");

        let y = m.find(4, PAGE_GENERIC_DESKTOP, USAGE_Y).expect("Y");
        assert_eq!((y.bit_offset, y.bit_size), (24, 16));
        assert_eq!(y.logical_max, 2261);

        let cc = m.find(4, PAGE_DIGITIZER, USAGE_CONTACT_COUNT).expect("contact count");
        assert_eq!((cc.bit_offset, cc.bit_size), (56, 8));

        let b1 = m.find(4, PAGE_BUTTON, 1).expect("Klickflaeche");
        assert_eq!(b1.bit_offset, 64);

        // 88 Bit = 11 Byte, plus Berichtsnummer und zwei Laengenbytes = 14
        // — genau das `wMaxInputLength`, das das Geraet ansagt.
        //
        // Und der Umschalter in den Praezisionsmodus muss auffindbar sein,
        // sonst sendet es ueberhaupt keinen Bericht 4.
        let im = m.find_feature(PAGE_DIGITIZER, USAGE_INPUT_MODE).expect("Device Mode");
        assert_eq!(im.report_id, 3);

        // Und der Block, der frueher 1634 Felder erzeugt hat, erzeugt jetzt
        // keine: Fuellbits sind keine Felder.
        assert!(m.fields.len() < 100, "{} Felder aus 381 Bytes", m.fields.len());
    }

    /// Der Umschalter in den Praezisionsmodus ist ZWEI Bytes lang.
    ///
    /// Am Geraet quittierte das Elan einen Ein-Byte-Feature-Bericht auf
    /// dem Bus und schaltete NICHT um — es kam weiter nur die
    /// Maus-Nachahmung. Im Deskriptor steht `Report Size 16` an
    /// `Input Mode`, und ein zu kurzer Feature-Bericht wird verworfen.
    /// Die Laenge gehoert deshalb aus dem Deskriptor gerechnet und nicht
    /// geraten — samt Fuellbits, die keine Felder sind.
    #[test]
    fn a_feature_report_is_as_long_as_the_descriptor_says() {
        let d = include_bytes!("../testdata/elan06fa.bin");
        let m = parse(d);

        let im = m.find_feature(PAGE_DIGITIZER, USAGE_INPUT_MODE).expect("Device Mode");
        assert_eq!(im.report_id, 3);
        assert_eq!(im.bit_size, 16, "Report Size 16 — nicht 8");
        assert_eq!(m.report_bytes(Kind::Feature, 3), 2);

        // Und so sieht die Nutzlast aus, die das Geraet erwartet.
        let mut payload = alloc::vec![0u8; m.report_bytes(Kind::Feature, 3)];
        insert(&mut payload, im, 3);
        assert_eq!(&payload[..], &[0x03, 0x00]);

        // Bericht 5 endet auf 14 Fuellbits: aus den FELDERN allein kaeme
        // ein Byte heraus, richtig sind zwei.
        assert_eq!(m.report_bytes(Kind::Feature, 5), 2);
    }

    /// Schreiben und Lesen muessen sich treffen, auch quer ueber eine
    /// Bytegrenze und mit Vorzeichen.
    #[test]
    fn insert_and_extract_are_inverse() {
        let f = Field {
            kind: Kind::Feature, report_id: 1, usage_page: 0, usage: 0,
            bit_offset: 5, bit_size: 12, logical_min: -2048, logical_max: 2047,
            relative: false, constant: false,
        };
        let mut b = [0u8; 4];
        for v in [0i32, 1, -1, 2047, -2048, 1234] {
            insert(&mut b, &f, v);
            assert_eq!(extract(&b, &f), v, "{v}");
        }
    }

    /// `bSize == 3` heisst VIER Bytes. Wer drei liest, verschiebt alles
    /// danach — und merkt es erst an unsinnigen Koordinaten.
    #[test]
    fn four_byte_items_are_four_bytes() {
        // Logical Maximum (0x00FFFFFF) als 4-Byte-Item, dann X als 16 Bit.
        let d = &[
            0x05, 0x01,
            0x27, 0xFF, 0xFF, 0xFF, 0x00, // Logical Maximum, bSize=3 -> 4 Bytes
            0x15, 0x00,
            0x75, 0x10, 0x95, 0x01,
            0x09, 0x30,
            0x81, 0x02,
        ];
        let m = parse(d);
        let x = m.find(0, PAGE_GENERIC_DESKTOP, USAGE_X).expect("X");
        assert_eq!(x.bit_size, 16);
        assert_eq!(x.logical_max, 0x00FF_FFFF);
    }
}
