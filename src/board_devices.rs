//! Synthetic board peripherals at addresses taken from the firmware's I2C table.
//! Temperature registers are big-endian; ADC configuration uses pages 0 and 1.
//! These models do not supply calibration data or an audio sample stream.

pub enum Peripheral {
    HotSwap(crate::hotswap::HotSwap),
    Power(crate::power::Sequencer),
    Indicators(crate::indicators::Indicators),
    Mux(u8),
    Temperature {
        registers: [u16; 4],
        pointer: u8,
        byte: usize,
    },
    Fan {
        registers: Box<[u8; 256]>,
        pointer: u8,
    },
    AudioAdc {
        registers: Box<[[u8; 128]; 2]>,
        page: u8,
        pointer: u8,
    },
}

impl Peripheral {
    pub fn diagnostic(&self) -> Option<serde_json::Value> {
        match self {
            Self::HotSwap(device) => serde_json::to_value(device.summary()).ok(),
            Self::Power(device) => serde_json::to_value(device.summary()).ok(),
            Self::Indicators(device) => serde_json::to_value(device.summary()).ok(),
            Self::Mux(control) => {
                Some(serde_json::json!({"model": "I2C mux control only", "control": control}))
            }
            _ => None,
        }
    }

    pub fn temperature(degrees: i16) -> Self {
        Self::Temperature {
            registers: [(degrees * 256) as u16, 0x0080, 0x4b00, 0x5000],
            pointer: 0,
            byte: 0,
        }
    }

    /// Virtual dual fan, with tach counters consumed as 480000 / count RPM.
    /// Register layout comes from the 6.3.106 driver tables at 0x810b1178.
    pub fn fan() -> Self {
        Self::Fan {
            registers: Box::new([0; 256]),
            pointer: 0,
        }
    }

    pub fn audio_adc() -> Self {
        Self::AudioAdc {
            registers: Box::new([[0; 128]; 2]),
            page: 0,
            pointer: 0,
        }
    }

    pub fn select(&mut self, address: u8) {
        match self {
            Self::HotSwap(device) => device.select(address),
            Self::Power(device) => device.select(address),
            Self::Indicators(device) => device.select(address),
            Self::Mux(control) => *control = address,
            Self::Temperature { pointer, byte, .. } => {
                *pointer = address;
                *byte = 0;
            }
            Self::AudioAdc { pointer, .. } | Self::Fan { pointer, .. } => *pointer = address,
        }
    }

    pub fn write(&mut self, value: u8) {
        match self {
            Self::HotSwap(device) => device.write(value),
            Self::Power(device) => device.write(value),
            Self::Indicators(device) => device.write(value),
            Self::Mux(_) => {}
            Self::Fan { registers, pointer } => {
                // Tach registers are read-only. PWM 0..120 corresponds to 0..100%.
                if ![0x20, 0x21].contains(pointer) {
                    registers[*pointer as usize] = value;
                }
                *pointer = pointer.wrapping_add(1);
            }
            Self::Temperature {
                registers,
                pointer,
                byte,
            } => {
                if *pointer > 0 && *pointer < 4 {
                    let register = &mut registers[*pointer as usize];
                    if *byte % 2 == 0 {
                        *register = (*register & 0xff) | (u16::from(value) << 8);
                    } else {
                        *register = (*register & 0xff00) | u16::from(value);
                    }
                }
                *byte += 1;
            }
            Self::AudioAdc {
                registers,
                page,
                pointer,
            } => {
                if *pointer == 0 {
                    *page = value;
                } else if *page < 2 && *pointer < 128 {
                    registers[*page as usize][*pointer as usize] = value;
                }
                *pointer = pointer.wrapping_add(1);
            }
        }
    }

    pub fn read(&mut self) -> u8 {
        match self {
            Self::HotSwap(device) => device.read(),
            Self::Power(device) => device.read(),
            Self::Indicators(device) => device.read(),
            Self::Mux(control) => *control,
            Self::Fan { registers, pointer } => {
                let value = match *pointer {
                    0x20 | 0x21 => {
                        let fan = usize::from(*pointer - 0x20);
                        let pwm = u32::from(registers[0x26 + fan]).min(120);
                        let rpm = [11000, 16000][fan] * pwm / 120;
                        480000u32
                            .checked_div(rpm)
                            .map_or(255, |counter| counter.clamp(1, 254) as u8)
                    }
                    _ => registers[*pointer as usize],
                };
                *pointer = pointer.wrapping_add(1);
                value
            }

            Self::Temperature {
                registers,
                pointer,
                byte,
            } => {
                let value = registers
                    .get(*pointer as usize)
                    .copied()
                    .unwrap_or(u16::MAX);
                let result = value.to_be_bytes()[*byte % 2];
                *byte += 1;
                result
            }
            Self::AudioAdc {
                registers,
                page,
                pointer,
            } => {
                let result = if *pointer == 0 {
                    *page
                } else {
                    registers
                        .get(*page as usize)
                        .and_then(|bank| bank.get(*pointer as usize))
                        .copied()
                        .unwrap_or(0xff)
                };
                *pointer = pointer.wrapping_add(1);
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_tach_tracks_independent_pwm_channels_and_stopped_rotors() {
        let mut fan = Peripheral::fan();
        fan.select(0x20);
        assert_eq!([fan.read(), fan.read()], [255, 255]);
        fan.select(0x26);
        fan.write(120);
        fan.write(60);
        fan.select(0x20);
        assert_eq!([fan.read(), fan.read()], [43, 60]);
        fan.select(0x20);
        fan.write(0);
        fan.select(0x20);
        assert_eq!(fan.read(), 43);
        fan.select(0x26);
        fan.write(0);
        fan.select(0x20);
        assert_eq!([fan.read(), fan.read()], [255, 60]);
    }

    #[test]
    fn temperature_is_signed_big_endian_and_cannot_be_overwritten() {
        let mut sensor = Peripheral::temperature(-5);
        sensor.select(0);
        sensor.write(0);
        sensor.write(0);
        sensor.select(0);
        assert_eq!([sensor.read(), sensor.read()], (-1280i16).to_be_bytes());
        sensor.select(1);
        sensor.write(0x12);
        sensor.write(0x34);
        sensor.select(1);
        assert_eq!([sensor.read(), sensor.read()], [0x12, 0x34]);
    }

    #[test]
    fn adc_pages_keep_coefficients_separate_from_control_registers() {
        let mut adc = Peripheral::audio_adc();
        adc.select(0);
        adc.write(1);
        adc.select(0x33);
        adc.write(0x78);
        adc.write(0x3f);
        adc.select(0);
        adc.write(0);
        adc.select(0x33);
        assert_eq!(adc.read(), 0);
        adc.select(0);
        adc.write(1);
        adc.select(0x33);
        assert_eq!([adc.read(), adc.read()], [0x78, 0x3f]);
    }
}
