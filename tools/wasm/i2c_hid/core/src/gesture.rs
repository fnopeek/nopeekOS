//! Aus KONTAKTPUNKTEN wird ein Zeiger und eine Geste.
//!
//! Ein Touchpad meldet Orte, keine Wege, und es meldet keine Gesten: „zwei
//! Finger wandern nach unten" steht in keinem Bericht. Unter Linux macht
//! das libinput, hier dieses Modul — und es steht im Kern und nicht im
//! Wasm-Modul, weil genau diese Logik zweimal falsch ausgeliefert wurde
//! und beide Male erst am Geraet auffiel.
//!
//! Vier Dinge, die nicht offensichtlich sind:
//!
//! 1. **Ein Bild kann ueber mehrere Berichte kommen.** Ein Geraet mit einem
//!    Kontaktplatz schickt je Finger einen Bericht; `Contact Count` steht
//!    nur im ersten. Die Regel ist die von `hid-multitouch.c`: der Bericht,
//!    der eine Kontaktzahl TRAEGT, eroeffnet das Bild.
//! 2. **Die Geste haelt, bis abgehoben wird.** Haengt sie an der Fingerzahl
//!    DIESES Bildes, zappelt sie — faellt ein Folgebericht aus, sieht ein
//!    Bild einen Finger statt zwei. Wer bei jedem Wechsel den Bezugspunkt
//!    wegwirft, rechnet immer die Strecke null.
//! 3. **Wer die Taste durchdrueckt, bewegt den Finger.** Eine Fingerkuppe
//!    verformt sich unter dem Druck, ihr Schwerpunkt wandert — und ohne
//!    Gegenmittel geht diese Wanderung ungefiltert an den Zeiger, genau
//!    dann, wenn man etwas Kleines treffen will. Deshalb werden die Finger
//!    beim Tastendruck FESTGEHALTEN (libinput: `tp_pin_fingers`) und erst
//!    gelöst, wenn sie wirklich weit wandern.
//! 4. **Ein Antippen drueckt sofort und laesst SPAETER los.** Nur so kann
//!    daraus ein Ziehen werden: setzt der Finger im Fenster wieder auf, ist
//!    die Taste noch unten. Der Preis ist ein Zeitgeber, den der Rufer
//!    treten muss (`tick`) — ohne ihn bliebe die Taste gedrueckt, und
//!    genau das war der Fehler von 0.13.0.

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
        /// Ein Antippen, das fertig ist: Druck UND Loslassen in einem.
        /// 0 = keines, 2 = rechts.
        ///
        /// Nur fuer das, was nicht ziehen kann. Ein Antippen mit EINEM
        /// Finger laeuft ueber [`Tracker::hold`], weil daraus noch ein
        /// Ziehen werden kann; ein Rechtsklick zieht nichts.
        tap: u8,
    },
}

/// Ein Antippen dauert hoechstens so lange. libinput nimmt denselben
/// Wert; darueber ist es ein Halten und kein Tippen.
///
/// Dieselbe Spanne gilt danach noch einmal als FENSTER: wer darin wieder
/// aufsetzt, zieht (oder tippt ein zweites Mal), statt neu anzufangen.
const TAP_MS: u64 = 180;

/// Wo die Tipp-Maschine gerade steht.
///
/// Die Zustaende von libinput (`evdev-mt-touchpad-tap.c`) auf das
/// reduziert, was ein Finger und zwei Finger brauchen. Der mittlere ist
/// der, um den es geht: nach einem Antippen ist noch NICHT entschieden,
/// ob daraus ein Ziehen oder ein zweites Antippen wird — das sagt erst,
/// was der Finger danach tut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tap {
    /// Nichts laeuft.
    Idle,
    /// Ein Antippen ist zu Ende, die Taste ist GEDRUECKT, das Fenster
    /// laeuft ab `since`.
    Held { since: u64 },
    /// Im Fenster hat ein Finger wieder aufgesetzt. Offen, was es wird.
    DragOrTap,
    /// Es ist ein Ziehen: die Taste bleibt, bis abgehoben wird.
    Dragging,
}

