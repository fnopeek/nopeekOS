//! Aus KONTAKTPUNKTEN wird ein Zeiger und eine Geste.
//!
//! Ein Touchpad meldet Orte, keine Wege, und es meldet keine Gesten: „zwei
//! Finger wandern nach unten" steht in keinem Bericht. Unter Linux macht
//! das libinput, hier dieses Modul — und es steht im Kern und nicht im
//! Wasm-Modul, weil genau diese Logik zweimal falsch ausgeliefert wurde
//! und beide Male erst am Geraet auffiel.
//!
//! Zwei Dinge, die nicht offensichtlich sind und die beide Male der Fehler
//! waren:
//!
//! 1. **Ein Bild kann ueber mehrere Berichte kommen.** Ein Geraet mit einem
//!    Kontaktplatz schickt je Finger einen Bericht; `Contact Count` steht
//!    nur im ersten. Die Regel ist die von `hid-multitouch.c`: der Bericht,
//!    der eine Kontaktzahl TRAEGT, eroeffnet das Bild.
//! 2. **Die Geste haelt, bis abgehoben wird.** Haengt sie an der Fingerzahl
//!    DIESES Bildes, zappelt sie — faellt ein Folgebericht aus, sieht ein
//!    Bild einen Finger statt zwei. Wer bei jedem Wechsel den Bezugspunkt
//!    wegwirft, rechnet immer die Strecke null.

/// Ein aufliegender Finger: Kennung, Ort.
pub type Contact = (i32, i32, i32);

/// Was ein Bericht ergeben hat.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    /// Das Bild ist noch nicht vollstaendig — es fehlen Finger.
    Pending,
    /// Ein vollstaendiges Bild.
    Frame {
        /// Aufliegende Finger in diesem Bild.
        n: usize,
        /// Die Geste, die seit dem Aufsetzen gilt.
        gesture: usize,
        dx: i32,
        dy: i32,
        /// Rollrasten, Vorzeichen wie ein Mausrad (positiv = nach oben).
        scroll: i32,
        /// Waagrechte Rollrasten, positiv = nach RECHTS (wie REL_HWHEEL).
        ///
        /// Eigene Achse mit eigenem Speicher und eigenem Schritt: das Pad
        /// ist breiter als hoch, ein Schritt aus der Hoehe waere quer zu
        /// fein. Beide Achsen laufen unabhaengig, wie bei libinput —
        /// eine Achssperre wuerde einen schraegen Wisch halbieren.
        hscroll: i32,
        /// Ein ANTIPPEN ist gerade zu Ende gegangen: mit wievielen
        /// Fingern? 0 = keines, 1 = links, 2 = rechts.
        ///
        /// In der Maus-Nachahmung macht die Firmware daraus selbst einen
        /// Klick. Im Praezisionsmodus tut sie es NICHT — dort ist ein
        /// Tippen nur ein Kontakt, der kommt und geht, und wer den Klick
        /// nicht selbst bildet, hat ihn verloren.
        tap: u8,
    },
}

/// Ein Antippen dauert hoechstens so lange. libinput nimmt denselben
/// Wert; darueber ist es ein Halten und kein Tippen.
const TAP_MS: u64 = 180;

pub struct Tracker {
    /// Geraeteeinheiten je Rollraste, aus dem logischen Bereich des
    /// Geraets hergeleitet.
    step: i32,
    /// Dasselbe fuer die waagrechte Achse — aus der BREITE.
    hstep: i32,
    /// Soweit darf ein Finger wandern und es bleibt ein Tippen.
    tap_move: i32,
    /// Wann und wo hat die laufende Beruehrung angefangen?
    down_ms: u64,
    down_x: i32,
    down_y: i32,
    moved: bool,
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
}

impl Tracker {
    pub fn new(step: i32, hstep: i32, tap_move: i32) -> Self {
        Tracker {
            step: step.max(1),
            hstep: hstep.max(1),
            tap_move: tap_move.max(1),
            down_ms: 0,
            down_x: 0,
            down_y: 0,
            moved: false,
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
        }
    }

    /// Die Finger des zuletzt vollstaendigen Bildes — fuers Log.
    pub fn frame(&self) -> &[Contact] {
        &self.frame[..self.frame_n]
    }

    /// Einen Bericht einspeisen.
    ///
    /// `cc` ist die gemeldete Kontaktzahl (negativ: das Geraet fuehrt gar
    /// keine), `present` sind die Plaetze dieses Berichts, deren Tip-Switch
    /// gesetzt ist, und `slots` ist die Zahl der Plaetze im Bericht —
    /// gezaehlt wird gegen sie, nicht gegen die aufliegenden, sonst
    /// endet ein Bild mit einem abgehobenen Finger nie.
    pub fn feed(&mut self, cc: i32, present: &[Contact], slots: usize, now_ms: u64) -> Out {
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
        self.decide(now_ms)
    }

