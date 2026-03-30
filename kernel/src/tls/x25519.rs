//! X25519 Key Exchange (RFC 7748)
//!
//! Curve25519 ECDH, 5×51-bit limbs (donna-style).
//! Inspired by curve25519-dalek and donna reference implementations.

/// Compute X25519(scalar, point). Returns 32-byte shared secret.
pub fn x25519(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    let u = fe_decode(point);
    let result = montgomery_ladder(&k, &u);
    fe_encode(&result)
}

/// X25519 base point multiplication.
pub fn x25519_base(scalar: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(scalar, &base)
}

// Field element: 5 limbs of 51 bits each.
// p = 2^255 - 19
type Fe = [u64; 5];
const MASK51: u64 = (1u64 << 51) - 1;

fn fe_decode(bytes: &[u8; 32]) -> Fe {
    let mut b = *bytes;
    b[31] &= 127;
    let w0 = u64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]);
    let w1 = u64::from_le_bytes([b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15]]);
    let w2 = u64::from_le_bytes([b[16],b[17],b[18],b[19],b[20],b[21],b[22],b[23]]);
    let w3 = u64::from_le_bytes([b[24],b[25],b[26],b[27],b[28],b[29],b[30],b[31]]);
    [
        w0 & MASK51,
        ((w0 >> 51) | (w1 << 13)) & MASK51,
        ((w1 >> 38) | (w2 << 26)) & MASK51,
        ((w2 >> 25) | (w3 << 39)) & MASK51,
        (w3 >> 12) & MASK51,
    ]
}

fn fe_encode(h: &Fe) -> [u8; 32] {
    let f = fe_full_reduce(h);
    let w0 = f[0] | (f[1] << 51);
    let w1 = (f[1] >> 13) | (f[2] << 38);
    let w2 = (f[2] >> 26) | (f[3] << 25);
    let w3 = (f[3] >> 39) | (f[4] << 12);
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&w0.to_le_bytes());
    out[8..16].copy_from_slice(&w1.to_le_bytes());
    out[16..24].copy_from_slice(&w2.to_le_bytes());
    out[24..32].copy_from_slice(&w3.to_le_bytes());
    out
}

fn fe_zero() -> Fe { [0; 5] }
fn fe_one() -> Fe { [1, 0, 0, 0, 0] }

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    [a[0]+b[0], a[1]+b[1], a[2]+b[2], a[3]+b[3], a[4]+b[4]]
}

fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    // Add 2*p to avoid underflow. Limbs may be up to ~54 bits after this.
    // 2*p = [2*(2^51-19), 2*(2^51-1), ...]
    [
        (a[0] + 0xFFFFFFFFFFFDA) - b[0],
        (a[1] + 0xFFFFFFFFFFFFE) - b[1],
        (a[2] + 0xFFFFFFFFFFFFE) - b[2],
        (a[3] + 0xFFFFFFFFFFFFE) - b[3],
        (a[4] + 0xFFFFFFFFFFFFE) - b[4],
    ]
}

fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    // Use u128 for intermediate products.
    // With 5×51 limbs, each product term is at most 51+51=102 bits.
    // Sum of 5 such terms: ~105 bits. Fits in u128.
    let b0 = b[0] as u128;
    let b1 = b[1] as u128;
    let b2 = b[2] as u128;
    let b3 = b[3] as u128;
    let b4 = b[4] as u128;

    // Precompute 19*b[i] for reduction (2^255 ≡ 19 mod p)
    let b1_19 = 19 * b1;
    let b2_19 = 19 * b2;
    let b3_19 = 19 * b3;
    let b4_19 = 19 * b4;

    let a0 = a[0] as u128;
    let a1 = a[1] as u128;
    let a2 = a[2] as u128;
    let a3 = a[3] as u128;
    let a4 = a[4] as u128;

    let mut r0 = a0*b0 + a1*b4_19 + a2*b3_19 + a3*b2_19 + a4*b1_19;
    let mut r1 = a0*b1 + a1*b0   + a2*b4_19 + a3*b3_19 + a4*b2_19;
    let mut r2 = a0*b2 + a1*b1   + a2*b0    + a3*b4_19 + a4*b3_19;
    let mut r3 = a0*b3 + a1*b2   + a2*b1    + a3*b0    + a4*b4_19;
    let mut r4 = a0*b4 + a1*b3   + a2*b2    + a3*b1    + a4*b0;

    // Carry propagation
    let c = r0 >> 51; r0 &= MASK51 as u128;
    r1 += c;
    let c = r1 >> 51; r1 &= MASK51 as u128;
    r2 += c;
    let c = r2 >> 51; r2 &= MASK51 as u128;
    r3 += c;
    let c = r3 >> 51; r3 &= MASK51 as u128;
    r4 += c;
    let c = r4 >> 51; r4 &= MASK51 as u128;
    r0 += c * 19; // Wrap: 2^255 ≡ 19
    let c = r0 >> 51; r0 &= MASK51 as u128;
    r1 += c;

    [r0 as u64, r1 as u64, r2 as u64, r3 as u64, r4 as u64]
}

fn fe_sq(a: &Fe) -> Fe {
    fe_mul(a, a)
}

