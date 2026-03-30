//! X25519 Key Exchange (RFC 7748)
//!
//! Elliptic curve Diffie-Hellman on Curve25519.
//! Montgomery ladder implementation, constant-time.

/// Compute X25519(scalar, point). Returns 32-byte shared secret.
pub fn x25519(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    // Clamp scalar per RFC 7748
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    let u = decode_u_coordinate(point);
    let result = montgomery_ladder(&k, &u);
    encode_u_coordinate(&result)
}

/// X25519 base point multiplication (for generating public keys).
pub fn x25519_base(scalar: &[u8; 32]) -> [u8; 32] {
    let base = {
        let mut b = [0u8; 32];
        b[0] = 9;
        b
    };
    x25519(scalar, &base)
}

// Field element: 5 limbs of 51 bits each (fits in u64)
// p = 2^255 - 19
type Fe = [u64; 5];

const MASK51: u64 = (1u64 << 51) - 1;

fn fe_zero() -> Fe { [0; 5] }
fn fe_one() -> Fe { [1, 0, 0, 0, 0] }

fn decode_u_coordinate(bytes: &[u8; 32]) -> Fe {
    let mut b = *bytes;
    b[31] &= 127; // Mask top bit per RFC 7748
    let mut f = fe_zero();
    f[0] = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], 0, 0]) & MASK51;
    f[1] = (u64::from_le_bytes([b[6], b[7], b[8], b[9], b[10], b[11], b[12], 0]) >> 3) & MASK51;
    f[2] = (u64::from_le_bytes([b[12], b[13], b[14], b[15], b[16], b[17], b[18], b[19]]) >> 6) & MASK51;
    f[3] = (u64::from_le_bytes([b[19], b[20], b[21], b[22], b[23], b[24], b[25], 0]) >> 1) & MASK51;
    f[4] = (u64::from_le_bytes([b[25], b[26], b[27], b[28], b[29], b[30], b[31], 0]) >> 4) & MASK51;
    f
}

fn encode_u_coordinate(f: &Fe) -> [u8; 32] {
    let mut h = *f;
    fe_reduce(&mut h);

    let mut out = [0u8; 32];
    let mut acc: u128 = 0;
    let mut bits = 0u32;
    let mut pos = 0;

    for &limb in h.iter() {
        acc |= (limb as u128) << bits;
        bits += 51;
        while bits >= 8 && pos < 32 {
            out[pos] = acc as u8;
            acc >>= 8;
            bits -= 8;
            pos += 1;
        }
    }
    if pos < 32 {
        out[pos] = acc as u8;
    }
    out
}

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3], a[4] + b[4]]
}

fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    // Add 2*p to avoid underflow
    [
        a[0] + 0xFFFFFFFFFFFDA - b[0],
        a[1] + 0xFFFFFFFFFFFFE - b[1],
        a[2] + 0xFFFFFFFFFFFFE - b[2],
        a[3] + 0xFFFFFFFFFFFFE - b[3],
        a[4] + 0xFFFFFFFFFFFFE - b[4],
    ]
}

fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let mut t = [0u128; 5];

    // Schoolbook multiply with reduction
    // 2^255 ≡ 19 (mod p), so overflow from limb 4 wraps with factor 19
    let b19_1 = b[1] as u128 * 19;
    let b19_2 = b[2] as u128 * 19;
    let b19_3 = b[3] as u128 * 19;
    let b19_4 = b[4] as u128 * 19;

    t[0] = a[0] as u128 * b[0] as u128
         + a[1] as u128 * b19_4
         + a[2] as u128 * b19_3
         + a[3] as u128 * b19_2
         + a[4] as u128 * b19_1;

    t[1] = a[0] as u128 * b[1] as u128
         + a[1] as u128 * b[0] as u128
         + a[2] as u128 * b19_4
         + a[3] as u128 * b19_3
         + a[4] as u128 * b19_2;

    t[2] = a[0] as u128 * b[2] as u128
         + a[1] as u128 * b[1] as u128
         + a[2] as u128 * b[0] as u128
         + a[3] as u128 * b19_4
         + a[4] as u128 * b19_3;

    t[3] = a[0] as u128 * b[3] as u128
         + a[1] as u128 * b[2] as u128
         + a[2] as u128 * b[1] as u128
         + a[3] as u128 * b[0] as u128
         + a[4] as u128 * b19_4;

    t[4] = a[0] as u128 * b[4] as u128
         + a[1] as u128 * b[3] as u128
         + a[2] as u128 * b[2] as u128
         + a[3] as u128 * b[1] as u128
         + a[4] as u128 * b[0] as u128;

    // Carry propagation
    let mut r = fe_zero();
    let mut carry: u128;
    carry = t[0] >> 51; r[0] = (t[0] & MASK51 as u128) as u64; t[1] += carry;
    carry = t[1] >> 51; r[1] = (t[1] & MASK51 as u128) as u64; t[2] += carry;
    carry = t[2] >> 51; r[2] = (t[2] & MASK51 as u128) as u64; t[3] += carry;
    carry = t[3] >> 51; r[3] = (t[3] & MASK51 as u128) as u64; t[4] += carry;
    carry = t[4] >> 51; r[4] = (t[4] & MASK51 as u128) as u64;
    r[0] += (carry * 19) as u64;
    carry = (r[0] >> 51) as u128; r[0] &= MASK51;
    r[1] += carry as u64;
    r
}

fn fe_sq(a: &Fe) -> Fe {
    fe_mul(a, a)
}

