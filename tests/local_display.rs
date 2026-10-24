//! Exercise the same internal connection in authenticated and default no-auth mode.
#![cfg(feature = "desktop")]
#[path = "support/core.rs"]
mod core;
use anyhow::{Result, ensure};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wayx::{
    backend::Backend,
    bridge::{API, Api},
    x11_session::{LocalDisplay, X11Session},
};
use x11rb::{connection::Connection, protocol::xproto::*};

#[test]
#[ignore = "requires built XLibre core; WAYX_TEST_LOCAL_AUTH optionally selects Xauthority"]
fn internal_services_work_in_both_auth_modes() -> Result<()> {
    let mut arguments = vec![
        "XWayX".into(),
        ":192".into(),
        "-wayx-rootless".into(),
        "-screen".into(),
        "0".into(),
        "640x480x24".into(),
        "-crtcs".into(),
        "1@640x480".into(),
        "-listen".into(),
        "tcp".into(),
        "-nolisten".into(),
        "local".into(),
    ];
    let directory = tempfile::tempdir()?;
    if std::env::var_os("WAYX_TEST_LOCAL_AUTH").is_some() {
        let path = directory.path().join("Xauthority");
        let mut bytes = vec![255, 255, 0, 0, 0, 3];
        bytes.extend_from_slice(b"192");
        bytes.extend_from_slice(&[0, 18]);
        bytes.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
        bytes.extend_from_slice(&[0, 16]);
        bytes.extend_from_slice(&[7; 16]);
        std::fs::write(&path, bytes)?;
        arguments.extend(["-auth".into(), path.to_string_lossy().into_owned()]);
    }
    let config = LocalDisplay::from_args(&mut arguments)?;
    let backend = Arc::new(Mutex::new(Backend::default()));
    let server = core::CoreServer::start(
        backend.clone(),
        Api { ..API },
        &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.lock().unwrap().windows.contains_key(&1) {
            ensure!(Instant::now() < deadline, "Core did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        if matches!(config.auth, wayx::x11_session::LocalAuth::Xauthority(_)) {
            for cookie in [Vec::new(), vec![8; 16]] {
                let stream = x11rb::rust_connection::DefaultStream::from_tcp_stream(
                    std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 6192))?,
                )?
                .0;
                let protocol = if cookie.is_empty() {
                    Vec::new()
                } else {
                    b"MIT-MAGIC-COOKIE-1".to_vec()
                };
                ensure!(
                    x11rb::rust_connection::RustConnection::connect_to_stream_with_auth_info(
                        stream, 0, protocol, cookie
                    )
                    .is_err(),
                    "Authenticated server accepted missing or invalid cookie"
                );
            }
        }
        let mut session = X11Session::connect_local(&config, backend.clone())?;
        session.enable_window_management()?;
        session.enable_ime()?;
        // A second WM must be rejected, proving the internal owner is active.
        let mut other = X11Session::connect_local(&config, backend.clone())?;
        ensure!(other.enable_window_management().is_err());
        ensure!(other.enable_ime().is_err());
        drop(other);
        // WM initialization publishes the supporting window on the root.
        let display = "127.0.0.1:192";
        if matches!(config.auth, wayx::x11_session::LocalAuth::None) {
            let (conn, screen) = x11rb::connect(Some(display))?;
            let root = conn.setup().roots[screen].root;
            let atom = conn
                .intern_atom(false, b"_NET_SUPPORTING_WM_CHECK")?
                .reply()?
                .atom;
            ensure!(
                !conn
                    .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)?
                    .reply()?
                    .value
                    .is_empty()
            );
        }
        session.tick()?;
        Ok(())
    })();
    server.finish()?;
    result
}
