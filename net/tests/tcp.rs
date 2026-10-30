//! TCP scripts in the manner of packetdrill, on a virtual clock.
//!
//! Every `.pkt` file under `tests/tcp/` is one script; each line is a time
//! and an event:
//!
//! ```text
//! +0.01  < S. 0:0(0) ack 1 win 65535 <mss 1460,wscale 7>    segment in
//! +0     > . 1:1(0) ack 1                                    segment out
//! +0     connect 192.0.2.2:80 | listen 80 [backlog] | accept | send 100 | recv 100 | close
//! +0     status 1                                            connect_status
//! +0     state FIN_WAIT_1                         the state, or GONE when freed
//! +0     port 5555               segments to a port nobody uses (our ISN 0)
//! +0     from 40001              the peer's next segments from this port
//! +0     < icmp 3 4 mtu 1000 seq 1    ICMP error quoting our segment at seq 1
//! ```
//!
//! Times are seconds, `+` relative to the previous line. Sequence numbers
//! are relative to the sender's ISN, as in packetdrill. Flags: `S` SYN, `F`
//! FIN, `R` RST, `P` PSH, `.` ACK. On an outbound line `win` and the option
//! list are checked only when given, and `?` matches any value. Every
//! segment the stack sends must be expected by a `>` line, in order and at
//! the time the line names.
//!
//! Scripts under `tests/tcp/` must pass; scripts under `tests/tcp/open/`
//! describe the behaviour an open issue asks for and fail until it is fixed.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use net_core::tcp::{self, Env, Stack, Stats, KERNEL};

const OUR_IP: [u8; 4] = [192, 0, 2, 1];
/// The ISN the script's peer uses.
const PEER_ISN: u32 = 1_000_000;

struct TestEnv {
    now_us: u64,
    rng: u64,
    /// Segments sent, with when.
    sent: VecDeque<([u8; 4], Vec<u8>, u64)>,
    pmtu: u32,
}

impl Env for TestEnv {
    fn ticks(&self) -> u64 { self.now_us / 10_000 }
    fn uptime_us(&self) -> u64 { self.now_us }
    fn our_ip(&self) -> [u8; 4] { OUR_IP }
    fn random_u64(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
    fn free_mib(&self) -> usize { 1024 }
    fn mtu(&self, _dst: [u8; 4]) -> u32 { self.pmtu }
    fn update_pmtu(&mut self, _dst: [u8; 4], mtu: u32) {
        let mtu = if mtu < net_core::ip::MIN_PMTU { self.pmtu.min(net_core::ip::MIN_PMTU) } else { mtu };
        self.pmtu = self.pmtu.min(mtu);
    }
    fn ip_send(&mut self, dst: [u8; 4], segment: &[u8], _id: u16) -> bool {
        self.sent.push_back((dst, segment.to_vec(), self.now_us));
        true
    }
}

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

#[derive(Debug, Clone, PartialEq)]
enum Opt {
    Mss(u16),
    SackOk,
    Nop,
    Ts(Option<u32>, Option<u32>),
    WScale(u8),
    Sack(Vec<(u32, u32)>),
    Eol,
}

/// One segment line: what the script says or what came off the wire.
#[derive(Debug)]
struct Seg {
    flags: u8,
    seq: u32,
    len: u32,
    any_seq: bool,
    ack: Option<u32>,
    win: Option<u16>,
    opts: Option<Vec<Opt>>,
}

fn parse_flags(s: &str) -> Result<u8, String> {
    let mut f = 0;
    for c in s.chars() {
        f |= match c {
            'S' => SYN, 'F' => FIN, 'R' => RST, 'P' => PSH, '.' => ACK,
            _ => return Err(format!("unknown flag {c:?}")),
        };
    }
    Ok(f)
}

fn flags_str(f: u8) -> String {
    let mut s = String::new();
    for (bit, c) in [(SYN, 'S'), (FIN, 'F'), (RST, 'R'), (PSH, 'P'), (ACK, '.')] {
        if f & bit != 0 { s.push(c); }
    }
    s
}

fn parse_num<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("bad number {s:?}"))
}

/// `?` is a wildcard.
fn parse_maybe(s: &str) -> Result<Option<u32>, String> {
    if s == "?" { Ok(None) } else { parse_num(s).map(Some) }
}

