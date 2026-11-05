//! Polling AHCI subset behind IOB HBA addresses 50040000 and 50060000.
//! Standard command headers/FIS/PRDT; a blank 64 MiB volatile ATA device per HBA.
//! Physical PHY and the IOB-to-OMAP interrupt connection are not modeled.

use crate::iob::Memory;
use serde::Serialize;

const SECTORS: u32 = 131072;
const MAX_TRANSFER: usize = 1024 * 1024;

#[derive(Clone, Default, Serialize)]
pub struct Summary {
    pub sectors: u32,
    pub commands: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub errors: u64,
    pub allocated_bytes: usize,
    pub interrupt_pending: bool,
    pub last_error: Option<String>,
}

pub struct Ahci {
    global: [u32; 16],
    port: [u32; 32],
    disk: Memory,
    summary: Summary,
}

impl Default for Ahci {
    fn default() -> Self {
        let mut device = Self {
            global: [0; 16],
            port: [0; 32],
            disk: Memory::new(SECTORS * 512),
            summary: Summary {
                sectors: SECTORS,
                ..Default::default()
            },
        };
        device.reset();
        device
    }
}

impl Ahci {
    fn reset(&mut self) {
        self.global.fill(0);
        self.port.fill(0);
        self.global[0] = 31 << 8; // 32 slots, one port, 32-bit DMA, no NCQ.
        self.global[1] = 1 << 31;
        self.global[3] = 1;
        self.global[4] = 0x00010300;
        self.port[0x20 / 4] = 0x40;
        self.port[0x24 / 4] = 0x00000101;
        self.port[0x28 / 4] = 0x113;
    }

    pub fn summary(&self) -> Summary {
        let mut summary = self.summary.clone();
        summary.allocated_bytes = self.disk.allocated();
        summary.interrupt_pending = self.global[1] & 2 != 0 && self.global[2] & 1 != 0;
        summary
    }

    fn refresh_irq(&mut self) {
        if self.port[4] & self.port[5] != 0 {
            self.global[2] |= 1;
        }
    }

    pub fn access(
        &mut self,
        memory: &mut Memory,
        write: bool,
        offset: u32,
        value: u32,
    ) -> Option<u32> {
        if offset < 0x40 {
            let index = (offset / 4) as usize;
            if write {
                match offset {
                    4 if value & 1 != 0 => self.reset(),
                    4 => self.global[1] = value & ((1 << 31) | 2),
                    8 => self.global[2] &= !value,
                    _ => {}
                }
                self.refresh_irq();
            }
            return (!write).then_some(self.global[index]);
        }
        if !(0x100..0x180).contains(&offset) {
            return (!write).then_some(0);
        }
        let register = ((offset - 0x100) / 4) as usize;
        if write {
            match register {
                4 | 12 => self.port[register] &= !value,
                6 => {
                    self.port[6] = value & !(0xc008)
                        | if value & 1 != 0 { 1 << 15 } else { 0 }
                        | if value & 0x10 != 0 { 1 << 14 } else { 0 }
                }
                8..=10 => {} // Task-file, signature and link status are read-only.
                14 => {
                    self.port[14] |= value;
                    for slot in 0..32 {
                        if value & (1 << slot) != 0 {
                            self.issue(memory, slot);
                        }
                    }
                }
                _ => self.port[register] = value,
            }
            self.refresh_irq();
        }
        (!write).then_some(self.port[register])
    }

    fn issue(&mut self, memory: &mut Memory, slot: u32) {
        self.summary.commands += 1;
        let result = self.execute(memory, slot);
        let (status, transferred) = match result {
            Ok(bytes) => {
                self.summary.last_error = None;
                (0x50, bytes)
            }
            Err(error) => {
                self.summary.errors += 1;
                self.summary.last_error = Some(error);
                self.port[4] |= 1 << 30;
                (0x51, 0)
            }
        };
        self.port[8] = status | if status & 1 != 0 { 4 << 8 } else { 0 };
        self.port[4] |= 1;
        self.port[14] &= !(1 << slot);
        self.port[13] &= !(1 << slot);
        if self.port[1] == 0 {
            if let Some(header) = self.port[0].checked_add(slot * 32 + 4) {
                let _ = memory.write(header, &transferred.to_le_bytes());
            }
        }
        // D2H Register FIS, when the driver supplied a valid receive area.
        let base = self.port[2];
        if self.port[3] == 0 && base != 0 && base & 255 == 0 {
            let mut fis = [0; 20];
            fis[..4].copy_from_slice(&[
                0x34,
                0x40,
                status as u8,
                if status & 1 != 0 { 4 } else { 0 },
            ]);
            if let Some(address) = base.checked_add(0x40) {
                let _ = memory.write(address, &fis);
            }
        }
    }

