fn main() {
    let root = std::env::var("JSCORPUS").unwrap();
    let page = std::env::args().nth(1).unwrap();
    let dir = std::path::Path::new(&root).join(&page);
    let mut fs_: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|x| x.path())
        .filter(|p| p.extension().is_some_and(|x| x == "js")).collect();
    fs_.sort_by_key(|p| p.file_stem().and_then(|s| s.to_str())
        .and_then(|s| s.rsplit("__").next().map(|n| n.parse::<u32>().unwrap_or(0))).unwrap_or(0));
    let mut sess = beak_engine::js::Session::new(2_000_000);
    let hp = std::path::Path::new(&root).parent().unwrap().join("html").join(format!("{page}.html"));
    if let Ok(html) = std::fs::read_to_string(&hp) {
        let dom = beak_engine::dom::parse(&html);
        sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
    }
    let mut shown = 0;
    for f in &fs_ {
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let prog = beak_engine::js::parse(&src, false)
                .or_else(|_| beak_engine::js::parse(&src, true))
                .map_err(|e| format!("SyntaxError: {}", e.msg))?;
            sess.run(&prog)
        }));
        if let Ok(Ok(())) = r { continue }
        let why = match r { Err(_) => "Absturz".to_string(), Ok(Err(e)) => e, _ => unreachable!() };
        println!("{}  ({} B)\n   -> {why}", f.file_name().unwrap().to_string_lossy(), src.len());
        shown += 1;
        if shown >= 4 { return }
    }
}
