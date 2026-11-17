//! NTP — Network Time Protocol (SNTP client)
//!
//! Simple NTP query to get wall-clock time.
//! Uses UDP port 123, parses NTP v4 response.
//!
//! SNTP is unauthenticated, and certificate validity rests on this clock.
//! A reply counts only if it comes from the asked server, is a server reply
//! (mode 4) from a synchronised source (stratum 1..15, leap != 3), and
//! echoes our random transmit timestamp — so an off-path forger has to
//! guess a random port and 64 random bits.

use super::udp;

const NTP_PORT: u16 = 123;
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800; // seconds from 1900 to 1970

/// Waits of the three attempts, in ticks: 1 s, 2 s, 4 s.
const ATTEMPT_TICKS: [u64; 3] = [100, 200, 400];

/// The time from an NTP server: Unix nanoseconds at the moment the reply
/// arrived (RFC 5905's offset with the round trip split evenly, the
/// server's own time between receive and transmit taken out), and the
/// `time::uptime_ns` of that moment. Blocking, 7 s at most: three
/// requests, each with a fresh nonce, waiting longer each time.
pub fn query(server_ip: [u8; 4]) -> Option<(u64, u64)> {
    // Warm the next hop's MAC; a blind wait pays its timeout even when the
    // entry is already cached.
    if let Some(hop) = super::ipv4::arp_target_for(server_ip) {
        let _ = super::arp::resolve(hop, 10);
    }
    let local_port = udp::listen_ephemeral()?;
    let mut result = None;
    'attempts: for wait in ATTEMPT_TICKS {
        // SNTP request; the transmit timestamp is random, not the time.
        // The server copies it into the reply's origin field; it is the
        // nonce that ties the reply to us and says nothing about our clock.
        let mut req = [0u8; 48];
        req[0] = 0x23; // LI=0, Version=4, Mode=3 (client)
        let nonce = crate::csprng::random_u64().to_be_bytes();
        req[40..48].copy_from_slice(&nonce);
        let t1 = crate::time::uptime_ns();
        if !udp::send(server_ip, local_port, NTP_PORT, &req) {
            break;
        }
        let t0 = crate::time::ticks();
        while crate::time::ticks() - t0 <= wait {
            super::poll();
            while let Some((src, port, data)) = udp::recv(local_port) {
                if src == server_ip && port == NTP_PORT {
                    if let Some((t2, t3)) = parse_reply(&data, &nonce) {
                        let t4 = crate::time::uptime_ns();
                        // Our wait less the server's hold, half each way.
                        let delay = (t4 - t1).saturating_sub(t3.saturating_sub(t2));
                        result = Some((t3 + delay / 2, t4));
                        break 'attempts;
                    }
                }
            }
            if udp::take_error(local_port) == Some((server_ip, NTP_PORT)) {
                break 'attempts;
            }
            crate::smp::per_core::reply_wait();
        }
    }
    udp::unlisten(local_port);
    result
}

/// The server's receive and transmit times as Unix nanoseconds, if `data`
/// is a reply to the request that carried `nonce`.
fn parse_reply(data: &[u8], nonce: &[u8; 8]) -> Option<(u64, u64)> {
    if data.len() < 48 { return None; }
    let leap = data[0] >> 6;
    let mode = data[0] & 0x07;
    let stratum = data[1];
    if mode != 4 || leap == 3 || !(1..=15).contains(&stratum) { return None; }
    if data[24..32] != nonce[..] { return None; }
    // Receive (32) and transmit (40) timestamps: seconds since
    // 1900-01-01, then a 32-bit binary fraction.
    let ts = |at: usize| -> Option<u64> {
        let w = |i: usize| u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as u64;
        let unix = w(at).checked_sub(NTP_EPOCH_OFFSET)?;
        Some(unix * 1_000_000_000 + (w(at + 4) * 1_000_000_000 >> 32))
    };
    let (rx, tx) = (ts(32)?, ts(40)?);
    // Earlier than this is not believable, and certificate checks would
    // stop being enforced below it.
    (tx / 1_000_000_000 >= crate::time::CLOCK_SANE_FLOOR && rx <= tx).then_some((rx, tx))
}
