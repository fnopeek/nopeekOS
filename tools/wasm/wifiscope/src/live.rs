//! The live view: a window with the controls on top and a canvas the
//! compositor scales on the GPU.
//!
//! Two views:
//!
//! - Track: one channel, A's carrier on all the time. Each frame is the
//!   mean over the averaging time; a slow average of the frames is the
//!   background (the crosstalk and A's own DC), and what is left is the
//!   signal. Left: the signal's size, its movement within the frame and the
//!   noise over time; right: the signal in the IQ plane.
//! - Sweep: the off/on pair on every channel of `CH_SWEEP`. Per channel the
//!   background is subtracted and the rest divided by the background's
//!   phase, which removes the 180 deg flips and puts the crosstalk's path
//!   at range zero. The range profile over the channels is one row of a
//!   waterfall: the echo sounder.
//!
//! `Rec` keeps every frame or sweep and writes `wifiscope/live.csv` when
//! recording stops.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use nopeek_widgets::prefab;
use nopeek_widgets::style::Spacing;
use nopeek_widgets::*;

use crate::host;
use crate::scope::{self, Stat, CH_SWEEP};

// ── Numbers ──────────────────────────────────────────────────────

const PI: f64 = core::f64::consts::PI;

fn abs(x: f64) -> f64 {
    if x < 0.0 { -x } else { x }
}

fn floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}

fn sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f64::from_bits((x.to_bits() >> 1) + 0x1FF8_0000_0000_0000);
    for _ in 0..5 {
        y = 0.5 * (y + x / y);
    }
    y
}

/// sin on [-pi, pi] after reduction; error below 1e-6.
fn sin(x: f64) -> f64 {
    let x = x - 2.0 * PI * floor(x / (2.0 * PI) + 0.5);
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0
        * (1.0 - x2 / 110.0 * (1.0 - x2 / 156.0))))))
}

fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let (ax, ay) = (abs(x), abs(y));
    let (z, swap) = if ay > ax { (ax / ay, true) } else { (ay / ax, false) };
    let z2 = z * z;
    let mut a = z * (0.999_866 + z2 * (-0.330_299_5 + z2 * (0.180_141 + z2 * (-0.085_133
        + z2 * 0.020_835_1))));
    if swap {
        a = PI / 2.0 - a;
    }
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 { -a } else { a }
}

#[derive(Clone, Copy, Default)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn new(re: f64, im: f64) -> C {
        C { re, im }
    }
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
    fn scale(self, k: f64) -> C {
        C::new(self.re * k, self.im * k)
    }
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    fn conj(self) -> C {
        C::new(self.re, -self.im)
    }
    fn mag(self) -> f64 {
        sqrt(self.re * self.re + self.im * self.im)
    }
    fn deg(self) -> f64 {
        atan2(self.im, self.re) * 180.0 / PI
    }
    fn dot(self, o: C) -> f64 {
        self.re * o.re + self.im * o.im
    }
}

fn mean(s: &Stat) -> C {
    let n = s.n as f64;
    C::new(s.si as f64 / n, s.sq as f64 / n)
}

/// Standard error of a block's complex mean.
fn sem(s: &Stat) -> f64 {
    let n = s.n as f64;
    let m = mean(s);
    let var = s.sii as f64 / n - m.re * m.re + s.sqq as f64 / n - m.im * m.im;
    sqrt(if var > 0.0 { var } else { 0.0 } / n)
}

// ── Text ─────────────────────────────────────────────────────────

fn fmt_f(out: &mut String, x: f64, dec: u32) {
    let mut x = x;
    if x < 0.0 {
        out.push('-');
        x = -x;
    }
    let mut k = 1u64;
    for _ in 0..dec {
        k *= 10;
    }
    let v = (x * k as f64 + 0.5) as u64;
    fmt_u(out, v / k);
    if dec > 0 {
        out.push('.');
        let mut frac = v % k;
        let mut d = k / 10;
        while d > 0 {
            out.push((b'0' + (frac / d) as u8) as char);
            frac %= d;
            d /= 10;
        }
    }
}

