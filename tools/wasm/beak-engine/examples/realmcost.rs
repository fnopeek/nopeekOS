// What a realm costs, and whether dropping it frees it.
//
// A JS realm is cyclic and `Rc` never reaches zero inside a cycle; that is
// what `Interp::teardown` is for. Anyone adding to the prototypes or the
// global object should re-check the third line here.
//
//   cargo run --release --example realmcost      (N=<count> for more runs)
//
// The first line is the cost of a held realm; the second says nothing about
// a leak (the allocator does not return memory to the system); the third is
// the actual question: does the (n+1)-th realm still cost anything?
fn rss_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    for l in s.lines() {
        if let Some(r) = l.strip_prefix("VmRSS:") {
            return r.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    0
}
fn main() {
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let base = rss_kb();
    {
        let mut v = Vec::new();
        for _ in 0..n { v.push(beak_engine::js::interp::Interp::new()); }
        println!("{n} Interps GEHALTEN: +{} KB  = {} KB je Realm", rss_kb() - base, (rss_kb() - base) / n as u64);
    }
    println!("nach dem Fallenlassen:     +{} KB", rss_kb() - base);
    let b2 = rss_kb();
    for _ in 0..n { let _ = beak_engine::js::interp::Interp::new(); }
    println!("{n} einzeln erzeugt+fallen: +{} KB", rss_kb() - b2);
}