pub struct Tracker {
    /// Geraeteeinheiten je Rollraste, aus dem logischen Bereich des
    /// Geraets hergeleitet.
    step: i32,
    /// Dasselbe fuer die waagrechte Achse — aus der BREITE.
    hstep: i32,
    /// Soweit darf ein Finger wandern und es bleibt ein Tippen.
    tap_move: i32,
    /// Soweit darf ein Finger unter einer GEDRUECKTEN Taste wandern,
    /// bevor der Zeiger ihm wieder folgt.
    pin_move: i32,
    /// Wann hat die laufende Beruehrung angefangen?
    down_ms: u64,
    /// Weg des VERFOLGTEN Fingers seit dem Aufsetzen.
    ///
    /// Nicht der Abstand zu einem gemerkten Punkt: der Punkt gehoerte
    /// `frame[0]`, und welcher Finger das ist, entscheidet das Geraet neu
    /// in jedem Bericht. Tauschte es die Reihenfolge, sprang die Strecke
    /// um den FINGERABSTAND und jedes Antippen mit zwei Fingern fiel aus.
    /// Aufsummiert wird deshalb dieselbe Strecke, die auch der Zeiger
    /// bekommt — und die ist beim Fingerwechsel null.
    tap_dx: i32,
    tap_dy: i32,
    moved: bool,
    /// Lag waehrend DIESER Beruehrung eine physische Taste an? Dann war
    /// sie kein Antippen — sonst gaebe jeder Druck zwei Klicks.
    btn_seen: bool,
    btn_was: bool,
    /// Die Finger sind festgehalten: ihr Weg geht nicht an den Zeiger.
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

    /// Die Finger des zuletzt vollstaendigen Bildes — fuers Log.
    pub fn frame(&self) -> &[Contact] {
        &self.frame[..self.frame_n]
    }

    /// Die Taste, die ein Antippen gerade GEDRUECKT haelt. 0 = keine.
    ///
    /// Ein PEGEL, keine Flanke: der Rufer legt ihn neben die physischen
    /// Tasten und meldet die Lage, wenn sie sich aendert.
    pub fn hold(&self) -> u8 {
        self.hold
    }

    /// Den Zeitgeber weiterdrehen, auch wenn kein Bericht kam.
    ///
    /// **Muss laufen, solange der Treiber laeuft.** Ein Antippen drueckt
    /// sofort und wartet mit dem Loslassen auf das Fenster; kommt danach
    /// nie wieder ein Bericht — und ein Touchpad, das niemand beruehrt,
    /// schickt keinen —, bliebe die Taste ohne diesen Tritt fuer immer
    /// unten.
    pub fn tick(&mut self, now_ms: u64) -> u8 {
        self.expire(now_ms);
        self.hold
    }

    /// Das Fenster nach einem Antippen ablaufen lassen.
    fn expire(&mut self, now_ms: u64) {
        if let Tap::Held { since } = self.tap_state {
            if now_ms.saturating_sub(since) > TAP_MS {
                self.hold = 0;
                self.tap_state = Tap::Idle;
            }
        }
    }

