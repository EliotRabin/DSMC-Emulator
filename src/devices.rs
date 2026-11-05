//! Instruction-clocked OMAP boot devices. Unsupported devices never claim success.
use crate::unicorn::{CPSR, Cpu, LR, PC, SPSR};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Serialize)]
pub struct Interrupts {
    pub masks: [u32; 3],
    software: [u32; 3],
    #[serde(skip)]
    priorities: [u8; 96],
    #[serde(skip)]
    fiq: BTreeSet<usize>,
    pub levels: BTreeSet<usize>,
    pub active: Option<usize>,
    pub deliveries: BTreeMap<usize, u64>,
    pub running: bool,
    enabled: bool,
    pending: bool,
    pub next: u64,
    pub period: u64,
    pub external_i2c: bool,
    uart: [u32; 3],
    i2c: [(u32, u32); 3],
}

impl Interrupts {
    pub fn new(period: u64) -> Self {
        Self {
            masks: [u32::MAX; 3],
            software: [0; 3],
            priorities: [0; 96],
            fiq: BTreeSet::new(),
            levels: BTreeSet::new(),
            active: None,
            deliveries: BTreeMap::new(),
            running: false,
            enabled: false,
            pending: false,
            next: 0,
            period,
            external_i2c: false,
            uart: [0; 3],
            i2c: [(0, 0); 3],
        }
    }

    pub fn level(&mut self, n: usize, on: bool) {
        if on {
            self.levels.insert(n);
        } else {
            self.levels.remove(&n);
        }
    }

    pub fn update(&mut self, n: u64) {
        if self.running && n >= self.next {
            self.pending = true;
            self.next = n + self.period;
        }
        self.level(38, self.pending && self.enabled);
        for i in 0..3 {
            if !self.external_i2c {
                self.level([56, 57, 61][i], self.i2c[i].0 & self.i2c[i].1 != 0);
            }
            self.level([72, 73, 74][i], self.uart[i] & 2 != 0);
        }
    }

    pub fn next_irq(&self) -> Option<usize> {
        if self.active.is_some() {
            return None;
        }
        (0..96)
            .filter(|i| {
                (self.levels.contains(i) || self.software[i / 32] & (1 << (i % 32)) != 0)
                    && self.masks[i / 32] & (1 << (i % 32)) == 0
                    && !self.fiq.contains(i)
            })
            .min_by_key(|i| (self.priorities[*i], *i))
    }

    pub fn deliver(&mut self, c: Cpu, pc: u32, n: u64) {
        self.update(n);
        let Some(irq) = self.next_irq() else { return };
        let saved = c.reg(CPSR);
        if saved & 0x80 != 0 {
            return;
        }
        let control = c.cp(1, 0, 0, 0);
        if control & (1 << 30) != 0 {
            return;
        }
        let v = if control & (1 << 13) != 0 {
            0xffff0000
        } else {
            c.cp(12, 0, 0, 0)
        };
        c.set(
            CPSR,
            (saved & !((0x3f << 10) | (3 << 25) | (1 << 24) | 0x3f)) | 0x92,
        );
        c.set(SPSR, saved);
        c.set(LR, pc.wrapping_add(4));
        c.set(PC, v + 0x18);
        self.active = Some(irq);
        *self.deliveries.entry(irq).or_default() += 1;
    }

