//! ARM machine setup, native callbacks and observable execution state.

use crate::{
    devices::{Boot, Interrupts},
    display::{Control, Display, Frame},
    input::{Input, NativeInput},
    offline::{MenuEntry, Offline},
    storage::Storage,
    unicorn::{Api, CPSR, Cpu, Handle, LR, PC, R0, SP},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    path::PathBuf,
    time::Instant,
};
pub const EXPECTED_SHA256: &str =
    "9abec2a4fe588cc523c9b87b9190729a67a1e750463b08b5e175d32309f29ef0";
#[derive(Clone)]
pub struct Config {
    pub firmware: PathBuf,
    pub assets: PathBuf,
    pub offline: bool,
    pub virtual_storage: bool,
    pub hardware: bool,
    pub allow_unknown: bool,
}

#[derive(Clone, Default, Serialize)]
pub struct Report {
    pub status: String,
    pub error: Option<String>,
    pub instructions: u64,
    pub elapsed_seconds: f64,
    pub mips: f64,
    pub pc: u32,
    pub firmware_sha256: String,
    pub full_camera_boot_verified: bool,
    pub registers: Vec<(String, u32)>,
    pub console: String,
    pub stages: Vec<String>,
    pub menu: Vec<MenuEntry>,
    pub controls: Vec<Control>,
    pub mmio_pages: usize,
    pub mmio_accesses: u64,
    pub irq_deliveries: u64,
    pub input_delivered: u64,
    pub native_manager_count: u32,
    pub hardware_enabled: bool,
    pub framebuffer: Option<[usize; 2]>,
    pub hardware: Option<crate::hardware::Summary>,
    pub native_hardware_checks: BTreeMap<String, bool>,
}
// Engine never crosses threads: the worker constructs and destroys it locally.
// Callbacks refer to the stable boxed State, and uc_close runs before State drops.
pub struct Engine {
    handle: Handle,
    state: Box<State>,
}

struct State {
    api: *const Api,
    instructions: u64,
    initialization_returns: BTreeMap<u32, (String, u32)>,
    native_checks: BTreeMap<String, bool>,
    last_irq: u64,
    pages: BTreeSet<u32>,
    accesses: u64,
    boot: Boot,
    irq: Interrupts,
    hardware: Option<crate::hardware::Hardware>,
    display: Display,
    storage: Option<Storage>,
    input: NativeInput,
    offline: Offline,
    config: Config,
    console: Vec<u8>,
    reason: Option<String>,
    fault: Option<String>,
    hash: String,
    started: Instant,
}

impl Engine {
    pub fn new(config: Config) -> Result<Self, String> {
        let data = std::fs::read(&config.firmware).map_err(|e| e.to_string())?;
        if data.is_empty() || data.len() > 128 * 1024 * 1024 {
            return Err("Image vide ou trop grande pour la RAM".into());
        }
        let hash = format!("{:x}", Sha256::digest(&data));
        if (config.offline || config.virtual_storage)
            && hash != EXPECTED_SHA256
            && !config.allow_unknown
        {
            return Err(concat!(
                "Le profil interface exige le dsmc_program.mem extrait de RED 6.3.106. ",
                "Un paquet de mise a jour .bin ne peut pas etre execute directement.",
            )
            .into());
        }
        let handle = Handle::new()?;
        let cpu = handle.cpu();
        cpu.cortex_a8()?;
        cpu.map(0x80000000, 128 * 1024 * 1024)?;
        cpu.map(0, 0x10000)?;
        cpu.map(0x40200000, 0x10000)?;
        cpu.write(0x80000000, &data)?;
        cpu.set(CPSR, 0xd3);
        cpu.set(SP, 0x87fffff0);
        cpu.set(LR, 0x1000);
        cpu.set(PC, 0x80000000);
        cpu.set_cp(12, 0, 0, 0, 0x80000000)?;
        let storage = if config.virtual_storage {
            Some(Storage::new(cpu, &config.assets)?)
        } else {
            None
        };
        let hardware = if config.hardware {
            Some(crate::hardware::Hardware::new())
        } else {
            None
        };
        let mut irq = Interrupts::new(if config.hardware { 100000 } else { 1000 });
        irq.external_i2c = config.hardware;
        let mut state = Box::new(State {
            api: &*handle.api,
            instructions: 0,
            initialization_returns: BTreeMap::new(),
            native_checks: BTreeMap::new(),
            last_irq: 0,
            pages: BTreeSet::new(),
            accesses: 0,
            boot: Boot::default(),
            irq,
            hardware,
            display: Display::default(),
            storage,
            input: NativeInput::default(),
            offline: Offline::default(),
            config,
            console: Vec::new(),
            reason: None,
            fault: None,
            hash,
            started: Instant::now(),
        });
        let user = (&mut *state as *mut State).cast();
        unsafe {
            cpu.hook(8, block as *mut c_void, user, 1, 0)?;
            cpu.hook(112, unmapped as *mut c_void, user, 1, 0)?;
            cpu.hook(
                1024 | 2048,
                memory as *mut c_void,
                user,
                0x10000000,
                0x6fffffff,
            )?;
            for (i, bytes) in data.chunks_exact(4).enumerate() {
                if bytes == [0x70, 0, 0x60, 0xe1] {
                    let address = 0x80000000 + i as u64 * 4;
                    cpu.hook(4, smc as *mut c_void, user, address, address)?;
                }
            }
        }
        Ok(Self { handle, state })
    }

