//! Timer Controller
//!
//! The GBA has 4 hardware timers that can be clocked by the system clock
//! (with a configurable prescaler) or cascaded from the previous timer.
//! Each timer can generate an IRQ on overflow.

use serde::{Deserialize, Serialize};

/// Prescaler bit mask in control register
const PRESCALER_MASK: u16 = 0x3;
/// Cascade mode bit
const CASCADE_BIT: u16 = 1 << 2;
/// IRQ on overflow bit
const IRQ_ENABLE: u16 = 1 << 6;
/// Timer enable bit
const TIMER_ENABLE: u16 = 1 << 7;

/// Prescaler divider values indexed by the 2-bit prescaler field
const PRESCALER_DIVIDERS: [u32; 4] = [1, 64, 256, 1024];

/// A single hardware timer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timer {
    /// Current counter value (readable via TMxCNT_L)
    pub counter: u16,
    /// Reload value (written via TMxCNT_L)
    pub reload: u16,
    /// Control register (TMxCNT_H)
    pub control: u16,
    /// Internal prescaler cycle accumulator
    prescaler_counter: u32,
}

impl Timer {
    /// Create a new timer with default values
    fn new() -> Self {
        Self {
            counter: 0,
            reload: 0,
            control: 0,
            prescaler_counter: 0,
        }
    }

    /// Check if the timer is enabled
    pub fn is_enabled(&self) -> bool {
        self.control & TIMER_ENABLE != 0
    }

    /// Check if cascade mode is active
    pub fn is_cascade(&self) -> bool {
        self.control & CASCADE_BIT != 0
    }

    /// Check if IRQ on overflow is enabled
    pub fn irq_enabled(&self) -> bool {
        self.control & IRQ_ENABLE != 0
    }

    /// Get the prescaler divider for this timer
    pub fn prescaler(&self) -> u32 {
        PRESCALER_DIVIDERS[(self.control & PRESCALER_MASK) as usize]
    }
}

/// Timer controller managing all 4 timers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerController {
    /// The 4 hardware timers
    pub timers: [Timer; 4],
}

impl Default for TimerController {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerController {
    /// Create a new timer controller with all timers disabled
    pub fn new() -> Self {
        Self {
            timers: [Timer::new(), Timer::new(), Timer::new(), Timer::new()],
        }
    }

    /// Advance all timers by the given number of CPU cycles.
    ///
    /// Returns how many times each timer overflowed (for IRQ generation/cascade).
    /// Timer 0 cannot be in cascade mode. Cascade timers increment when the
    /// previous timer overflows rather than from the prescaler.
    pub fn tick(&mut self, cycles: u32) -> [u32; 4] {
        let mut overflow_counts = [0; 4];

        for i in 0..4 {
            if !self.timers[i].is_enabled() {
                continue;
            }

            // Cascade timers are driven by the previous timer's overflow
            if self.timers[i].is_cascade() && i > 0 {
                let increments = overflow_counts[i - 1];
                if increments != 0 {
                    overflow_counts[i] = Self::apply_increments(&mut self.timers[i], increments);
                }
                continue;
            }

            // Normal (prescaler-driven) timer
            let prescaler = self.timers[i].prescaler();
            self.timers[i].prescaler_counter += cycles;
            let increments = self.timers[i].prescaler_counter / prescaler;
            self.timers[i].prescaler_counter %= prescaler;

            if increments != 0 {
                overflow_counts[i] = Self::apply_increments(&mut self.timers[i], increments);
            }
        }

        overflow_counts
    }

    fn apply_increments(timer: &mut Timer, increments: u32) -> u32 {
        if increments == 0 {
            return 0;
        }

        let counter = timer.counter as u32;
        let reload = timer.reload as u32;
        let first_span = 0x1_0000 - counter;

        if increments < first_span {
            timer.counter = counter.wrapping_add(increments) as u16;
            return 0;
        }

        let period = 0x1_0000 - reload;
        let remaining = increments - first_span;
        let additional_overflows = remaining / period;
        let leftover = remaining % period;
        timer.counter = reload.wrapping_add(leftover) as u16;

        1 + additional_overflows
    }

    /// Read the current counter value for a timer
    pub fn read_counter(&self, timer: usize) -> u16 {
        self.timers[timer].counter
    }

    /// Read the control register for a timer
    pub fn read_control(&self, timer: usize) -> u16 {
        self.timers[timer].control
    }

    /// Write the reload value for a timer
    pub fn write_reload(&mut self, timer: usize, value: u16) {
        self.timers[timer].reload = value;
    }

