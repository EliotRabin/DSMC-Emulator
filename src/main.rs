#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod app;
mod devices;
mod display;
mod engine;
mod hardware;
mod i2c;
mod input;
mod nand;
mod offline;
mod onewire;
mod storage;
mod unicorn;
mod worker;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--headless" || a == "--self-test") {
        if let Err(e) = headless(&args) {
            eprintln!("{e}");
            std::process::exit(1)
        }
        return;
    }
    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_inner_size([1360., 920.])
            .with_min_inner_size([1050., 740.]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "DSMC Emulator",
        native,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    ) {
        eprintln!("{e}");
    }
}

fn headless(args: &[String]) -> Result<(), String> {
    let value = |key: &str| {
        args.iter()
            .position(|s| s == key)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--self-test") {
        return self_test();
    }
    let firmware = PathBuf::from(value("--firmware").ok_or("--headless requires --firmware PATH")?);
    let mut outputs = std::collections::BTreeSet::new();
    for key in ["--report", "--frame"] {
        if let Some(p) = value(key) {
            let key = validate_output(std::path::Path::new(&p), &firmware)?;
            if !outputs.insert(key) {
                return Err(
                    "Les chemins du rapport et de la capture doivent etre differents".into(),
                );
            }
        }
    }
    let assets = value("--assets")
        .map(PathBuf::from)
        .unwrap_or_else(|| firmware.parent().unwrap().to_path_buf());
    let check_hardware = args.iter().any(|s| s == "--check-hardware");
    let offline = !check_hardware && !args.iter().any(|s| s == "--raw");
    let config = engine::Config {
        firmware,
        assets,
        offline,
        hardware: check_hardware || args.iter().any(|s| s == "--hardware"),
        virtual_storage: offline || check_hardware,
        allow_unknown: args.iter().any(|s| s == "--allow-unknown"),
    };
    let mut machine = engine::Engine::new(config)?;
    let max: u64 = value("--steps")
        .unwrap_or_else(|| "600000000".into())
        .parse()
        .map_err(|_| "invalid --steps")?;
    let timeout: u64 = value("--timeout")
        .unwrap_or_else(|| "180".into())
        .parse()
        .map_err(|_| "invalid --timeout")?;
    let check_ui = args.iter().any(|s| s == "--check-ui");
    let mut phase = 0;
    let mut next_action = 0;
    let mut ack = std::collections::BTreeSet::new();
    let start = Instant::now();
    let mut last = Instant::now();
    while !machine.done()
        && machine.report().instructions < max
        && start.elapsed() < Duration::from_secs(timeout)
    {
        machine.run_slice(2_000_000, 100_000)?;
        if last.elapsed() > Duration::from_secs(5) {
            let r = machine.report();
            eprintln!(
                "{} M / {:.2} M instr/s / pc={:08x} / {}",
                r.instructions / 1_000_000,
                r.mips,
                r.pc,
                r.stages.last().cloned().unwrap_or_default()
            );
            last = Instant::now();
        }
        if check_hardware {
            let r = machine.report();
            let boards = r.hardware.as_ref().is_some_and(|h| {
                let indexes: std::collections::BTreeSet<_> = [0, 1, 2, 3, 4, 6, 7, 8, 9]
                    .into_iter()
                    .filter_map(|i| h.native_board_index.get(i).copied())
                    .collect();
                indexes.len() == 9 && indexes.iter().all(|v| *v < 9)
            });
            if r.native_hardware_checks.len() == 3
                && r.native_hardware_checks.values().all(|v| *v)
                && boards
            {
                phase = 5;
                eprintln!("Native hardware initialization and nine virtual boards: OK");
                break;
            }
        }
        if check_ui {
            let r = machine.report();
            ack.retain(|a| {
                r.controls
                    .iter()
                    .any(|c| c.label == "OK" && c.address == *a)
            });
            if r.instructions >= next_action {
                if let Some(ok) = r
                    .controls
                    .iter()
                    .find(|b| b.label == "OK" && !ack.contains(&b.address))
                {
                    let [x, y, right, bottom] = ok.rect;
                    machine.enqueue(input::Input::Down((x + right) / 2, (y + bottom) / 2));
                    machine.enqueue(input::Input::Up((x + right) / 2, (y + bottom) / 2));
                    ack.insert(ok.address);
                    next_action = r.instructions + 2_000_000;
                } else {
                    let has = |name: &str| r.menu.iter().any(|m| m.label == name);
                    let label = match phase {
                        1 if has("Settings") && has("Media") => Some("Settings"),
                        2 if has("Display") && has("Look") => Some("Display"),
                        2 if has("Settings") => Some("Settings"),
                        3 if has("Display") => Some("Display"),
                        _ => None,
                    };
                    if let Some(label) = label {
                        if let Some(button) = r.controls.iter().find(|b| b.label == label) {
                            let [x, y, right, bottom] = button.rect;
                            machine.enqueue(input::Input::Down((x + right) / 2, (y + bottom) / 2));
                            machine.enqueue(input::Input::Up((x + right) / 2, (y + bottom) / 2));
                            phase = if label == "Settings" { 2 } else { 3 };
                            eprintln!("Clicked native {label}, phase {phase}");
                            next_action = r.instructions + 8_000_000;
                        }
                    } else if phase == 0
                        && r.framebuffer.is_some()
                        && r.instructions
                            > if r.hardware_enabled {
                                480_000_000
                            } else {
                                260_000_000
                            }
                    {
                        machine.enqueue(input::Input::Menu);
                        phase = 1;
                        next_action = r.instructions + 2_000_000;
                    } else if phase == 3 && has("Tools") && has("Zebra") {
                        machine.enqueue(input::Input::KeyDown(0x295));
                        machine.enqueue(input::Input::KeyUp(0x295));
                        phase = 4;
                        next_action = r.instructions + 2_000_000;
                    } else if phase == 4 && has("Display") && has("Look") {
                        eprintln!("Native UI check passed: Menu -> Settings -> Display -> Escape");
                        phase = 5;
                        break;
                    }
                }
            }
        }
        if !check_ui && args.iter().any(|s| s == "--until-frame") && machine.frame().is_some() {
            break;
        }
    }
    let report = machine.report();
    if let Some(p) = value("--frame") {
        if let Some(f) = machine.frame() {
            let mut b = format!("P6\n{} {}\n255\n", f.width, f.height).into_bytes();
            for p in f.rgba.chunks_exact(4) {
                b.extend(&p[..3]);
            }
            std::fs::write(p, b).map_err(|e| e.to_string())?;
        }
    }
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    if let Some(p) = value("--report") {
        std::fs::write(p, &json).map_err(|e| e.to_string())?;
    }
    println!("{json}");
    if (check_ui || check_hardware) && phase != 5 {
        return Err(format!("Native check incomplete (phase {phase})"));
    }
    if let Some(e) = report.error {
        return Err(e);
    }
    Ok(())
}

