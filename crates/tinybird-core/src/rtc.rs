//! The cartridge clock.
//!
//! Ruby, Sapphire and Emerald carry a Seiko S-3511A real-time clock on the
//! cartridge, wired to the Game Pak's four GPIO pins. FireRed and LeafGreen do
//! not - there is no chip on those boards - which is why berry growth there is
//! counted in steps and here it is counted in days.
//!
//! The chip is not memory-mapped. Three halfwords are overlaid on the start of
//! the ROM:
//!
//!   0x080000C4  data       bit 0 SCK, bit 1 SIO, bit 2 CS
//!   0x080000C6  direction  1 = the GBA drives that pin, 0 = the chip does
//!   0x080000C8  control    bit 0 = the three registers read back; 0 = the ROM
//!                          underneath shows through instead
//!
//! Everything else is bit-banged over those pins: the game raises CS, clocks
//! eight bits of a command out on SCK, then clocks the answer back in. Which is
//! why this is a state machine rather than a set of registers.
//!
//! # Time
//!
//! The clock does not read the host's wall clock itself. `tinybird-core` runs
//! on `wasm32-unknown-unknown`, where `SystemTime::now` panics, so the host
//! pushes the time in with [`Rtc::set_wall_clock`] and this advances between
//! pushes from the cycles the machine has actually run. A host that pushes
//! every frame gets true wall-clock time; one that pushes once gets a clock
//! that runs with the emulation, which is also what fast forward ought to do to
//! a cartridge that is being fast forwarded.

use serde::{Deserialize, Serialize};

/// Cycles in one second of emulated time.
const CYCLES_PER_SECOND: u64 = 16_777_216;

/// Pin levels, as an offset into the cartridge: bit 0 SCK, 1 SIO, 2 CS.
pub const GPIO_DATA: u32 = 0xC4;
/// Which pins the GBA drives. A set bit is an output, from console to chip.
pub const GPIO_DIRECTION: u32 = 0xC6;
/// Bit 0 makes the three registers readable. Clear, the ROM shows through.
pub const GPIO_CONTROL: u32 = 0xC8;

const PIN_SCK: u8 = 1 << 0;
const PIN_SIO: u8 = 1 << 1;
const PIN_CS: u8 = 1 << 2;

/// Fixed high nibble of every command byte.
const COMMAND_TAG: u8 = 0b0110_0000;
/// Set on a command that reads from the chip rather than writing to it.
const COMMAND_READ: u8 = 1;

/// 24-hour mode. The games set it; without it hours come back modulo 12.
const STATUS_24_HOUR: u8 = 0x40;

/// What the transfer is in the middle of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    /// CS is low. Nothing is happening.
    Idle,
    /// Clocking in the eight bits that say which register, and which way.
    Command,
    /// Moving that register's bytes, in whichever direction was asked for.
    Data,
}

/// The cartridge's clock chip.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rtc {
    /// Seconds since the Unix epoch at the last push from the host.
    base_unix: i64,
    /// Cycles run since that push, so the clock moves between pushes.
    cycles: u64,

    /// Pin levels as last written by the game.
    data: u8,
    /// Which pins the GBA is driving. A pin it is not driving reads whatever
    /// the chip is putting there.
    direction: u8,
    /// When clear, these three addresses read as the ROM underneath them.
    /// Games leave it clear until they want the clock, and a cartridge with no
    /// chip on it never sets it at all.
    readable: bool,

    phase: Phase,
    /// Bits shifted in or out so far in the current byte.
    shift: u8,
    bits: u8,
    /// The command byte of the transfer in progress.
    command: u8,
    /// The bytes being moved, and how far through them we are.
    buffer: Vec<u8>,
    index: usize,
    /// The chip's own status byte. Bit 6 is 24-hour mode.
    status: u8,
}

impl Default for Rtc {
    fn default() -> Self {
        Self {
            base_unix: 0,
            cycles: 0,
            data: 0,
            direction: 0,
            readable: false,
            phase: Phase::Idle,
            shift: 0,
            bits: 0,
            command: 0,
            buffer: Vec::new(),
            index: 0,
            status: STATUS_24_HOUR,
        }
    }
}

