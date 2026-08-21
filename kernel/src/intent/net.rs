//! Network intents: ping, traceroute, netstat, resolve, net info, wlan

use crate::kprintln;
use super::parse_ip;

pub fn intent_ping(args: &str) {
    let mut it = args.split_whitespace();
    let host = it.next().unwrap_or("");
    // One probe is a coin flip on a radio link — the same lesson ARP taught us
    // today. A single lost echo said "the host is down" when the host was fine,
    // so the default is four and the summary says how many came back.
    let count: u16 = it.next().and_then(|s| s.parse().ok()).unwrap_or(4).clamp(1, 32);
    if host.is_empty() {
        kprintln!("[npk] Usage: ping <host or ip> [count]");
        return;
    }

    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        match crate::net::dns::resolve(host) {
            Some(ip) => {
                kprintln!("[npk] {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
                ip
            }
            None => {
                kprintln!("[npk] Could not resolve '{}'", host);
                return;
            }
        }
    };

    // Resolve the next hop properly instead of firing a blind request at a
    // hardcoded QEMU address and spinning 100 000 times: that helped only under
    // QEMU and cost 100 ms everywhere else.
    let hop = crate::net::ipv4::arp_target_for(ip);
    if crate::net::arp::resolve(hop, 50).is_none() {
        kprintln!("[npk] ping: {}.{}.{}.{} did not answer ARP - sending anyway",
            hop[0], hop[1], hop[2], hop[3]);
    }

    let mut got = 0u16;
    let mut best = u64::MAX;
    let mut worst = 0u64;
    let mut total = 0u64;
    for seq in 1..=count {
        let _ = crate::net::icmp::ping_received(); // clear any stale flag
        let t0 = crate::interrupts::ticks();
        crate::net::icmp::ping(ip, seq);
        let mut hit = false;
        while crate::interrupts::ticks().wrapping_sub(t0) < 100 { // 1 s per probe
            crate::net::poll();
            if crate::net::icmp::ping_received() { hit = true; break; }
            core::hint::spin_loop();
        }
        if hit {
            let ms = crate::interrupts::ticks().wrapping_sub(t0) * 10;
            got += 1;
            total += ms;
            if ms < best { best = ms; }
            if ms > worst { worst = ms; }
        } else {
            kprintln!("[npk] seq={} timeout", seq);
        }
        // Space the probes out; back to back they share one fate on a bad link.
        if seq < count {
            let until = crate::interrupts::ticks() + 20;
            while crate::interrupts::ticks() < until { crate::net::poll(); }
        }
    }

    kprintln!("[npk] {} sent, {} received, {}% lost{}",
        count, got, (count - got) as u32 * 100 / count as u32,
        if got > 0 { "" } else { " — nothing came back" });
    if got > 0 {
        kprintln!("[npk] rtt min/avg/max = {}/{}/{} ms (10 ms resolution)",
            best, total / got as u64, worst);
    }
}

pub fn intent_traceroute(args: &str) {
    let target = args.trim();
    if target.is_empty() {
        kprintln!("[npk] Usage: traceroute <ip or hostname>");
        return;
    }

    let ip = if let Some(ip) = parse_ip(target) {
        ip
    } else {
        match crate::net::dns::resolve(target) {
            Some(ip) => {
                kprintln!("[npk] {} -> {}.{}.{}.{}", target, ip[0], ip[1], ip[2], ip[3]);
                ip
            }
            None => { kprintln!("[npk] Could not resolve '{}'", target); return; }
        }
    };

    // ARP resolve gateway
    crate::net::arp::request([10, 0, 2, 2]);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

    kprintln!("[npk] Traceroute to {}.{}.{}.{} (max 20 hops)", ip[0], ip[1], ip[2], ip[3]);

    for ttl in 1..=20u8 {
        crate::net::icmp::ping_ttl(ip, ttl as u16, ttl);

        let t0 = crate::interrupts::ticks();
        let mut _found = false;

        loop {
            crate::net::poll();

            if let Some(from) = crate::net::icmp::ttl_expired_from() {
                kprintln!("  {:>2}  {}.{}.{}.{}", ttl, from[0], from[1], from[2], from[3]);
                _found = true;
                break;
            }
            if crate::net::icmp::ping_received() {
                kprintln!("  {:>2}  {}.{}.{}.{} (destination)", ttl, ip[0], ip[1], ip[2], ip[3]);
                return; // reached destination
            }
            if crate::interrupts::ticks() - t0 > 100 { // 1s per hop
                kprintln!("  {:>2}  *", ttl);
                _found = true;
                break;
            }
            core::hint::spin_loop();
        }
    }
}

