//! Register-level power sequencing used by the 6.3.106 drivers.
//!
//! Rail voltages are synthetic nominal values, not factory calibration. PMBus
//! transfers keep a command and a byte index; they are not byte register bursts.
//! Analog ramps, faults, nonvolatile programming and PMBus PEC are not modeled.
use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize)]
pub struct Summary {
    pub model: &'static str,
    pub millivolts: Vec<u16>,
    pub enabled: Vec<bool>,
    pub gpio: Vec<u8>,
    pub unsupported: BTreeMap<String, u64>,
}

pub struct Sequencer {
    legacy: bool,
    nominal: Vec<u16>,
    enabled: Vec<bool>,
    gpio: [u8; 26],
    pin: u8,
    page: u8,
    command: u8,
    byte: usize,
    unsupported: BTreeMap<String, u64>,
    config: Box<[u8; 512]>,
    window: u16,
    unlocked: bool,
}

impl Sequencer {
    pub fn new(legacy: bool, nominal: &[u16]) -> Self {
        Self {
            legacy,
            nominal: nominal.to_vec(),
            enabled: vec![true; nominal.len()],
            gpio: [0; 26],
            pin: 0,
            page: 0,
            command: 0,
            byte: 0,
            unsupported: BTreeMap::new(),
            config: Box::new([0; 512]),
            window: 0,
            unlocked: false,
        }
    }

    pub fn summary(&self) -> Summary {
        Summary {
            model: if self.legacy {
                "UCD9081"
            } else {
                "UCD9090 PMBus subset"
            },
            millivolts: (0..self.nominal.len()).map(|i| self.voltage(i)).collect(),
            enabled: self.enabled.clone(),
            gpio: self.gpio.to_vec(),
            unsupported: self.unsupported.clone(),
        }
    }

    fn voltage(&self, rail: usize) -> u16 {
        if self.enabled.get(rail) == Some(&true) {
            self.nominal[rail]
        } else {
            0
        }
    }

    fn unsupported(&mut self, write: bool) {
        let key = format!(
            "{}:0x{:02x}",
            if write { "write" } else { "read" },
            self.command
        );
        *self.unsupported.entry(key).or_default() += 1;
    }

    pub fn select(&mut self, command: u8) {
        self.command = command;
        self.byte = 0;
    }

    pub fn write(&mut self, value: u8) {
        if self.legacy {
            self.legacy_write(value);
            return;
        }
        if self.byte != 0 {
            self.unsupported(true);
        } else {
            match self.command {
                0x00 => self.page = value,
                0x01 if matches!(value, 0 | 0x40 | 0x80) => {
                    if self.page == 0xff {
                        self.enabled.fill(value == 0x80);
                    } else if let Some(enabled) = self.enabled.get_mut(usize::from(self.page)) {
                        *enabled = value == 0x80;
                    } else {
                        self.unsupported(true);
                    }
                }
                0xfa => self.pin = value,
                0xfb if usize::from(self.pin) < self.gpio.len() => {
                    // Enable is an apply strobe. Status is read-only, bit 3.
                    if value & 1 != 0 {
                        self.gpio[usize::from(self.pin)] = value & 7;
                    }
                }
                _ => self.unsupported(true),
            }
        }
        self.byte += 1;
    }

    pub fn read(&mut self) -> u8 {
        if self.legacy {
            return self.legacy_read();
        }
        let value = match (self.command, self.byte) {
            (0x00, 0) => self.page,
            (0x01, 0) if usize::from(self.page) < self.enabled.len() => {
                if self.enabled[usize::from(self.page)] {
                    0x80
                } else {
                    0
                }
            }
            (0x20, 0) => 0x17, // Linear16 exponent -9.
            (0x8b, 0..=1) if usize::from(self.page) < self.nominal.len() => {
                let raw =
                    ((u32::from(self.voltage(usize::from(self.page))) * 512 + 500) / 1000) as u16;
                raw.to_le_bytes()[self.byte]
            }
            (0xfa, 0) => self.pin,
            (0xfb, 0) if usize::from(self.pin) < self.gpio.len() => {
                let config = self.gpio[usize::from(self.pin)];
                config | if config & 6 == 6 { 8 } else { 0 }
            }
            _ => {
                self.unsupported(false);
                0xff
            }
        };
        self.byte += 1;
        value
    }

    fn advance_legacy(&mut self) {
        self.command = match self.command {
            0x0f => 0,
            0x31 => 0x30,
            0x33 => 0x32,
            command => command.wrapping_add(1),
        };
    }

