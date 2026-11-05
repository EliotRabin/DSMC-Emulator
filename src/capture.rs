//! Virtual sensor and PC recorder, independent of ARM execution and firmware UI pixels.

use crate::{
    avi::{AviWriter, jpeg},
    display::Frame,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;
pub const FPS: u32 = 24;

/// Moving bars and a moving marker make frozen frames and ordering visible.
pub fn test_pattern(width: usize, height: usize, serial: u64) -> Frame {
    let colors = [
        [235, 235, 235],
        [235, 235, 16],
        [16, 235, 235],
        [16, 235, 16],
        [235, 16, 235],
        [235, 16, 16],
        [16, 16, 235],
        [16, 16, 16],
    ];
    let mut rgba = vec![0; width * height * 4];
    let marker = serial as usize % width.max(1);
    for y in 0..height {
        for x in 0..width {
            let color = if x.abs_diff(marker) < 3 || y == serial as usize % height {
                [255, 255, 255]
            } else if y > height * 3 / 4 {
                let shade = ((x + serial as usize * 3) % width * 255 / width) as u8;
                [shade; 3]
            } else {
                colors[(x * 8 / width).min(7)]
            };
            let offset = (y * width + x) * 4;
            rgba[offset..offset + 3].copy_from_slice(&color);
            rgba[offset + 3] = 255;
        }
    }
    Frame {
        width,
        height,
        rgba,
        serial,
    }
}

#[derive(Clone, Default)]
pub struct Status {
    pub preview: bool,
    pub recording: bool,
    pub generated: u64,
    pub recorded: u64,
    pub bytes: u64,
    pub late_frames: u64,
    pub output: Option<PathBuf>,
    pub snapshot: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct View {
    pub status: Status,
    pub frame: Option<Arc<Frame>>,
}

pub enum Command {
    Preview(bool),
    Record(PathBuf),
    StopRecord,
    Snapshot(PathBuf),
}

type Recording = AviWriter<BufWriter<File>>;

pub struct Capture {
    pub view: Arc<Mutex<View>>,
    tx: SyncSender<Command>,
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn finish(recording: &mut Option<Recording>, status: &mut Status) {
    if let Some(mut writer) = recording.take() {
        status.recorded = writer.frames();
        if let Err(error) = writer.finish() {
            status.error = Some(error.to_string());
        }
        status.bytes = writer.bytes().unwrap_or(status.bytes);
    }
    status.recording = false;
}

fn create(path: &PathBuf) -> io::Result<File> {
    // Never overwrite firmware, assets or an existing clip/snapshot.
    OpenOptions::new().write(true).create_new(true).open(path)
}

impl Capture {
    pub fn new() -> Self {
        Self::spawn(WIDTH, HEIGHT, FPS)
    }

    fn spawn(width: usize, height: usize, fps: u32) -> Self {
        let view = Arc::new(Mutex::new(View::default()));
        let shared = view.clone();
        let quit = Arc::new(AtomicBool::new(false));
        let stop = quit.clone();
        let (tx, rx) = mpsc::sync_channel(8);
        let thread = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut status = Status::default();
                let mut recording: Option<Recording> = None;
                let mut frame: Option<Arc<Frame>> = None;
                let period = Duration::from_secs_f64(1.0 / f64::from(fps));
                let mut next_frame = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    let wait = if status.preview || status.recording {
                        next_frame.saturating_duration_since(Instant::now())
                    } else {
                        Duration::from_millis(100)
                    };
                    if let Ok(command) = rx.recv_timeout(wait) {
                        match command {
                            Command::Preview(enabled) => {
                                if !enabled {
                                    finish(&mut recording, &mut status);
                                }
                                if enabled && !status.preview {
                                    next_frame = Instant::now();
                                }
                                status.preview = enabled;
                            }
                            Command::Record(path) if recording.is_none() => {
                                match create(&path).and_then(|file| {
                                    AviWriter::new(BufWriter::new(file), width, height, fps)
                                }) {
                                    Ok(writer) => {
                                        recording = Some(writer);
                                        status.recording = true;
                                        status.preview = true;
                                        status.recorded = 0;
                                        status.bytes = 0;
                                        status.error = None;
                                        status.output = Some(path);
                                        next_frame = Instant::now();
                                    }
                                    Err(error) => status.error = Some(error.to_string()),
                                }
                            }
                            Command::Record(_) => {}
                            Command::StopRecord => finish(&mut recording, &mut status),
                            Command::Snapshot(path) => {
                                let result = frame
                                    .as_ref()
                                    .ok_or_else(|| {
                                        io::Error::other("Start the sensor preview first")
                                    })
                                    .and_then(|frame| jpeg(frame, &mut Vec::new()))
                                    .and_then(|bytes| {
                                        let mut file = create(&path)?;
                                        file.write_all(&bytes)?;
                                        file.flush()
                                    });
                                match result {
                                    Ok(()) => {
                                        status.snapshot = Some(path);
                                        status.error = None;
                                    }
                                    Err(error) => status.error = Some(error.to_string()),
                                }
                            }
                        }
                    }
                    let now = Instant::now();
                    if (status.preview || status.recording) && now >= next_frame {
                        // Do not silently speed up a recording when disk/encoding cannot keep up.
                        if now.saturating_duration_since(next_frame) > period * 4 {
                            if status.recording {
                                finish(&mut recording, &mut status);
                                status.error = Some(
                                    "Recording stopped: encoder or disk cannot keep up with 24 fps"
                                        .into(),
                                );
                            }
                            status.late_frames += 1;
                            next_frame = now;
                        }
                        status.generated += 1;
                        let captured = Arc::new(test_pattern(width, height, status.generated));
                        if let Some(writer) = &mut recording {
                            let result = writer.push(&captured);
                            status.recorded = writer.frames();
                            status.bytes = writer.bytes().unwrap_or(status.bytes);
                            if let Err(error) = result {
                                finish(&mut recording, &mut status);
                                status.error = Some(error.to_string());
                            }
                        }
                        frame = Some(captured);
                        next_frame += period;
                    }
                    let mut view = shared.lock().unwrap_or_else(|error| error.into_inner());
                    view.status = status.clone();
                    view.frame = frame.clone();
                }
                finish(&mut recording, &mut status);
                shared
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .status = status;
            }));
            if result.is_err() {
                let mut view = shared.lock().unwrap_or_else(|error| error.into_inner());
                view.status.preview = false;
                view.status.recording = false;
                view.status.error = Some("Le module de capture s'est interrompu.".into());
            }
        });
        Self {
            view,
            tx,
            quit,
            thread: Some(thread),
        }
    }

    pub fn send(&self, command: Command) -> bool {
        self.tx.try_send(command).is_ok()
    }

    pub fn stop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Deterministic export for local inspection with a media decoder; no firmware needed.
