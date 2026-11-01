//! Images made by the Linux tools (sfdisk, mkfs.fat, mtools), read back.

use std::path::{Path, PathBuf};
use std::process::Command;

use extfs_core::fat::Fat;
use extfs_core::{part, Dev, IoError};

struct Img {
    data: Vec<u8>,
    bs: u32,
    start: u64,
    count: u64,
}

impl Dev for Img {
    fn block_size(&self) -> u32 { self.bs }
    fn blocks(&self) -> u64 { self.count }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let n = buf.len() as u64 / self.bs as u64;
        if buf.len() % self.bs as usize != 0 || lba + n > self.count { return Err(IoError); }
        let off = ((self.start + lba) * self.bs as u64) as usize;
        let src = self.data.get(off..off + buf.len()).ok_or(IoError)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("extfs-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(cmd: &str, args: &[&str]) {
    let st = Command::new(cmd).args(args).status().unwrap_or_else(|_| panic!("{cmd} missing"));
    assert!(st.success(), "{cmd} {args:?}");
}

fn rand_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x as u8 }).collect()
}

/// A FAT image of `mb` MiB with files, long names, folders and a
/// fragmented file.
fn make_fat(path: &Path, mb: u64, fat: &str, offset_sectors: u64) -> Vec<(String, Vec<u8>)> {
    let d = path.parent().unwrap();
    let img = path.to_str().unwrap();
    let mut args = vec!["-C", "-F", fat, "-n", "TESTVOL"];
    let off = offset_sectors.to_string();
    if offset_sectors > 0 { args.extend(["--offset", off.as_str()]); }
    let kib = (mb * 1024).to_string();
    let _ = std::fs::remove_file(path);
    if offset_sectors > 0 {
        run("truncate", &["-s", &format!("{}M", mb + 2), img]);
        let args: Vec<&str> = vec!["-F", fat, "-n", "TESTVOL", "--offset", off.as_str(), img, &kib];
        run("mkfs.fat", &args);
    } else {
        args.extend([img, kib.as_str()]);
        run("mkfs.fat", &args);
    }
    let at = format!("{}@@{}", img, offset_sectors * 512);
    let mut files = Vec::new();
    let put = |files: &mut Vec<(String, Vec<u8>)>, name: &str, data: Vec<u8>| {
        let f = d.join(format!("payload-{}", path.file_name().unwrap().to_str().unwrap()));
        std::fs::write(&f, &data).unwrap();
        run("mcopy", &["-o", "-i", &at, f.to_str().unwrap(), &format!("::/{name}")]);
        files.push((name.to_string(), data));
    };
    run("mmd", &["-i", &at, "::/Sub Folder"]);
    run("mmd", &["-i", &at, "::/Sub Folder/Deeper"]);
    put(&mut files, "HELLO.TXT", b"hello\n".to_vec());
    put(&mut files, "a long file name with spaces.txt", rand_bytes(5000, 1));
    put(&mut files, "Grüße – ünïcode.md", b"utf8 name\n".to_vec());
    put(&mut files, "Sub Folder/Deeper/inner.bin", rand_bytes(70_000, 2));
    // Fragment: fill, delete every other file, then write a large file.
    for k in 0..8 { put(&mut files, &format!("filler{k}.bin"), rand_bytes(40_000, 10 + k)); }
    for k in (0..8).step_by(2) {
        run("mdel", &["-i", &at, &format!("::/filler{k}.bin")]);
        files.retain(|(n, _)| n != &format!("filler{k}.bin"));
    }
    put(&mut files, "big.bin", rand_bytes(3 << 20, 3));
    put(&mut files, "empty.txt", Vec::new());
    files
}

fn check_fs(img: &Img, files: &[(String, Vec<u8>)]) {
    let dev = Img { data: img.data.clone(), bs: img.bs, start: img.start, count: img.count };
    let mut fs = Fat::mount(dev).expect("mount");
    assert_eq!(fs.label(), "TESTVOL");
    assert!(fs.free_bytes() < fs.total_bytes());
    let root = fs.root();
    let names: Vec<String> = fs.read_dir(&root).unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"Sub Folder".to_string()), "{names:?}");
    assert!(names.contains(&"a long file name with spaces.txt".to_string()), "{names:?}");
    assert!(names.contains(&"Grüße – ünïcode.md".to_string()), "{names:?}");
    for (name, data) in files {
        let node = fs.lookup(name).unwrap().unwrap_or_else(|| panic!("{name} not found"));
        assert_eq!(node.size, data.len() as u64, "{name}");
        // Whole, and in odd pieces to walk the cursor.
        let mut out = vec![0u8; data.len()];
        let mut at = 0;
        while at < out.len() {
            let piece = (out.len() - at).min(12_345);
            let n = fs.read(&node, at as u64, &mut out[at..at + piece]).unwrap();
            assert!(n > 0);
            at += n;
        }
        assert!(out == *data, "{name} content");
    }
    // Case-insensitive, and the 8.3 alias.
    assert!(fs.lookup("hello.txt").unwrap().is_some());
    assert!(fs.lookup("SUB FOLDER/deeper/INNER.BIN").unwrap().is_some());
    assert!(fs.lookup("nope").unwrap().is_none());
    // A read from the middle after one at the end.
    let big = files.iter().find(|(n, _)| n == "big.bin").unwrap();
    let node = fs.lookup("big.bin").unwrap().unwrap();
    let mut out = vec![0u8; 1000];
    fs.read(&node, 3_000_000, &mut out).unwrap();
    assert_eq!(&out[..], &big.1[3_000_000..3_001_000]);
    let n = fs.read(&node, 1_000_000, &mut out).unwrap();
    assert_eq!(&out[..n], &big.1[1_000_000..1_000_000 + n]);
}

