//! Cartridge EEPROM.
//!
//! EEPROM is the odd one out among GBA backup chips. SRAM and flash are
//! memory-mapped — the game reads and writes bytes at `0x0E000000` and the
//! contents are simply there. EEPROM is a *serial* part: the game clocks one
//! bit at a time through the top of the Game Pak window, one bit per halfword
//! access, and because a single bit per transfer is far too slow to drive from
//! the CPU, every real game drives it with DMA.
//!
//! A command is a burst of written bits followed by a burst of read bits:
//!
//! ```text
//! read   write phase:  1 1 <address> 0                  (9 or 17 bits)
//!        read phase:   0 0 0 0 <64 data bits>           (68 bits)
//!
//! write  write phase:  1 0 <address> <64 data bits> 0   (73 or 81 bits)
//!        read phase:   1 once the chip has finished
//! ```
//!
//! The address is 6 bits on a 4 Kbit part and 14 on a 64 Kbit one. Nothing in
//! the cartridge reports which is fitted and the game never declares it, so the
//! width has to be inferred — and the only honest signal is how long a command
//! turns out to be, since the two widths differ by exactly 8 bits. That is why
//! commands are parsed when the burst *ends* rather than as bits arrive: the
//! length is the evidence, and it is only complete at the end.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Zero bits the chip sends before the data of a read.
const READ_PREAMBLE_BITS: usize = 4;
/// One EEPROM block, in bits. Every access moves exactly one block.
const BLOCK_BITS: usize = 64;
/// One EEPROM block, in bytes.
pub const BLOCK_BYTES: usize = BLOCK_BITS / 8;

/// Address width of a 4 Kbit part, and the block count it addresses.
const SMALL_ADDRESS_BITS: usize = 6;
const SMALL_BLOCKS: usize = 64;
/// Address width of a 64 Kbit part, and the block count it addresses.
const LARGE_ADDRESS_BITS: usize = 14;
const LARGE_BLOCKS: usize = 1024;

/// Size of a 4 Kbit EEPROM, in bytes.
pub const SMALL_SIZE: usize = SMALL_BLOCKS * BLOCK_BYTES;
/// Size of a 64 Kbit EEPROM, in bytes.
pub const LARGE_SIZE: usize = LARGE_BLOCKS * BLOCK_BYTES;

/// Command lengths, in written bits, for each address width.
const SMALL_READ_LEN: usize = 2 + SMALL_ADDRESS_BITS + 1;
const LARGE_READ_LEN: usize = 2 + LARGE_ADDRESS_BITS + 1;
const SMALL_WRITE_LEN: usize = 2 + SMALL_ADDRESS_BITS + BLOCK_BITS + 1;
const LARGE_WRITE_LEN: usize = 2 + LARGE_ADDRESS_BITS + BLOCK_BITS + 1;

/// A serial EEPROM on the cartridge bus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Eeprom {
    /// Stored data. Sized once the address width is known.
    memory: Vec<u8>,
    /// Address width in bits, or `None` until a command reveals it.
    address_bits: Option<usize>,
    /// Bits the game has clocked in since the last read.
    rx: Vec<bool>,
    /// Bits waiting to be clocked out, oldest first.
    tx: VecDeque<bool>,
    /// Set when a write has landed and the host should persist the data.
    dirty: bool,
}

impl Default for Eeprom {
    fn default() -> Self {
        Self::new()
    }
}

impl Eeprom {
    /// A chip with nothing stored on it.
    pub fn new() -> Self {
        Self {
            // Erased EEPROM reads as all ones, and a game tells "no save yet"
            // from that pattern; starting at zero would look like a save file
            // full of zeroes rather than an empty chip.
            memory: vec![0xFF; LARGE_SIZE],
            address_bits: None,
            rx: Vec::new(),
            tx: VecDeque::new(),
            dirty: false,
        }
    }

    /// The stored data, for handing to the host to persist.
    pub fn data(&self) -> &[u8] {
        &self.memory
    }

    /// Replace the stored data with a previously saved image.
    pub fn load(&mut self, data: &[u8]) {
        let len = data.len().min(self.memory.len());
        self.memory[..len].copy_from_slice(&data[..len]);
        // A save file the size of one part tells us which part it came from,
        // which spares the first command from having to establish it.
        self.address_bits = match data.len() {
            SMALL_SIZE => Some(SMALL_ADDRESS_BITS),
            LARGE_SIZE => Some(LARGE_ADDRESS_BITS),
            _ => self.address_bits,
        };
    }

