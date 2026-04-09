//! DMA Controller
//!
//! The GBA has 4 DMA channels for fast memory transfers. Each channel has
//! configurable source/destination address control, transfer size, and
//! start timing (immediate, VBlank, HBlank, or special).

use crate::bus::Bus;
use serde::{Deserialize, Serialize};

/// Destination address control shift in the control register
const DEST_ADDR_CTRL_SHIFT: u16 = 5;
/// Source address control shift in the control register
const SRC_ADDR_CTRL_SHIFT: u16 = 7;
/// Repeat bit position
const REPEAT_BIT: u16 = 1 << 9;
/// Transfer type bit (0=16-bit, 1=32-bit)
const TRANSFER_32BIT: u16 = 1 << 10;
/// Start timing shift
const START_TIMING_SHIFT: u16 = 12;
/// IRQ on completion bit
const IRQ_ENABLE: u16 = 1 << 14;
/// DMA enable bit
const DMA_ENABLE: u16 = 1 << 15;

/// Start timing values
const TIMING_IMMEDIATE: u16 = 0;
/// VBlank start timing
const TIMING_VBLANK: u16 = 1;
/// HBlank start timing
const TIMING_HBLANK: u16 = 2;
/// Special start timing
const TIMING_SPECIAL: u16 = 3;

/// Source address masks per channel
const SOURCE_MASKS: [u32; 4] = [
    0x07FF_FFFF, // DMA0: 27-bit
    0x0FFF_FFFF, // DMA1: 28-bit
    0x0FFF_FFFF, // DMA2: 28-bit
    0x0FFF_FFFF, // DMA3: 28-bit
];

/// Destination address masks per channel
const DEST_MASKS: [u32; 4] = [
    0x07FF_FFFF, // DMA0: 27-bit
    0x07FF_FFFF, // DMA1: 27-bit
    0x07FF_FFFF, // DMA2: 27-bit
    0x0FFF_FFFF, // DMA3: 28-bit
];

/// Maximum transfer counts per channel (0 means the max value)
const COUNT_MASKS: [u16; 4] = [
    0x3FFF, // DMA0: 14-bit (max 0x4000)
    0x3FFF, // DMA1: 14-bit
    0x3FFF, // DMA2: 14-bit
    0xFFFF, // DMA3: 16-bit (max 0x10000)
];

/// Maximum counts when the count register is 0
const COUNT_MAX: [u32; 4] = [
    0x4000,  // DMA0
    0x4000,  // DMA1
    0x4000,  // DMA2
    0x10000, // DMA3
];

/// A single DMA channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmaChannel {
    /// Source address register (SAD)
    pub source: u32,
    /// Destination address register (DAD)
    pub dest: u32,
    /// Word count register
    pub count: u16,
    /// DMA control register (DMAxCNT_H)
    pub control: u16,
    /// Latched source address (set on enable)
    internal_source: u32,
    /// Latched destination address (set on enable)
    internal_dest: u32,
    /// Latched word count (set on enable)
    internal_count: u16,
    /// Whether this channel has a pending transfer
    pub pending: bool,
}

impl DmaChannel {
    /// Create a new DMA channel with default values
    fn new() -> Self {
        Self {
            source: 0,
            dest: 0,
            count: 0,
            control: 0,
            internal_source: 0,
            internal_dest: 0,
            internal_count: 0,
            pending: false,
        }
    }

    /// Check if the channel is enabled
    pub fn is_enabled(&self) -> bool {
        self.control & DMA_ENABLE != 0
    }

    /// Get the start timing for this channel
    fn start_timing(&self) -> u16 {
        (self.control >> START_TIMING_SHIFT) & 0x3
    }

    /// Get the destination address control mode
    fn dest_addr_ctrl(&self) -> u16 {
        (self.control >> DEST_ADDR_CTRL_SHIFT) & 0x3
    }

    /// Get the source address control mode
    fn src_addr_ctrl(&self) -> u16 {
        (self.control >> SRC_ADDR_CTRL_SHIFT) & 0x3
    }

    /// Check if transfer is 32-bit (true) or 16-bit (false)
    fn is_32bit(&self) -> bool {
        self.control & TRANSFER_32BIT != 0
    }

    /// Check if repeat is enabled
    fn is_repeat(&self) -> bool {
        self.control & REPEAT_BIT != 0
    }