#[test]
fn fat32_superfloppy() {
    let d = dir();
    let p = d.join("f32.img");
    let files = make_fat(&p, 64, "32", 0);
    let data = std::fs::read(&p).unwrap();
    let count = data.len() as u64 / 512;
    let mut img = Img { data, bs: 512, start: 0, count };
    let parts = part::scan(&mut img).unwrap();
    assert_eq!(parts, vec![part::Part { start: 0, count }]);
    check_fs(&img, &files);
}

#[test]
fn fat16_and_fat12() {
    let d = dir();
    for (fat, mb) in [("16", 32), ("12", 8)] {
        let p = d.join(format!("f{fat}.img"));
        let files = make_fat(&p, mb, fat, 0);
        let data = std::fs::read(&p).unwrap();
        let count = data.len() as u64 / 512;
        let img = Img { data, bs: 512, start: 0, count };
        let fs = Fat::mount(Img { data: img.data.clone(), bs: 512, start: 0, count }).unwrap();
        assert_eq!(fs.fat_bits().to_string(), fat);
        check_fs(&img, &files);
    }
}

#[test]
fn mbr_and_gpt() {
    let d = dir();
    for label in ["dos", "gpt"] {
        let p = d.join(format!("{label}.img"));
        let img = p.to_str().unwrap();
        let _ = std::fs::remove_file(&p);
        run("truncate", &["-s", "100M", img]);
        let script = format!("label: {label}\nstart=2048, size=40M\nstart=104448, size=40M\n");
        let mut c = Command::new("sfdisk").args(["-q", img]).stdin(std::process::Stdio::piped()).spawn().unwrap();
        use std::io::Write;
        c.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        assert!(c.wait().unwrap().success());
        let files = make_fat_in(&p, 2048, 40);
        let data = std::fs::read(&p).unwrap();
        let count = data.len() as u64 / 512;
        let mut whole = Img { data, bs: 512, start: 0, count };
        let parts = part::scan(&mut whole).unwrap();
        assert_eq!(parts, vec![
            part::Part { start: 2048, count: 40 * 2048 },
            part::Part { start: 104448, count: 40 * 2048 },
        ], "{label}");
        let vol = Img { data: whole.data.clone(), bs: 512, start: 2048, count: 40 * 2048 };
        check_fs(&vol, &files);
    }
}

/// A FAT32 file system inside an existing image at `start` sectors.
fn make_fat_in(path: &Path, start: u64, mb: u64) -> Vec<(String, Vec<u8>)> {
    let kib = (mb * 1024).to_string();
    let off = start.to_string();
    run("mkfs.fat", &["-F", "32", "-n", "TESTVOL", "--offset", &off, path.to_str().unwrap(), &kib]);
    // `make_fat` would recreate the image; fill it the same way.
    fill(path, start)
}

fn fill(path: &Path, start: u64) -> Vec<(String, Vec<u8>)> {
    let d = path.parent().unwrap();
    let at = format!("{}@@{}", path.to_str().unwrap(), start * 512);
    let mut files = Vec::new();
    run("mmd", &["-i", &at, "::/Sub Folder"]);
    run("mmd", &["-i", &at, "::/Sub Folder/Deeper"]);
    for (name, data) in [
        ("HELLO.TXT", b"hello\n".to_vec()),
        ("a long file name with spaces.txt", rand_bytes(5000, 1)),
        ("Grüße – ünïcode.md", b"utf8 name\n".to_vec()),
        ("Sub Folder/Deeper/inner.bin", rand_bytes(70_000, 2)),
        ("big.bin", rand_bytes(3 << 20, 3)),
    ] {
        let f = d.join(format!("payload-{}", path.file_name().unwrap().to_str().unwrap()));
        std::fs::write(&f, &data).unwrap();
        run("mcopy", &["-o", "-i", &at, f.to_str().unwrap(), &format!("::/{name}")]);
        files.push((name.to_string(), data));
    }
    files
}

/// Random damage must give errors, never a panic or a hang.
#[test]
fn damaged_images_do_not_panic() {
    let d = dir();
    let p = d.join("dmg.img");
    make_fat(&p, 16, "16", 0);
    let clean = std::fs::read(&p).unwrap();
    let count = clean.len() as u64 / 512;
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for round in 0..300 {
        let mut data = clean.clone();
        // Damage the boot sector, the FATs and the root directory.
        for _ in 0..1 + round % 16 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let at = (x as usize) % 200_000;
            data[at] = (x >> 32) as u8;
        }
        let mut img = Img { data, bs: 512, start: 0, count };
        let _ = part::scan(&mut img);
        let Ok(mut fs) = Fat::mount(img) else { continue };
        let root = fs.root();
        if let Ok(list) = fs.read_dir(&root) {
            for e in list.iter().take(20) {
                let mut buf = vec![0u8; 100_000];
                if e.node.is_dir { let _ = fs.read_dir(&e.node); } else { let _ = fs.read(&e.node, 0, &mut buf); }
            }
        }
        let _ = fs.lookup("Sub Folder/Deeper/inner.bin");
    }
}
