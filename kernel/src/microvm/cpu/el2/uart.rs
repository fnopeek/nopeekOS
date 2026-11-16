//! The guest's console: a 16550 at `platform::UART_BASE`, one byte per
//! register (reg-shift 0), without an interrupt; Linux's 8250 driver polls
//! it. Its registers behave as the x86 guest's COM1 (`svm::enable`'s
//! `SerialState`): transmit always ready, receive from bytes the host
//! injected, and the console lines scanned for a panic and a shutdown.

const PANIC_PREFIX: &[u8] = b"Kernel panic - not syncing: ";

pub struct Uart {
    dlab: bool,
    lcr: u8,
    ier: u8,
    mcr: u8,
    scr: u8,
    line: [u8; 256],
    line_n: usize,
    pub panic_observed: bool,
    panic_msg: [u8; 192],
    panic_msg_n: usize,
    halt_observed: bool,
    rx: [u8; 128],
    rx_pos: usize,
    rx_n: usize,
}

impl Uart {
    pub fn new() -> Self {
        Uart {
            dlab: false, lcr: 0, ier: 0, mcr: 0, scr: 0,
            line: [0; 256], line_n: 0,
            panic_observed: false, panic_msg: [0; 192], panic_msg_n: 0, halt_observed: false,
            rx: [0; 128], rx_pos: 0, rx_n: 0,
        }
    }

    pub fn inject(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(self.rx.len());
        self.rx[..n].copy_from_slice(&bytes[..n]);
        self.rx_pos = 0;
        self.rx_n = n;
    }

    pub fn read(&mut self, off: u64) -> u8 {
        match off {
            0 if self.dlab => 1,
            0 => {
                if self.rx_pos < self.rx_n {
                    let b = self.rx[self.rx_pos];
                    self.rx_pos += 1;
                    b
                } else { 0 }
            }
            1 if self.dlab => 0,
            1 => self.ier,
            // IIR: no interrupt pending, FIFOs enabled.
            2 => 0xc1,
            3 => self.lcr,
            4 => self.mcr,
            // LSR: THR and TSR empty, data ready if injected bytes remain.
            5 => 0x60 | (self.rx_pos < self.rx_n) as u8,
            // MSR: CTS, DSR, DCD.
            6 => 0xb0,
            7 => self.scr,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u64, v: u8) {
        match off {
            0 if !self.dlab => self.put_char(v),
            1 if !self.dlab => self.ier = v & 0x0f,
            3 => {
                self.lcr = v;
                self.dlab = v & 0x80 != 0;
            }
            4 => self.mcr = v & 0x1f,
            7 => self.scr = v,
            _ => {}
        }
    }

    fn put_char(&mut self, byte: u8) {
        crate::microvm::boottime::guest_byte();
        if byte == b'\n' || self.line_n == self.line.len() {
            self.emit();
            return;
        }
        if byte != b'\r' {
            self.line[self.line_n] = byte;
            self.line_n += 1;
        }
    }

    fn emit(&mut self) {
        let n = self.line_n;
        self.scan_for_panic(n);
        self.scan_for_shutdown(n);
        let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
        crate::microvm::boottime::guest_line(s);
        self.line_n = 0;
    }

    pub fn flush(&mut self) {
        if self.line_n > 0 {
            self.emit();
        }
    }

    fn scan_for_panic(&mut self, n: usize) {
        if self.panic_observed { return; }
        let line = &self.line[..n];
        if let Some(start) = find(line, PANIC_PREFIX) {
            self.panic_observed = true;
            let body = &line[start + PANIC_PREFIX.len()..];
            let k = body.len().min(self.panic_msg.len());
            self.panic_msg[..k].copy_from_slice(&body[..k]);
            self.panic_msg_n = k;
        }
    }

    pub fn panic_msg(&self) -> &str {
        core::str::from_utf8(&self.panic_msg[..self.panic_msg_n]).unwrap_or("?")
    }

    /// A clean shutdown is PSCI SYSTEM_OFF on Arm; the line only tells the
    /// window to close early, as on x86.
    fn scan_for_shutdown(&mut self, n: usize) {
        if self.halt_observed { return; }
        let line = &self.line[..n];
        if find(line, b"reboot: System halted").is_some()
            || find(line, b"reboot: Power down").is_some()
            || find(line, b"reboot: Restarting").is_some()
        {
            self.halt_observed = true;
            crate::kprintln!("[microvm] guest shut down — closing window + saving profile");
            crate::microvm::cpu::note_guest_shutdown();
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() { return None; }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}
