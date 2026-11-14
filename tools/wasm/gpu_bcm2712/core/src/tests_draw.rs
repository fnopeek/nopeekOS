//! `v3d_draw` against v3dv: each `testdata/draw` file is a render pass v3dg
//! recorded with Mesa's v3dv (shaders/gen_draw_tests.py). The same pass is
//! recorded here from v3dg's programs, prog_data and buffer addresses, and
//! every byte of the binning list, render list and indirect buffers
//! (uniform streams included, filled as `v3dv_write_uniforms` fills them)
//! and the submit parameters must match.

use crate::v3d_draw::*;
use crate::shaders_gen::DRAW_PIPELINES;
use std::collections::{BTreeMap, VecDeque};

const SCENARIOS: [(&str, &str); 11] = [
    ("one_64x64", include_str!("../testdata/draw/one_64x64.txt")),
    ("nine_kinds_256x128", include_str!("../testdata/draw/nine_kinds_256x128.txt")),
    ("three_1920x1080", include_str!("../testdata/draw/three_1920x1080.txt")),
    ("three_3840x2160", include_str!("../testdata/draw/three_3840x2160.txt")),
    ("sub_area_256x128", include_str!("../testdata/draw/sub_area_256x128.txt")),
    ("sub_area_aligned_512x256", include_str!("../testdata/draw/sub_area_aligned_512x256.txt")),
    ("sub_area_edge_300x200", include_str!("../testdata/draw/sub_area_edge_300x200.txt")),
    ("same_pipeline_256x128", include_str!("../testdata/draw/same_pipeline_256x128.txt")),
    ("rev10_256x128", include_str!("../testdata/draw/rev10_256x128.txt")),
    ("many_draws_640x480", include_str!("../testdata/draw/many_draws_640x480.txt")),
    ("prefetch_slot_256x128", include_str!("../testdata/draw/prefetch_slot_256x128.txt")),
];

/// One program's uniform list: (`quniform_contents`, data).
type Uniforms = Vec<(u32, u32)>;
/// A pipeline's kind, its "<stage>_<field>" values, and the uniform lists
/// of its fragment, vertex and coordinate shader.
type RecordedPipeline = (u32, BTreeMap<String, Vec<String>>, [Uniforms; 3]);

struct Recorded {
    keys: BTreeMap<String, Vec<String>>,
    draws: Vec<(u32, [u32; 4])>,
    /// Pipelines by kind, in v3dg's order.
    pipelines: Vec<RecordedPipeline>,
    vpm: Vec<(VpmConfig, VpmConfig)>,
    /// (kind, va, size, bytes) in each list's order.
    bos: Vec<(String, u32, u32, Vec<u8>)>,
}

fn num(s: &str) -> u32 {
    s.parse().unwrap_or_else(|_| panic!("number {}", s))
}

fn parse(text: &str) -> Recorded {
    let mut r = Recorded { keys: BTreeMap::new(), draws: Vec::new(), pipelines: Vec::new(), vpm: Vec::new(),
                           bos: Vec::new() };
    let mut bytes: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut bo_lines = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.is_empty() || f[0].starts_with('#') {
            continue;
        }
        let rest: Vec<String> = f[1..].iter().map(|s| s.to_string()).collect();
        match f[0] {
            "draw" => r.draws.push((num(f[1]), [num(f[2]), num(f[3]), num(f[4]), num(f[5])])),
            "pipeline" => r.pipelines.push((num(f[1]), BTreeMap::new(), Default::default())),
            "vpm_bin" | "vpm" => {
                let c = VpmConfig { a_s: num(f[1]), v_e: num(f[2]), v_c: num(f[3]) };
                if f[0] == "vpm_bin" {
                    r.vpm.push((c, VpmConfig::default()));
                } else {
                    r.vpm.last_mut().unwrap().1 = c;
                }
            }
            "bo" => bo_lines.push((f[1].to_string(), num(f[2]), num(f[3]))),
            "bytes" => {
                let hex = if f[2] == "-" { "" } else { f[2] };
                let b = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
                bytes.insert(num(f[1]), b);
            }
            // "<stage>-<kind>_<field>" of the last pipeline.
            k if k.len() > 3 && k.as_bytes()[2] == b'-' => {
                let (stage_kind, field) = k.split_once('_').unwrap();
                let stage = &stage_kind[..2];
                let p = r.pipelines.last_mut().unwrap();
                if field == "uniform" {
                    let s = ["fs", "vs", "cs"].iter().position(|&x| x == stage).unwrap();
                    assert_eq!(num(f[1]) as usize, p.2[s].len());
                    p.2[s].push((num(f[2]), num(f[3])));
                } else if field == "varying_flags" {
                    p.1.insert(format!("{}_{}_{}", stage, field, f[1]), rest[1..].to_vec());
                } else {
                    p.1.insert(format!("{}_{}", stage, field), rest);
                }
            }
            k => {
                r.keys.insert(k.to_string(), rest);
            }
        }
    }
    for (kind, va, size) in bo_lines {
        let mut b = bytes.remove(&va).unwrap_or_default();
        assert!(b.len() <= size as usize);
        b.resize(size as usize, 0);
        r.bos.push((kind, va, size, b));
    }
    r
}

