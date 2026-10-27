//! regen-icons — rasterize Phosphor SVGs into a nopeekOS icon atlas.
//!
//! Reads `icons/phosphor/*.svg` (relative to repo root), rasterizes
//! each via resvg at 5 sizes (16/24/32/48/64 actual px, alpha only),
//! packs into a binary blob at `release/assets/phosphor.atlas`. That
//! blob then ships with the installer and lands at
//! `sys/icons/phosphor` in npkFS.
//!
//! Atlas format (little-endian throughout):
//!
//! ```text
//! 0x00  magic      : [u8; 8] = b"NPKIATLS"
//! 0x08  version    : u16 = 1
//! 0x0A  num_icons  : u16
//! 0x0C  num_sizes  : u16
//! 0x0E  _pad       : u16 = 0
//! 0x10  sizes      : [u16; num_sizes]            — px edge length
//! ....  index      : [IconEntry; num_icons]
//!         IconEntry { icon_id: u16, _pad: u16, offsets: [u32; num_sizes] }
//! ....  data       : alpha bytes, per-icon per-size, concatenated
//!                    size S contributes exactly S*S bytes
//! ....  outlines   : (version 2) b"NPKIPATH", count: u16, _pad: u16, then
//!                    per icon { icon_id: u16, contours: u16,
//!                    per contour { points: u16, [x: i16, y: i16; points] } }
//! ```
//!
//! The outlines let the kernel rasterize an icon at any size:
//! every stroke is turned into its fill
//! outline here, curves are flattened, and a point is in 1/64 units of the
//! 256 grid, so the kernel only fills closed polygons by the nonzero rule.
//!
//! `IconId` values match the frozen numbers in
//! `kernel/src/shade/widgets/abi.rs` — see the `ICONS` table below.
//! Appending a new IconId = add a row, regen, commit atlas + kernel
//! ABI extension in the same release.
//!
//! Run from repo root: `cargo run --release --manifest-path tools/regen-icons/Cargo.toml`
//! Output: `release/assets/phosphor.atlas` (regenerate + commit).

use std::fs;
use std::path::{Path, PathBuf};

/// One icon in the atlas — which SVG filename supplies its shape,
/// which IconId discriminant it corresponds to in the kernel ABI.
struct IconEntry {
    id:  u16,
    svg: &'static str,
    /// Label for logging only.
    name: &'static str,
}