fn fe_mul121666(a: &Fe) -> Fe {
    let c: u128 = 121666;
    let mut t = [0u128; 5];
    t[0] = a[0] as u128 * c;
    t[1] = a[1] as u128 * c;
    t[2] = a[2] as u128 * c;
    t[3] = a[3] as u128 * c;
    t[4] = a[4] as u128 * c;

    let mut r = fe_zero();
    let mut carry: u128;
    carry = t[0] >> 51; r[0] = (t[0] & MASK51 as u128) as u64; t[1] += carry;
    carry = t[1] >> 51; r[1] = (t[1] & MASK51 as u128) as u64; t[2] += carry;
    carry = t[2] >> 51; r[2] = (t[2] & MASK51 as u128) as u64; t[3] += carry;
    carry = t[3] >> 51; r[3] = (t[3] & MASK51 as u128) as u64; t[4] += carry;
    carry = t[4] >> 51; r[4] = (t[4] & MASK51 as u128) as u64;
    r[0] += (carry * 19) as u64;
    carry = (r[0] >> 51) as u128; r[0] &= MASK51;
    r[1] += carry as u64;
    r
}

/// Full reduction mod p = 2^255 - 19
fn fe_reduce(h: &mut Fe) {
    // Carry chain
    let mut carry: u64;
    carry = h[0] >> 51; h[0] &= MASK51; h[1] += carry;
    carry = h[1] >> 51; h[1] &= MASK51; h[2] += carry;
    carry = h[2] >> 51; h[2] &= MASK51; h[3] += carry;
    carry = h[3] >> 51; h[3] &= MASK51; h[4] += carry;
    carry = h[4] >> 51; h[4] &= MASK51; h[0] += carry * 19;
    carry = h[0] >> 51; h[0] &= MASK51; h[1] += carry;

    // Conditional subtract p
    let mut q = (h[0] + 19) >> 51;
    q = (h[1] + q) >> 51;
    q = (h[2] + q) >> 51;
    q = (h[3] + q) >> 51;
    q = (h[4] + q) >> 51;

    h[0] += 19 * q;
    carry = h[0] >> 51; h[0] &= MASK51; h[1] += carry;
    carry = h[1] >> 51; h[1] &= MASK51; h[2] += carry;
    carry = h[2] >> 51; h[2] &= MASK51; h[3] += carry;
    carry = h[3] >> 51; h[3] &= MASK51; h[4] += carry;
    h[4] &= MASK51;
}

/// Compute a^(-1) mod p using Fermat's little theorem: a^(p-2)
fn fe_invert(a: &Fe) -> Fe {
    // p-2 = 2^255 - 21
    // Use addition chain
    let z2 = fe_sq(a);                     // a^2
    let z9 = {
        let t = fe_sq(&z2);                // a^4
        let t = fe_sq(&t);                 // a^8
        fe_mul(&t, a)                      // a^9
    };
    let z11 = fe_mul(&z9, &z2);            // a^11
    let z_5_0 = {
        let t = fe_sq(&z11);               // a^22
        fe_mul(&t, &z9)                    // a^31 = a^(2^5-1)
    };
    let z_10_0 = {
        let mut t = fe_sq(&z_5_0);
        for _ in 1..5 { t = fe_sq(&t); }
        fe_mul(&t, &z_5_0)                 // a^(2^10-1)
    };
    let z_20_0 = {
        let mut t = fe_sq(&z_10_0);
        for _ in 1..10 { t = fe_sq(&t); }
        fe_mul(&t, &z_10_0)                // a^(2^20-1)
    };
    let z_40_0 = {
        let mut t = fe_sq(&z_20_0);
        for _ in 1..20 { t = fe_sq(&t); }
        fe_mul(&t, &z_20_0)                // a^(2^40-1)
    };
    let z_50_0 = {
        let mut t = fe_sq(&z_40_0);
        for _ in 1..10 { t = fe_sq(&t); }
        fe_mul(&t, &z_10_0)                // a^(2^50-1)
    };
    let z_100_0 = {
        let mut t = fe_sq(&z_50_0);
        for _ in 1..50 { t = fe_sq(&t); }
        fe_mul(&t, &z_50_0)                // a^(2^100-1)
    };
    let z_200_0 = {
        let mut t = fe_sq(&z_100_0);
        for _ in 1..100 { t = fe_sq(&t); }
        fe_mul(&t, &z_100_0)               // a^(2^200-1)
    };
    let z_250_0 = {
        let mut t = fe_sq(&z_200_0);
        for _ in 1..50 { t = fe_sq(&t); }
        fe_mul(&t, &z_50_0)                // a^(2^250-1)
    };
    let mut t = fe_sq(&z_250_0);
    for _ in 1..5 { t = fe_sq(&t); }       // a^(2^255-32)
    fe_mul(&t, &z11)                        // a^(2^255-21) = a^(p-2)
}

/// Constant-time conditional swap
fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
    let mask = 0u64.wrapping_sub(swap); // 0 or 0xFFF...
    for i in 0..5 {
        let t = mask & (a[i] ^ b[i]);
        a[i] ^= t;
        b[i] ^= t;
    }
}

/// Montgomery ladder scalar multiplication
fn montgomery_ladder(scalar: &[u8; 32], u_point: &Fe) -> Fe {
    let mut x_2 = fe_one();
    let mut z_2 = fe_zero();
    let mut x_3 = *u_point;
    let mut z_3 = fe_one();

    let mut swap: u64 = 0;

    // Process bits from high to low (bit 254 down to 0)
    for pos in (0..255).rev() {
        let byte = scalar[pos / 8];
        let bit = ((byte >> (pos & 7)) & 1) as u64;

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
        z_2 = fe_mul(&e, &fe_add(&aa, &fe_mul121666(&e)));
    }

    cswap(swap, &mut x_2, &mut x_3);
    cswap(swap, &mut z_2, &mut z_3);

    // Return x_2 * z_2^(-1)
    fe_mul(&x_2, &fe_invert(&z_2))
}
