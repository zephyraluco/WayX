//! ICCCM CLIPBOARD adapter. XLibre remains responsible for Selection protocol.
//! Supports UTF8_STRING, STRING, TARGETS, TIMESTAMP and bounded INCR transfers.
use crate::bridge::SharedBackend;
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::*,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
const LIMIT: usize = 16 * 1024 * 1024;
const CHUNK: usize = 60 * 1024;
x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        CLIPBOARD, UTF8_STRING, TARGETS, TIMESTAMP, INCR, WAYX_SELECTION,
    }
}
struct Send {
    data: Vec<u8>,
    offset: usize,
    target: Atom,
    started: Instant,
}
pub struct SelectionBridge {
    conn: std::sync::Arc<RustConnection>,
    window: Window,
    atoms: Atoms,
    backend: SharedBackend,
    revision: u64,
    owner: Window,
    pending: bool,
    request_started: Instant,
    receive: Option<Vec<u8>>,
    send: HashMap<(Window, Atom), Send>,
    timestamp: Timestamp,
    queued_event: Option<Event>,
    manager: Option<crate::wm::WindowManager>,
    pub(crate) ime: Option<crate::ime::ImeBridge>,
}
impl SelectionBridge {
    pub fn new(conn: RustConnection, screen: usize, backend: SharedBackend) -> Result<Self> {
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
        let atoms = Atoms::new(&conn)?.reply()?;
        conn.xfixes_query_version(5, 0)?.reply()?;
        conn.xfixes_select_selection_input(
            window,
            atoms.CLIPBOARD,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            atoms.WAYX_SELECTION,
            AtomEnum::STRING,
            &[],
        )?
        .check()?;
        let owner = conn.get_selection_owner(atoms.CLIPBOARD)?.reply()?.owner;
        conn.flush()?;
        let mut bridge = Self {
            conn: std::sync::Arc::new(conn),
            window,
            atoms,
            backend,
            revision: 0,
            owner: 0,
            pending: false,
            request_started: Instant::now(),
            receive: None,
            send: HashMap::new(),
            timestamp: 0,
            queued_event: None,
            manager: None,
            ime: None,
        };
        bridge.request(owner)?;
        Ok(bridge)
    }
    pub fn connect(display: &str, backend: SharedBackend) -> Result<Self> {
        let (conn, screen) = RustConnection::connect(Some(display))?;
        Self::new(conn, screen, backend)
    }
    pub fn connect_local(
        display: u16,
        xauthority: &std::path::Path,
        backend: SharedBackend,
    ) -> Result<Self> {
        ensure!(display <= 59535, "Display number exceeds TCP range");
        let cookie = crate::auth::xauthority_cookie(xauthority, display)?;
        #[cfg(unix)]
        let stream = if let Ok(unix) =
            std::os::unix::net::UnixStream::connect(format!("/tmp/.X11-unix/X{display}"))
        {
            x11rb::rust_connection::DefaultStream::from_unix_stream(unix)?.0
        } else {
            x11rb::rust_connection::DefaultStream::from_tcp_stream(std::net::TcpStream::connect(
                (std::net::Ipv4Addr::LOCALHOST, 6000 + display),
            )?)?
            .0
        };
        #[cfg(not(unix))]
        let stream = x11rb::rust_connection::DefaultStream::from_tcp_stream(
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 6000 + display))?,
        )?
        .0;
        let conn = RustConnection::connect_to_stream_with_auth_info(
            stream,
            0,
            b"MIT-MAGIC-COOKIE-1".to_vec(),
            cookie.to_vec(),
        )?;
        Self::new(conn, 0, backend)
    }
    /// Register the WayX XIM server on the internal connection.
    pub fn enable_ime(&mut self) -> Result<()> {
        self.ime = Some(crate::ime::ImeBridge::new(self.conn.clone(), 0)?);
        Ok(())
    }
    /// Enable the rootless window manager on this internal X11 connection.
    pub fn enable_window_management(&mut self) -> Result<()> {
        self.manager = Some(crate::wm::WindowManager::new(
            &self.conn,
            0,
            self.window,
            self.backend.clone(),
        )?);
        Ok(())
    }
    fn request(&mut self, owner: Window) -> Result<()> {
        self.owner = owner;
        if owner != 0 && owner != self.window && !self.pending && self.receive.is_none() {
            self.conn.convert_selection(
                self.window,
                self.atoms.CLIPBOARD,
                self.atoms.UTF8_STRING,
                self.atoms.WAYX_SELECTION,
                x11rb::CURRENT_TIME,
            )?;
            self.pending = true;
            self.request_started = Instant::now();
        }
        Ok(())
    }
    fn received(&mut self, bytes: &[u8], latin1: bool) -> Result<()> {
        ensure!(bytes.len() <= LIMIT, "X11 clipboard exceeds 16 MiB");
        let text = if latin1 {
            bytes.iter().map(|&b| char::from(b)).collect()
        } else {
            String::from_utf8(bytes.to_vec()).context("X11 clipboard is not UTF-8")?
        };
        let mut backend = self.backend.lock().unwrap();
        backend.set_clipboard(&text)?;
        self.revision = backend.clipboard_revision;
        Ok(())
    }
    fn selection_request(&mut self, e: SelectionRequestEvent) -> Result<()> {
        let property = if e.property == 0 {
            e.target
        } else {
            e.property
        };
        let mut accepted = true;
        if e.selection != self.atoms.CLIPBOARD {
            accepted = false;
        } else if e.target == self.atoms.TARGETS {
            self.conn.change_property32(
                PropMode::REPLACE,
                e.requestor,
                property,
                AtomEnum::ATOM,
                &[
                    self.atoms.TARGETS,
                    self.atoms.TIMESTAMP,
                    self.atoms.UTF8_STRING,
                    AtomEnum::STRING.into(),
                ],
            )?;
        } else if e.target == self.atoms.TIMESTAMP {
            self.conn.change_property32(
                PropMode::REPLACE,
                e.requestor,
                property,
                AtomEnum::INTEGER,
                &[self.timestamp],
            )?;
        } else if e.target == self.atoms.UTF8_STRING || e.target == u32::from(AtomEnum::STRING) {
            let text = self.backend.lock().unwrap().clipboard.clone();
            let bytes: Vec<u8> = if e.target == self.atoms.UTF8_STRING {
                text.into_bytes()
            } else {
                text.chars()
                    .map(|c| u8::try_from(c as u32).unwrap_or(b'?'))
                    .collect()
            };
            if bytes.len() > CHUNK {
                ensure!(
                    self.send.len() < 32,
                    "Too many simultaneous X11 clipboard transfers"
                );
                self.conn.change_window_attributes(
                    e.requestor,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::PROPERTY_CHANGE | EventMask::FOCUS_CHANGE),
                )?;
                self.conn.change_property32(
                    PropMode::REPLACE,
                    e.requestor,
                    property,
                    self.atoms.INCR,
                    &[bytes.len() as u32],
                )?;
                self.send.insert(
                    (e.requestor, property),
                    Send {
                        data: bytes,
                        offset: 0,
                        target: e.target,
                        started: Instant::now(),
                    },
                );
            } else {
                self.conn.change_property8(
                    PropMode::REPLACE,
                    e.requestor,
                    property,
                    e.target,
                    &bytes,
                )?;
            }
        } else {
            accepted = false;
        }
        self.conn.send_event(
            false,
            e.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: e.time,
                requestor: e.requestor,
                selection: e.selection,
                target: e.target,
                property: if accepted { property } else { 0 },
            },
        )?;
        Ok(())
    }
    pub(crate) fn queue_pending_event(&mut self) -> Result<bool> {
        if self.queued_event.is_none() {
            self.queued_event = self.conn.poll_for_event()?;
        }
        Ok(self.queued_event.is_some())
    }
    pub fn tick(&mut self) -> Result<()> {
        if (self.pending || self.receive.is_some())
            && self.request_started.elapsed() > Duration::from_secs(30)
        {
            self.pending = false;
            self.receive = None;
            tracing::warn!("X11 clipboard conversion timed out");
        }
        let revision = self.backend.lock().unwrap().clipboard_revision;
        if revision != self.revision {
            self.conn.set_selection_owner(
                self.window,
                self.atoms.CLIPBOARD,
                x11rb::CURRENT_TIME,
            )?;
            self.revision = revision;
            self.owner = self.window;
        }
        loop {
            let event = match self.queued_event.take() {
                Some(event) => Some(event),
                None => self.conn.poll_for_event()?,
            };
            let Some(event) = event else {
                break;
            };
            if let Some(ime) = &mut self.ime
                && ime.filter_event(&event)?
                && matches!(event, Event::SelectionRequest(_))
            {
                continue;
            }
            if let Some(manager) = &mut self.manager
                && manager.handle(&self.conn, &event)?
            {
                continue;
            }
            match event {
                Event::SelectionRequest(e) => self.selection_request(e)?,
                Event::XfixesSelectionNotify(e) => {
                    if e.owner == self.window {
                        self.timestamp = e.selection_timestamp;
                    }
                    self.request(e.owner)?;
                }
                Event::SelectionClear(_) => {}
                Event::SelectionNotify(e)
                    if e.requestor == self.window && e.selection == self.atoms.CLIPBOARD =>
                {
                    self.pending = false;
                    if e.property == 0 {
                        continue;
                    }
                    let p = self
                        .conn
                        .get_property(
                            true,
                            self.window,
                            e.property,
                            AtomEnum::ANY,
                            0,
                            (LIMIT / 4 + 1) as u32,
                        )?
                        .reply()?;
                    ensure!(
                        p.bytes_after == 0 && p.value.len() <= LIMIT,
                        "X11 clipboard exceeds transfer limit"
                    );
                    if p.type_ == self.atoms.INCR {
                        self.receive = Some(Vec::new());
                        self.conn.delete_property(self.window, e.property)?;
                    } else if p.format == 8
                        && (p.type_ == self.atoms.UTF8_STRING
                            || p.type_ == u32::from(AtomEnum::STRING))
                    {
                        self.received(&p.value, p.type_ == u32::from(AtomEnum::STRING))?;
                    }
                }
                Event::PropertyNotify(e) => {
                    if e.window == self.window
                        && e.atom == self.atoms.WAYX_SELECTION
                        && e.state == Property::NEW_VALUE
                        && self.receive.is_some()
                    {
                        let p = self
                            .conn
                            .get_property(
                                true,
                                self.window,
                                e.atom,
                                AtomEnum::ANY,
                                0,
                                (LIMIT / 4 + 1) as u32,
                            )?
                            .reply()?;
                        if p.type_ == self.atoms.INCR {
                            continue;
                        }
                        ensure!(
                            p.bytes_after == 0
                                && p.format == 8
                                && (p.type_ == self.atoms.UTF8_STRING
                                    || p.type_ == u32::from(AtomEnum::STRING)),
                            "Invalid incremental clipboard chunk"
                        );
                        let data = self.receive.as_mut().unwrap();
                        ensure!(
                            data.len() + p.value.len() <= LIMIT,
                            "Incremental clipboard exceeds limit"
                        );
                        data.extend_from_slice(&p.value);
                        if p.value.is_empty() {
                            let data = self.receive.take().unwrap();
                            self.received(&data, p.type_ == u32::from(AtomEnum::STRING))?;
                        }
                    } else if e.state == Property::DELETE {
                        let key = (e.window, e.atom);
                        if let Some(send) = self.send.get_mut(&key) {
                            let end = (send.offset + CHUNK).min(send.data.len());
                            self.conn.change_property8(
                                PropMode::REPLACE,
                                e.window,
                                e.atom,
                                send.target,
                                &send.data[send.offset..end],
                            )?;
                            let finished = end == send.offset;
                            send.offset = end;
                            if finished {
                                self.send.remove(&key);
                            }
                        }
                    }
                }
                Event::Error(error) => tracing::warn!(?error, "X11 selection request failed"),
                _ => {}
            }
        }
        if let Some(manager) = &mut self.manager {
            manager.tick(&self.conn)?;
        }
        self.send
            .retain(|_, send| send.started.elapsed() < Duration::from_secs(30));
        self.conn.flush()?;
        Ok(())
    }
}

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
impl SelectionBridge {
    pub(crate) fn watch(&self, wake: crate::notify::UiWake) -> Result<ReadinessWatch> {
        ReadinessWatch::new(self.conn.clone(), wake)
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
