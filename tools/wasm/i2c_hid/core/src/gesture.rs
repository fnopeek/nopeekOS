//! Contact points become a pointer and gestures.
//!
//! A touchpad reports positions, not motion, and no gestures: "two fingers
//! move down" is in no report. On Linux this is libinput's job. It lives in
//! the core library rather than the wasm module so it can be tested on the
//! host.
//!
//! Four non-obvious points:
//!
//! 1. A frame can span several reports. A device with one contact slot
//!    sends one report per finger; `Contact Count` is only in the first.
//!    As in `hid-multitouch.c`, the report carrying a contact count opens
//!    the frame.
//! 2. The gesture holds until all fingers lift. Tied to the finger count of
//!    each frame it would flicker whenever a follow-up report is lost, and
//!    resetting the reference point on every change yields zero distance.
//! 3. Pressing the button moves the finger: the fingertip deforms and its
//!    centroid wanders, right when the user aims at something small. So the
//!    fingers are pinned on button press (libinput: `tp_pin_fingers`) and
//!    released only after moving far.
//! 4. A tap presses immediately and releases later; only then can it turn
//!    into a drag when the finger lands again within the window. This needs
//!    a timer the caller must drive (`tick`), or the button stays down.

/// A finger on the pad: ID, position.
pub type Contact = (i32, i32, i32);

/// What a report produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// The frame is not complete yet; fingers are missing.
    Pending,
    /// A complete frame.
    Frame {
        /// Fingers on the pad in this frame.
        n: usize,
        /// The gesture in effect since touch-down.
        gesture: usize,
        dx: i32,
        dy: i32,
        /// Scroll detents, sign like a mouse wheel (positive = up).
        scroll: i32,
        /// Horizontal scroll detents, positive = right (like REL_HWHEEL).
        ///
        /// Separate axis with its own accumulator and step: the pad is
        /// wider than tall, so a step derived from the height would be too
        /// fine horizontally. Both axes run independently, as in libinput;
        /// an axis lock would halve a diagonal swipe.
        hscroll: i32,
        /// A completed tap: press and release in one. 0 = none, 2 = right.
        ///
        /// Only for taps that cannot drag. A one-finger tap goes through
        /// [`Tracker::hold`] because it may still become a drag; a right
        /// click never drags.
        tap: u8,
    },
}

/// Maximum duration of a tap (same value as libinput); longer is a hold.
///
/// The same span then serves as the window: landing again within it drags
/// (or taps a second time) instead of starting over.
const TAP_MS: u64 = 180;

/// State of the tap machine.
///
/// libinput's states (`evdev-mt-touchpad-tap.c`) reduced to what one and
/// two fingers need. After a tap it is not yet decided whether a drag or a
/// second tap follows; the finger's next action decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tap {
    /// Nothing in progress.
    Idle,
    /// A tap has ended, the button is pressed, the window runs from
    /// `since`.
    Held { since: u64 },
    /// A finger landed again within the window; outcome still open.
    DragOrTap,
    /// It is a drag: the button stays down until lift-off.
    Dragging,
}

pub struct Tracker {
    /// Device units per scroll detent, derived from the device's logical
    /// range.
    step: i32,
    /// The same for the horizontal axis, from the width.
    hstep: i32,
    /// How far a finger may move and still count as a tap.
    tap_move: i32,
    /// How far a finger may move under a pressed button before the pointer
    /// follows it again.
    pin_move: i32,
    /// When did the current touch start?
    down_ms: u64,
    /// Distance travelled by the tracked finger since touch-down.
    ///
    /// Not the distance to a remembered point: that point belonged to
    /// `frame[0]`, and the device may reorder fingers in every report,
    /// which would jump the distance by the finger spacing. Instead the
    /// same deltas the pointer gets are summed, and those are zero on a
    /// finger change.
    tap_dx: i32,
    tap_dy: i32,
    moved: bool,
    /// Was a physical button pressed during this touch? Then it was not a
    /// tap; otherwise every press would yield two clicks.
    btn_seen: bool,
    btn_was: bool,
    /// Fingers are pinned: their motion does not reach the pointer.
    pinned: bool,
    pin_dx: i32,
    pin_dy: i32,
    frame: [Contact; 8],
    frame_n: usize,
    expected: usize,
    collected: usize,
    in_frame: bool,
    have_ref: bool,
    track_id: i32,
    rx: i32,
    ry: i32,
    scroll_acc: i32,
    hscroll_acc: i32,
    gesture_n: usize,
    tap_state: Tap,
    hold: u8,
}

