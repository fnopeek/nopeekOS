//! exFAT images made and checked by the Linux side: mkfs.exfat, the kernel
//! driver through a udisks loop mount, fsck.exfat.

use std::path::{Path, PathBuf};
use std::process::Command;

use extfs_core::exfat::Exfat;
use extfs_core::fat::Error;
use extfs_core::{Dev, IoError, Volume};

struct Img {
    data: Vec<u8>,
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
        Ok(())
    }
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("extfs-exfat-{}", std::process::id()));
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

fn mkfs(path: &Path, label: &str, mb: u64, cluster: Option<&str>) {
    let _ = std::fs::remove_file(path);
    run("truncate", &["-s", &format!("{mb}M"), path.to_str().unwrap()]);
    let mut args = vec!["-L", label];
    if let Some(c) = cluster { args.extend(["-c", c]); }
    args.push(path.to_str().unwrap());
    run("mkfs.exfat", &args);
}

/// Mount the image through udisks, run `f` on the mount point, unmount.
fn mounted(path: &Path, f: impl FnOnce(&Path)) {
    let out = run("udisksctl", &["loop-setup", "-f", path.to_str().unwrap(), "--no-user-interaction"]);
    let dev = out.trim().rsplit(' ').next().unwrap().trim_end_matches('.').to_string();
    let out = run("udisksctl", &["mount", "-b", &dev, "--no-user-interaction"]);
    let mp = out.trim().rsplit(" at ").next().unwrap().trim_end_matches('.').to_string();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(Path::new(&mp))));
    run("udisksctl", &["unmount", "-b", &dev, "--no-user-interaction"]);
    run("udisksctl", &["loop-delete", "-b", &dev, "--no-user-interaction"]);
    if let Err(e) = r { std::panic::resume_unwind(e); }
}

fn read_all<V: Volume>(fs: &mut V, path: &str) -> Vec<u8> {
    let n = fs.lookup(path).unwrap().unwrap_or_else(|| panic!("{path} not found"));
    let (_, size, _) = V::info(&n);
    let mut out = vec![0u8; size as usize];
    let mut at = 0;
    while at < out.len() {
        let piece = (out.len() - at).min(12_345);
        let k = fs.read(&n, at as u64, &mut out[at..at + piece]).unwrap();
        assert!(k > 0);
        at += k;
    }
    out
}

/// Files the Linux driver wrote, read back here.
fn linux_writes_we_read(label: &str, cluster: Option<&str>) {
    let d = dir();
    let p = d.join(format!("{label}.img"));
    mkfs(&p, label, 64, cluster);
    let big = rand_bytes(3_000_123, 3);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    mounted(&p, |mp| {
        std::fs::create_dir_all(mp.join("Photos/2026 Sommer")).unwrap();
        let put = |files: &mut Vec<(String, Vec<u8>)>, name: &str, data: Vec<u8>| {
            if let Some(parent) = mp.join(name).parent() { std::fs::create_dir_all(parent).unwrap(); }
            std::fs::write(mp.join(name), &data).unwrap();
            files.push((name.to_string(), data));
        };
        put(&mut files, "HELLO.TXT", b"hello\n".to_vec());
        put(&mut files, "a name longer than fifteen characters.txt", rand_bytes(5000, 1));
        put(&mut files, "Photos/2026 Sommer/Grüße aus Zürich – lange Namen.jpg", rand_bytes(70_000, 2));
        for k in 0..8 { put(&mut files, &format!("filler{k}.bin"), rand_bytes(40_000, 10 + k)); }
        for k in (0..8).step_by(2) {
            std::fs::remove_file(mp.join(format!("filler{k}.bin"))).unwrap();
            files.retain(|(n, _)| n != &format!("filler{k}.bin"));
        }
        put(&mut files, "big.bin", big.clone());
        put(&mut files, "empty.txt", Vec::new());
        // Enough entries for the root to take more than one cluster.
        for k in 0..150 { put(&mut files, &format!("many/entry number {k:03} with a long name.txt"), vec![k as u8; 10]); }
    });
    let mut fs = Exfat::mount(Img { data: std::fs::read(&p).unwrap() }).unwrap();
    assert_eq!(fs.label(), label);
    assert!(!fs.dirty());
    let root = fs.lookup("").unwrap().unwrap();
    let names: Vec<String> = Volume::list(&mut fs, &root).unwrap().into_iter().map(|e| e.0).collect();
    assert!(names.contains(&"a name longer than fifteen characters.txt".to_string()), "{names:?}");
    for (name, data) in &files {
        assert!(read_all(&mut fs, name) == *data, "{name}");
    }
    assert!(fs.lookup("hello.txt").unwrap().is_some());
    assert!(fs.lookup("PHOTOS/2026 SOMMER/grüße aus zürich – lange namen.jpg").unwrap().is_some());
    let many = fs.lookup("many").unwrap().unwrap();
    assert_eq!(Volume::list(&mut fs, &many).unwrap().len(), 150);
}

#[test]
fn read_default_clusters() { linux_writes_we_read("EXREAD1", None); }

#[test]
fn read_small_clusters() { linux_writes_we_read("EXREAD2", Some("4K")); }

