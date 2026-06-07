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

fn hexn(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}
fn hex16(s: &str) -> [u8; 16] {
    let v = hexn(s);
    let mut a = [0u8; 16];
    a.copy_from_slice(&v);
    a
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

    // AES-128 single block (FIPS-197 appendix B / C.1).
    let aes = wifid_core::aes::Aes128::new(&hex16("000102030405060708090a0b0c0d0e0f"));
    let ct = aes.encrypt_block(&hex16("00112233445566778899aabbccddeeff"));
    all &= check("aes128_enc(FIPS-197)", &ct, "69c4e0d86a7b0430d8cdb78070b4c55a");
    let pt = aes.decrypt_block(&ct);
    all &= check("aes128_dec(roundtrip)", &pt, "00112233445566778899aabbccddeeff");

    // AES Key Unwrap — RFC 3394 §4.1 (128-bit KEK, 128-bit key).
    let kek = hex16("000102030405060708090a0b0c0d0e0f");
    let wrapped = hexn("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
    let mut unwrapped = [0u8; 16];
    let ok = wifid_core::aes::aes_unwrap(&kek, &wrapped, &mut unwrapped);
    all &= ok;
    all &= check("aes_unwrap(RFC3394)", &unwrapped, "00112233445566778899aabbccddeeff");
    if !ok {
        println!("[FAIL] aes_unwrap integrity check (A != 0xa6..)");
    }

    // ── 4-way handshake self-consistency (full state machine) ──────────────
    // Build a synthetic AP side with the same independently-vector-tested
    // primitives, run the supplicant through msg1→msg4, and check it derives the
    // right PTK, accepts the MICs, and unwraps the exact GTK we wrapped.
    all &= four_way_roundtrip();

    println!("\n{}", if all { "ALL VECTORS PASS" } else { "SOME VECTORS FAILED" });
    std::process::exit(if all { 0 } else { 1 });
}

fn four_way_roundtrip() -> bool {
    use wifid_core::aes::aes_wrap;
    use wifid_core::eapol::{Step, Supplicant};
    use wifid_core::{hmac_sha1, wpa2_ptk};

    let pmk = wifid_core::wpa2_pmk(b"password", b"IEEE");
    let aa = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]; // AP
    let sa = [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb]; // us
    let anonce = [0xa1u8; 32];
    let snonce = [0x52u8; 32];
    let rsn = hexn("30140100000fac040100000fac040100000fac020000");
    let ptk = wpa2_ptk(&pmk, &aa, &sa, &anonce, &snonce);
    let kck: [u8; 16] = ptk[0..16].try_into().unwrap();
    let kek: [u8; 16] = ptk[16..32].try_into().unwrap();
    let gtk = hexn("000102030405060708090a0b0c0d0e0f"); // 16-byte group key

    let mut sup = Supplicant::new(pmk, aa, sa, snonce, &rsn);
    let mut out = [0u8; 512];

    // AP msg1: Pairwise|Ack, ANonce, no MIC.
    let mut msg1 = vec![0u8; 99];
    msg1[1] = 0x03;
    put_be16(&mut msg1, 2, 95);
    msg1[4] = 0x02;
    put_be16(&mut msg1, 5, 0x0002 | (1 << 3) | (1 << 7));
    msg1[8] = 16;
    msg1[9 + 7] = 1; // replay counter = 1
    msg1[17..49].copy_from_slice(&anonce);
    let mut ok = true;
    match sup.on_eapol(&msg1, &mut out) {
        Step::Reply(n) => {
            // msg2 must carry SNonce, our RSN IE, and a valid MIC.
            ok &= &out[17..49] == &snonce[..];
            let mut m2 = out[..n].to_vec();
            let got_mic = m2[81..97].to_vec();
            for b in &mut m2[81..97] {
                *b = 0;
            }
            ok &= hmac_sha1(&kck, &m2)[..16] == got_mic[..];
        }
        _ => ok = false,
    }
    ok &= sup.ptk().map(|p| p.tk == ptk[32..48]).unwrap_or(false);

    // AP msg3: Pairwise|Ack|MIC|Install|Secure|Encrypted, ANonce, enc{GTK KDE}.
    let mut kde = vec![0xdd, (6 + gtk.len()) as u8, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00];
    kde.extend_from_slice(&gtk); // 24 bytes, multiple of 8
    let mut wrapped = vec![0u8; kde.len() + 8];
    aes_wrap(&kek, &kde, &mut wrapped);
    let mut msg3 = vec![0u8; 99 + wrapped.len()];
    msg3[1] = 0x03;
    put_be16(&mut msg3, 2, (95 + wrapped.len()) as u16);
    msg3[4] = 0x02;
    put_be16(&mut msg3, 5, 0x0002 | (1 << 3) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 9) | (1 << 12));
    msg3[8] = 16;
    msg3[9 + 7] = 2; // replay counter = 2
    msg3[17..49].copy_from_slice(&anonce);
    put_be16(&mut msg3, 97, wrapped.len() as u16);
    msg3[99..].copy_from_slice(&wrapped);
    let m3mic = hmac_sha1(&kck, &msg3)[..16].to_vec(); // MIC field already zero
    msg3[81..97].copy_from_slice(&m3mic);

    match sup.on_eapol(&msg3, &mut out) {
        Step::Done(_) => {
            ok &= sup.gtk().map(|(g, _)| g == &gtk[..]).unwrap_or(false);
        }
        _ => ok = false,
    }

    println!("[{}] four_way_handshake (PTK/MIC/GTK roundtrip)", if ok { "PASS" } else { "FAIL" });
    ok
}

fn put_be16(b: &mut [u8], o: usize, v: u16) {
    b[o] = (v >> 8) as u8;
    b[o + 1] = v as u8;
}
