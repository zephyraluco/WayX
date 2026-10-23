//! Exercise an independent Mesa client over TCP, including actual VBO pixels.
#[path = "support/core.rs"]
mod core;
use anyhow::{Context, Result, ensure};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use wayx::{
    backend::Backend,
    bridge::{API, Api},
};
#[test]
#[ignore = "requires XLibre plugin and GLX client; set WAYX_TEST_CORE and WAYX_TEST_GLX_CLIENT"]
fn glx_should_render_modern_software_opengl_over_tcp() -> Result<()> {
    let rootless = std::env::var_os("WAYX_TEST_ROOTLESS").is_some();
    let backend = Arc::new(Mutex::new(Backend::default()));
    let server = core::CoreServer::start(
        backend.clone(),
        Api { ..API },
        &[
            "XWayX",
            ":197",
            if rootless {
                "-wayx-rootless"
            } else {
                "-nocursor"
            },
            "-screen",
            "0",
            "640x480x24",
            "-crtcs",
            "1@640x480",
            "-listen",
            "tcp",
            "+iglx",
            "-ac",
        ],
    )?;
    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.lock().unwrap().windows.contains_key(&1) {
            ensure!(Instant::now() < deadline, "Server did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut child = Command::new(std::env::var("WAYX_TEST_GLX_CLIENT")?)
            .env("DISPLAY", "127.0.0.1:197")
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .env("GALLIUM_DRIVER", "llvmpipe")
            .env_remove("LIBGL_ALWAYS_INDIRECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let attempt = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut id = None;
            loop {
                ensure!(Instant::now() < deadline, "GLX render timed out");
                while let Ok(line) = receive.try_recv() {
                    let line = line?;
                    println!("{line}");
                    if line.starts_with("GLX_READY ") {
                        id = Some(
                            line.split_whitespace()
                                .nth(1)
                                .context("Missing GLX window")?
                                .parse::<u32>()?,
                        );
                    }
                }
                if let Some(id) = id {
                    let b = backend.lock().unwrap();
                    if let Some(window) = b.windows.get(&if rootless { id } else { 1 }) {
                        let fb = &window.framebuffer;
                        let (x, y) = if rootless { (32, 32) } else { (52, 52) };
                        let offset = (y * fb.width as usize + x) * 4;
                        if fb.pixels.get(offset..offset + 3) == Some(&[0, 0, 255]) {
                            break;
                        }
                    }
                }
                if let Some(status) = child.try_wait()? {
                    anyhow::bail!("GLX client exited: {status}");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })();
        if attempt.is_err() {
            let _ = child.kill();
        } else {
            child.stdin.take().unwrap().write_all(b"\n")?;
        }
        let status = child.wait()?;
        reader.join().unwrap();
        attempt?;
        ensure!(status.success(), "GLX client failed: {status}");
        Ok(())
    })();
    server.finish()?;
    result
}