    /// Write the control register for a timer.
    ///
    /// If the enable bit transitions from 0 to 1, the reload value is
    /// loaded into the counter and the prescaler accumulator is reset.
    pub fn write_control(&mut self, timer: usize, value: u16) {
        let was_enabled = self.timers[timer].is_enabled();
        self.timers[timer].control = value;

        // Rising edge of enable bit: load reload into counter
        if !was_enabled && (value & TIMER_ENABLE != 0) {
            self.timers[timer].counter = self.timers[timer].reload;
            self.timers[timer].prescaler_counter = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timer_new() {
        let tc = TimerController::new();
        for t in &tc.timers {
            assert_eq!(t.counter, 0);
            assert_eq!(t.reload, 0);
            assert_eq!(t.control, 0);
            assert!(!t.is_enabled());
        }
    }

    #[test]
    fn test_timer_enable_loads_reload() {
        let mut tc = TimerController::new();

        tc.write_reload(0, 0xFF00);
        tc.write_control(0, TIMER_ENABLE); // prescaler=1

        assert_eq!(tc.read_counter(0), 0xFF00);
    }

    #[test]
    fn test_timer_tick_prescaler_1() {
        let mut tc = TimerController::new();

        tc.write_reload(0, 0xFFFC);
        tc.write_control(0, TIMER_ENABLE); // prescaler=1

        // Counter starts at 0xFFFC, needs 4 ticks to overflow
        let ov = tc.tick(3);
        assert_eq!(ov[0], 0);
        assert_eq!(tc.read_counter(0), 0xFFFF);

        let ov = tc.tick(1);
        assert_eq!(ov[0], 1);
        // After overflow, counter reloads to 0xFFFC
        assert_eq!(tc.read_counter(0), 0xFFFC);
    }

    #[test]
    fn test_timer_tick_prescaler_64() {
        let mut tc = TimerController::new();

        tc.write_reload(0, 0xFFFF);
        // Prescaler = 64 (bits 0-1 = 01)
        tc.write_control(0, TIMER_ENABLE | 0x01);

        assert_eq!(tc.read_counter(0), 0xFFFF);

        // 63 cycles should not overflow yet
        let ov = tc.tick(63);
        assert_eq!(ov[0], 0);
        assert_eq!(tc.read_counter(0), 0xFFFF);

        // 1 more cycle = 64 total, counter increments and overflows
        let ov = tc.tick(1);
        assert_eq!(ov[0], 1);
        assert_eq!(tc.read_counter(0), 0xFFFF); // reloaded
    }

    #[test]
    fn test_timer_cascade() {
        let mut tc = TimerController::new();

        // Timer 0: prescaler=1, reload=0xFFFE (overflows after 2 ticks)
        tc.write_reload(0, 0xFFFE);
        tc.write_control(0, TIMER_ENABLE);

        // Timer 1: cascade mode, reload=0xFFFE
        tc.write_reload(1, 0xFFFE);
        tc.write_control(1, TIMER_ENABLE | CASCADE_BIT);

        // Timer 0 needs 2 ticks to overflow
        let ov = tc.tick(1);
        assert_eq!(ov[0], 0);
        assert_eq!(ov[1], 0);
        assert_eq!(tc.read_counter(0), 0xFFFF);
        assert_eq!(tc.read_counter(1), 0xFFFE);

        // Timer 0 overflows, timer 1 increments
        let ov = tc.tick(1);
        assert_eq!(ov[0], 1);
        assert_eq!(ov[1], 0);
        assert_eq!(tc.read_counter(0), 0xFFFE); // reloaded
        assert_eq!(tc.read_counter(1), 0xFFFF);

        // Timer 0 overflows again, timer 1 overflows too
        let ov = tc.tick(2);
        assert_eq!(ov[0], 1);
        assert_eq!(ov[1], 1);
        assert_eq!(tc.read_counter(1), 0xFFFE); // reloaded
    }

    #[test]
    fn test_timer_irq_flag() {
        let tc = TimerController::new();
        // IRQ disabled by default
        assert!(!tc.timers[0].irq_enabled());
    }

    #[test]
    fn test_timer_disabled_no_tick() {
        let mut tc = TimerController::new();

        tc.write_reload(0, 0xFFFF);
        // Don't enable the timer
        tc.timers[0].counter = 0xFFFF;

        let ov = tc.tick(100);
        assert_eq!(ov[0], 0);
        assert_eq!(tc.read_counter(0), 0xFFFF);
    }
}