/// Writes made here, checked by fsck.exfat and read back by Linux.
fn we_write_linux_reads(label: &str, cluster: Option<&str>) {
    let d = dir();
    let p = d.join(format!("{label}.img"));
    mkfs(&p, label, 64, cluster);
    // A folder the Linux driver made, contiguous, that we grow.
    mounted(&p, |mp| {
        std::fs::create_dir(mp.join("linux dir")).unwrap();
        std::fs::write(mp.join("linux dir/existing.txt"), b"from linux\n").unwrap();
        std::fs::write(mp.join("replace me.txt"), b"old\n").unwrap();
    });
    let mut fs = Exfat::mount(Img { data: std::fs::read(&p).unwrap() }).unwrap();
    fs.set_now(1_793_548_800);
    let free0 = fs.free_bytes();
    let big = rand_bytes(3_000_123, 7);
    let write = |fs: &mut Exfat<Img>, path: &str, data: &[u8], piece: usize| {
        fs.create(path).unwrap();
        let mut at = 0;
        while at < data.len() {
            let n = (data.len() - at).min(piece);
            fs.append(path, at as u64, &data[at..at + n]).unwrap();
            at += n;
        }
        fs.sync().unwrap();
    };
    fs.mkdir("Photos").unwrap();
    fs.mkdir("Photos/2026 Sommer").unwrap();
    write(&mut fs, "Photos/2026 Sommer/Grüße aus Zürich – ein langer Name.jpg", &big, 1 << 20);
    write(&mut fs, "odd pieces.bin", &big[..777_777], 4097);
    write(&mut fs, "empty.dat", &[], 100);
    for k in 0..60 {
        write(&mut fs, &format!("linux dir/added entry {k:02} with a long name.txt"), format!("n{k}\n").as_bytes(), 100);
    }
    fs.rename("odd pieces.bin", "Photos/moved odd.bin", false).unwrap();
    fs.rename("empty.dat", "EMPTY.DAT", false).unwrap();
    assert_eq!(fs.rename("Photos/moved odd.bin", "replace me.txt", false), Err(Error::Exists));
    fs.rename("Photos/moved odd.bin", "replace me.txt", true).unwrap();
    fs.mkdir("gone").unwrap();
    fs.remove("gone").unwrap();
    assert_eq!(fs.remove("Photos"), Err(Error::NotEmpty));
    assert_eq!(fs.mkdir("bad:name"), Err(Error::BadName));
    fs.remove("linux dir/added entry 07 with a long name.txt").unwrap();
    fs.sync().unwrap();
    assert!(fs.free_bytes() < free0);
    let img = fs.into_dev();
    std::fs::write(&p, &img.data).unwrap();
    run("fsck.exfat", &["-n", p.to_str().unwrap()]);
    mounted(&p, |mp| {
        assert_eq!(std::fs::read(mp.join("Photos/2026 Sommer/Grüße aus Zürich – ein langer Name.jpg")).unwrap(), big);
        assert_eq!(std::fs::read(mp.join("replace me.txt")).unwrap(), &big[..777_777]);
        assert_eq!(std::fs::read(mp.join("EMPTY.DAT")).unwrap(), b"");
        assert_eq!(std::fs::read(mp.join("linux dir/existing.txt")).unwrap(), b"from linux\n");
        assert_eq!(std::fs::read(mp.join("linux dir/added entry 59 with a long name.txt")).unwrap(), b"n59\n");
        assert!(!mp.join("linux dir/added entry 07 with a long name.txt").exists());
        assert!(!mp.join("gone").exists());
        assert!(!mp.join("Photos/moved odd.bin").exists());
    });
    run("fsck.exfat", &["-n", p.to_str().unwrap()]);
}

#[test]
fn write_default_clusters() { we_write_linux_reads("EXWRITE1", None); }

#[test]
fn write_small_clusters() { we_write_linux_reads("EXWRITE2", Some("4K")); }

/// Random damage must give errors, never a panic or a hang.
#[test]
fn damaged_exfat_does_not_panic() {
    let d = dir();
    let p = d.join("dmg.img");
    mkfs(&p, "EXDMG", 16, Some("4K"));
    mounted(&p, |mp| {
        std::fs::create_dir(mp.join("sub")).unwrap();
        for k in 0..20 { std::fs::write(mp.join(format!("sub/file {k}.bin")), rand_bytes(9000, k)).unwrap(); }
    });
    let clean = std::fs::read(&p).unwrap();
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for round in 0..300 {
        let mut data = clean.clone();
        for _ in 0..1 + round % 16 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let at = (x as usize) % (4 << 20);
            data[at] = (x >> 32) as u8;
        }
        let Ok(mut fs) = Exfat::mount(Img { data }) else { continue };
        let root = fs.lookup("").unwrap().unwrap();
        if let Ok(list) = Volume::list(&mut fs, &root) {
            for (_, n) in list.iter().take(5) {
                let (is_dir, _, _) = Exfat::<Img>::info(n);
                if is_dir { let _ = Volume::list(&mut fs, n); }
            }
        }
        if let Ok(Some(n)) = fs.lookup("sub/file 3.bin") {
            let mut buf = vec![0u8; 10_000];
            let _ = fs.read(&n, 0, &mut buf);
        }
        let _ = fs.mkdir("new");
        let _ = fs.sync();
    }
}