    /// Whether a write has landed since the flag was last cleared.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Acknowledge the writes reported by [`Eeprom::is_dirty`].
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Clock one bit in from the game.
    pub fn write_bit(&mut self, bit: bool) {
        // Any write starts a fresh command; a read burst the game abandoned
        // part-way must not bleed into it.
        self.tx.clear();
        self.rx.push(bit);
    }

    /// Clock one bit out to the game.
    ///
    /// The pending command is executed here, because this is the moment the
    /// written burst is known to be complete and therefore the moment its
    /// length can be trusted.
    pub fn read_bit(&mut self) -> bool {
        if !self.rx.is_empty() {
            self.execute();
            self.rx.clear();
        }
        // A chip with nothing to say reports ready, which is what a game polls
        // for after a write.
        self.tx.pop_front().unwrap_or(true)
    }

    fn execute(&mut self) {
        let Some(bits) = self.learn_address_width() else {
            return;
        };
        if self.rx.len() < 2 + bits {
            return;
        }

        let command = (self.rx[0], self.rx[1]);
        let mut address = 0usize;
        for i in 0..bits {
            address = (address << 1) | usize::from(self.rx[2 + i]);
        }

        let blocks = if bits == SMALL_ADDRESS_BITS {
            SMALL_BLOCKS
        } else {
            LARGE_BLOCKS
        };
        // Wrap rather than trust the address: a game that has not yet worked
        // out its own chip can clock in a wider address than the part has.
        let offset = (address % blocks) * BLOCK_BYTES;

        match command {
            (true, true) => self.begin_read(offset),
            (true, false) => self.commit_write(offset, 2 + bits),
            // "0x" is not a command any documented part answers.
            _ => {}
        }
    }

    /// Work out the address width from the length of the command just seen.
    fn learn_address_width(&mut self) -> Option<usize> {
        let width = match self.rx.len() {
            SMALL_READ_LEN | SMALL_WRITE_LEN => Some(SMALL_ADDRESS_BITS),
            LARGE_READ_LEN | LARGE_WRITE_LEN => Some(LARGE_ADDRESS_BITS),
            // A length that matches nothing is not evidence. Fall back to what
            // an earlier command established, if anything.
            _ => self.address_bits,
        };
        if let Some(width) = width {
            self.address_bits = Some(width);
        }
        width
    }

    fn begin_read(&mut self, offset: usize) {
        self.tx.clear();
        for _ in 0..READ_PREAMBLE_BITS {
            self.tx.push_back(false);
        }
        // A block travels as one big-endian 64-bit quantity, so the last byte
        // of the game's little-endian buffer goes out first. Getting this
        // backwards is not a crash — it stores a byte-reversed image, which a
        // game reads back as a corrupt save file.
        for byte in self.memory[offset..offset + BLOCK_BYTES].iter().rev() {
            for shift in (0..8).rev() {
                self.tx.push_back((byte >> shift) & 1 == 1);
            }
        }
    }

    fn commit_write(&mut self, offset: usize, data_start: usize) {
        if self.rx.len() < data_start + BLOCK_BITS {
            return;
        }
        for (index, byte) in self.memory[offset..offset + BLOCK_BYTES]
            .iter_mut()
            .rev()
            .enumerate()
        {
            let mut value = 0u8;
            for bit in 0..8 {
                value = (value << 1) | u8::from(self.rx[data_start + index * 8 + bit]);
            }
            *byte = value;
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a command the way DMA does: write every bit, then read.
    fn write_bits(chip: &mut Eeprom, bits: &[bool]) {
        for &bit in bits {
            chip.write_bit(bit);
        }
    }

    fn address_bits(address: usize, width: usize) -> Vec<bool> {
        (0..width)
            .rev()
            .map(|shift| (address >> shift) & 1 == 1)
            .collect()
    }

    fn read_command(address: usize, width: usize) -> Vec<bool> {
        let mut bits = vec![true, true];
        bits.extend(address_bits(address, width));
        bits.push(false);
        bits
    }

    fn write_command(address: usize, width: usize, data: [u8; 8]) -> Vec<bool> {
        let mut bits = vec![true, false];
        bits.extend(address_bits(address, width));
        for byte in data {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1 == 1);
            }
        }
        bits.push(false);
        bits
    }

    fn read_block(chip: &mut Eeprom) -> [u8; 8] {
        // Four preamble bits, then the data.
        for _ in 0..READ_PREAMBLE_BITS {
            assert!(!chip.read_bit(), "preamble bits are zero");
        }
        let mut out = [0u8; 8];
        for byte in &mut out {
            for _ in 0..8 {
                *byte = (*byte << 1) | u8::from(chip.read_bit());
            }
        }
        out
    }

