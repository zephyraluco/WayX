//! One-shot X11 readiness; protocol reads remain on the UI thread.
use anyhow::Result;
use x11rb::rust_connection::RustConnection;

#[derive(Default)]
struct ReadinessState {
    generation: usize,
    armed: bool,
}
/// One-shot socket readiness wakes the UI without reading X11 bytes on another thread.
/// The connection is retained until deregistration and watcher shutdown complete.
pub(crate) struct ReadinessWatch {
    poller: std::sync::Arc<polling::Poller>,
    conn: std::sync::Arc<RustConnection>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    failure: std::sync::Arc<std::sync::Mutex<Option<std::io::Error>>>,
    armed: std::sync::Arc<std::sync::Mutex<ReadinessState>>,
}
impl ReadinessWatch {
    pub fn pause(&self) -> std::io::Result<()> {
        let mut armed = self
            .armed
            .lock()
            .map_err(|_| std::io::Error::other("readiness gate poisoned"))?;
        armed.armed = false;
        self.poller
            .modify(self.conn.stream(), polling::Event::none(armed.generation))
    }
    pub fn arm(&self) -> std::io::Result<()> {
        if let Some(error) = self
            .failure
            .lock()
            .map_err(|_| std::io::Error::other("readiness error mutex poisoned"))?
            .take()
        {
            return Err(error);
        }
        let mut armed = self
            .armed
            .lock()
            .map_err(|_| std::io::Error::other("readiness gate poisoned"))?;
        armed.generation = (armed.generation + 1) % (usize::MAX - 1);
        armed.armed = true;
        self.poller.modify(
            self.conn.stream(),
            polling::Event::readable(armed.generation),
        )
    }
}
impl Drop for ReadinessWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = self.poller.notify();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = self.poller.delete(self.conn.stream());
    }
}
impl ReadinessWatch {
    pub(crate) fn new(
        conn: std::sync::Arc<RustConnection>,
        wake: crate::notify::UiWake,
    ) -> Result<Self> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let poller = Arc::new(polling::Poller::new()?);
        // SAFETY: ReadinessWatch retains conn and removes it before dropping it.
        unsafe {
            poller.add(conn.stream(), polling::Event::none(1))?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(std::sync::Mutex::new(None));
        let worker_failure = failure.clone();
        let armed = Arc::new(std::sync::Mutex::new(ReadinessState::default()));
        let worker_armed = armed.clone();
        let worker_poller = poller.clone();
        let worker_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("wayx-x11-ready".into())
            .spawn(move || {
                let mut events = polling::Events::new();
                while !worker_stop.load(Ordering::Acquire) {
                    events.clear();
                    match worker_poller.wait(&mut events, None) {
                        Ok(_) if !events.is_empty() => {
                            if let Ok(mut armed) = worker_armed.lock()
                                && armed.armed
                                && events.iter().any(|event| event.key == armed.generation)
                            {
                                armed.armed = false;
                                wake();
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            if let Ok(mut failure) = worker_failure.lock() {
                                *failure = Some(error);
                            }
                            wake();
                            break;
                        }
                    }
                }
            })?;
        Ok(ReadinessWatch {
            poller,
            conn,
            stop,
            thread: Some(thread),
            failure,
            armed,
        })
    }
}
