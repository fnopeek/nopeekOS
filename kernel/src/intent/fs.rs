//! Filesystem intents: store, fetch, cat, grep, head, wc, hexdump, delete, mkdir, rmdir, list, fsinfo, disk

use crate::{kprint, kprintln, capability};
use crate::capability::CapId;
use super::resolve_path;

/// Helper: fetch an npkFS object and return its data.
/// Name is resolved relative to cwd.
pub(super) fn fetch_object(name: &str) -> Option<alloc::vec::Vec<u8>> {
    let path = resolve_path(name);
    match crate::npkfs::fetch(&path) {
        Ok((data, _)) => Some(data),
        Err(e) => { kprintln!("[npk] '{}': {}", name, e); None }
    }
}

/// Parse "args > target" redirect syntax. Returns (args, Option<store_name>).
pub(super) fn parse_redirect(args: &str) -> (&str, Option<&str>) {
    if let Some(idx) = args.rfind('>') {
        let target = args[idx + 1..].trim();
        let rest = args[..idx].trim();
        if target.is_empty() { (args, None) } else { (rest, Some(target)) }
    } else {
        (args, None)
    }
}

pub fn intent_store(args: &str, cap_id: CapId) {
    let mut parts = args.splitn(2, ' ');
    let name = match parts.next() {
        Some(n) if !n.is_empty() => n,
        _ => { kprintln!("[npk] Usage: store <name> <data>"); return; }
    };
    let data = parts.next().unwrap_or("");
    if data.is_empty() {
        kprintln!("[npk] Usage: store <name> <data>");
        return;
    }

    let path = resolve_path(name);
    if let Err(e) = crate::npkfs::validate_user_name(&path) {
        kprintln!("[npk] Store error: {}", e);
        return;
    }
    // Auto-create parent directories
    if let Some(idx) = path.rfind('/') {
        super::ensure_parents(&path[..idx]);
    }
    match crate::npkfs::upsert(&path, data.as_bytes(), cap_id) {
        Ok(hash) => {
            kprint!("[npk] Stored '{}' ({} bytes, hash: ", path, data.len());
            for b in &hash[..4] { kprint!("{:02x}", b); }
            kprintln!("...)");
        }
        Err(e) => kprintln!("[npk] Store error: {}", e),
    }
}

pub fn intent_fetch(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: fetch <name>");
        return;
    }
    let path = resolve_path(name);

    match crate::npkfs::fetch(&path) {
        Ok((data, _hash)) => {
            match core::str::from_utf8(&data) {
                Ok(s) => kprintln!("{}", s),
                Err(_) => {
                    kprintln!("[npk] ({} bytes, binary)", data.len());
                }
            }
        }
        Err(e) => kprintln!("[npk] Fetch error: {}", e),
    }
}

pub fn intent_cat(args: &str) {
    let (args, redirect) = parse_redirect(args);
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: cat <name> [> target]");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    if let Some(target) = redirect {
        let target_path = resolve_path(target);
        if let Err(e) = crate::npkfs::validate_user_name(&target_path) {
            kprintln!("[npk] Store error: {}", e);
            return;
        }
        match crate::npkfs::upsert(&target_path, &data, capability::CAP_NULL) {
            Ok(_) => kprintln!("[npk] Copied -> '{}' ({} bytes)", target_path, data.len()),
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
        return;
    }

    match core::str::from_utf8(&data) {
        Ok(text) => kprintln!("{}", text),
        Err(_) => kprintln!("[npk] ({} bytes, binary — use 'hexdump {}')", data.len(), name),
    }
}

