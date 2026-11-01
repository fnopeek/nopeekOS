//! Contact points of a touch screen become an absolute pointer.
//!
//! A direct device (`INPUT_MT_DIRECT` in hid-multitouch) points where the
//! finger is; there is no motion to derive and no gesture. The pointer
//! follows the oldest contact still down, as `input_mt_report_pointer_emulation`
//! does, and the left button is held while any contact is down.
//!
//! Frames are assembled as in `mt_touch_report`: the report with a non-zero
//! Contact Count opens a frame, and it is complete once that many slots
//! arrived. A device that stops reporting while fingers are down releases
//! them after `STICKY_MS` (`MT_QUIRK_STICKY_FINGERS`, `mt_expired_timeout`).

use alloc::vec::Vec;

/// A contact: ID, X, Y in device units.
pub type Contact = (i32, i32, i32);

/// hid-multitouch's sticky-finger timeout.
pub const STICKY_MS: u64 = 100;

/// Where the pointer goes: position on 0..=65535 per axis, and whether a
/// contact is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
    pub down: bool,
}

/// Logical range of one axis.
#[derive(Clone, Copy, Debug)]
pub struct Axis {
    pub min: i32,
    pub max: i32,
}

impl Axis {
    /// A device value on 0..=65535.
    pub fn scale(&self, v: i32) -> u16 {
        let span = (self.max as i64 - self.min as i64).max(1);
        let off = (v as i64 - self.min as i64).clamp(0, span);
        (off * 65535 / span) as u16
    }
}

pub struct Tracker {
    x: Axis,
    y: Axis,
    max_contacts: usize,
    frame: Vec<Contact>,
    expected: usize,
    collected: usize,
    in_frame: bool,
    /// Contacts down, oldest first.
    order: Vec<i32>,
    last: Option<Point>,
    last_ms: u64,
}

impl Tracker {
    pub fn new(x: Axis, y: Axis, max_contacts: usize) -> Self {
        Tracker {
            x, y, max_contacts: max_contacts.max(1),
            frame: Vec::new(), expected: 0, collected: 0, in_frame: false,
            order: Vec::new(), last: None, last_ms: 0,
        }
    }

    /// One report: its Contact Count (-1 without the field), the contacts
    /// touching in it (palms already left out) and its number of slots.
    /// Returns the pointer once a frame is complete.
    pub fn feed(&mut self, cc: i32, present: &[Contact], slots: usize, now_ms: u64) -> Option<Point> {
        self.last_ms = now_ms;
        if cc > 0 || !self.in_frame {
            self.expected = if cc > 0 { cc as usize } else { 0 };
            self.frame.clear();
            self.collected = 0;
            self.in_frame = true;
        }
        for c in present {
            if self.frame.len() < self.max_contacts && !self.frame.iter().any(|f| f.0 == c.0) {
                self.frame.push(*c);
            }
        }
        self.collected += slots;
        if self.collected < self.expected {
            return None;
        }
        self.in_frame = false;
        self.order.retain(|id| self.frame.iter().any(|c| c.0 == *id));
        for c in &self.frame {
            if !self.order.contains(&c.0) { self.order.push(c.0); }
        }
        let p = match self.order.first().and_then(|id| self.frame.iter().find(|c| c.0 == *id)) {
            Some(&(_, x, y)) => Point { x: self.x.scale(x), y: self.y.scale(y), down: true },
            None => {
                let (x, y) = self.last.map(|p| (p.x, p.y)).unwrap_or((0, 0));
                Point { x, y, down: false }
            }
        };
        self.last = Some(p);
        Some(p)
    }

    /// When `tick` must run next: while a contact is down.
    pub fn next_deadline(&self) -> Option<u64> {
        if self.last.is_some_and(|p| p.down) { Some(self.last_ms + STICKY_MS) } else { None }
    }

    /// Releases contacts the device stopped reporting.
    pub fn tick(&mut self, now_ms: u64) -> Option<Point> {
        let p = self.last?;
        if !p.down || now_ms < self.last_ms + STICKY_MS { return None; }
        self.order.clear();
        self.in_frame = false;
        let up = Point { down: false, ..p };
        self.last = Some(up);
        Some(up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Tracker {
        Tracker::new(Axis { min: 0, max: 1000 }, Axis { min: 0, max: 500 }, 10)
    }

    #[test]
    fn scales_to_the_full_range() {
        let a = Axis { min: 0, max: 1000 };
        assert_eq!(a.scale(0), 0);
        assert_eq!(a.scale(1000), 65535);
        assert_eq!(a.scale(500), 32767);
        assert_eq!(a.scale(-5), 0);
        assert_eq!(a.scale(2000), 65535);
    }

    #[test]
    fn down_move_up() {
        let mut t = t();
        assert_eq!(t.feed(1, &[(7, 1000, 500)], 1, 0), Some(Point { x: 65535, y: 65535, down: true }));
        assert_eq!(t.feed(1, &[(7, 0, 0)], 1, 10), Some(Point { x: 0, y: 0, down: true }));
        assert_eq!(t.feed(0, &[], 1, 20), Some(Point { x: 0, y: 0, down: false }));
    }

    #[test]
    fn follows_the_oldest_contact() {
        let mut t = t();
        t.feed(1, &[(1, 100, 100)], 1, 0);
        let p = t.feed(2, &[(2, 900, 400), (1, 200, 100)], 2, 10).unwrap();
        assert_eq!(p.x, Axis { min: 0, max: 1000 }.scale(200));
        let p = t.feed(1, &[(2, 900, 400)], 2, 20).unwrap();
        assert!(p.down);
        assert_eq!(p.x, Axis { min: 0, max: 1000 }.scale(900));
    }

    #[test]
    fn hybrid_frame_waits_for_all_slots() {
        let mut t = t();
        assert_eq!(t.feed(2, &[(1, 100, 100)], 1, 0), None);
        assert!(t.feed(0, &[(2, 300, 300)], 1, 1).unwrap().down);
    }

    #[test]
    fn sticky_fingers_release() {
        let mut t = t();
        t.feed(1, &[(1, 100, 100)], 1, 0);
        assert_eq!(t.next_deadline(), Some(STICKY_MS));
        assert_eq!(t.tick(50), None);
        assert!(!t.tick(STICKY_MS).unwrap().down);
        assert_eq!(t.next_deadline(), None);
        assert_eq!(t.tick(500), None);
    }
}