pub fn intent_netstat() {
    let conns = crate::net::tcp::list_connections();
    kprintln!();
    kprintln!("  Active TCP Connections");
    kprintln!("  ─────────────────────");
    if conns.is_empty() {
        kprintln!("  (none)");
    } else {
        kprintln!("  {:>6}  {:>21}  {}", "Local", "Remote", "State");
        for (lport, rip, rport, state) in &conns {
            kprintln!("  {:>6}  {}.{}.{}.{}:{:<5}  {}",
                lport, rip[0], rip[1], rip[2], rip[3], rport, state);
        }
    }
    bridge_report();
    kprintln!();
}

/// The microVM's side of the wire. Prints itself only when the bridge has
/// something to say — not gated on `vm_active()`, which despite the name means
/// "a VM is running on the COOPERATIVE Core-0 path" and is therefore always
/// false for the fiber-mode guest this report exists for. The counters are their
/// own gate: they are zeroed at VM teardown, so a host without a guest stays
/// silent, and a guest whose network just died still answers.
///
/// Read it as a decision tree, top to bottom. `guest -> host` still climbing
/// says the guest is alive and the masquerade is taking its packets; if
/// `host -> guest` has stopped with it, the replies are not coming back or the
/// mapping no longer matches. If BOTH climb and the guest still sees nothing,
/// the loss is in delivery, and the three `lost` numbers say which wall: a full
/// staging queue is backpressure, a full table means no new connection can open
/// at all, and an egress refusal means the frame never reached the wire. Call it
/// twice a few seconds apart — the per-second figures come from the gap.
fn bridge_report() {
    let b = crate::microvm::devices::nat::bridge_stats();
    let up = crate::microvm::guest_running();
    if !up && !b.active && b.rx_pkts == 0 && b.tx_pkts == 0 && b.live == 0 {
        return;
    }
    kprintln!();
    if up && b.frames_in == 0 {
        // Say what is true and no more. `tx_pkts` counts only MASQUERADED
        // egress, so gating on it called a guest that had sent nothing but ARP
        // "silent". `frames_in` is counted at the door, before classification.
        kprintln!("  microVM bridge — guest up {} s, {} frames from it",
            b.up_s, b.frames_in);
        kprintln!("  ─────────────────────────────");
        if b.kicks == 0 {
            kprintln!("  the guest has not rung the TX doorbell once. Either it has");
            kprintln!("  nothing to send yet (userspace still booting — check `up`");
            kprintln!("  above) or its queue-1 kick never reaches our MMIO handler.");
        } else {
            kprintln!("  {} doorbells, but its TX ring was empty every time —", b.kicks);
            kprintln!("  we are looking at the ring wrong, not at the masquerade.");
        }
        return;
    }
    kprintln!("  microVM bridge (L3 masquerade){}",
        if b.active { "" } else { "  — idle, no flow yet" });
    kprintln!("  ─────────────────────────────");
    if b.window_ms > 0 {
        kprintln!("  guest → host  {:>8} pkt  {:>7} KB   {:>6} pkt/s",
            b.tx_pkts, b.tx_bytes / 1024, b.tx_pps);
        kprintln!("  host → guest  {:>8} pkt  {:>7} KB   {:>6} pkt/s   (over {} ms)",
            b.rx_pkts, b.rx_bytes / 1024, b.rx_pps, b.window_ms);
    } else {
        kprintln!("  guest → host  {:>8} pkt  {:>7} KB", b.tx_pkts, b.tx_bytes / 1024);
        kprintln!("  host → guest  {:>8} pkt  {:>7} KB   (run again for a rate)",
            b.rx_pkts, b.rx_bytes / 1024);
    }
    kprintln!("  from guest    {} frames in {} doorbells   {} arp  {} non-ip   (up {} s)",
        b.frames_in, b.kicks, b.arp_in, b.other_in, b.up_s);
    kprintln!("  flows         {} tcp  {} udp opened   {} live of {}",
        b.flows_tcp, b.flows_udp, b.live, b.cap);
    kprintln!("  staging       queue {} (peak {} of {})   guest rx ring low-water {}",
        b.iq, b.iq_hi, b.iq_cap, b.rxring_min);
    kprintln!("  lost          {} queue-full   {} TABLE-FULL   {} egress-refused   {} guest-ring-full",
        b.drop_queue, b.drop_table, b.drop_egress, b.inject_false);
    kprintln!("  delivery      rx wait avg {} us  max {} us   {} irq raised",
        b.rxlat_avg_us, b.rxlat_max_us, b.net_irq);
    if b.gro {
        kprintln!("  gro           on   {} frames  {} segments merged",
            b.gro_frames, b.gro_segs);
    }
    // The vCPU that pumps this bridge is the SAME fiber that copies the guest's
    // framebuffer, ~8 MB a frame, inline on its MMIO exit — on Intel, where the
    // net has no off-vCPU worker to fall back on. Printed next to `rx wait` on
    // purpose: a browser that starts painting and a delivery latency that goes
    // to milliseconds in the same breath is the whole story, and neither number
    // says it alone.
    kprintln!("  gpu on vcpu   {} KB copied in {} transfers   {} KB/s now",
        b.gpu_kb, b.gpu_xfers, b.gpu_kbps);
    let (no_link, by_driver) = crate::netdev::tx_reject_stats();
    if no_link > 0 || by_driver > 0 {
        kprintln!("  egress        REFUSED since boot: {} no-link  {} by the driver",
            no_link, by_driver);
    }
}