fn fmt_u(out: &mut String, v: u64) {
    let mut d = [0u8; 20];
    let mut k = d.len();
    let mut v = v;
    loop {
        k -= 1;
        d[k] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for &b in &d[k..] {
        out.push(b as char);
    }
}

// ── Pixels ───────────────────────────────────────────────────────

const W: usize = 800;
const H: usize = 420;
const CANVAS: u32 = 1;

type Rgb = (u8, u8, u8);

const BG: Rgb = (18, 20, 24);
const GRID: Rgb = (48, 52, 60);
const GREEN: Rgb = (90, 220, 120);
const ORANGE: Rgb = (240, 160, 60);
const GREY: Rgb = (120, 124, 132);
const CYAN: Rgb = (80, 200, 240);
const WHITE: Rgb = (230, 230, 230);

struct Px {
    buf: Vec<u8>,
}

impl Px {
    fn new() -> Px {
        Px { buf: alloc::vec![0; W * H * 4] }
    }
    fn fill(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Rgb) {
        for y in y0..(y0 + h).min(H) {
            for x in x0..(x0 + w).min(W) {
                self.set(x, y, c);
            }
        }
    }
    fn set(&mut self, x: usize, y: usize, c: Rgb) {
        if x < W && y < H {
            let i = (y * W + x) * 4;
            self.buf[i] = c.2;
            self.buf[i + 1] = c.1;
            self.buf[i + 2] = c.0;
            self.buf[i + 3] = 255;
        }
    }
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            if x >= 0 && y >= 0 {
                self.set(x as usize, y as usize, c);
            }
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }
}

/// Black, blue, cyan, yellow, white over 0..1.
fn heat(v: f64) -> Rgb {
    let v = if v < 0.0 { 0.0 } else if v > 1.0 { 1.0 } else { v };
    let stops: [(f64, Rgb); 5] = [
        (0.0, (10, 10, 20)), (0.3, (30, 60, 200)), (0.55, (40, 200, 220)),
        (0.8, (250, 220, 60)), (1.0, (255, 255, 255)),
    ];
    for k in 1..stops.len() {
        if v <= stops[k].0 {
            let (a, ca) = stops[k - 1];
            let (b, cb) = stops[k];
            let t = (v - a) / (b - a);
            let mix = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * t) as u8;
            return (mix(ca.0, cb.0), mix(ca.1, cb.1), mix(ca.2, cb.2));
        }
    }
    stops[4].1
}

// ── State ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum View {
    Track,
    Sweep,
}

const AVG_MS: [u64; 4] = [100, 250, 500, 1000];
const TAU_S: [f64; 4] = [2.0, 5.0, 10.0, 30.0];
const TX_MAX: u32 = 7;
/// The kernel starts the WiFi driver again only within an hour.
const LIFETIME_MS: u64 = 55 * 60 * 1000;

/// Range bins of the sweep profile.
const BINS: usize = 76;
const BIN_M: f64 = 0.1;
const C_LIGHT: f64 = 299_792_458.0;
/// Pairs per channel per sweep.
const SWEEP_PAIRS: usize = 2;

struct Live {
    view: View,
    ch: usize,
    txg: u32,
    avg: usize,
    tau: usize,
    bg: Option<C>,
    /// Per frame: |signal|, movement, noise.
    hist: VecDeque<(f64, f64, f64)>,
    trail: VecDeque<C>,
    z: C,
    d: C,
    mv: f64,
    noise: f64,
    sbg: [Option<C>; CH_SWEEP.len()],
    profile: [f64; BINS],
    peak_scale: f64,
    waterfall: Vec<f64>,
    sweep_ms: u64,
    rec: Option<Vec<u8>>,
    t0: u64,
    status: String,
    note: String,
}

// Actions.
const A_TRACK: u32 = 1;
const A_SWEEP: u32 = 2;
const A_TX_DOWN: u32 = 3;
const A_TX_UP: u32 = 4;
const A_CH_DOWN: u32 = 5;
const A_CH_UP: u32 = 6;
const A_AVG: u32 = 7;
const A_TAU: u32 = 8;
const A_ZERO: u32 = 9;
const A_REC: u32 = 10;
const A_QUIT: u32 = 11;

fn btn(label: &str, act: u32, on: bool) -> Widget {
    let style = if on { prefab::ButtonStyle::Primary } else { prefab::ButtonStyle::Secondary };
    prefab::button(label, style, ActionId(act))
}

fn text(s: &str, muted: bool) -> Widget {
    let mut m = alloc::vec![Modifier::PaddingXY { x: 8, y: 3 }];
    if muted {
        m.push(Modifier::Tint(Token::OnSurfaceMuted));
    }
    Widget::Text { content: s.into(), style: TextStyle::Mono, modifiers: m }
}

