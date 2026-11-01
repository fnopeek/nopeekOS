//! Writes by this crate, checked by the Linux tools: `fsck.fat` must find
//! nothing to fix, and mtools must read back what was written.

use std::path::{Path, PathBuf};
use std::process::Command;

use extfs_core::fat::{Error, Fat};
use extfs_core::{Dev, IoError};

struct Img {
    data: Vec<u8>,
    flushes: usize,
    writes: usize,
}

impl Dev for Img {
    fn block_size(&self) -> u32 { 512 }
    fn blocks(&self) -> u64 { self.data.len() as u64 / 512 }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let off = (lba * 512) as usize;
        buf.copy_from_slice(self.data.get(off..off + buf.len()).ok_or(IoError)?);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let off = (lba * 512) as usize;
        self.data.get_mut(off..off + buf.len()).ok_or(IoError)?.copy_from_slice(buf);
        self.writes += 1;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), IoError> { self.flushes += 1; Ok(()) }
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("extfs-write-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(cmd: &str, args: &[&str]) -> String {
    let o = Command::new(cmd).args(args).output().unwrap_or_else(|_| panic!("{cmd} missing"));
    let text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{cmd} {args:?}:\n{text}");
    text
}

fn rand_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x as u8 }).collect()
}

fn fresh(path: &Path, fat: &str, mb: u64, cluster_sectors: Option<&str>) -> Fat<Img> {
    let _ = std::fs::remove_file(path);
    let kib = (mb * 1024).to_string();
    let mut args = vec!["-C", "-F", fat, "-n", "WRITETEST"];
    if let Some(c) = cluster_sectors { args.extend(["-s", c]); }
    args.extend([path.to_str().unwrap(), kib.as_str()]);
    run("mkfs.fat", &args);
    let mut fs = Fat::mount(Img { data: std::fs::read(path).unwrap(), flushes: 0, writes: 0 }).unwrap();
    fs.set_now(1_793_548_800);
    fs
}

fn save(fs: Fat<Img>, path: &Path) -> Img {
    let img = fs.into_dev();
    std::fs::write(path, &img.data).unwrap();
    img
}