impl Rtc {
    /// A clock reading the epoch, with its registers switched off.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tell the clock what the time is now.
    ///
    /// The host calls this with seconds since the Unix epoch. Calling it often
    /// keeps the cartridge on wall-clock time; calling it once and letting
    /// [`Rtc::step`] carry it forward keeps the cartridge on emulated time.
    pub fn set_wall_clock(&mut self, unix_seconds: i64) {
        self.base_unix = unix_seconds;
        self.cycles = 0;
    }

    /// Advance the clock by cycles the machine has run.
    pub fn step(&mut self, cycles: u64) {
        self.cycles = self.cycles.saturating_add(cycles);
    }

    /// The time the chip would report, in seconds since the epoch.
    fn now(&self) -> i64 {
        self.base_unix
            .saturating_add((self.cycles / CYCLES_PER_SECOND) as i64)
    }

    /// Whether the game has switched the registers on.
    ///
    /// Until it does, reads fall through to the cartridge - which is what a
    /// board with no clock on it does forever.
    pub fn registers_are_live(&self) -> bool {
        self.readable
    }

    /// Read one of the three registers, by cartridge offset.
    pub fn read(&self, offset: u32) -> u16 {
        match offset {
            GPIO_DATA => u16::from(self.pin_levels()),
            GPIO_DIRECTION => u16::from(self.direction),
            GPIO_CONTROL => u16::from(self.readable),
            _ => 0,
        }
    }

    /// What the pins read as.
    ///
    /// A pin the GBA is driving reads back what the GBA put there. A pin it is
    /// not driving reads what the chip is putting there, which while the chip
    /// is answering is the next bit of the answer.
    fn pin_levels(&self) -> u8 {
        let mut level = self.data & self.direction;
        if self.direction & PIN_SIO == 0 && self.sending() {
            let byte = self.buffer.get(self.index).copied().unwrap_or(0);
            if byte >> self.bits & 1 != 0 {
                level |= PIN_SIO;
            }
        }
        level
    }

    fn sending(&self) -> bool {
        self.phase == Phase::Data && self.command & COMMAND_READ != 0
    }

    /// Write one of the three registers, by cartridge offset.
    ///
    /// Writing the data register is how every transfer happens: the game moves
    /// the pins, and the edges between one write and the next are the clock.
    pub fn write(&mut self, offset: u32, value: u16) {
        match offset {
            GPIO_DATA => self.write_pins(value as u8 & 0b111),
            GPIO_DIRECTION => self.direction = value as u8 & 0b1111,
            GPIO_CONTROL => self.readable = value & 1 != 0,
            _ => {}
        }
    }

    fn write_pins(&mut self, value: u8) {
        let was = self.data;
        self.data = value;

        if value & PIN_CS == 0 {
            // Dropping CS abandons whatever was in flight. The games do this
            // between commands, so it is the ordinary way a transfer ends.
            self.phase = Phase::Idle;
            self.shift = 0;
            self.bits = 0;
            return;
        }

        if was & PIN_CS == 0 {
            // CS rising: a new transfer, which always starts with a command.
            self.phase = Phase::Command;
            self.shift = 0;
            self.bits = 0;
            self.buffer.clear();
            self.index = 0;
            return;
        }

        // Everything else happens on a rising SCK edge.
        if was & PIN_SCK != 0 || value & PIN_SCK == 0 {
            return;
        }

        match self.phase {
            Phase::Idle => {}
            Phase::Command => self.clock_command_bit(value & PIN_SIO != 0),
            Phase::Data => self.clock_data_bit(value & PIN_SIO != 0),
        }
    }

    /// The command byte arrives most significant bit first.
    fn clock_command_bit(&mut self, bit: bool) {
        self.shift = (self.shift << 1) | u8::from(bit);
        self.bits += 1;
        if self.bits < 8 {
            return;
        }

        // Some games clock the byte out the other way round. The tag nibble is
        // fixed, so it is what says which way this one did it.
        let command = if self.shift & 0xF0 == COMMAND_TAG {
            self.shift
        } else {
            self.shift.reverse_bits()
        };

        self.command = command;
        self.bits = 0;
        self.shift = 0;
        self.index = 0;
        self.phase = Phase::Data;
        self.buffer = self.register_bytes(register_of(command));

        if self.buffer.is_empty() {
            // A reset, or a register with nothing in it. Nothing to move.
            self.phase = Phase::Idle;
        }
    }