fn self_test() -> Result<(), String> {
    let h = unicorn::Handle::new()?;
    let c = h.cpu();
    c.cortex_a8()?;
    c.map(0x80000000, 4096)?;
    c.bytes(0x80000000, &[0xe3a0002a, 0xeafffffe]);
    c.set(unicorn::CPSR, 0xd3);
    c.start(0x80000000, 1, 0)?;
    if c.reg(unicorn::R0) != 42 {
        return Err("ARM fixture failed".into());
    }
    c.set_cp(12, 0, 0, 0, 0x80000000)?;
    if c.cp(12, 0, 0, 0) != 0x80000000 {
        return Err("VBAR fixture failed".into());
    }
    println!("Unicorn ARM Cortex-A8: OK (r0=42, VBAR=0x80000000)");
    Ok(())
}

fn validate_output(path: &std::path::Path, firmware: &std::path::Path) -> Result<String, String> {
    let resolved = path
        .canonicalize()
        .or_else(|_| {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            Ok::<_, std::io::Error>(
                parent.canonicalize()?.join(
                    path.file_name()
                        .ok_or_else(|| std::io::Error::other("Nom de fichier manquant"))?,
                ),
            )
        })
        .map_err(|e| e.to_string())?;
    if firmware.canonicalize().ok().as_ref() == Some(&resolved) {
        return Err("Le firmware original ne peut pas etre remplace par un export".into());
    }
    let key = resolved.to_string_lossy().into_owned();
    Ok(if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    })
}
