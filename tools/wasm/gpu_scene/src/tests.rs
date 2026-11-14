//! plan_draws against plan: the same areas; and draw_push's layout.

use crate::plan::*;
use crate::scene::{checked, draw_push, surface, DRAW_PUSH_WORDS};
use alloc::vec::Vec;

#[derive(Default)]
struct Rec {
    areas: Vec<rect>,
}

impl sink for Rec {
    fn bins(&mut self, _: u32, _: &[u32]) -> Result<(), i32> {
        Ok(())
    }
    fn compose(&mut self, area: &rect, _: &bin_table) -> Result<(), i32> {
        self.areas.push(*area);
        Ok(())
    }
}

impl draw_sink for Rec {
    fn draw(&mut self, area: &rect) -> Result<(), i32> {
        self.areas.push(*area);
        Ok(())
    }
}

fn r(x: u32, y: u32, w: u32, h: u32) -> rect {
    rect { x, y, w, h }
}

fn shape(kind: u8, reach: rect) -> item_shape {
    item_shape { kind, reach, cover: (EMPTY, EMPTY) }
}

const SCREEN: rect = rect { x: 0, y: 0, w: 1920, h: 1080 };
const LIM: limits = limits { bins_words: 1 << 20 };

fn scene() -> Vec<item_shape> {
    alloc::vec![
        item_shape { kind: KIND_BACKGROUND, reach: SCREEN, cover: (SCREEN, SCREEN) },
        shape(KIND_CHROME, r(100, 100, 800, 600)),
        shape(7, r(120, 140, 400, 300)),
        shape(KIND_PANEL, r(0, 1040, 1920, 40)),
        shape(7, r(10, 1045, 200, 30)),
        shape(KIND_PANEL, r(1500, 200, 300, 400)),
        shape(8, r(1510, 210, 50, 50)),
    ]
}

#[test]
fn draws_cover_what_compose_covers() {
    let items = scene();
    for damage in [alloc::vec![SCREEN], alloc::vec![r(50, 50, 300, 300), r(1400, 1000, 400, 80)],
                   alloc::vec![r(1600, 300, 10, 10)]] {
        let mut a = Rec::default();
        plan(&mut a, &LIM, &SCREEN, &damage, &items).unwrap();
        let mut b = Rec::default();
        plan_draws(&mut b, &SCREEN, &damage, &items).unwrap();
        assert_eq!(a.areas, b.areas, "damage {:?}", damage);
        assert_eq!(b.areas, damage);
    }
}

fn checked_with(items: Vec<[u32; 16]>) -> checked {
    let mut frame = [0u32; 32];
    frame[3] = 7;
    frame[4] = 0x1000;
    frame[5] = 0x1;
    frame[6] = 3840;
    frame[7] = 0x2000;
    frame[9] = 960;
    frame[10] = 0x3000;
    frame[12] = 1920;
    frame[13] = 200;
    frame[14] = 0x123456;
    frame[15] = 40;
    frame[18] = 0x4000;
    frame[19] = 0x2;
    let target = surface { va: 0, pitch: 7680, width: 1920, height: 1080, size: 7680 * 1080 };
    checked { screen: SCREEN, width: 1920, height: 1080, target, damage: Vec::new(), shapes: Vec::new(),
              frame, items }
}

#[test]
fn draw_push_layout() {
    let mut words = Vec::new();
    for kind in 1..=9u32 {
        let mut w = [0u32; 16];
        w[0] = kind;
        w[15] = 0xabcd;
        words.push(w);
    }
    let c = checked_with(words);
    let at = r(10, 20, 30, 40);
    let src = |kind: usize| {
        let p = draw_push(&c, kind - 1, &at);
        assert_eq!(p.len(), DRAW_PUSH_WORDS);
        assert_eq!(&p[0..4], &[10, 20, 40, 60]);
        assert_eq!(p[4], kind as u32);
        assert_eq!(p[19], 0xabcd);
        assert_eq!(&p[20..24], &[7, 200, 0x123456, 40]);
        assert_eq!(&p[27..29], &[1920, 1080]);
        [p[24], p[25], p[26]]
    };
    assert_eq!(src(1), [0x1000, 0x1, 3840]);
    assert_eq!(src(3), [0x2000, 0, 960]);
    assert_eq!(src(6), [0x2000, 0, 960]);
    assert_eq!(src(4), [0x3000, 0, 1920]);
    assert_eq!(src(9), [0x4000, 0x2, 0]);
    for k in [2, 5, 7, 8] {
        assert_eq!(src(k), [0; 3]);
    }
}

/// A small deterministic generator for the property test.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as u32
    }
    fn rect(&mut self) -> rect {
        let (x, y) = (self.next(60), self.next(60));
        r(x, y, 1 + self.next(40), 1 + self.next(40))
    }
}

#[test]
fn visible_parts_are_the_reach_less_later_covers_pixel_for_pixel() {
    let mut g = Lcg(7);
    for _ in 0..2000 {
        let n = 1 + g.next(6) as usize;
        let items: Vec<item_shape> = (0..n).map(|_| {
            let reach = g.rect();
            let c0 = if g.next(2) == 0 { reach.intersect(&g.rect()) } else { EMPTY };
            let c1 = if g.next(3) == 0 { reach.intersect(&g.rect()) } else { EMPTY };
            item_shape { kind: 8, reach, cover: (c0, c1) }
        }).collect();
        let area = g.rect();
        for i in 0..n {
            let parts = visible_parts(&items, n, i, &area);
            let mut seen = [[0u8; 100]; 100];
            for p in &parts {
                assert!(!p.is_empty());
                for y in p.y..p.y1() {
                    for x in p.x..p.x1() {
                        seen[y as usize][x as usize] += 1;
                    }
                }
            }
            for y in 0..100u32 {
                for x in 0..100u32 {
                    let px = r(x, y, 1, 1);
                    let reached = items[i].reach.intersect(&area).contains(&px);
                    let covered = items[i + 1..].iter().any(|it| it.cover.0.contains(&px) || it.cover.1.contains(&px));
                    let n = seen[y as usize][x as usize];
                    // Never twice, never outside the reach; every pixel no
                    // later item covers is drawn.
                    assert!(n <= 1);
                    assert!(n == 0 || reached);
                    if reached && !covered {
                        assert_eq!(n, 1, "pixel {},{} of item {} missing", x, y, i);
                    }
                }
            }
        }
    }
}
