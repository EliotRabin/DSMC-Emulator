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

#[derive(Clone, Serialize)]
pub struct MmioObservation {
    pub address: u32,
    pub write: bool,
    pub size: usize,
    pub pc: u32,
    pub value: u32,
    pub hits: u64,
}

#[derive(Clone, Serialize)]
pub struct BootstrapThread {
    pub address: u32,
    pub name: String,
    pub priority: u32,
    pub state: u32,
    pub saved_stack: Vec<u32>,
}

#[derive(Clone, Serialize)]
pub struct FanState {
    pub installed: bool,
    pub kind: u32,
    pub rated_rpm: u32,
    pub driver_attached: bool,
}

#[derive(Clone, Default, Serialize)]
pub struct Report {
    pub status: String,
    pub error: Option<String>,
    pub instructions: u64,
    pub idle_instructions: u64,
    pub elapsed_seconds: f64,
    pub mips: f64,
    pub pc: u32,
    pub firmware_sha256: String,
    pub full_camera_boot_verified: bool,
    pub ui_ready: bool,
    pub registers: Vec<(String, u32)>,
    pub console: String,
    pub stages: Vec<String>,
    pub menu: Vec<MenuEntry>,
    pub controls: Vec<Control>,
    pub mmio_pages: usize,
    pub mmio_accesses: u64,
    pub irq_deliveries: u64,
    pub instruction_timer_period: u64,
    pub virtual_ticks: u32,
    pub input_delivered: u64,
    pub native_manager_count: u32,
    pub native_manager_callers: Vec<u32>,
    pub bootstrap_objects: BTreeMap<String, u32>,
    pub bootstrap_callbacks: BTreeMap<String, u64>,
    pub bootstrap_threads: BTreeMap<String, BootstrapThread>,
    pub graphics_lock_depth: u32,
    pub graphics_lock_owner: u32,
    pub rear_enumeration_flags: u32,
    pub rear_enumeration_pending: bool,
    pub hardware_enabled: bool,
    pub framebuffer: Option<[usize; 2]>,
    pub hardware: Option<crate::hardware::Summary>,
    pub native_hardware_checks: BTreeMap<String, bool>,
    pub native_warnings: Vec<String>,
    pub fans: Vec<FanState>,
    pub ui_ready_at: Option<u64>,
    pub cooling_init_result: Option<u32>,
    pub firmware_notifications: Vec<String>,
    pub notification_overflow: u64,
    pub unclassified_mmio: Vec<MmioObservation>,
    pub unclassified_mmio_overflow: u64,
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
    slice_end: u64,
    idle_instructions: u64,
    ui_ready_at: Option<u64>,
    idle_loop_verified: bool,
    initialization_returns: BTreeMap<u32, (String, u32)>,
    native_checks: BTreeMap<String, bool>,
    last_irq: u64,
    pages: BTreeSet<u32>,
    accesses: u64,
    unclassified_mmio: BTreeMap<(u32, bool), MmioObservation>,
    unclassified_mmio_overflow: u64,
    boot: Boot,
    irq: Interrupts,
    hardware: Option<crate::hardware::Hardware>,
    board_i2c: Option<crate::i2c::I2c>,
    native_warnings: BTreeMap<String, String>,
    uart_line: Vec<u8>,
    firmware_notifications: BTreeSet<String>,
    notification_overflow: u64,
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
        let mut irq = Interrupts::new(if config.hardware { 10_000 } else { 1000 });
        irq.external_i2c = config.hardware || config.offline;
        let mut state = Box::new(State {
            api: &*handle.api,
            instructions: 0,
            slice_end: u64::MAX,
            idle_instructions: 0,
            ui_ready_at: None,
            idle_loop_verified: hash == EXPECTED_SHA256
                && data.get(0x1104c..0x11064)
                    == Some(
                        [
                            0xe30725fcu32,
                            0xe3482110,
                            0xe5921000,
                            0xe2811001,
                            0xe5821000,
                            0xeafffff9,
                        ]
                        .iter()
                        .flat_map(|word| word.to_le_bytes())
                        .collect::<Vec<_>>()
                        .as_slice(),
                    ),
            initialization_returns: BTreeMap::new(),
            native_checks: BTreeMap::new(),
            last_irq: 0,
            pages: BTreeSet::new(),
            accesses: 0,
            unclassified_mmio: BTreeMap::new(),
            unclassified_mmio_overflow: 0,
            boot: Boot::default(),
            irq,
            hardware,
            board_i2c: (config.offline && !config.hardware).then(crate::i2c::I2c::new),
            native_warnings: BTreeMap::new(),
            uart_line: Vec::new(),
            firmware_notifications: BTreeSet::new(),
            notification_overflow: 0,
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
        // Unicorn's instruction-count limiter adds an instruction hook internally.
        // The existing block callback can enforce our approximate budget instead.
        self.state.slice_end = if count == 0 {
            u64::MAX
        } else {
            self.state.instructions.saturating_add(count as u64)
        };
        if let Err(e) = cpu.start(pc, 0, micros) {
            self.state.reason = Some("Erreur processeur".into());
            self.state.fault = Some(e.clone());
            return Err(e);
        }
        if self.state.config.offline {
            self.state.offline.queue_graphics_bootstrap(
                cpu,
                &mut self.state.input,
                self.state.config.hardware,
            );
        }
        // A busy RTOS can render the UI without ever entering its idle loop.
        if self.state.ui_ready_at.is_none()
            && self.state.offline.ui_initialized()
            && self.state.display.dimensions(cpu).is_some()
        {
            self.state.ui_ready_at = Some(self.state.instructions);
        }
        Ok(())
    }

    pub fn instructions(&self) -> u64 {
        self.state.instructions
    }

    pub fn done(&self) -> bool {
        self.state.reason.is_some()
    }

    pub fn input_is_idle(&self) -> bool {
        self.state.input.is_idle()
    }

    pub fn enqueue(&mut self, i: Input) {
        self.state.input.enqueue(i)
    }

    pub fn sensor_frame(&mut self, frame: &Frame) {
        if let Some(hardware) = &mut self.state.hardware {
            hardware.sensor_frame(self.handle.cpu(), frame, &mut self.state.irq);
        }
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
        let framebuffer = state.display.dimensions(cpu);
        let mut registers: Vec<_> = (0..13)
            .map(|i| (format!("r{i}"), cpu.reg(R0 + i)))
            .collect();
        for (name, id) in [("sp", SP), ("lr", LR), ("pc", PC), ("cpsr", CPSR)] {
            registers.push((name.into(), cpu.reg(id)))
        }
        let mut threads = state.offline.callback_threads.clone();
        threads.insert("graphics_lock_owner".into(), cpu.word(0x824d4300));
        threads.insert("rear_io_read_thread".into(), 0x821c357c);
        threads.insert("peg_thread".into(), 0x824d63d8);
        Report {
            rear_enumeration_flags: cpu.word(0x821be7c8),
            rear_enumeration_pending: cpu.word(0x810b18e4) & 255 != 0,
            graphics_lock_depth: cpu.word(0x824d42fc),
            graphics_lock_owner: cpu.word(0x824d4300),
            status: state.reason.clone().unwrap_or_else(|| "Execution".into()),
            error: state.fault.clone(),
            instructions: state.instructions,
            idle_instructions: state.idle_instructions,
            elapsed_seconds: elapsed,
            mips: (state.instructions - state.idle_instructions) as f64 / elapsed.max(0.001) / 1e6,
            pc: cpu.reg(PC),
            firmware_sha256: state.hash.clone(),
            full_camera_boot_verified: false,
            ui_ready: state.offline.ui_initialized() && framebuffer.is_some(),
            registers,
            console: String::from_utf8_lossy(&state.console).into_owned(),
            stages: state.offline.stages.clone(),
            menu: state.offline.menu.clone(),
            controls: state.display.controls(cpu),
            mmio_pages: state.pages.len(),
            mmio_accesses: state.accesses,
            irq_deliveries: state.irq.deliveries.values().sum(),
            instruction_timer_period: state.irq.period,
            virtual_ticks: cpu.word(0x810dab34),
            input_delivered: state.input.delivered,
            native_manager_count: state.offline.manager_count,
            native_manager_callers: state.offline.manager_callers.clone(),
            bootstrap_objects: [
                ("device_manager", 0x824d3e40),
                ("display", 0x825de904),
                ("peg_class", 0x825de908),
                ("peg_queue", 0x825de90c),
                ("manager_task", 0x81198398),
            ]
            .into_iter()
            .map(|(name, address)| (name.into(), cpu.word(address)))
            .chain([
                (
                    "record".into(),
                    cpu.word(cpu.word(0x824d3e40).wrapping_add(0x14)),
                ),
                (
                    "record_class".into(),
                    cpu.word(
                        cpu.word(cpu.word(0x824d3e40).wrapping_add(0x14))
                            .wrapping_add(0x34),
                    ),
                ),
                ("gate".into(), u32::from(state.offline.gate)),
                ("controls_queued".into(), u32::from(state.offline.controls)),
                ("monitor_queued".into(), u32::from(state.offline.monitor)),
            ])
            .collect(),
            bootstrap_callbacks: state.offline.callback_counts.clone(),
            bootstrap_threads: threads
                .iter()
                .filter(|(_, address)| cpu.word(**address) == 0x54485244)
                .map(|(name, address)| {
                    let stack = cpu.word(address + 8);
                    let bytes = cpu.read(stack, 256).unwrap_or_default();
                    (
                        name.clone(),
                        BootstrapThread {
                            address: *address,
                            name: Storage::text(cpu, cpu.word(address + 0x28)).unwrap_or_default(),
                            priority: cpu.word(address + 0xa8),
                            state: cpu.word(address + 0x30),
                            saved_stack: bytes
                                .chunks_exact(4)
                                .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                                .collect(),
                        },
                    )
                })
                .collect(),
            hardware_enabled: state.config.hardware,
            framebuffer,
            hardware: state.hardware.as_ref().map(|h| h.summary(cpu)),
            native_hardware_checks: state.native_checks.clone(),
            native_warnings: state.native_warnings.values().cloned().collect(),
            cooling_init_result: state.offline.cooling_result,
            fans: [0x810b1210, 0x810b1230]
                .into_iter()
                .map(|address| FanState {
                    installed: cpu.word(address) & 255 != 0,
                    kind: cpu.word(address + 4),
                    rated_rpm: cpu.word(address + 0x10),
                    driver_attached: cpu.word(address + 0x18) != 0,
                })
                .collect(),
            ui_ready_at: state.ui_ready_at,
            firmware_notifications: state.firmware_notifications.iter().cloned().collect(),
            notification_overflow: state.notification_overflow,
            unclassified_mmio: state.unclassified_mmio.values().cloned().collect(),
            unclassified_mmio_overflow: state.unclassified_mmio_overflow,
        }
    }
}

impl State {
    fn on_block(&mut self, cpu: Cpu, pc: u32, size: u32) {
        // Stop before this block executes; account for it on the next slice.
        if self.instructions >= self.slice_end {
            cpu.stop();
            return;
        }
        self.instructions += (size / 4).max(1) as u64;
        self.observe_initialization(cpu, pc);
        if pc == 0x806ce8a4 && cpu.reg(R0) >= 1 && self.native_warnings.len() < 128 {
            if let Ok(message) = Storage::text(cpu, cpu.word(cpu.reg(SP) + 8)) {
                let site = format!("{:08x}: {message}", cpu.reg(LR));
                self.native_warnings.entry(site.clone()).or_insert_with(|| {
                    let arguments: Vec<_> = (0..6)
                        .map(|i| {
                            let value = cpu.word(cpu.reg(SP) + 12 + i * 4);
                            if let Ok(text) = Storage::text(cpu, value) {
                                format!("0x{value:08x}={text:?}")
                            } else {
                                format!("0x{value:08x}")
                            }
                        })
                        .collect();
                    format!(
                        "{site} at {} instructions; args={}",
                        self.instructions,
                        arguments.join(", ")
                    )
                });
            }
        }

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
        self.coalesce_idle(cpu, pc);
        self.observe_boot_stage(pc);
        self.tick_interrupts(cpu, pc);
    }

