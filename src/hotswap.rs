//! LTC4222 register subset, following Analog Devices datasheet 4222fb.
//! Two independent, ideal 12 V / 0.5 A loads; no analog ramp, fault injection,
//! ALERT interrupt routing or ADC point-and-shoot conversion timing.
use serde::Serialize;

#[derive(Serialize)]
pub struct Summary {
    pub model: &'static str,
    pub enabled: [bool; 2],
    pub millivolts: [u16; 2],
    pub milliamps: [u16; 2],
    pub unsupported: u64,
}

pub struct HotSwap {
    registers: [u8; 21],
    pointer: u8,
    unsupported: u64,
}
impl HotSwap {
    pub fn new() -> Self {
        let mut registers = [0; 21];
        registers[0] = 0x1b;
        registers[4] = 0x1b;
        Self {
            registers,
            pointer: 0xd0,
            unsupported: 0,
        }
    }
    fn on(&self, channel: usize) -> bool {
        self.registers[channel * 4] & 8 != 0
    }
    pub fn select(&mut self, command: u8) {
        self.pointer = command;
    }
    pub fn summary(&self) -> Summary {
        let enabled = std::array::from_fn(|channel| self.on(channel));
        Summary {
            model: "LTC4222 register subset",
            enabled,
            millivolts: enabled.map(|on| if on { 12000 } else { 0 }),
            milliamps: enabled.map(|on| if on { 500 } else { 0 }),
            unsupported: self.unsupported,
        }
    }
    fn adc(&self, command: u8) -> u8 {
        let pair = (command - 0xd8) / 2;
        let channel = usize::from(pair & 1);
        let raw: u16 = if self.on(channel) {
            match pair {
                0 | 1 => 384,
                4 | 5 => 40,
                _ => 0,
            }
        } else {
            0
        };
        // SOURCE LSB = 31.25 mV, SENSE LSB = 62.5 uV; firmware uses 5 mOhm.
        (raw << 6).to_be_bytes()[usize::from(command & 1)]
    }
    pub fn read(&mut self) -> u8 {
        let value = match self.pointer {
            0xd2 | 0xd6 => {
                let channel = usize::from((self.pointer - 0xd2) / 4);
                let control = self.registers[channel * 4];
                let gpio_high = match control >> 6 {
                    0 => self.on(channel),
                    1 => !self.on(channel),
                    2 => control & 0x20 != 0,
                    _ => false, // No external GPIO input connected.
                };
                0x10 | if self.on(channel) { 0x80 } else { 8 } | (u8::from(gpio_high) << 6)
            }
            0xd8..=0xe3 => self.adc(self.pointer),
            0xd0..=0xd7 => self.registers[usize::from(self.pointer - 0xd0)],
            _ => {
                self.unsupported += 1;
                0xff
            }
        };
        self.pointer = self.pointer.wrapping_add(1);
        value
    }
    pub fn write(&mut self, value: u8) {
        match self.pointer {
            0xd0 | 0xd4 => self.registers[usize::from(self.pointer - 0xd0)] = value,
            0xd1 | 0xd5 => self.registers[usize::from(self.pointer - 0xd0)] = value & 0x3f,
            0xd3 | 0xd7 => self.registers[usize::from(self.pointer - 0xd0)] &= value & 0x3f,
            _ => self.unsupported += 1,
        }
        self.pointer = self.pointer.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_switch_changes_status_and_adc_without_affecting_other_channel() {
        let mut device = HotSwap::new();
        device.select(0xd8);
        assert_eq!([device.read(), device.read()], (384u16 << 6).to_be_bytes());
        device.select(0xe0);
        assert_eq!([device.read(), device.read()], (40u16 << 6).to_be_bytes());
        device.select(0xd0);
        device.write(0);
        device.select(0xd2);
        assert_eq!(device.read() & 0x88, 8);
        device.select(0xd8);
        assert_eq!([device.read(), device.read()], [0, 0]);
        assert_eq!(device.summary().millivolts, [0, 12000]);
        device.select(0xd6);
        device.write(0); // Status is read-only.
        device.select(0xd6);
        assert_eq!(device.read() & 0x80, 0x80);
    }
}
