use super::*;
use crate::input::{Input, Media};

/// HP Wireless Keyboard and Mouse (`03f0:5341`), interface 0: a boot
/// keyboard. Read from `/sys/bus/usb/devices/*/report_descriptor`.
const HP_KBD: &[u8] = include_bytes!("../testdata/hp5341_kbd.bin");
/// The same dongle, interface 1: mouse (report 1), a vendor feature
/// (report 3), consumer control (report 4), system control (report 5).
const HP_MOUSE: &[u8] = include_bytes!("../testdata/hp5341_mouse.bin");

fn feed(dev: &mut Device, input: &mut Input, report: &[u8]) -> input::Frame {
    dev.input_report(report, &mut |e| input.event(&e)).expect("report");
    input.sync()
}

#[test]
fn keyboard_layout() {
    let d = parse(HP_KBD).expect("parse");
    assert_eq!(d.applications().collect::<Vec<_>>(), [GD_KEYBOARD]);
    let inp = &d.report_enum[ReportType::Input as usize];
    assert!(!inp.numbered);
    let r = inp.get(0).expect("report 0");
    assert_eq!(r.size, 64, "modifiers, reserved byte, six key slots");
    // Modifiers: one variable field, eight usages E0..E7. The reserved byte
    // has no usage and registers no field.
    assert_eq!(r.fields.len(), 2);
    let m = &r.fields[0];
    assert!(m.is_variable());
    assert_eq!(m.usage.len(), 8);
    assert_eq!(m.usage[0].hid, UP_KEYBOARD | 0xe0);
    // Keys: an array of six 8-bit slots over usages 0..255.
    let k = &r.fields[1];
    assert!(!k.is_variable());
    assert_eq!((k.report_offset, k.report_size, k.report_count), (16, 8, 6));
    assert_eq!(k.usage.len(), 256);
    assert_eq!(k.usage[4].hid, UP_KEYBOARD | 4);
    assert_eq!(d.max_report_bytes(ReportType::Input), 8);
}