    pub fn run_slice(&mut self, count: usize, micros: u64) -> Result<(), String> {
        if self.state.reason.is_some() {
            return Ok(());
        }
        let cpu = self.handle.cpu();
        let pc = cpu.reg(PC) | ((cpu.reg(CPSR) >> 5) & 1);
        if let Err(e) = cpu.start(pc, count, micros) {
            self.state.reason = Some("Erreur processeur".into());
            self.state.fault = Some(e.clone());
            return Err(e);
        }
        Ok(())
    }

    pub fn done(&self) -> bool {
        self.state.reason.is_some()
    }

    pub fn enqueue(&mut self, i: Input) {
        self.state.input.enqueue(i)
    }

    pub fn frame(&self) -> Option<Frame> {
        self.state
            .display
            .frame(self.handle.cpu(), self.state.instructions)
    }

    pub fn report(&self) -> Report {
        let state = &self.state;
        let cpu = self.handle.cpu();
        let elapsed = state.started.elapsed().as_secs_f64();
        let frame = state.display.frame(cpu, state.instructions);
        let mut registers: Vec<_> = (0..13)
            .map(|i| (format!("r{i}"), cpu.reg(R0 + i)))
            .collect();
        for (name, id) in [("sp", SP), ("lr", LR), ("pc", PC), ("cpsr", CPSR)] {
            registers.push((name.into(), cpu.reg(id)))
        }
        Report {
            status: state.reason.clone().unwrap_or_else(|| "Execution".into()),
            error: state.fault.clone(),
            instructions: state.instructions,
            elapsed_seconds: elapsed,
            mips: state.instructions as f64 / elapsed.max(0.001) / 1e6,
            pc: cpu.reg(PC),
            firmware_sha256: state.hash.clone(),
            full_camera_boot_verified: false,
            registers,
            console: String::from_utf8_lossy(&state.console).into_owned(),
            stages: state.offline.stages.clone(),
            menu: state.offline.menu.clone(),
            controls: state.display.controls(cpu),
            mmio_pages: state.pages.len(),
            mmio_accesses: state.accesses,
            irq_deliveries: state.irq.deliveries.values().sum(),
            input_delivered: state.input.delivered,
            native_manager_count: state.offline.manager_count,
            hardware_enabled: state.config.hardware,
            framebuffer: frame.map(|f| [f.width, f.height]),
            hardware: state.hardware.as_ref().map(|h| h.summary(cpu)),
            native_hardware_checks: state.native_checks.clone(),
        }
    }
}

