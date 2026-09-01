fn main() {
    use beak_engine::js::regexp::Regex;
    let cases: &[(&str, &str, &str, Option<&str>)] = &[
        ("abc", "", "xxabcyy", Some("abc")),
        ("a+", "", "baaad", Some("aaa")),
        ("a+?", "", "baaad", Some("a")),
        ("^b", "", "abc", None),
        ("^b", "m", "a\nbc", Some("b")),
        ("[a-c]+", "", "zzabcz", Some("abc")),
        ("[^a-c]+", "", "abzz", Some("zz")),
        (r"\d{2,3}", "", "x1234", Some("123")),
        (r"\bfoo\b", "", "a foo b", Some("foo")),
        ("(ab)+", "", "ababx", Some("abab")),
        ("(a)(b)", "", "zab", Some("ab")),
        (r"(a)\1", "", "xaay", Some("aa")),
        ("a(?=b)", "", "ac ab", Some("a")),
        ("a(?!b)", "", "ab ac", Some("a")),
        ("(?<=x)y", "", "zy xy", Some("y")),
        ("colou?r", "", "color", Some("color")),
        ("A", "i", "xax", Some("a")),
        ("a.c", "", "a\nc", None),
        ("a.c", "s", "a\nc", Some("a\nc")),
        ("(?:ab)+", "", "abab", Some("abab")),
        ("(?<n>a)(?<m>b)", "", "ab", Some("ab")),
        (r"^\s*$", "", "   ", Some("   ")),
        ("x|y|z", "", "aaz", Some("z")),
        (r"\$\d+", "", "cost $42", Some("$42")),
        ("(a+)+$", "", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaab", None),  // katastrophal
    ];
    let (mut ok, mut bad) = (0, 0);
    for (pat, fl, hay, want) in cases {
        let chars: Vec<char> = hay.chars().collect();
        let r = match Regex::new(pat, fl) { Ok(r) => r, Err(e) => { println!("PARSE {pat}: {e}"); bad += 1; continue } };
        let got = r.exec(&chars, 0).and_then(|m| m.caps[0].map(|(a, b)| chars[a..b].iter().collect::<String>()));
        if got.as_deref() == *want { ok += 1 } else {
            bad += 1;
            println!("FAIL /{pat}/{fl} auf {hay:?}: erwartet {want:?}, bekam {got:?}");
        }
    }
    println!("{ok} ok, {bad} fehlgeschlagen");
}
