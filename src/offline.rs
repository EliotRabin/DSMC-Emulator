//! Explicit offline inspection profile, ported from the original Python runner.
//! These shortcuts never claim a complete camera boot or generate menu pixels.
use crate::{
    display::valid,
    storage::Storage,
    unicorn::{Cpu, LR, PC, R0, SP},
};
use serde::Serialize;
use std::collections::BTreeMap;
#[derive(Clone, Serialize)]
pub struct MenuEntry {
    pub id: u32,
    pub label: String,
}

#[derive(Default)]
pub struct Offline {
    pub gate: bool,
    pub manager: bool,
    pub monitor: bool,
    pub controls: bool,
    pub async_launch: bool,
    cleanup: BTreeMap<u32, (u32, u32)>,
    pub menu: Vec<MenuEntry>,
    pub replies: u64,
    pub manager_count: u32,
    pub manager_callers: Vec<u32>,
    pub stages: Vec<String>,
}

impl Offline {
    fn launch(&mut self, c: Cpu, callback: u32, pc: u32) {
        c.set(R0, callback);
        c.set(R0 + 1, 0);
        c.set(LR, pc);
        self.async_launch = true;
        c.set(PC, 0x8007fbfc);
    }

    pub fn dispatch(
        &mut self,
        c: Cpu,
        pc: u32,
        n: u64,
        hardware: bool,
        storage: Option<&Storage>,
    ) -> bool {
        if pc == 0x8009e1cc {
            self.manager_count += 1;
            self.manager_callers.push(c.reg(LR));
        }
        if pc == 0x8069d2f0 && c.reg(R0 + 1) == 0x60 {
            let connection = c.reg(R0);
            if connection != 0
                && connection == c.word(0x81198330)
                && c.word(connection + 0xe34) & 0x10 == 0
            {
                let _ = c.write(0x4020f900, &[0; 0x68]);
                c.put(0x4020f910, 0x4020fa00);
                let _ = c.write(0x4020fa00, b"0\0");
                c.bytes(
                    0x4020fa80,
                    &[
                        0xe92d4010, 0xe59fc008, 0xe12fff3c, 0xe3a00000, 0xe8bd8010, 0x806b089c,
                    ],
                );
                c.set(R0 + 1, 0x4020f900);
                c.set(R0 + 2, 0x60);
                c.set(PC, 0x4020fa80);
                self.replies += 1;
                return true;
            }
        }
        if pc == 0x80839d6c {
            let vector = c.reg(R0 + 1);
            let (begin, end) = (c.word(vector + 4), c.word(vector + 8));
            if begin != 0
                && end >= begin
                && (end - begin) % 0x3c == 0
                && (1..=128).contains(&((end - begin) / 0x3c))
            {
                let mut entries = Vec::new();
                for node in (begin..end).step_by(0x3c) {
                    let len = c.word(node + 0x20);
                    let capacity = c.word(node + 0x24);
                    if len > 4096 || len > capacity {
                        break;
                    }
                    let p = if capacity > 15 {
                        c.word(node + 0x10)
                    } else {
                        node + 0x10
                    };
                    let label = c
                        .read(p, len as usize)
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    entries.push(MenuEntry {
                        id: c.word(node + 4),
                        label,
                    });
                }
                if !entries.is_empty() {
                    self.menu = entries;
                }
            }
        }
        if pc == 0x80aa1148 {
            return false;
        } // handled by the exact rectangle fast path
        if [0x800bc170, 0x801abe0c].contains(&pc) {
            c.set(R0, 1);
            c.set(PC, c.reg(LR));
            return true;
        }
        if pc == 0x805f3090 && (0x80a90000..0x80ae0000).contains(&c.reg(LR)) {
            let length = c.reg(R0 + 2) as usize;
            let ok = length <= 8 * 1024 * 1024 && valid(c.reg(R0 + 1), length);
            c.set(R0, if ok { 0 } else { u32::MAX });
            c.set(PC, c.reg(LR));
            return true;
        }
        if pc == 0x800f4b78 {
            let event = c.reg(R0);
            if (valid(event, 8) || (0x4020f000..=0x4020fff8).contains(&event))
                && c.word(event + 4) == 1
                && c.word(0x824d63d8) == 0x54485244
            {
                c.set(PC, c.reg(LR));
                return true;
            }
        }
        if self.async_launch && pc == 0x80c2227c && c.reg(R0 + 2) == 0x8007fbac {
            c.bytes(c.reg(SP) + 8, &[15, 15]);
            self.async_launch = false;
        }
        if pc == 0x4020f104 && self.manager {
            let sp = c.reg(SP) - 8;
            c.set(SP, sp);
            c.bytes(sp, &[0, 0x1000]);
            for (i, v) in [0, 0x10000, 0x80cedb90, 15].into_iter().enumerate() {
                c.set(R0 + i as i32, v)
            }
            c.set(LR, 0x4020f108);
            c.set(PC, 0x8009e1cc);
            return true;
        }
        if pc == 0x4020f108 && self.manager {
            c.set(SP, c.reg(SP) + 8);
            c.set(PC, 0x4020f10c);
            return true;
        }
        if pc == 0x4020f200 && self.monitor {
            c.set(R0, c.word(0x824d3e40));
            c.set(R0 + 1, 0x4020f400);
            c.set(PC, 0x80819354);
            return true;
        }
        if pc == 0x4020f304 || pc == 0x4020f604 {
            let thread = c.word(0x810da120);
            if let Some((baseline, ret)) = self.cleanup.get(&thread).copied() {
                let count = c.word(0x824d42fc);
                if c.word(0x824d4300) == thread && count > baseline && count <= 1024 {
                    c.set(LR, pc);
                    c.set(PC, 0x80a9e0d0);
                } else {
                    self.cleanup.remove(&thread);
                    c.set(PC, ret);
                }
                return true;
            }
        }
        if pc == 0x4020f600 || pc == 0x4020f300 {
            self.cleanup
                .insert(c.word(0x810da120), (c.word(0x824d42fc), c.reg(LR)));
            c.put(pc + 4, 0xe1a00000);
            c.set(LR, pc + 4);
            let manager = c.word(0x824d3e40);
            c.set(R0, c.word(manager + 0x14));
            c.set(R0 + 1, if pc == 0x4020f600 { 1 } else { manager + 0x1c });
            c.set(
                PC,
                if pc == 0x4020f600 {
                    0x80826058
                } else {
                    0x8086f120
                },
            );
            return true;
        }
        if [0x80c3ec3c, 0x80c3ed0c].contains(&pc) {
            let s = c.word(0x825e5ecc);
            if valid(s, 8) && c.word(s) == 0xc0c00000 && c.word(s + 4) != 0xbaba01ff {
                c.put(s + 4, 0xbaba01ff)
            }
        }
        if pc != 0x8001104c {
            return false;
        }
        if !self.gate
            && n >= 20000000
            && storage.is_some_and(|s| s.reads > 0 && s.writes > 0)
            && c.word(0x819208cc) == 0x53454d41
            && c.word(0x819208dc) != 0
        {
            self.gate = true;
            c.set(R0, 0x819208cc);
            c.set(LR, pc);
            c.set(PC, 0x80c22240);
            return true;
        }
        if n < 180000000 {
            return false;
        }
        if hardware && !self.manager {
            self.manager = true;
        }
        if !self.manager && c.word(0x81198398) == 0 {
            c.bytes(
                0x4020f100,
                &[0xe92d4010, 0xeafffffe, 0xe1a00000, 0xe8bd8010],
            );
            self.manager = true;
            self.launch(c, 0x4020f100, pc);
            return true;
        }
        let manager = c.word(0x824d3e40);
        let record = if manager != 0 {
            c.word(manager + 0x14)
        } else {
            0
        };
        if !self.controls && record != 0 && c.word(record + 0x34) == c.word(0x825de908) {
            c.put(0x4020f300, 0xe1a00000);
            self.controls = true;
            self.launch(c, 0x4020f300, pc);
            return true;
        }
        if !self.monitor && c.word(0x825de904) != 0 && manager != 0 {
            let mut attrs = [0; 0x148];
            for (offset, rect) in [
                (4, [0i16, 0, 959, 539]),
                (12, [0, 0, 959, 47]),
                (20, [0, 492, 959, 539]),
                (28, [0, 48, 959, 491]),
                (36, [0, 96, 959, 491]),
            ] {
                for (i, v) in rect.into_iter().enumerate() {
                    attrs[offset + i * 2..offset + i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
            attrs[0x3c..0x46].copy_from_slice(b"VIRTUAL-PC");
            attrs[0x34] = 1;
            attrs[0x13d] = 1;
            attrs[0x140] = 1;
            let _ = c.write(0x4020f400, &attrs);
            c.put(0x4020f200, 0xe1a00000);
            self.monitor = true;
            self.launch(c, 0x4020f200, pc);
            return true;
        }
        false
    }
}