    /// Check if IRQ on completion is enabled
    fn irq_enabled(&self) -> bool {
        self.control & IRQ_ENABLE != 0
    }
}

/// DMA controller managing all 4 channels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmaController {
    /// The 4 DMA channels
    pub channels: [DmaChannel; 4],
}

impl Default for DmaController {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaController {
    /// Create a new DMA controller with all channels disabled
    pub fn new() -> Self {
        Self {
            channels: [
                DmaChannel::new(),
                DmaChannel::new(),
                DmaChannel::new(),
                DmaChannel::new(),
            ],
        }
    }

    /// Read the control register for a channel
    pub fn read_control(&self, channel: usize) -> u16 {
        self.channels[channel].control
    }

    /// Write the source address register for a channel
    pub fn write_source(&mut self, channel: usize, value: u32) {
        self.channels[channel].source = value & SOURCE_MASKS[channel];
    }

    /// Write the destination address register for a channel
    pub fn write_dest(&mut self, channel: usize, value: u32) {
        self.channels[channel].dest = value & DEST_MASKS[channel];
    }

    /// Write the word count register for a channel
    pub fn write_count(&mut self, channel: usize, value: u16) {
        self.channels[channel].count = value & COUNT_MASKS[channel];
    }

    /// Write the control register for a channel.
    ///
    /// If bit 15 (enable) transitions from 0 to 1, the source, destination,
    /// and count values are latched. If the start timing is immediate, the
    /// channel is marked as pending.
    pub fn write_control(&mut self, channel: usize, value: u16) {
        let was_enabled = self.channels[channel].is_enabled();
        self.channels[channel].control = value;

        // Rising edge of enable bit: latch registers
        if !was_enabled && (value & DMA_ENABLE != 0) {
            let ch = &mut self.channels[channel];
            ch.internal_source = ch.source;
            ch.internal_dest = ch.dest;
            ch.internal_count = ch.count;

            // If immediate timing, mark as pending
            if ch.start_timing() == TIMING_IMMEDIATE {
                ch.pending = true;
            }
        }
    }

    /// Execute a DMA transfer for the given channel.
    ///
    /// Returns `true` if an IRQ was requested on completion.
    pub fn run_channel<B: Bus>(&mut self, channel: usize, bus: &mut B) -> bool {
        let ch = &mut self.channels[channel];
        if !ch.is_enabled() || !ch.pending {
            return false;
        }

        ch.pending = false;

        let transfer_size: u32 = if ch.is_32bit() { 4 } else { 2 };
        let count = if ch.internal_count == 0 {
            COUNT_MAX[channel]
        } else {
            ch.internal_count as u32
        };

        let src_delta: i32 = match ch.src_addr_ctrl() {
            0 => transfer_size as i32,  // Increment
            1 => -(transfer_size as i32), // Decrement
            2 => 0,                      // Fixed
            _ => 0,                      // Prohibited (treat as fixed)
        };

        let dest_delta: i32 = match ch.dest_addr_ctrl() {
            0 | 3 => transfer_size as i32,  // Increment or Increment/Reload
            1 => -(transfer_size as i32),     // Decrement
            2 => 0,                           // Fixed
            _ => 0,
        };

        let mut src = ch.internal_source;
        let mut dst = ch.internal_dest;

        for _ in 0..count {
            if ch.is_32bit() {
                let value = bus.read_u32(src);
                bus.write_u32(dst, value);
            } else {
                let value = bus.read_u16(src);
                bus.write_u16(dst, value);
            }
            src = (src as i64 + src_delta as i64) as u32;
            dst = (dst as i64 + dest_delta as i64) as u32;
        }

        // Update internal addresses
        ch.internal_source = src;
        ch.internal_dest = dst;

        let irq = ch.irq_enabled();

        // Handle repeat and timing
        if ch.start_timing() != TIMING_IMMEDIATE && ch.is_repeat() {
            // Reload count for next trigger
            ch.internal_count = ch.count;
            // If dest control is increment/reload (3), reload dest too
            if ch.dest_addr_ctrl() == 3 {
                ch.internal_dest = ch.dest;
            }
        } else {
            // Disable the channel after transfer
            ch.control &= !DMA_ENABLE;
        }

        irq
    }