pub fn intent_grep(args: &str) {
    let (args, redirect) = parse_redirect(args);
    let args = args.trim();

    let (pattern, name) = match args.split_once(' ') {
        Some((p, n)) => (p.trim(), n.trim()),
        None => {
            kprintln!("[npk] Usage: grep <pattern> <name> [> target]");
            return;
        }
    };

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    let text = match core::str::from_utf8(&data) {
        Ok(t) => t,
        Err(_) => { kprintln!("[npk] '{}' is binary", name); return; }
    };

    let pattern_lower = alloc::string::String::from(pattern).to_ascii_lowercase();
    let mut matches = alloc::vec::Vec::new();
    let mut match_count = 0u32;

    for (i, line) in text.lines().enumerate() {
        let line_lower = alloc::string::String::from(line).to_ascii_lowercase();
        if line_lower.contains(pattern_lower.as_str()) {
            match_count += 1;
            if redirect.is_some() {
                matches.extend_from_slice(line.as_bytes());
                matches.push(b'\n');
            } else {
                kprintln!("  {:4}: {}", i + 1, line);
            }
        }
    }

    if let Some(target) = redirect {
        let target_path = resolve_path(target);
        match crate::npkfs::upsert(&target_path, &matches, capability::CAP_NULL) {
            Ok(_) => kprintln!("[npk] {} matches -> '{}'", match_count, target_path),
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
    } else if match_count == 0 {
        kprintln!("[npk] No matches for '{}' in '{}'", pattern, name);
    } else {
        kprintln!("[npk] {} matches", match_count);
    }
}

pub fn intent_head(args: &str) {
    let args = args.trim();
    let (name, count) = if let Some((n, c)) = args.rsplit_once(' ') {
        match c.parse::<usize>() {
            Ok(num) => (n.trim(), num),
            Err(_) => (args, 10),
        }
    } else {
        (args, 10)
    };

    if name.is_empty() {
        kprintln!("[npk] Usage: head <name> [lines]");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    match core::str::from_utf8(&data) {
        Ok(text) => {
            for line in text.lines().take(count) {
                kprintln!("{}", line);
            }
        }
        Err(_) => kprintln!("[npk] '{}' is binary", name),
    }
}

pub fn intent_wc(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: wc <name>");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    let bytes = data.len();
    match core::str::from_utf8(&data) {
        Ok(text) => {
            let lines = text.lines().count();
            let words = text.split_whitespace().count();
            kprintln!("  {} lines  {} words  {} bytes  {}", lines, words, bytes, name);
        }
        Err(_) => {
            kprintln!("  {} bytes (binary)  {}", bytes, name);
        }
    }
}

pub fn intent_hexdump(args: &str) {
    let args = args.trim();
    // Optional limit: hexdump name 128
    let (name, limit) = if let Some((n, l)) = args.rsplit_once(' ') {
        match l.parse::<usize>() {
            Ok(num) => (n.trim(), num),
            Err(_) => (args, 256),
        }
    } else {
        (args, 256)
    };

    if name.is_empty() {
        kprintln!("[npk] Usage: hexdump <name> [bytes]");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    let show = data.len().min(limit);
    for (i, chunk) in data[..show].chunks(16).enumerate() {
        kprint!("  {:04x}  ", i * 16);
        for (j, &b) in chunk.iter().enumerate() {
            kprint!("{:02x} ", b);
            if j == 7 { kprint!(" "); }
        }
        // Padding for short lines
        for j in chunk.len()..16 {
            kprint!("   ");
            if j == 7 { kprint!(" "); }
        }
        kprint!(" |");
        for &b in chunk {
            if b >= 0x20 && b <= 0x7E {
                kprint!("{}", b as char);
            } else {
                kprint!(".");
            }
        }
        kprintln!("|");
    }

    if show < data.len() {
        kprintln!("[npk] ({} of {} bytes shown)", show, data.len());
    }
}

pub fn intent_delete(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: delete <name>");
        return;
    }
    let path = resolve_path(name);

    match crate::npkfs::delete(&path) {
        Ok(()) => kprintln!("[npk] Deleted '{}'", path),
        Err(e) => kprintln!("[npk] Delete error: {}", e),
    }
}

pub fn intent_mkdir(args: &str) {
    let dir = args.trim().trim_end_matches('/');
    if dir.is_empty() {
        kprintln!("[npk] Usage: mkdir <path>");
        return;
    }
    let resolved = resolve_path(dir);
    match crate::npkfs::fs::ensure_dirs(&resolved) {
        Ok(()) => kprintln!("[npk] Created '{}'", resolved),
        Err(e) => kprintln!("[npk] mkdir error: {:?}", e),
    }
}

pub fn intent_rmdir(args: &str) {
    let dir = args.trim().trim_end_matches('/');
    if dir.is_empty() || dir == "." {
        kprintln!("[npk] Usage: rmdir <path>");
        return;
    }
    let resolved = resolve_path(dir);
    let cwd = super::get_cwd();
    if resolved == cwd || cwd.starts_with(&alloc::format!("{}/", resolved)) {
        kprintln!("[npk] Cannot remove current working directory");
        return;
    }

    use crate::npkfs::fs::Error as FsErr;
    match crate::npkfs::fs::delete(&resolved) {
        Ok(()) => kprintln!("[npk] Removed '{}'", resolved),
        Err(FsErr::NotEmpty) => kprintln!("[npk] Directory '{}' is not empty", resolved),
        Err(FsErr::NotFound) => kprintln!("[npk] Directory '{}' not found", resolved),
        Err(e) => kprintln!("[npk] rmdir error: {:?}", e),
    }
}

pub fn intent_list(args: &str) {
    let filter = args.trim();
    let resolved = if !filter.is_empty() {
        resolve_path(filter)
    } else {
        super::get_cwd()
    };

    use crate::npkfs::object::EntryKind;
    let entries = match crate::npkfs::fs::list(&resolved) {
        Ok(Some(v)) => v,
        Ok(None) => {
            kprintln!("[npk] '{}': not found", resolved);
            return;
        }
        Err(e) => {
            kprintln!("[npk] List error: {:?}", e);
            return;
        }
    };

    kprintln!();

    if entries.is_empty() {
        kprintln!("  (empty)");
    } else {
        // Directories first, then files (entries arrive sorted by name).
        for e in &entries {
            if e.kind == EntryKind::Dir {
                kprintln!("  {}/", e.name);
            }
        }
        for e in &entries {
            if e.kind != EntryKind::File { continue; }
            // Hide kernel-internal entries from user listings.
            if e.name.starts_with(".npk-") { continue; }
            kprint!("  {:<24} ", e.name);
            if e.size >= 1024 {
                kprint!("{:>6} KB  ", e.size / 1024);
            } else {
                kprint!("{:>6} B   ", e.size);
            }
            for b in &e.hash[..4] { kprint!("{:02x}", b); }
            kprintln!();
        }
    }

    if let Some((_, free, count, _)) = crate::npkfs::stats() {
        kprintln!();
        kprintln!("  {} objects, {} free blocks", count, free);
    }
    kprintln!();
}

pub fn intent_fsinfo() {
    match crate::npkfs::stats() {
        Some((total, free, objects, generation)) => {
            let total_mb = total * crate::npkfs::BLOCK_SIZE as u64 / (1024 * 1024);
            let free_mb = free * crate::npkfs::BLOCK_SIZE as u64 / (1024 * 1024);
            let used = total - free;
            kprintln!();
            kprintln!("  npkFS – Content-Addressed Object Store");
            kprintln!("  ──────────────────────────────────────");
            kprintln!("  Total:       {} blocks ({} MB)", total, total_mb);
            kprintln!("  Used:        {} blocks", used);
            kprintln!("  Free:        {} blocks ({} MB)", free, free_mb);
            kprintln!("  Objects:     {}", objects);
            kprintln!("  Generation:  {}", generation);
            kprintln!("  Hash:        BLAKE3");
            kprintln!("  TRIM:        {}", if crate::blkdev::has_discard() { "active" } else { "unavailable" });
            kprintln!();
        }
        None => kprintln!("[npk] Filesystem not mounted"),
    }
}

pub fn intent_disk_info() {
    use crate::blkdev;
    match blkdev::capacity() {
        Some(cap) => {
            let mb = (cap * 512) / (1024 * 1024);
            let blocks = blkdev::block_count().unwrap_or(0);
            let is_nvme = crate::nvme::is_available();
            let dev = if is_nvme { "NVMe" } else { "virtio-blk" };
            kprintln!();
            kprintln!("  Block Device ({})", dev);
            kprintln!("  ────────────────────────");
            if is_nvme {
                if let Some(model) = crate::nvme::model_name() {
                    kprintln!("  Model:     {}", model);
                }
                if let Some(sn) = crate::nvme::serial_number() {
                    kprintln!("  Serial:    {}", sn);
                }
            }
            kprintln!("  Capacity:  {} sectors / {} blocks ({} MB)", cap, blocks, mb);
            kprintln!("  Sector:    512 bytes");
            kprintln!("  Block:     4096 bytes");
            kprintln!("  TRIM:      {}", if blkdev::has_discard() { "supported" } else { "not available" });
            if is_nvme {
                let max_blocks = crate::nvme::max_blocks_per_cmd();
                kprintln!("  Max/cmd:   {} blocks ({} KB)", max_blocks, max_blocks * 4);
            }
            kprintln!("  Status:    online");
            print_crypto_bench();
            kprintln!();
        }
        None => kprintln!("[npk] No block device available"),
    }
}

/// Micro-bench BLAKE3 + AES-256-GCM on a 1 MB buffer to confirm the
/// HW backends (AVX2 / AES-NI) fire at runtime. Returns
/// `(blake3_mbs, aes_enc_mbs, aes_dec_mbs)`.
pub fn crypto_bench() -> (u64, u64, u64) {
    use crate::interrupts::{rdtsc, tsc_freq};

    const SIZE: usize = 1024 * 1024;
    const ITERS: usize = 4;

    let buf = alloc::vec![0xA5u8; SIZE];
    let hz = tsc_freq();
    if hz == 0 { return (0, 0, 0); }
    let bytes = (SIZE * ITERS) as u64;

    let t0 = rdtsc();
    let mut acc = [0u8; 32];
    for _ in 0..ITERS {
        let h = *blake3::hash(&buf).as_bytes();
        for i in 0..32 { acc[i] ^= h[i]; }
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let blake3_mbs = (bytes * hz) / (dt * 1024 * 1024);
    let _ = acc;

    let key = [0x42u8; 32];
    let nonce = [0x17u8; 12];
    let t0 = rdtsc();
    let mut total = 0usize;
    for _ in 0..ITERS {
        let ct = crate::crypto::aead_encrypt_aes(&key, &nonce, &buf);
        total = total.wrapping_add(ct.len());
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let aes_enc_mbs = (bytes * hz) / (dt * 1024 * 1024);

    let template = crate::crypto::aead_encrypt_aes(&key, &nonce, &buf);
    let mut clones: alloc::vec::Vec<alloc::vec::Vec<u8>> =
        alloc::vec::Vec::with_capacity(ITERS);
    for _ in 0..ITERS { clones.push(template.clone()); }

    let t0 = rdtsc();
    let mut ok = 0usize;
    for c in clones.iter_mut() {
        if crate::crypto::aead_decrypt_aes_in_place(&key, &nonce, c).is_some() {
            ok = ok.wrapping_add(1);
        }
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let aes_dec_mbs = (bytes * hz) / (dt * 1024 * 1024);
    let _ = (total, ok);

    // ── Custom AES-GCM (aead_hw.rs) ────────────────────────────────
    // Validate first: encrypt with both, compare ciphertext+tag.
    let ct_ref = crate::crypto::aead_encrypt_aes(&key, &nonce, &buf);
    let ct_hw  = crate::crypto::aead_encrypt_aes_hw(&key, &nonce, &buf);
    let bytes_match = ct_ref == ct_hw;
    if !bytes_match {
        kprintln!("  Crypto[hw]: COMPAT FAIL — ct_hw differs from aes-gcm crate");
    }

    // HW encrypt bench
    let t0 = rdtsc();
    let mut total_hw = 0usize;
    for _ in 0..ITERS {
        let ct = crate::crypto::aead_encrypt_aes_hw(&key, &nonce, &buf);
        total_hw = total_hw.wrapping_add(ct.len());
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let hw_enc_mbs = (bytes * hz) / (dt * 1024 * 1024);

    // HW decrypt-in-place bench
    let template_hw = crate::crypto::aead_encrypt_aes_hw(&key, &nonce, &buf);
    let mut clones_hw: alloc::vec::Vec<alloc::vec::Vec<u8>> =
        alloc::vec::Vec::with_capacity(ITERS);
    for _ in 0..ITERS { clones_hw.push(template_hw.clone()); }

    let t0 = rdtsc();
    let mut ok_hw = 0usize;
    for c in clones_hw.iter_mut() {
        if crate::crypto::aead_decrypt_aes_hw_in_place(&key, &nonce, c).is_some() {
            ok_hw = ok_hw.wrapping_add(1);
        }
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let hw_dec_mbs = (bytes * hz) / (dt * 1024 * 1024);
    let _ = (total_hw, ok_hw);

    if bytes_match {
        kprintln!("  Crypto[hw]: enc {} dec(in-place) {} MB/s | bytes match={}",
            hw_enc_mbs, hw_dec_mbs, bytes_match);
    }

    (blake3_mbs, aes_enc_mbs, aes_dec_mbs)
}

fn print_crypto_bench() {
    let (b, e, d) = crypto_bench();
    kprintln!("  Crypto:    BLAKE3 {} MB/s | AES-GCM enc {} dec(in-place) {} MB/s",
        b, e, d);
    print_cpu_features();
}

/// Print HW feature bits (raw CPUID) alongside the cpufeatures crate's
/// runtime view of the same features. A mismatch — HW reports
/// pclmulqdq but the crate sees `false` — explains why aes-gcm /
/// blake3 might pick a soft path in spite of `target-feature = +pclmulqdq`.
fn print_cpu_features() {
    use core::arch::x86_64::__cpuid;

    // CPUID(1) ECX bits we care about for crypto + SIMD.
    let cpuid1 = __cpuid(1);
    let ecx = cpuid1.ecx;
    let hw_pclmul = (ecx & (1 << 1)) != 0;
    let hw_aes    = (ecx & (1 << 25)) != 0;
    let hw_xsave  = (ecx & (1 << 26)) != 0;
    let hw_osxsave= (ecx & (1 << 27)) != 0;
    let hw_avx    = (ecx & (1 << 28)) != 0;

    // CPUID(7,0) — extended features. EBX bit 5 = AVX2.
    // ECX bit 10 = VPCLMULQDQ (VEX/EVEX-encoded carry-less multiply,
    // 256/512-bit parallel — Ice Lake / Zen 3+. polyval 0.7+ requires
    // this for its HW path; N100 (Atom-class) likely does NOT have it
    // even though plain PCLMULQDQ works.)
    let cpuid7 = __cpuid(7);
    let hw_avx2 = (cpuid7.ebx & (1 << 5)) != 0;
    let hw_vpclmul = (cpuid7.ecx & (1 << 10)) != 0;

    // What the cpufeatures crate's runtime detection actually says —
    // same primitives aes-gcm / blake3 see.
    cpufeatures::new!(crate_aes,         "aes");
    cpufeatures::new!(crate_pclmulqdq,   "pclmulqdq");
    cpufeatures::new!(crate_avx2,        "avx2");
    let crate_aes_v       = crate_aes::get();
    let crate_pclmulqdq_v = crate_pclmulqdq::get();
    let crate_avx2_v      = crate_avx2::get();

    kprintln!("  HW(cpuid): aes={} pclmul={} vpclmul={} avx={} avx2={} xsave={} osxsave={}",
        hw_aes as u8, hw_pclmul as u8, hw_vpclmul as u8,
        hw_avx as u8, hw_avx2 as u8,
        hw_xsave as u8, hw_osxsave as u8);
    kprintln!("  cpufeats:  aes={} pclmul={} avx2={}",
        crate_aes_v as u8, crate_pclmulqdq_v as u8, crate_avx2_v as u8);
}

pub fn intent_disk_read(args: &str) {
    let sector: u64 = match args.parse() {
        Ok(n) => n,
        Err(_) => { kprintln!("[npk] Usage: disk read <sector>"); return; }
    };

    let mut buf = [0u8; 512];
    match crate::blkdev::read_sector(sector, &mut buf) {
        Ok(()) => {
            kprintln!();
            kprintln!("  Sector {}:", sector);
            hex_dump(&buf);
            kprintln!();
        }
        Err(e) => kprintln!("[npk] Read error: {}", e),
    }
}

pub fn intent_disk_write(args: &str) {
    let mut parts = args.splitn(2, ' ');
    let sector: u64 = match parts.next().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => { kprintln!("[npk] Usage: disk write <sector> <text>"); return; }
    };
    let text = parts.next().unwrap_or("");
    if text.is_empty() {
        kprintln!("[npk] Usage: disk write <sector> <text>");
        return;
    }

    let mut buf = [0u8; 512];
    let len = text.len().min(512);
    buf[..len].copy_from_slice(&text.as_bytes()[..len]);

    match crate::blkdev::write_sector(sector, &buf) {
        Ok(()) => kprintln!("[npk] Wrote {} bytes to sector {}", len, sector),
        Err(e) => kprintln!("[npk] Write error: {}", e),
    }
}

fn hex_dump(buf: &[u8; 512]) {
    let mut prev = [0xFFu8; 16]; // impossible first value
    let mut collapsed = false;

    for row in 0..32 {
        let off = row * 16;
        let line = &buf[off..off + 16];

        if row > 0 && line == &prev[..] {
            if !collapsed {
                kprintln!("  *");
                collapsed = true;
            }
            continue;
        }
        collapsed = false;
        prev.copy_from_slice(line);

        kprint!("  {:04x}: ", off);
        for (j, &b) in line.iter().enumerate() {
            kprint!("{:02x} ", b);
            if j == 7 { kprint!(" "); }
        }
        kprint!(" |");
        for &b in line {
            if b >= 0x20 && b < 0x7F {
                kprint!("{}", b as char);
            } else {
                kprint!(".");
            }
        }
        kprintln!("|");
    }
}
