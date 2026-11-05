//! OMAP DMA/SPI/GPIO transport with a software genlock FPGA endpoint.
use crate::{
    devices::Interrupts,
    display::valid,
    i2c::{BASES, I2c},
    nand::Nand,
    onewire::OneWire,
    unicorn::Cpu,
};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
const SPI: [u32; 4] = [0x48098000, 0x4809a000, 0x480b8000, 0x480ba000];
const GPIO: [u32; 6] = [
    0x48310000, 0x49050000, 0x49052000, 0x49054000, 0x49056000, 0x49058000,
];
#[derive(Clone, Default, Serialize)]
pub struct Summary {
    pub dma_completed: u64,
    pub dma_bytes: u64,
    pub dma_errors: Vec<String>,
    pub spi_words: u64,
    pub i2c_transactions: u64,
    pub board_devices: Vec<serde_json::Value>,
    pub power_sequencer_kinds: Vec<u32>,
    pub i2c_nacks: u64,
    pub missing_i2c_devices: Vec<(usize, u8, u64)>,
    pub onewire_resets: u64,
    pub virtual_board_count: usize,
    pub nand_programs: u64,
    pub nand_erases: u64,
    pub native_board_index: Vec<u8>,
    pub genlock_fpga: crate::fpga::Summary,
    pub capture: crate::ccdc::Summary,
    pub iob: crate::iob::Summary,
    pub coverage: &'static [crate::coverage::Component],
}

pub struct Hardware {
    i2c: I2c,
    nand: Nand,
    wire: OneWire,
    genlock: crate::fpga::Genlock,
    ccdc: crate::ccdc::Ccdc,
    iob: crate::iob::Iob,
    remote: [crate::fpga::Remote; 4],
    gpio: [BTreeMap<u32, u32>; 6],
    dma: [[u32; 24]; 32],
    progress: [Option<u32>; 32],
    irq_status: [u32; 4],
    irq_enable: [u32; 4],
    spi: [BTreeMap<u32, u32>; 4],
    rx: [[VecDeque<u32>; 4]; 4],
    completed: u64,
    bytes: u64,
    errors: VecDeque<String>,
    words: u64,
}

impl Hardware {
    pub fn new() -> Self {
        Self {
            i2c: I2c::new(),
            nand: Nand::new(),
            wire: OneWire::new(),
            genlock: crate::fpga::Genlock::default(),
            ccdc: crate::ccdc::Ccdc::default(),
            iob: crate::iob::Iob::default(),
            remote: std::array::from_fn(|_| crate::fpga::Remote::default()),
            gpio: std::array::from_fn(|_| BTreeMap::from([(0x34, u32::MAX), (0x3c, 0), (0x38, 0)])),
            dma: [[0; 24]; 32],
            progress: [None; 32],
            irq_status: [0; 4],
            irq_enable: [0; 4],
            spi: std::array::from_fn(|_| BTreeMap::new()),
            rx: std::array::from_fn(|_| std::array::from_fn(|_| VecDeque::new())),
            completed: 0,
            bytes: 0,
            errors: VecDeque::new(),
            words: 0,
        }
    }

    pub fn summary(&self, c: Cpu) -> Summary {
        let mut genlock_fpga = self.genlock.summary();
        genlock_fpga.remote_reads = self.remote.iter().map(|remote| remote.reads).sum();
        genlock_fpga.remote_writes = self.remote.iter().map(|remote| remote.writes).sum();
        genlock_fpga.remote_unsupported = self.remote.iter().map(|remote| remote.unsupported).sum();
        Summary {
            dma_completed: self.completed,
            dma_bytes: self.bytes,
            dma_errors: self.errors.iter().cloned().collect(),
            spi_words: self.words,
            i2c_transactions: self.i2c.transactions,
            board_devices: self.i2c.board_devices(),
            power_sequencer_kinds: (0..6).map(|index| c.word(0x81065e00 + index * 4)).collect(),
            i2c_nacks: self.i2c.nacks,
            missing_i2c_devices: self.i2c.missing_devices(),
            onewire_resets: self.wire.resets,
            virtual_board_count: 9,
            nand_programs: self.nand.programs,
            nand_erases: self.nand.erases,
            native_board_index: c.read(0x819be74c, 54).unwrap_or_default(),
            genlock_fpga,
            capture: self.ccdc.summary(),
            iob: self.iob.summary(),
            coverage: crate::coverage::COMPONENTS,
        }
    }