fn fe_mul_a24(a: &Fe) -> Fe {
    let c: u128 = 121665; // a24 = (A-2)/4 where A=486662 for Curve25519
    let mut r0 = a[0] as u128 * c;
    let mut r1 = a[1] as u128 * c;
    let mut r2 = a[2] as u128 * c;
    let mut r3 = a[3] as u128 * c;
    let mut r4 = a[4] as u128 * c;

    let carry = r0 >> 51; r0 &= MASK51 as u128; r1 += carry;
    let carry = r1 >> 51; r1 &= MASK51 as u128; r2 += carry;
    let carry = r2 >> 51; r2 &= MASK51 as u128; r3 += carry;
    let carry = r3 >> 51; r3 &= MASK51 as u128; r4 += carry;
    let carry = r4 >> 51; r4 &= MASK51 as u128; r0 += carry * 19;
    let carry = r0 >> 51; r0 &= MASK51 as u128; r1 += carry;

    [r0 as u64, r1 as u64, r2 as u64, r3 as u64, r4 as u64]
}

/// Full reduction mod p, producing canonical form [0, p)
fn fe_full_reduce(h: &Fe) -> Fe {
    let mut f = *h;
    // Carry chain
    let mut c: u64;
    c = f[0] >> 51; f[0] &= MASK51; f[1] += c;
    c = f[1] >> 51; f[1] &= MASK51; f[2] += c;
    c = f[2] >> 51; f[2] &= MASK51; f[3] += c;
    c = f[3] >> 51; f[3] &= MASK51; f[4] += c;
    c = f[4] >> 51; f[4] &= MASK51; f[0] += c * 19;
    c = f[0] >> 51; f[0] &= MASK51; f[1] += c;

    // Check if f >= p by testing if f + 19 >= 2^255
    let mut g = f;
    g[0] += 19;
    c = g[0] >> 51; g[0] &= MASK51; g[1] += c;
    c = g[1] >> 51; g[1] &= MASK51; g[2] += c;
    c = g[2] >> 51; g[2] &= MASK51; g[3] += c;
    c = g[3] >> 51; g[3] &= MASK51; g[4] += c;
    let overflow = g[4] >> 51; // 1 if f >= p, 0 otherwise
    g[4] &= MASK51;

    // Conditional select: if overflow, use g (= f - p), else use f
    let mask = overflow.wrapping_neg(); // 0xFFFF... if overflow, 0 if not
    [
        (f[0] & !mask) | (g[0] & mask),
        (f[1] & !mask) | (g[1] & mask),
        (f[2] & !mask) | (g[2] & mask),
        (f[3] & !mask) | (g[3] & mask),
        (f[4] & !mask) | (g[4] & mask),
    ]
}

fn fe_invert(a: &Fe) -> Fe {
    // a^(p-2) via addition chain
    let z2 = fe_sq(a);
    let z9 = { let t = fe_sq(&fe_sq(&z2)); fe_mul(&t, a) };
    let z11 = fe_mul(&z9, &z2);
    let z_5_0 = fe_mul(&fe_sq(&z11), &z9);
    let z_10_0 = { let mut t = z_5_0; for _ in 0..5 { t = fe_sq(&t); } fe_mul(&t, &z_5_0) };
    let z_20_0 = { let mut t = z_10_0; for _ in 0..10 { t = fe_sq(&t); } fe_mul(&t, &z_10_0) };
    let z_40_0 = { let mut t = z_20_0; for _ in 0..20 { t = fe_sq(&t); } fe_mul(&t, &z_20_0) };
    let z_50_0 = { let mut t = z_40_0; for _ in 0..10 { t = fe_sq(&t); } fe_mul(&t, &z_10_0) };
    let z_100_0 = { let mut t = z_50_0; for _ in 0..50 { t = fe_sq(&t); } fe_mul(&t, &z_50_0) };
    let z_200_0 = { let mut t = z_100_0; for _ in 0..100 { t = fe_sq(&t); } fe_mul(&t, &z_100_0) };
    let z_250_0 = { let mut t = z_200_0; for _ in 0..50 { t = fe_sq(&t); } fe_mul(&t, &z_50_0) };
    let mut t = z_250_0;
    for _ in 0..5 { t = fe_sq(&t); }
    fe_mul(&t, &z11)
}

fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
    let mask = 0u64.wrapping_sub(swap);
    for i in 0..5 {
        let t = mask & (a[i] ^ b[i]);
        a[i] ^= t;
        b[i] ^= t;
    }
}

fn montgomery_ladder(scalar: &[u8; 32], u_point: &Fe) -> Fe {
    let mut x_2 = fe_one();
    let mut z_2 = fe_zero();
    let mut x_3 = *u_point;
    let mut z_3 = fe_one();
    let mut swap: u64 = 0;

    for pos in (0..255).rev() {
        let bit = ((scalar[pos / 8] >> (pos & 7)) & 1) as u64;
        swap ^= bit;
        cswap(swap, &mut x_2, &mut x_3);
        cswap(swap, &mut z_2, &mut z_3);
        swap = bit;

        let a = fe_add(&x_2, &z_2);
        let aa = fe_sq(&a);
        let b = fe_sub(&x_2, &z_2);
        let bb = fe_sq(&b);
        let e = fe_sub(&aa, &bb);
        let c = fe_add(&x_3, &z_3);
        let d = fe_sub(&x_3, &z_3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        x_3 = fe_sq(&fe_add(&da, &cb));
        z_3 = fe_mul(u_point, &fe_sq(&fe_sub(&da, &cb)));
        x_2 = fe_mul(&aa, &bb);
        z_2 = fe_mul(&e, &fe_add(&aa, &fe_mul_a24(&e)));
    }

    cswap(swap, &mut x_2, &mut x_3);
    cswap(swap, &mut z_2, &mut z_3);
    fe_mul(&x_2, &fe_invert(&z_2))
}
