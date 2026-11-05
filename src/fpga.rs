//! Software genlock FPGA register transport for RED 6.3.106.
//!
//! Protocol evidence: 0x807b5124 (read), 0x807b5208 (write), and
//! 0x807b5258 (scratch-register test). This is not the IOB/ISP video fabric.

use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

/// Byte-oriented IOB/ISP SPI transport, observed in fpga_write/fpga_read
/// (0x805de1c4/0x805de260). Only the command mailbox RAM is implemented.
/// Remote commands terminate with an explicit error, never invented success.
#[derive(Default)]
pub struct Remote {
    header: Vec<u8>,
    payload: Vec<u8>,
    reply: VecDeque<u8>,
    memory: BTreeMap<u32, u32>,
    pub reads: u64,
    pub writes: u64,
    pub unsupported: u64,
}

impl Remote {
    const MAILBOX: u32 = 0xe0000000;
    const STATUS: u32 = 0xe00003fc;

    pub fn deselect(&mut self) {
        self.header.clear();
        self.payload.clear();
        self.reply.clear();
    }

    fn address(&self) -> u32 {
        u32::from_le_bytes(self.header[1..5].try_into().expect("complete SPI header"))
    }

    fn read(&mut self, address: u32) -> u32 {
        self.reads += 1;
        if (Self::MAILBOX..=Self::STATUS).contains(&address) && address.is_multiple_of(4) {
            *self.memory.get(&address).unwrap_or(&0)
        } else {
            self.unsupported += 1;
            u32::MAX
        }
    }

    fn write(&mut self, address: u32, value: u32) {
        self.writes += 1;
        if (Self::MAILBOX..=Self::STATUS).contains(&address) && address.is_multiple_of(4) {
            self.memory.insert(address, value);
            if address == Self::MAILBOX && value != 0 {
                // Tensilica/IOB execution is absent: DONE with nonzero error status.
                self.memory.insert(Self::STATUS, 0x80000001);
                self.unsupported += 1;
            }
        } else {
            self.unsupported += 1;
        }
    }

    pub fn transfer(&mut self, byte: u8) -> u8 {
        if let Some(reply) = self.reply.pop_front() {
            return reply;
        }
        if self.header.len() < 5 {
            self.header.push(byte);
            if self.header.len() == 5 && self.header[0] & 0x80 == 0 {
                let words = self.header[0] & 0x7f;
                if words == 0 {
                    self.unsupported += 1;
                    return 0xff;
                }
                self.reply.push_back(0x1f);
                for word in 0..words {
                    let address = self.address().wrapping_add(u32::from(word) * 4);
                    let value = self.read(address);
                    self.reply.extend(value.to_le_bytes());
                }
            }
        } else if self.header[0] & 0x80 != 0 {
            let words = (self.header[0] & 0x7f) as usize;
            if words == 0 || self.payload.len() >= words * 4 {
                return 0xff;
            }
            self.payload.push(byte);
            if self.payload.len().is_multiple_of(4) {
                let offset = self.payload.len() - 4;
                let address = self.address().wrapping_add(offset as u32);
                let value = u32::from_le_bytes(self.payload[offset..].try_into().unwrap());
                self.write(address, value);
            }
        }
        0xff
    }
}

#[derive(Clone, Default, Serialize)]
pub struct Summary {
    pub backend: &'static str,
    pub register_reads: u64,
    pub register_writes: u64,
    pub configuration_bytes: u64,
    pub configured: bool,
    pub unsupported_registers: u64,
    pub video_pipeline_emulated: bool,
    pub remote_reads: u64,
    pub remote_writes: u64,
    pub remote_unsupported: u64,
}

pub struct Genlock {
    registers: [u16; 32],
    address: u16,
    phase: u8,
    read: bool,
    program: bool,
    summary: Summary,
}

impl Default for Genlock {
    fn default() -> Self {
        Self {
            registers: [0; 32],
            address: 0,
            phase: 0,
            read: false,
            program: true,
            summary: Summary {
                backend: "software-genlock-registers",
                ..Default::default()
            },
        }
    }
}

impl Genlock {
    pub fn summary(&self) -> Summary {
        self.summary.clone()
    }

    pub fn deselect(&mut self) {
        self.phase = 0;
    }

    /// GPIO138 is PROGRAM; GPIO162 INIT follows it in this functional model.
    pub fn program(&mut self, high: bool) {
        if !high && self.program {
            self.registers.fill(0);
            self.summary.configured = false;
            self.summary.configuration_bytes = 0;
            self.deselect();
        }
        self.program = high;
    }

    pub fn init_high(&self) -> bool {
        self.program
    }

