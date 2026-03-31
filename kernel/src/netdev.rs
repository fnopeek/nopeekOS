//! Network Device Abstraction
//!
//! Dispatches to virtio-net or Intel NIC, whichever is available.

use crate::{virtio_net, intel_nic};
use crate::virtio_net::NetError;

pub const MTU: usize = 1514;

pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if intel_nic::is_available() {
        intel_nic::send(frame)
    } else {
        virtio_net::send(frame)
    }
}

pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    if intel_nic::is_available() {
        intel_nic::recv(buf)
    } else {
        virtio_net::recv(buf)
    }
}

pub fn mac() -> Option<[u8; 6]> {
    if intel_nic::is_available() {
        intel_nic::mac()
    } else {
        virtio_net::mac()
    }
}

pub fn is_available() -> bool {
    intel_nic::is_available() || virtio_net::is_available()
}
