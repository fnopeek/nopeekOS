fn main() {
    let html = r#"<html><head><title>T</title></head><body>
      <div id="main" class="wrap big"><p class="x">eins</p><p class="x">zwei</p></div>
      <ul><li>a</li><li>b</li></ul></body></html>"#;
    let dom = beak_engine::dom::parse(html);
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let mut s = beak_engine::js::Session::new(u64::MAX);
    s.interp.set_document(doc);
    let script = r##"
      var d = document;
      if (d.body === null) throw new Error("kein body");
      if (d.body !== d.body) throw new Error("Identitaet");
      var m = d.getElementById("main");
      if (!m) throw new Error("getElementById");
      if (m.tagName !== "DIV") throw new Error("tagName " + m.tagName);
      if (m.className !== "wrap big") throw new Error("className");
      if (m.getAttribute("id") !== "main") throw new Error("getAttribute");
      if (m.children.length !== 2) throw new Error("children " + m.children.length);
      if (d.querySelectorAll("p.x").length !== 2) throw new Error("querySelectorAll");
      if (d.querySelector("#main .x").textContent !== "eins") throw new Error("descendant-sel");
      if (d.getElementsByTagName("li").length !== 2) throw new Error("byTagName");
      if (m.parentNode !== d.body) throw new Error("parentNode");
      if (m.children[0].nextSibling !== m.children[1]) throw new Error("nextSibling");
      m.classList.add("neu");
      if (!m.classList.contains("neu")) throw new Error("classList.add");
      m.classList.remove("big");
      if (m.className !== "wrap neu") throw new Error("classList.remove: " + m.className);
      var n = d.createElement("span");
      n.textContent = "drei";
      m.appendChild(n);
      if (m.children.length !== 3) throw new Error("appendChild");
      if (d.querySelector("#main span").textContent !== "drei") throw new Error("neuer Knoten nicht findbar");
      n.remove();
      if (m.children.length !== 2) throw new Error("remove");
      var hits = 0;
      d.body.addEventListener("click", function(){ hits++ });
      if (d.body.matches("body") !== true) throw new Error("matches");
      var t = 0;
      d.querySelectorAll(".x").forEach(function(e){ t += e.textContent.length });
      if (t !== 8) throw new Error("forEach ueber Treffer: " + t);
    "##;
    let r = match beak_engine::js::parse(script, false) {
        Err(e) => Err(format!("SyntaxError: {} @{}", e.msg, e.at)),
        Ok(p) => s.run(&p),
    };
    match r {
        Ok(()) => println!("DOM-Bindung: alle Proben ok"),
        Err(e) => println!("FEHLER: {e}"),
    }
}
