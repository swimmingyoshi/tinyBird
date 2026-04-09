//! Cached debug configuration helpers.
//!
//! The emulator exposes a number of `TINYBIRD_*` environment variables for
//! tracing and watchpoints. Reading environment variables in instruction-hot
//! paths is expensive, so we parse them once and reuse the cached values.

use std::sync::OnceLock;

/// Cached debug configuration parsed from environment variables.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugConfig {
    /// Optional inclusive cycle trace range.
    pub trace_range: Option<(u64, u64)>,
    /// Whether PC transition tracing is enabled.
    pub pc_trace: bool,
    /// Whether IRQ tracing is enabled.
    pub irq_debug: bool,
    /// Whether DMA tracing is enabled.
    pub dma_debug: bool,
    /// Whether PPU register write tracing is enabled.
    pub ppu_debug: bool,
    /// Whether SWI tracing is enabled.
    pub swi_debug: bool,
    /// Whether CPU instruction tracing is enabled.
    pub cpu_debug: bool,
    /// Whether real-BIOS SWI tracing is enabled.
    pub real_swi_debug: bool,
    /// Whether compare-instruction tracing is enabled.
    pub cmp_debug: bool,
    /// Optional watched address for bus writes.
    pub watch_addr: Option<u32>,
    /// Minimum cycle threshold for watch logging.
    pub watch_after: u64,
}

fn parse_bool(name: &str) -> bool {
    std::env::var(name).is_ok()
}

fn parse_trace_range() -> Option<(u64, u64)> {
    let raw = std::env::var("TINYBIRD_TRACE_RANGE").ok()?;
    let mut parts = raw.split('-').filter_map(|s| s.parse::<u64>().ok());
    let start = parts.next()?;
    let end = parts.next()?;
    Some((start, end))
}

fn parse_watch_addr() -> Option<u32> {
    std::env::var("TINYBIRD_WATCH_ADDR")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
}

fn parse_watch_after() -> u64 {
    std::env::var("TINYBIRD_WATCH_AFTER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Return the cached debug configuration for this process.
pub fn config() -> &'static DebugConfig {
    static CONFIG: OnceLock<DebugConfig> = OnceLock::new();
    CONFIG.get_or_init(|| DebugConfig {
        trace_range: parse_trace_range(),
        pc_trace: parse_bool("TINYBIRD_PC_TRACE"),
        irq_debug: parse_bool("TINYBIRD_IRQ_DEBUG"),
        dma_debug: parse_bool("TINYBIRD_DMA_DEBUG"),
        ppu_debug: parse_bool("TINYBIRD_PPU_DEBUG"),
        swi_debug: parse_bool("TINYBIRD_SWI_DEBUG"),
        cpu_debug: parse_bool("TINYBIRD_CPU_DEBUG"),
        real_swi_debug: parse_bool("TINYBIRD_REAL_SWI_DEBUG"),
        cmp_debug: parse_bool("TINYBIRD_CMP_DEBUG"),
        watch_addr: parse_watch_addr(),
        watch_after: parse_watch_after(),
    })
}