    #[test]
    fn an_erased_chip_reads_as_ones() {
        let mut chip = Eeprom::new();
        write_bits(&mut chip, &read_command(0, LARGE_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), [0xFF; 8]);
    }

    #[test]
    fn a_written_block_reads_back() {
        let mut chip = Eeprom::new();
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        write_bits(&mut chip, &write_command(5, LARGE_ADDRESS_BITS, data));
        // Polling after a write must report ready.
        assert!(chip.read_bit());
        assert!(chip.is_dirty());

        write_bits(&mut chip, &read_command(5, LARGE_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), data);
    }

    #[test]
    fn blocks_do_not_overlap() {
        let mut chip = Eeprom::new();
        write_bits(&mut chip, &write_command(0, LARGE_ADDRESS_BITS, [0xAA; 8]));
        chip.read_bit();
        write_bits(&mut chip, &write_command(1, LARGE_ADDRESS_BITS, [0xBB; 8]));
        chip.read_bit();

        write_bits(&mut chip, &read_command(0, LARGE_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), [0xAA; 8]);
        write_bits(&mut chip, &read_command(1, LARGE_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), [0xBB; 8]);
    }

    /// The width is not configured anywhere; it has to come from the traffic.
    #[test]
    fn the_address_width_is_learned_from_the_command_length() {
        let mut small = Eeprom::new();
        write_bits(&mut small, &read_command(0, SMALL_ADDRESS_BITS));
        small.read_bit();
        assert_eq!(small.address_bits, Some(SMALL_ADDRESS_BITS));

        let mut large = Eeprom::new();
        write_bits(&mut large, &read_command(0, LARGE_ADDRESS_BITS));
        large.read_bit();
        assert_eq!(large.address_bits, Some(LARGE_ADDRESS_BITS));
    }

    #[test]
    fn a_4kbit_part_round_trips_too() {
        let mut chip = Eeprom::new();
        let data = [9, 8, 7, 6, 5, 4, 3, 2];
        write_bits(&mut chip, &write_command(3, SMALL_ADDRESS_BITS, data));
        chip.read_bit();
        write_bits(&mut chip, &read_command(3, SMALL_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), data);
    }

    #[test]
    fn an_address_past_the_end_wraps_rather_than_panicking() {
        let mut chip = Eeprom::new();
        // 6-bit part, but the game clocks in an address of 63 plus a carry.
        write_bits(
            &mut chip,
            &write_command(0xFFFF, SMALL_ADDRESS_BITS, [7; 8]),
        );
        chip.read_bit();
        write_bits(&mut chip, &read_command(0xFFFF & 63, SMALL_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), [7; 8]);
    }

    #[test]
    fn a_save_image_restores_and_reveals_the_width() {
        let mut chip = Eeprom::new();
        let mut image = vec![0xFF; SMALL_SIZE];
        image[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        chip.load(&image);
        assert_eq!(chip.address_bits, Some(SMALL_ADDRESS_BITS));

        // The stored image is the game's buffer; on the wire it comes back
        // last byte first.
        write_bits(&mut chip, &read_command(0, SMALL_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), [8, 7, 6, 5, 4, 3, 2, 1]);
    }

    /// The order a block travels in, pinned against a real cartridge.
    ///
    /// The Minish Cap stamps `AGBZELDA:THE MINISH CAP:` across its first three
    /// blocks. On the wire the last byte of a block arrives first, so the bits
    /// spelling `ADLEZBGA` are what must land as `AGBZELDA`. Storing the wire
    /// order verbatim gives a byte-reversed image, which the game reads back as
    /// a corrupt save slot rather than failing outright.
    #[test]
    fn a_block_travels_big_endian() {
        let mut chip = Eeprom::new();
        let wire = *b"ADLEZBGA";
        write_bits(&mut chip, &write_command(0, LARGE_ADDRESS_BITS, wire));
        chip.read_bit();

        assert_eq!(&chip.data()[..8], b"AGBZELDA");

        // And it leaves the way it arrived.
        write_bits(&mut chip, &read_command(0, LARGE_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), wire);
    }

    #[test]
    fn an_abandoned_read_does_not_bleed_into_the_next_command() {
        let mut chip = Eeprom::new();
        write_bits(&mut chip, &write_command(2, LARGE_ADDRESS_BITS, [0x5A; 8]));
        chip.read_bit();

        write_bits(&mut chip, &read_command(2, LARGE_ADDRESS_BITS));
        chip.read_bit(); // take one bit, then walk away
        write_bits(&mut chip, &read_command(2, LARGE_ADDRESS_BITS));
        assert_eq!(read_block(&mut chip), [0x5A; 8]);
    }
}