    fn legacy_write(&mut self, value: u8) {
        match self.command {
            // These two control bytes are used by the firmware's UCD9081 HAL.
            0x1a => self.gpio[0] = value,
            0x1b => {
                for (rail, enabled) in self.enabled.iter_mut().enumerate() {
                    *enabled = value & (1 << rail) != 0;
                }
            }
            0x2e => self.unlocked = value == 2,
            0x2f if value == 0xc0 => {
                self.enabled.fill(false);
                self.gpio.fill(0);
            }
            0x2f if value == 0 => self.enabled.fill(true),
            0x30 => self.window = (self.window & 0xff00) | u16::from(value),
            0x31 => self.window = (self.window & 0xff) | (u16::from(value) << 8),
            0x32 | 0x33 => {
                if self.unlocked {
                    if let Some(index) = self.window.checked_sub(0xe000).filter(|i| *i < 512) {
                        self.config[usize::from(index)] = value;
                    } else {
                        self.unsupported(true);
                    }
                } else {
                    self.unsupported(true);
                }
                self.window = self.window.wrapping_add(1);
            }
            _ => self.unsupported(true),
        }
        self.advance_legacy();
    }

    fn legacy_read(&mut self) -> u8 {
        let value = match self.command {
            0..=0x0f => {
                let raw = ((u32::from(self.voltage(usize::from(self.command / 2))) * 1024 + 1650)
                    / 3300)
                    .min(1023) as u16;
                raw.to_be_bytes()[usize::from(self.command & 1)]
            }
            0x1a => self.gpio[0],
            0x1b => self
                .enabled
                .iter()
                .enumerate()
                .fold(0, |v, (i, on)| v | (u8::from(*on) << i)),
            0x20..=0x26 => 0, // No synthetic fault is injected.
            0x2e => {
                if self.unlocked {
                    2
                } else {
                    0
                }
            }
            0x30 => self.window as u8,
            0x31 => (self.window >> 8) as u8,
            0x32 | 0x33 => {
                let value = self
                    .window
                    .checked_sub(0xe000)
                    .filter(|i| *i < 512)
                    .map_or(0xff, |index| self.config[usize::from(index)]);
                self.window = self.window.wrapping_add(1);
                value
            }
            _ => {
                self.unsupported(false);
                0xff
            }
        };
        self.advance_legacy();
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write(s: &mut Sequencer, command: u8, value: u8) {
        s.select(command);
        s.write(value);
    }

    #[test]
    fn pmbus_word_endian_page_selection_and_broadcast_shutdown() {
        let mut s = Sequencer::new(false, &[1000, 1800]);
        write(&mut s, 0, 1);
        s.select(0x8b);
        assert_eq!(u16::from_le_bytes([s.read(), s.read()]), 922);
        write(&mut s, 0, 0xff);
        write(&mut s, 1, 0x40);
        assert_eq!(s.summary().millivolts, [0, 0]);
        write(&mut s, 0, 0);
        write(&mut s, 1, 0x80);
        assert_eq!(s.summary().millivolts, [1000, 0]);
        write(&mut s, 0, 100);
        s.select(0x8b);
        assert_eq!(s.read(), 0xff);
        assert_eq!(s.summary().unsupported["read:0x8b"], 1);
    }

    #[test]
    fn gpio_apply_strobe_and_status_do_not_change_other_pins() {
        let mut s = Sequencer::new(false, &[1000]);
        write(&mut s, 0xfa, 3);
        write(&mut s, 0xfb, 7);
        s.select(0xfb);
        assert_eq!(s.read(), 15);
        write(&mut s, 0xfb, 0);
        s.select(0xfb);
        assert_eq!(s.read(), 15);
        write(&mut s, 0xfa, 4);
        s.select(0xfb);
        assert_eq!(s.read(), 0);
    }

    #[test]
    fn legacy_adc_is_big_endian_and_tracks_enable_mask() {
        let mut s = Sequencer::new(true, &[1650, 2500]);
        s.select(0);
        assert_eq!([s.read(), s.read()], [2, 0]);
        write(&mut s, 0x1b, 2);
        s.select(0);
        assert_eq!([s.read(), s.read()], [0, 0]);
        assert!(u16::from_be_bytes([s.read(), s.read()]) > 700);
    }

    #[test]
    fn legacy_configuration_window_requires_unlock_and_preserves_address_bytes() {
        let mut s = Sequencer::new(true, &[1000]);
        s.select(0x30);
        s.write(0);
        s.write(0xe0);
        write(&mut s, 0x32, 0x77);
        write(&mut s, 0x2e, 2);
        s.select(0x30);
        s.write(0);
        s.write(0xe0);
        s.select(0x32);
        s.write(0xab);
        s.write(0xcd);
        s.select(0x30);
        s.write(0);
        s.write(0xe0);
        s.select(0x32);
        assert_eq!([s.read(), s.read()], [0xab, 0xcd]);
        assert_eq!(s.summary().unsupported["write:0x32"], 1);
    }
}