fn mtype(path: &Path, name: &str) -> Vec<u8> {
    let o = Command::new("mtype").args(["-i", path.to_str().unwrap(), &format!("::/{name}")])
        .env("MTOOLS_SKIP_CHECK", "1").output().unwrap();
    assert!(o.status.success(), "mtype {name}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

fn write_file(fs: &mut Fat<Img>, path: &str, data: &[u8], piece: usize) {
    fs.create(path).unwrap();
    let mut at = 0;
    while at < data.len() {
        let n = (data.len() - at).min(piece);
        fs.append(path, at as u64, &data[at..at + n]).unwrap();
        at += n;
    }
    fs.sync().unwrap();
}

fn exercise(fat: &str, mb: u64, cluster: Option<&str>) {
    let d = dir();
    let p = d.join(format!("w{fat}-{}.img", cluster.unwrap_or("d")));
    let mut fs = fresh(&p, fat, mb, cluster);
    let free0 = fs.free_bytes();

    let big = rand_bytes(3_000_123, 7);
    let small = b"hello from nopeekOS\n".to_vec();
    fs.mkdir("Photos").unwrap();
    fs.mkdir("Photos/2026 Sommer").unwrap();
    write_file(&mut fs, "README.TXT", &small, 4096);
    write_file(&mut fs, "readme.md", &small, 4096);
    write_file(&mut fs, "Photos/2026 Sommer/Grüße aus Zürich.jpg", &big, 1 << 20);
    write_file(&mut fs, "Photos/odd pieces.bin", &big[..777_777], 4097);
    for k in 0..12 {
        write_file(&mut fs, &format!("Long Name Number {k}.txt"), format!("file {k}\n").as_bytes(), 100);
    }
    write_file(&mut fs, "empty.dat", &[], 100);
    // Overwrite cuts the old content.
    write_file(&mut fs, "readme.md", b"second\n", 100);
    // Rename: same folder, case only, into a folder, over a file.
    fs.rename("Long Name Number 0.txt", "Renamed in place.txt", false).unwrap();
    fs.rename("Long Name Number 1.txt", "long name number 1.TXT", false).unwrap();
    fs.rename("Long Name Number 2.txt", "Photos/moved.txt", false).unwrap();
    assert_eq!(fs.rename("Long Name Number 3.txt", "Long Name Number 4.txt", false), Err(Error::Exists));
    fs.rename("Long Name Number 3.txt", "Long Name Number 4.txt", true).unwrap();
    fs.mkdir("Moved Folder").unwrap();
    fs.rename("Moved Folder", "Photos/Moved Folder", false).unwrap();
    assert_eq!(fs.rename("Photos", "Photos/2026 Sommer/x", false), Err(Error::BadName));
    // Remove: a file, a non-empty folder refused, an empty one.
    fs.remove("Long Name Number 5.txt").unwrap();
    assert_eq!(fs.remove("Photos"), Err(Error::NotEmpty));
    fs.mkdir("gone").unwrap();
    fs.remove("gone").unwrap();
    assert_eq!(fs.mkdir("bad:name"), Err(Error::BadName));
    assert_eq!(fs.mkdir("readme.md"), Err(Error::Exists));
    fs.sync().unwrap();
    assert!(fs.free_bytes() < free0);

    let img = save(fs, &p);
    assert!(img.flushes > 0);
    // State byte clean after sync.
    let state_off = if fat == "32" { 65 } else { 37 };
    assert_eq!(img.data[state_off] & 1, 0, "dirty bit left set");

    let out = run("fsck.fat", &["-n", "-v", p.to_str().unwrap()]);
    assert!(!out.contains("Free cluster summary wrong") && !out.contains("differ"), "fsck:\n{out}");
    let mdir = run("mdir", &["-/", "-i", p.to_str().unwrap(), "::/"]);
    for name in ["Grüße aus Zürich.jpg", "Renamed in place.txt", "long name number 1.TXT", "moved.txt",
                 "Moved Folder", "README", "readme.md", "Long Name Number 4.txt"] {
        assert!(mdir.contains(name), "{name} missing:\n{mdir}");
    }
    assert!(!mdir.contains("Long Name Number 5.txt") && !mdir.contains("gone"), "{mdir}");
    assert_eq!(mtype(&p, "Photos/2026 Sommer/Grüße aus Zürich.jpg"), big);
    assert_eq!(mtype(&p, "Photos/odd pieces.bin"), &big[..777_777]);
    assert_eq!(mtype(&p, "README.TXT"), small);
    assert_eq!(mtype(&p, "readme.md"), b"second\n");
    assert_eq!(mtype(&p, "Long Name Number 4.txt"), b"file 3\n");
    assert_eq!(mtype(&p, "empty.dat"), b"");

    // And this crate reads it back the same way.
    let mut fs = Fat::mount(Img { data: img.data, flushes: 0, writes: 0 }).unwrap();
    let n = fs.lookup("Photos/2026 Sommer/Grüße aus Zürich.jpg").unwrap().unwrap();
    let mut back = vec![0u8; big.len()];
    let mut at = 0;
    while at < back.len() { at += fs.read(&n, at as u64, &mut back[at..]).unwrap(); }
    assert_eq!(back, big);
}

#[test]
fn fat32_writes() { exercise("32", 64, None); }

#[test]
fn fat32_small_clusters() { exercise("32", 64, Some("1")); }

#[test]
fn fat16_writes() { exercise("16", 32, None); }

#[test]
fn fat12_writes() { exercise("12", 8, None); }

/// The fixed FAT12/16 root fills up; the volume stays consistent.
#[test]
fn full_root_and_full_volume() {
    let d = dir();
    let p = d.join("full.img");
    let mut fs = fresh(&p, "12", 2, None);
    let mut made = 0;
    loop {
        match fs.create(&format!("a rather long name to use many slots {made}.txt")) {
            Ok(()) => made += 1,
            Err(Error::NoSpace) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(made > 5);
    fs.sync().unwrap();
    // Fill the data area until it is full.
    fs.mkdir("x").ok();
    let _ = fs.remove(&format!("a rather long name to use many slots {}.txt", made - 1));
    fs.create("big").unwrap();
    let chunk = vec![0x5Au8; 65536];
    let mut at = 0u64;
    loop {
        match fs.append("big", at, &chunk) {
            Ok(()) => at += chunk.len() as u64,
            Err(Error::NoSpace) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    fs.sync().unwrap();
    save(fs, &p);
    let out = run("fsck.fat", &["-n", "-v", p.to_str().unwrap()]);
    assert!(!out.contains("Free cluster summary wrong"), "fsck:\n{out}");
}

/// A stream of appends writes its data and little else: the entry and the
/// FAT follow once, at the end.
#[test]
fn appends_write_mostly_data() {
    let d = dir();
    let p = d.join("stream.img");
    let mut fs = fresh(&p, "32", 64, None);
    fs.create("stream.bin").unwrap();
    fs.sync().unwrap();
    let before = fs.into_dev();
    let w0 = before.writes;
    let mut fs = Fat::mount(before).unwrap();
    let chunk = vec![0xA5u8; 1 << 20];
    for k in 0..16u64 {
        fs.append("stream.bin", k << 20, &chunk).unwrap();
    }
    fs.sync().unwrap();
    let img = save(fs, &p);
    // 16 data writes; with 512-byte clusters the FAT of 16 MiB is 256
    // sectors, written in runs of the cache's 32 to both copies (16); the
    // dirty bit twice, the entry, FSINFO.
    assert!(img.writes - w0 <= 60, "{} writes for 16 MiB", img.writes - w0);
    run("fsck.fat", &["-n", p.to_str().unwrap()]);
    assert_eq!(mtype(&p, "stream.bin").len(), 16 << 20);
}
