//! Network intents: ping, traceroute, netstat, resolve, net info, wlan

use crate::kprintln;
use super::parse_ip;

pub fn intent_ping(args: &str) {
    let host = args.trim();
    if host.is_empty() {
        kprintln!("[npk] Usage: ping <host or ip>");
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

    // Send ARP first to resolve gateway
    crate::net::arp::request([10, 0, 2, 2]);
    // Brief poll to get ARP reply
    for _ in 0..100_000 {
        crate::net::poll();
        core::hint::spin_loop();
    }

    crate::net::icmp::ping(ip, 1);

    // Poll for reply
    let t0 = crate::interrupts::ticks();
    loop {
        crate::net::poll();
        if crate::net::icmp::ping_received() {
            break;
        }
        let elapsed = crate::interrupts::ticks() - t0;
        if elapsed > 300 {
            kprintln!("[npk] Ping timeout");
            break;
        }
        core::hint::spin_loop();
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
    kprintln!();
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
}
