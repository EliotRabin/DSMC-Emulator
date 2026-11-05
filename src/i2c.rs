//! OMAP I2C FIFO transactions with virtual TPS65950 and brain GPIO expanders.
use crate::devices::Interrupts;
use std::collections::BTreeMap;
pub const BASES: [u32; 3] = [0x48070000, 0x48072000, 0x48060000];
struct Slave {
    regs: [u8; 256],
    peripheral: Option<crate::board_devices::Peripheral>,
    pointer: u8,
    pending: bool,
    ports: usize,
    bank: u8,
    unlock: u8,
    epoch: u64,
    anchor: u64,
}

impl Slave {
    fn new(bank: u8, ports: usize) -> Self {
        let mut s = Self {
            regs: [0; 256],
            peripheral: None,
            pointer: 0,
            pending: true,
            ports,
            bank,
            unlock: 0,
            epoch: 0,
            anchor: 0,
        };
        if bank == 0x4b {
            s.regs[0x2a] = 0x80
        }
        for i in 0..ports {
            s.regs[ports + i] = 255;
            s.regs[3 * ports + i] = 255
        }
        s
    }

    fn write(&mut self, v: u8, n: u64) {
        if self.pending {
            self.pointer = v;
            if let Some(peripheral) = &mut self.peripheral {
                peripheral.select(v);
            }
            self.pending = false;
            return;
        }
        if let Some(peripheral) = &mut self.peripheral {
            peripheral.write(v);
            return;
        }
        let r = self.pointer as usize;
        self.pointer = self.pointer.wrapping_add(1);
        if self.ports > 0 {
            if r >= self.ports && r < 4 * self.ports {
                self.regs[r] = v
            }
            return;
        }
        if self.bank == 0x4b {
            if r == 0x44 {
                self.unlock = if v == 0xce {
                    1
                } else if v == 0xec && self.unlock == 1 {
                    2
                } else {
                    0
                };
                self.regs[r] = if self.unlock == 2 { 3 } else { 0 };
                return;
            }
            if r == 0x2a {
                self.regs[r] &= !v;
                return;
            }
            if (0x1c..=0x22).contains(&r) {
                self.regs[r] = v;
                let b: Vec<_> = self.regs[0x1c..0x22]
                    .iter()
                    .map(|x| (x >> 4) as u32 * 10 + (x & 15) as u32)
                    .collect();
                if let Some(epoch) = epoch(b[5] + 2000, b[4], b[3], b[2], b[1], b[0]) {
                    self.epoch = epoch;
                    self.anchor = n
                }
                return;
            }
        }
        self.regs[r] = v;
    }

    fn read(&mut self, n: u64) -> u8 {
        if let Some(peripheral) = &mut self.peripheral {
            return peripheral.read();
        }
        let r = self.pointer as usize;
        self.pointer = self.pointer.wrapping_add(1);
        if r < self.ports {
            let mask = self.regs[3 * self.ports + r];
            return (self.regs[self.ports + r] & !mask) ^ self.regs[2 * self.ports + r];
        }
        if self.bank == 0x4b && (0x1c..=0x22).contains(&r) {
            let b = date(self.epoch + n.saturating_sub(self.anchor) / 1_000_000);
            let v = b[r - 0x1c];
            return (((v / 10) << 4) | (v % 10)) as u8;
        }
        self.regs[r]
    }
}

