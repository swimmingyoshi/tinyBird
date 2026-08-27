//! The link cable.
//!
//! Four consoles at most, one of them the parent, exchanging a halfword each
//! per transfer. The parent decides when a transfer happens; the children put
//! their halfword on the wire and wait to be clocked. Every console ends up
//! holding all four values, which is what makes trading and battling possible.
//!
//! # Where the state lives
//!
//! The registers themselves — `SIOCNT`, `RCNT`, `SIOMULTI0-3`, `SIOMLT_SEND` —
//! stay in the bus's I/O array, like every other memory-mapped register. This
//! type holds only what a register cannot: which console this is, how many are
//! attached, and a transfer that has begun but not yet landed.
//!
//! That split is deliberate. Savestates serialize the I/O array already, so the
//! registers persist without the state format changing at all, and the parts
//! kept here are exactly the parts that *should not* persist: a restored save
//! is a console that has just been unplugged, and the host reconnects it.
//!
//! # How a transfer is driven
//!
//! The core never talks to a network. It says a transfer wants to happen and
//! accepts the answer; carrying halfwords between consoles is the host's job,
//! whether that is a `Vec<Gba>` in a test or four browsers in a room.
//!
//! ```text
//! parent sets START  ->  transfer_pending() is true
//!                        host reads send_value() from every console
//!                        host calls deliver(values) on every console
//!                        cable clocks for the wire time
//!                        SIOMULTI0-3 land, START clears, IRQ fires
//! ```
//!
//! A child that is ahead of the parent spins in its own polling loop waiting
//! for START to clear, which is what keeps consoles in step without the host
//! having to stall anything: the game's own wait *is* the synchronisation.

/// I/O offset of `SIOMULTI0`, relative to `0x0400_0000`.
pub const SIOMULTI0: usize = 0x120;
/// `SIOCNT`, the control register.
pub const SIOCNT: usize = 0x128;
/// `SIOMLT_SEND` in multi-player mode, `SIODATA8` in normal mode.
pub const SIOMLT_SEND: usize = 0x12A;
/// `RCNT`, which picks what the port is for at all.
pub const RCNT: usize = 0x134;

/// How many consoles one cable joins.
pub const MAX_PLAYERS: usize = 4;

/// The interrupt this raises, as a bit in `IE` and `IF`.
pub const IRQ_BIT: u16 = 0x0080;

/// What a `SIOMULTI` slot reads as when nobody has filled it.
///
/// The hardware blanks all four the moment a transfer starts, so a console
/// that is not attached reads as absent rather than as having sent zero —
/// which a game would take for real data.
pub const NO_DATA: u16 = 0xFFFF;

/// Baud rates in bits per second, indexed by `SIOCNT` bits 0-1.
const BAUD_RATES: [u32; 4] = [9600, 38400, 57600, 115200];

/// Bits on the wire per console: a start bit, sixteen of data, and a stop bit.
const BITS_PER_CONSOLE: u32 = 18;

/// The system clock, for turning a baud rate into cycles.
const CLOCK_HZ: u32 = 16_777_216;

/// What the port is wired up as, from `RCNT` bits 14-15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortMode {
    /// Normal, multi-player or UART, as `SIOCNT` decides.
    Serial,
    /// The pins driven directly, which is how some cartridges reach a
    /// real-time clock or a rumble pack.
    GeneralPurpose,
    /// The link port acting as a controller for a host machine.
    JoyBus,
}

impl PortMode {
    /// Read the mode out of `RCNT`.
    pub fn from_rcnt(rcnt: u16) -> Self {
        match (rcnt >> 14) & 0b11 {
            0b00 | 0b01 => Self::Serial,
            0b10 => Self::GeneralPurpose,
            _ => Self::JoyBus,
        }
    }
}

/// Which serial protocol, from `SIOCNT` bits 12-13.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerialMode {
    /// Two consoles, eight bits.
    Normal8,
    /// Two consoles, thirty-two bits. Also how a multiboot image travels.
    Normal32,
    /// Up to four consoles, sixteen bits each. What the Pokémon games use.
    MultiPlayer,
    /// A plain serial line, which no commercial cartridge is known to use.
    Uart,
}

impl SerialMode {
    /// Read the mode out of `SIOCNT`.
    pub fn from_control(siocnt: u16) -> Self {
        match (siocnt >> 12) & 0b11 {
            0b00 => Self::Normal8,
            0b01 => Self::Normal32,
            0b10 => Self::MultiPlayer,
            _ => Self::Uart,
        }
    }
}

