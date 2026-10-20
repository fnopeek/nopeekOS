// The module's command stream against RADV's: radv_stream.rs rebuilds the
// queue preamble IB and the command buffer acoc recorded (shaders_gen.rs
// RADV_RECORDED_*) from the templates, with RADV's own values in the holes,
// and every word must match, padding included.

#![allow(dead_code, non_camel_case_types, non_upper_case_globals)]

extern crate alloc;

#[path = "../src/shaders_gen.rs"]
mod shaders_gen;
#[path = "../src/radv_stream.rs"]
mod radv_stream;

use radv_stream as rs;
use shaders_gen as sg;

/// The push-pointer word RADV wrote into `tpl`, for kernel `k`.
fn recorded_ptr(tpl: &[(u32, sg::Hole)], k: &sg::Kernel) -> u32 {
    let Some(n) = k.push_ptr_sgpr else { return 0 };
    let want = match n {
        2 => sg::Hole::UserData2,
        3 => sg::Hole::UserData3,
        _ => panic!("push pointer in user SGPR {n}"),
    };
    tpl.iter().find(|e| e.1 == want).map_or(0, |e| e.0)
}

fn compare(what: &str, mine: &[u32], radv: &[u32]) -> bool {
    if mine == radv {
        println!("{what}: module = RADV ({} dwords)", radv.len());
        return true;
    }
    println!("{what}: module {} dwords, RADV {}", mine.len(), radv.len());
    for i in 0..mine.len().max(radv.len()) {
        let (a, b) = (mine.get(i).copied(), radv.get(i).copied());
        if a != b {
            println!("  dword {i}: module {a:08x?}, RADV {b:08x?}");
        }
    }
    false
}

fn main() {
    let mut ok = true;
    let mut pre = Vec::new();
    rs::preamble(&mut pre);
    ok &= compare("preamble IB", &pre, sg::RADV_RECORDED_PREAMBLE);

    let k = rs::KERNELS.iter().position(|k| {
        core::ptr::eq(k.code.as_ptr(), match sg::RADV_RECORDED_KERNEL {
            "blur_box" => sg::BLUR_BOX.code.as_ptr(),
            "blur_down" => sg::BLUR_DOWN.code.as_ptr(),
            "compose" => sg::COMPOSE.code.as_ptr(),
            _ => sg::FILL_PATTERN.code.as_ptr(),
        })
    }).expect("recorded kernel");
    let kernel = rs::KERNELS[k];
    let push: Vec<u32> = (0..kernel.push_bytes / 4).map(|i| sg::PUSH_TAG | i).collect();
    let mut cmd = Vec::new();
    let mut prev = None;
    // dispatch; the same with a barrier; new push constants; another pipeline
    for step in 0..4 {
        let tpl = match step {
            1 => sg::RADV_BARRIER_DISPATCH,
            3 => rs::template_for(k, Some(usize::MAX)),
            _ => rs::template_for(k, prev),
        };
        let d = rs::dispatch {
            kernel,
            code_va: sg::RADV_RECORDED_SHADER_VA,
            push: &push,
            push_ptr: recorded_ptr(tpl, kernel),
            grid: sg::RADV_GRID,
        };
        rs::emit(&mut cmd, tpl, &d);
        prev = Some(k);
    }
    rs::end(&mut cmd);
    ok &= compare("command buffer", &cmd, sg::RADV_RECORDED_CMDBUF);
    std::process::exit(if ok { 0 } else { 1 });
}