    /// Data bytes move least significant bit first, in both directions.
    fn clock_data_bit(&mut self, bit: bool) {
        let writing = self.command & COMMAND_READ == 0;
        if writing && bit {
            self.shift |= 1 << self.bits;
        }

        self.bits += 1;
        if self.bits < 8 {
            return;
        }

        if writing {
            let written = self.shift;
            if let Some(slot) = self.buffer.get_mut(self.index) {
                *slot = written;
            }
            self.absorb(register_of(self.command), self.index, written);
        }

        self.shift = 0;
        self.bits = 0;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.phase = Phase::Idle;
        }
    }

    /// What a register holds right now.
    fn register_bytes(&self, register: u8) -> Vec<u8> {
        let time = civil_from_unix(self.now());
        match register {
            // Reset. The games issue it before setting the clock up.
            0 => Vec::new(),
            1 => vec![self.status],
            2 => vec![
                bcd(time.year_in_century),
                bcd(time.month),
                bcd(time.day),
                bcd(time.weekday),
                self.hour_byte(time.hour),
                bcd(time.minute),
                bcd(time.second),
            ],
            3 => vec![
                self.hour_byte(time.hour),
                bcd(time.minute),
                bcd(time.second),
            ],
            _ => Vec::new(),
        }
    }

    /// The hour, with the afternoon flag the chip sets in 12-hour mode.
    fn hour_byte(&self, hour: u8) -> u8 {
        if self.status & STATUS_24_HOUR != 0 {
            bcd(hour)
        } else {
            let afternoon = if hour >= 12 { 0x80 } else { 0 };
            bcd(hour % 12) | afternoon
        }
    }

    /// A byte the game wrote into a register.
    ///
    /// Only the status register is kept: the games write it to ask for 24-hour
    /// mode, and refusing would have every hour come back wrong. Writes to the
    /// date and the time are dropped - the clock reported here is the host's,
    /// and letting a game move it would put the machine's idea of now somewhere
    /// the host cannot follow.
    fn absorb(&mut self, register: u8, index: usize, value: u8) {
        if register == 1 && index == 0 {
            self.status = value & (STATUS_24_HOUR | 0x80);
        }
    }
}

/// Which register a command byte selects.
fn register_of(command: u8) -> u8 {
    (command >> 1) & 0b111
}

/// Binary-coded decimal, which is how the chip reports every field.
fn bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// A moment, in the fields the chip reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Civil {
    year_in_century: u8,
    month: u8,
    day: u8,
    /// 0 is Sunday, which is what the games expect.
    weekday: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

