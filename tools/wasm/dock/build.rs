//! Emit the dock's AppMeta blob under $OUT_DIR for the `.npk.app_meta`
//! custom section. (The dock excludes itself from its own catalog, so
//! this mainly identifies it to other launchers.)

use nopeek_widgets::app_meta::{encode, AppMeta, IconRef};
use nopeek_widgets::IconId;

fn main() {
    let meta = AppMeta {
        display_name: "Dock".into(),
        description:  "App dock".into(),
        icon:         IconRef::Builtin(IconId::Folders),
    };
    let bytes = encode(&meta).expect("encode AppMeta");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let path = std::path::Path::new(&out_dir).join("app_meta.bin");
    std::fs::write(&path, &bytes).expect("write app_meta.bin");
    println!("cargo:rerun-if-changed=build.rs");
}