    /// Return the target sound FIFO for a special-timing DMA channel.
    pub fn sound_fifo_target(&self, channel: usize) -> Option<usize> {
        let ch = &self.channels[channel];
        if !ch.is_enabled() || ch.start_timing() != TIMING_SPECIAL {
            return None;
        }

        match ch.internal_dest & !0x3 {
            0x0400_00A0 => Some(0),
            0x0400_00A4 => Some(1),
            _ => None,
        }
    }

    /// Execute a special-timing DMA request for sound FIFO A/B.
    ///
    /// Returns the target FIFO index, the transferred bytes, the valid byte
    /// count, and whether an IRQ was requested on completion.
    pub fn run_sound_fifo<B: Bus>(
        &mut self,
        channel: usize,
        bus: &mut B,
    ) -> Option<(usize, [u8; 16], usize, bool)> {
        let ch = &mut self.channels[channel];
        if !ch.is_enabled() || ch.start_timing() != TIMING_SPECIAL {
            return None;
        }

        let fifo = match ch.internal_dest & !0x3 {
            0x0400_00A0 => 0,
            0x0400_00A4 => 1,
            _ => return None,
        };

        let src_delta: i32 = match ch.src_addr_ctrl() {
            0 => 4,   // Increment
            1 => -4,  // Decrement
            2 => 0,   // Fixed
            _ => 0,   // Prohibited -> treat as fixed
        };

        let words = if ch.internal_count == 0 {
            4
        } else {
            (ch.internal_count as usize).min(4)
        };

        let mut src = ch.internal_source;
        let mut bytes = [0u8; 16];
        for i in 0..words {
            let value = bus.read_u32(src);
            let word_bytes = value.to_le_bytes();
            bytes[i * 4..i * 4 + 4].copy_from_slice(&word_bytes);
            // Do NOT write to the I/O mirror here. Writing to the FIFO address
            // (0x040000A0/A4) marks it dirty, and sync_io_to_apu would replay
            // those bytes into the FIFO again, causing a double-write that
            // corrupts audio data. The caller (service_sound_fifo_dma) feeds
            // the data directly to the APU via apu.fifo_write().
            src = (src as i64 + src_delta as i64) as u32;
        }

        ch.internal_source = src;
        let irq = ch.irq_enabled();

        if ch.is_repeat() {
            ch.internal_count = ch.count;
            if ch.dest_addr_ctrl() == 3 {
                ch.internal_dest = ch.dest;
            }
        } else {
            ch.control &= !DMA_ENABLE;
        }

        Some((fifo, bytes, words * 4, irq))
    }

    /// Called on VBlank: mark channels with VBlank timing as pending
    pub fn on_vblank(&mut self) {
        for ch in &mut self.channels {
            if ch.is_enabled() && ch.start_timing() == TIMING_VBLANK {
                ch.pending = true;
            }
        }
    }

    /// Called on HBlank: mark channels with HBlank timing as pending
    pub fn on_hblank(&mut self) {
        for ch in &mut self.channels {
            if ch.is_enabled() && ch.start_timing() == TIMING_HBLANK {
                ch.pending = true;
            }
        }
    }