    fn decode(o: u32) -> Option<(usize, u32)> {
        if (0x2c..0x7c).contains(&o) {
            Some((((o - 0x2c) / 0x14) as usize, (o - 0x2c) % 0x14))
        } else {
            None
        }
    }

    fn reg(&self, p: usize, o: u32) -> u32 {
        *self.spi[p].get(&o).unwrap_or(&0)
    }

    fn irqs(&self, irq: &mut Interrupts) {
        for b in 0..4 {
            irq.level(12 + b, self.irq_status[b] & self.irq_enable[b] != 0);
            irq.level(
                [65, 66, 91, 48][b],
                self.reg(b, 0x18) & self.reg(b, 0x1c) != 0,
            );
        }
    }

    fn transfer(&mut self, p: usize, ch: usize, v: u32) {
        let conf = self.reg(p, 0x2c + ch as u32 * 0x14);
        let bits = ((conf >> 7) & 31) + 1;
        let mask = if bits == 32 {
            u32::MAX
        } else {
            (1 << bits) - 1
        };
        self.words += 1;
        let reply = if p == 2 && ch == 1 && bits == 16 {
            self.genlock.transfer(v as u16) as u32
        } else if p == 3 && ch == 0 && bits == 8 {
            let select = self.gpio[2][&0x3c];
            let target = (((select >> 12) & 1) << 1) | ((select >> 13) & 1);
            u32::from(self.remote[target as usize].transfer(v as u8))
        } else {
            // Logical device 7: genlock configuration, selected by GPIO71/73/75.
            let outputs = self.gpio[2][&0x3c];
            if p == 1
                && ch == 0
                && bits == 8
                && outputs & ((1 << 7) | (1 << 9) | (1 << 11)) == (1 << 7) | (1 << 9)
            {
                self.genlock.configure(v as u8);
            }
            mask
        };
        if (conf >> 12) & 3 != 2 && self.rx[p][ch].len() < 4096 {
            self.rx[p][ch].push_back(reply & mask)
        }
        *self.spi[p].entry(0x18).or_default() |= (1 << (ch * 4)) | 0x20000;
    }