impl Tracker {
    pub fn new(step: i32, hstep: i32, tap_move: i32, pin_move: i32) -> Self {
        Tracker {
            step: step.max(1),
            hstep: hstep.max(1),
            tap_move: tap_move.max(1),
            pin_move: pin_move.max(1),
            down_ms: 0,
            tap_dx: 0,
            tap_dy: 0,
            moved: false,
            btn_seen: false,
            btn_was: false,
            pinned: false,
            pin_dx: 0,
            pin_dy: 0,
            frame: [(0, 0, 0); 8],
            frame_n: 0,
            expected: 0,
            collected: 0,
            in_frame: false,
            have_ref: false,
            track_id: -1,
            rx: 0,
            ry: 0,
            scroll_acc: 0,
            hscroll_acc: 0,
            gesture_n: 0,
            tap_state: Tap::Idle,
            hold: 0,
        }
    }

    /// Fingers of the last complete frame, for logging.
    pub fn frame(&self) -> &[Contact] {
        &self.frame[..self.frame_n]
    }

    /// The button a tap is currently holding down. 0 = none.
    ///
    /// A level, not an edge: the caller combines it with the physical
    /// buttons and reports when the state changes.
    pub fn hold(&self) -> u8 {
        self.hold
    }

    /// When `tick` must run again, while a tap holds the button. `None`: no
    /// timer pending; the driver may wait for the next report.
    pub fn next_deadline(&self) -> Option<u64> {
        match self.tap_state {
            Tap::Held { since } => Some(since + TAP_MS + 1),
            _ => None,
        }
    }

    /// Advance the timer even when no report arrived.
    ///
    /// Must run as long as the driver runs. A tap presses immediately and
    /// waits for the window to release; an untouched touchpad sends no
    /// reports, so without this the button would stay down forever.
    pub fn tick(&mut self, now_ms: u64) -> u8 {
        self.expire(now_ms);
        self.hold
    }

    /// Let the window after a tap expire.
    fn expire(&mut self, now_ms: u64) {
        if let Tap::Held { since } = self.tap_state {
            if now_ms.saturating_sub(since) > TAP_MS {
                self.hold = 0;
                self.tap_state = Tap::Idle;
            }
        }
    }

    /// Feed one report.
    ///
    /// `cc` is the reported contact count (negative: the device has none),
    /// `present` are the slots of this report with tip switch set, `slots`
    /// is the number of slots in the report (counted against those, not
    /// against touching fingers, or a frame with a lifted finger never
    /// ends), and `btn` says whether a physical button is pressed in this
    /// report.
    pub fn feed(
        &mut self, cc: i32, present: &[Contact], slots: usize, now_ms: u64, btn: bool,
    ) -> Out {
        self.expire(now_ms);
        if cc > 0 || !self.in_frame {
            self.expected = if cc > 0 { cc as usize } else { 0 };
            self.frame_n = 0;
            self.collected = 0;
            self.in_frame = true;
        }
        for c in present {
            if self.frame_n < self.frame.len() {
                self.frame[self.frame_n] = *c;
                self.frame_n += 1;
            }
        }
        self.collected += slots;
        if self.collected < self.expected {
            return Out::Pending;
        }
        self.in_frame = false;
        self.decide(now_ms, btn)
    }