    fn execute(&mut self, memory: &mut Memory, slot: u32) -> Result<u32, String> {
        if self.port[6] & 0x11 != 0x11 || self.port[0] & 1023 != 0 || self.port[1] != 0 {
            return Err("AHCI engine is stopped or command-list address is invalid".into());
        }
        let header = self.port[0]
            .checked_add(slot * 32)
            .ok_or("AHCI command-list overflow")?;
        let flags = memory.word(header)?;
        let entries = (flags >> 16) as usize;
        let table = memory.word(header + 8)?;
        if flags & 31 != 5 || entries > 128 || table & 127 != 0 || memory.word(header + 12)? != 0 {
            return Err("Unsupported AHCI command header or PRDT geometry".into());
        }
        let fis = memory.read(table, 20)?;
        if fis[0] != 0x27 || fis[1] & 0x80 == 0 {
            return Err("Invalid ATA host-to-device FIS".into());
        }
        let opcode = fis[2];
        if matches!(opcode, 0xe7 | 0xea) {
            return Ok(0);
        } // Volatile flush, no host file.
        let write = matches!(opcode, 0x30 | 0x35 | 0xca);
        let identify = opcode == 0xec;
        if !identify && !matches!(opcode, 0x20 | 0x25 | 0xc8 | 0x30 | 0x35 | 0xca) {
            return Err(format!("ATA command {opcode:02x} is not implemented"));
        }
        if (flags & 0x40 != 0) != write {
            return Err("AHCI DMA direction disagrees with ATA command".into());
        }
        let extended = matches!(opcode, 0x25 | 0x35);
        let lba = u64::from(fis[4])
            | (u64::from(fis[5]) << 8)
            | (u64::from(fis[6]) << 16)
            | if extended {
                (u64::from(fis[8]) << 24) | (u64::from(fis[9]) << 32) | (u64::from(fis[10]) << 40)
            } else {
                u64::from(fis[7] & 15) << 24
            };
        let count = if identify {
            1
        } else {
            let raw = u32::from(fis[12]) | if extended { u32::from(fis[13]) << 8 } else { 0 };
            if raw == 0 {
                if extended { 65536 } else { 256 }
            } else {
                raw
            }
        };
        let bytes = count as usize * 512;
        if bytes > MAX_TRANSFER || (!identify && lba + u64::from(count) > u64::from(SECTORS)) {
            return Err("ATA transfer is too large or outside the virtual SSD".into());
        }
        let mut segments = Vec::new();
        let mut covered = 0;
        for entry in 0..entries {
            let at = table
                .checked_add(0x80 + entry as u32 * 16)
                .ok_or("AHCI PRDT overflow")?;
            let address = memory.word(at)?;
            let length = ((memory.word(at + 12)? & 0x3fffff) + 1) as usize;
            if memory.word(at + 4)? != 0 || !memory.valid(address, length) || length > MAX_TRANSFER
            {
                return Err("AHCI PRDT points outside IOB DDR".into());
            }
            let take = length.min(bytes - covered);
            segments.push((address, take));
            covered += take;
            if covered == bytes {
                break;
            }
        }
        if covered != bytes {
            return Err("AHCI PRDT does not cover the entire ATA payload".into());
        }
        if write {
            let mut payload = Vec::with_capacity(bytes);
            for (address, length) in segments {
                payload.extend(memory.read(address, length)?);
            }
            self.disk.write((lba * 512) as u32, &payload)?;
            self.summary.written_bytes += bytes as u64;
        } else {
            let payload = if identify {
                identify_device()
            } else {
                self.disk.read((lba * 512) as u32, bytes)?
            };
            let mut offset = 0;
            for (address, length) in segments {
                memory.write(address, &payload[offset..offset + length])?;
                offset += length;
            }
            self.summary.read_bytes += bytes as u64;
        }
        Ok(bytes as u32)
    }
}