fn key(m: &BTreeMap<String, Vec<String>>, k: &str) -> u32 {
    num(&m.get(k).unwrap_or_else(|| panic!("no {}", k))[0])
}

fn flag(m: &BTreeMap<String, Vec<String>>, k: &str) -> bool {
    key(m, k) != 0
}

fn prog_data(m: &BTreeMap<String, Vec<String>>, s: &str) -> ProgData {
    ProgData {
        threads: key(m, &format!("{}_threads", s)),
        single_seg: flag(m, &format!("{}_single_seg", s)),
        tmu_dirty_rcl: flag(m, &format!("{}_tmu_dirty_rcl", s)),
        uniform_count: key(m, &format!("{}_uniforms", s)),
    }
}

fn vs_variant(m: &BTreeMap<String, Vec<String>>, s: &str) -> Variant<VsProgData> {
    let k = |f: &str| key(m, &format!("{}_{}", s, f));
    Variant {
        code: k("va"),
        prog: VsProgData {
            base: prog_data(m, s),
            uses_iid: k("uses_iid") != 0,
            uses_biid: k("uses_biid") != 0,
            uses_vid: k("uses_vid") != 0,
            vpm_input_size: k("vpm_input_size"),
            vpm_output_size: k("vpm_output_size"),
            vcm_cache_size: k("vcm_cache_size"),
            writes_psiz: k("writes_psiz") != 0,
        },
    }
}

fn fs_variant(m: &BTreeMap<String, Vec<String>>) -> Variant<FsProgData> {
    let k = |f: &str| key(m, &format!("fs_{}", f));
    let mut fs = FsProgData {
        base: prog_data(m, "fs"),
        uses_pid: k("uses_pid") != 0,
        num_inputs: k("num_inputs"),
        writes_z: k("writes_z") != 0,
        disable_ez: k("disable_ez") != 0,
        uses_center_w: k("uses_center_w") != 0,
        uses_implicit_point_line_varyings: k("uses_implicit_point_line_varyings") != 0,
        lock_scoreboard_on_first_thrsw: k("lock_scoreboard_on_first_thrsw") != 0,
        ..Default::default()
    };
    for i in 0..VARYING_FLAG_WORDS {
        let v = &m[&format!("fs_varying_flags_{}", i)];
        fs.flat_shade_flags[i] = num(&v[0]);
        fs.noperspective_flags[i] = num(&v[1]);
        fs.centroid_flags[i] = num(&v[2]);
    }
    Variant { code: k("va"), prog: fs }
}

/// Hands out the buffers v3dg's job had, in order, per kind.
struct Replay {
    queues: BTreeMap<&'static str, VecDeque<(u32, u32)>>,
}