fn tree(l: &Live) -> Widget {
    let mut tx = String::from("TX ");
    fmt_u(&mut tx, l.txg as u64);
    let mut ch = String::from("ch ");
    fmt_u(&mut ch, CH_SWEEP[l.ch] as u64);
    let mut avg = String::from("avg ");
    fmt_u(&mut avg, AVG_MS[l.avg]);
    avg.push_str(" ms");
    let mut tau = String::from("bg ");
    fmt_f(&mut tau, TAU_S[l.tau], 0);
    tau.push_str(" s");
    Widget::Column {
        children: alloc::vec![
            Widget::Row {
                children: alloc::vec![
                    btn("Track", A_TRACK, l.view == View::Track),
                    btn("Sweep", A_SWEEP, l.view == View::Sweep),
                    btn("TX -", A_TX_DOWN, false),
                    text(&tx, false),
                    btn("TX +", A_TX_UP, false),
                    btn("<", A_CH_DOWN, false),
                    text(&ch, false),
                    btn(">", A_CH_UP, false),
                    btn(&avg, A_AVG, false),
                    btn(&tau, A_TAU, false),
                    btn("Zero", A_ZERO, false),
                    btn(if l.rec.is_some() { "Rec *" } else { "Rec" }, A_REC, l.rec.is_some()),
                    btn("Quit", A_QUIT, false),
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: alloc::vec![Modifier::PaddingXY { x: 8, y: 6 }],
            },
            Widget::Divider,
            Widget::Canvas {
                id: CanvasId(CANVAS),
                width: W as u16,
                height: H as u16,
                modifiers: alloc::vec![Modifier::Flex(1), Modifier::Background(Token::Page)],
            },
            Widget::Divider,
            text(&l.status, false),
            text(&l.note, true),
        ],
        spacing: 0,
        align: Align::Stretch,
        modifiers: alloc::vec![],
    }
}

fn commit(l: &Live) {
    if let Ok(bytes) = wire::encode(&tree(l)) {
        host_w::scene_commit(&bytes);
    }
}

use nopeek_widgets::host as host_w;

fn show(px: &Px) {
    if !host_w::canvas_frame(CANVAS, &px.buf, W as u32, H as u32) {
        host_w::canvas_commit(CANVAS, &px.buf, W as u32, H as u32);
    }
}

/// A message in the window until it is closed: the card is not ours or the
/// bring-up failed.
pub fn hold_message(msg: &str) {
    let w = Widget::Column {
        children: alloc::vec![text("wifiscope", false), text(msg, true)],
        spacing: 0,
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::PaddingXY { x: 12, y: 12 }],
    };
    if let Ok(bytes) = wire::encode(&w) {
        host_w::scene_commit(&bytes);
    }
    let mut buf = [0u8; 64];
    loop {
        match nopeek_widgets::events::poll(&mut buf) {
            nopeek_widgets::events::Poll::Gone => return,
            nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Escape)) => {
                host_w::close_widget();
                return;
            }
            _ => {}
        }
        host_w::wait(1, 500);
    }
}

/// Shown while the chip comes up.
pub fn splash(msg: &str) {
    let w = Widget::Column {
        children: alloc::vec![text("wifiscope", false), text(msg, true)],
        spacing: 0,
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::PaddingXY { x: 12, y: 12 }],
    };
    if let Ok(bytes) = wire::encode(&w) {
        host_w::scene_commit(&bytes);
    }
}

// ── Measurement ──────────────────────────────────────────────────

/// Switch the carrier on at `txg`, checked; false if it never showed.
fn carrier_on(h: i32, txg: u32) -> bool {
    let (off, on, _) = scope::pair(h, txg);
    mean(&on).sub(mean(&off)).mag() >= 10.0
}

impl Live {
    fn reset_track(&mut self) {
        self.bg = None;
        self.hist.clear();
        self.trail.clear();
    }

    fn retune(&mut self, h: i32) {
        scope::tune(h, CH_SWEEP[self.ch]);
        if !carrier_on(h, self.txg) {
            self.note = String::from("the carrier did not come on; press TX or a channel again");
        }
        self.reset_track();
    }