    /// Return the highest priority (lowest index) pending channel, if any
    pub fn check_pending(&self) -> Option<usize> {
        for (i, ch) in self.channels.iter().enumerate() {
            if ch.pending && ch.is_enabled() {
                return Some(i);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SimpleBus;

    #[test]
    fn test_dma_new() {
        let dma = DmaController::new();
        for ch in &dma.channels {
            assert_eq!(ch.source, 0);
            assert_eq!(ch.dest, 0);
            assert_eq!(ch.count, 0);
            assert_eq!(ch.control, 0);
            assert!(!ch.is_enabled());
        }
    }

    #[test]
    fn test_dma_address_masking() {
        let mut dma = DmaController::new();

        // DMA0 source is 27-bit
        dma.write_source(0, 0xFFFF_FFFF);
        assert_eq!(dma.channels[0].source, 0x07FF_FFFF);

        // DMA3 dest is 28-bit
        dma.write_dest(3, 0xFFFF_FFFF);
        assert_eq!(dma.channels[3].dest, 0x0FFF_FFFF);

        // DMA0 count is 14-bit
        dma.write_count(0, 0xFFFF);
        assert_eq!(dma.channels[0].count, 0x3FFF);

        // DMA3 count is 16-bit
        dma.write_count(3, 0xFFFF);
        assert_eq!(dma.channels[3].count, 0xFFFF);
    }

    #[test]
    fn test_dma_immediate_transfer_16bit() {
        let mut dma = DmaController::new();
        let mut bus = SimpleBus::new(None);

        // Write some data to EWRAM source
        bus.write_u16(0x0200_0000, 0x1234);
        bus.write_u16(0x0200_0002, 0x5678);
        bus.write_u16(0x0200_0004, 0x9ABC);

        // Set up DMA0 to copy 3 halfwords from EWRAM to IWRAM
        dma.write_source(0, 0x0200_0000);
        dma.write_dest(0, 0x0300_0000);
        dma.write_count(0, 3);

        // Enable with immediate timing, 16-bit transfer
        // Bits: enable(15) | immediate timing(12-13=00) | src inc(7-8=00) | dest inc(5-6=00)
        dma.write_control(0, DMA_ENABLE);

        assert!(dma.channels[0].pending);
        assert_eq!(dma.check_pending(), Some(0));

        let irq = dma.run_channel(0, &mut bus);
        assert!(!irq);

        // Verify the data was copied
        assert_eq!(bus.read_u16(0x0300_0000), 0x1234);
        assert_eq!(bus.read_u16(0x0300_0002), 0x5678);
        assert_eq!(bus.read_u16(0x0300_0004), 0x9ABC);

        // Channel should be disabled after immediate transfer
        assert!(!dma.channels[0].is_enabled());
    }

    #[test]
    fn test_dma_immediate_transfer_32bit() {
        let mut dma = DmaController::new();
        let mut bus = SimpleBus::new(None);

        bus.write_u32(0x0200_0000, 0xDEAD_BEEF);
        bus.write_u32(0x0200_0004, 0xCAFE_BABE);

        dma.write_source(0, 0x0200_0000);
        dma.write_dest(0, 0x0300_0000);
        dma.write_count(0, 2);

        // Enable with 32-bit transfer
        dma.write_control(0, DMA_ENABLE | TRANSFER_32BIT);

        let irq = dma.run_channel(0, &mut bus);
        assert!(!irq);

        assert_eq!(bus.read_u32(0x0300_0000), 0xDEAD_BEEF);
        assert_eq!(bus.read_u32(0x0300_0004), 0xCAFE_BABE);
    }

    #[test]
    fn test_dma_irq_on_completion() {
        let mut dma = DmaController::new();
        let mut bus = SimpleBus::new(None);

        dma.write_source(0, 0x0200_0000);
        dma.write_dest(0, 0x0300_0000);
        dma.write_count(0, 1);

        // Enable with IRQ
        dma.write_control(0, DMA_ENABLE | IRQ_ENABLE);

        let irq = dma.run_channel(0, &mut bus);
        assert!(irq);
    }

    #[test]
    fn test_dma_vblank_timing() {
        let mut dma = DmaController::new();

        dma.write_source(0, 0x0200_0000);
        dma.write_dest(0, 0x0300_0000);
        dma.write_count(0, 1);

        // Enable with VBlank timing
        let control = DMA_ENABLE | (TIMING_VBLANK << START_TIMING_SHIFT);
        dma.write_control(0, control);

        // Should not be pending immediately
        assert!(!dma.channels[0].pending);
        assert_eq!(dma.check_pending(), None);

        // Trigger VBlank
        dma.on_vblank();
        assert!(dma.channels[0].pending);
        assert_eq!(dma.check_pending(), Some(0));
    }

    #[test]
    fn test_dma_channel_priority() {
        let mut dma = DmaController::new();

        // Enable channels 1 and 2 with VBlank timing
        for ch in [1, 2] {
            dma.write_source(ch, 0x0200_0000);
            dma.write_dest(ch, 0x0300_0000);
            dma.write_count(ch, 1);
            let control = DMA_ENABLE | (TIMING_VBLANK << START_TIMING_SHIFT);
            dma.write_control(ch, control);
        }

        dma.on_vblank();

        // Channel 1 should have higher priority than 2
        assert_eq!(dma.check_pending(), Some(1));
    }
}
