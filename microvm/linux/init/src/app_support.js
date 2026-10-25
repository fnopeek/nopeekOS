// about:support probe appended by microvm-init with `npksupport`: the
// graphics and media sections of Troubleshoot.snapshot() (what about:support
// shows) as `[about]` lines on /dev/kmsg, 20 s after startup and again
// after 90 s, when a video has had time to start its decoder.
try {
  let Cc = Components.classes, Ci = Components.interfaces;
  let kmsg = function (text) {
    let f = Cc["@mozilla.org/file/local;1"].createInstance(Ci.nsIFile);
    f.initWithPath("/dev/kmsg");
    let s = Cc["@mozilla.org/network/file-output-stream;1"].createInstance(Ci.nsIFileOutputStream);
    s.init(f, 0x02, 0, 0);
    // One record per write; kmsg records are short.
    for (let line of text.split("\n")) {
      for (let at = 0; at < line.length; at += 700) {
        let rec = "<0>[about] " + line.substr(at, 700) + "\n";
        s.write(rec, rec.length);
      }
    }
    s.close();
  };
  let show = function (name, value) {
    if (value === undefined || value === null) return;
    let text = typeof value == "string" ? value : JSON.stringify(value);
    kmsg(name + ": " + text);
  };
  let dump = function (when) {
    let { Troubleshoot } = ChromeUtils.importESModule("resource://gre/modules/Troubleshoot.sys.mjs");
    Troubleshoot.snapshot().then(snap => {
      kmsg("---- about:support at " + when + " ----");
      let g = snap.graphics || {};
      for (let k of ["compositing", "windowLayerManagerType", "windowLayerManagerRemote", "webgl1Renderer",
                     "webgl1Version", "webgl2Renderer", "webgl2Version", "webgpuDefaultAdapter",
                     "supportsHardwareH264", "currentAudioBackend"]) {
        show("graphics." + k, g[k]);
      }
      let fl = g.featureLog || {};
      for (let f of fl.features || []) {
        let steps = (f.log || []).map(l => l.type + ":" + l.status + (l.message ? " (" + l.message + ")" : "")).join("; ");
        kmsg("feature " + f.name + " = " + f.status + (steps ? " | " + steps : ""));
      }
      for (let f of fl.fallbacks || []) {
        kmsg("fallback " + f.name + ": " + f.message);
      }
      show("graphics.crashGuards", g.crashGuards);
      show("graphics.failures", g.failures);
      show("graphics.indices", g.indices);
      let m = snap.media || {};
      show("media.codecSupportInfo", m.codecSupportInfo);
      for (let k in m) {
        if (k != "codecSupportInfo" && k != "audioOutputDevices" && k != "audioInputDevices") show("media." + k, m[k]);
      }
      kmsg("---- end ----");
    }, e => kmsg("snapshot failed: " + e));
  };
  let os = Cc["@mozilla.org/observer-service;1"].getService(Ci.nsIObserverService);
  let later = function (ms, when) {
    let t = Cc["@mozilla.org/timer;1"].createInstance(Ci.nsITimer);
    t.initWithCallback({ notify() { dump(when); } }, ms, Ci.nsITimer.TYPE_ONE_SHOT);
    return t;
  };
  let timers = [];
  let obs = {
    observe() {
      os.removeObserver(obs, "browser-delayed-startup-finished");
      timers.push(later(20000, "+20 s"), later(90000, "+90 s"));
    }
  };
  os.addObserver(obs, "browser-delayed-startup-finished");
} catch (e) {}
