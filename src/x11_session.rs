//! One internal X11 connection and event dispatcher for WM, XIM and Selection.
use crate::{bridge::SharedBackend, selection::Selection};
use anyhow::{Result, ensure};
use std::{sync::Arc, time::Instant};
use x11rb::{connection::Connection, protocol::Event, rust_connection::RustConnection};

pub struct X11Session {
    conn: Arc<RustConnection>,
    selection: Selection,
    manager: Option<crate::wm::WindowManager>,
    pub(crate) ime: Option<crate::ime::ImeBridge>,
    backend: SharedBackend,
    screen: usize,
    queued_event: Option<Event>,
}
impl X11Session {
    pub fn new(conn: RustConnection, screen: usize, backend: SharedBackend) -> Result<Self> {
        let conn = Arc::new(conn);
        let selection = Selection::new(conn.clone(), screen, backend.clone())?;
        Ok(Self {
            conn,
            selection,
            manager: None,
            ime: None,
            backend,
            screen,
            queued_event: None,
        })
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
        self.ime = Some(crate::ime::ImeBridge::new(self.conn.clone(), self.screen)?);
        Ok(())
    }
    /// Enable the rootless window manager on this internal X11 connection.
    pub fn enable_window_management(&mut self) -> Result<()> {
        self.manager = Some(crate::wm::WindowManager::new(
            &self.conn,
            self.screen,
            self.selection.window,
            self.backend.clone(),
        )?);
        Ok(())
    }
    pub(crate) fn queue_pending_event(&mut self) -> Result<bool> {
        if self.queued_event.is_none() {
            self.queued_event = self.conn.poll_for_event()?;
        }
        Ok(self.queued_event.is_some())
    }
    pub(crate) fn watch(
        &self,
        wake: crate::notify::UiWake,
    ) -> Result<crate::readiness::ReadinessWatch> {
        crate::readiness::ReadinessWatch::new(self.conn.clone(), wake)
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.selection.next_deadline()
    }
    pub fn tick(&mut self) -> Result<()> {
        self.selection.maintain()?;
        // Bound one UI batch so sustained protocol traffic cannot starve input.
        // queue_pending_event() schedules the remainder before readiness is armed.
        for _ in 0..256 {
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
            self.selection.handle(event)?;
        }
        if let Some(manager) = &mut self.manager {
            manager.tick(&self.conn)?;
        }
        self.conn.flush()?;
        Ok(())
    }
}