/// A transfer the parent has begun.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// `START` is set and the other consoles have not answered yet.
    ///
    /// How long this lasts is not the cable's business: locally it is one call,
    /// over a network it is a round trip. The game sees a slow transfer, which
    /// is something real hardware does too.
    Waiting,
    /// Every halfword is in and the cable is clocking them out.
    Clocking {
        /// Cycles still to run before the data lands.
        cycles_left: u32,
        /// What each console sent, parent first.
        values: [u16; MAX_PLAYERS],
    },
}

/// The serial port.
#[derive(Clone, Debug, Default)]
pub struct Sio {
    /// Which console this is. `None` when nothing is plugged in.
    id: Option<u8>,
    /// How many consoles share the cable, including this one.
    players: u8,
    /// A transfer in progress, if any.
    phase: Option<Phase>,
}

impl Sio {
    /// A console with nothing plugged into it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Plug this console into a cable as `id`, one of `players` consoles.
    ///
    /// Console 0 is the parent and is the only one that may start a transfer.
    /// Real hardware works this out from which socket the cable is in; here the
    /// host decides, which is the same thing as far as the game can tell.
    pub fn connect(&mut self, id: u8, players: u8) {
        let players = players.clamp(1, MAX_PLAYERS as u8);
        self.id = Some(id.min(players.saturating_sub(1)));
        self.players = players;
    }

    /// Unplug the cable, abandoning any transfer that was in flight.
    pub fn disconnect(&mut self) {
        self.id = None;
        self.players = 0;
        self.phase = None;
    }

    /// Whether a cable with somebody on the other end is attached.
    pub fn connected(&self) -> bool {
        self.id.is_some() && self.players > 1
    }

    /// Which console this is, if any.
    pub fn id(&self) -> Option<u8> {
        self.id
    }

    /// How many consoles share the cable.
    pub fn players(&self) -> u8 {
        self.players
    }

    /// Whether this console is the one that starts transfers.
    pub fn is_parent(&self) -> bool {
        self.id == Some(0)
    }

    /// Whether a transfer has begun and not yet landed.
    pub fn busy(&self) -> bool {
        self.phase.is_some()
    }

    /// Whether a transfer is waiting on the other consoles.
    ///
    /// The host watches this to know when to go and collect everybody's
    /// halfword.
    pub fn transfer_pending(&self) -> bool {
        matches!(self.phase, Some(Phase::Waiting))
    }

    /// Begin a transfer, if this console may and one is not already running.
    ///
    /// Returns whether anything started, which is what tells the caller to
    /// blank `SIOMULTI0-3`.
    pub fn start_transfer(&mut self) -> bool {
        if !self.connected() || !self.is_parent() || self.phase.is_some() {
            return false;
        }
        self.phase = Some(Phase::Waiting);
        true
    }

    /// Accept a transfer the parent has begun.
    ///
    /// A child never starts one, but it is busy while one is running, and its
    /// `START` bit reads as set for as long as that lasts.
    pub fn join_transfer(&mut self) -> bool {
        if !self.connected() || self.is_parent() || self.phase.is_some() {
            return false;
        }
        self.phase = Some(Phase::Waiting);
        true
    }

    /// Hand back what every console put on the wire.
    ///
    /// The data does not land immediately: the cable still has to clock it out,
    /// and a game that times its transfers would notice if it did not.
    ///
    /// `cycles` is how long that takes, and it comes from the **parent**. The
    /// parent drives the clock line, so its baud rate is the cable's baud rate
    /// and a child's own setting does not enter into it. Reading each console's
    /// own register instead looks harmless and is not: Pokémon moves the parent
    /// to 115200 while a child is still configured for 9600, so the child would
    /// hold the cable for twelve times too long, still be clocking when the
    /// next transfer arrived, and miss it. Miss enough and the game gives up
    /// with a communication error.
    pub fn deliver(&mut self, values: [u16; MAX_PLAYERS], cycles: u32) -> bool {
        // Only onto a console that is actually waiting for this transfer.
        //
        // Accepting while still clocking the previous one replaced it, so its
        // halfword never reached the game at all. One lost halfword in the
        // middle of a trade is not a glitch the game rides out: it checks what
        // it receives, and a gap is what "Sorry, we have a link error" means.
        if !matches!(self.phase, Some(Phase::Waiting)) {
            return false;
        }
        self.phase = Some(Phase::Clocking {
            cycles_left: cycles.max(1),
            values,
        });
        true
    }

    /// Give up on a transfer that is waiting on consoles that never answered.
    ///
    /// Returns whether there was one. A console waiting for data is frozen —
    /// that is the whole point of the wait — so without a way out a single
    /// lost message stops the game for good rather than for a moment.
    pub fn abandon(&mut self) -> bool {
        if matches!(self.phase, Some(Phase::Waiting)) {
            self.phase = None;
            return true;
        }
        false
    }

