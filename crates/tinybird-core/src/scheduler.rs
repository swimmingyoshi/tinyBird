//! Event Scheduler
//!
//! The scheduler manages timed events for the emulator, including:
//! - Display interrupts (VBlank, HBlank)
//! - Timer overflows
//! - DMA transfers
//! - Serial communication

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use serde::{Deserialize, Serialize};

/// Maximum number of pending events
const MAX_EVENTS: usize = 64;

/// Event ID type
pub type EventId = u64;

/// Scheduled event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique event ID
    pub id: EventId,
    /// Time when the event should fire (in CPU cycles)
    pub time: u64,
    /// Event type
    pub event_type: EventType,
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Event {}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap (earliest time first)
        other
            .time
            .cmp(&self.time)
            .then_with(|| self.id.cmp(&other.id))
    }
}

/// Type of scheduled event
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    /// VBlank interrupt
    VBlank,
    /// HBlank interrupt
    HBlank,
    /// Timer overflow
    TimerOverflow(u8),
    /// DMA transfer
    Dma(u8),
    /// Serial communication
    Serial,
    /// Custom event
    Custom(u8),
}

/// Event callback function type
pub type EventCallback = fn(EventType);

/// Event scheduler
#[derive(Clone, Serialize, Deserialize)]
pub struct Scheduler {
    /// Priority queue of pending events
    events: BinaryHeap<Event>,
    /// Next event ID to assign
    next_id: EventId,
    /// Current time in CPU cycles
    current_time: u64,
    /// Event callbacks
    #[serde(skip, default)]
    callbacks: [Option<EventCallback>; 16],
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            events: BinaryHeap::with_capacity(MAX_EVENTS),
            next_id: 0,
            current_time: 0,
            callbacks: [None; 16],
        }
    }

    /// Get the current time
    pub fn current_time(&self) -> u64 {
        self.current_time
    }

    /// Set the current time
    pub fn set_current_time(&mut self, time: u64) {
        self.current_time = time;
    }

    /// Advance the current time
    pub fn advance(&mut self, cycles: u64) {
        self.current_time = self.current_time.wrapping_add(cycles);
    }

    /// Schedule a new event
    pub fn schedule(&mut self, event_type: EventType, delay: u64) -> EventId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let event = Event {
            id,
            time: self.current_time.wrapping_add(delay),
            event_type,
        };

        if self.events.len() < MAX_EVENTS {
            self.events.push(event);
        }

        id
    }

    /// Schedule an event at an absolute time
    pub fn schedule_absolute(&mut self, event_type: EventType, time: u64) -> EventId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let event = Event {
            id,
            time,
            event_type,
        };

        if self.events.len() < MAX_EVENTS {
            self.events.push(event);
        }

        id
    }

    /// Cancel a scheduled event
    pub fn cancel(&mut self, id: EventId) -> bool {
        // Note: This requires rebuilding the heap, which is O(n)
        // For better performance, we could use lazy cancellation
        let len_before = self.events.len();
        self.events.retain(|e| e.id != id);
        self.events.len() != len_before
    }

    /// Get the time until the next event
    pub fn time_until_next_event(&self) -> Option<u64> {
        self.events
            .peek()
            .map(|e| e.time.saturating_sub(self.current_time))
    }

    /// Get the next event time
    pub fn next_event_time(&self) -> Option<u64> {
        self.events.peek().map(|e| e.time)
    }

    /// Check if there are pending events
    pub fn has_pending_events(&self) -> bool {
        !self.events.is_empty()
    }

    /// Process all events that are due
    /// Returns the number of events processed
    pub fn process_events<F>(&mut self, mut callback: F) -> usize
    where
        F: FnMut(EventType),
    {
        let mut processed = 0;

        while let Some(event) = self.events.peek() {
            if event.time > self.current_time {
                break;
            }

            if let Some(event) = self.events.pop() {
                callback(event.event_type);
                processed += 1;
            }
        }

        processed
    }

    /// Register a callback for an event type
    pub fn register_callback(&mut self, event_type: EventType, callback: EventCallback) {
        let idx = match event_type {
            EventType::VBlank => 0,
            EventType::HBlank => 1,
            EventType::TimerOverflow(n) => 2 + (n & 3) as usize,
            EventType::Dma(n) => 6 + (n & 3) as usize,
            EventType::Serial => 10,
            EventType::Custom(n) => 11 + (n & 3) as usize,
        };
        self.callbacks[idx] = Some(callback);
    }

    /// Remove a callback
    pub fn unregister_callback(&mut self, event_type: EventType) {
        let idx = match event_type {
            EventType::VBlank => 0,
            EventType::HBlank => 1,
            EventType::TimerOverflow(n) => 2 + (n & 3) as usize,
            EventType::Dma(n) => 6 + (n & 3) as usize,
            EventType::Serial => 10,
            EventType::Custom(n) => 11 + (n & 3) as usize,
        };
        self.callbacks[idx] = None;
    }

    /// Clear all pending events
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Get the number of pending events
    pub fn pending_count(&self) -> usize {
        self.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_event() {
        let mut scheduler = Scheduler::new();

        let _id = scheduler.schedule(EventType::VBlank, 100);
        assert_eq!(scheduler.pending_count(), 1);
        assert_eq!(scheduler.time_until_next_event(), Some(100));

        scheduler.advance(100);
        let mut processed = 0;
        scheduler.process_events(|_| {
            processed += 1;
        });
        assert_eq!(processed, 1);
        assert_eq!(scheduler.pending_count(), 0);
    }

    #[test]
    fn test_schedule_multiple_events() {
        let mut scheduler = Scheduler::new();

        scheduler.schedule(EventType::VBlank, 100);
        scheduler.schedule(EventType::HBlank, 50);
        scheduler.schedule(EventType::TimerOverflow(0), 150);

        assert_eq!(scheduler.pending_count(), 3);
        assert_eq!(scheduler.time_until_next_event(), Some(50));

        scheduler.advance(50);
        let mut events = Vec::new();
        scheduler.process_events(|e| {
            events.push(e);
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], EventType::HBlank);

        scheduler.advance(50);
        let mut events = Vec::new();
        scheduler.process_events(|e| {
            events.push(e);
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], EventType::VBlank);
    }

    #[test]
    fn test_cancel_event() {
        let mut scheduler = Scheduler::new();

        let _id1 = scheduler.schedule(EventType::VBlank, 100);
        let id2 = scheduler.schedule(EventType::HBlank, 50);

        assert!(scheduler.cancel(id2));
        assert_eq!(scheduler.pending_count(), 1);
        assert_eq!(scheduler.time_until_next_event(), Some(100));

        assert!(!scheduler.cancel(id2)); // Already cancelled
    }

    #[test]
    fn test_absolute_scheduling() {
        let mut scheduler = Scheduler::new();
        scheduler.set_current_time(1000);

        scheduler.schedule_absolute(EventType::VBlank, 1100);
        assert_eq!(scheduler.time_until_next_event(), Some(100));
    }
}
