use nopeek_widgets::app_meta::{encode, AppMeta, IconRef};
use nopeek_widgets::IconId;

fn main() {
    let meta = AppMeta {
        display_name: "GPU Intel TGL".into(),
        description:  "Intel Tiger Lake graphics (i915 port)".into(),
        icon:         IconRef::Builtin(IconId::List),
    };
    let bytes = encode(&meta).expect("encode AppMeta");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let path = std::path::Path::new(&out_dir).join("app_meta.bin");
    std::fs::write(&path, &bytes).expect("write app_meta.bin");
    // intel_gt (engines with their contexts) and i915 live on the stack.
    println!("cargo:rustc-link-arg=-zstack-size=4194304");
    println!("cargo:rerun-if-changed=build.rs");
}