    pub fn done_high(&self) -> bool {
        self.summary.configured
    }

    /// The update stream is consumed, not executed as an FPGA bitstream.
    /// DONE means the software transport has received a configuration payload.
    pub fn configure(&mut self, byte: u8) {
        if self.program {
            self.summary.configuration_bytes += 1;
            // The loader appends a zero byte. Do not assert DONE for padding alone.
            self.summary.configured |= byte != 0;
        }
    }

    /// One 16-bit McSPI word, with chip select held across the transaction.
    pub fn transfer(&mut self, word: u16) -> u16 {
        let phase = self.phase;
        self.phase = self.phase.saturating_add(1);
        if phase == 0 {
            self.read = word & 0x8000 != 0;
            self.address = word & 0x7fff;
            return 0;
        }
        let index = self.address as usize;
        if self.read {
            match phase {
                2 => 0x5a5a,
                3 => {
                    self.summary.register_reads += 1;
                    if let Some(value) = self.registers.get(index) {
                        *value
                    } else {
                        self.summary.unsupported_registers += 1;
                        u16::MAX
                    }
                }
                _ => 0,
            }
        } else {
            if phase == 1 {
                // Version is a read-only, synthetic 0.0 identifier, not camera hardware.
                if index != 0x0e {
                    if let Some(value) = self.registers.get_mut(index) {
                        *value = word;
                        self.summary.register_writes += 1;
                    } else {
                        self.summary.unsupported_registers += 1;
                    }
                }
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_write(remote: &mut Remote, address: u32, value: u32) {
        remote.deselect();
        for byte in [0x81]
            .into_iter()
            .chain(address.to_le_bytes())
            .chain(value.to_le_bytes())
        {
            remote.transfer(byte);
        }
    }

    fn remote_read(remote: &mut Remote, address: u32) -> u32 {
        remote.deselect();
        for byte in [1].into_iter().chain(address.to_le_bytes()) {
            remote.transfer(byte);
        }
        assert_eq!(remote.transfer(0), 0x1f);
        u32::from_le_bytes(std::array::from_fn(|_| remote.transfer(0)))
    }

    #[test]
    fn remote_mailbox_round_trip_and_command_failure_are_explicit() {
        let mut remote = Remote::default();
        remote_write(&mut remote, 0xe0000004, 0x12345678);
        assert_eq!(remote_read(&mut remote, 0xe0000004), 0x12345678);
        remote_write(&mut remote, 0xe0000000, 0xe0000004);
        assert_eq!(remote_read(&mut remote, 0xe00003fc), 0x80000001);
        assert_eq!(remote_read(&mut remote, 0x1234), u32::MAX);
        assert_eq!(remote.unsupported, 2);
    }

    #[test]
    fn deselection_discards_incomplete_remote_writes() {
        let mut remote = Remote::default();
        for byte in [0x81, 4, 0, 0, 0xe0, 0xff, 0xff] {
            remote.transfer(byte);
        }
        assert_eq!(remote_read(&mut remote, 0xe0000004), 0);
        assert_eq!(remote.writes, 0);
    }

    #[test]
    fn native_scratch_and_clock_register_transactions() {
        let mut fpga = Genlock::default();
        for (address, value) in [(3, 0xbabe), (0x0c, 0x1ff), (9, 2), (0x15, 0x20)] {
            fpga.deselect();
            fpga.transfer(address);
            fpga.transfer(value);
            fpga.deselect();
            let reply: Vec<_> = [address | 0x8000, 0, 0, 0]
                .into_iter()
                .map(|word| fpga.transfer(word))
                .collect();
            assert_eq!(reply, [0, 0, 0x5a5a, value]);
        }
        assert_eq!(fpga.summary().register_reads, 4);
        assert_eq!(fpga.summary().register_writes, 4);
    }

    #[test]
    fn reset_aborts_transactions_and_clears_configuration() {
        let mut fpga = Genlock::default();
        fpga.configure(0);
        assert!(!fpga.done_high());
        fpga.configure(0xaa);
        assert!(fpga.done_high());
        fpga.transfer(3);
        fpga.program(false);
        assert!(!fpga.init_high());
        assert!(!fpga.done_high());
        fpga.configure(0xff);
        assert_eq!(fpga.summary().configuration_bytes, 0);
        fpga.program(true);
        assert!(fpga.init_high());
        assert_eq!(fpga.transfer(0x8003), 0);
        assert_eq!(fpga.transfer(0), 0);
        assert_eq!(fpga.transfer(0), 0x5a5a);
        assert_eq!(fpga.transfer(0), 0);
    }
}