    pub fn access(
        &mut self,
        c: Cpu,
        write: bool,
        a: u32,
        size: usize,
        v: u32,
        n: u64,
        irq: &mut Interrupts,
    ) -> Option<Option<u32>> {
        let page = a & !0xfff;
        let o = a & 0xfff;
        let mut answer = None;
        if let Some(answer) = self.iob.access(write, a, size, v) {
            return Some(answer);
        }
        if page == crate::ccdc::BASE {
            return Some(self.ccdc.access(write, o, v, irq));
        }
        if page == 0x480b2000 {
            return Some(self.wire.access(write, o, v, irq));
        }
        if let Some(i) = GPIO.iter().position(|p| *p == page) {
            let r = &mut self.gpio[i];
            if write {
                match o {
                    0x10 if v & 2 != 0 => {
                        r.insert(0x34, u32::MAX);
                        r.insert(0x3c, 0);
                        r.insert(0x38, 0);
                    }
                    0x90 => *r.entry(0x3c).or_default() &= !v,
                    0x94 => *r.entry(0x3c).or_default() |= v,
                    0x18 => *r.entry(o).or_default() &= !v,
                    _ => {
                        r.insert(o, v);
                    }
                }
            } else {
                answer = Some(match o {
                    0x14 => 1,
                    0x38 => (r[&0x38] & r[&0x34]) | (r[&0x3c] & !r[&0x34]),
                    _ => *r.get(&o).unwrap_or(&0),
                });
            }
            if i == 4 && write && self.gpio[4][&0x34] & (1 << 10) == 0 {
                self.genlock.program(self.gpio[4][&0x3c] & (1 << 10) != 0);
            }
            if !write && o == 0x38 {
                let signal = match i {
                    4 => Some((21, self.genlock.done_high())),
                    5 => Some((2, self.genlock.init_high())),
                    _ => None,
                };
                if let Some((bit, high)) = signal {
                    // External FPGA signals affect input pins only.
                    if self.gpio[i][&0x34] & (1 << bit) != 0 {
                        answer =
                            answer.map(|value| (value & !(1 << bit)) | (u32::from(high) << bit));
                    }
                }
            }
            return Some(answer);
        }
        if a == 0x6e00007c && write {
            self.nand.command(v as u8);
            return Some(None);
        }
        if a == 0x6e000080 && write {
            self.nand.address(v as u8);
            return Some(None);
        }
        if a == 0x6e000084 || a == 0x10000000 {
            if write {
                self.nand.write(&v.to_le_bytes()[..size.min(4)]);
            } else {
                let b = self.nand.read(size);
                let _ = c.write(a, &b);
            }
            return Some(None);
        }
        if !write && (a == 0x6e000014 || a == 0x6e000054) {
            return Some(Some(if a == 0x6e000014 { 1 } else { 0x100 }));
        }
        if let Some(bus) = BASES.iter().position(|p| *p == page) {
            answer = self.i2c.access(bus, write, o, size, v, n, irq);
            self.irqs(irq);
            return Some(answer);
        }
        if page == 0x48056000 {
            if (8..0x18).contains(&o) && o % 4 == 0 {
                let b = ((o - 8) / 4) as usize;
                if write {
                    self.irq_status[b] &= !v
                } else {
                    answer = Some(self.irq_status[b])
                }
            } else if (0x18..0x28).contains(&o) && o % 4 == 0 {
                let b = ((o - 0x18) / 4) as usize;
                if write {
                    self.irq_enable[b] = v
                } else {
                    answer = Some(self.irq_enable[b])
                }
            } else if o == 0x28 && !write {
                answer = Some(1)
            } else if o == 0x2c && write && v & 2 != 0 {
                self.dma = [[0; 24]; 32];
                self.progress = [None; 32];
                self.irq_status = [0; 4];
                self.irq_enable = [0; 4];
            } else if (0x80..0xc80).contains(&o) && o % 4 == 0 {
                let ch = ((o - 0x80) / 0x60) as usize;
                let reg = (((o - 0x80) % 0x60) / 4) as usize;
                if write {
                    if reg == 3 {
                        self.dma[ch][reg] &= !v
                    } else {
                        self.dma[ch][reg] = v
                    }
                    if reg == 0 {
                        self.progress[ch] = if v & 0x80 != 0 { Some(0) } else { None }
                    }
                } else {
                    answer = Some(self.dma[ch][reg])
                }
            }
        } else {
            let p = SPI.iter().position(|p| *p == page)?;
            if o == 0x14 && !write {
                answer = Some(1)
            } else if o == 0x10 && write && v & 2 != 0 {
                self.spi[p].clear();
                if p == 2 {
                    self.genlock.deselect();
                }
                if p == 3 {
                    for remote in &mut self.remote {
                        remote.deselect();
                    }
                }
                for f in &mut self.rx[p] {
                    f.clear()
                }
            } else if o == 0x18 {
                if write {
                    *self.spi[p].entry(o).or_default() &= !v
                } else {
                    answer = Some(self.reg(p, o))
                }
            } else if let Some((ch, reg)) = Self::decode(o) {
                if write {
                    if p == 2 && ch == 1 && reg == 0 && v & 0x100000 == 0 {
                        self.genlock.deselect();
                    }
                    if p == 3 && ch == 0 && reg == 0 && v & 0x100000 == 0 {
                        for remote in &mut self.remote {
                            remote.deselect();
                        }
                    }
                    self.spi[p].insert(o, v);
                    if reg == 0xc {
                        self.transfer(p, ch, v)
                    }
                } else {
                    answer = Some(match reg {
                        4 => 6 | u32::from(!self.rx[p][ch].is_empty()),
                        0x10 => self.rx[p][ch].pop_front().unwrap_or(u32::MAX),
                        _ => self.reg(p, o),
                    })
                }
            } else if write {
                self.spi[p].insert(o, v);
            } else {
                answer = Some(self.reg(p, o))
            }
        }
        self.irqs(irq);
        Some(answer)
    }

    fn endpoint(a: u32) -> Option<(usize, usize, u32)> {
        let p = SPI.iter().position(|p| *p == a & !0xfff)?;
        let (ch, r) = Self::decode(a & 0xfff)?;
        Some((p, ch, r))
    }

    fn ready(&self, e: Option<(usize, usize, u32)>, read: bool) -> bool {
        let Some((p, ch, r)) = e else { return true };
        r == if read { 0x10 } else { 0xc }
            && self.reg(p, 0x34 + ch as u32 * 0x14) & 1 != 0
            && self.reg(p, 0x2c + ch as u32 * 0x14) & if read { 0x8000 } else { 0x4000 } != 0
            && (!read || !self.rx[p][ch].is_empty())
    }

