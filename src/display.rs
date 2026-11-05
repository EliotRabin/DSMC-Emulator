//! Native firmware surfaces, not a recreation of camera menus.
use crate::unicorn::{Cpu, R0, SP};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
pub fn valid(a: u32, n: usize) -> bool {
    a >= 0x80000000 && (a as u64 + n as u64) <= 0x88000000
}

#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub serial: u64,
}

#[derive(Default)]
pub struct Display {
    surfaces: BTreeMap<u32, u32>,
    pub accelerated: u64,
}

#[derive(Clone, Serialize)]
pub struct Control {
    pub address: u32,
    pub label: String,
    pub rect: [i16; 4],
}

impl Display {
    pub fn observe(&mut self, c: Cpu) {
        let d = c.reg(R0 + 3);
        if valid(d, 56) && self.surfaces.len() < 32 {
            self.surfaces.insert(d, c.word(c.reg(SP)));
        }
    }

    pub fn frame(&self, c: Cpu, serial: u64) -> Option<Frame> {
        let screen = c.word(0x825de904);
        if !valid(screen, 0x38) || c.word(screen + 0x34) != 0x8102f4cc {
            return None;
        }
        let mut candidates: Vec<_> = self.surfaces.keys().copied().collect();
        let region = c.word(screen);
        if valid(region, 20) {
            candidates.push(c.word(region + 16));
        }
        let mut best = None;
        for surface in candidates {
            if !valid(surface, 4) {
                continue;
            }
            let bitmap = c.word(surface);
            if !valid(bitmap, 16) {
                continue;
            }
            let b = c.read(bitmap, 16).ok()?;
            let w = u16::from_le_bytes([b[2], b[3]]) as usize;
            let h = u16::from_le_bytes([b[4], b[5]]) as usize;
            let pixels = u32::from_le_bytes(b[12..16].try_into().ok()?);
            if b[1] != 32 || w == 0 || h == 0 || w > 1920 || h > 1080 || !valid(pixels, w * h * 4) {
                continue;
            }
            if best.is_none_or(|(_, bw, bh)| w * h > bw * bh) {
                best = Some((pixels, w, h));
            }
        }
        let (p, w, h) = best?;
        let mut rgba = c.read(p, w * h * 4).ok()?;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Some(Frame {
            width: w,
            height: h,
            rgba,
            serial,
        })
    }

    pub fn controls(&self, c: Cpu) -> Vec<Control> {
        let mut pending = vec![c.word(0x825de908)];
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        while let Some(w) = pending.pop() {
            if seen.len() >= 512 || result.len() >= 128 {
                break;
            }
            if !valid(w, 0x6c) || !seen.insert(w) {
                continue;
            }
            pending.push(c.word(w + 0x30));
            if w != c.word(0x825de908) && c.word(w + 0x44) & 1 == 0 {
                continue;
            }
            pending.push(c.word(w + 0x2c));
            if c.word(w + 0x50) != 0x80fa0304 {
                continue;
            }
            let raw = c.read(w + 4, 8).unwrap_or_default();
            if raw.len() != 8 {
                continue;
            }
            let rect = std::array::from_fn(|i| i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]));
            if rect[0] < 0
                || rect[1] < 0
                || rect[2] < rect[0]
                || rect[3] < rect[1]
                || rect[2] >= 1920
                || rect[3] >= 1080
            {
                continue;
            }
            let n = c.word(w + 0x60) as usize;
            let a = c.word(w + 0x58);
            if n == 0 || n > 256 || !valid(a, n * 2) {
                continue;
            }
            let b = c.read(a, n * 2).unwrap_or_default();
            let text: Vec<u16> = b
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .take_while(|v| *v != 0)
                .collect();
            result.push(Control {
                address: w,
                label: String::from_utf16_lossy(&text),
                rect,
            });
        }
        result
    }

    pub fn fill(&mut self, c: Cpu) -> bool {
        let screen = c.reg(R0);
        if !valid(screen, 0x38) || c.word(screen + 0x34) != 0x8102f4cc {
            return false;
        }
        let (x1, x2, y) = (c.reg(R0 + 1), c.reg(R0 + 2), c.reg(R0 + 3));
        let sp = c.reg(SP);
        if !valid(sp, 8) {
            return false;
        }
        let (color, h) = (c.word(sp), c.word(sp + 4));
        if (color != 0 && color >> 24 != 255) || x2 < x1 || x2 >= 1920 || h == 0 || h > 1080 {
            return false;
        }
        let width = (x2 - x1 + 1) as usize;
        let region = c.word(screen);
        if !valid(region, 8) {
            return false;
        }
        let (table, origin) = (c.word(region), c.word(region + 4));
        let Some(first) = y.checked_sub(origin) else {
            return false;
        };
        if first as u64 + h as u64 > 1080
            || !valid(
                table.saturating_add(first.saturating_mul(4)),
                h as usize * 4,
            )
        {
            return false;
        }
        let rows: Vec<_> = (0..h)
            .map(|r| c.word(table + (first + r) * 4).saturating_add(x1 * 4))
            .collect();
        if rows.iter().any(|a| !valid(*a, width * 4)) {
            return false;
        }
        for a in rows {
            if color != 0 {
                let b: Vec<_> = (0..width).flat_map(|_| color.to_le_bytes()).collect();
                let _ = c.write(a, &b);
            } else {
                let mut b = c.read(a, width * 4).unwrap_or_default();
                for p in b.chunks_exact_mut(4) {
                    if p[3] == 0 {
                        p.fill(0)
                    }
                }
                let _ = c.write(a, &b);
            }
        }
        self.accelerated += 1;
        true
    }
}
