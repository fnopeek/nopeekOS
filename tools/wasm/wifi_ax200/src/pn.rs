//! CCMP replay check: `iwl_mvm_check_pn` (mvm/rxmq.c), the driver-side
//! form of mac80211's `ieee80211_crypto_ccmp_decrypt` (wpa.c). The firmware
//! decrypts and checks the MIC, but a frame repeated by a third party
//! decrypts just as well; only its packet number gives it away.
//!
//! As in Linux the check runs where a frame is handed up, after the
//! block-ack reorder buffer (`iwl_mvm_pass_packet_to_mac80211`), so the
//! numbers it sees on a session are in order.

use crate::host;

/// TIDs 0..15 plus one counter for non-QoS frames (`IEEE80211_NUM_TIDS + 1`).
const COUNTERS: usize = 17;

/// What a frame carries into the check.
#[derive(Clone, Copy)]
pub struct Pn {
    /// 48-bit CCMP packet number.
    pub pn: u64,
    /// Counter index: the TID, or 16 for a non-QoS frame.
    pub idx: u8,
    /// Sent to a group address: checked against the group key's counter.
    pub group: bool,
    /// A later sub-frame of an A-MSDU carries the first one's number
    /// (`RX_FLAG_ALLOW_SAME_PN`).
    pub same_ok: bool,
    /// The frame was protected; unprotected frames are not checked here.
    pub valid: bool,
}

impl Pn {
    pub const NONE: Pn = Pn { pn: 0, idx: 0, group: false, same_ok: false, valid: false };
}

/// Last accepted number per counter, pairwise [0] and group [1].
// SAFETY (all statics here): the module is single-threaded and nothing in
// this file calls back into code that could re-enter it.
static mut RX_PN: [[u64; COUNTERS]; 2] = [[0; COUNTERS]; 2];
static mut REPLAYS: u32 = 0;

/// Hand a decoded frame up unless its packet number is a replay.
pub fn deliver(frame: &[u8], p: Pn) {
    if p.valid && !accept(p) {
        // SAFETY: see above.
        unsafe { REPLAYS = REPLAYS.wrapping_add(1) };
        return;
    }
    host::netdev_submit_rx(frame);
}

fn accept(p: Pn) -> bool {
    let idx = (p.idx as usize).min(COUNTERS - 1);
    // SAFETY: see above.
    let last = unsafe { &mut (*(&raw mut RX_PN))[p.group as usize][idx] };
    if p.pn < *last || (p.pn == *last && !p.same_ok) {
        return false;
    }
    *last = p.pn;
    true
}

/// A key was installed: its counters start over, the group key's at the
/// RSC the AP gave with it.
pub fn reset(group: bool, start: u64) {
    // SAFETY: see above.
    unsafe { (*(&raw mut RX_PN))[group as usize] = [start; COUNTERS] };
}

/// Frames dropped as replays since boot.
pub fn replays() -> u32 {
    // SAFETY: see above.
    unsafe { REPLAYS }
}
