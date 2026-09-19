//! i2c_hid_core — HID over I2C auf einem Synopsys-Designware-Bus.
//!
//! no_std + alloc; dieselbe Bibliothek baut fuer den std-Pruefstand und fuer
//! das wasm32-Modul. Quelle fuer jede Zeile ist der Linux-Treiber
//! (6.18.26, im Cache unter `~/.cache/nopeekos/linux-src/`); jede Abweichung
//! steht als Kommentar an ihrer Stelle.
//!
//! Aufbau in der Reihenfolge, in der es laeuft:
//!
//! 1. [`discover`] — was die Firmware sagt (DSDT): Controller, Slave-Adresse,
//!    Deskriptor-Register, GPIO-Pin.
//! 2. [`dw_i2c`] — der Synopsys-Designware-Bus, auf dem das Geraet haengt.
//! 3. [`hid`] — HID over I2C: Deskriptor, Power, Reset, Eingabeberichte.
//! 4. [`report`] — der Report-Deskriptor: was ein Byte im Bericht bedeutet.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod discover;
pub mod dw_i2c;
pub mod hid;
pub mod report;
