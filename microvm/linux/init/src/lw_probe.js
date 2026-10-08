
// Startup probe appended by microvm-init: LibreWolf's own startup times,
// relative to its process creation, as one line on /dev/kmsg at first paint.
try {
  let Cc = Components.classes, Ci = Components.interfaces;
  let su = Cc["@mozilla.org/toolkit/app-startup;1"].getService(Ci.nsIAppStartup);
  let os = Cc["@mozilla.org/observer-service;1"].getService(Ci.nsIObserverService);
  let emit = function () {
    let i = su.getStartupInfo();
    let p = i.process ? i.process.getTime() : 0;
    let at = k => (i[k] ? (i[k].getTime() - p) : -1);
    let line = "<0>[lw-startup] main=" + at("main") + " window=" + at("createTopLevelWindow") +
               " paint=" + at("firstPaint") + " (ms after process start)\n";
    let f = Cc["@mozilla.org/file/local;1"].createInstance(Ci.nsIFile);
    f.initWithPath("/dev/kmsg");
    let s = Cc["@mozilla.org/network/file-output-stream;1"].createInstance(Ci.nsIFileOutputStream);
    s.init(f, 0x02, 0, 0);
    s.write(line, line.length);
    s.close();
  };
  let obs = { observe() { os.removeObserver(obs, "widget-first-paint"); emit(); } };
  os.addObserver(obs, "widget-first-paint");
} catch (e) {}