    /// One Track frame: blocks for the averaging time.
    fn track_frame(&mut self, h: i32) {
        let end = host::now_ms() + AVG_MS[self.avg];
        let mut zs: Vec<C> = Vec::new();
        let mut noise = 0.0;
        while host::now_ms() < end || zs.is_empty() {
            let s = scope::block(h);
            noise += sem(&s);
            zs.push(mean(&s));
        }
        let n = zs.len() as f64;
        let z = zs.iter().fold(C::default(), |a, &b| a.add(b)).scale(1.0 / n);
        let mv = zs.iter().map(|&b| b.sub(z).mag()).sum::<f64>() / n;
        self.noise = noise / n;
        let bg = match self.bg {
            None => z,
            Some(b) => {
                let k = (AVG_MS[self.avg] as f64 / 1000.0 / TAU_S[self.tau]).min(1.0);
                b.add(z.sub(b).scale(k))
            }
        };
        self.bg = Some(bg);
        self.z = z;
        self.d = z.sub(bg);
        self.mv = mv;
        self.hist.push_back((self.d.mag(), mv, self.noise));
        while self.hist.len() > 560 {
            self.hist.pop_front();
        }
        self.trail.push_back(self.d);
        while self.trail.len() > 120 {
            self.trail.pop_front();
        }
        if let Some(r) = self.rec.as_mut() {
            scope::put_i(r, (host::now_ms() - self.t0) as i64);
            scope::put(r, ",track,");
            scope::put_i(r, CH_SWEEP[self.ch] as i64);
            r.push(b',');
            scope::put_i(r, self.txg as i64);
            for v in [z.re, z.im, bg.re, bg.im, mv, self.noise] {
                r.push(b',');
                scope::put_i(r, (v * 1000.0) as i64);
            }
            r.push(b'\n');
        }
    }

    /// One sweep over all channels; afterwards the carrier is on at the
    /// Track channel again.
    fn sweep(&mut self, h: i32) {
        let t = host::now_ms();
        let mut q = [C::default(); CH_SWEEP.len()];
        let mut ok = [false; CH_SWEEP.len()];
        for (k, &ch) in CH_SWEEP.iter().enumerate() {
            scope::tune(h, ch);
            let mut acc: Vec<C> = Vec::new();
            for _ in 0..SWEEP_PAIRS {
                let (off, on, _) = scope::pair(h, self.txg);
                let z = mean(&on).sub(mean(&off));
                if z.mag() >= 10.0 {
                    acc.push(z);
                }
            }
            scope::carrier(h, None);
            let Some(&first) = acc.first() else { continue };
            let refz = self.sbg[k].unwrap_or(first);
            let z = acc.iter().fold(C::default(), |a, &b| {
                a.add(if b.dot(refz) < 0.0 { b.scale(-1.0) } else { b })
            }).scale(1.0 / acc.len() as f64);
            let bg = match self.sbg[k] {
                None => z,
                Some(b) => b.add(z.sub(b).scale(0.1)),
            };
            self.sbg[k] = Some(bg);
            let unit = bg.conj().scale(1.0 / bg.mag().max(1e-9));
            q[k] = z.sub(bg).mul(unit);
            ok[k] = true;
            if let Some(r) = self.rec.as_mut() {
                scope::put_i(r, (host::now_ms() - self.t0) as i64);
                scope::put(r, ",sweep,");
                scope::put_i(r, ch as i64);
                r.push(b',');
                scope::put_i(r, self.txg as i64);
                for v in [z.re, z.im, bg.re, bg.im, 0.0, 0.0] {
                    r.push(b',');
                    scope::put_i(r, (v * 1000.0) as i64);
                }
                r.push(b'\n');
            }
        }
        for b in 0..BINS {
            let r = b as f64 * BIN_M;
            let mut acc = C::default();
            let mut n = 0.0;
            for (k, &ch) in CH_SWEEP.iter().enumerate() {
                if !ok[k] {
                    continue;
                }
                let f = (5000.0 + 5.0 * ch as f64) * 1e6;
                let ph = 4.0 * PI * f * r / C_LIGHT;
                acc = acc.add(q[k].mul(C::new(cos(ph), sin(ph))));
                n += 1.0;
            }
            self.profile[b] = if n > 0.0 { acc.mag() / n } else { 0.0 };
        }
        let top = self.profile.iter().cloned().fold(0.0, f64::max);
        self.peak_scale = (self.peak_scale * 0.95).max(top).max(0.5);
        let row: Vec<f64> = self.profile.iter().map(|&v| v / self.peak_scale).collect();
        self.waterfall.splice(0..0, row);
        self.waterfall.truncate(BINS * 320);
        self.sweep_ms = host::now_ms() - t;
    }

