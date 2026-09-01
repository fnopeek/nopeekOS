fn main() {
    let script = r##"
      if (!/a+/.test("baaa")) throw new Error("test");
      if ("a1b2".replace(/\d/g, "#") !== "a#b#") throw new Error("replace-g");
      if ("a1b2".replace(/(\d)/g, "[$1]") !== "a[1]b[2]") throw new Error("replace-$1");
      if ("2026-09-01".split("-").length !== 3) throw new Error("split-str");
      if ("a1b".split(/(\d)/).join("|") !== "a|1|b") throw new Error("split-caps");
      var m = /(\w+)@(\w+)/.exec("mail bob@host x");
      if (!m || m[1] !== "bob" || m[2] !== "host") throw new Error("exec-groups");
      if (m.index !== 5) throw new Error("index " + m.index);
      var g = /o/g, n = 0;
      while (g.exec("foo boo")) n++;
      if (n !== 4) throw new Error("lastIndex-Schleife: " + n);
      if ("xAx".match(/a/i)[0] !== "A") throw new Error("ignoreCase");
      if ("abc".search(/b/) !== 1) throw new Error("search");
      if ("aaa".replace(/a/g, function(x, i){ return i }) !== "012") throw new Error("replace-fn");
      var nm = /(?<y>\d{4})-(?<m>\d{2})/.exec("am 2026-09-01");
      if (nm.groups.y !== "2026" || nm.groups.m !== "09") throw new Error("named");
      if (String(/ab/gi) !== "/ab/gi") throw new Error("toString: " + String(/ab/gi));
      if ("abc".replace(/x*/g, "-") !== "-a-b-c-") throw new Error("leerer Treffer: " + "abc".replace(/x*/g, "-"));
      if (new RegExp("a\\d").test("a5") !== true) throw new Error("ctor");
    "##;
    match beak_engine::js::run(script, false) {
        Ok(()) => println!("RegExp in JS: alle Proben ok"),
        Err(e) => println!("FEHLER: {e}"),
    }
}
