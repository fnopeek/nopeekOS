//! The fbench workloads as native x86-64 code, the baseline forge is
//! measured against: `cargo run --release --example native
//! --target x86_64-unknown-linux-gnu`.

extern crate alloc;

#[path = "../src/work.rs"]
mod work;

fn main() {
    println!("[fbench] {} native", env!("CARGO_PKG_VERSION"));
    let mut total = 0.0;
    for w in work::ALL {
        let t0 = std::time::Instant::now();
        let sum = (w.run)();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        total += ms;
        println!("{:<10} {:>10.1} ms  ({:016x})", w.name, ms, sum);
    }
    println!("total      {:>10.1} ms", total);
}
