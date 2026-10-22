//! Real Xlib XIM interoperability against the embedded XLibre core.
#![cfg(feature = "desktop")]
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CString,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use wayx::{
    backend::{Backend, InputEvent, event},
    bridge::{API, Api, Handle},
    ime::{ImeBridge, ImeError},
};
use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
#[test]
#[ignore = "requires XLibre plugin and compiled Xlib helper; set WAYX_TEST_CORE and WAYX_TEST_XIM_CLIENT"]
fn xim_should_deliver_unicode_preedit_and_follow_context_focus() -> Result<()> {
    let path = std::env::var("WAYX_TEST_CORE").context("Set WAYX_TEST_CORE")?;
    let backend = Arc::new(Mutex::new(Backend::default()));
    let shared = backend.clone();
    let worker = std::thread::spawn(move || -> Result<i32> {
        // SAFETY: pinned plugin, live vtable, arguments and backend until run returns.
        let library = unsafe { libloading::Library::new(path) }?;
        let run = unsafe {
            library.get::<unsafe extern "C" fn(
                *const Api,
                *mut Handle,
                i32,
                *mut *mut std::ffi::c_char,
            ) -> i32>(b"wayx_xserver_run\0")
        }?;
        let args = [
            "XWayX",
            ":196",
            "-wayx-rootless",
            "-screen",
            "0",
            "640x480x24",
            "-crtcs",
            "1@640x480",
            if cfg!(windows) {
                "-listen"
            } else {
                "-nolisten"
            },
            "tcp",
            "-ac",
        ]
        .iter()
        .map(|s| CString::new(*s))
        .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut argv: Vec<_> = args.iter().map(|s| s.as_ptr().cast_mut()).collect();
        argv.push(std::ptr::null_mut());
        let mut handle = Handle(shared);
        Ok(unsafe { run(&API, &mut handle, args.len() as i32, argv.as_mut_ptr()) })
    });
    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.lock().unwrap().windows.contains_key(&1) {
            ensure!(Instant::now() < deadline, "Server did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (conn, screen) = x11rb::connect(Some(":196"))?;
        let conn = Arc::new(conn);
        let mut ime = ImeBridge::new(conn.clone(), screen)?;
        ensure!(matches!(
            ImeBridge::new(conn.clone(), screen),
            Err(ImeError::AlreadyRunning)
        ));
        let atom = conn.intern_atom(false, b"@server=wayx")?.reply()?.atom;
        let owner = conn.get_selection_owner(atom)?.reply()?.owner;
        let locale = if cfg!(target_os = "macos") {
            "en_US.UTF-8"
        } else {
            "C.UTF-8"
        };
        for position in [false, true] {
            let mut command = Command::new(std::env::var("WAYX_TEST_XIM_CLIENT")?);
            command
                .env("WAYX_XIM_POSITION", if position { "1" } else { "" })
                .env("DISPLAY", ":196")
                .env("LC_ALL", locale)
                .stdout(Stdio::piped());
            if !position {
                command.env_remove("WAYX_XIM_POSITION");
            }
            let mut child = command.spawn()?;
            let output = child.stdout.take().unwrap();
            let (send, receive) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    if send.send(line).is_err() {
                        break;
                    }
                }
            });
            let attempt = (|| -> Result<()> {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut stage = 0;
                let mut windows = (0, 0);
                loop {
                    ensure!(
                        Instant::now() < deadline,
                        "XIM test timed out at stage {stage}"
                    );
                    while let Some(event) = conn.poll_for_event()? {
                        ime.filter_event(&event)?;
                    }
                    while let Ok(line) = receive.try_recv() {
                        let line = line?;
                        if line.starts_with("READY ") {
                            let ids: Vec<u32> = line
                                .split_whitespace()
                                .skip(1)
                                .map(str::parse)
                                .collect::<std::result::Result<_, _>>()?;
                            windows = (ids[0], ids[1]);
                            stage = 1;
                        } else if line == "FOCUS" {
                            stage = 3;
                        } else if line == "DONE" {
                            return Ok(());
                        }
                    }
                    if stage == 1 && ime.cursor(windows.0)?.is_some() {
                        ensure!(ime.cursor(windows.0)? == Some((25, 40)));
                        ensure!(matches!(
                            ime.commit(windows.1, "wrong window"),
                            Err(ImeError::NoContext)
                        ));
                        ensure!(matches!(
                            ime.commit(windows.0, &"x".repeat(8193)),
                            Err(ImeError::TextTooLong)
                        ));
                        ime.preedit_at(windows.0, "中文组合", Some((3, 3)))?;
                        ime.preedit(windows.0, "中文")?;
                        ime.commit(windows.0, "中文输入测试：你好世界。日本語の入力。한국어 입력. This commit is longer than 32 bytes. 😀")?;
                        stage = 2;
                    }
                    if stage == 3 && ime.cursor(windows.1)?.is_some() {
                        ensure!(matches!(
                            ime.commit(windows.0, "stale focus"),
                            Err(ImeError::NoContext)
                        ));
                        ensure!(ime.cursor(windows.1)? == Some((50, 60)));
                        ensure!(!ime.inline_preedit());
                        ime.preedit(windows.1, "组合")?;
                        ime.commit(windows.1, "第二窗口 한국어 日本語")?;
                        stage = 4;
                    }
                    if let Some(status) = child.try_wait()? {
                        anyhow::bail!("Xlib client exited before DONE: {status}");
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })();
            if attempt.is_err() {
                let _ = child.kill();
            }
            let status = child.wait()?;
            reader.join().unwrap();
            attempt?;
            ensure!(status.success(), "Xlib client failed: {status}");
            while let Some(event) = conn.poll_for_event()? {
                ime.filter_event(&event)?;
            }
            ensure!(matches!(
                ime.commit(1, "destroyed"),
                Err(ImeError::NoContext)
            ));
            ensure!(
                conn.query_tree(owner)?.reply()?.children.is_empty(),
                "Clean disconnect leaked a transport window"
            );
        }
        let mut crashed = Command::new(std::env::var("WAYX_TEST_XIM_CLIENT")?)
            .env("DISPLAY", ":196")
            .env("LC_ALL", locale)
            .env("WAYX_XIM_ABORT", "1")
            .stdout(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_context = false;
        loop {
            if Instant::now() >= deadline {
                let _ = crashed.kill();
                let _ = crashed.wait();
                anyhow::bail!("Crashed client cleanup timed out");
            }
            while let Some(event) = conn.poll_for_event()? {
                ime.filter_event(&event)?;
            }
            saw_context |= ime.cursor(1)?.is_some();
            if crashed.try_wait()?.is_some() && conn.query_tree(owner)?.reply()?.children.is_empty()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        ensure!(saw_context, "Crash test never established an input context");
        ensure!(matches!(ime.commit(1, "crashed"), Err(ImeError::NoContext)));

        Ok(())
    })();
    backend.lock().unwrap().push_input(InputEvent {
        kind: event::SHUTDOWN,
        ..Default::default()
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() {
        ensure!(Instant::now() < deadline, "Server did not stop");
        std::thread::sleep(Duration::from_millis(5));
    }
    ensure!(worker.join().expect("XLibre worker panicked")? == 0);
    result
}