    fn decide(&mut self, now_ms: u64, btn: bool) -> Out {
        // Button first. It decides two things: that the fingers are pinned,
        // and that this touch can no longer become a tap.
        if btn {
            self.btn_seen = true;
            if !self.btn_was {
                self.pinned = true;
                self.pin_dx = 0;
                self.pin_dy = 0;
            }
        } else {
            self.pinned = false;
        }
        self.btn_was = btn;

        let n = self.frame_n;
        if n == 0 {
            // All fingers lifted: only now does the gesture end, and only
            // here is it known whether it was a tap.
            let quick = !self.moved
                && !self.btn_seen
                && now_ms.saturating_sub(self.down_ms) <= TAP_MS;
            let fingers = self.gesture_n;
            let mut tap = 0u8;
            match self.tap_state {
                Tap::DragOrTap if quick && fingers == 1 => {
                    // Double tap: the held button is released and the
                    // second click is emitted whole — two clicks, a double
                    // click.
                    self.hold = 0;
                    tap = 1;
                    self.tap_state = Tap::Idle;
                }
                Tap::DragOrTap | Tap::Dragging => {
                    // Dragged and released.
                    self.hold = 0;
                    self.tap_state = Tap::Idle;
                }
                _ if quick && fingers == 1 => {
                    // A tap: press. The release waits for the window, or it
                    // could never become a drag.
                    self.hold = 1;
                    self.tap_state = Tap::Held { since: now_ms };
                }
                _ if quick && fingers == 2 => {
                    // Two fingers: a right click never drags, so press and
                    // release at once.
                    tap = 2;
                    self.tap_state = Tap::Idle;
                }
                _ => self.tap_state = Tap::Idle,
            }
            self.have_ref = false;
            self.gesture_n = 0;
            self.scroll_acc = 0;
            self.hscroll_acc = 0;
            self.moved = false;
            self.btn_seen = false;
            self.pinned = false;
            return Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap };
        }
        if self.gesture_n == 0 {
            // Touch-down: a possible tap starts here.
            self.down_ms = now_ms;
            self.tap_dx = 0;
            self.tap_dy = 0;
            self.moved = false;
            self.btn_seen = btn;
            if let Tap::Held { .. } = self.tap_state {
                // Landed again within the window. The button stays down;
                // the next frames decide what it becomes.
                self.tap_state = Tap::DragOrTap;
            }
        }
        if n > self.gesture_n {
            self.gesture_n = n;
        }

        // Compute motion from the same finger: the one with the lowest ID
        // in the frame. Otherwise the distance jumps by the finger spacing
        // when the device reorders them.
        let mut pick = 0usize;
        for i in 1..n {
            if self.frame[i].0 < self.frame[pick].0 {
                pick = i;
            }
        }
        let (cid, x, y) = self.frame[pick];
        let (mut ddx, mut ddy) = if self.have_ref && cid == self.track_id {
            (x - self.rx, y - self.ry)
        } else {
            // First touch or a different finger: no motion, or the pointer
            // would jump to where the finger landed.
            (0, 0)
        };
        self.have_ref = true;
        self.track_id = cid;
        self.rx = x;
        self.ry = y;

        // Measure first, then pin: whether a finger moved is decided by the
        // real distance. Asking after pinning always yields zero and would
        // report a tap after every button press.
        self.tap_dx += ddx;
        self.tap_dy += ddy;
        if self.tap_dx.abs() > self.tap_move || self.tap_dy.abs() > self.tap_move {
            self.moved = true;
        }

        if self.pinned {
            self.pin_dx += ddx;
            self.pin_dy += ddy;
            if self.pin_dx.abs() > self.pin_move || self.pin_dy.abs() > self.pin_move {
                // Far enough; the pointer follows again. The accumulated
                // distance is dropped: replaying it would jump by exactly
                // the threshold.
                self.pinned = false;
            }
            ddx = 0;
            ddy = 0;
        }

        // A finger that rests or moves is dragging.
        if self.tap_state == Tap::DragOrTap
            && (self.moved || now_ms.saturating_sub(self.down_ms) > TAP_MS)
        {
            self.tap_state = Tap::Dragging;
        }