/// The icon set. Order matches the kernel's IconId numbering; new icons
/// must be appended, not inserted.
const ICONS: &[IconEntry] = &[
    IconEntry { id: 1,  svg: "folder.svg",              name: "Folder" },
    IconEntry { id: 2,  svg: "file.svg",                name: "File" },
    IconEntry { id: 3,  svg: "arrow-left.svg",          name: "ArrowLeft" },
    IconEntry { id: 4,  svg: "arrow-right.svg",         name: "ArrowRight" },
    IconEntry { id: 5,  svg: "arrow-up.svg",            name: "ArrowUp" },
    IconEntry { id: 6,  svg: "arrow-down.svg",          name: "ArrowDown" },
    IconEntry { id: 7,  svg: "house.svg",               name: "Home" },
    IconEntry { id: 8,  svg: "download-simple.svg",     name: "Download" },
    IconEntry { id: 9,  svg: "magnifying-glass.svg",    name: "MagnifyingGlass" },
    IconEntry { id: 10, svg: "x.svg",                   name: "X" },
    IconEntry { id: 11, svg: "check.svg",               name: "Check" },
    IconEntry { id: 12, svg: "gear.svg",                name: "Gear" },
    IconEntry { id: 13, svg: "power.svg",               name: "Power" },
    IconEntry { id: 14, svg: "lock-simple.svg",         name: "Lock" },
    IconEntry { id: 15, svg: "terminal.svg",            name: "Terminal" },
    IconEntry { id: 16, svg: "trash.svg",               name: "Trash" },
    IconEntry { id: 17, svg: "dots-three-vertical.svg", name: "DotsThreeVertical" },
    IconEntry { id: 18, svg: "list.svg",                name: "List" },
    IconEntry { id: 19, svg: "monitor.svg",             name: "Monitor" },
    IconEntry { id: 20, svg: "file-text.svg",           name: "FileText" },
    IconEntry { id: 21, svg: "folder-open.svg",         name: "FolderOpen" },
    IconEntry { id: 22, svg: "image.svg",               name: "Image" },
    IconEntry { id: 23, svg: "hard-drives.svg",         name: "HardDrives" },
    IconEntry { id: 24, svg: "code.svg",                name: "Code" },
    IconEntry { id: 25, svg: "folders.svg",             name: "Folders" },
    IconEntry { id: 26, svg: "caret-right.svg",         name: "CaretRight" },
    IconEntry { id: 27, svg: "arrow-clockwise.svg",     name: "ArrowClockwise" },
    IconEntry { id: 28, svg: "globe.svg",                name: "Globe" },
    IconEntry { id: 29, svg: "camera.svg",              name: "Camera" },
    IconEntry { id: 30, svg: "battery-empty.svg",       name: "BatteryEmpty" },
    IconEntry { id: 31, svg: "battery-low.svg",         name: "BatteryLow" },
    IconEntry { id: 32, svg: "battery-medium.svg",      name: "BatteryMedium" },
    IconEntry { id: 33, svg: "battery-high.svg",        name: "BatteryHigh" },
    IconEntry { id: 34, svg: "battery-full.svg",        name: "BatteryFull" },
    IconEntry { id: 35, svg: "battery-charging.svg",    name: "BatteryCharging" },
    IconEntry { id: 36, svg: "battery-warning.svg",     name: "BatteryWarning" },
    IconEntry { id: 37, svg: "plug.svg",                name: "Plug" },
    IconEntry { id: 38, svg: "speaker-high.svg",        name: "SpeakerHigh" },
    IconEntry { id: 39, svg: "speaker-low.svg",         name: "SpeakerLow" },
    IconEntry { id: 40, svg: "speaker-x.svg",           name: "SpeakerX" },
    IconEntry { id: 41, svg: "minus.svg",               name: "Minus" },
    IconEntry { id: 42, svg: "plus.svg",                name: "Plus" },
    IconEntry { id: 43, svg: "bird.svg",                name: "Bird" },
    IconEntry { id: 44, svg: "music-notes.svg",         name: "MusicNotes" },
    IconEntry { id: 45, svg: "play.svg",                name: "Play" },
    IconEntry { id: 46, svg: "pause.svg",               name: "Pause" },
    IconEntry { id: 47, svg: "skip-back.svg",           name: "SkipBack" },
    IconEntry { id: 48, svg: "skip-forward.svg",        name: "SkipForward" },
    IconEntry { id: 49, svg: "file-audio.svg",          name: "FileAudio" },
    IconEntry { id: 50, svg: "arrow-counter-clockwise.svg", name: "ArrowCounterClockwise" },
    IconEntry { id: 51, svg: "play-circle.svg",         name: "PlayCircle" },
    IconEntry { id: 52, svg: "fire.svg",                name: "Fire" },
    IconEntry { id: 53, svg: "squares-four.svg",        name: "SquaresFour" },
    IconEntry { id: 54, svg: "sliders-horizontal.svg",  name: "SlidersHorizontal" },
    IconEntry { id: 55, svg: "wifi-high.svg",         name: "WifiHigh" },
    IconEntry { id: 56, svg: "wifi-medium.svg",       name: "WifiMedium" },
    IconEntry { id: 57, svg: "wifi-low.svg",          name: "WifiLow" },
    IconEntry { id: 58, svg: "wifi-none.svg",         name: "WifiNone" },
    IconEntry { id: 59, svg: "wifi-slash.svg",        name: "WifiSlash" },
    IconEntry { id: 60, svg: "network.svg",           name: "Network" },
];

/// Rasterized sizes, in actual pixels (not HiDPI-scaled).
const SIZES: &[u16] = &[16, 24, 32, 48, 64];

const ATLAS_VERSION: u16 = 2;
const MAGIC: &[u8; 8] = b"NPKIATLS";
const PATH_MAGIC: &[u8; 8] = b"NPKIPATH";
/// Outline coordinate units per pixel of the 256 grid.
const UNITS: f32 = 64.0;
/// Longest chord a flattened curve may have, in grid px. At 64 px an icon
/// is 1/4 of the grid, so a chord's sagitta stays far below a pixel.
const CHORD: f32 = 1.5;

type Contour = Vec<(f32, f32)>;

