//! OMAP3 CCDC subset: synthetic RAW8/unpacked RAW capture into guest RAM, IRQ24.
//! Register evidence: camisp init/enable/ISR at 80560b44/80561068/805608e4.
//! Register layout cross-checked with Linux's TI omap3isp/ispreg.h.

use crate::{
    devices::Interrupts,
    display::{Frame, valid},
    unicorn::Cpu,
};
use serde::Serialize;

pub const BASE: u32 = 0x480bc000;
const FRAME_IRQ: u32 = (1 << 31) | (1 << 8);
const ERROR_IRQ: u32 = 1 << 29;

#[derive(Clone, Default, Serialize)]
pub struct Summary {
    pub enabled: bool,
    pub frames: u64,
    pub bytes: u64,
    pub overruns: u64,
    pub rejected: u64,
    pub last_error: Option<String>,
}

pub struct Ccdc {
    registers: [u32; 448],
    summary: Summary,
}

impl Default for Ccdc {
    fn default() -> Self {
        let mut registers = [0; 448];
        registers[8 / 4] = 1;
        Self {
            registers,
            summary: Summary::default(),
        }
    }
}

impl Ccdc {
    fn irq(&self, irq: &mut Interrupts) {
        irq.level(24, self.registers[0x10 / 4] & self.registers[0x0c / 4] != 0);
    }

    pub fn access(
        &mut self,
        write: bool,
        offset: u32,
        value: u32,
        irq: &mut Interrupts,
    ) -> Option<u32> {
        if offset >= 0x400 && !(0x600..0x700).contains(&offset) || !offset.is_multiple_of(4) {
            return Some(0);
        }
        let index = (offset / 4) as usize;
        if write {
            match offset {
                4 if value & 2 != 0 => {
                    *self = Self::default();
                }
                0x10 => self.registers[index] &= !value,
                8 => {}
                _ => self.registers[index] = value,
            }
        }
        self.summary.enabled = self.registers[0x604 / 4] & 1 != 0;
        self.irq(irq);
        (!write).then_some(self.registers[index])
    }

    pub fn summary(&self) -> Summary {
        self.summary.clone()
    }

    pub fn receive(&mut self, cpu: Cpu, source: &Frame, irq: &mut Interrupts) {
        if self.registers[0x604 / 4] & 1 == 0 {
            return;
        }
        if self.registers[0x10 / 4] & self.registers[0x0c / 4] & FRAME_IRQ != 0 {
            self.summary.overruns += 1;
            return;
        }
        let result = self.write_frame(cpu, source);
        match result {
            Ok(bytes) => {
                self.summary.frames += 1;
                self.summary.bytes += bytes as u64;
                self.summary.last_error = None;
                self.registers[0x10 / 4] |= FRAME_IRQ;
            }
            Err(error) => {
                self.summary.rejected += 1;
                self.summary.last_error = Some(error);
                self.registers[0x10 / 4] |= ERROR_IRQ;
            }
        }
        self.irq(irq);
    }

    fn write_frame(&self, cpu: Cpu, source: &Frame) -> Result<usize, String> {
        let mode = self.registers[0x608 / 4];
        if mode & (1 << 17) == 0 {
            return Err("CCDC memory write is disabled".into());
        }
        if mode & ((3 << 12) | (1 << 7)) != 0 || self.registers[0x628 / 4] != 0 {
            return Err(
                "CCDC supports progressive RAW only; YUV/interlaced layout is not implemented"
                    .into(),
            );
        }
        let width = (self.registers[0x614 / 4] & 0x7fff) as usize + 1;
        let height = (self.registers[0x61c / 4] & 0x7fff) as usize + 1;
        let stride = (self.registers[0x624 / 4] & 0xffe0) as usize;
        let address = self.registers[0x62c / 4];
        let packed = mode & (1 << 11) != 0;
        let bits = match (mode >> 8) & 7 {
            0 => 16,
            4 => 12,
            5 => 11,
            6 => 10,
            7 => 8,
            _ => return Err("Unsupported CCDC RAW depth".into()),
        };
        if packed && bits != 8 {
            return Err("Only 8-bit packed RAW is implemented".into());
        }
        let row_bytes = width * if packed { 1 } else { 2 };
        if width > 1920
            || height > 1080
            || stride < row_bytes
            || address & 31 != 0
            || !valid(address, stride * (height - 1) + row_bytes)
            || source.width == 0
            || source.height == 0
            || source.width > 1920
            || source.height > 1080
            || source.rgba.len() != source.width * source.height * 4
        {
            return Err("Invalid CCDC frame geometry or destination buffer".into());
        }
        let mut row = vec![0; row_bytes];
        for y in 0..height {
            let source_y = y * source.height / height;
            for x in 0..width {
                let offset = (source_y * source.width + x * source.width / width) * 4;
                let rgb = &source.rgba[offset..offset + 3];
                let gray =
                    ((u32::from(rgb[0]) * 77 + u32::from(rgb[1]) * 150 + u32::from(rgb[2]) * 29)
                        >> 8) as u16;
                if packed {
                    row[x] = gray as u8;
                } else {
                    row[x * 2..x * 2 + 2].copy_from_slice(&(gray << (bits - 8)).to_le_bytes());
                }
            }
            cpu.write(address + (y * stride) as u32, &row)?;
        }
        Ok(row_bytes * height)
    }
}
