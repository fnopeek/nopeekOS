//! RX block-ack reorder buffer — port of `iwl_mvm_reorder` (mvm/rxmq.c).
//!
//! An aggregating AP sends a whole A-MPDU and expects a single block ack, which
//! means frames may arrive with holes: MPDU 5 can be on the air before the
//! retransmission of MPDU 3. Handing that to the IP stack as-is looks like
//! reordering to TCP and costs more than the aggregation gains, so a receiver
//! that accepts a block-ack session MUST hold frames back until the gap closes.
//! That buffer is this file, and it is the whole reason we declined ADDBA before.
//!
//! Differences from Linux, all because the firmware does the hard part: it hands
//! us the session id, this frame's sequence number and the "next expected"
//! (NSSN) in every descriptor, so there is no window arithmetic to derive. And
//! with one RX queue there is one buffer per session instead of one per queue.
//!
//! Frames are stored DECODED (Ethernet, as `rx_classify` produced them) — the
//! 802.11 header has done its job by then and the reorder decision needs only
//! the descriptor.

use crate::host;
use crate::regs::*;

/// Window we advertise in the ADDBA response, in MPDUs. The AP usually asks for
/// 64; a responder may answer with less and 32 already collapses the per-frame
/// overhead (32 frames share one preamble and one block ack). It also halves the
/// memory: the buffer is real storage in our linear memory, not a pointer list
/// like Linux's skb queue.
pub const BA_WIN: usize = 32;

/// Longest frame we keep. `rx_classify` already caps decoded frames at 1600.
const BA_FRAME_MAX: usize = 1600;

/// Sequence numbers are 12-bit and wrap.
const SN_MODULO: u16 = 1 << 12;

/// a < b in 12-bit sequence space (ieee80211_sn_less).
pub fn sn_less(a: u16, b: u16) -> bool {
    ((a.wrapping_sub(b)) & (SN_MODULO - 1)) > SN_MODULO / 2
}
pub fn sn_inc(sn: u16) -> u16 {
    (sn + 1) & (SN_MODULO - 1)
}
pub fn sn_add(sn: u16, n: u16) -> u16 {
    (sn + n) & (SN_MODULO - 1)
}

/// Frame storage. Static rather than a field of the driver struct: the driver
/// lives on the stack in `_start`, and 51 KB of window does not belong there.
static mut SLOT: [[u8; BA_FRAME_MAX]; BA_WIN] = [[0; BA_FRAME_MAX]; BA_WIN];
/// Payload length per slot, 0 = empty.
static mut SLOT_LEN: [u16; BA_WIN] = [0; BA_WIN];

static mut REORDER: Reorder = Reorder::NEW;

/// The one session. Free function rather than a driver field because the RX path
/// classifies frames inside a closure that cannot also borrow the driver.
pub fn session() -> &'static mut Reorder {
    // SAFETY: single-threaded WASM module; no host call re-enters the RX path.
    unsafe { &mut *(&raw mut REORDER) }
}

/// One RX aggregation session. We run at most one — the AP opens BE (TID 0) for
/// bulk traffic, and a second session on another TID would need its own window.
pub struct Reorder {
    /// Firmware session id, or INVALID while no session is up.
    pub baid: u8,
    pub tid: u8,
    /// Next sequence number we expect to deliver.
    pub head_sn: u16,
    pub stored: u16,
    /// False until the first in-window frame; a session that starts mid-burst
    /// must not treat the frames already on the air as holes.
    pub valid: bool,
    /// `now_ms` of the last delivery, for the stall release below.
    pub last_move_ms: u64,
    // Counters, all reported by `wlan`.
    pub delivered: u32,
    pub buffered: u32,
    pub dups: u32,
    pub old_sn: u32,
    pub stalls: u32,
}

/// A hole that never fills would park the window forever: the AP is supposed to
/// close it with a BAR, but a BAR that is itself lost leaves the buffer holding
/// frames the stack needs. Linux runs a per-session timer; we check the clock on
/// the pass that notices stored frames, which is the same guarantee without a
/// timer we do not have.
const STALL_MS: u64 = 60;

