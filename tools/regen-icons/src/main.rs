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
//! ```
//!
//! `IconId` values match the frozen enum in
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

/// The v1 icon set. Order matches the kernel's IconId enum; new
/// icons must be appended, not inserted.
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
];

/// Rasterized sizes, in actual pixels (not HiDPI-scaled).
const SIZES: &[u16] = &[16, 24, 32, 48, 64];

const ATLAS_VERSION: u16 = 1;
const MAGIC: &[u8; 8] = b"NPKIATLS";

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

    fs::write(&out_path, &blob).expect("write atlas");
    println!("regen-icons: wrote {} bytes → {}", blob.len(), out_path.display());
}