fn identify_device() -> Vec<u8> {
    let mut data = vec![0; 512];
    for (index, value) in [
        (0, 0x0040u16),
        (49, 0x0300),
        (53, 1),
        (60, SECTORS as u16),
        (61, (SECTORS >> 16) as u16),
        (80, 0x007e),
        (83, 0x4400),
        (86, 0x0400),
        (100, SECTORS as u16),
        (101, (SECTORS >> 16) as u16),
    ] {
        data[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, length, text) in [
        (20, 20, "VIRTUAL-PC-0001"),
        (46, 8, "0.1"),
        (54, 40, "DSMC VOLATILE RAM SSD"),
    ] {
        let mut padded = vec![b' '; length];
        padded[..text.len()].copy_from_slice(text.as_bytes());
        for pair in padded.chunks_exact_mut(2) {
            pair.swap(0, 1);
        }
        data[offset..offset + length].copy_from_slice(&padded);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(
        device: &mut Ahci,
        memory: &mut Memory,
        opcode: u8,
        lba: u64,
        bytes: usize,
        bad_buffer: bool,
    ) {
        let header = 0x400400;
        let table: u32 = 0x401000;
        let payload: u32 = if bad_buffer { 0xfffff000 } else { 0x410000 };
        let flags: u32 = 5
            | (1 << 16)
            | if matches!(opcode, 0x35 | 0x30 | 0xca) {
                0x40
            } else {
                0
            };
        memory.write(header, &flags.to_le_bytes()).unwrap();
        memory.write(header + 8, &table.to_le_bytes()).unwrap();
        let mut fis = [0; 20];
        fis[0] = 0x27;
        fis[1] = 0x80;
        fis[2] = opcode;
        fis[7] = 0x40;
        for (index, shift) in [(4, 0), (5, 8), (6, 16), (8, 24), (9, 32), (10, 40)] {
            fis[index] = (lba >> shift) as u8;
        }
        fis[12] = (bytes / 512) as u8;
        memory.write(table, &fis).unwrap();
        memory.write(table + 0x80, &payload.to_le_bytes()).unwrap();
        memory
            .write(table + 0x8c, &(bytes as u32 - 1).to_le_bytes())
            .unwrap();
        device.access(memory, true, 0x100, header);
        device.access(memory, true, 0x118, 0x11);
        device.access(memory, true, 0x114, u32::MAX);
        device.access(memory, true, 4, 0x80000002);
        device.access(memory, true, 0x138, 1);
        assert_eq!(device.access(memory, false, 0x138, 0), Some(0));
    }

    #[test]
    fn identify_write_read_flush_and_reset_preserve_the_volatile_media() {
        let mut device = Ahci::default();
        let mut memory = Memory::new(512 * 1024 * 1024);
        issue(&mut device, &mut memory, 0xec, 0, 512, false);
        assert_eq!(device.access(&mut memory, false, 0x120, 0), Some(0x50));
        assert_eq!(memory.word(0x410000 + 120).unwrap(), SECTORS);
        assert!(device.summary().interrupt_pending);
        device.access(&mut memory, true, 0x110, u32::MAX);
        device.access(&mut memory, true, 8, 1);
        assert!(!device.summary().interrupt_pending);
        let bytes: Vec<u8> = (0..512).map(|index| (index % 251) as u8).collect();
        memory.write(0x410000, &bytes).unwrap();
        issue(&mut device, &mut memory, 0x35, 17, 512, false);
        device.access(&mut memory, true, 4, 1);
        memory.write(0x410000, &[0; 512]).unwrap();
        issue(&mut device, &mut memory, 0x25, 17, 512, false);
        assert_eq!(memory.read(0x410000, 512).unwrap(), bytes);
        assert_eq!(memory.word(0x400404).unwrap(), 512);
        issue(&mut device, &mut memory, 0xea, 0, 512, false);
        assert_eq!(device.summary().errors, 0);
        assert_eq!(device.summary().written_bytes, 512);
        assert_eq!(device.summary().read_bytes, 1024);
    }

    #[test]
    fn invalid_dma_and_out_of_range_lba_fail_without_disk_writes() {
        let mut device = Ahci::default();
        let mut memory = Memory::new(512 * 1024 * 1024);
        issue(&mut device, &mut memory, 0x35, 0, 512, true);
        issue(
            &mut device,
            &mut memory,
            0x35,
            u64::from(SECTORS),
            512,
            false,
        );
        issue(&mut device, &mut memory, 0x99, 0, 512, false);
        assert_eq!(device.summary().errors, 3);
        assert_eq!(device.summary().allocated_bytes, 0);
        assert_eq!(device.access(&mut memory, false, 0x120, 0), Some(0x451));
        assert!(device.port[4] & (1 << 30) != 0);
    }
}