impl Reorder {
    pub const NEW: Reorder = Reorder {
        baid: IWL_RX_REORDER_DATA_INVALID_BAID,
        tid: 0,
        head_sn: 0,
        stored: 0,
        valid: false,
        last_move_ms: 0,
        delivered: 0,
        buffered: 0,
        dups: 0,
        old_sn: 0,
        stalls: 0,
    };

    pub fn active(&self) -> bool {
        self.baid != IWL_RX_REORDER_DATA_INVALID_BAID
    }

    /// Session accepted by the firmware: it answered our ADD_STA with this id.
    pub fn start(&mut self, baid: u8, tid: u8, ssn: u16) {
        self.flush();
        self.baid = baid;
        self.tid = tid;
        self.head_sn = ssn;
        self.valid = false;
        self.last_move_ms = host::now_ms();
    }

    /// Session gone (DELBA, deauth, reassociation). Everything still held goes
    /// up: out of order beats never.
    pub fn stop(&mut self) {
        self.flush();
        self.baid = IWL_RX_REORDER_DATA_INVALID_BAID;
        self.valid = false;
    }

    /// Deliver every stored frame regardless of holes.
    fn flush(&mut self) {
        if self.stored == 0 {
            return;
        }
        for i in 0..BA_WIN {
            self.emit_slot(i);
        }
        self.stored = 0;
    }

    fn emit_slot(&mut self, index: usize) {
        // SAFETY: single-threaded module; the slot arrays are touched only here
        // and in `store`, never across a host call that could re-enter.
        let len = unsafe { SLOT_LEN[index] } as usize;
        if len == 0 {
            return;
        }
        unsafe {
            SLOT_LEN[index] = 0;
            let slots = &*(&raw const SLOT);
            host::netdev_submit_rx(&slots[index][..len]);
        }
        self.delivered = self.delivered.wrapping_add(1);
    }

    fn store(&mut self, sn: u16, frame: &[u8]) -> bool {
        if frame.len() > BA_FRAME_MAX {
            return false;
        }
        let index = (sn as usize) % BA_WIN;
        // SAFETY: as in emit_slot.
        unsafe {
            if SLOT_LEN[index] != 0 {
                // Slot occupied by a frame a full window away — the window has
                // outrun itself. Release the old one rather than lose it.
                self.emit_slot(index);
                self.stored = self.stored.saturating_sub(1);
            }
            let slots = &mut *(&raw mut SLOT);
            slots[index][..frame.len()].copy_from_slice(frame);
            SLOT_LEN[index] = frame.len() as u16;
        }
        if self.stored == 0 {
            // The clock the stall release runs on starts when a hole appears —
            // not on the last delivery. Refreshing it on every release would
            // keep it from ever firing while traffic flows, which is exactly
            // when a hole hurts.
            self.last_move_ms = host::now_ms();
        }
        self.stored += 1;
        self.buffered = self.buffered.wrapping_add(1);
        true
    }

    /// Deliver everything below `nssn` (iwl_mvm_release_frames). Empty slots are
    /// normal: NSSN moving past a sequence number means the firmware saw it, not
    /// that we hold it.
    pub fn release_upto(&mut self, nssn: u16) {
        // A jump FORWARD wider than the window means everything held is below
        // nssn — walk the slots once instead of stepping through up to 2047
        // sequence numbers one at a time in the RX path. Only forward: NSSN can
        // legitimately sit behind head_sn after a stall release, and treating
        // that as a jump would drag the window backwards.
        let ahead = sn_less(self.head_sn, nssn);
        if ahead && (nssn.wrapping_sub(self.head_sn) & (SN_MODULO - 1)) as usize > BA_WIN {
            self.flush();
            self.head_sn = nssn;
            return;
        }
        let mut ssn = self.head_sn;
        while sn_less(ssn, nssn) {
            let index = (ssn as usize) % BA_WIN;
            // SAFETY: as in emit_slot.
            if unsafe { SLOT_LEN[index] } != 0 {
                self.emit_slot(index);
                self.stored = self.stored.saturating_sub(1);
            }
            ssn = sn_inc(ssn);
        }
        self.head_sn = nssn;
    }