/// Seconds since the epoch, as a date and a time.
///
/// The days-to-civil arithmetic is Howard Hinnant's: it treats March as the
/// first month so the leap day falls at the end of the year and needs no
/// special case, then shifts back at the end.
fn civil_from_unix(unix: i64) -> Civil {
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);

    // 1970-01-01 was a Thursday.
    let weekday = (days + 4).rem_euclid(7) as u8;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u8;
    let month = (if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    }) as u8;
    let year = if month <= 2 { year + 1 } else { year };

    Civil {
        year_in_century: year.rem_euclid(100) as u8,
        month,
        day,
        weekday,
        hour: (seconds / 3_600) as u8,
        minute: ((seconds / 60) % 60) as u8,
        second: (seconds % 60) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one command the way a game does: raise CS, clock the command out
    /// most significant bit first, then clock the answer back in.
    fn transfer(rtc: &mut Rtc, command: u8, bytes: usize) -> Vec<u8> {
        // The GBA drives SCK, SIO and CS while it is sending.
        rtc.write(GPIO_DIRECTION, u16::from(PIN_SCK | PIN_SIO | PIN_CS));
        rtc.write(GPIO_DATA, 0);
        rtc.write(GPIO_DATA, u16::from(PIN_CS));

        for bit in (0..8).rev() {
            let sio = if command >> bit & 1 != 0 { PIN_SIO } else { 0 };
            rtc.write(GPIO_DATA, u16::from(PIN_CS | sio));
            rtc.write(GPIO_DATA, u16::from(PIN_CS | sio | PIN_SCK));
        }

        // Let the chip drive SIO for the answer.
        rtc.write(GPIO_DIRECTION, u16::from(PIN_SCK | PIN_CS));

        let mut out = Vec::new();
        for _ in 0..bytes {
            let mut byte = 0u8;
            for bit in 0..8 {
                rtc.write(GPIO_DATA, u16::from(PIN_CS));
                if rtc.read(GPIO_DATA) as u8 & PIN_SIO != 0 {
                    byte |= 1 << bit;
                }
                rtc.write(GPIO_DATA, u16::from(PIN_CS | PIN_SCK));
            }
            out.push(byte);
        }

        rtc.write(GPIO_DATA, 0);
        out
    }

    #[test]
    fn the_registers_stay_invisible_until_a_game_asks_for_them() {
        let rtc = Rtc::new();
        assert!(
            !rtc.registers_are_live(),
            "a board with no clock on it never switches these on, and neither \
             should one whose game has not asked yet"
        );
    }

    #[test]
    fn the_control_register_is_what_switches_them_on() {
        let mut rtc = Rtc::new();
        rtc.write(GPIO_CONTROL, 1);
        assert!(rtc.registers_are_live());
        assert_eq!(rtc.read(GPIO_CONTROL), 1);
        rtc.write(GPIO_CONTROL, 0);
        assert!(!rtc.registers_are_live());
    }

    #[test]
    fn a_date_command_reports_the_time_it_was_given() {
        let mut rtc = Rtc::new();
        // 2026-08-28T15:04:05Z, a Friday.
        rtc.set_wall_clock(1_787_929_445);

        let out = transfer(&mut rtc, COMMAND_TAG | (2 << 1) | COMMAND_READ, 7);
        assert_eq!(out.len(), 7);
        assert_eq!(out[0], bcd(26), "year within the century");
        assert_eq!(out[1], bcd(8), "month");
        assert_eq!(out[2], bcd(28), "day");
        assert_eq!(out[3], 5, "Friday, counting Sunday as zero");
        assert_eq!(out[4], bcd(15), "hour, in 24-hour mode");
        assert_eq!(out[5], bcd(4), "minute");
        assert_eq!(out[6], bcd(5), "second");
    }

    #[test]
    fn the_time_command_reports_the_clock_without_the_date() {
        let mut rtc = Rtc::new();
        rtc.set_wall_clock(1_787_929_445);
        let out = transfer(&mut rtc, COMMAND_TAG | (3 << 1) | COMMAND_READ, 3);
        assert_eq!(out, vec![bcd(15), bcd(4), bcd(5)]);
    }

    /// A command clocked out the other way round is still that command. Games
    /// differ on the bit order, and the fixed tag nibble is what says which.
    #[test]
    fn a_command_sent_least_significant_bit_first_is_understood_too() {
        let mut rtc = Rtc::new();
        rtc.set_wall_clock(1_787_929_445);
        let forward = COMMAND_TAG | (3 << 1) | COMMAND_READ;
        let out = transfer(&mut rtc, forward.reverse_bits(), 3);
        assert_eq!(out, vec![bcd(15), bcd(4), bcd(5)]);
    }

    #[test]
    fn the_clock_runs_on_the_cycles_the_machine_runs() {
        let mut rtc = Rtc::new();
        rtc.set_wall_clock(1_787_929_445);
        rtc.step(CYCLES_PER_SECOND * 90);

        let out = transfer(&mut rtc, COMMAND_TAG | (3 << 1) | COMMAND_READ, 3);
        assert_eq!(out, vec![bcd(15), bcd(5), bcd(35)], "ninety seconds later");
    }

    #[test]
    fn twelve_hour_mode_flags_the_afternoon() {
        let mut rtc = Rtc::new();
        rtc.set_wall_clock(1_787_929_445);
        rtc.status = 0;

        let out = transfer(&mut rtc, COMMAND_TAG | (3 << 1) | COMMAND_READ, 3);
        assert_eq!(out[0], bcd(3) | 0x80, "three in the afternoon");
    }

    #[test]
    fn dates_convert_at_the_edges_that_usually_go_wrong() {
        // The epoch itself, a Thursday.
        let epoch = civil_from_unix(0);
        assert_eq!((epoch.year_in_century, epoch.month, epoch.day), (70, 1, 1));
        assert_eq!(epoch.weekday, 4);

        // A leap day, in a century year that is also a leap year.
        let leap = civil_from_unix(951_782_400);
        assert_eq!((leap.year_in_century, leap.month, leap.day), (0, 2, 29));

        // The last second of a year.
        let eve = civil_from_unix(1_767_225_599);
        assert_eq!((eve.year_in_century, eve.month, eve.day), (25, 12, 31));
        assert_eq!((eve.hour, eve.minute, eve.second), (23, 59, 59));
    }
}
