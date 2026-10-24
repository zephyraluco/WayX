//! One internal X11 connection and event dispatcher for WM, XIM and Selection.
use crate::{bridge::SharedBackend, selection::Selection};
use anyhow::{Result, ensure};
use std::{sync::Arc, time::Instant};
use x11rb::{connection::Connection, protocol::Event, rust_connection::RustConnection};

#[derive(Debug, PartialEq, Eq)]
pub enum LocalAuth {
    None,
    Xauthority(std::path::PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub struct LocalDisplay {
    pub number: u16,
    pub auth: LocalAuth,
}

impl LocalDisplay {
    /// Normalize the native server and its internal clients to the same auth mode.
    pub fn from_args(args: &mut Vec<String>) -> Result<Self> {
        let number = args
            .iter()
            .find_map(|arg| {
                arg.strip_prefix(':')
                    .and_then(|value| value.parse::<u16>().ok())
            })
            .ok_or_else(|| anyhow::anyhow!("Specify a local display, e.g. :100"))?;
        ensure!(number <= 59535, "Display number exceeds TCP range");
        let auth_options: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "-auth")
            .collect();
        ensure!(auth_options.len() <= 1, "Specify -auth only once");
        let no_auth = args.iter().any(|arg| arg == "-ac");
        let auth = if let Some((index, _)) = auth_options.first() {
            ensure!(!no_auth, "Choose either -ac or -auth FILE");
            let path = args
                .get(index + 1)
                .filter(|value| !value.starts_with('-'))
                .ok_or_else(|| anyhow::anyhow!("-auth requires an Xauthority file"))?;
            let path = std::path::PathBuf::from(path);
            // Fail before starting a server if its internal clients cannot authenticate.
            crate::auth::xauthority_cookie(&path, number)?;
            LocalAuth::Xauthority(path)
        } else {
            if !no_auth {
                args.push("-ac".into());
            }
            LocalAuth::None
        };
        Ok(Self { number, auth })
    }
}

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
    pub fn connect_local(config: &LocalDisplay, backend: SharedBackend) -> Result<Self> {
        let display = config.number;
        ensure!(display <= 59535, "Display number exceeds TCP range");
        let (protocol, cookie) = match &config.auth {
            LocalAuth::Xauthority(path) => (
                b"MIT-MAGIC-COOKIE-1".to_vec(),
                crate::auth::xauthority_cookie(path, display)?.to_vec(),
            ),
            LocalAuth::None => (Vec::new(), Vec::new()),
        };
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
        let conn = RustConnection::connect_to_stream_with_auth_info(stream, 0, protocol, cookie)?;
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

#[cfg(test)]
mod config_tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn defaults_to_no_auth_and_normalizes_only_once() {
        let mut arguments = args(&[":100", "-listen", "tcp"]);
        assert_eq!(
            LocalDisplay::from_args(&mut arguments).unwrap(),
            LocalDisplay {
                number: 100,
                auth: LocalAuth::None
            }
        );
        LocalDisplay::from_args(&mut arguments).unwrap();
        assert_eq!(arguments.iter().filter(|value| *value == "-ac").count(), 1);
    }

    #[test]
    fn explicit_auth_does_not_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Xauthority");
        let mut bytes = vec![255, 255, 0, 0, 0, 3];
        bytes.extend_from_slice(b"100");
        bytes.extend_from_slice(&[0, 18]);
        bytes.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
        bytes.extend_from_slice(&[0, 16]);
        bytes.extend_from_slice(&[7; 16]);
        std::fs::write(&path, bytes).unwrap();
        let mut arguments = vec![":100".into(), "-auth".into(), path.to_str().unwrap().into()];
        assert_eq!(
            LocalDisplay::from_args(&mut arguments).unwrap().auth,
            LocalAuth::Xauthority(path.clone())
        );
        assert!(!arguments.iter().any(|value| value == "-ac"));
        std::fs::remove_file(path).unwrap();
        assert!(LocalDisplay::from_args(&mut arguments).is_err());
    }

    #[test]
    fn rejects_conflicting_or_incomplete_auth_and_invalid_displays() {
        for values in [
            vec![":100", "-ac", "-auth", "file"],
            vec![":100", "-auth"],
            vec![":100", "-auth", "-ac"],
            vec![":100", "-auth", "a", "-auth", "b"],
            vec![":65535"],
            vec![],
        ] {
            assert!(
                LocalDisplay::from_args(&mut args(&values)).is_err(),
                "{values:?}"
            );
        }
    }
}