/// `wlan` — one screen with everything needed to diagnose the WiFi link.
///
/// Two halves that must be read together: what the KERNEL sees of the WASM NIC
/// (queues, drops, active interface) and what the DRIVER reports about the air
/// (rates, retries, airtime). A link that is slow because the negotiated rate is
/// legacy looks nothing like one that is slow because the TX queue keeps
/// overflowing, and only both halves side by side tell them apart.
///
/// `wlan reset` zeroes the kernel counters so a single speed test can be
/// measured on its own; the driver's counters are cumulative and it reports
/// throughput over its own 1-second window regardless.
// ── `wlan set` — one file, one key at a time ─────────────────────────────
//
// The wifi settings live in a single `key: value` object, but `store` REPLACES
// what it writes: typing a second key from the loop dropped the first, and no
// app may write this file (a module only gets `sys/config/<its own name>`).
// That left the file creatable and not editable. So the read-modify-write lives
// here, where the capability already exists.

const WIFI_CFG: &str = "sys/config/wifi";

/// The keys the wifi stack actually reads. A typo that wrote silently is how an
/// afternoon gets spent measuring a setting that never arrived.
const WIFI_KEYS: &[&str] = &["ssid", "band", "ampdu", "txagg", "ht40", "vht", "bawin", "ps", "btcoex", "settle_ms"];

fn wlan_set_usage() {
    kprintln!("[wlan] Usage: wlan set <key> <value> | wlan unset <key>");
    kprintln!("[wlan]   ssid: <name>        the network to join (else: loudest)");
    kprintln!("[wlan]   band: 5 | 2.4 | auto");
    kprintln!("[wlan]   ampdu: on | off     RX aggregation (throughput)");
    kprintln!("[wlan]   txagg: on | off     TX aggregation (EXPERIMENT: tid_disable_tx=0)");
    kprintln!("[wlan]   ht40: on | off      40 MHz (measured best: on)");
    kprintln!("[wlan]   vht: on | off       80 MHz (needs ht40 on; measured SLOWER than 40)");
    kprintln!("[wlan]   bawin: <n>          cap the RX reorder window (0/unset = what the AP asks)");
    kprintln!("[wlan]   ps: on | off        power save (default off = CAM)");
    kprintln!("[wlan]   btcoex: on | off");
    kprintln!("[wlan]   settle_ms: <n>      pause before the first scan");
    kprintln!("[wlan] the passphrase is separate: store /sys/config/wifi_psk <pass>");
}

pub fn intent_wlan_set(args: &str, cap_id: crate::capability::CapId) {
    let a = args.trim();
    let (rest, remove) = if let Some(r) = a.strip_prefix("unset") {
        (r.trim(), true)
    } else if let Some(r) = a.strip_prefix("set") {
        (r.trim(), false)
    } else {
        wlan_set_usage();
        return;
    };
    if rest.is_empty() {
        wlan_set_usage();
        return;
    }
    // Only the first space splits: an SSID may contain them.
    let (key, value) = match rest.split_once(' ') {
        Some((k, v)) => (k.trim().trim_end_matches(':'), v.trim()),
        None => (rest.trim_end_matches(':'), ""),
    };
    if !WIFI_KEYS.contains(&key) {
        kprintln!("[wlan] unknown key '{}'", key);
        wlan_set_usage();
        return;
    }
    if !remove && value.is_empty() {
        wlan_set_usage();
        return;
    }

    let current = crate::npkfs::fetch(WIFI_CFG).map(|(d, _)| d).unwrap_or_default();
    let mut out = alloc::string::String::new();
    let mut hit = false;
    for line in core::str::from_utf8(&current).unwrap_or("").lines() {
        let this = line.split_once(':').map(|(k, _)| k.trim()).unwrap_or("");
        if this == key {
            hit = true;
            if remove {
                continue;
            }
            out.push_str(key);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        } else if !line.trim().is_empty() {
            out.push_str(line.trim());
            out.push('\n');
        }
    }
    if !hit {
        if remove {
            kprintln!("[wlan] '{}' was not set", key);
            return;
        }
        out.push_str(key);
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }

    super::ensure_parents("sys/config");
    match crate::npkfs::upsert(WIFI_CFG, out.as_bytes(), cap_id) {
        Ok(_) => {
            kprintln!("[wlan] {} now reads:", WIFI_CFG);
            for line in out.lines() {
                kprintln!("[wlan]   {}", line);
            }
            kprintln!("[wlan] the driver reads this at start-up - restart it to apply");
        }
        Err(e) => kprintln!("[wlan] write failed: {}", e),
    }
}

