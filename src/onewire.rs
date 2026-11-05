//! Read-only synthetic board EEPROM identities over the HDQ/1-Wire bus.
use crate::devices::Interrupts;
pub const PARTS: [&str; 9] = [
    "131-0087", "131-0649", "131-0526", "131-0091", "131-0085", "131-0088", "131-0328", "131-0528",
    "131-0527",
];
pub fn crc8(b: &[u8]) -> u8 {
    let mut crc = 0;
    for mut byte in b.iter().copied() {
        for _ in 0..8 {
            let mix = (crc ^ byte) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8c
            }
            byte >>= 1
        }
    }
    crc
}

pub struct OneWire {
    roms: Vec<[u8; 8]>,
    memory: Vec<[u8; 512]>,
    phase: u8,
    selected: Option<usize>,
    payload: Vec<u8>,
    candidates: Vec<usize>,
    bit: usize,
    search_phase: u8,
    address: usize,
    control: u32,
    status: u32,
    tx: u8,
    rx: u8,
    pub resets: u64,
}

impl OneWire {
    pub fn new() -> Self {
        let mut roms = Vec::new();
        let mut memory = Vec::new();
        for (i, part) in PARTS.iter().enumerate() {
            let mut rom = [0; 8];
            rom[0] = 0x23;
            rom[1] = (i + 1) as u8;
            rom[7] = crc8(&rom[..7]);
            roms.push(rom);
            let mut b = [0; 512];
            let serial = format!("VIRTUAL{:03}", i + 1);
            b[..serial.len()].copy_from_slice(serial.as_bytes());
            b[0x10..0x17].copy_from_slice(b"VIRTUAL");
            b[0x20..0x20 + part.len()].copy_from_slice(part.as_bytes());
            b[0x30..0x32].copy_from_slice(b"03");
            memory.push(b);
        }
        let mut s = Self {
            roms,
            memory,
            phase: 0,
            selected: None,
            payload: Vec::new(),
            candidates: Vec::new(),
            bit: 0,
            search_phase: 0,
            address: 0,
            control: 0,
            status: 0,
            tx: 0,
            rx: 255,
            resets: 0,
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.phase = 0;
        self.selected = None;
        self.payload.clear();
        self.candidates = (0..self.roms.len()).collect();
        self.bit = 0;
        self.search_phase = 0;
        self.address = 0;
    }

    fn tx_byte(&mut self, v: u8) {
        match self.phase {
            0 => match v {
                0xf0 => {
                    self.phase = 1;
                    self.bit = 0;
                    self.search_phase = 0
                }
                0x55 => {
                    self.phase = 2;
                    self.payload.clear()
                }
                0xcc => {
                    self.selected = if self.roms.len() == 1 { Some(0) } else { None };
                    self.phase = 3
                }
                0x33 => {
                    self.phase = 6;
                    self.address = 0
                }
                _ => {}
            },
            2 => {
                self.payload.push(v);
                if self.payload.len() == 8 {
                    self.selected = self.roms.iter().position(|r| r.as_slice() == self.payload);
                    self.phase = 3;
                    self.payload.clear()
                }
            }
            3 => {
                if v == 0xf0 {
                    self.phase = 4;
                    self.payload.clear()
                } else {
                    self.phase = 7
                }
            }
            4 => {
                self.payload.push(v);
                if self.payload.len() == 2 {
                    self.address = u16::from_le_bytes([self.payload[0], self.payload[1]]) as usize;
                    self.phase = 5
                }
            }
            _ => {}
        }
    }

    fn rx_byte(&mut self) -> u8 {
        if self.phase == 5 {
            if let Some(i) = self.selected {
                let v = self.memory[i].get(self.address).copied().unwrap_or(255);
                self.address += 1;
                return v;
            }
        }
        if self.phase == 6 {
            let v = self.roms.iter().fold(255, |v, r| v & r[self.address % 8]);
            self.address += 1;
            return v;
        }
        255
    }

    fn rx_bit(&mut self) -> u8 {
        if self.phase != 1 || self.bit >= 64 {
            return 1;
        }
        let target = if self.search_phase == 0 {
            0
        } else if self.search_phase == 1 {
            1
        } else {
            return 1;
        };
        let found = self
            .candidates
            .iter()
            .any(|i| (self.roms[*i][self.bit / 8] >> (self.bit % 8)) & 1 == target);
        self.search_phase += 1;
        u8::from(!found)
    }

    fn tx_bit(&mut self, v: u8) {
        if self.phase == 1 && self.bit < 64 && self.search_phase == 2 {
            self.candidates
                .retain(|i| (self.roms[*i][self.bit / 8] >> (self.bit % 8)) & 1 == v);
            self.bit += 1;
            self.search_phase = 0
        }
    }

    pub fn access(&mut self, write: bool, o: u32, v: u32, irq: &mut Interrupts) -> Option<u32> {
        let mut answer = None;
        if write {
            match o {
                4 => self.tx = v as u8,
                0x14 if v & 2 != 0 => {
                    self.control = 0;
                    self.status = 0;
                    self.reset()
                }
                0xc => {
                    self.control = v & !0x10;
                    if v & 0x10 != 0 {
                        if v & 4 != 0 {
                            self.reset();
                            self.resets += 1;
                            self.control =
                                (self.control & !12) | if self.roms.is_empty() { 0 } else { 8 };
                            self.status |= 1;
                        } else if v & 2 != 0 {
                            self.rx = if v & 0x80 != 0 {
                                self.rx_bit()
                            } else {
                                self.rx_byte()
                            };
                            self.status |= 2;
                        } else {
                            if v & 0x80 != 0 {
                                self.tx_bit(self.tx & 1)
                            } else {
                                self.tx_byte(self.tx)
                            }
                            self.status |= 4;
                        }
                    }
                }
                _ => {}
            }
        } else {
            answer = Some(match o {
                8 => self.rx as u32,
                0xc => self.control,
                0x10 => {
                    let v = self.status;
                    self.status = 0;
                    v
                }
                0x18 => 1,
                _ => 0,
            })
        }
        irq.level(58, self.status != 0 && self.control & 0x40 != 0);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_selects_all_virtual_roms_with_valid_crc() {
        let mut s = OneWire::new();
        for expected in s.roms.clone() {
            s.reset();
            s.tx_byte(0xf0);
            for bit in 0..64 {
                let a = s.rx_bit();
                let b = s.rx_bit();
                assert!(!(a == 1 && b == 1));
                s.tx_bit((expected[bit / 8] >> (bit % 8)) & 1);
            }
            assert_eq!(s.candidates.len(), 1);
            assert_eq!(s.roms[s.candidates[0]], expected);
            assert_eq!(crc8(&expected), 0);
        }
    }
}