#[test]
fn keyboard_reports() {
    let mut d = parse(HP_KBD).unwrap();
    let mut i = Input::default();

    // Left Shift + 'a'.
    feed(&mut d, &mut i, &[0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(i.modifiers(), 0x02);
    assert_eq!(i.non_modifier_keys().collect::<Vec<_>>(), [0x04]);

    // 'b' joins, 'a' stays: the slot order does not matter.
    feed(&mut d, &mut i, &[0x00, 0, 0x05, 0x04, 0, 0, 0, 0]);
    assert_eq!(i.modifiers(), 0);
    assert_eq!(i.non_modifier_keys().collect::<Vec<_>>(), [0x04, 0x05]);

    // ErrorRollOver in every slot: the report is ignored, nothing released.
    let f = feed(&mut d, &mut i, &[0x00, 0, 1, 1, 1, 1, 1, 1]);
    assert!(!f.keys_changed);
    assert_eq!(i.non_modifier_keys().collect::<Vec<_>>(), [0x04, 0x05]);

    // All up.
    feed(&mut d, &mut i, &[0; 8]);
    assert!(i.keys().is_empty());
}

#[test]
fn keyboard_led_report() {
    let d = parse(HP_KBD).unwrap();
    // NumLock is LED usage 1; the report is five LED bits and three padding.
    let id = d.find_output_usage(UP_LED | 0x01).expect("NumLock");
    assert_eq!(id, 0);
    assert_eq!(d.output_report_zeroed(id).unwrap(), [0u8]);
}

#[test]
fn mouse_interface_layout() {
    let d = parse(HP_MOUSE).expect("parse");
    assert_eq!(d.applications().collect::<Vec<_>>(),
        [GD_MOUSE, 0xff00_0001, UP_CONSUMER | 1, UP_GENDESK | 0x80]);
    let inp = &d.report_enum[ReportType::Input as usize];
    assert!(inp.numbered);
    let ids: Vec<u32> = inp.reports.iter().map(|r| r.id).collect();
    assert_eq!(ids, [1, 4, 5], "report 3 is a feature");
    // 8 buttons + 2 x 12-bit motion + 8-bit wheel = 40 bits, plus the ID.
    assert_eq!(inp.get(1).unwrap().size, 40);
    assert_eq!(d.max_report_bytes(ReportType::Input), 6);
    let x = &inp.get(1).unwrap().fields[1];
    assert_eq!((x.report_offset, x.report_size, x.report_count), (8, 12, 2));
    assert_eq!((x.logical_minimum, x.logical_maximum), (-2047, 2047));
    assert!(x.is_relative());
}

#[test]
fn mouse_motion_buttons_wheel() {
    let mut d = parse(HP_MOUSE).unwrap();
    let mut i = Input::default();
    let mut r = [1u8, 0, 0, 0, 0, 0];
    r[1] = 0x01; // left button
    field_implement(&mut r[1..], 8, 12, s32ton(300, 12));
    field_implement(&mut r[1..], 20, 12, s32ton(-3, 12));
    r[5] = 0x01; // wheel +1
    let f = feed(&mut d, &mut i, &r);
    assert_eq!((f.dx, f.dy, f.wheel), (300, -3, 1), "12-bit motion survives");
    assert!(f.buttons_changed);
    assert_eq!(i.buttons, 0x01);

    // Same buttons, no motion: nothing changed.
    let f = feed(&mut d, &mut i, &[1, 0x01, 0, 0, 0, 0]);
    assert!(!f.buttons_changed);
    assert_eq!((f.dx, f.dy, f.wheel), (0, 0, 0));

    // Release; right button.
    let f = feed(&mut d, &mut i, &[1, 0x02, 0, 0, 0, 0]);
    assert!(f.buttons_changed);
    assert_eq!(i.buttons, 0x02);
}

#[test]
fn consumer_and_system_control() {
    let mut d = parse(HP_MOUSE).unwrap();
    let mut i = Input::default();
    // Volume up pressed, then released: one action.
    let f = feed(&mut d, &mut i, &[4, 0xe9, 0x00]);
    assert_eq!(f.media.as_slice(), [Media::VolumeUp]);
    let f = feed(&mut d, &mut i, &[4, 0x00, 0x00]);
    assert!(f.media.is_empty());
    let f = feed(&mut d, &mut i, &[4, 0xe2, 0x00]);
    assert_eq!(f.media.as_slice(), [Media::Mute]);
    // Play/Pause has no consumer here: reported, not dropped.
    let f = feed(&mut d, &mut i, &[4, 0xcd, 0x00]);
    assert_eq!(f.unmapped.as_slice(), [UP_CONSUMER | 0xcd]);
    // System Power Down, a variable bit.
    let f = feed(&mut d, &mut i, &[5, 0x01]);
    assert_eq!(f.unmapped.as_slice(), [UP_GENDESK | 0x81]);
}

#[test]
fn short_and_unknown_reports() {
    let mut d = parse(HP_MOUSE).unwrap();
    let mut i = Input::default();
    // A report ID the descriptor does not name is dropped quietly.
    let f = feed(&mut d, &mut i, &[9, 1, 2, 3]);
    assert_eq!((f.dx, f.media.is_empty()), (0, true));
    // A short report is padded with zeros (`hid_report_raw_event`).
    let f = feed(&mut d, &mut i, &[1, 0x01]);
    assert!(f.buttons_changed);
    // A numbered device needs its ID byte.
    assert_eq!(d.input_report(&[], &mut |_| {}), Err(Error::ShortReport));
}

/// The boot mouse descriptor from the HID specification (Appendix E.10).
#[test]
fn boot_mouse_spec() {
    let desc: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01,
        0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
        0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81,
        0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0,
    ];
    let mut d = parse(desc).unwrap();
    let mut i = Input::default();
    let f = feed(&mut d, &mut i, &[0x04, 5, (-7i8) as u8]);
    assert_eq!(i.buttons, 0x04, "middle");
    assert_eq!((f.dx, f.dy), (5, -7));
}

#[test]
fn bad_descriptors() {
    // Collection never closed.
    assert_eq!(parse(&[0x05, 0x01, 0xA1, 0x01]).unwrap_err(), Error::UnbalancedCollection);
    // Report ID 0 is invalid.
    assert!(matches!(parse(&[0x85, 0x00]), Err(Error::Item(0))));
    // An item cut off at the end.
    assert!(matches!(parse(&[0x05]), Err(Error::Truncated(0))));
}

#[test]
fn sign_helpers() {
    assert_eq!(snto32(0xfff, 12), -1);
    assert_eq!(snto32(0x7ff, 12), 2047);
    assert_eq!(s32ton(-1, 12), 0xfff);
    assert_eq!(s32ton(5000, 12), 0x7ff, "saturates");
    assert_eq!(field_extract(&[0xab, 0xcd, 0xef], 4, 12), 0xcda);
}