/// Fill outlines of `svg_path` in 256-grid px: fills as they are, strokes
/// as the outline tiny-skia strokes them to, every curve flattened.
fn outline(svg_path: &Path) -> Vec<Contour> {
    let svg_data = fs::read(svg_path).expect("read svg");
    let tree = usvg::Tree::from_data(&svg_data, &usvg::Options::default())
        .expect("parse svg");
    let size = tree.size();
    let fit = 256.0 / size.width().max(size.height());
    let mut out = Vec::new();
    walk(tree.root(), fit, &mut out);
    out
}

fn walk(group: &usvg::Group, fit: f32, out: &mut Vec<Contour>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, fit, out),
            usvg::Node::Path(p) if p.is_visible() => {
                let ts = p.abs_transform().post_scale(fit, fit);
                if p.fill().is_some() {
                    if let Some(d) = p.data().clone().transform(ts) { flatten(&d, out); }
                }
                if let Some(st) = p.stroke() {
                    // Stroked in the path's own space, fine enough for the
                    // largest size the grid is drawn at.
                    let stroked = p.data().stroke(&st.to_tiny_skia(), 4.0);
                    if let Some(d) = stroked.and_then(|d| d.transform(ts)) { flatten(&d, out); }
                }
            }
            _ => {}
        }
    }
}

fn flatten(path: &tiny_skia::Path, out: &mut Vec<Contour>) {
    use tiny_skia::PathSegment as S;
    let mut cur: Contour = Vec::new();
    let mut last = (0.0f32, 0.0f32);
    let end = |cur: &mut Contour, out: &mut Vec<Contour>| {
        if cur.len() >= 3 { out.push(core::mem::take(cur)); } else { cur.clear(); }
    };
    for seg in path.segments() {
        match seg {
            S::MoveTo(p) => { end(&mut cur, out); last = (p.x, p.y); cur.push(last); }
            S::LineTo(p) => { last = (p.x, p.y); cur.push(last); }
            S::QuadTo(c, p) => {
                let (c, p) = ((c.x, c.y), (p.x, p.y));
                let n = steps(&[last, c, p]);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push((u * u * last.0 + 2.0 * u * t * c.0 + t * t * p.0,
                              u * u * last.1 + 2.0 * u * t * c.1 + t * t * p.1));
                }
                last = p;
            }
            S::CubicTo(c1, c2, p) => {
                let (c1, c2, p) = ((c1.x, c1.y), (c2.x, c2.y), (p.x, p.y));
                let n = steps(&[last, c1, c2, p]);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    cur.push((a * last.0 + b * c1.0 + c * c2.0 + d * p.0,
                              a * last.1 + b * c1.1 + c * c2.1 + d * p.1));
                }
                last = p;
            }
            S::Close => end(&mut cur, out),
        }
    }
    end(&mut cur, out);
}

/// Segments for a curve: its control polygon's length in chords.
fn steps(pts: &[(f32, f32)]) -> usize {
    let len: f32 = pts.windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .sum();
    ((len / CHORD).ceil() as usize).clamp(1, 256)
}

fn rasterize(svg_path: &Path, size: u16) -> Vec<u8> {
    let svg_data = fs::read(svg_path).expect("read svg");
    let tree = usvg::Tree::from_data(&svg_data, &usvg::Options::default())
        .expect("parse svg");

    // Fit SVG into target square, centred, preserving aspect ratio.
    let svg_size = tree.size();
    let scale = size as f32 / svg_size.width().max(svg_size.height());
    let rendered_w = (svg_size.width()  * scale).round() as u32;
    let rendered_h = (svg_size.height() * scale).round() as u32;
    let ox = ((size as u32).saturating_sub(rendered_w)) / 2;
    let oy = ((size as u32).saturating_sub(rendered_h)) / 2;

    let mut pixmap = tiny_skia::Pixmap::new(size as u32, size as u32).expect("pixmap");
    let transform = tiny_skia::Transform::from_scale(scale, scale)
        .post_translate(ox as f32, oy as f32);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Phosphor SVGs use `currentColor` (usually black); resvg paints
    // opaque black. We keep just the alpha channel and let the kernel
    // tint per theme token at composite time.
    pixmap.pixels().iter().map(|p| p.alpha()).collect()
}

