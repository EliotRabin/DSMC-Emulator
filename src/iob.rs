//! Banked GPMC -> IOB bus, observed at 805f2e8c/805f3090 in RED 6.3.106.
//! DDR and SSD contents are synthetic, bounded and volatile. No host disk is opened.

use crate::sata::Ahci;
use serde::Serialize;
use std::collections::BTreeMap;

pub struct Memory {
    size: u32,
    pages: BTreeMap<u32, Box<[u8; 4096]>>,
}

impl Memory {
    pub fn new(size: u32) -> Self {
        Self {
            size,
            pages: BTreeMap::new(),
        }
    }

    pub fn valid(&self, address: u32, length: usize) -> bool {
        length <= 2 * 1024 * 1024 && address as u64 + length as u64 <= u64::from(self.size)
    }

    pub fn read(&self, address: u32, length: usize) -> Result<Vec<u8>, String> {
        if !self.valid(address, length) {
            return Err("IOB memory range is invalid".into());
        }
        let mut result = vec![0; length];
        let mut copied = 0;
        while copied < length {
            let at = address + copied as u32;
            let offset = (at % 4096) as usize;
            let count = (4096 - offset).min(length - copied);
            if let Some(page) = self.pages.get(&(at / 4096)) {
                result[copied..copied + count].copy_from_slice(&page[offset..offset + count]);
            }
            copied += count;
        }
        Ok(result)
    }

    pub fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), String> {
        if !self.valid(address, bytes.len()) {
            return Err("IOB memory range is invalid".into());
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let first = address / 4096;
        let last = (address + bytes.len() as u32 - 1) / 4096;
        let new_pages = (first..=last)
            .filter(|page| !self.pages.contains_key(page))
            .count();
        if self.pages.len() + new_pages > 8192 {
            return Err("Volatile memory overlay reached its 32 MiB limit".into());
        }
        for page in first..=last {
            self.pages
                .entry(page)
                .or_insert_with(|| Box::new([0; 4096]));
        }
        let mut copied = 0;
        while copied < bytes.len() {
            let at = address + copied as u32;
            let offset = (at % 4096) as usize;
            let count = (4096 - offset).min(bytes.len() - copied);
            self.pages.get_mut(&(at / 4096)).unwrap()[offset..offset + count]
                .copy_from_slice(&bytes[copied..copied + count]);
            copied += count;
        }
        Ok(())
    }

    pub fn word(&self, address: u32) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.read(address, 4)?.try_into().unwrap(),
        ))
    }

    pub fn allocated(&self) -> usize {
        self.pages.len() * 4096
    }
}

#[derive(Clone, Default, Serialize)]
pub struct Summary {
    pub ddr_allocated_bytes: usize,
    pub bank: u32,
    pub unmapped_accesses: u64,
    pub last_unmapped_address: Option<u32>,
    pub sata: Vec<crate::sata::Summary>,
    pub unmapped_addresses: BTreeMap<u32, u64>,
}

pub struct Iob {
    bank: u32,
    memory: Memory,
    sata: [Ahci; 2],
    unmapped: u64,
    last_unmapped: Option<u32>,
    unmapped_addresses: BTreeMap<u32, u64>,
}

impl Default for Iob {
    fn default() -> Self {
        Self {
            bank: 0,
            memory: Memory::new(512 * 1024 * 1024),
            sata: std::array::from_fn(|_| Ahci::default()),
            unmapped: 0,
            last_unmapped: None,
            unmapped_addresses: BTreeMap::new(),
        }
    }
}

impl Iob {
    pub fn access(
        &mut self,
        write: bool,
        address: u32,
        size: usize,
        value: u32,
    ) -> Option<Option<u32>> {
        if address == 0x18000018 {
            if write {
                self.bank = value & 63;
            }
            return Some((!write).then_some(self.bank));
        }
        if !(0x1c000000..0x20000000).contains(&address) {
            return None;
        }
        let remote = (self.bank << 26) | (address & 0x03ffffff);
        if ![1, 2, 4].contains(&size) {
            self.unknown(remote);
            return Some(Some(u32::MAX));
        }
        if self.memory.valid(remote, size) {
            if write {
                if self
                    .memory
                    .write(remote, &value.to_le_bytes()[..size])
                    .is_err()
                {
                    self.unknown(remote);
                }
                return Some(None);
            }
            let mut bytes = [0; 4];
            bytes[..size].copy_from_slice(&self.memory.read(remote, size).unwrap());
            return Some(Some(u32::from_le_bytes(bytes)));
        }
        for (index, base) in [0x50040000, 0x50060000].into_iter().enumerate() {
            if (base..base + 0x1000).contains(&remote) && size == 4 && remote.is_multiple_of(4) {
                return Some(self.sata[index].access(
                    &mut self.memory,
                    write,
                    remote - base,
                    value,
                ));
            }
        }
        self.unknown(remote);
        Some((!write).then_some(u32::MAX))
    }

    fn unknown(&mut self, address: u32) {
        self.unmapped += 1;
        self.last_unmapped = Some(address);
        if self.unmapped_addresses.len() < 128 || self.unmapped_addresses.contains_key(&address) {
            *self.unmapped_addresses.entry(address).or_default() += 1;
        }
    }

    pub fn summary(&self) -> Summary {
        Summary {
            ddr_allocated_bytes: self.memory.allocated(),
            bank: self.bank,
            unmapped_accesses: self.unmapped,
            last_unmapped_address: self.last_unmapped,
            unmapped_addresses: self.unmapped_addresses.clone(),
            sata: self.sata.iter().map(Ahci::summary).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banked_window_keeps_ddr_banks_distinct_and_decodes_sata() {
        let mut bus = Iob::default();
        bus.access(true, 0x1c000004, 4, 0x1234);
        bus.access(true, 0x18000018, 2, 1);
        assert_eq!(bus.access(false, 0x1c000004, 4, 0), Some(Some(0)));
        bus.access(true, 0x18000018, 2, 0);
        assert_eq!(bus.access(false, 0x1c000004, 4, 0), Some(Some(0x1234)));
        bus.access(true, 0x18000018, 2, 0x14);
        assert_eq!(bus.access(false, 0x1c04000c, 4, 0), Some(Some(1)));
        assert_eq!(bus.access(false, 0x1c080000, 4, 0), Some(Some(u32::MAX)));
        assert_eq!(bus.summary().unmapped_accesses, 1);
    }

    #[test]
    fn invalid_memory_ranges_do_not_wrap_or_allocate() {
        let mut memory = Memory::new(4096);
        assert!(memory.write(u32::MAX, &[1, 2]).is_err());
        assert!(memory.read(4095, 2).is_err());
        assert_eq!(memory.allocated(), 0);
    }
}