    fn decide(&mut self, now_ms: u64) -> Out {
        let n = self.frame_n;
        if n == 0 {
            // Alle Finger weg: erst JETZT endet die Geste — und erst hier
            // steht fest, ob sie ein Antippen war.
            let tap = if self.gesture_n >= 1
                && self.gesture_n <= 2
                && !self.moved
                && now_ms.saturating_sub(self.down_ms) <= TAP_MS
            {
                self.gesture_n as u8
            } else {
                0
            };
            self.have_ref = false;
            self.gesture_n = 0;
            self.scroll_acc = 0;
            self.hscroll_acc = 0;
            self.moved = false;
            return Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap };
        }
        if self.gesture_n == 0 {
            // Aufsetzen: hier faengt ein moegliches Antippen an.
            self.down_ms = now_ms;
            self.down_x = self.frame[0].1;
            self.down_y = self.frame[0].2;
            self.moved = false;
        }
        if n > self.gesture_n {
            self.gesture_n = n;
        }
        if (self.frame[0].1 - self.down_x).abs() > self.tap_move
            || (self.frame[0].2 - self.down_y).abs() > self.tap_move
        {
            self.moved = true;
        }

        // Den Weg aus DEMSELBEN Finger rechnen: dem mit der kleinsten
        // Kennung im Bild. Ohne das springt die Strecke um den
        // Fingerabstand, sobald das Geraet die Reihenfolge tauscht.
        let mut pick = 0usize;
        for i in 1..n {
            if self.frame[i].0 < self.frame[pick].0 {
                pick = i;
            }
        }
        let (cid, x, y) = self.frame[pick];
        let (ddx, ddy) = if self.have_ref && cid == self.track_id {
            (x - self.rx, y - self.ry)
        } else {
            // Erste Beruehrung oder anderer Finger: kein Weg, sonst
            // spraenge der Zeiger dorthin, wo aufgesetzt wurde.
            (0, 0)
        };
        self.have_ref = true;
        self.track_id = cid;
        self.rx = x;
        self.ry = y;

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
            // Y waechst nach UNTEN, ein Rad zaehlt nach OBEN — deshalb
            // dreht die senkrechte Achse ihr Vorzeichen. Quer nicht: X
            // waechst nach rechts, und REL_HWHEEL zaehlt auch nach rechts.
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

    /// Ein Geraet mit EINEM Platz: zwei Finger sind zwei Berichte, und die
    /// Kontaktzahl steht nur im ersten.
    fn two_finger(t: &mut Tracker, a: Contact, b: Contact) -> Out {
        two_finger_at(t, a, b, 0)
    }

    fn two_finger_at(t: &mut Tracker, a: Contact, b: Contact, at: u64) -> Out {
        assert_eq!(t.feed(2, &[a], 1, at), Out::Pending, "nach dem ersten Bericht fehlt ein Finger");
        t.feed(0, &[b], 1, at)
    }