fn main() {
    // Resolve paths relative to the repo root (cargo run CWD = repo root).
    let repo_root = PathBuf::from(std::env::current_dir().expect("cwd"));
    let svg_dir = repo_root.join("icons").join("phosphor");
    let out_dir = repo_root.join("release").join("assets");
    fs::create_dir_all(&out_dir).expect("mkdir assets");
    let out_path = out_dir.join("phosphor.atlas");

    println!("regen-icons: {} icons × {} sizes", ICONS.len(), SIZES.len());

    // Pass 1: rasterize every (icon, size) tuple, stash alpha bytes.
    let mut alpha_per_icon_size: Vec<Vec<Vec<u8>>> = Vec::with_capacity(ICONS.len());
    for icon in ICONS {
        let path = svg_dir.join(icon.svg);
        if !path.exists() {
            panic!("missing SVG: {}", path.display());
        }
        let mut sizes = Vec::with_capacity(SIZES.len());
        for &sz in SIZES {
            let alpha = rasterize(&path, sz);
            assert_eq!(alpha.len(), (sz as usize) * (sz as usize));
            sizes.push(alpha);
        }
        alpha_per_icon_size.push(sizes);
        println!("  {:<22} id={}", icon.name, icon.id);
    }

    // Pass 2: layout the atlas. Header + sizes + index + data.
    let num_icons = ICONS.len() as u16;
    let num_sizes = SIZES.len() as u16;

    let header_len   = 8 + 2 + 2 + 2 + 2;                // magic + version + num_icons + num_sizes + pad
    let sizes_len    = SIZES.len() * 2;
    let entry_len    = 2 + 2 + SIZES.len() * 4;          // id + pad + offsets
    let index_len    = ICONS.len() * entry_len;
    let data_start   = header_len + sizes_len + index_len;

    let mut blob: Vec<u8> = Vec::with_capacity(data_start + 200_000);

    // Header
    blob.extend_from_slice(MAGIC);
    blob.extend_from_slice(&ATLAS_VERSION.to_le_bytes());
    blob.extend_from_slice(&num_icons.to_le_bytes());
    blob.extend_from_slice(&num_sizes.to_le_bytes());
    blob.extend_from_slice(&0u16.to_le_bytes());

    // Sizes
    for &sz in SIZES { blob.extend_from_slice(&sz.to_le_bytes()); }

    // Index — written after we know per-(icon, size) offsets. Reserve
    // space first, then back-patch.
    let index_offset = blob.len();
    blob.resize(blob.len() + index_len, 0);

    // Data — concatenate, track offsets.
    let mut data_cursor: u32 = data_start as u32;
    let mut data_bytes: Vec<u8> = Vec::with_capacity(200_000);
    let mut offsets_per_icon: Vec<Vec<u32>> = Vec::with_capacity(ICONS.len());

    for alpha_sizes in &alpha_per_icon_size {
        let mut offsets = Vec::with_capacity(SIZES.len());
        for alpha in alpha_sizes {
            offsets.push(data_cursor);
            data_bytes.extend_from_slice(alpha);
            data_cursor += alpha.len() as u32;
        }
        offsets_per_icon.push(offsets);
    }

    // Back-patch index.
    for (i, icon) in ICONS.iter().enumerate() {
        let base = index_offset + i * entry_len;
        blob[base..base + 2].copy_from_slice(&icon.id.to_le_bytes());
        blob[base + 2..base + 4].copy_from_slice(&0u16.to_le_bytes());
        for (j, &off) in offsets_per_icon[i].iter().enumerate() {
            let at = base + 4 + j * 4;
            blob[at..at + 4].copy_from_slice(&off.to_le_bytes());
        }
    }

    // Append data.
    blob.extend_from_slice(&data_bytes);

    // Outlines (version 2).
    blob.extend_from_slice(PATH_MAGIC);
    blob.extend_from_slice(&num_icons.to_le_bytes());
    blob.extend_from_slice(&0u16.to_le_bytes());
    let mut points = 0usize;
    for icon in ICONS {
        let contours = outline(&svg_dir.join(icon.svg));
        blob.extend_from_slice(&icon.id.to_le_bytes());
        blob.extend_from_slice(&(contours.len() as u16).to_le_bytes());
        for c in &contours {
            blob.extend_from_slice(&(c.len() as u16).to_le_bytes());
            for &(x, y) in c {
                for v in [x, y] {
                    let q = (v * UNITS).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                    blob.extend_from_slice(&q.to_le_bytes());
                }
            }
            points += c.len();
        }
    }
    println!("regen-icons: outlines, {} points", points);

    fs::write(&out_path, &blob).expect("write atlas");
    println!("regen-icons: wrote {} bytes → {}", blob.len(), out_path.display());
}
