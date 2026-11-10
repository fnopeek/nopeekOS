//! The Pi 5's Ethernet port: `macb_probe`, `ndo_open`, the phylib poll with
//! phylink's resolve, the interrupt as RP1 delivers it, and `ndo_stop`, in
//! Linux's order over [`gem`], [`phy`], [`gpio_rp1`](crate::gpio_rp1) and RP1's interrupt
//! chip. [`clk_rp1::eth_clocks`] runs once before (the `rp1-clk` probe).
//!
//! Timing the caller provides: after [`Eth::open`], [`Eth::poll_link`] at
//! once and then every second (`PHY_STATE_TIME`); [`Gem::tx_lpi_work`] when
//! the timer [`Gem::take_lpi_timer`] asked for expires; [`Gem::rx_poll`] /
//! [`Gem::tx_poll`] after an interrupt asked for them, again while they
//! return `true`; [`Gem::tx_error_task`] / [`Gem::hresp_error_task`] after
//! an interrupt reported those errors.

use crate::clk_rp1;
use crate::gem::{self, Dma, Gem, Irq, Rings};
use crate::gpio_rp1::{Line, Pin};
use crate::hw::{Block, Hw};
use crate::phy::{self, Event, Mdio, Phy};
use crate::rp1::{Rp1, Trigger};

/// The DT's `ethernet@100000` with the board's `phy-reset-gpios`.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// `ethernet-phy@1`.
    pub phy_addr: u32,
    pub reset_gpio: u32,
    pub reset_active_low: bool,
    /// `phy-reset-duration`.
    pub reset_ms: u64,
    /// `phy-reset-post-delay`.
    pub reset_post_ms: u64,
    /// `RP1_INT_ETH`.
    pub irq: u32,
    /// `local-mac-address` as the firmware left it.
    pub dt_mac: Option<[u8; 6]>,
}

pub const PI5: Config = Config {
    phy_addr: 1, reset_gpio: 32, reset_active_low: true, reset_ms: 5, reset_post_ms: 0, irq: 6, dt_mac: None,
};

/// phylink's `lpi_timer_default` for macb.
const LPI_TIMER_US: u64 = 250_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Gem(gem::Error),
    Phy(phy::Error),
    NoPin(u32),
}

impl From<phy::Error> for Error {
    fn from(e: phy::Error) -> Self { Error::Phy(e) }
}

impl From<gem::Error> for Error {
    fn from(e: gem::Error) -> Self { Error::Gem(e) }
}

/// The MAC's MDIO bus as phylib sees one PHY on it.
struct Bus<'a, H: Hw> {
    hw: &'a mut H,
    addr: u32,
}

impl<H: Hw> Mdio for Bus<'_, H> {
    fn read(&mut self, reg: u32) -> Result<u16, phy::Error> {
        gem::mdio_read(self.hw, self.addr, reg).map_err(|_| phy::Error::Timeout)
    }
    fn write(&mut self, reg: u32, v: u16) -> Result<(), phy::Error> {
        gem::mdio_write(self.hw, self.addr, reg, v).map_err(|_| phy::Error::Timeout)
    }
    fn delay_us(&mut self, us: u64) { self.hw.delay_us(us) }
}

pub struct Eth {
    pub gem: Gem,
    pub phy: Phy,
    pub reset: Line,
    cfg: Config,
    /// phylink's resolved link as the MAC was told.
    pub link: Option<phy::Link>,
    lpi: bool,
}

fn valid_mac(a: &[u8; 6]) -> bool { a[0] & 1 == 0 && *a != [0; 6] }

