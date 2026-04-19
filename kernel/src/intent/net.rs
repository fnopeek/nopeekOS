//! Network intents: ping, traceroute, netstat, resolve, net info

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
                kprintln!("    State   UP (no IP)");
            } else {
                kprintln!("    IPv4    {}.{}.{}.{}/{}", ip[0], ip[1], ip[2], ip[3], prefix);
                kprintln!("    State   UP");
            }
        } else {
            kprintln!("    State   DOWN");
        }
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