    pub fn access(&mut self, c: Cpu, write: bool, a: u32, v: u32, n: u64) -> Option<u32> {
        let p = a & !0xfff;
        let o = a & 0xfff;
        let mut answer = None;
        if p == 0x48200000 {
            if (0x100..0x280).contains(&o) && o % 4 == 0 && write {
                let i = ((o - 0x100) / 4) as usize;
                self.priorities[i] = ((v >> 2) & 63) as u8;
                if v & 1 != 0 {
                    self.fiq.insert(i);
                } else {
                    self.fiq.remove(&i);
                }
            }
            if (0x80..0xe0).contains(&o) {
                let b = ((o - 0x80) / 0x20) as usize;
                let r = (o - 0x80) % 0x20;
                if write {
                    match r {
                        4 => self.masks[b] = v,
                        8 => self.masks[b] &= !v,
                        12 => self.masks[b] |= v,
                        16 => self.software[b] |= v,
                        20 => self.software[b] &= !v,
                        _ => {}
                    }
                } else if r == 4 {
                    answer = Some(self.masks[b])
                } else if r == 0 || r == 0x18 {
                    let mut v = self.software[b];
                    for i in &self.levels {
                        if i / 32 == b {
                            v |= 1 << (i % 32)
                        }
                    }
                    answer = Some(if r == 0x18 { v & !self.masks[b] } else { v });
                }
            } else if o == 0x48 && write && v & 1 != 0 {
                self.active = None
            } else if o == 0x40 && !write {
                answer = Some(self.active.map(|v| v as u32).unwrap_or(0xffffff80))
            } else if o == 0x14 && !write {
                answer = Some(1)
            }
        } else if p == 0x49032000 {
            if write {
                match o {
                    0x24 => {
                        self.running = v & 1 != 0;
                        self.next = n + self.period
                    }
                    0x1c => self.enabled = v & 2 != 0,
                    0x18 if v & 2 != 0 => self.pending = false,
                    _ => {}
                }
            } else {
                answer = match o {
                    0x14 => Some(1),
                    0x18 => Some(if self.pending { 2 } else { 0 }),
                    _ => None,
                };
            }
        } else if let Some(i) = [0x48070000, 0x48072000, 0x48060000]
            .iter()
            .position(|b| *b == p)
        {
            let d = &mut self.i2c[i];
            if write {
                match o {
                    4 => d.1 = v & 0xffff,
                    8 => d.0 &= !v,
                    0x24 if v & 1 != 0 => d.0 |= 2,
                    _ => {}
                }
            } else {
                answer = match o {
                    8 => Some(d.0),
                    4 => Some(d.1),
                    _ => None,
                };
            }
        } else if let Some(i) = [0x4806a000, 0x4806c000, 0x49020000]
            .iter()
            .position(|b| *b == p)
        {
            if write && o == 4 && c.word(p + 0xc) & 0x80 == 0 {
                self.uart[i] = v
            } else if !write && o == 8 {
                answer = Some(if self.uart[i] & 2 != 0 { 2 } else { 1 })
            }
        }
        answer
    }
}

#[derive(Default)]
pub struct Boot {
    ticks: u32,
    anchor: u64,
    running: bool,
    pub overrides: BTreeMap<u32, u32>,
}

impl Boot {
    fn time(&self, n: u64) -> u32 {
        self.ticks.wrapping_add(if self.running {
            n.saturating_sub(self.anchor).wrapping_mul(26) as u32
        } else {
            0
        })
    }

    pub fn access(&mut self, c: Cpu, write: bool, a: u32, v: u32, n: u64) -> Option<u32> {
        let p = a & !0xfff;
        let o = a & 0xfff;
        if write {
            if a == 0x48004004 {
                c.put(0x48004024, u32::from(v & 7 == 7))
            }
            if p == 0x48088000 {
                if o == 0x28 {
                    self.ticks = v;
                    self.anchor = n
                } else if o == 0x24 {
                    self.ticks = self.time(n);
                    self.anchor = n;
                    self.running = v & 1 != 0;
                }
            }
            return None;
        }
        if let Some(v) = self.overrides.get(&a) {
            return Some(*v);
        }
        if ([0x4806a000, 0x4806c000, 0x49020000].contains(&p) && o == 0x58)
            || ([0x48070000, 0x48072000, 0x48060000].contains(&p) && o == 0x10)
            || (p == 0x48056000 && o == 0x28)
            || ([
                0x48098000, 0x4809a000, 0x480b8000, 0x480ba000, 0x49034000, 0x49036000, 0x49038000,
            ]
            .contains(&p)
                && o == 0x14)
            || a == 0x48094014
            || a == 0x5d000014
        {
            return Some(1);
        }
        if a == 0x6e000054 {
            return Some(0x100);
        }
        if a == 0x6e000084 && c.word(0x6e00007c) & 255 == 0x70 {
            return Some(0x40);
        }
        if p == 0x48088000 {
            return match o {
                0x14 => Some(1),
                0x28 => Some(self.time(n)),
                _ => None,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_mask_and_ack() {
        let mut i = Interrupts::new(1000);
        i.level(58, true);
        assert!(i.next_irq().is_none());
        i.masks[1] &= !(1 << 26);
        assert_eq!(i.next_irq(), Some(58));
        i.active = Some(58);
        assert!(i.next_irq().is_none());
        i.active = None;
        i.level(58, false);
        assert!(i.next_irq().is_none());
    }

    #[test]
    fn timer_is_pending_until_ack() {
        let mut i = Interrupts::new(1000);
        i.running = true;
        i.enabled = true;
        i.update(1000);
        assert!(i.levels.contains(&38));
        i.update(1001);
        assert!(i.levels.contains(&38));
    }
}