        if self.gesture_n >= 2 {
            self.scroll_acc += ddy;
            let clicks = self.scroll_acc / self.step;
            if clicks != 0 {
                self.scroll_acc -= clicks * self.step;
            }
            self.hscroll_acc += ddx;
            let hclicks = self.hscroll_acc / self.hstep;
            if hclicks != 0 {
                self.hscroll_acc -= hclicks * self.hstep;
            }
            // Y grows downward, a wheel counts upward, so the vertical axis
            // flips its sign. Not horizontally: X grows right and
            // REL_HWHEEL also counts right.
            Out::Frame {
                n, gesture: self.gesture_n, dx: 0, dy: 0,
                scroll: -clicks, hscroll: hclicks, tap: 0,
            }
        } else {
            Out::Frame {
                n, gesture: self.gesture_n, dx: ddx, dy: ddy,
                scroll: 0, hscroll: 0, tap: 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tracker with the dimensions most tests use: detent 50, tap travel
    /// 40, pin travel 80.
    fn tr() -> Tracker { Tracker::new(50, 50, 40, 80) }

    /// A device with one slot: two fingers are two reports, and the contact
    /// count is only in the first.
    fn two_finger(t: &mut Tracker, a: Contact, b: Contact) -> Out {
        two_finger_at(t, a, b, 0)
    }

    fn two_finger_at(t: &mut Tracker, a: Contact, b: Contact, at: u64) -> Out {
        assert_eq!(t.feed(2, &[a], 1, at, false), Out::Pending,
            "nach dem ersten Bericht fehlt ein Finger");
        t.feed(0, &[b], 1, at, false)
    }

    #[test]
    fn a_frame_spanning_two_reports_is_one_frame() {
        let mut t = tr();
        let out = two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        match out {
            Out::Frame { n, gesture, .. } => {
                assert_eq!(n, 2);
                assert_eq!(gesture, 2);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn two_fingers_moving_down_scroll_up_is_negative() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Both fingers 50 units down: exactly one detent.
        let out = two_finger(&mut t, (0, 100, 250), (1, 400, 260));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// If a frame falls back to one finger once, it must not become
    /// pointing, and above all the reference point must not be lost, or
    /// the distance is zero in every frame and it never scrolls.
    #[test]
    fn a_frame_that_drops_to_one_finger_keeps_scrolling() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // In between, a frame sees only one finger.
        let out = t.feed(1, &[(0, 100, 250)], 1, 0, false);
        assert_eq!(out, Out::Frame { n: 1, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 },
            "die Geste haelt, und die Strecke kommt aus demselben Finger");
        // And then two again.
        let out = two_finger(&mut t, (0, 100, 300), (1, 400, 310));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// A lost follow-up report must not make the driver wait forever. The
    /// next report with a contact count opens a new frame.
    #[test]
    fn a_lost_continuation_report_does_not_stall_forever() {
        let mut t = tr();
        assert_eq!(t.feed(2, &[(0, 100, 200)], 1, 0, false), Out::Pending);
        // The second report is lost; the next frame starts.
        assert_eq!(t.feed(2, &[(0, 100, 210)], 1, 0, false), Out::Pending);
        let out = t.feed(0, &[(1, 400, 220)], 1, 0, false);
        assert!(matches!(out, Out::Frame { n: 2, .. }), "{out:?}");
    }

    #[test]
    fn one_finger_moves_the_pointer_and_never_scrolls() {
        let mut t = tr();
        // Touch-down: no jump.
        assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 0, false),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 130, 400)], 1, 0, false),
            Out::Frame { n: 1, gesture: 1, dx: 30, dy: 200, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Lift-off ends the gesture; otherwise the next one-finger touch would
    /// keep scrolling.
    #[test]
    fn lifting_ends_the_gesture() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Lifted late enough that it is not a tap.
        assert_eq!(t.feed(0, &[], 1, 500, false),
            Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 500, false),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 100, 260)], 1, 520, false),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 60, scroll: 0, hscroll: 0, tap: 0 },
            "ein Finger zeigt, auch nach einer Rollgeste");
    }

    /// One-finger tap: the button goes down immediately and waits for the
    /// window to release, or it could never become a drag.
    #[test]
    fn a_short_still_touch_presses_and_holds() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 103, 198)], 1, 40, false);
        let out = t.feed(0, &[], 1, 90, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 },
            "der Klick steht im Pegel, nicht im Impuls");
        assert_eq!(t.hold(), 1, "die Taste ist unten");
    }

    /// ... and goes up once the window has passed. Without `tick` it would
    /// stay down forever, since an untouched touchpad sends no reports.
    #[test]
    fn a_held_tap_releases_when_the_window_expires() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 90, false);
        assert_eq!(t.hold(), 1);
        assert_eq!(t.tick(200), 1, "im Fenster bleibt sie unten");
        assert_eq!(t.tick(300), 0, "danach geht sie auf");
    }

    /// Tap and drag: tap, land again immediately, move — the button stays
    /// down until lift-off. The soft click used to move a window without
    /// pressing.
    #[test]
    fn tap_then_touch_again_drags() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 80, false);
        assert_eq!(t.hold(), 1);
        // Landed again within the window and moved.
        t.feed(1, &[(0, 100, 200)], 1, 120, false);
        let out = t.feed(1, &[(0, 100, 400)], 1, 160, false);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 200, scroll: 0, hscroll: 0, tap: 0 },
            "der Finger zieht und der Zeiger folgt");
        assert_eq!(t.hold(), 1, "die Taste bleibt unten, solange gezogen wird");
        // The timer must not end the drag.
        assert_eq!(t.tick(900), 1, "ein Ziehen laeuft nicht ab");
        t.feed(0, &[], 1, 950, false);
        assert_eq!(t.hold(), 0, "abgehoben heisst losgelassen");
    }

    /// The opposite case: two quick taps are two clicks, not a drag. The
    /// held button is released and the second click is emitted whole.
    #[test]
    fn tap_then_quick_tap_is_two_clicks() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 80, false);
        assert_eq!(t.hold(), 1);
        t.feed(1, &[(0, 100, 202)], 1, 120, false);
        let out = t.feed(0, &[], 1, 180, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 1 },
            "der zweite Klick kommt als Impuls");
        assert_eq!(t.hold(), 0, "und der erste ist losgelassen");
    }

    /// Landing again within the window and resting also drags, without any
    /// motion at all.
    #[test]
    fn touching_again_and_resting_also_drags() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 80, false);
        t.feed(1, &[(0, 100, 200)], 1, 120, false);
        // Rest until the window of the second contact has passed.
        t.feed(1, &[(0, 100, 200)], 1, 400, false);
        assert_eq!(t.hold(), 1);
        assert_eq!(t.tick(900), 1, "ein Ziehen laeuft nicht ab");
        t.feed(0, &[], 1, 950, false);
        assert_eq!(t.hold(), 0);
    }

    #[test]
    fn two_fingers_tapped_are_a_right_click() {
        let mut t = tr();
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 210), 0);
        let out = t.feed(0, &[], 1, 80, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 2 });
        assert_eq!(t.hold(), 0, "ein Rechtsklick zieht nichts und haelt nichts");
    }

    /// If the device reorders fingers between frames, a remembered start
    /// point would jump by the finger spacing and a two-finger tap would
    /// turn into motion, losing the right click.
    #[test]
    fn a_two_finger_tap_survives_swapped_contact_order() {
        let mut t = tr();
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 900), 0);
        // Same frame, different order, no finger moved.
        two_finger_at(&mut t, (1, 400, 900), (0, 100, 200), 40);
        let out = t.feed(0, &[], 1, 80, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 2 },
            "der Fingerwechsel ist keine Bewegung");
    }

    /// Moving the finger means pointing, not clicking; otherwise every
    /// short swipe would click.
    #[test]
    fn a_touch_that_moved_is_not_a_tap() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 100, 300)], 1, 40, false);
        let out = t.feed(0, &[], 1, 90, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.hold(), 0);
    }

    /// Resting is holding, and a hold is not a tap.
    #[test]
    fn a_long_still_touch_is_not_a_tap() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        let out = t.feed(0, &[], 1, 400, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.hold(), 0);
    }

    /// A scroll gesture never ends as a click.
    #[test]
    fn a_scroll_never_ends_as_a_tap() {
        let mut t = tr();
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 210), 0);
        two_finger_at(&mut t, (0, 100, 260), (1, 400, 270), 30);
        let out = t.feed(0, &[], 1, 60, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Pinning. Pressing the pad deforms the fingertip and its centroid
    /// wanders; without this gate that motion would reach the pointer right
    /// when the user aims at something small.
    #[test]
    fn a_pressed_button_pins_the_finger() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        // Button down, and the finger slides 30 units.
        let out = t.feed(1, &[(0, 130, 220)], 1, 20, true);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 },
            "unter der Taste steht der Zeiger still");
    }

    /// But dragging with the button pressed must work: once the finger
    /// moves far enough, the pointer follows again.
    #[test]
    fn a_pinned_finger_is_released_when_it_really_moves() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 100, 200)], 1, 20, true);
        // 90 > pin_move (80): the gate opens, but this frame stays still;
        // replaying the accumulated distance would be a jump.
        let out = t.feed(1, &[(0, 100, 290)], 1, 40, true);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        let out = t.feed(1, &[(0, 100, 340)], 1, 60, true);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 50, scroll: 0, hscroll: 0, tap: 0 },
            "ab jetzt folgt der Zeiger wieder");
    }

    /// Pinning ends with the button, not with the finger.
    #[test]
    fn releasing_the_button_unpins() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 100, 200)], 1, 20, true);
        t.feed(1, &[(0, 100, 210)], 1, 40, true);
        let out = t.feed(1, &[(0, 100, 240)], 1, 60, false);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 30, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Under the button the pointer measures zero motion; asking for a tap
    /// after pinning would treat every physical click as a tap and emit it
    /// twice.
    #[test]
    fn a_physical_click_is_never_also_a_tap() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, true);
        t.feed(1, &[(0, 105, 205)], 1, 30, true);
        let out = t.feed(0, &[], 1, 60, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.hold(), 0, "der Druck war der Klick — ein zweiter waere erfunden");
    }

    /// Two fingers moving right scroll right, and the vertical axis stays
    /// still.
    #[test]
    fn two_fingers_moving_right_scroll_right() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        let out = two_finger(&mut t, (0, 150, 200), (1, 450, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 1, tap: 0 });
    }

    /// A diagonal swipe carries both axes. An axis lock would halve it, and
    /// libinput does not lock during two-finger scrolling.
    #[test]
    fn a_diagonal_swipe_carries_both_axes() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        let out = two_finger(&mut t, (0, 150, 250), (1, 450, 260));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 1, tap: 0 });
    }

    /// The horizontal axis has its own step: a pad is wider than tall, and
    /// a step derived from the height would be too fine horizontally.
    #[test]
    fn the_horizontal_step_is_its_own() {
        let mut t = Tracker::new(50, 100, 40, 80);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // 50 across is below the horizontal step: no detent yet.
        let out = two_finger(&mut t, (0, 150, 200), (1, 450, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        let out = two_finger(&mut t, (0, 200, 200), (1, 500, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 1, tap: 0 });
    }

    /// If the device reorders fingers, the distance must not jump by the
    /// finger spacing.
    #[test]
    fn swapped_contact_order_does_not_jump() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 900));
        // Same frame, different order, both 50 down.
        let out = two_finger(&mut t, (1, 400, 950), (0, 100, 250));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// Partial distances accumulate until they make a detent; otherwise
    /// every frame would emit one and scrolling would be unusably fast.
    #[test]
    fn short_moves_accumulate_into_one_click() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        for i in 1..5 {
            let y = 200 + i * 10;
            let out = two_finger(&mut t, (0, 100, y), (1, 400, y + 10));
            assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 },
                "nach {} Einheiten noch keine Raste", i * 10);
        }
        let out = two_finger(&mut t, (0, 100, 250), (1, 400, 260));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// A device with two slots per report sends both fingers at once; there
    /// are no follow-up reports.
    #[test]
    fn a_device_with_two_slots_needs_no_continuation() {
        let mut t = tr();
        assert!(matches!(
            t.feed(2, &[(0, 100, 200), (1, 400, 210)], 2, 0, false),
            Out::Frame { n: 2, gesture: 2, .. }));
    }
}