    #[test]
    fn a_frame_spanning_two_reports_is_one_frame() {
        let mut t = Tracker::new(50, 50, 40);
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
        let mut t = Tracker::new(50, 50, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Beide Finger 50 Einheiten nach unten: genau eine Raste.
        let out = two_finger(&mut t, (0, 100, 250), (1, 400, 260));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// Der Fehler, an dem 0.17.0 gescheitert ist: faellt ein Bild einmal
    /// auf EINEN Finger zurueck, darf daraus kein Zeigen werden — und vor
    /// allem darf der Bezugspunkt nicht verloren gehen, sonst ist die
    /// Strecke in JEDEM Bild null und es rollt nie.
    #[test]
    fn a_frame_that_drops_to_one_finger_keeps_scrolling() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Zwischendurch sieht ein Bild nur einen Finger.
        let out = t.feed(1, &[(0, 100, 250)], 1, 0);
        assert_eq!(out, Out::Frame { n: 1, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 },
            "die Geste haelt, und die Strecke kommt aus demselben Finger");
        // Und danach wieder zwei.
        let out = two_finger(&mut t, (0, 100, 300), (1, 400, 310));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// Ein verlorener Folgebericht darf den Treiber nicht fuer immer
    /// warten lassen. Der naechste Bericht MIT Kontaktzahl eroeffnet.
    #[test]
    fn a_lost_continuation_report_does_not_stall_forever() {
        let mut t = Tracker::new(50, 50, 40);
        assert_eq!(t.feed(2, &[(0, 100, 200)], 1, 0), Out::Pending);
        // Der zweite Bericht geht verloren; das naechste Bild faengt an.
        assert_eq!(t.feed(2, &[(0, 100, 210)], 1, 0), Out::Pending);
        let out = t.feed(0, &[(1, 400, 220)], 1, 0);
        assert!(matches!(out, Out::Frame { n: 2, .. }), "{out:?}");
    }

    #[test]
    fn one_finger_moves_the_pointer_and_never_scrolls() {
        let mut t = Tracker::new(50, 50, 40);
        // Aufsetzen: KEIN Sprung.
        assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 0),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 130, 400)], 1, 0),
            Out::Frame { n: 1, gesture: 1, dx: 30, dy: 200, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Abheben beendet die Geste — sonst rollte die naechste Beruehrung
    /// mit EINEM Finger weiter.
    #[test]
    fn lifting_ends_the_gesture() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Spaet genug abgehoben, dass es kein Antippen ist.
        assert_eq!(t.feed(0, &[], 1, 500),
            Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 500),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 100, 260)], 1, 520),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 60, scroll: 0, hscroll: 0, tap: 0 },
            "ein Finger zeigt, auch nach einer Rollgeste");
    }

    /// ANTIPPEN. Im Praezisionsmodus bildet die Firmware den Klick nicht
    /// mehr selbst — wer ihn nicht hier baut, hat ihn verloren.
    #[test]
    fn a_short_still_touch_is_a_tap() {
        let mut t = Tracker::new(50, 50, 40);
        t.feed(1, &[(0, 100, 200)], 1, 0);
        t.feed(1, &[(0, 103, 198)], 1, 40);
        let out = t.feed(0, &[], 1, 90);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 1 });
    }

    #[test]
    fn two_fingers_tapped_are_a_right_click() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 210), 0);
        let out = t.feed(0, &[], 1, 80);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 2 });
    }

    /// Wer den Finger bewegt hat, wollte zeigen und nicht klicken — sonst
    /// klickt jedes kurze Wischen.
    #[test]
    fn a_touch_that_moved_is_not_a_tap() {
        let mut t = Tracker::new(50, 50, 40);
        t.feed(1, &[(0, 100, 200)], 1, 0);
        t.feed(1, &[(0, 100, 300)], 1, 40);
        let out = t.feed(0, &[], 1, 90);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Wer liegen bleibt, haelt — und ein Halten ist kein Tippen.
    #[test]
    fn a_long_still_touch_is_not_a_tap() {
        let mut t = Tracker::new(50, 50, 40);
        t.feed(1, &[(0, 100, 200)], 1, 0);
        let out = t.feed(0, &[], 1, 400);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Eine Rollgeste endet nie als Klick.
    #[test]
    fn a_scroll_never_ends_as_a_tap() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 210), 0);
        two_finger_at(&mut t, (0, 100, 260), (1, 400, 270), 30);
        let out = t.feed(0, &[], 1, 60);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Zwei Finger nach RECHTS rollen nach rechts — und die senkrechte
    /// Achse bleibt dabei still.
    #[test]
    fn two_fingers_moving_right_scroll_right() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        let out = two_finger(&mut t, (0, 150, 200), (1, 450, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 1, tap: 0 });
    }

    /// Ein schraeger Wisch traegt BEIDE Achsen. Eine Achssperre wuerde
    /// ihn halbieren, und libinput sperrt beim Zweifinger-Rollen nicht.
    #[test]
    fn a_diagonal_swipe_carries_both_axes() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        let out = two_finger(&mut t, (0, 150, 250), (1, 450, 260));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 1, tap: 0 });
    }

    /// Die quere Achse hat ihren EIGENEN Schritt: ein Pad ist breiter als
    /// hoch, und ein Schritt aus der Hoehe waere quer zu fein.
    #[test]
    fn the_horizontal_step_is_its_own() {
        let mut t = Tracker::new(50, 100, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // 50 quer ist unter dem queren Schritt — noch keine Raste.
        let out = two_finger(&mut t, (0, 150, 200), (1, 450, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        let out = two_finger(&mut t, (0, 200, 200), (1, 500, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 1, tap: 0 });
    }

    /// Tauscht das Geraet die Reihenfolge der Finger, darf die Strecke
    /// nicht um den Fingerabstand springen.
    #[test]
    fn swapped_contact_order_does_not_jump() {
        let mut t = Tracker::new(50, 50, 40);
        two_finger(&mut t, (0, 100, 200), (1, 400, 900));
        // Dasselbe Bild, andere Reihenfolge, beide 50 nach unten.
        let out = two_finger(&mut t, (1, 400, 950), (0, 100, 250));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// Teilstrecken laufen auf, bis sie eine Raste ergeben — sonst kaeme
    /// bei jedem Bild eine und das Rollen waere unbrauchbar schnell.
    #[test]
    fn short_moves_accumulate_into_one_click() {
        let mut t = Tracker::new(50, 50, 40);
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

    /// Ein Geraet mit ZWEI Plaetzen im Bericht meldet beide Finger auf
    /// einmal — dann gibt es keine Folgeberichte.
    #[test]
    fn a_device_with_two_slots_needs_no_continuation() {
        let mut t = Tracker::new(50, 50, 40);
        assert!(matches!(
            t.feed(2, &[(0, 100, 200), (1, 400, 210)], 2, 0),
            Out::Frame { n: 2, gesture: 2, .. }));
    }
}