impl State {
    fn on_block(&mut self, cpu: Cpu, pc: u32, size: u32) {
        self.instructions += (size / 4).max(1) as u64;
        self.observe_initialization(cpu, pc);

        if pc == 0x1000 {
            self.reason = Some("Retour au PC hote".into());
            cpu.stop();
            return;
        }
        if self.input.dispatch(
            cpu,
            pc,
            self.instructions,
            self.config.offline,
            &mut self.offline.async_launch,
        ) {
            return;
        }
        if self.config.offline {
            if pc == 0x80aa1148 && self.display.fill(cpu) {
                cpu.set(PC, cpu.reg(LR));
                return;
            }
            if self.offline.dispatch(
                cpu,
                pc,
                self.instructions,
                self.config.hardware,
                self.storage.as_ref(),
            ) {
                return;
            }
        }
        if pc == 0x80aa21c4 {
            self.display.observe(cpu)
        }
        if let Some(state) = &mut self.storage {
            if state.dispatch(cpu, pc) {
                return;
            }
        }
        self.observe_boot_stage(pc);
        self.tick_interrupts(cpu, pc);
    }

    fn observe_initialization(&mut self, cpu: Cpu, pc: u32) {
        if self.config.hardware {
            if let Some((name, expected)) = self.initialization_returns.remove(&pc) {
                self.native_checks.insert(name, cpu.reg(R0) == expected);
            }
            for (address, name, expected) in [
                (0x8058b8f8, "TPS65950", 1),
                (0x807b0e64, "brain GPIO expander", 0),
                (0x805751d0, "NAND", 1),
            ] {
                if pc == address {
                    self.initialization_returns
                        .insert(cpu.reg(LR), (name.into(), expected));
                }
            }
        }
    }

    fn observe_boot_stage(&mut self, pc: u32) {
        for (address, name) in [
            (0x80565434, "Initialisation des cartes"),
            (0x80565790, "Creation des pilotes"),
            (0x80008df0, "Application"),
            (0x80a9d9ec, "Moteur graphique PEG"),
            (0x80aa9760, "Initialisation graphique"),
            (0x8009e2b4, "Services camera"),
            (0x80106b80, "Definitions des menus"),
            (0x800ebac0, "Interface utilisateur"),
        ] {
            if pc == address && !self.offline.stages.iter().any(|state| state == name) {
                self.offline.stages.push(name.into());
            }
        }
    }

    fn tick_interrupts(&mut self, cpu: Cpu, pc: u32) {
        if pc == 0x8001104c && self.config.hardware && self.irq.running {
            self.irq.update(self.instructions);
            if self.irq.next_irq().is_none() && self.irq.active.is_none() {
                self.irq.next = self.irq.next.min(self.instructions)
            }
        }
        if self.instructions - self.last_irq >= 128 {
            self.last_irq = self.instructions;
            if let Some(h) = &mut self.hardware {
                h.tick(cpu, &mut self.irq);
            }
            self.irq.deliver(cpu, pc, self.instructions);
        }
    }

    fn peripheral(address: u32, size: usize) -> bool {
        let e = address as u64 + size as u64;
        [
            (0x10000000u64, 0x20000000u64),
            (0x48000000, 0x60000000),
            (0x68000000, 0x70000000),
        ]
        .iter()
        .any(|(lo, hi)| address as u64 >= *lo && e <= *hi)
    }

    fn on_memory(&mut self, cpu: Cpu, write: bool, address: u32, size: usize, value: u32) {
        if !self.pages.contains(&(address & !0xfff)) {
            return;
        }
        self.accesses += 1;
        let mut answer = self
            .boot
            .access(cpu, write, address, value, self.instructions);
        let modeled = self.hardware.as_mut().and_then(|h| {
            h.access(
                cpu,
                write,
                address,
                size,
                value,
                self.instructions,
                &mut self.irq,
            )
        });
        if let Some(value) = modeled {
            answer = value;
        }
        if !(modeled.is_some() && crate::i2c::BASES.contains(&(address & !0xfff))) {
            if let Some(value) = self
                .irq
                .access(cpu, write, address, value, self.instructions)
            {
                answer = Some(value)
            }
        }
        let p = address & !0xfff;
        let o = address & 0xfff;
        if [0x4806a000, 0x4806c000, 0x49020000].contains(&p) {
            if !write && o == 0x14 {
                answer = Some(0x60)
            } else if write
                && o == 0
                && value & 255 != 0
                && cpu.word(p + 0xc) & 0x80 == 0
                && self.console.len() < 65536
            {
                self.console.push(value as u8)
            }
        }
        if !write {
            if let Some(value) = answer {
                let b = (value as u64).to_le_bytes();
                let _ = cpu.write(address, &b[..size.min(8)]);
            }
        }
    }
}