pub fn intent_wlan(args: &str) {
    if args.trim() == "reset" {
        crate::netdev::wasm_nic_stats_reset();
        kprintln!("[wlan] kernel-side counters cleared");
        return;
    }

    let s = crate::netdev::wasm_nic_stats();
    let registered = crate::netdev::wasm_nic_available();
    let link = crate::netdev::wasm_nic_link_up();
    let prefer = crate::config::get("net_prefer").unwrap_or_else(|| "lan".into());
    let active = match crate::netdev::active() {
        crate::netdev::Active::Intel => "intel (wired)",
        crate::netdev::Active::Rtl => "rtl8153 (usb-lan)",
        crate::netdev::Active::Wasm => "wlan (wifi)",
        crate::netdev::Active::Virtio => "virtio-net",
        crate::netdev::Active::None => "none",
    };

    kprintln!();
    kprintln!("  Kernel view");
    kprintln!("  ───────────");
    kprintln!("  wlan       {}, carrier {}",
        if registered { "registered" } else { "NOT registered — driver not running" },
        if link { "UP" } else { "DOWN" });
    kprintln!("  routing    active={}  net_prefer={}", active, prefer.trim());
    kprintln!("  tx queue   enq {}  deq {}  backlog {} B", s.tx_enqueued, s.tx_dequeued, s.tx_backlog);
    // Frames the TX path REFUSED, as opposed to sent and unanswered. The
    // counters existed since the DNS hunt that motivated them and were never
    // printed, so a SYN that never reached the air still looked exactly like a
    // SYN the peer ignored — which is the question `connect timeout: state
    // SynSent` leaves open.
    let (no_link, tx_err) = crate::netdev::tx_reject_stats();
    if no_link > 0 || tx_err > 0 {
        kprintln!("  tx refused  no-link {}  by-driver {} (never reached the air)",
                  no_link, tx_err);
    }
    if s.tx_drops_oversize > 0 {
        kprintln!("  tx OVERSIZE {} frames past the {}-byte MTU — a BUG upstream, not congestion",
                  s.tx_drops_oversize, crate::netdev::MTU);
    }
    kprintln!("  tx drops   aqm {} (codel, latency control)  full {} (driver too slow)",
        s.tx_drops_aqm, s.tx_drops_full);
    kprintln!("  rx ring    in {}  dropped {} (ring full — driver outran core-0 drain)",
        s.rx_to_ring, s.rx_ring_drops);

    // Whether there is an address at all. "WiFi is up but there is no DHCP
    // lease" and "the link never actually came up" look identical from a
    // terminal, and they need opposite fixes.
    let ip = crate::net::arp::our_ip();
    if ip == [0, 0, 0, 0] {
        kprintln!("  address    NONE — no lease (link must be AUTHORIZED first)");
    } else {
        kprintln!("  address    {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    }

    // The control channel between driver and supplicant. A dropped message here
    // is a dropped 4-way handshake step, and events piling up means wifid has
    // stopped reading — the usual reason a link associates but never authorizes.
    let c = crate::wifi::stats();
    kprintln!("  ctrl chan  driver→wifid {} sent, {} queued, {} DROPPED",
        c.events_sent, c.events_queued, c.events_dropped);
    kprintln!("             wifid→driver {} sent, {} queued, {} DROPPED",
        c.cmds_sent, c.cmds_queued, c.cmds_dropped);
    if c.events_sent > 0 && c.cmds_sent == 0 {
        // Only meaningful once the driver has actually handed over something
        // that needs an answer. A lone READY with no reply means the AP never
        // started the handshake — the supplicant has nothing to answer yet, and
        // blaming it here sent the last diagnosis down the wrong path.
        kprintln!("             (no reply yet — expected while the AP has not sent EAPOL msg1)");
    }

    // The driver half. Printed verbatim: the kernel does not know what an
    // AX200 rate code means, and should not have to.
    let mut any = false;
    for name in crate::drivers::report::names() {
        if let Some((text, at_ms)) = crate::drivers::report::get(&name) {
            let now = crate::interrupts::ticks().saturating_mul(10);
            let age = now.saturating_sub(at_ms);
            kprintln!();
            kprintln!("  Driver report — {} ({} ms ago)", name, age);
            kprintln!("  ───────────────");
            for line in text.lines() {
                kprintln!("  {}", line);
            }
            any = true;
        }
    }
    if !any {
        kprintln!();
        kprintln!("  (no driver report — the driver is not running, or predates");
        kprintln!("   npk_driver_report; start it with 'driver wifi_ax200')");
    }

    // wifid runs in an invisible autostart window, so its log is the only place
    // the supplicant's side of the handshake is visible at all. Show the tail —
    // the interesting lines ("missed READY", "4-way FAILED", "Idle") are there.
    kprintln!();
    kprintln!("  Supplicant log — sys/log/wifid (tail)");
    kprintln!("  ───────────────");
    match crate::npkfs::fetch("sys/log/wifid") {
        Ok((bytes, _)) => {
            let text = alloc::string::String::from_utf8_lossy(&bytes);
            let lines: alloc::vec::Vec<&str> =
                text.lines().filter(|l| !l.trim().is_empty()).collect();
            let start = lines.len().saturating_sub(12);
            for line in &lines[start..] {
                kprintln!("  {}", line);
            }
            if lines.is_empty() {
                kprintln!("  (empty)");
            }
        }
        Err(_) => kprintln!("  (no log — wifid has not run since the last install)"),
    }
    kprintln!();
}

pub fn intent_resolve(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: resolve <hostname>");
        return;
    }
    match crate::net::dns::resolve(name) {
        Some(ip) => kprintln!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
        None => kprintln!("[npk] Could not resolve '{}'", name),
    }
}

pub fn intent_net_info() {
    let ifaces = crate::netdev::list();
    if ifaces.is_empty() {
        kprintln!("[npk] No network interfaces available");
        return;
    }

    let ip = crate::net::arp::our_ip();
    let prefix = crate::net::ipv4::prefix_len();

    kprintln!();
    kprintln!("  Interfaces");
    kprintln!("  ──────────");
    for iface in &ifaces {
        kprintln!("  {}  {}", iface.name, iface.driver);
        kprintln!("    MAC     {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            iface.mac[0], iface.mac[1], iface.mac[2],
            iface.mac[3], iface.mac[4], iface.mac[5]);
        if iface.primary {
            if ip == [0, 0, 0, 0] {
                kprintln!("    IPv4    (no lease)");
            } else {
                kprintln!("    IPv4    {}.{}.{}.{}/{}", ip[0], ip[1], ip[2], ip[3], prefix);
            }
        }
        // State reflects the real carrier (WiFi: associated + keyed; wired:
        // present), not whether this is the primary interface.
        kprintln!("    State   {}", if iface.link_up { "UP" } else { "DOWN" });
        kprintln!();
    }

    let gw = crate::net::ipv4::gateway();
    let dns = crate::net::dns::server();
    let primary_name = ifaces.iter().find(|i| i.primary).map(|i| i.name).unwrap_or("?");

    kprintln!("  Routing");
    kprintln!("  ───────");
    if ip == [0, 0, 0, 0] {
        kprintln!("  Default  (none)");
    } else {
        kprintln!("  Default  {}.{}.{}.{} via {}", gw[0], gw[1], gw[2], gw[3], primary_name);
    }
    kprintln!("  DNS      {}.{}.{}.{}", dns[0], dns[1], dns[2], dns[3]);
    kprintln!();

    // The next-hop table. A wrong MAC here is invisible from the outside — it
    // looks exactly like the far end being down, and it takes out everything
    // that leaves the segment while LAN-direct traffic keeps working. The
    // gateway's row is the one to check first, so it is marked.
    let table = crate::net::arp::table();
    kprintln!("  Neighbours");
    kprintln!("  ──────────");
    if table.is_empty() {
        kprintln!("  (none learned yet)");
    }
    for (nip, nmac, age) in table {
        kprintln!("  {}.{}.{}.{}{}  {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  {} s ago",
            nip[0], nip[1], nip[2], nip[3],
            if nip == gw { " (gateway)" } else { "" },
            nmac[0], nmac[1], nmac[2], nmac[3], nmac[4], nmac[5], age);
    }
    kprintln!();
}
