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
pub(crate) struct Selection {
    conn: std::sync::Arc<RustConnection>,
    pub(crate) window: Window,
    atoms: Atoms,
    backend: SharedBackend,
    revision: u64,
    owner: Window,
    pending: bool,
    request_started: Instant,
    receive: Option<Vec<u8>>,
    send: HashMap<(Window, Atom), Send>,
    timestamp: Timestamp,
}
impl Selection {
    pub fn new(
        conn: std::sync::Arc<RustConnection>,
        screen: usize,
        backend: SharedBackend,
    ) -> Result<Self> {
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
            conn,
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
        };
        bridge.request(owner)?;
        Ok(bridge)
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
    pub(crate) fn maintain(&mut self) -> Result<()> {
        if (self.pending || self.receive.is_some())
            && self.request_started.elapsed() >= Duration::from_secs(30)
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
        self.send
            .retain(|_, send| send.started.elapsed() < Duration::from_secs(30));
        Ok(())
    }
    pub(crate) fn handle(&mut self, event: Event) -> Result<()> {
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
                    return Ok(());
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
                    && (p.type_ == self.atoms.UTF8_STRING || p.type_ == u32::from(AtomEnum::STRING))
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
                        return Ok(());
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
        Ok(())
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let receive = (self.pending || self.receive.is_some())
            .then_some(self.request_started + Duration::from_secs(30));
        self.send
            .values()
            .map(|send| send.started + Duration::from_secs(30))
            .chain(receive)
            .min()
    }
}

/// Compatibility name for callers that used the former combined bridge.
pub use crate::x11_session::X11Session as SelectionBridge;
