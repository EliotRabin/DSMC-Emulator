use crate::{
    display::Frame,
    engine::{Config, Engine, Report},
    input::Input,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct View {
    pub report: Report,
    pub frame: Option<Arc<Frame>>,
    pub running: bool,
    pub paused: bool,
    pub finished: bool,
}

pub enum Command {
    Pause(bool),
    Input(Input),
}

pub struct Worker {
    pub view: Arc<Mutex<View>>,
    tx: SyncSender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(config: Config) -> Self {
        let view = Arc::new(Mutex::new(View {
            running: true,
            ..Default::default()
        }));
        let shared = view.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        let thread = thread::spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut engine = match Engine::new(config) {
                    Ok(e) => e,
                    Err(e) => {
                        let mut v = shared.lock().unwrap_or_else(|e| e.into_inner());
                        v.report.status = "Impossible de demarrer".into();
                        v.report.error = Some(e);
                        return;
                    }
                };
                let mut paused = false;
                let mut published = Instant::now() - Duration::from_secs(1);
                while !quit.load(Ordering::Relaxed) {
                    while let Ok(command) = rx.try_recv() {
                        match command {
                            Command::Pause(p) => paused = p,
                            Command::Input(i) => engine.enqueue(i),
                        }
                    }
                    if !paused {
                        let _ = engine.run_slice(2_000_000, 30_000);
                    } else {
                        thread::sleep(Duration::from_millis(15))
                    }
                    if published.elapsed() >= Duration::from_millis(250) || engine.done() {
                        let frame = engine.frame().map(Arc::new);
                        let report = engine.report();
                        let mut v = shared.lock().unwrap_or_else(|e| e.into_inner());
                        v.report = report;
                        if frame.is_some() {
                            v.frame = frame
                        }
                        v.paused = paused;
                        published = Instant::now();
                    }
                    if engine.done() {
                        break;
                    }
                }
                let mut v = shared.lock().unwrap_or_else(|e| e.into_inner());
                v.report = engine.report();
                if quit.load(Ordering::Relaxed) {
                    v.report.status = "Arrete".into();
                }
            }));
            let mut v = shared.lock().unwrap_or_else(|e| e.into_inner());
            if run.is_err() {
                v.report.status = "Erreur interne".into();
                v.report.error = Some("Le thread d'emulation s'est interrompu".into())
            }
            v.running = false;
            v.finished = true;
        });
        Self {
            view,
            tx,
            stop,
            thread: Some(thread),
        }
    }

    pub fn send(&self, c: Command) -> bool {
        self.tx.try_send(c).is_ok()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(worker: &Worker, check: impl Fn(&View) -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if check(&worker.view.lock().unwrap_or_else(|e| e.into_inner())) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("worker did not reach expected state")
    }

    #[test]
    fn pause_resume_and_stop_preserve_the_live_machine() {
        let path = std::env::temp_dir().join(format!("dsmc-worker-{}.mem", std::process::id()));
        std::fs::write(&path, 0xeafffffeu32.to_le_bytes()).unwrap();
        let mut w = Worker::start(Config {
            firmware: path.clone(),
            assets: std::env::temp_dir(),
            offline: false,
            virtual_storage: false,
            hardware: false,
            allow_unknown: true,
        });
        wait(&w, |v| v.report.instructions > 0);
        assert!(w.send(Command::Pause(true)));
        wait(&w, |v| v.paused);
        let n = w.view.lock().unwrap().report.instructions;
        thread::sleep(Duration::from_millis(350));
        assert_eq!(w.view.lock().unwrap().report.instructions, n);
        assert!(w.send(Command::Pause(false)));
        wait(&w, |v| v.report.instructions > n);
        w.stop();
        assert!(!w.view.lock().unwrap().running);
        std::fs::remove_file(path).unwrap();
    }
}