pub fn export_test_clip(path: PathBuf) -> io::Result<()> {
    let mut writer = AviWriter::new(BufWriter::new(create(&path)?), WIDTH, HEIGHT, FPS)?;
    for serial in 0..FPS {
        writer.push(&test_pattern(WIDTH, HEIGHT, serial as u64))?;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(capture: &Capture, predicate: impl Fn(&View) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if predicate(&capture.view.lock().unwrap()) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "capture did not reach expected state: {:?}",
            capture.view.lock().unwrap().status.error
        );
    }

    #[test]
    fn record_stop_snapshot_and_drop_finalize_files_without_firmware() {
        let stem = std::env::temp_dir().join(format!(
            "dsmc-capture-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let clip = stem.with_extension("avi");
        let snapshot = stem.with_extension("jpg");
        let second = stem.with_extension("second.avi");
        let capture = Capture::spawn(64, 32, 12);
        assert!(capture.send(Command::Record(clip.clone())));
        wait(&capture, |view| view.status.recorded >= 3);
        assert!(capture.send(Command::StopRecord));
        wait(&capture, |view| !view.status.recording);
        assert!(capture.send(Command::Snapshot(snapshot.clone())));
        wait(&capture, |view| {
            view.status.snapshot.as_ref() == Some(&snapshot)
        });
        let image = image::open(&snapshot).unwrap();
        assert_eq!((image.width(), image.height()), (64, 32));
        assert!(capture.send(Command::Record(second.clone())));
        wait(&capture, |view| {
            view.status.recording && view.status.recorded >= 2
        });
        drop(capture);
        for path in [&clip, &second] {
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
                bytes.len() - 8
            );
            assert!(bytes.windows(4).any(|bytes| bytes == b"idx1"));
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_file(snapshot).unwrap();
    }

    #[test]
    fn existing_output_is_never_truncated() {
        let path = std::env::temp_dir().join(format!("dsmc-preserve-{}.avi", std::process::id()));
        std::fs::write(&path, b"existing clip").unwrap();
        assert!(create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing clip");
        std::fs::remove_file(path).unwrap();
    }
}
