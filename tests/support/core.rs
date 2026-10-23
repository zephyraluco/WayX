//! Own the embedded core's ABI arguments, library lifetime and bounded shutdown.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{CString, c_char},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use wayx::{
    backend::{InputEvent, event},
    bridge::{Api, Handle, SharedBackend},
};

type RunCore = unsafe extern "C" fn(*const Api, *mut Handle, i32, *mut *mut c_char) -> i32;

pub struct CoreServer {
    backend: SharedBackend,
    worker: Option<JoinHandle<Result<i32>>>,
}

impl CoreServer {
    pub fn start(backend: SharedBackend, api: Api, arguments: &[&str]) -> Result<Self> {
        let path = std::env::var_os("WAYX_TEST_CORE").context("Set WAYX_TEST_CORE")?;
        let arguments = arguments
            .iter()
            .map(|argument| CString::new(*argument))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let shared = backend.clone();
        let worker = thread::spawn(move || {
            // SAFETY: tests select the trusted core; its library, API, handle and
            // NUL-terminated arguments remain alive until the blocking run returns.
            let library = unsafe { libloading::Library::new(path) }?;
            let run = unsafe { library.get::<RunCore>(b"wayx_xserver_run\0") }?;
            let mut argv: Vec<_> = arguments.iter().map(|s| s.as_ptr().cast_mut()).collect();
            argv.push(std::ptr::null_mut());
            let mut handle = Handle(shared);
            Ok(unsafe { run(&api, &mut handle, arguments.len() as i32, argv.as_mut_ptr()) })
        });
        Ok(Self {
            backend,
            worker: Some(worker),
        })
    }

    pub fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    pub fn finish(mut self) -> Result<()> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<()> {
        self.backend
            .lock()
            .map_err(|_| anyhow::anyhow!("Core backend lock poisoned"))?
            .push_input(InputEvent {
                kind: event::SHUTDOWN,
                ..Default::default()
            })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.is_running() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let worker = self.worker.take().context("Core worker already joined")?;
        ensure!(
            worker.is_finished(),
            "XLibre did not shut down within 5 seconds"
        );
        let status = worker
            .join()
            .map_err(|_| anyhow::anyhow!("XLibre worker panicked"))??;
        ensure!(status == 0, "XLibre returned failure: {status}");
        Ok(())
    }
}

impl Drop for CoreServer {
    fn drop(&mut self) {
        if self.worker.is_some()
            && let Err(error) = self.shutdown()
        {
            eprintln!("XLibre cleanup failed: {error:#}");
        }
    }
}