/// `macb_probe` with `raspberrypi_rp1_config`. `random_mac` is used when
/// neither the DT nor the address registers hold a valid address
/// (`eth_hw_addr_random`).
pub fn probe(hw: &mut impl Hw, rp1: &Rp1, cfg: &Config, random_mac: [u8; 6]) -> Result<Eth, Error> {
    let rates = clk_rp1::macb_clk_init(hw);
    let caps = gem::probe_caps(hw)?;
    let mac = match cfg.dt_mac.filter(valid_mac) {
        Some(m) => m,
        None => Gem::get_hwaddr(hw).unwrap_or(random_mac),
    };
    let pin = Pin::new(cfg.reset_gpio).ok_or(Error::NoPin(cfg.reset_gpio))?;
    let reset = Line::request_output(hw, pin, cfg.reset_active_low, false);

    // macb_init: devm_request_irq, then the registers.
    rp1.irq_enable(hw, cfg.irq, Trigger::Level);
    rp1.irq_unmask(hw, cfg.irq);
    let gem = Gem::init(hw, caps, mac, rates.sys, rates.tsu);

    // macb_mii_init: the bus resets the PHY, then finds it at its address.
    Gem::mdio_enable(hw);
    reset.set(hw, true);
    hw.delay_us(cfg.reset_ms * 1000);
    reset.set(hw, false);
    hw.delay_us(cfg.reset_post_ms * 1000);
    let mut bus = Bus { hw: &mut *hw, addr: cfg.phy_addr };
    let id = phy::read_id(&mut bus)?;
    if id != phy::PHY_ID_BCM54213PE {
        return Err(Error::Phy(phy::Error::Id(id)));
    }
    let phy = phy::probe(&mut bus)?;

    // register_netdev's banner reads the revision.
    hw.r32(Block::Rp1Bar1, gem::GEM_BASE + 0xfc);
    Ok(Eth { gem, phy, reset, cfg: *cfg, link: None, lpi: false })
}

impl Eth {
    fn bus<'a, H: Hw>(&self, hw: &'a mut H) -> Bus<'a, H> { Bus { hw, addr: self.cfg.phy_addr } }

    /// `__dev_open`: `macb_open` (rings, `macb_init_hw`,
    /// `macb_phylink_connect`, `gem_ptp_init` with the time `now`), then
    /// `ndo_set_rx_mode`.
    pub fn open(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, rings: Rings, now_s: u64, now_ns: u32) -> Result<(), Error> {
        self.gem.open(hw, dma, rings)?;
        self.phy.attach(&mut self.bus(hw))?;
        self.gem.mac_config(hw);
        self.phy.start(&mut self.bus(hw))?;
        self.gem.ptp_init(hw, now_s, now_ns);
        self.gem.set_rx_mode(hw, false, false, &[]);
        Ok(())
    }

    /// `phylink_resolve` for a PHY event: the MAC's link and LPI.
    fn resolve(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, ev: Option<Event>) {
        match ev {
            Some(Event::Up(l)) if self.link.is_none() => {
                self.gem.link_up(hw, dma, l.speed, l.full_duplex, l.rx_pause);
                if l.tx_lpi {
                    self.gem.enable_tx_lpi(LPI_TIMER_US);
                    self.lpi = true;
                }
                self.link = Some(l);
            }
            Some(Event::Down) if self.link.is_some() => {
                if self.lpi {
                    self.gem.disable_tx_lpi(hw);
                    self.lpi = false;
                }
                self.gem.link_down(hw);
                self.link = None;
            }
            _ => {}
        }
    }

    /// One `phy_state_machine` run and the resolve it queues.
    pub fn poll_link(&mut self, hw: &mut impl Hw, dma: &mut impl Dma) -> Result<Option<Event>, Error> {
        let ev = self.phy.poll(&mut self.bus(hw))?;
        self.resolve(hw, dma, ev);
        Ok(ev)
    }

    /// RP1's level interrupt for the MAC: `rp1_chained_handle_irq` ->
    /// `handle_level_irq` (mask, `macb_interrupt`, unmask), then the IACK.
    pub fn interrupt(&mut self, hw: &mut impl Hw, rp1: &Rp1) -> Irq {
        rp1.irq_mask(hw, self.cfg.irq);
        let irq = self.gem.interrupt(hw);
        rp1.irq_unmask(hw, self.cfg.irq);
        rp1.irq_ack(hw, self.cfg.irq);
        irq
    }

    /// `__dev_close`: `macb_close` (phylink stops the PHY and takes the
    /// link down, reset, buffers back, PTP off).
    pub fn close(&mut self, hw: &mut impl Hw, dma: &mut impl Dma) -> Result<(), Error> {
        self.gem.begin_close();
        let ev = self.phy.stop(&mut self.bus(hw));
        if let Ok(ev) = ev {
            self.resolve(hw, dma, ev.or(Some(Event::Down)));
        }
        self.gem.close(hw, dma);
        ev.map(|_| ()).map_err(Error::Phy)
    }
}
