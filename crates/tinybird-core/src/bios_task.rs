//! Interruptible bulk BIOS helpers. Each step performs bounded bus work so
//! timers, video and the serial cable can advance between pieces of a SWI.

use crate::{Bus, Cpu, CpuMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BiosTask {
    pc: u32,
    mode: CpuMode,
    sp: u32,
    thumb: bool,
    src: u32,
    dst: u32,
    remaining: u32,
    work: Work,
}

#[derive(Clone, Serialize, Deserialize)]
enum Work {
    Copy {
        width: u32,
        fill: bool,
        value: Option<u32>,
    },
    Decode {
        lz: bool,
        halfwords: bool,
        flags: u8,
        bits: u8,
        run: u32,
        offset: usize,
        repeated: Option<u8>,
        output: Vec<u8>,
    },
}

impl BiosTask {
    pub(crate) fn supports(comment: u8) -> bool {
        matches!(comment, 0x0b | 0x0c | 0x11 | 0x12 | 0x14 | 0x15)
    }

    pub(crate) fn new(comment: u8, cpu: &Cpu, bus: &mut impl Bus) -> Self {
        let mut src = cpu.registers.get_reg(0);
        let dst = cpu.registers.get_reg(1);
        let control = cpu.registers.get_reg(2);
        let (remaining, work) = if matches!(comment, 0x0b | 0x0c) {
            let count = control & 0x1f_ffff;
            (
                if comment == 0x0c {
                    (count + 7) & !7
                } else {
                    count
                },
                Work::Copy {
                    width: if comment == 0x0c || control & (1 << 24) != 0 {
                        4
                    } else {
                        2
                    },
                    fill: control & (1 << 26) != 0,
                    value: None,
                },
            )
        } else {
            let size = bus.read_u32(src) >> 8;
            src = src.wrapping_add(4);
            (
                size,
                Work::Decode {
                    lz: matches!(comment, 0x11 | 0x12),
                    halfwords: matches!(comment, 0x12 | 0x15),
                    flags: 0,
                    bits: 0,
                    run: 0,
                    offset: 0,
                    repeated: None,
                    output: Vec::new(),
                },
            )
        };
        Self {
            pc: cpu.fetch_addr(),
            mode: cpu.registers.mode(),
            sp: cpu.registers.sp(),
            thumb: cpu.is_thumb_mode(),
            src,
            dst,
            remaining,
            work,
        }
    }

    pub(crate) fn can_resume(&self, cpu: &Cpu) -> bool {
        self.pc == cpu.fetch_addr()
            && self.mode == cpu.registers.mode()
            && self.sp == cpu.registers.sp()
            && self.thumb == cpu.is_thumb_mode()
    }

    /// Return true when the SWI has completed. IRQ handlers may run between
    /// calls; the caller leaves the SWI's PC in place until the last piece.
    pub(crate) fn step(&mut self, bus: &mut impl Bus) -> bool {
        if self.remaining == 0 {
            return true;
        }
        match &mut self.work {
            Work::Copy { width, fill, value } => {
                let word = if *fill && value.is_some() {
                    value.unwrap()
                } else {
                    let word = if *width == 4 {
                        bus.read_u32(self.src)
                    } else {
                        bus.read_u16(self.src) as u32
                    };
                    *value = Some(word);
                    word
                };
                if *width == 4 {
                    bus.write_u32(self.dst, word);
                } else {
                    bus.write_u16(self.dst, word as u16);
                }
                if !*fill {
                    self.src = self.src.wrapping_add(*width);
                }
                self.dst = self.dst.wrapping_add(*width);
            }
            Work::Decode {
                lz,
                halfwords,
                flags,
                bits,
                run,
                offset,
                repeated,
                output,
            } => {
                if *run == 0 {
                    *repeated = None;
                    *offset = 0;
                    if *lz {
                        if *bits == 0 {
                            *flags = bus.read_u8(self.src);
                            self.src = self.src.wrapping_add(1);
                            *bits = 8;
                        }
                        let compressed = *flags & 0x80 != 0;
                        *flags <<= 1;
                        *bits -= 1;
                        if compressed {
                            let first = bus.read_u8(self.src);
                            let second = bus.read_u8(self.src.wrapping_add(1));
                            self.src = self.src.wrapping_add(2);
                            *run = (first >> 4) as u32 + 3;
                            *offset = (((first & 15) as usize) << 8 | second as usize) + 1;
                        } else {
                            *run = 1;
                        }
                    } else {
                        let flag = bus.read_u8(self.src);
                        self.src = self.src.wrapping_add(1);
                        *run = (flag & 127) as u32 + if flag & 128 != 0 { 3 } else { 1 };
                        if flag & 128 != 0 {
                            *repeated = Some(bus.read_u8(self.src));
                            self.src = self.src.wrapping_add(1);
                        }
                    }
                }
                let byte = if *offset != 0 {
                    output
                        .get(output.len().wrapping_sub(*offset))
                        .copied()
                        .unwrap_or(0)
                } else if let Some(byte) = *repeated {
                    byte
                } else {
                    let byte = bus.read_u8(self.src);
                    self.src = self.src.wrapping_add(1);
                    byte
                };
                output.push(byte);
                if !*halfwords {
                    bus.write_u8(self.dst, byte);
                    self.dst = self.dst.wrapping_add(1);
                } else if output.len() % 2 == 0 {
                    bus.write_u16(
                        self.dst,
                        u16::from_le_bytes([output[output.len() - 2], byte]),
                    );
                    self.dst = self.dst.wrapping_add(2);
                } else if self.remaining == 1 {
                    bus.write_u16(self.dst, byte as u16);
                }
                *run -= 1;
            }
        }
        self.remaining -= 1;
        self.remaining == 0
    }

    pub(crate) fn finish(&self, cpu: &mut Cpu) {
        cpu.pipeline.execute_addr = self.pc;
        cpu.registers
            .set_pc(self.pc.wrapping_add(if self.thumb { 4 } else { 8 }));
        cpu.pipeline
            .set_fetch_addr(self.pc.wrapping_add(if self.thumb { 2 } else { 4 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bios, SimpleBus};

    #[test]
    fn resumable_helpers_match_bulk_results() {
        // Literals, overlapping LZ backreferences, RLE runs, and odd VRAM
        // output lengths exercise the decoder state across yield points.
        let lz = [0x10, 9, 0, 0, 0x10, b'A', b'B', b'C', 0x30, 2];
        let rl = [0x30, 9, 0, 0, 0x83, b'A', 2, b'B', b'C', b'D'];
        for comment in [0x0b, 0x0c, 0x11, 0x12, 0x14, 0x15] {
            for fill in [false, true] {
                let mut cpu = Cpu::new();
                cpu.registers.set_reg(0, 0x0200_0000);
                cpu.registers.set_reg(1, 0x0200_1000);
                cpu.registers
                    .set_reg(2, 13 | (1 << 24) | if fill { 1 << 26 } else { 0 });
                let mut expected = SimpleBus::new(None);
                let data = if matches!(comment, 0x14 | 0x15) {
                    &rl[..]
                } else {
                    &lz[..]
                };
                for (i, byte) in data.iter().enumerate() {
                    expected.write_u8(0x0200_0000 + i as u32, *byte);
                }
                let mut actual = expected.clone();
                Bios::handle_swi(comment, &mut cpu.registers.clone(), &mut expected);
                let mut task = BiosTask::new(comment, &cpu, &mut actual);
                let mut steps = 0;
                loop {
                    actual.begin_instruction_timing();
                    let done = task.step(&mut actual);
                    assert!(actual.finish_instruction_timing() < 100);
                    steps += 1;
                    assert!(steps < 100);
                    // Every suspension point must survive serialization.
                    task = bincode::deserialize(&bincode::serialize(&task).unwrap()).unwrap();
                    if done {
                        break;
                    }
                }
                for i in 0..80 {
                    assert_eq!(
                        actual.read_u8(0x0200_1000 + i),
                        expected.read_u8(0x0200_1000 + i),
                        "SWI {comment:x}, byte {i}"
                    );
                }
            }
        }
    }
}