fn parse_opts(s: &str) -> Result<Vec<Opt>, String> {
    let mut out = Vec::new();
    for item in s.split(',').map(str::trim).filter(|x| !x.is_empty()) {
        let w: Vec<&str> = item.split_whitespace().collect();
        out.push(match w.as_slice() {
            ["mss", v] => Opt::Mss(parse_num(v)?),
            ["sackOK"] => Opt::SackOk,
            ["nop"] => Opt::Nop,
            ["eol"] => Opt::Eol,
            ["wscale", v] => Opt::WScale(parse_num(v)?),
            ["TS", "val", v, "ecr", e] => Opt::Ts(parse_maybe(v)?, parse_maybe(e)?),
            ["sack", rest @ ..] => {
                let mut blocks = Vec::new();
                for b in rest {
                    let (l, r) = b.split_once(':').ok_or("sack block is l:r")?;
                    blocks.push((parse_num(l)?, parse_num(r)?));
                }
                Opt::Sack(blocks)
            }
            _ => return Err(format!("unknown option {item:?}")),
        });
    }
    Ok(out)
}

/// `FLAGS START:END(LEN) [ack N] [win N] [<opts>]`
fn parse_seg(s: &str) -> Result<Seg, String> {
    let (main, opts) = match s.split_once('<') {
        Some((m, o)) => (m, Some(parse_opts(o.trim_end().strip_suffix('>').ok_or("options end with >")?)?)),
        None => (s, None),
    };
    let w: Vec<&str> = main.split_whitespace().collect();
    if w.len() < 2 { return Err("segment needs flags and START:END(LEN)".into()); }
    let flags = parse_flags(w[0])?;
    let (range, len) = w[1].split_once('(').ok_or("START:END(LEN)")?;
    let (start, end) = range.split_once(':').ok_or("START:END(LEN)")?;
    let len: u32 = parse_num(len.strip_suffix(')').ok_or("START:END(LEN)")?)?;
    // `?:?` matches any sequence number, as for a fresh ISN.
    let any_seq = start == "?" && end == "?";
    let (start, end): (u32, u32) = if any_seq { (0, len) } else { (parse_num(start)?, parse_num(end)?) };
    if end.wrapping_sub(start) != len { return Err(format!("{start}:{end} is not {len} bytes")); }
    let mut seg = Seg { flags, seq: start, len, any_seq, ack: None, win: None, opts };
    let mut i = 2;
    while i < w.len() {
        match (w[i], w.get(i + 1)) {
            ("ack", Some(v)) => seg.ack = Some(parse_num(v)?),
            ("win", Some(v)) => seg.win = Some(parse_num(v)?),
            (x, _) => return Err(format!("unexpected {x:?}")),
        }
        i += 2;
    }
    Ok(seg)
}

fn wire_opts(seg: &[u8], hdr_len: usize) -> Vec<Opt> {
    let mut out = Vec::new();
    let mut i = 20;
    while i < hdr_len {
        match seg[i] {
            0 => { out.push(Opt::Eol); break; }
            1 => { out.push(Opt::Nop); i += 1; }
            kind => {
                let len = seg[i + 1] as usize;
                let b = &seg[i + 2..i + len];
                out.push(match kind {
                    2 => Opt::Mss(u16::from_be_bytes([b[0], b[1]])),
                    3 => Opt::WScale(b[0]),
                    4 => Opt::SackOk,
                    5 => Opt::Sack(b.chunks(8).map(|c| (
                        u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
                        u32::from_be_bytes([c[4], c[5], c[6], c[7]])))
                        .collect()),
                    8 => Opt::Ts(Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                                 Some(u32::from_be_bytes([b[4], b[5], b[6], b[7]]))),
                    k => panic!("stack sent unknown option kind {k}"),
                });
                i += len;
            }
        }
    }
    out
}

fn opts_match(want: &[Opt], got: &[Opt]) -> bool {
    want.len() == got.len() && want.iter().zip(got).all(|(w, g)| match (w, g) {
        (Opt::Ts(wv, we), Opt::Ts(gv, ge)) =>
            wv.is_none_or(|v| Some(v) == *gv) && we.is_none_or(|e| Some(e) == *ge),
        _ => w == g,
    })
}

