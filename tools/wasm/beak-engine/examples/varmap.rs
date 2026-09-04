// Worauf loest ein Name im ECHTEN Bootstrap-Blatt auf?
fn main() {
    let css = include_str!("../assets/bootstrap.min.css");
    let out = beak_engine::vars::resolve_vars(css, beak_engine::css::Media::new(1902.0, false), &[]);
    for name in ["body{margin:0", "var(--bs-body-bg)", "background-color:#fff", "background-color:#212529"] {
        println!("\n── {name} ──");
        let mut from = 0;
        let mut n = 0;
        while let Some(i) = out[from..].find(name) {
            let a = from + i;
            let b = (a + 420).min(out.len());
            println!("   …{}…", &out[a.saturating_sub(40)..b].replace('\n', " "));
            from = a + name.len();
            n += 1;
            if n >= 3 { break }
        }
    }
}