    /// Einen Bericht einspeisen.
    ///
    /// `cc` ist die gemeldete Kontaktzahl (negativ: das Geraet fuehrt gar
    /// keine), `present` sind die Plaetze dieses Berichts, deren Tip-Switch
    /// gesetzt ist, `slots` ist die Zahl der Plaetze im Bericht —
    /// gezaehlt wird gegen sie, nicht gegen die aufliegenden, sonst
    /// endet ein Bild mit einem abgehobenen Finger nie — und `btn` sagt,
    /// ob eine physische Taste dieses Berichts anliegt.
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
        // Die Taste zuerst. Sie entscheidet zweierlei: dass die Finger
        // festgehalten werden, und dass aus dieser Beruehrung kein
        // Antippen mehr werden kann.
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
            // Alle Finger weg: erst JETZT endet die Geste — und erst hier
            // steht fest, ob sie ein Antippen war.
            let quick = !self.moved
                && !self.btn_seen
                && now_ms.saturating_sub(self.down_ms) <= TAP_MS;
            let fingers = self.gesture_n;
            let mut tap = 0u8;
            match self.tap_state {
                Tap::DragOrTap if quick && fingers == 1 => {
                    // Zweimal kurz getippt: die gehaltene Taste geht auf,
                    // und der zweite Klick kommt ganz. Zusammen mit dem
                    // ersten sind das zwei — ein Doppelklick.
                    self.hold = 0;
                    tap = 1;
                    self.tap_state = Tap::Idle;
                }
                Tap::DragOrTap | Tap::Dragging => {
                    // Gezogen und losgelassen.
                    self.hold = 0;
                    self.tap_state = Tap::Idle;
                }
                _ if quick && fingers == 1 => {
                    // Ein Antippen: DRUECKEN. Das Loslassen wartet auf das
                    // Fenster, sonst koennte daraus nie ein Ziehen werden.
                    self.hold = 1;
                    self.tap_state = Tap::Held { since: now_ms };
                }
                _ if quick && fingers == 2 => {
                    // Zwei Finger: ein Rechtsklick zieht nichts, also
                    // Druck und Loslassen in einem Stueck.
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
            // Aufsetzen: hier faengt ein moegliches Antippen an.
            self.down_ms = now_ms;
            self.tap_dx = 0;
            self.tap_dy = 0;
            self.moved = false;
            self.btn_seen = btn;
            if let Tap::Held { .. } = self.tap_state {
                // Im Fenster wieder aufgesetzt. Die Taste bleibt unten;
                // was es wird, sagen die naechsten Bilder.
                self.tap_state = Tap::DragOrTap;
            }
        }
        if n > self.gesture_n {
            self.gesture_n = n;
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
        let (mut ddx, mut ddy) = if self.have_ref && cid == self.track_id {
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

        // Erst messen, dann festhalten: ob ein Finger gewandert ist,
        // entscheidet die ECHTE Strecke. Wer das nach dem Festhalten
        // fragt, bekommt immer null — und meldete nach jedem Tastendruck
        // obendrein ein Antippen.
        self.tap_dx += ddx;
        self.tap_dy += ddy;
        if self.tap_dx.abs() > self.tap_move || self.tap_dy.abs() > self.tap_move {
            self.moved = true;
        }

        if self.pinned {
            self.pin_dx += ddx;
            self.pin_dy += ddy;
            if self.pin_dx.abs() > self.pin_move || self.pin_dy.abs() > self.pin_move {
                // Weit genug — der Zeiger folgt wieder. Die aufgelaufene
                // Strecke bleibt liegen: sie NACHZUREICHEN waere ein
                // Sprung um genau die Schwelle.
                self.pinned = false;
            }
            ddx = 0;
            ddy = 0;
        }

        // Ein Finger, der liegen bleibt oder wandert, zieht.
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

    /// Ein Tracker mit den Massen, mit denen fast alle Tests rechnen:
    /// Raste 50, Tippweg 40, Festhalteweg 80.
    fn tr() -> Tracker { Tracker::new(50, 50, 40, 80) }

    /// Ein Geraet mit EINEM Platz: zwei Finger sind zwei Berichte, und die
    /// Kontaktzahl steht nur im ersten.
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
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Zwischendurch sieht ein Bild nur einen Finger.
        let out = t.feed(1, &[(0, 100, 250)], 1, 0, false);
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
        let mut t = tr();
        assert_eq!(t.feed(2, &[(0, 100, 200)], 1, 0, false), Out::Pending);
        // Der zweite Bericht geht verloren; das naechste Bild faengt an.
        assert_eq!(t.feed(2, &[(0, 100, 210)], 1, 0, false), Out::Pending);
        let out = t.feed(0, &[(1, 400, 220)], 1, 0, false);
        assert!(matches!(out, Out::Frame { n: 2, .. }), "{out:?}");
    }

    #[test]
    fn one_finger_moves_the_pointer_and_never_scrolls() {
        let mut t = tr();
        // Aufsetzen: KEIN Sprung.
        assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 0, false),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 130, 400)], 1, 0, false),
            Out::Frame { n: 1, gesture: 1, dx: 30, dy: 200, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Abheben beendet die Geste — sonst rollte die naechste Beruehrung
    /// mit EINEM Finger weiter.
    #[test]
    fn lifting_ends_the_gesture() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        // Spaet genug abgehoben, dass es kein Antippen ist.
        assert_eq!(t.feed(0, &[], 1, 500, false),
            Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 500, false),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.feed(1, &[(0, 100, 260)], 1, 520, false),
            Out::Frame { n: 1, gesture: 1, dx: 0, dy: 60, scroll: 0, hscroll: 0, tap: 0 },
            "ein Finger zeigt, auch nach einer Rollgeste");
    }

    /// ANTIPPEN mit einem Finger: die Taste geht SOFORT runter und wartet
    /// mit dem Loslassen auf das Fenster — sonst koennte daraus nie ein
    /// Ziehen werden.
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

    /// … und geht wieder auf, wenn das Fenster durch ist. **Ohne `tick`
    /// bliebe sie fuer immer unten** — ein Touchpad, das niemand beruehrt,
    /// schickt keinen Bericht mehr.
    #[test]
    fn a_held_tap_releases_when_the_window_expires() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 90, false);
        assert_eq!(t.hold(), 1);
        assert_eq!(t.tick(200), 1, "im Fenster bleibt sie unten");
        assert_eq!(t.tick(300), 0, "danach geht sie auf");
    }

    /// TIPPEN UND ZIEHEN: antippen, sofort wieder auflegen, wandern — die
    /// Taste bleibt unten, bis abgehoben wird. Das ist der „sanfte Klick",
    /// mit dem man ein Fenster verschiebt, ohne durchzudruecken.
    #[test]
    fn tap_then_touch_again_drags() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 80, false);
        assert_eq!(t.hold(), 1);
        // Im Fenster wieder aufgesetzt und gewandert.
        t.feed(1, &[(0, 100, 200)], 1, 120, false);
        let out = t.feed(1, &[(0, 100, 400)], 1, 160, false);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 200, scroll: 0, hscroll: 0, tap: 0 },
            "der Finger zieht und der Zeiger folgt");
        assert_eq!(t.hold(), 1, "die Taste bleibt unten, solange gezogen wird");
        // Der Zeitgeber darf das Ziehen NICHT beenden.
        assert_eq!(t.tick(900), 1, "ein Ziehen laeuft nicht ab");
        t.feed(0, &[], 1, 950, false);
        assert_eq!(t.hold(), 0, "abgehoben heisst losgelassen");
    }

    /// Und der Gegenfall: zweimal kurz getippt sind ZWEI Klicks, kein
    /// Ziehen. Die gehaltene Taste geht auf, der zweite Klick kommt ganz.
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

    /// Wer im Fenster wieder aufsetzt und LIEGEN bleibt, zieht auch —
    /// ohne einen einzigen Punkt Bewegung.
    #[test]
    fn touching_again_and_resting_also_drags() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(0, &[], 1, 80, false);
        t.feed(1, &[(0, 100, 200)], 1, 120, false);
        // Liegen bleiben, bis das Fenster des zweiten Kontakts durch ist.
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

    /// Der Bug, den `frame[0]` versteckt hat: tauscht das Geraet zwischen
    /// den Bildern die Reihenfolge der Finger, sprang der gemerkte
    /// Startpunkt um den FINGERABSTAND — und aus dem Antippen mit zwei
    /// Fingern wurde eine Bewegung. Damit fiel der Rechtsklick aus.
    #[test]
    fn a_two_finger_tap_survives_swapped_contact_order() {
        let mut t = tr();
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 900), 0);
        // Dasselbe Bild, andere Reihenfolge, kein Finger hat sich bewegt.
        two_finger_at(&mut t, (1, 400, 900), (0, 100, 200), 40);
        let out = t.feed(0, &[], 1, 80, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 2 },
            "der Fingerwechsel ist keine Bewegung");
    }

    /// Wer den Finger bewegt hat, wollte zeigen und nicht klicken — sonst
    /// klickt jedes kurze Wischen.
    #[test]
    fn a_touch_that_moved_is_not_a_tap() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 100, 300)], 1, 40, false);
        let out = t.feed(0, &[], 1, 90, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.hold(), 0);
    }

    /// Wer liegen bleibt, haelt — und ein Halten ist kein Tippen.
    #[test]
    fn a_long_still_touch_is_not_a_tap() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        let out = t.feed(0, &[], 1, 400, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.hold(), 0);
    }

    /// Eine Rollgeste endet nie als Klick.
    #[test]
    fn a_scroll_never_ends_as_a_tap() {
        let mut t = tr();
        two_finger_at(&mut t, (0, 100, 200), (1, 400, 210), 0);
        two_finger_at(&mut t, (0, 100, 260), (1, 400, 270), 30);
        let out = t.feed(0, &[], 1, 60, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// FESTHALTEN. Wer das Pad durchdrueckt, verformt die Fingerkuppe —
    /// ihr Schwerpunkt wandert, und ohne dieses Tor ginge die Wanderung
    /// ungefiltert an den Zeiger. Genau dann, wenn man etwas Kleines
    /// treffen will.
    #[test]
    fn a_pressed_button_pins_the_finger() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        // Taste runter, und der Finger rutscht dabei 30 Einheiten.
        let out = t.feed(1, &[(0, 130, 220)], 1, 20, true);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 },
            "unter der Taste steht der Zeiger still");
    }

    /// Aber ein ZIEHEN mit gedrueckter Taste muss gehen: wandert der
    /// Finger weit genug, folgt der Zeiger wieder.
    #[test]
    fn a_pinned_finger_is_released_when_it_really_moves() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 100, 200)], 1, 20, true);
        // 90 > pin_move (80): das Tor geht auf, aber DIESES Bild bleibt
        // still — die aufgelaufene Strecke nachzureichen waere ein Sprung.
        let out = t.feed(1, &[(0, 100, 290)], 1, 40, true);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        let out = t.feed(1, &[(0, 100, 340)], 1, 60, true);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 50, scroll: 0, hscroll: 0, tap: 0 },
            "ab jetzt folgt der Zeiger wieder");
    }

    /// Das Festhalten endet mit der Taste, nicht mit dem Finger.
    #[test]
    fn releasing_the_button_unpins() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, false);
        t.feed(1, &[(0, 100, 200)], 1, 20, true);
        t.feed(1, &[(0, 100, 210)], 1, 40, true);
        let out = t.feed(1, &[(0, 100, 240)], 1, 60, false);
        assert_eq!(out, Out::Frame { n: 1, gesture: 1, dx: 0, dy: 30, scroll: 0, hscroll: 0, tap: 0 });
    }

    /// Und der Fehler, den das Festhalten sonst einbaut: unter der Taste
    /// misst der Zeiger null Weg — wer das Antippen DANACH fragt, haelt
    /// jeden physischen Klick fuer ein Tippen und gibt ihn doppelt aus.
    #[test]
    fn a_physical_click_is_never_also_a_tap() {
        let mut t = tr();
        t.feed(1, &[(0, 100, 200)], 1, 0, true);
        t.feed(1, &[(0, 105, 205)], 1, 30, true);
        let out = t.feed(0, &[], 1, 60, false);
        assert_eq!(out, Out::Frame { n: 0, gesture: 0, dx: 0, dy: 0, scroll: 0, hscroll: 0, tap: 0 });
        assert_eq!(t.hold(), 0, "der Druck war der Klick — ein zweiter waere erfunden");
    }

    /// Zwei Finger nach RECHTS rollen nach rechts — und die senkrechte
    /// Achse bleibt dabei still.
    #[test]
    fn two_fingers_moving_right_scroll_right() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        let out = two_finger(&mut t, (0, 150, 200), (1, 450, 210));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: 0, hscroll: 1, tap: 0 });
    }

    /// Ein schraeger Wisch traegt BEIDE Achsen. Eine Achssperre wuerde
    /// ihn halbieren, und libinput sperrt beim Zweifinger-Rollen nicht.
    #[test]
    fn a_diagonal_swipe_carries_both_axes() {
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 210));
        let out = two_finger(&mut t, (0, 150, 250), (1, 450, 260));
        assert_eq!(out, Out::Frame {
            n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 1, tap: 0 });
    }

    /// Die quere Achse hat ihren EIGENEN Schritt: ein Pad ist breiter als
    /// hoch, und ein Schritt aus der Hoehe waere quer zu fein.
    #[test]
    fn the_horizontal_step_is_its_own() {
        let mut t = Tracker::new(50, 100, 40, 80);
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
        let mut t = tr();
        two_finger(&mut t, (0, 100, 200), (1, 400, 900));
        // Dasselbe Bild, andere Reihenfolge, beide 50 nach unten.
        let out = two_finger(&mut t, (1, 400, 950), (0, 100, 250));
        assert_eq!(out, Out::Frame { n: 2, gesture: 2, dx: 0, dy: 0, scroll: -1, hscroll: 0, tap: 0 });
    }

    /// Teilstrecken laufen auf, bis sie eine Raste ergeben — sonst kaeme
    /// bei jedem Bild eine und das Rollen waere unbrauchbar schnell.
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

    /// Ein Geraet mit ZWEI Plaetzen im Bericht meldet beide Finger auf
    /// einmal — dann gibt es keine Folgeberichte.
    #[test]
    fn a_device_with_two_slots_needs_no_continuation() {
        let mut t = tr();
        assert!(matches!(
            t.feed(2, &[(0, 100, 200), (1, 400, 210)], 2, 0, false),
            Out::Frame { n: 2, gesture: 2, .. }));
    }
}
