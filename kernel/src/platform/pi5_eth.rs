//! The Raspberry Pi 5's Ethernet: RP1's GEM (Cadence, `macb` in Linux)
//! with its BCM54213PE PHY, as the network device `netdev` dispatches to.
//! The register work is `pi5_core`'s port of Linux; this module holds the
//! bound device and gives it to the stack.

use crate::netdev::{NetError, MTU};
use spin::Mutex;

/// The bound device. None until `init` found and opened it.
struct Device {
    mac: [u8; 6],
    link: bool,
}

static DEV: Mutex<Option<Device>> = Mutex::new(None);

pub fn is_available() -> bool {
    DEV.lock().is_some()
}

pub fn link_up() -> bool {
    DEV.lock().as_ref().is_some_and(|d| d.link)
}

pub fn mac() -> Option<[u8; 6]> {
    DEV.lock().as_ref().map(|d| d.mac)
}

pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU {
        return Err(NetError::FrameTooLarge);
    }
    Err(NetError::NotInitialized)
}

pub fn recv(_buf: &mut [u8; MTU]) -> Option<usize> {
    None
}
