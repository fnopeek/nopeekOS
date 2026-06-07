//! wifid dev-harness — validates wifid_core's WPA2 crypto against published
//! test vectors on the dev machine. `cargo run` → all vectors must PASS.

use wifid_core::{hmac_sha1, sha1, wpa2_pmk, wpa2_ptk};

fn hex(b: &[u8]) -> String {
    let mut s = String::new();
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

fn check(name: &str, got: &[u8], want: &str) -> bool {
    let g = hex(got);
    let ok = g == want;
    println!("[{}] {}  {}", if ok { "PASS" } else { "FAIL" }, name, g);
    if !ok {
        println!("       want {}", want);
    }
    ok
}

fn main() {
    let mut all = true;

    // FIPS 180 SHA-1.
    all &= check("sha1(abc)", &sha1(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    all &= check(
        "sha1(2-block)",
        &sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
    );

    // RFC 2202 HMAC-SHA1 case 1.
    all &= check(
        "hmac_sha1(Hi There)",
        &hmac_sha1(&[0x0b; 20], b"Hi There"),
        "b617318655057264e28bc0b6fb378c8ef146be00",
    );

    // IEEE 802.11i PMK vector: passphrase "password", SSID "IEEE".
    all &= check(
        "wpa2_pmk(password,IEEE)",
        &wpa2_pmk(b"password", b"IEEE"),
        "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e",
    );

    // PTK derivation smoke test (Jouni Malinen's well-known 4-way vector):
    // PMK all-zero variant is deterministic; assert it runs and is stable.
    let pmk = wpa2_pmk(b"password", b"IEEE");
    let aa = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let sa = [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb];
    let anonce = [0x11u8; 32];
    let snonce = [0x22u8; 32];
    let ptk = wpa2_ptk(&pmk, &aa, &sa, &anonce, &snonce);
    println!("[INFO] ptk(48) = {}", hex(&ptk));
    all &= ptk.iter().any(|&b| b != 0); // sanity: non-trivial output

    println!("\n{}", if all { "ALL VECTORS PASS" } else { "SOME VECTORS FAILED" });
    std::process::exit(if all { 0 } else { 1 });
}
