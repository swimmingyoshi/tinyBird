# tinyBird Development Progress

> **A Game Boy Advance Emulator written in Rust**

Last Updated: March 26, 2026

---

## Table of Contents

1. [Project Overview](#project-overview)
2. [Phase Status](#phase-status)
3. [Test Results](#test-results)
4. [Known Issues](#known-issues)
5. [Build Instructions](#build-instructions)
6. [Architecture Summary](#architecture-summary)

---

## Project Overview

**tinyBird** is a Game Boy Advance (GBA) emulator written in Rust. The project aims to provide accurate emulation of the GBA hardware while maintaining clean, well-documented, and performant code.

### Key Features

- **ARM7TDMI CPU Core**: Full implementation of ARM and Thumb instruction sets
- **Memory System**: Complete GBA memory map with proper mirroring and open bus behavior
- **Event Scheduler**: Timing system for synchronized emulation of hardware events
- **Modular Architecture**: Clean separation of concerns across multiple modules

### Target Hardware

- **CPU**: ARM7TDMI (ARMv4T architecture)
- **Clock Speed**: 16.78 MHz
- **Screen**: 240×160 pixels
- **Memory**: Various regions including BIOS, EWRAM, IWRAM, VRAM, OAM, and more

---

## Phase Status

### Phase 1: Core CPU Implementation ✅

- [x] ARM instruction set decoder
- [x] Thumb instruction set decoder
- [x] Register file with mode banking (37 registers)
- [x] CPSR/SPSR status registers
- [x] Condition code evaluation
- [x] 3-stage pipeline simulation (Fetch-Decode-Execute)
- [x] Data processing instructions (AND, EOR, SUB, RSB, ADD, ADC, SBC, RSC, TST, TEQ, CMP, CMN, ORR, MOV, BIC, MVN)
- [x] Multiply instructions (MUL, MLA)
- [x] Load/Store instructions (LDR, STR, LDRB, STRB)
- [x] Branch instructions (B, BL, BX, BLX)
- [x] SWI exception handling (stub)
- [x] Processor mode switching (User, FIQ, IRQ, Supervisor, Abort, Undefined, System)

### Phase 2: Memory System ✅

- [x] Memory map implementation
- [x] BIOS region (32KB, read-only after boot)
- [x] EWRAM - External Work RAM (256KB)
- [x] IWRAM - Internal Work RAM (32KB)
- [x] I/O registers (1KB)
- [x] Palette RAM (1KB)
- [x] VRAM - Video RAM (96KB)
- [x] OAM - Object Attribute Memory (1KB)
- [x] ROM region (max 32MB with mirroring)
- [x] SRAM (32KB)
- [x] Memory mirroring for all regions
- [x] Open bus behavior
- [x] Wait state configuration

### Phase 3: Infrastructure ✅

- [x] Event scheduler for timed events
- [x] VBlank/HBlank interrupt scheduling
- [x] Timer overflow events
- [x] DMA events
- [x] Top-level GBA struct
- [x] ROM loading
- [x] BIOS loading (optional)
- [x] Reset functionality
- [x] Step-by-step execution
- [x] Frame-based execution

### Phase 4: Display System 🔄

- [ ] LCD controller implementation
- [ ] Background layers (BG0-BG3)
- [ ] Sprite/object rendering
- [ ] Window system
- [ ] Color effects
- [ ] Framebuffer management
- [ ] Display modes (0-6)

### Phase 5: Input System 🔄

- [ ] Button input handling
- [ ] Key input register (KEYINPUT)
- [ ] Interrupt on key press
- [ ] Game linker port emulation

### Phase 6: Audio System 🔄

- [ ] PSG (Programmable Sound Generator) channels
- [ ] DMA sound support
- [ ] FIFO sound support
- [ ] Audio output

### Phase 7: Save System 🔄

- [ ] SRAM save support
- [ ] Flash save support
- [ ] EEPROM save support
- [ ] Save state (savestate) functionality

### Phase 8: Performance & Optimization 🔄

- [ ] Instruction caching
- [ ] Dynamic recompilation (future)
- [ ] Threaded rendering
- [ ] Frame skipping

### Phase 9: Frontend (Future) 🔄

- [ ] SDL2/OpenGL frontend
- [ ] GUI interface
- [ ] Debugging tools
- [ ] Cheats support

---

## Test Results

### Current Status: ✅ 31/31 Tests Passing

All tests pass with no failures.

### Test Coverage by Module

| Module | Tests | Status |
|--------|-------|--------|
| `cpu::arm` | 3 | ✅ Passing |
| `cpu::thumb` | 2 | ✅ Passing |
| `cpu::pipeline` | 5 | ✅ Passing |
| `cpu::registers` | 5 | ✅ Passing |
| `cpu` (general) | 2 | ✅ Passing |
| `bus` | 4 | ✅ Passing |
| `gba` | 4 | ✅ Passing |
| `scheduler` | 4 | ✅ Passing |
| `tests` (lib) | 2 | ✅ Passing |
| **Total** | **31** | **✅ All Passing** |

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test module
cargo test cpu::arm

# Run tests with coverage (requires cargo-tarpaulin)
cargo tarpaulin --out Html
```

---

## Known Issues

### Current Issues

1. **SWI Implementation** ⚠️
   - Software Interrupt handler is a stub
   - No BIOS syscall emulation
   - Priority: Medium

2. **Exception Handling** ⚠️
   - IRQ/FIQ exception entry partially implemented
   - Exception vector table not fully utilized
   - Priority: Medium

3. **Missing Instructions** ⚠️
   - Some multiply instructions (UMULL, UMLAL, SMULL, SMLAL) not fully implemented
   - Halfword load/store (LDRH, STRH, LDRSB, LDRSH) not implemented
   - Load/Store Multiple (LDM, STM) not implemented
   - Priority: High

4. **No Display/Audio** ⚠️
   - PPU (Picture Processing Unit) not implemented
   - No graphics output
   - Audio system not implemented
   - Priority: High

5. **No Frontend** ⚠️
   - Library-only implementation
   - No GUI or display frontend
   - Priority: Medium

### Completed Items (No Longer Issues)

- ✅ ~~Compiler warnings~~ - All warnings resolved
- ✅ ~~Unused imports~~ - Cleaned up
- ✅ ~~Missing documentation~~ - Added `#[allow(missing_docs)]` where appropriate
- ✅ ~~Unused variables~~ - Fixed (e.g., `cond` parameter in ARM decoder)
- ✅ ~~Unnecessary mut keywords~~ - Removed

---

## Build Instructions

### Prerequisites

- **Rust**: 1.70.0 or later (2021 edition)
- **Cargo**: Rust package manager (included with Rust)

### Building

```bash
# Clone the repository
git clone https://github.com/your-org/tinybird.git
cd tinybird

# Build in debug mode
cargo build

# Build in release mode (optimized)
cargo build --release

# Build documentation
cargo doc --open
```

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with verbose output
cargo test --verbose

# Run only unit tests
cargo test --lib

# Run tests for specific module
cargo test --package tinybird-core
```

### Checking and Linting

```bash
# Check for errors without building
cargo check

# Run clippy linter
cargo clippy

# Format code
cargo fmt

# Verify formatting
cargo fmt --check
```

### Workspace Structure

```bash
tinybird/
├── Cargo.toml              # Workspace root
├── PROGRESS.md             # This file
└── crates/
    └── tinybird-core/      # Core emulation library
        ├── Cargo.toml
        └── src/
            ├── lib.rs      # Library entry point
            ├── bus.rs      # Memory bus implementation
            ├── gba.rs      # Top-level GBA struct
            ├── memory_map.rs # Memory map constants
            ├── scheduler.rs # Event scheduler
            └── cpu/        # CPU implementation
                ├── mod.rs
                ├── arm.rs  # ARM instruction decoder/executor
                ├── thumb.rs # Thumb instruction decoder/executor
                ├── pipeline.rs # Pipeline simulation
                └── registers.rs # Register file
```

---

## Architecture Summary

### Crate Structure

```
tinybird-core (Main Library)
│
├── cpu/                    # CPU Core
│   ├── arm.rs             # ARM (32-bit) instruction set
│   ├── thumb.rs           # Thumb (16-bit) instruction set
│   ├── pipeline.rs        # 3-stage pipeline simulation
│   └── registers.rs       # Register file with mode banking
│
├── bus.rs                  # Memory bus and access traits
├── memory_map.rs          # Memory region constants
├── scheduler.rs           # Event scheduling system
├── gba.rs                 # Top-level emulator struct
└── lib.rs                 # Library entry and re-exports
```

### Key Components

#### 1. CPU Core (`cpu/`)

The CPU module implements the ARM7TDMI processor:

- **ARM Decoder/Executor**: Handles 32-bit ARM instructions
- **Thumb Decoder/Executor**: Handles 16-bit Thumb instructions
- **Pipeline**: Simulates 3-stage pipeline (Fetch-Decode-Execute)
- **Registers**: 37 registers with mode banking

```rust
// Example: Creating and using the CPU
let mut cpu = Cpu::new();
cpu.reset();
cpu.step(&mut bus);
```

#### 2. Memory Bus (`bus.rs`)

Implements the GBA memory bus with:

- Address decoding
- Memory mirroring
- Open bus behavior
- Read/write access traits

```rust
// Bus trait
pub trait Bus {
    fn read_u8(&self, addr: u32) -> u8;
    fn read_u16(&self, addr: u32) -> u16;
    fn read_u32(&self, addr: u32) -> u32;
    fn write_u8(&mut self, addr: u32, value: u8);
    fn write_u16(&mut self, addr: u32, value: u16);
    fn write_u32(&mut self, addr: u32, value: u32);
}
```

#### 3. Memory Map (`memory_map.rs`)

Defines all GBA memory regions:

| Region | Start | End | Size |
|--------|-------|-----|------|
| BIOS | 0x00000000 | 0x00003FFF | 32KB |
| EWRAM | 0x02000000 | 0x0203FFFF | 256KB |
| IWRAM | 0x03000000 | 0x03007FFF | 32KB |
| I/O | 0x04000000 | 0x040003FF | 1KB |
| Palette | 0x05000000 | 0x050003FF | 1KB |
| VRAM | 0x06000000 | 0x06017FFF | 96KB |
| OAM | 0x07000000 | 0x070003FF | 1KB |
| ROM | 0x08000000 | 0x09FFFFFF | 32MB max |
| SRAM | 0x0E000000 | 0x0E007FFF | 32KB |

#### 4. Event Scheduler (`scheduler.rs`)

Manages timed events for:

- VBlank/HBlank interrupts
- Timer overflows
- DMA transfers
- Custom events

```rust
// Schedule an event
scheduler.schedule(EventType::VBlank, cycles_until_event);
```

#### 5. GBA Struct (`gba.rs`)

Top-level emulator structure that ties everything together:

```rust
pub struct Gba {
    pub cpu: Cpu,
    pub bus: SimpleBus,
    pub scheduler: Scheduler,
    pub state: GbaState,
    // ... other fields
}
```

### Data Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│    Fetch    │────▶│   Decode    │────▶│   Execute   │
│  (Pipeline) │     │  (ARM/Thumb)│     │  (CPU Ops)  │
└─────────────┘     └─────────────┘     └─────────────┘
       │                   │                   │
       ▼                   ▼                   ▼
┌─────────────────────────────────────────────────────┐
│                      Bus                             │
│  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐      │
│  │ BIOS │ │ RAM  │ │  I/O │ │ VRAM │ │ ROM  │ ...  │
│  └──────┘ └──────┘ └──────┘ └──────┘ └──────┘      │
└─────────────────────────────────────────────────────┘
                           │
                           ▼
                  ┌─────────────────┐
                  │   Scheduler     │
                  │  (Timed Events) │
                  └─────────────────┘
```

### Design Principles

1. **Accuracy First**: Prioritize accurate emulation over performance
2. **Modular Design**: Clean separation between components
3. **Test Coverage**: Comprehensive unit tests for all modules
4. **Documentation**: Well-documented public APIs
5. **Idiomatic Rust**: Follow Rust best practices and conventions

---

## Development Roadmap

### Short Term (Next Sprint)

- [ ] Implement remaining load/store instructions
- [ ] Complete multiply instruction set
- [ ] Add basic LCD controller stub
- [ ] Implement interrupt handling

### Medium Term

- [ ] Background rendering (Mode 0)
- [ ] Sprite rendering
- [ ] Timer implementation
- [ ] DMA implementation
- [ ] Basic frontend (SDL2)

### Long Term

- [ ] Full display system (all modes)
- [ ] Audio emulation
- [ ] Save system
- [ ] Performance optimization
- [ ] Debugger integration

---

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Write tests for new functionality
4. Ensure all tests pass
5. Submit a pull request

---

## License

MIT License - See LICENSE file for details

---

## Acknowledgments

- [GBATEK](https://problemkaputt.de/gbatek.htm) - GBA technical documentation
- [ARM7TDMI Manual](https://developer.arm.com/documentation/ddi0234/latest/) - ARM architecture reference
- Rust community for excellent tooling and libraries