    /// A FRAME_RELEASE notification for our session: the firmware advanced the
    /// window without giving us a frame (it saw the MPDUs on air).
    pub fn on_frame_release(&mut self, baid: u8, nssn: u16) {
        if !self.active() || baid != self.baid {
            return;
        }
        self.release_upto(nssn);
    }

    /// A hole that has stood for STALL_MS gives up its claim. Call once per poll
    /// pass; it costs one clock read when frames are held and nothing otherwise.
    pub fn tick(&mut self) {
        if self.stored == 0 {
            return;
        }
        if host::now_ms().wrapping_sub(self.last_move_ms) < STALL_MS {
            return;
        }
        self.stalls = self.stalls.wrapping_add(1);
        // Release the whole window: head_sn moves past everything we hold, so a
        // late arrival is then correctly treated as old rather than re-buffered.
        let upto = sn_add(self.head_sn, BA_WIN as u16);
        self.release_upto(upto);
    }

    /// The decision, per received data frame. `true` = we took the frame (it is
    /// buffered or dropped); `false` = caller delivers it now.
    ///
    /// Mirrors iwl_mvm_reorder's order exactly: invalid BAID and non-session
    /// frames pass through, duplicates and outdated frames are dropped, an
    /// in-order frame with nothing held is delivered without touching the
    /// buffer, and only a frame that actually sits ahead of a hole is stored.
    pub fn on_frame(&mut self, reorder: u32, status: u32, amsdu_last: bool, frame: &[u8]) -> bool {
        let baid = ((reorder & IWL_RX_MPDU_REORDER_BAID_MASK) >> IWL_RX_MPDU_REORDER_BAID_SHIFT) as u8;
        if baid == IWL_RX_REORDER_DATA_INVALID_BAID || !self.active() || baid != self.baid {
            return false;
        }

        let nssn = (reorder & IWL_RX_MPDU_REORDER_NSSN_MASK) as u16;
        let sn = ((reorder & IWL_RX_MPDU_REORDER_SN_MASK) >> IWL_RX_MPDU_REORDER_SN_SHIFT) as u16;

        if !self.valid {
            // Do not start on a frame the firmware already considers old — that
            // is the tail of a burst that began before the session. head_sn
            // stays at the SSN the session was set up with; NSSN pulls it
            // forward on the first release.
            if reorder & IWL_RX_MPDU_REORDER_BA_OLD_SN != 0 {
                return false;
            }
            self.valid = true;
        }

        if status & IWL_RX_MPDU_STATUS_DUPLICATE != 0 {
            self.dups = self.dups.wrapping_add(1);
            return true; // consumed = dropped
        }
        if reorder & IWL_RX_MPDU_REORDER_BA_OLD_SN != 0 {
            self.old_sn = self.old_sn.wrapping_add(1);
            return true;
        }

        // Nothing held and the firmware has already moved past this frame, or it
        // is exactly the one we were waiting for: straight through.
        if self.stored == 0 && sn_less(sn, nssn) {
            if amsdu_last {
                self.head_sn = nssn;
            }
            return false;
        }
        if self.stored == 0 && sn == self.head_sn {
            if amsdu_last {
                self.head_sn = sn_inc(self.head_sn);
            }
            return false;
        }

        if !self.store(sn, frame) {
            return false; // oversized — better out of order than dropped
        }

        // An A-MSDU's NSSN advances on its FIRST sub-frame, so acting on it
        // before the last one arrives would release frames still in flight.
        if amsdu_last {
            self.release_upto(nssn);
        } else if self.stored == 1 {
            self.head_sn = nssn;
        }
        true
    }
}