    /// End a transfer that is still clocking, handing back its data.
    ///
    /// The wire is busy until the cable finishes, and on hardware the parent
    /// cannot start another until it does. Across a network the parent has its
    /// own clock and can ask again first, so rather than drop what is on the
    /// wire this finishes it early. A few thousand cycles of inaccuracy is
    /// nothing beside a missing halfword.
    pub fn take_clocking(&mut self) -> Option<[u16; MAX_PLAYERS]> {
        match self.phase.take() {
            Some(Phase::Clocking { values, .. }) => Some(values),
            other => {
                self.phase = other;
                None
            }
        }
    }

    /// How long a transfer occupies the cable, in cycles.
    ///
    /// Eighteen bits per console at the selected baud rate. Two consoles at
    /// 115200 bps comes to about 5,200 cycles, or a third of a scanline.
    pub fn transfer_cycles(siocnt: u16, players: u8) -> u32 {
        let baud = BAUD_RATES[(siocnt & 0b11) as usize];
        let bits = BITS_PER_CONSOLE * u32::from(players.max(1));
        // At least one cycle, so a game can never spin through transfers
        // without time passing.
        ((u64::from(bits) * u64::from(CLOCK_HZ)) / u64::from(baud)).max(1) as u32
    }

    /// Run the cable for `cycles`, returning the data if the transfer lands.
    ///
    /// The caller writes the values into `SIOMULTI0-3`, clears `START`, and
    /// raises the interrupt if `SIOCNT` asked for one.
    pub fn tick(&mut self, cycles: u32) -> Option<[u16; MAX_PLAYERS]> {
        let Some(Phase::Clocking { cycles_left, values }) = &mut self.phase else {
            return None;
        };
        *cycles_left = cycles_left.saturating_sub(cycles);
        if *cycles_left > 0 {
            return None;
        }
        let values = *values;
        self.phase = None;
        Some(values)
    }

    /// Impose the bits the hardware owns onto a `SIOCNT` the game has written.
    ///
    /// Bits 2 to 6 are outputs of the link hardware rather than settings: which
    /// end of the cable this is, whether every console is ready, this console's
    /// player number, and whether the last transfer failed. A game may write
    /// whatever it likes to them and has to read back what is really true.
    ///
    /// `START` is bit 7, and only the parent may write it. On a child it
    /// reports whether a transfer is running, so a child writing it must not
    /// thereby claim the cable.
    pub fn apply_terminal_bits(&self, written: u16) -> u16 {
        // Clear bits 2-6 and rebuild them; the error flag stays low, since
        // nothing here can produce the kind of failure it reports.
        let mut value = written & !0x007C;

        match self.id {
            // Bit 2 is the SI terminal: low on the parent, high on a child.
            Some(0) => {}
            Some(id) => {
                value |= 0x0004;
                value |= (u16::from(id) & 0b11) << 4;
            }
            // Nothing attached: SI floats high and there is no player number.
            None => value |= 0x0004,
        }

        // Bit 3 is the SD terminal, high once every console is ready.
        if self.connected() {
            value |= 0x0008;
        }

        value
    }