    fn finish(&mut self, ch: usize, status: u32, error: Option<String>) {
        self.dma[ch][0] &= !0x80;
        self.dma[ch][3] |= status;
        self.progress[ch] = None;
        if status & self.dma[ch][2] != 0 {
            for b in &mut self.irq_status {
                *b |= 1 << ch
            }
        }
        if let Some(e) = error {
            if self.errors.len() >= 16 {
                self.errors.pop_front();
            }
            self.errors.push_back(e);
        } else {
            self.completed += 1
        }
    }

    fn ram(a: u32, n: usize) -> bool {
        valid(a, n) || (a >= 0x40200000 && a as u64 + n as u64 <= 0x40210000)
    }

    pub fn sensor_frame(&mut self, cpu: Cpu, frame: &crate::display::Frame, irq: &mut Interrupts) {
        self.ccdc.receive(cpu, frame, irq);
    }

    pub fn busy_dma(&self) -> bool {
        self.progress.iter().any(Option::is_some)
    }

    pub fn tick(&mut self, c: Cpu, irq: &mut Interrupts) {
        for ch in 0..32 {
            let Some(mut done) = self.progress[ch] else {
                continue;
            };
            let r = self.dma[ch];
            let width = 1u32 << (r[4] & 3);
            let elems = r[5];
            let count = elems as u64 * r[6] as u64;
            let (src, dst) = (r[7], r[8]);
            let (sm, dm) = ((r[0] >> 12) & 3, (r[0] >> 14) & 3);
            if width > 4 || count == 0 || count > 2 * 1024 * 1024 || sm > 1 || dm > 1 {
                self.finish(
                    ch,
                    0x100,
                    Some("Unsupported DMA geometry/address mode".into()),
                );
                continue;
            }
            let (se, de) = (Self::endpoint(src), Self::endpoint(dst));
            let sn = [0x10000000, 0x6e000084].contains(&src);
            let dn = [0x10000000, 0x6e000084].contains(&dst);
            if !self.ready(se, true) || !self.ready(de, false) {
                continue;
            }
            let mut error = None;
            for _ in 0..256.min(count as u32 - done) {
                let a = src.wrapping_add(if sm == 1 { done * width } else { 0 });
                let b = dst.wrapping_add(if dm == 1 { done * width } else { 0 });
                if se.is_none() && !sn && !Self::ram(a, width as usize) {
                    error = Some(format!("Unmodeled DMA source 0x{a:08x}"));
                    break;
                }
                if de.is_none() && !dn && !Self::ram(b, width as usize) {
                    error = Some(format!("Unmodeled DMA destination 0x{b:08x}"));
                    break;
                }
                let data = if sn {
                    self.nand.read(width as usize)
                } else if let Some((p, ch, _)) = se {
                    let Some(v) = self.rx[p][ch].pop_front() else {
                        break;
                    };
                    v.to_le_bytes()[..width as usize].to_vec()
                } else {
                    match c.read(a, width as usize) {
                        Ok(b) => b,
                        Err(e) => {
                            error = Some(e);
                            break;
                        }
                    }
                };
                if dn {
                    self.nand.write(&data)
                } else if let Some((p, ch, _)) = de {
                    let mut b = [0; 4];
                    b[..data.len()].copy_from_slice(&data);
                    self.transfer(p, ch, u32::from_le_bytes(b))
                } else if let Err(e) = c.write(b, &data) {
                    error = Some(e);
                    break;
                }
                done += 1;
                self.bytes += width as u64;
            }
            if let Some(e) = error {
                self.finish(ch, 0x100, Some(e));
                continue;
            }
            self.progress[ch] = Some(done);
            self.dma[ch][0x34 / 4] = src.wrapping_add(if sm == 1 { done * width } else { 0 });
            self.dma[ch][0x38 / 4] = dst.wrapping_add(if dm == 1 { done * width } else { 0 });
            self.dma[ch][0x3c / 4] = done % elems;
            self.dma[ch][0x40 / 4] = done / elems;
            if done as u64 == count {
                self.finish(ch, 0x20, None)
            }
        }
        self.irqs(irq);
    }
}
