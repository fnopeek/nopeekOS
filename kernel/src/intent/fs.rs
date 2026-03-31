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
    let marker = alloc::format!("{}/.dir", resolved);
    if crate::npkfs::exists(&marker) {
        kprintln!("[npk] Directory '{}' already exists", resolved);
        return;
    }
    super::ensure_parents(&resolved);
    kprintln!("[npk] Created '{}'", resolved);
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

    let prefix = alloc::format!("{}/", resolved);
    if let Ok(entries) = crate::npkfs::list() {
        let has_content = entries.iter().any(|(n, _, _)| {
            n.starts_with(prefix.as_str()) && !n.ends_with("/.dir")
        });
        if has_content {
            kprintln!("[npk] Directory '{}' is not empty", resolved);
            return;
        }
    }

    let marker = alloc::format!("{}/.dir", resolved);
    match crate::npkfs::delete(&marker) {
        Ok(()) => kprintln!("[npk] Removed '{}'", resolved),
        Err(_) => kprintln!("[npk] Directory '{}' not found", resolved),
    }
}

pub fn intent_list(args: &str) {
    let filter = args.trim();
    // Use explicit arg, or cwd, or root
    let resolved = if !filter.is_empty() {
        resolve_path(filter)
    } else {
        super::get_cwd()
    };
    let prefix = if !resolved.is_empty() {
        let mut p = resolved;
        if !p.ends_with('/') { p.push('/'); }
        Some(p)
    } else {
        None
    };

    match crate::npkfs::list() {
        Ok(entries) => {
            // Filter and collect visible entries (hide .npk-* and .dir markers)
            let visible: alloc::vec::Vec<_> = entries.iter()
                .filter(|(name, _, _)| {
                    if name.starts_with(".npk-") { return false; }
                    if name.ends_with("/.dir") { return false; }
                    if let Some(ref pfx) = prefix {
                        return name.starts_with(pfx.as_str());
                    }
                    true
                })
                .collect();

            // Collect known dirs (from .dir markers and from object prefixes)
            let mut dirs: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
            for (name, _, _) in &entries {
                // .dir markers define empty dirs (register all levels)
                if let Some(dir) = name.strip_suffix("/.dir") {
                    let mut remaining = dir;
                    loop {
                        let d = alloc::string::String::from(remaining);
                        if !dirs.contains(&d) { dirs.push(d); }
                        match remaining.rfind('/') {
                            Some(idx) => remaining = &remaining[..idx],
                            None => break,
                        }
                    }
                }
                // Objects with / define implicit dirs (all levels)
                let mut remaining = name.as_str();
                while let Some(idx) = remaining.rfind('/') {
                    remaining = &remaining[..idx];
                    let d = alloc::string::String::from(remaining);
                    if !dirs.contains(&d) { dirs.push(d); }
                }
            }
            dirs.sort();

            kprintln!();

            if visible.is_empty() && dirs.is_empty() {
                kprintln!("  (empty)");
            } else {
                // Determine the current "depth" we're listing
                let prefix_str = prefix.as_deref().unwrap_or("");

                // Show subdirectories at this level
                let mut shown_dirs: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
                for dir in &dirs {
                    let rel = if !prefix_str.is_empty() {
                        match dir.strip_prefix(prefix_str) {
                            Some(r) => r,
                            None => continue,
                        }
                    } else {
                        dir.as_str()
                    };
                    // Only show immediate children (no nested /, no self-reference)
                    if rel.is_empty() || rel == "." || rel.contains('/') { continue; }
                    if shown_dirs.contains(&rel) { continue; }
                    shown_dirs.push(rel);

                    // Count objects in this dir
                    let dir_prefix = alloc::format!("{}/", dir);
                    let count = entries.iter()
                        .filter(|(n, _, _)| n.starts_with(dir_prefix.as_str()) && !n.ends_with("/.dir") && !n.starts_with(".npk-"))
                        .count();
                    if count == 0 {
                        kprintln!("  {}/", rel);
                    } else {
                        kprintln!("  {}/  ({} objects)", rel, count);
                    }
                }

                // Show files at this level (no / after prefix)
                for (name, size, hash) in &visible {
                    let rel = if !prefix_str.is_empty() {
                        match name.strip_prefix(prefix_str) {
                            Some(r) => r,
                            None => continue,
                        }
                    } else {
                        name.as_str()
                    };
                    // Only show files at this level (not in subdirs)
                    if rel.contains('/') { continue; }

                    kprint!("  {:<24} ", rel);
                    if *size >= 1024 {
                        kprint!("{:>6} KB  ", size / 1024);
                    } else {
                        kprint!("{:>6} B   ", size);
                    }
                    for b in &hash[..4] { kprint!("{:02x}", b); }
                    kprintln!();
                }
            }

            if let Some((_, free, count, _)) = crate::npkfs::stats() {
                kprintln!();
                kprintln!("  {} objects, {} free blocks", count, free);
            }
            kprintln!();
        }
        Err(e) => kprintln!("[npk] List error: {}", e),
    }
}

pub fn intent_fsinfo() {
    match crate::npkfs::stats() {
        Some((total, free, objects, gen)) => {
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
            kprintln!("  Generation:  {}", gen);
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
            let dev = if crate::nvme::is_available() { "NVMe" } else { "virtio-blk" };
            kprintln!();
            kprintln!("  Block Device ({})", dev);
            kprintln!("  ────────────────────────");
            kprintln!("  Capacity:  {} sectors / {} blocks ({} MB)", cap, blocks, mb);
            kprintln!("  Sector:    512 bytes");
            kprintln!("  Block:     4096 bytes");
            kprintln!("  TRIM:      {}", if blkdev::has_discard() { "supported" } else { "not available" });
            kprintln!("  Status:    online");
            kprintln!();
        }
        None => kprintln!("[npk] No block device available"),
    }
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