    fn draw_track(&self, px: &mut Px) {
        px.fill(0, 0, W, H, BG);
        let (pw, ph) = (560usize, H);
        let top = self.hist.iter().map(|x| x.0.max(x.1)).fold(0.0, f64::max).max(self.noise * 4.0).max(1.0) * 1.2;
        for g in 1..4 {
            px.fill(0, ph * g / 4, pw, 1, GRID);
        }
        let y = |v: f64| (ph as f64 - 1.0 - v / top * (ph as f64 - 2.0)) as i32;
        let n = self.hist.len();
        for (i, w) in self.hist.iter().zip(self.hist.iter().skip(1)).enumerate() {
            let x0 = (pw - n + i) as i32;
            px.line(x0, y(w.0.2), x0 + 1, y(w.1.2), GREY);
            px.line(x0, y(w.0.1), x0 + 1, y(w.1.1), ORANGE);
            px.line(x0, y(w.0.0), x0 + 1, y(w.1.0), GREEN);
        }
        // IQ plane.
        let (cx, cy, r) = (680i32, 210i32, 110i32);
        px.fill(570, 0, 1, H, GRID);
        px.line(cx - r, cy, cx + r, cy, GRID);
        px.line(cx, cy - r, cx, cy + r, GRID);
        let k = r as f64 / top;
        let pts: Vec<(i32, i32)> = self.trail.iter()
            .map(|d| (cx + (d.re * k) as i32, cy - (d.im * k) as i32)).collect();
        for w in pts.windows(2) {
            px.line(w[0].0, w[0].1, w[1].0, w[1].1, CYAN);
        }
        if let Some(&(x, y)) = pts.last() {
            px.fill((x - 2).max(0) as usize, (y - 2).max(0) as usize, 5, 5, WHITE);
        }
    }

    fn draw_sweep(&self, px: &mut Px) {
        px.fill(0, 0, W, H, BG);
        let ph = 96usize;
        let colw = W as f64 / BINS as f64;
        for m in 1..8 {
            let x = (m as f64 / BIN_M * colw) as usize;
            px.fill(x, 0, 1, H, GRID);
        }
        let mut last: Option<(i32, i32)> = None;
        for b in 0..BINS {
            let v = self.profile[b] / self.peak_scale.max(1e-9);
            let x = ((b as f64 + 0.5) * colw) as i32;
            let y = (ph as f64 - 2.0 - v.min(1.0) * (ph as f64 - 6.0)) as i32;
            if let Some((x0, y0)) = last {
                px.line(x0, y0, x, y, GREEN);
            }
            last = Some((x, y));
        }
        px.fill(0, ph, W, 1, GRID);
        let rows = self.waterfall.len() / BINS;
        for row in 0..rows.min(H - ph - 1) {
            for b in 0..BINS {
                let c = heat(self.waterfall[row * BINS + b]);
                let x0 = (b as f64 * colw) as usize;
                let x1 = ((b + 1) as f64 * colw) as usize;
                px.fill(x0, ph + 1 + row, x1 - x0, 1, c);
            }
        }
    }

