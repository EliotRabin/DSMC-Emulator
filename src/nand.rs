//! Volatile x16 NAND: synthetic 2c:ba, 2048+64-byte pages, no ECC/calibration.
use std::collections::BTreeMap;
const PAGE: usize = 2112;
const PAGES: u32 = 131072;
#[derive(Default)]
pub struct Nand {
    mode: u8,
    addr: Vec<u8>,
    cursor: usize,
    row: u32,
    pages: BTreeMap<u32, Vec<u8>>,
    program: Vec<u8>,
    pub status: u8,
    latched: bool,
    range: (u32, u32),
    unlock_start: u32,
    overflow: bool,
    pub programs: u64,
    pub erases: u64,
    pub read_bytes: u64,
}

impl Nand {
    pub fn new() -> Self {
        Self {
            status: 0xc0,
            range: (0, PAGES - 1),
            ..Default::default()
        }
    }

    pub fn address(&mut self, v: u8) {
        if self.addr.len() < 5 {
            self.addr.push(v)
        }
        if self.mode == 1 {
            self.cursor = self.addr[0] as usize
        } else if self.mode == 7 && self.addr.len() == 3 {
            self.unlock_start = self.decode(true).0
        } else if self.mode == 8 && self.addr.len() == 3 {
            self.range = (self.unlock_start, self.decode(true).0)
        }
    }

    fn decode(&self, erase: bool) -> (u32, usize) {
        let byte = |i: usize| self.addr.get(i).copied().unwrap_or(0) as u32;
        if erase {
            (byte(0) | (byte(1) << 8) | (byte(2) << 16), 0)
        } else {
            (
                byte(2) | (byte(3) << 8) | (byte(4) << 16),
                ((byte(0) | (byte(1) << 8)) * 2) as usize,
            )
        }
    }

    pub fn command(&mut self, v: u8) {
        match v {
            0xff => {
                self.mode = 0;
                self.addr.clear();
                self.program.clear();
                self.status = 0xc0;
                self.latched = false
            }
            0x90 => {
                self.mode = 1;
                self.addr.clear();
                self.cursor = 0
            }
            0x70 => self.mode = 2,
            0 => {
                if self.latched && self.mode == 2 {
                    self.mode = 4;
                    self.latched = false
                } else {
                    self.mode = 3;
                    self.addr.clear()
                }
            }
            0x30 => {
                let (row, col) = self.decode(false);
                self.row = row;
                self.cursor = col;
                self.mode = 4;
                self.latched = true;
                self.status = if row < PAGES && col < PAGE {
                    0xc0
                } else {
                    0xc1
                }
            }
            0x80 => {
                self.mode = 5;
                self.addr.clear();
                self.program.clear();
                self.overflow = false;
                self.latched = false
            }
            0x10 => {
                let (row, col) = self.decode(false);
                if self.overflow
                    || row < self.range.0
                    || row > self.range.1
                    || row >= PAGES
                    || col + self.program.len() > PAGE
                    || (!self.pages.contains_key(&row)
                        && self.pages.len() >= 32 * 1024 * 1024 / PAGE)
                {
                    self.status = 0xc1
                } else {
                    let page = self.pages.entry(row).or_insert_with(|| vec![255; PAGE]);
                    for (i, v) in self.program.iter().enumerate() {
                        page[col + i] &= *v
                    }
                    self.programs += 1;
                    self.status = 0xc0
                }
                self.mode = 2;
            }
            0x60 => {
                self.mode = 6;
                self.addr.clear();
                self.latched = false
            }
            0xd0 => {
                let (row, _) = self.decode(true);
                if row >= PAGES || row < self.range.0 || row > self.range.1 {
                    self.status = 0xc1
                } else {
                    let start = row / 64 * 64;
                    for r in start..start + 64 {
                        self.pages.remove(&r);
                    }
                    self.erases += 1;
                    self.status = 0xc0
                }
                self.mode = 2
            }
            0x23 => {
                self.mode = 7;
                self.addr.clear()
            }
            0x24 => {
                self.mode = 8;
                self.addr.clear()
            }
            0x2a => {
                self.range = (1, 0);
                self.mode = 2;
                self.latched = false
            }
            _ => {
                self.mode = 9;
                self.status = 0xc1
            }
        }
    }

    pub fn read(&mut self, n: usize) -> Vec<u8> {
        let mut b = vec![255; n];
        if self.mode == 2 {
            b.fill(0);
            if n > 0 {
                b[0] = self.status
            }
            return b;
        }
        if self.mode == 1 {
            b.fill(0);
            if n > 0 {
                b[0] = [0x2c, 0xba].get(self.cursor).copied().unwrap_or(255)
            }
            self.cursor += 1;
            return b;
        }
        if self.mode != 4 {
            return b;
        }
        self.read_bytes += n as u64;
        if let Some(page) = self.pages.get(&self.row) {
            for (i, v) in b.iter_mut().enumerate() {
                if let Some(a) = page.get(self.cursor + i) {
                    *v = *a
                }
            }
        }
        self.cursor = self.cursor.saturating_add(n);
        b
    }

    pub fn write(&mut self, b: &[u8]) {
        if self.mode == 5 {
            if self.program.len() + b.len() <= PAGE {
                self.program.extend(b)
            } else {
                self.overflow = true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn addr(n: &mut Nand, row: u32) {
        for b in [0, 0, row as u8, (row >> 8) as u8, (row >> 16) as u8] {
            n.address(b)
        }
    }

    #[test]
    fn flash_only_clears_bits_and_erase_restores() {
        let mut n = Nand::new();
        for value in [0x0f, 0xf0] {
            n.command(0x80);
            addr(&mut n, 70);
            n.write(&[value]);
            n.command(0x10);
            assert_eq!(n.status, 0xc0)
        }
        n.command(0);
        addr(&mut n, 70);
        n.command(0x30);
        assert_eq!(n.read(1), [0]);
        n.command(0x60);
        for b in [70, 0, 0] {
            n.address(b)
        }
        n.command(0xd0);
        n.command(0);
        addr(&mut n, 70);
        n.command(0x30);
        assert_eq!(n.read(1), [255]);
    }

    #[test]
    fn locked_program_fails() {
        let mut n = Nand::new();
        n.command(0x2a);
        n.command(0x80);
        addr(&mut n, 0);
        n.write(&[0]);
        n.command(0x10);
        assert_eq!(n.status, 0xc1);
        assert!(n.pages.is_empty());
    }
}
