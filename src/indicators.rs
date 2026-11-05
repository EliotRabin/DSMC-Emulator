//! I2C LED control register subsets. No optical waveform is generated.
use serde::Serialize;

#[derive(Serialize)]
pub struct Summary {
    pub model: &'static str,
    pub registers: Vec<u8>,
    pub modes: Vec<u8>,
    pub unsupported: u64,
}

pub struct Indicators {
    seven: bool,
    registers: [u8; 11],
    pointer: u8,
    increment: bool,
    unsupported: u64,
}

impl Indicators {
    pub fn new(seven: bool) -> Self {
        Self {
            seven,
            registers: [0; 11],
            pointer: 0,
            increment: false,
            unsupported: 0,
        }
    }
    pub fn select(&mut self, command: u8) {
        self.pointer = command & 0xf;
        self.increment = command & 0x10 != 0;
        if command & 0xe0 != 0 {
            self.unsupported += 1;
        }
    }
    fn limit(&self) -> u8 {
        if self.seven { 10 } else { 5 }
    }
    fn advance(&mut self) {
        if self.increment {
            self.pointer = if self.pointer >= self.limit() {
                0
            } else {
                self.pointer + 1
            };
        }
    }
    pub fn write(&mut self, value: u8) {
        if self.pointer <= self.limit() && (self.seven || self.pointer != 0) {
            self.registers[usize::from(self.pointer)] = value;
        } else {
            self.unsupported += 1;
        }
        self.advance();
    }
    pub fn read(&mut self) -> u8 {
        let value = if self.pointer <= self.limit() {
            self.registers[usize::from(self.pointer)]
        } else {
            self.unsupported += 1;
            0xff
        };
        self.advance();
        value
    }
    pub fn summary(&self) -> Summary {
        let modes = if self.seven {
            (0..7)
                .map(|i| {
                    (0..3).fold(0, |v, plane| {
                        v | (((self.registers[plane] >> i) & 1) << plane)
                    })
                })
                .collect()
        } else {
            (0..4).map(|i| (self.registers[5] >> (2 * i)) & 3).collect()
        };
        Summary {
            model: if self.seven {
                "TCA6507 register subset"
            } else {
                "PCA9533 register subset"
            },
            registers: self.registers[..=usize::from(self.limit())].to_vec(),
            modes,
            unsupported: self.unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn main_led_bitplanes_and_increment_command() {
        let mut led = Indicators::new(true);
        led.select(0x10);
        led.write(1);
        led.write(2);
        led.write(4);
        assert_eq!(led.summary().modes, [1, 2, 4, 0, 0, 0, 0]);
        led.select(1);
        led.write(8);
        led.write(16);
        assert_eq!(led.summary().registers[1], 16);
    }
    #[test]
    fn side_led_prescalers_pwm_and_channel_modes_are_separate() {
        let mut led = Indicators::new(false);
        led.select(0x11);
        for value in [20, 30, 40, 50] {
            led.write(value);
        }
        led.select(5);
        led.write(0xe4);
        assert_eq!(led.summary().modes, [0, 1, 2, 3]);
        assert_eq!(&led.summary().registers[1..5], &[20, 30, 40, 50]);
        led.select(0);
        led.write(0xff);
        led.select(0);
        assert_eq!(led.read(), 0);
    }
}