    /// Impose the transfer bit (bit 7) from the state of the wire.
    ///
    /// Separate from the terminals because it is only the cable's to set in
    /// multi-player mode. In normal mode bit 7 is the game's own start bit for
    /// a transfer this does not carry, and overwriting it would be answering a
    /// question that was not asked.
    pub fn apply_busy_bit(&self, value: u16) -> u16 {
        if self.busy() {
            value | 0x0080
        } else {
            value & !0x0080
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_mode_comes_from_the_top_two_bits_of_rcnt() {
        assert_eq!(PortMode::from_rcnt(0x0000), PortMode::Serial);
        assert_eq!(PortMode::from_rcnt(0x8000), PortMode::GeneralPurpose);
        assert_eq!(PortMode::from_rcnt(0xC000), PortMode::JoyBus);
    }

    #[test]
    fn multi_player_mode_is_bit_13_without_bit_12() {
        assert_eq!(SerialMode::from_control(0x0000), SerialMode::Normal8);
        assert_eq!(SerialMode::from_control(0x1000), SerialMode::Normal32);
        assert_eq!(SerialMode::from_control(0x2000), SerialMode::MultiPlayer);
        assert_eq!(SerialMode::from_control(0x3000), SerialMode::Uart);
    }

    #[test]
    fn only_the_parent_starts_a_transfer() {
        let mut parent = Sio::new();
        parent.connect(0, 2);
        assert!(parent.start_transfer());

        let mut child = Sio::new();
        child.connect(1, 2);
        assert!(!child.start_transfer(), "a child must not claim the cable");
        assert!(child.join_transfer());
    }

    #[test]
    fn an_unplugged_console_never_transfers() {
        let mut alone = Sio::new();
        assert!(!alone.start_transfer());

        // One console is not a cable, whatever it has been told.
        alone.connect(0, 1);
        assert!(!alone.connected());
        assert!(!alone.start_transfer());
    }

    #[test]
    fn data_lands_only_after_the_cable_has_clocked_it_out() {
        let mut sio = Sio::new();
        sio.connect(0, 2);
        sio.start_transfer();
        assert!(sio.transfer_pending());

        // Multi-player mode at 115200 bps, as the parent has it set.
        let siocnt = 0x2003;
        let total = Sio::transfer_cycles(siocnt, 2);
        sio.deliver([0xAAAA, 0xBBBB, NO_DATA, NO_DATA], total);
        assert!(
            !sio.transfer_pending(),
            "the data is in; the wire is not free yet"
        );
        assert!(sio.busy());

        assert_eq!(sio.tick(total - 1), None, "landed a cycle early");
        assert_eq!(sio.tick(1), Some([0xAAAA, 0xBBBB, NO_DATA, NO_DATA]));
        assert!(!sio.busy());
    }

    /// The parent clocks the cable, so a child's own baud setting is not the
    /// cable's. Pokémon moves the parent to 115200 while a child is still set
    /// to 9600, and timing the child from its own register held the wire
    /// twelve times too long — long enough to still be busy when the next
    /// transfer came, so it missed them until the game gave up.
    #[test]
    fn a_child_runs_at_the_parents_rate_not_its_own() {
        let mut child = Sio::new();
        child.connect(1, 2);
        child.join_transfer();

        // The parent is at 115200; this console still says 9600.
        let parent_cycles = Sio::transfer_cycles(0x2003, 2);
        child.deliver([1, 2, NO_DATA, NO_DATA], parent_cycles);

        assert_eq!(child.tick(parent_cycles - 1), None);
        assert_eq!(
            child.tick(1),
            Some([1, 2, NO_DATA, NO_DATA]),
            "the child held the cable past the parent's transfer"
        );
    }

    #[test]
    fn a_slower_baud_rate_holds_the_cable_for_longer() {
        let fastest = Sio::transfer_cycles(0x2003, 2);
        let slowest = Sio::transfer_cycles(0x2000, 2);
        assert!(slowest > fastest * 10, "9600 against 115200 is twelvefold");

        // Two consoles at 115200: 36 bits, about a third of a scanline.
        assert_eq!(fastest, 5242);

        // And a fuller cable takes longer still.
        assert!(Sio::transfer_cycles(0x2003, 4) > fastest);
    }

    #[test]
    fn the_hardware_owns_the_terminal_and_id_bits() {
        let mut parent = Sio::new();
        parent.connect(0, 2);
        // A game writing every bit must not be able to claim it is a child,
        // nor hand itself a player number.
        let seen = parent.apply_terminal_bits(0xFFFF);
        assert_eq!(seen & 0x0004, 0, "the parent's SI terminal is low");
        assert_eq!(seen & 0x0008, 0x0008, "both consoles are ready");
        assert_eq!((seen >> 4) & 0b11, 0, "the parent is player zero");
        assert_eq!(seen & 0x0040, 0, "no error to report");

        let mut child = Sio::new();
        child.connect(2, 4);
        let seen = child.apply_terminal_bits(0x0000);
        assert_eq!(seen & 0x0004, 0x0004, "a child's SI terminal is high");
        assert_eq!((seen >> 4) & 0b11, 2, "the player number survives a zero write");
    }

    #[test]
    fn an_unplugged_console_reports_a_bad_connection() {
        let sio = Sio::new();
        let seen = sio.apply_terminal_bits(0xFFFF);
        assert_eq!(seen & 0x0008, 0, "SD is low with nothing attached");
        assert_eq!(sio.apply_busy_bit(seen) & 0x0080, 0, "and nothing is ever busy");
    }

    #[test]
    fn the_start_bit_reports_the_cable_rather_than_the_write() {
        let mut child = Sio::new();
        child.connect(1, 2);
        // A child writing START does not start anything, and reads back idle.
        assert_eq!(child.apply_busy_bit(0x0080) & 0x0080, 0);

        child.join_transfer();
        assert_eq!(
            child.apply_busy_bit(0x0000) & 0x0080,
            0x0080,
            "a child is busy while the parent clocks the cable"
        );
    }
}