impl BoAlloc for Replay {
    fn alloc(&mut self, kind: BoKind, size: u32) -> Option<u32> {
        let k = match kind {
            BoKind::Bcl => "bcl",
            BoKind::Rcl => "rcl",
            BoKind::Indirect => "indirect",
            BoKind::TileAlloc => "tile_alloc",
            BoKind::TileState => "tile_state",
        };
        let (va, want) = self.queues.get_mut(k)?.pop_front().unwrap_or_else(|| panic!("one {} too many", k));
        assert_eq!(size, want, "{} size", k);
        Some(va)
    }
}

fn push_words(rect: [u32; 4]) -> [u32; PUSH_WORDS] {
    let mut p: [u32; PUSH_WORDS] = core::array::from_fn(|j| 0xc0de_0000 | j as u32);
    p[..4].copy_from_slice(&rect);
    p
}

/// `generated`: the programs' prog_data and uniform lists from
/// shaders_gen's DRAW_PIPELINES (code addresses stay v3dg's), the streams
/// filled by `write_uniforms`.
fn check(name: &str, text: &str, generated: bool) {
    let r = parse(text);
    let size = &r.keys["size"];
    let area = &r.keys["area"];
    let (width, height) = (num(&size[0]), num(&size[1]));
    let image = Image { va: key(&r.keys, "image_va"), stride: key(&r.keys, "image_stride"), width, height };
    let render_area = Rect { x: num(&area[0]), y: num(&area[1]), width: num(&area[2]), height: num(&area[3]) };
    let viewport = Viewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0,
                              max_depth: 1.0 };
    let scissor = Rect { x: 0, y: 0, width, height };

    let mut pipelines = Vec::new();
    let mut kinds = Vec::new();
    for (i, (kind, m, _)) in r.pipelines.iter().enumerate() {
        let mut p = Pipeline {
            vs: vs_variant(m, "vs"),
            cs: vs_variant(m, "cs"),
            fs: fs_variant(m),
            color_write_mask: 0x1,
        };
        if generated {
            let g = DRAW_PIPELINES.iter().find(|g| g.kind == *kind).expect("a generated pipeline");
            assert_eq!(g.vs.prog, p.vs.prog, "{}: vs prog_data", name);
            assert_eq!(g.cs.prog, p.cs.prog, "{}: cs prog_data", name);
            assert_eq!(g.fs.prog, p.fs.prog, "{}: fs prog_data", name);
            p.vs.prog = g.vs.prog;
            p.cs.prog = g.cs.prog;
            p.fs.prog = g.fs.prog;
        }
        assert_eq!(compute_vpm_config(&p.cs.prog, &p.vs.prog), r.vpm[i], "{}: vpm config", name);
        pipelines.push(p);
        kinds.push(*kind);
    }
    let draws: Vec<Draw> = r.draws.iter().map(|&(k, rect)| Draw {
        pipeline: kinds.iter().position(|&x| x == k).unwrap(),
        push: push_words(rect),
        vertex_count: 4,
    }).collect();
    let pass = Pass { rev: key(&r.keys, "rev"), image, load: true, render_area, viewport, scissor, pipelines: &pipelines,
                      draws: &draws };

    let mut queues: BTreeMap<&'static str, VecDeque<(u32, u32)>> = BTreeMap::new();
    for (k, va, size, _) in &r.bos {
        let k: &'static str = match k.as_str() {
            "bcl" => "bcl",
            "rcl" => "rcl",
            _ => "indirect",
        };
        queues.entry(k).or_default().push_back((*va, *size));
    }
    queues.entry("tile_alloc").or_default().push_back((key(&r.keys, "qma"), key(&r.keys, "qms")));
    queues.entry("tile_state").or_default().push_back((key(&r.keys, "qts"), key(&r.keys, "qts_size")));
    let mut alloc = Replay { queues };

    // v3dv_write_uniforms for the contents these programs use.
    let (scale, translate) = viewport_compute_xform(&viewport);
    let mut fill = |d: usize, stage: Stage, words: &mut [u32]| {
        if generated {
            let g = DRAW_PIPELINES.iter().find(|g| g.kind == kinds[draws[d].pipeline]).expect("a pipeline");
            let list = match stage {
                Stage::Fragment => g.fs.uniforms,
                Stage::Vertex => g.vs.uniforms,
                Stage::Coordinate => g.cs.uniforms,
            };
            assert!(write_uniforms(list, &draws[d].push, &viewport, words), "{}: uniforms", name);
            return;
        }
        let s = match stage {
            Stage::Fragment => 0,
            Stage::Vertex => 1,
            Stage::Coordinate => 2,
        };
        let list = &r.pipelines[draws[d].pipeline].2[s];
        assert_eq!(list.len(), words.len());
        for (w, &(contents, data)) in words.iter_mut().zip(list) {
            *w = match contents {
                0 => data,
                1 => draws[d].push[data as usize],
                2 => (scale[0] * CLIPPER_XY_GRANULARITY).to_bits(),
                3 => (scale[1] * CLIPPER_XY_GRANULARITY).to_bits(),
                4 => translate[2].to_bits(),
                5 => scale[2].to_bits(),
                c => panic!("{}: uniform contents {}", name, c),
            };
        }
    };
    let job = record(&pass, &mut alloc, &mut fill).unwrap_or_else(|_| panic!("{}: out of memory", name));
    for (k, q) in &alloc.queues {
        assert!(q.is_empty(), "{}: {} buffers v3dv had are left", name, k);
    }

    for (what, got) in [("bcl", &job.bcl), ("rcl", &job.rcl), ("indirect", &job.indirect)] {
        let want: Vec<_> = r.bos.iter().filter(|b| b.0 == what).collect();
        assert_eq!(got.len(), want.len(), "{}: {} buffers", name, what);
        for (g, w) in got.iter().zip(want) {
            assert_eq!(g.va, w.1, "{}: {} address", name, what);
            if g.bytes != w.3 {
                let at = g.bytes.iter().zip(&w.3).position(|(a, b)| a != b).unwrap();
                let lo = at.saturating_sub(8);
                panic!("{}: {} buffer {:#x} differs at {:#x}: got {:02x?}, v3dv {:02x?}", name, what, g.va, at,
                    &g.bytes[lo..(at + 24).min(g.bytes.len())], &w.3[lo..(at + 24).min(w.3.len())]);
            }
        }
    }
    let want = SubmitCl {
        bcl_start: key(&r.keys, "bcl_start"),
        bcl_end: key(&r.keys, "bcl_end"),
        rcl_start: key(&r.keys, "rcl_start"),
        rcl_end: key(&r.keys, "rcl_end"),
        qma: key(&r.keys, "qma"),
        qms: key(&r.keys, "qms"),
        qts: key(&r.keys, "qts"),
        flush_cache: flag(&r.keys, "flush_cache"),
    };
    assert_eq!(job.submit, want, "{}: submit", name);
    assert_eq!(job.bin_job().end, want.bcl_end);
    assert_eq!(job.render_list(), (want.rcl_start, want.rcl_end));
}

#[test]
fn every_render_pass_is_recorded_byte_for_byte_as_v3dv_records_it() {
    for (name, text) in SCENARIOS {
        check(name, text, false);
    }
}

#[test]
fn the_generated_programs_and_their_uniforms_record_what_v3dv_records() {
    for (name, text) in SCENARIOS {
        check(name, text, true);
    }
}

#[test]
fn the_scenarios_cover_what_they_are_named_for() {
    let rs: BTreeMap<&str, Recorded> = SCENARIOS.iter().map(|&(n, t)| (n, parse(t))).collect();
    let count = |n: &str, k: &str| rs[n].bos.iter().filter(|b| b.0 == k).count();
    // The binning list chains on by BRANCH, the indirect state grows.
    assert!(count("many_draws_640x480", "bcl") > 1);
    assert!(count("many_draws_640x480", "indirect") > 2);
    // More than 256 tiles: supertiles bigger than a tile.
    assert!(num(&rs["three_3840x2160"].keys["size"][0]) / 64 * (num(&rs["three_3840x2160"].keys["size"][1]) / 64) > 256);
    assert_eq!(key(&rs["rev10_256x128"].keys, "rev"), 10);
    assert_ne!(rs["sub_area_256x128"].keys["area"][0], "0");
}