fn guarded(raw: *mut c_void, user: *mut c_void, callback: impl FnOnce(&mut State, Cpu)) {
    // SAFETY: Engine pins State; Unicorn invokes synchronously on its owning worker.
    let state = unsafe { &mut *user.cast::<State>() };
    let cpu = Cpu::new(raw, unsafe { &*state.api });
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(state, cpu))).is_err() {
        state.reason = Some("Erreur interne".into());
        state.fault = Some("Panique interceptee dans un callback Unicorn".into());
        cpu.stop();
    }
}

unsafe extern "C" fn block(raw: *mut c_void, pc: u64, size: u32, user: *mut c_void) {
    guarded(raw, user, |state, cpu| state.on_block(cpu, pc as u32, size))
}

unsafe extern "C" fn smc(raw: *mut c_void, pc: u64, size: u32, user: *mut c_void) {
    guarded(raw, user, |_, cpu| cpu.set(PC, pc as u32 + size))
}

unsafe extern "C" fn memory(
    raw: *mut c_void,
    kind: i32,
    address: u64,
    size: i32,
    value: i64,
    user: *mut c_void,
) {
    guarded(raw, user, |state, cpu| {
        state.on_memory(cpu, kind == 17, address as u32, size as usize, value as u32)
    })
}

unsafe extern "C" fn unmapped(
    raw: *mut c_void,
    kind: i32,
    address: u64,
    size: i32,
    value: i64,
    user: *mut c_void,
) -> bool {
    let mut accepted = false;
    guarded(raw, user, |state, cpu| {
        if address > u32::MAX as u64
            || kind == 21
            || size <= 0
            || !State::peripheral(address as u32, size as usize)
        {
            return;
        }
        let end = (address + size as u64 + 4095) & !4095;
        let count = ((end - (address & !4095)) / 4096) as usize;
        if state.pages.len() + count > 4096 {
            return;
        }
        for page in ((address & !4095)..end).step_by(4096) {
            if !state.pages.contains(&(page as u32)) {
                if cpu.map(page as u32, 4096).is_err() {
                    return;
                }
                state.pages.insert(page as u32);
            }
        }
        if kind == 20 {
            state.on_memory(cpu, true, address as u32, size as usize, value as u32)
        }
        accepted = true;
    });
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(words: &[u32]) -> Engine {
        let path = std::env::temp_dir().join(
            format!(
                "dsmc-fixture-{}-{:?}.mem",
                std::process::id(),
                std::thread::current().id()
            )
            .replace(['(', ')'], ""),
        );
        let data: Vec<u8> = words.iter().flat_map(|value| value.to_le_bytes()).collect();
        std::fs::write(&path, data).unwrap();
        let result = Engine::new(Config {
            firmware: path.clone(),
            assets: std::env::temp_dir(),
            offline: false,
            virtual_storage: false,
            hardware: true,
            allow_unknown: true,
        });
        let _ = std::fs::remove_file(path);
        result.unwrap()
    }

    #[test]
    fn original_arm_calls_return_through_hook() {
        let mut e = fixture(&[0xe3a0002a, 0xe12fff1e]);
        e.run_slice(100, 100000).unwrap();
        assert!(e.done());
        assert_eq!(e.handle.cpu().reg(R0), 42);
        assert_eq!(e.report().pc, 0x1000);
    }

    #[test]
    fn invalid_ram_is_not_invented() {
        let mut e = fixture(&[0xe3a00209, 0xe12fff10]);
        assert!(e.run_slice(100, 100000).is_err());
        assert_eq!(e.state.pages.len(), 0);
    }

    #[test]
    fn register_and_mmio_callbacks_reach_virtual_gpio() {
        // ldr r0,base; mov r1,#1; str r1,[r0,#0x94]; ldr r2,[r0,#0x3c]; bx lr
        let mut e = fixture(&[
            0xe59f000c, 0xe3a01001, 0xe5801094, 0xe590203c, 0xe12fff1e, 0x49056000,
        ]);
        e.run_slice(100, 100000).unwrap();
        assert_eq!(e.handle.cpu().reg(R0 + 2), 1);
        assert!(e.state.pages.contains(&0x49056000));
    }

    #[test]
    fn native_dma_transfers_and_separate_acknowledgments() {
        let e = fixture(&[0xe12fff1e]);
        let cpu = e.handle.cpu();
        let mut h = crate::hardware::Hardware::new();
        let mut irq = Interrupts::new(1000);
        cpu.write(0x83000000, b"firmware").unwrap();
        for (o, value) in [
            (0x18, 1),
            (0x80 + 8, 0x20),
            (0x80 + 0x10, 0),
            (0x80 + 0x14, 8),
            (0x80 + 0x18, 1),
            (0x80 + 0x1c, 0x83000000),
            (0x80 + 0x20, 0x83001000),
            (0x80, 0x5080),
        ] {
            h.access(cpu, true, 0x48056000 + o, 4, value, 0, &mut irq);
        }
        h.tick(cpu, &mut irq);
        assert_eq!(cpu.read(0x83001000, 8).unwrap(), b"firmware");
        assert!(irq.levels.contains(&12));
        assert_eq!(
            h.access(cpu, false, 0x4805608c, 4, 0, 0, &mut irq),
            Some(Some(0x20))
        );
        h.access(cpu, true, 0x48056008, 4, 1, 0, &mut irq);
        assert!(!irq.levels.contains(&12));
        assert_eq!(
            h.access(cpu, false, 0x4805608c, 4, 0, 0, &mut irq),
            Some(Some(0x20))
        );
        h.access(cpu, true, 0x4805608c, 4, 0x20, 0, &mut irq);
        assert_eq!(
            h.access(cpu, false, 0x4805608c, 4, 0, 0, &mut irq),
            Some(Some(0))
        );
    }

    #[test]
    fn input_wrapper_preserves_live_task_registers_stack_and_flags() {
        let mut e = fixture(&[0xe0a00001, 0xe12fff1e]);
        let cpu = e.handle.cpu();
        for (address, value) in [
            (0x810da120, 0x83420000),
            (0x83420000, 0x54485244),
            (0x8342000c, 0x83430000),
            (0x83420010, 0x8343ffff),
            (0x824d63d8, 0x54485244),
            (0x825de90c, 0x83450000),
        ] {
            cpu.put(address, value)
        }
        cpu.bytes(
            0x80a9e148,
            &[
                0xe3a00001, 0xe3a01020, 0xe3a02030, 0xe3a03040, 0xe3a0c050, 0xe3500002, 0xe12fff1e,
            ],
        );
        for (id, value) in [
            (R0, 3),
            (R0 + 1, 4),
            (R0 + 2, 12),
            (R0 + 3, 13),
            (R0 + 12, 22),
            (SP, 0x8343fff0),
            (CPSR, 0x200f0053),
        ] {
            cpu.set(id, value)
        }
        e.enqueue(Input::Down(160, 85));
        e.run_slice(200, 100000).unwrap();
        let cpu = e.handle.cpu();
        assert_eq!(cpu.reg(R0), 8);
        assert_eq!(cpu.reg(R0 + 1), 4);
        assert_eq!(cpu.reg(R0 + 2), 12);
        assert_eq!(cpu.reg(R0 + 3), 13);
        assert_eq!(cpu.reg(R0 + 12), 22);
        assert_eq!(cpu.reg(SP), 0x8343fff0);
        assert_eq!(cpu.reg(CPSR) & 0xf0000, 0xf0000);
        assert_eq!(e.state.input.delivered, 1);
    }
}