    fn update_text(&mut self) {
        let left = LIFETIME_MS.saturating_sub(host::now_ms() - self.t0) / 1000;
        let mut s = String::new();
        match self.view {
            View::Track => {
                s.push_str("carrier ");
                fmt_f(&mut s, self.z.mag(), 1);
                s.push_str(" @ ");
                fmt_f(&mut s, self.z.deg(), 0);
                s.push_str(" deg   signal ");
                fmt_f(&mut s, self.d.mag(), 2);
                s.push_str("   movement ");
                fmt_f(&mut s, self.mv, 2);
                s.push_str("   noise ");
                fmt_f(&mut s, self.noise, 2);
            }
            View::Sweep => {
                s.push_str("sweep ");
                fmt_u(&mut s, self.sweep_ms);
                s.push_str(" ms   peak ");
                let (bi, bv) = self.profile.iter().enumerate()
                    .fold((0, 0.0), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
                fmt_f(&mut s, bi as f64 * BIN_M, 1);
                s.push_str(" m (");
                fmt_f(&mut s, bv, 2);
                s.push_str(")   scale ");
                fmt_f(&mut s, self.peak_scale, 2);
            }
        }
        s.push_str("   ");
        fmt_u(&mut s, left / 60);
        s.push(':');
        if left % 60 < 10 {
            s.push('0');
        }
        fmt_u(&mut s, left % 60);
        s.push_str(" left");
        self.status = s;
        if self.note.is_empty() || self.note.starts_with("green:") || self.note.starts_with("top:") {
            self.note = String::from(match self.view {
                View::Track => "green: signal after the background   orange: movement within a frame   grey: noise   right: signal in the IQ plane",
                View::Sweep => "top: range profile now   below: one row per sweep, newest on top   lines every 1 m from the antenna",
            });
        }
    }
}

/// The live view until Quit, Esc, closing the window or the lifetime.
pub fn run(h: i32) {
    let mut l = Live {
        view: View::Track,
        ch: CH_SWEEP.iter().position(|&c| c == scope::CH_START).unwrap_or(0),
        txg: 0,
        avg: 2,
        tau: 2,
        bg: None,
        hist: VecDeque::new(),
        trail: VecDeque::new(),
        z: C::default(),
        d: C::default(),
        mv: 0.0,
        noise: 0.0,
        sbg: [None; CH_SWEEP.len()],
        profile: [0.0; BINS],
        peak_scale: 0.5,
        waterfall: Vec::new(),
        sweep_ms: 0,
        rec: None,
        t0: host::now_ms(),
        status: String::new(),
        note: String::new(),
    };
    let mut px = Px::new();
    l.retune(h);
    l.update_text();
    commit(&l);
    let mut last_commit = 0u64;
    let mut ev = [0u8; 64];
    loop {
        let mut retune = false;
        let mut quit = false;
        let mut changed = false;
        loop {
            match nopeek_widgets::events::poll(&mut ev) {
                nopeek_widgets::events::Poll::Event(e) => {
                    changed = true;
                    match e {
                        Event::Key(KeyCode::Escape) => quit = true,
                        Event::Action(ActionId(a)) => match a {
                            A_TRACK => {
                                if l.view != View::Track {
                                    l.view = View::Track;
                                    retune = true;
                                }
                            }
                            A_SWEEP => l.view = View::Sweep,
                            A_TX_DOWN => {
                                l.txg = l.txg.saturating_sub(1);
                                retune = true;
                            }
                            A_TX_UP => {
                                l.txg = (l.txg + 1).min(TX_MAX);
                                retune = true;
                            }
                            A_CH_DOWN => {
                                l.ch = (l.ch + CH_SWEEP.len() - 1) % CH_SWEEP.len();
                                retune = true;
                            }
                            A_CH_UP => {
                                l.ch = (l.ch + 1) % CH_SWEEP.len();
                                retune = true;
                            }
                            A_AVG => {
                                l.avg = (l.avg + 1) % AVG_MS.len();
                                l.reset_track();
                            }
                            A_TAU => l.tau = (l.tau + 1) % TAU_S.len(),
                            A_ZERO => {
                                l.reset_track();
                                l.sbg = [None; CH_SWEEP.len()];
                                l.waterfall.clear();
                                l.peak_scale = 0.5;
                            }
                            A_REC => match l.rec.take() {
                                Some(r) => {
                                    let mut out = Vec::new();
                                    scope::put(&mut out, "# wifiscope live, values x1000\nt_ms,view,channel,tx_gain,z_re,z_im,bg_re,bg_im,movement,noise\n");
                                    out.extend_from_slice(&r);
                                    scope::save("live.csv", &out);
                                    l.note = String::from("written: wifiscope/live.csv");
                                }
                                None => l.rec = Some(Vec::new()),
                            },
                            A_QUIT => quit = true,
                            _ => {}
                        },
                        _ => {}
                    }
                }
                nopeek_widgets::events::Poll::Empty => break,
                nopeek_widgets::events::Poll::Gone => return,
            }
        }
        if quit || host::now_ms() - l.t0 > LIFETIME_MS {
            scope::carrier(h, None);
            if let Some(r) = l.rec.take() {
                let mut out = Vec::new();
                scope::put(&mut out, "# wifiscope live, values x1000\nt_ms,view,channel,tx_gain,z_re,z_im,bg_re,bg_im,movement,noise\n");
                out.extend_from_slice(&r);
                scope::save("live.csv", &out);
            }
            host_w::close_widget();
            return;
        }
        if retune && l.view == View::Track {
            l.note.clear();
            l.retune(h);
        }
        match l.view {
            View::Track => {
                l.track_frame(h);
                l.draw_track(&mut px);
            }
            View::Sweep => {
                l.sweep(h);
                l.draw_sweep(&mut px);
            }
        }
        show(&px);
        let now = host::now_ms();
        if changed || now - last_commit > 250 {
            l.update_text();
            commit(&l);
            last_commit = now;
        }
    }
}