fn leap(y: u32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn month_days(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn epoch(y: u32, m: u32, d: u32, h: u32, min: u32, sec: u32) -> Option<u64> {
    if !(2000..=2099).contains(&y)
        || !(1..=12).contains(&m)
        || d == 0
        || d > month_days(y, m)
        || h > 23
        || min > 59
        || sec > 59
    {
        return None;
    }
    let mut days = 0u64;
    for y in 2000..y {
        days += if leap(y) { 366 } else { 365 }
    }
    for month in 1..m {
        days += month_days(y, month) as u64
    }
    Some((days + d as u64 - 1) * 86400 + h as u64 * 3600 + min as u64 * 60 + sec as u64)
}

fn date(n: u64) -> [u32; 7] {
    let mut days = n / 86400;
    let weekday = ((days + 6) % 7) as u32;
    let mut y = 2000;
    while days >= if leap(y) { 366 } else { 365 } {
        days -= if leap(y) { 366 } else { 365 };
        y += 1
    }
    let mut m = 1;
    while days >= month_days(y, m) as u64 {
        days -= month_days(y, m) as u64;
        m += 1
    }
    [
        (n % 60) as u32,
        ((n / 60) % 60) as u32,
        ((n / 3600) % 24) as u32,
        days as u32 + 1,
        m,
        y % 100,
        weekday,
    ]
}

#[derive(Default)]
struct Controller {
    status: u32,
    enabled: u32,
    regs: BTreeMap<u32, u32>,
    remaining: u32,
    active: bool,
    reading: bool,
    slave: u8,
}

impl Controller {
    fn reg(&self, r: u32) -> u32 {
        *self.regs.get(&r).unwrap_or(&0)
    }

    fn complete(&mut self) {
        self.active = false;
        self.status |= 4;
        if self.reg(0x24) & 2 != 0 {
            self.status &= !0x1000;
            self.regs.entry(0x24).and_modify(|v| *v &= !2);
        }
    }

    fn refresh(&mut self) {
        self.status &= !0x6018;
        if self.active && self.remaining > 0 {
            let threshold = if self.reading {
                (self.reg(0x14) >> 8) & 63
            } else {
                self.reg(0x14) & 63
            } + 1;
            self.status |= if self.remaining >= threshold {
                if self.reading { 8 } else { 0x10 }
            } else if self.reading {
                0x2000
            } else {
                0x4000
            };
        }
    }
}

pub struct I2c {
    controllers: [Controller; 3],
    devices: BTreeMap<(usize, u8), Slave>,
    pub transactions: u64,
    pub nacks: u64,
    missing: BTreeMap<(usize, u8), u64>,
}

impl I2c {
    pub fn new() -> Self {
        let mut devices = BTreeMap::new();
        for bank in 0x48..=0x4b {
            devices.insert((0, bank), Slave::new(bank, 0));
        }
        devices.insert((0, 0x20), Slave::new(0, 2));
        devices.insert((1, 0x38), Slave::new(0, 1));
        for (bus, address, degrees) in [
            (2, 0x4a, 35),
            (1, 0x4b, 38),
            (1, 0x4a, 32),
            (1, 0x48, 36),
            (1, 0x49, 34),
        ] {
            let mut slave = Slave::new(0, 0);
            slave.peripheral = Some(crate::board_devices::Peripheral::temperature(degrees));
            devices.insert((bus, address), slave);
        }
        let mut fan = Slave::new(0, 0);
        fan.peripheral = Some(crate::board_devices::Peripheral::fan());
        devices.insert((1, 0x2f), fan);
        let mut adc = Slave::new(0, 0);
        adc.peripheral = Some(crate::board_devices::Peripheral::audio_adc());
        devices.insert((0, 0x19), adc);
        Self {
            controllers: std::array::from_fn(|_| Controller::default()),
            devices,
            transactions: 0,
            nacks: 0,
            missing: BTreeMap::new(),
        }
    }

    pub fn missing_devices(&self) -> Vec<(usize, u8, u64)> {
        self.missing
            .iter()
            .map(|((bus, address), count)| (*bus, *address, *count))
            .collect()
    }

    pub fn access(
        &mut self,
        bus: usize,
        write: bool,
        o: u32,
        size: usize,
        v: u32,
        n: u64,
        irq: &mut Interrupts,
    ) -> Option<u32> {
        let s = &mut self.controllers[bus];
        let mut answer = None;
        if write {
            match o {
                4 => s.enabled = v & 65535,
                8 => s.status &= !(v & !0x1000),
                0x20 if v & 2 != 0 => *s = Controller::default(),
                0x1c if s.active && !s.reading => {
                    if let Some(d) = self.devices.get_mut(&(bus, s.slave)) {
                        for i in 0..size.min(4) {
                            if s.remaining == 0 {
                                break;
                            }
                            d.write((v >> (8 * i)) as u8, n);
                            s.remaining -= 1
                        }
                        if s.remaining == 0 {
                            s.complete()
                        }
                    }
                }
                _ => {
                    s.regs.insert(o, v);
                    if o == 0x24 && v & 1 != 0 {
                        s.slave = (s.reg(0x2c) & 127) as u8;
                        s.reading = v & 0x200 == 0;
                        s.remaining = s.reg(0x18) & 65535;
                        s.active = false;
                        s.regs.insert(o, v & !1);
                        self.transactions += 1;
                        if let Some(d) = self.devices.get_mut(&(bus, s.slave)) {
                            if v & 0x8400 == 0x8400 {
                                s.status = 0x1000;
                                s.active = true;
                                if !s.reading {
                                    d.pending = true
                                }
                                if s.remaining == 0 {
                                    s.complete()
                                }
                            }
                        } else {
                            s.status = 2;
                            s.regs.entry(o).and_modify(|v| *v &= !2);
                            self.nacks += 1;
                            *self.missing.entry((bus, s.slave)).or_default() += 1
                        }
                    }
                }
            }
        } else {
            answer = Some(match o {
                4 => s.enabled,
                8 => s.status,
                0x10 => 1,
                0x18 => s.remaining,
                0x40 => s.remaining.min(63) << if s.reading { 8 } else { 0 },
                0x1c => {
                    let mut v = 0;
                    if s.active && s.reading {
                        if let Some(d) = self.devices.get_mut(&(bus, s.slave)) {
                            for i in 0..size.min(4) {
                                if s.remaining == 0 {
                                    break;
                                }
                                v |= (d.read(n) as u32) << (8 * i);
                                s.remaining -= 1
                            }
                            if s.remaining == 0 {
                                s.complete()
                            }
                        }
                    }
                    v
                }
                _ => s.reg(o),
            })
        }
        s.refresh();
        if !write && o == 8 {
            answer = Some(s.status)
        }
        irq.level([56, 57, 61][bus], s.status & s.enabled != 0);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rtc_leap_day_round_trip() {
        let t = epoch(2024, 2, 29, 23, 59, 59).unwrap();
        assert_eq!(&date(t)[..6], &[59, 59, 23, 29, 2, 24]);
        assert_eq!(&date(t + 1)[..6], &[0, 0, 0, 1, 3, 24]);
        assert!(epoch(2023, 2, 29, 0, 0, 0).is_none());
    }

    #[test]
    fn controller_reads_virtual_temperature_and_reports_unconnected_slaves() {
        let mut controller = I2c::new();
        let mut irq = Interrupts::new(1000);
        for (register, value) in [(0x2c, 0x4a), (0x18, 1), (0x24, 0x8603), (0x1c, 0)] {
            controller.access(1, true, register, 1, value, 0, &mut irq);
        }
        controller.access(1, true, 0x18, 4, 2, 0, &mut irq);
        controller.access(1, true, 0x24, 4, 0x8403, 0, &mut irq);
        assert_eq!(
            controller.access(1, false, 0x1c, 2, 0, 0, &mut irq),
            Some(32)
        );
        controller.access(1, true, 0x2c, 4, 0x7f, 0, &mut irq);
        controller.access(1, true, 0x24, 4, 0x8403, 0, &mut irq);
        assert_eq!(controller.missing_devices(), [(1, 0x7f, 1)]);
    }

    #[test]
    fn pmic_protection_keys() {
        let mut d = Slave::new(0x4b, 0);
        d.write(0x44, 0);
        d.write(0xce, 0);
        d.pending = true;
        d.write(0x44, 0);
        d.write(0xec, 0);
        assert_eq!(d.regs[0x44], 3);
        d.pending = true;
        d.write(0x44, 0);
        d.write(0, 0);
        assert_eq!(d.regs[0x44], 0);
    }
}
