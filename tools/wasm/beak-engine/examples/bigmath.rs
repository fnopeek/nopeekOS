//! Die Bignum-Rechnung gegen node halten, OHNE die Engine dazwischen.
//! Ein Fehler in `Big` ist sonst nur als merkwuerdiges JS-Ergebnis sichtbar.
use beak_engine::js::bigint::Big;

fn main() {
    let vals = [
        "0", "1", "-1", "255", "-255", "4294967296", "-4294967296",
        "123456789012345678901234567890", "-98765432109876543210",
        "18446744073709551616", "7", "-7", "3", "-3", "1000000007",
        "-170141183460469231731687303715884105728",
    ];
    let mut out = String::new();
    for a in vals {
        for b in vals {
            let (x, y) = (Big::parse(a).unwrap(), Big::parse(b).unwrap());
            out.push_str(&x.add(&y).to_string_radix(10)); out.push(' ');
            out.push_str(&x.sub(&y).to_string_radix(10)); out.push(' ');
            out.push_str(&x.mul(&y).to_string_radix(10)); out.push(' ');
            match x.div_rem(&y) {
                Some((q, r)) => { out.push_str(&q.to_string_radix(10)); out.push(' ');
                                  out.push_str(&r.to_string_radix(10)); out.push(' '); }
                None => out.push_str("DIV0 DIV0 "),
            }
            out.push_str(&x.bitop(&y, 0).to_string_radix(10)); out.push(' ');
            out.push_str(&x.bitop(&y, 1).to_string_radix(10)); out.push(' ');
            out.push_str(&x.bitop(&y, 2).to_string_radix(10)); out.push(' ');
            out.push_str(match x.cmp(&y) {
                core::cmp::Ordering::Less => "<", core::cmp::Ordering::Equal => "=",
                core::cmp::Ordering::Greater => ">" });
            out.push('\n');
        }
        out.push_str(&x_not(a)); out.push('\n');
    }
    for a in vals {
        let x = Big::parse(a).unwrap();
        for n in [0u64, 1, 5, 31, 32, 33, 64, 100] {
            out.push_str(&x.shl(n).to_string_radix(10)); out.push(' ');
            out.push_str(&x.shr(n).to_string_radix(10)); out.push(' ');
        }
        for b in [8u64, 16, 32, 64, 1] {
            out.push_str(&x.as_n(b, true).to_string_radix(10)); out.push(' ');
            out.push_str(&x.as_n(b, false).to_string_radix(10)); out.push(' ');
        }
        for r in [2u32, 8, 16, 36] { out.push_str(&x.to_string_radix(r)); out.push(' '); }
        out.push('\n');
    }
    for e in 0..40u64 {
        let b = Big::parse("3").unwrap();
        out.push_str(&b.pow(&Big::from_u64(e)).unwrap().to_string_radix(10));
        out.push('\n');
    }
    print!("{out}");
}

fn x_not(a: &str) -> String { Big::parse(a).unwrap().not().to_string_radix(10) }