fn checksum(src: [u8; 4], dst: [u8; 4], seg: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for w in [&src[..], &dst[..]] {
        sum += u16::from_be_bytes([w[0], w[1]]) as u32 + u16::from_be_bytes([w[2], w[3]]) as u32;
    }
    sum += 6 + seg.len() as u32;
    for c in seg.chunks(2) {
        sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    !(sum as u16)
}

struct Run {
    stack: Stack,
    env: TestEnv,
    handle: Option<usize>,
    peer: ([u8; 4], u16),
    local_port: u16,
    local_isn: Option<u32>,
    /// Payload bytes handed to `send` so far, for distinct data.
    sent_bytes: u64,
}

impl Run {
    fn new() -> Self {
        Run {
            stack: Stack::new([7; 32], Box::leak(Box::new(Stats::new())), 4 << 30),
            env: TestEnv { now_us: 0, rng: 0x9e37_79b9_7f4a_7c15, sent: VecDeque::new(), pmtu: 1500 },
            handle: None,
            peer: ([192, 0, 2, 2], 0),
            local_port: 0,
            local_isn: None,
            sent_bytes: 0,
        }
    }

    /// Move the clock to `to_us`, running the tick at every 10 ms boundary
    /// on the way where `next_deadline` says one is due, as the kernel's
    /// loops do: a timer it misses fails the script.
    fn advance(&mut self, to_us: u64) {
        while self.env.now_us < to_us {
            let next = (self.env.now_us / 10_000 + 1) * 10_000;
            self.env.now_us = next.min(to_us);
            let now = self.env.now_us / 10_000;
            if self.env.now_us == next && self.stack.next_deadline(now).is_some_and(|d| d <= now) {
                let pending = self.stack.tick(&mut self.env);
                for p in &pending {
                    tcp::send_pending(&mut self.env, self.stack.stats(), p);
                }
            }
        }
    }

    fn handle(&self) -> Result<usize, String> {
        self.handle.ok_or_else(|| "no connection yet".into())
    }

    fn inbound(&mut self, want: &Seg) -> Result<(), String> {
        let local_isn = self.local_isn.unwrap_or(0);
        let mut seg = vec![0u8; 20];
        let mut opts = Vec::new();
        for o in want.opts.iter().flatten() {
            match o {
                Opt::Mss(v) => { opts.extend([2, 4]); opts.extend(v.to_be_bytes()); }
                Opt::SackOk => opts.extend([4, 2]),
                Opt::Nop => opts.push(1),
                Opt::Eol => opts.push(0),
                Opt::WScale(v) => opts.extend([3, 3, *v]),
                Opt::Ts(v, e) => {
                    opts.extend([8, 10]);
                    opts.extend(v.unwrap_or(0).to_be_bytes());
                    opts.extend(e.unwrap_or(0).to_be_bytes());
                }
                Opt::Sack(blocks) => {
                    opts.extend([5, 2 + 8 * blocks.len() as u8]);
                    for (l, r) in blocks {
                        opts.extend(local_isn.wrapping_add(*l).to_be_bytes());
                        opts.extend(local_isn.wrapping_add(*r).to_be_bytes());
                    }
                }
            }
        }
        while opts.len() % 4 != 0 { opts.push(0); }
        seg.extend(&opts);
        let hdr = seg.len();
        seg[0..2].copy_from_slice(&self.peer.1.to_be_bytes());
        seg[2..4].copy_from_slice(&self.local_port.to_be_bytes());
        seg[4..8].copy_from_slice(&PEER_ISN.wrapping_add(want.seq).to_be_bytes());
        let ack = want.ack.map_or(0, |a| local_isn.wrapping_add(a));
        seg[8..12].copy_from_slice(&ack.to_be_bytes());
        seg[12] = ((hdr / 4) as u8) << 4;
        seg[13] = want.flags;
        seg[14..16].copy_from_slice(&want.win.unwrap_or(65535).to_be_bytes());
        seg.extend((0..want.len).map(|i| b'a' + (i % 26) as u8));
        let c = checksum(self.peer.0, OUR_IP, &seg);
        seg[16..18].copy_from_slice(&c.to_be_bytes());
        self.stack.input(&mut self.env, self.peer.0, &seg);
        Ok(())
    }

    /// `icmp TYPE CODE [mtu N] seq S`: an ICMP error from the peer quoting
    /// our segment at relative sequence S.
    fn icmp(&mut self, rest: &str) -> Result<(), String> {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let (mut kind, mut code, mut mtu, mut seq) = (None, None, 0u16, None);
        let mut i = 0;
        while i < w.len() {
            match w[i] {
                "mtu" => { mtu = parse_num(w.get(i + 1).ok_or("mtu N")?)?; i += 2; }
                "seq" => { seq = Some(parse_num::<u32>(w.get(i + 1).ok_or("seq S")?)?); i += 2; }
                x if kind.is_none() => { kind = Some(parse_num::<u8>(x)?); i += 1; }
                x => { code = Some(parse_num::<u8>(x)?); i += 1; }
            }
        }
        let local_isn = self.local_isn.ok_or("an ICMP error before our SYN")?;
        let mut m = vec![kind.ok_or("icmp TYPE")?, code.ok_or("icmp CODE")?, 0, 0, 0, 0];
        m.extend(mtu.to_be_bytes());
        let mut ip = vec![0x45, 0, 0, 40, 0, 0, 0x40, 0, 64, 6, 0, 0];
        ip.extend(OUR_IP);
        ip.extend(self.peer.0);
        m.extend(ip);
        m.extend(self.local_port.to_be_bytes());
        m.extend(self.peer.1.to_be_bytes());
        m.extend(local_isn.wrapping_add(seq.ok_or("seq S")?).to_be_bytes());
        let e = net_core::ip::icmp_error(&m, OUR_IP).ok_or("not an ICMP error the stack takes")?;
        self.stack.icmp_error(&mut self.env, &e);
        Ok(())
    }

    fn outbound(&mut self, want: &Seg) -> Result<(), String> {
        let (dst, seg, when) = self.env.sent.pop_front()
            .ok_or("expected a segment, the stack sent none")?;
        if when != self.env.now_us {
            return Err(format!("sent at {:.3} s, expected at {:.3} s",
                when as f64 / 1e6, self.env.now_us as f64 / 1e6));
        }
        let flags = seg[13];
        let seq = u32::from_be_bytes(seg[4..8].try_into().unwrap());
        if flags & SYN != 0 && self.local_isn.is_none() {
            self.local_isn = Some(seq);
        }
        let local_isn = self.local_isn.ok_or("an outbound segment before our SYN")?;
        let ack = u32::from_be_bytes(seg[8..12].try_into().unwrap());
        let hdr = ((seg[12] >> 4) as usize) * 4;
        let win = u16::from_be_bytes([seg[14], seg[15]]);
        let len = (seg.len() - hdr) as u32;
        // SACK blocks name the peer's sequence space, relative like the rest.
        let got_opts: Vec<Opt> = wire_opts(&seg, hdr).into_iter().map(|o| match o {
            Opt::Sack(b) => Opt::Sack(b.into_iter()
                .map(|(l, r)| (l.wrapping_sub(PEER_ISN), r.wrapping_sub(PEER_ISN))).collect()),
            o => o,
        }).collect();
        let rel_seq = seq.wrapping_sub(local_isn);
        let rel_ack = ack.wrapping_sub(PEER_ISN);
        let mut desc = format!("{} {}:{}({}) ack {} win {}", flags_str(flags), rel_seq,
            rel_seq.wrapping_add(len), len, rel_ack, win);
        if !got_opts.is_empty() { let _ = write!(desc, " {got_opts:?}"); }

        let mut bad = Vec::new();
        if dst != self.peer.0 { bad.push(format!("to {dst:?}")); }
        if u16::from_be_bytes([seg[0], seg[1]]) != self.local_port { bad.push("source port".into()); }
        if u16::from_be_bytes([seg[2], seg[3]]) != self.peer.1 { bad.push("destination port".into()); }
        if checksum(OUR_IP, dst, &{ let mut s = seg.clone(); s[16] = 0; s[17] = 0; s })
            != u16::from_be_bytes([seg[16], seg[17]]) { bad.push("checksum".into()); }
        if flags != want.flags { bad.push("flags".into()); }
        if (!want.any_seq && rel_seq != want.seq) || len != want.len { bad.push("sequence".into()); }
        if let Some(a) = want.ack {
            if flags & ACK == 0 || rel_ack != a { bad.push("ack".into()); }
        }
        if want.win.is_some_and(|w| w != win) { bad.push("window".into()); }
        if want.opts.as_ref().is_some_and(|o| !opts_match(o, &got_opts)) { bad.push("options".into()); }
        if bad.is_empty() { Ok(()) } else { Err(format!("{} differ; the stack sent {desc}", bad.join(", "))) }
    }

    fn event(&mut self, ev: &str) -> Result<(), String> {
        let (cmd, rest) = ev.split_once(char::is_whitespace).unwrap_or((ev, ""));
        let rest = rest.trim();
        match cmd {
            ">" => return self.outbound(&parse_seg(rest)?),
            _ => {}
        }
        if let Some((dst, seg, _)) = self.env.sent.front() {
            return Err(format!("unexpected segment to {dst:?}: flags {}", flags_str(seg[13])));
        }
        match cmd {
            "<" if rest.starts_with("icmp") => self.icmp(&rest[4..]),
            "<" => self.inbound(&parse_seg(rest)?),
            "connect" => {
                let (ip, port) = rest.split_once(':').ok_or("connect a.b.c.d:port")?;
                let ip: Vec<u8> = ip.split('.').map(parse_num).collect::<Result<_, _>>()?;
                self.peer = (ip.try_into().map_err(|_| "IPv4 address")?, parse_num(port)?);
                let h = self.stack.connect_start(&mut self.env, KERNEL, self.peer.0, self.peer.1)
                    .map_err(|e| format!("connect: {e}"))?;
                self.handle = Some(h);
                self.local_port = self.env.sent.front()
                    .map_or(0, |(_, s, _)| u16::from_be_bytes([s[0], s[1]]));
                Ok(())
            }
            "listen" => {
                let mut w = rest.split_whitespace();
                self.local_port = parse_num(w.next().ok_or("listen PORT [BACKLOG]")?)?;
                let backlog = w.next().map_or(Ok(128), parse_num)?;
                self.peer.1 = 40000;
                self.local_isn = None;
                self.handle = Some(self.stack.listen(self.local_port, backlog).map_err(|e| format!("listen: {e}"))?);
                Ok(())
            }
            "from" => {
                // A second peer connection: its own port, our ISN learned anew.
                self.peer.1 = parse_num(rest)?;
                self.local_isn = None;
                Ok(())
            }
            "accept" => {
                let h = self.stack.accept(self.handle()?).map_err(|e| format!("accept: {e}"))?;
                self.handle = Some(h.ok_or("accept: no connection waiting")?);
                Ok(())
            }
            "send" => {
                let n: u64 = parse_num(rest)?;
                let data: Vec<u8> = (self.sent_bytes..self.sent_bytes + n).map(|i| i as u8).collect();
                self.sent_bytes += n;
                let h = self.handle()?;
                self.stack.send(&mut self.env, h, &data).map_err(|e| format!("send: {e}"))
            }
            "recv" => {
                let n: usize = parse_num(rest)?;
                let mut buf = vec![0u8; n + 1];
                let h = self.handle()?;
                let got = self.stack.recv(&mut self.env, h, &mut buf).map_err(|e| format!("recv: {e}"))?;
                if got != n { return Err(format!("recv got {got} bytes, want {n}")); }
                Ok(())
            }
            "close" => {
                let h = self.handle()?;
                self.stack.close(&mut self.env, h).map_err(|e| format!("close: {e}"))
            }
            "port" => {
                self.local_port = parse_num(rest)?;
                self.peer.1 = 40000;
                self.local_isn = Some(0);
                Ok(())
            }
            "state" => {
                // A connection in TIME-WAIT lives on by its 4-tuple.
                let got = self.stack.state_name(self.handle()?)
                    .or_else(|| self.stack.time_wait_state(self.local_port, self.peer.0, self.peer.1))
                    .unwrap_or("GONE");
                if got != rest { Err(format!("state {got}, want {rest}")) } else { Ok(()) }
            }
            "status" => {
                let want: i32 = parse_num(rest)?;
                let got = self.stack.connect_status(self.handle()?);
                if got != want { Err(format!("connect_status {got}, want {want}")) } else { Ok(()) }
            }
            _ => Err(format!("unknown event {cmd:?}")),
        }
    }
}

/// Run one script; the error names the line.
fn run_script(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut run = Run::new();
    let mut t_us: u64 = 0;
    for (n, line) in text.lines().enumerate() {
        let line = line.split("//").next().unwrap().trim();
        if line.is_empty() { continue; }
        let at = |e: String| format!("{}:{}: {e}\n    {line}", path.display(), n + 1);
        let (time, ev) = line.split_once(char::is_whitespace).ok_or_else(|| at("time and event".into()))?;
        let secs: f64 = time.trim_start_matches('+').parse().map_err(|_| at(format!("bad time {time:?}")))?;
        let us = (secs * 1e6).round() as u64;
        t_us = if time.starts_with('+') { t_us + us } else { us };
        run.advance(t_us);
        run.event(ev.trim()).map_err(at)?;
    }
    if let Some((dst, seg, _)) = run.env.sent.front() {
        return Err(format!("{}: at the end: unexpected segment to {dst:?}: flags {}",
            path.display(), flags_str(seg[13])));
    }
    Ok(())
}

fn scripts(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tcp").join(dir);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pkt"))
        .collect();
    v.sort();
    v
}

fn run_all(dir: &str) -> Vec<String> {
    scripts(dir).iter().filter_map(|p| run_script(p).err()).collect()
}

#[test]
fn scripts_pass() {
    let failed = run_all("");
    assert!(failed.is_empty(), "\n{}", failed.join("\n\n"));
}

/// Behaviour open issues ask for: `cargo test -- --ignored` lists what is
/// still missing.
#[test]
#[ignore]
fn open_issues() {
    let failed = run_all("open");
    assert!(failed.is_empty(), "\n{}", failed.join("\n\n"));
}
