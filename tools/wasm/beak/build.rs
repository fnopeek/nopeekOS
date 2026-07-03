//! Emit beak's AppMeta as a binary blob under $OUT_DIR, linked into the WASM
//! `.npk.app_meta` custom section by the main crate.

use nopeek_widgets::app_meta::{encode, AppMeta, IconRef};
use nopeek_widgets::IconId;

fn main() {
    let meta = AppMeta {
        display_name: "beak".into(),
        description: "Native web browser".into(),
        // Globe until a dedicated Bird glyph is added to the icon set.
        icon: IconRef::Builtin(IconId::Globe),
    };
    let bytes = encode(&meta).expect("encode AppMeta");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let path = std::path::Path::new(&out_dir).join("app_meta.bin");
    std::fs::write(&path, &bytes).expect("write app_meta.bin");
    println!("cargo:rerun-if-changed=build.rs");
}