    fn coalesce_idle(&mut self, cpu: Cpu, pc: u32) {
        // Exact six-instruction RED idle loop: MOVW/MOVT r2, LDR/ADD/STR r1, B.
        // It only increments cpu_usage's counter. Preserve the final register state,
        // and never cross a timer deadline, a DMA event or the caller's slice budget.
        if pc != 0x8001104c
            || !self.idle_loop_verified
            || !self.irq.running
            || self.irq.active.is_some()
            || !self.input.is_idle()
            || self
                .hardware
                .as_ref()
                .is_some_and(|hardware| hardware.busy_dma())
            || self.irq.next_irq().is_some()
            || cpu.reg(CPSR) & 0xa0 != 0
        {
            return;
        }
        let horizon = self.irq.next.min(self.slice_end);
        let loops = horizon.saturating_sub(self.instructions) / 6;
        if loops == 0 {
            return;
        }
        let counter = cpu.word(0x811075fc).wrapping_add(loops as u32);
        cpu.put(0x811075fc, counter);
        cpu.set(R0 + 1, counter);
        cpu.set(R0 + 2, 0x811075fc);
        self.instructions += loops * 6;
        self.idle_instructions += loops * 6;
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
                (0x80680c80, "power sequencers", 0),
                (0x80583d14, "LED controllers", 1),
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
        let boot_known = answer.is_some() || [0x48088000, 0x48004000].contains(&(address & !0xfff));
        let mut modeled = self.hardware.as_mut().and_then(|h| {
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
        if let Some(bus) = crate::i2c::BASES
            .iter()
            .position(|base| address & !0xfff == *base)
        {
            if let Some(i2c) = &mut self.board_i2c {
                modeled = i2c
                    .access(
                        bus,
                        write,
                        address & 0xfff,
                        size,
                        value,
                        self.instructions,
                        &mut self.irq,
                    )
                    .map(Some);
                // Writes are also handled, even when no value is returned.
                if write {
                    modeled = Some(None);
                }
            }
        }
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
            } else if write && o == 0 && value & 255 != 0 && cpu.word(p + 0xc) & 0x80 == 0 {
                if self.console.len() < 65536 {
                    self.console.push(value as u8);
                }
                let byte = value as u8;
                if byte == b'\r' || byte == b'\n' {
                    let line = String::from_utf8_lossy(&self.uart_line);
                    // RCP clear notifications have only the numeric ID and an empty payload.
                    if let Some((_, payload)) = line.split_once(":NOTIFY:") {
                        if let Some((_, body)) = payload.split_once(':') {
                            if body != ":" && !body.is_empty() {
                                if self.firmware_notifications.len() < 128 {
                                    self.firmware_notifications.insert(line.into_owned());
                                } else {
                                    self.notification_overflow += 1;
                                }
                            }
                        }
                    }
                    self.uart_line.clear();
                } else if self.uart_line.len() < 4096 {
                    self.uart_line.push(byte);
                } else {
                    self.notification_overflow += 1;
                }
            }
        }
        let irq_known = [
            0x48200000, 0x49032000, 0x48070000, 0x48072000, 0x48060000, 0x4806a000, 0x4806c000,
            0x49020000,
        ]
        .contains(&p);
        if modeled.is_none() && !boot_known && !irq_known {
            let key = (address, write);
            if let Some(observation) = self.unclassified_mmio.get_mut(&key) {
                observation.hits += 1;
            } else if self.unclassified_mmio.len() < 64 {
                self.unclassified_mmio.insert(
                    key,
                    MmioObservation {
                        address,
                        write,
                        size,
                        pc: cpu.reg(PC),
                        value,
                        hits: 1,
                    },
                );
            } else {
                self.unclassified_mmio_overflow += 1;
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
    fn graphics_callback_releases_its_own_locks_without_inheriting_another_owner() {
        let engine = fixture(&[0xe12fff1e]);
        let cpu = engine.handle.cpu();
        let thread = 0x80001000;
        cpu.put(0x810da120, thread);
        cpu.put(0x824d42fc, 3);
        cpu.put(0x824d4300, thread + 0x100);
        cpu.set(LR, 0x80003000);
        let mut offline = Offline::default();
        offline.monitor = true;
        assert!(offline.dispatch(cpu, 0x4020f200, 0, true, None));
        assert_eq!(cpu.reg(LR), 0x4020f204);
        assert_eq!(cpu.word(0x824d42fc), 3);
        // The native callback later acquires the semaphore after the other task.
        cpu.put(0x824d4300, thread);
        cpu.put(0x824d42fc, 1);
        assert!(offline.dispatch(cpu, 0x4020f204, 0, true, None));
        assert_eq!(cpu.reg(PC), 0x80a9e0d0); // original PEG unlock, not a forced write
        assert_eq!(cpu.word(0x824d42fc), 1);
        // Model the unlock's return and ensure the callback then returns normally.
        cpu.put(0x824d42fc, 0);
        assert!(offline.dispatch(cpu, 0x4020f204, 0, true, None));
        assert_eq!(cpu.reg(PC), 0x80003000);
        assert_eq!(offline.callback_counts["monitor_completed"], 1);
    }

    #[test]
    fn notification_history_survives_console_limit_and_ignores_clear_packets() {
        let mut engine = fixture(&[0xe12fff1e]);
        let cpu = engine.handle.cpu();
        let uart = 0x49020000;
        cpu.map(uart, 4096).unwrap();
        engine.state.pages.insert(uart);
        let mut stream = b"diagnostic line\r\n".repeat(5000);
        stream.extend_from_slice(b"#$DRAGON:C:NOTIFY:id:\r\n");
        stream.extend_from_slice(b"#$DRAGON:C:NOTIFY:id:Fan:Stopped:0:58:\r\n");
        for byte in stream {
            engine.state.on_memory(cpu, true, uart, 1, u32::from(byte));
        }
        assert_eq!(engine.state.console.len(), 65536);
        let report = engine.report();
        assert_eq!(
            report.firmware_notifications,
            ["#$DRAGON:C:NOTIFY:id:Fan:Stopped:0:58:"]
        );
        assert_eq!(report.notification_overflow, 0);
    }

    #[test]
    fn idle_coalescing_preserves_counter_registers_flags_and_slice_boundary() {
        let loop_words = [
            0xe30725fc, 0xe3482110, 0xe5921000, 0xe2811001, 0xe5821000, 0xeafffff9,
        ];
        let mut snapshots = Vec::new();
        for accelerated in [false, true] {
            let mut engine = fixture(&[0xe12fff1e]);
            let cpu = engine.handle.cpu();
            cpu.bytes(0x8001104c, &loop_words);
            cpu.set(PC, 0x8001104c);
            cpu.set(CPSR, 0xa0000013);
            engine.state.idle_loop_verified = accelerated;
            engine.state.irq.running = true;
            engine.state.irq.next = 6000;
            engine.run_slice(6000, 0).unwrap();
            let cpu = engine.handle.cpu();
            snapshots.push((
                cpu.word(0x811075fc),
                cpu.reg(R0 + 1),
                cpu.reg(R0 + 2),
                cpu.reg(CPSR),
                cpu.reg(PC),
                engine.instructions(),
            ));
            assert_eq!(engine.state.idle_instructions > 0, accelerated);
        }
        assert_eq!(snapshots[0], snapshots[1]);
        assert_eq!(snapshots[1].0, 1000);
        assert_eq!(snapshots[1].5, 6000);
    }

    #[test]
    fn block_budget_resumes_without_skipping_guest_instructions() {
        // add r0,r0,#1; b back to add. Every slice must retain the loop's state.
        let mut engine = fixture(&[0xe2800001, 0xeafffffd]);
        for slice in 1..=3 {
            engine.run_slice(200, 0).unwrap();
            assert!(!engine.done());
            assert_eq!(engine.instructions(), slice * 200);
            assert_eq!(engine.handle.cpu().reg(R0), slice as u32 * 100);
        }
    }

    #[test]
    fn native_ccdc_registers_capture_raw_rows_raise_irq_and_reject_invalid_dma() {
        let registers = [
            (0x480bc00c, 0x80000100),
            (0x480bc608, (1 << 17) | (1 << 11) | (7 << 8)),
            (0x480bc614, 7), // eight RAW8 pixels per row
            (0x480bc61c, 3), // four rows
            (0x480bc624, 32),
            (0x480bc62c, 0x83000000),
            (0x480bc604, 1),
        ];
        // Guest ARM performs the MMIO writes; literal pool follows its return.
        let code_words = registers.len() * 3 + 1;
        let mut words = Vec::new();
        for (index, _) in registers.iter().enumerate() {
            let base = index * 12;
            let literal = (code_words + index * 2) * 4;
            words.extend([
                0xe59f0000 | (literal - base - 8) as u32,
                0xe59f1000 | (literal + 4 - base - 12) as u32,
                0xe5801000,
            ]);
        }
        words.push(0xe12fff1e);
        for (address, value) in registers {
            words.extend([address, value]);
        }
        let mut engine = fixture(&words);
        engine.handle.cpu().write(0x83000000, &[0xaa; 128]).unwrap();
        engine.run_slice(1000, 100000).unwrap();
        let source = Frame {
            width: 8,
            height: 4,
            rgba: vec![255; 128],
            serial: 1,
        };
        engine.sensor_frame(&source);
        assert!(engine.state.irq.levels.contains(&24));
        let memory = engine.handle.cpu().read(0x83000000, 128).unwrap();
        for row in memory.chunks_exact(32) {
            assert_eq!(&row[..8], &[255; 8]);
            assert!(row[8..].iter().all(|value| *value == 0xaa));
        }
        engine.sensor_frame(&source);
        assert_eq!(engine.report().hardware.unwrap().capture.overruns, 1);
        let cpu = engine.handle.cpu();
        let hardware = engine.state.hardware.as_mut().unwrap();
        hardware.access(
            cpu,
            true,
            0x480bc010,
            4,
            0x80000100,
            0,
            &mut engine.state.irq,
        );
        assert!(!engine.state.irq.levels.contains(&24));
        hardware.access(cpu, true, 0x480bc62c, 4, 0x1000, 0, &mut engine.state.irq);
        engine.sensor_frame(&source);
        let capture = engine.report().hardware.unwrap().capture;
        assert_eq!(capture.frames, 1);
        assert_eq!(capture.bytes, 32);
        assert_eq!(capture.rejected, 1);
        assert!(capture.last_error.unwrap().contains("destination"));
    }

    #[test]
    fn genlock_spi_round_trip_through_omap_registers() {
        let engine = fixture(&[0xe12fff1e]);
        let cpu = engine.handle.cpu();
        let mut hardware = crate::hardware::Hardware::new();
        let mut irq = Interrupts::new(1000);
        let config = 0x480b8040; // McSPI3 channel 1, 16-bit words.
        let mut access =
            |write, address, value| hardware.access(cpu, write, address, 4, value, 0, &mut irq);
        access(true, config, (15 << 7) | (1 << 20) | (2 << 12));
        access(true, config + 12, 3);
        access(true, config + 12, 0xbabe);
        access(true, config, 15 << 7); // deassert chip select
        access(true, config, (15 << 7) | (1 << 20));
        for (word, expected) in [(0x8003, 0), (0, 0), (0, 0x5a5a), (0, 0xbabe)] {
            access(true, config + 12, word);
            assert_eq!(access(false, config + 16, 0), Some(Some(expected)));
        }
        assert_eq!(hardware.summary(cpu).genlock_fpga.register_writes, 1);
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
    fn native_arm_can_select_the_iob_bank_and_read_sata_capabilities() {
        let mut engine = fixture(&[
            0xe59f0010, 0xe3a01014, 0xe5801000, 0xe59f2008, 0xe5923000, 0xe12fff1e, 0x18000018,
            0x1c04000c,
        ]);
        engine.run_slice(1000, 100000).unwrap();
        assert!(engine.done());
        assert_eq!(engine.handle.cpu().reg(R0 + 3), 1);
        assert_eq!(engine.report().hardware.unwrap().iob.bank, 0x14);
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
    fn monitor_bootstrap_can_create_the_first_screen_before_controls_exist() {
        let mut e = fixture(&[0xeafffffe]);
        let cpu = e.handle.cpu();
        cpu.put(0x824d3e40, 0x83460000);
        cpu.put(0x825de904, 0x83461000);
        cpu.put(0x83460014, 0);
        e.state.offline.gate = true;
        e.state
            .offline
            .queue_graphics_bootstrap(cpu, &mut e.state.input, true);
        assert!(!e.state.offline.monitor);
        cpu.put(0x821be7c8, 1);
        e.state
            .offline
            .queue_graphics_bootstrap(cpu, &mut e.state.input, true);
        assert!(e.state.offline.monitor);
        assert!(!e.state.offline.controls);
        assert!(!e.state.input.is_idle());
    }

    #[test]
    fn input_wrapper_preserves_live_task_registers_stack_and_flags() {
        assert_wrapper_preserves_context(false);
    }

    #[test]
    fn bootstrap_callback_preserves_busy_task_without_an_idle_loop() {
        assert_wrapper_preserves_context(true);
    }

    fn assert_wrapper_preserves_context(callback: bool) {
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
            if callback { 0x8007fbfc } else { 0x80a9e148 },
            &[
                0xe59fc01c, 0xe58c0000, 0xe3a00001, 0xe3a01020, 0xe3a02030, 0xe3a03040, 0xe3a0c050,
                0xe3500002, 0xe12fff1e, 0x83460000,
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
        if callback {
            // Constructors must be callable before the PEG thread/queue exists.
            cpu.put(0x824d63d8, 0);
            cpu.put(0x825de90c, 0);
            assert!(e.state.input.enqueue_callback(0x4020f300));
        } else {
            e.enqueue(Input::Down(160, 85));
        }
        e.run_slice(200, 100000).unwrap();
        let cpu = e.handle.cpu();
        assert_eq!(cpu.reg(R0), 8);
        assert_eq!(cpu.reg(R0 + 1), 4);
        assert_eq!(cpu.reg(R0 + 2), 12);
        assert_eq!(cpu.reg(R0 + 3), 13);
        assert_eq!(cpu.reg(R0 + 12), 22);
        assert_eq!(cpu.reg(SP), 0x8343fff0);
        assert_eq!(cpu.reg(CPSR) & 0xf0000, 0xf0000);
        assert_eq!(e.state.input.delivered, u64::from(!callback));
        assert_eq!(
            cpu.word(0x83460000),
            if callback { 0x4020f300 } else { 0x83450000 }
        );
    }
}
