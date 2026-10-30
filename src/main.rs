#[cfg(feature = "desktop")]
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
#[cfg(feature = "desktop")]
use std::path::PathBuf;
#[derive(Parser)]
#[command(version, about = "XLibre + Rust DDX + winit/wgpu display backend")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Open a framebuffer demo without an X server.
    #[cfg(feature = "desktop")]
    Demo {
        /// Exit after N successfully presented frames (automation/smoke testing).
        #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
        frames: Option<u32>,
    },
    /// Test GPU upload, composition and readback without a window system.
    #[cfg(feature = "desktop")]
    GpuSmoke,
    /// Run an XLibre DDX plugin on a worker and the native UI on the main thread.
    #[cfg(feature = "desktop")]
    Serve {
        #[arg(long)]
        core: PathBuf,
        /// Present each X11 top-level window as an independent native window.
        #[arg(long)]
        rootless: bool,
        /// Exit after N presented frames, then shut down XLibre.
        #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
        frames: Option<u32>,
        /// Host display scale: auto or a multiplier between 0.5 and 4 (1 = native pixels).
        #[arg(long, default_value = "auto")]
        scale: wayx::scale::ScalePolicy,
        /// All arguments after -- are passed unchanged to XLibre.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Report the C bridge ABI version.
    Abi,
}
fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wayx=info".into()),
        )
        .init();
    match Cli::parse().command {
        Command::Abi => println!(
            "WayX ABI {} ({} bytes)",
            wayx::bridge::ABI_VERSION,
            std::mem::size_of::<wayx::bridge::Api>()
        ),
        #[cfg(feature = "desktop")]
        Command::Demo { frames } => {
            let mut b = wayx::backend::Backend::default();
            b.create(1, 0, 800, 500)?;
            b.windows.get_mut(&1).context("Demo window missing")?.mapped = true;
            let mut pixels = vec![0u8; 800 * 500 * 4];
            for y in 0..500 {
                for x in 0..800 {
                    let n = (y * 800 + x) * 4;
                    pixels[n] = ((x / 40 + y / 40) % 2 * 100 + 40) as u8;
                    pixels[n + 1] = (y * 255 / 500) as u8;
                    pixels[n + 2] = (x * 255 / 800) as u8;
                }
            }
            b.windows.get_mut(&1).unwrap().framebuffer.upload(
                wayx::backend::Rect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 500,
                },
                800 * 4,
                &pixels,
            )?;
            b.flush_output();
            wayx::desktop::run(
                std::sync::Arc::new(std::sync::Mutex::new(b)),
                true,
                false,
                None,
                None,
                wayx::scale::DisplayOptions {
                    frame_limit: frames,
                    ..Default::default()
                },
                None,
            )?;
        }
        #[cfg(feature = "desktop")]
        Command::GpuSmoke => pollster::block_on(wayx::desktop::gpu_smoke())?,
        #[cfg(feature = "desktop")]
        Command::Serve {
            core,
            args,
            frames,
            rootless,
            scale,
        } => {
            let backend =
                std::sync::Arc::new(std::sync::Mutex::new(wayx::backend::Backend::default()));
            let mut args = args;
            let selection_config = Some(wayx::x11_session::LocalDisplay::from_args(&mut args)?);
            let (ready_send, ready_receive) = std::sync::mpsc::channel();
            let shared = backend.clone();
            let worker = std::thread::spawn(move || -> Result<i32> {
                let _completion = wayx::notify::WorkerExit(shared.clone());
                // XLibre installs process-wide signal handlers that can outlive dix_main.
                // Pin the trusted native plugin until process exit; argv and the handle
                // still outlive every server callback on this worker.
                let library =
                    std::mem::ManuallyDrop::new(unsafe { libloading::Library::new(core) }?);
                let run = unsafe {
                    library.get::<unsafe extern "C" fn(
                        *const wayx::bridge::Api,
                        *mut wayx::bridge::Handle,
                        i32,
                        *mut *mut std::ffi::c_char,
                    ) -> i32>(b"wayx_xserver_run\0")
                }?;
                if rootless {
                    ready_receive
                        .recv()
                        .context("Native desktop layout initialization failed")?;
                }
                let mut argv = vec![std::ffi::CString::new("XWayX")?];
                if rootless {
                    argv.push(std::ffi::CString::new("-wayx-rootless")?);
                }
                for arg in args {
                    argv.push(std::ffi::CString::new(arg)?);
                }
                let argc = i32::try_from(argv.len())?;
                let mut pointers: Vec<_> = argv.iter().map(|arg| arg.as_ptr().cast_mut()).collect();
                pointers.push(std::ptr::null_mut());
                let mut handle = wayx::bridge::Handle(shared);
                Ok(unsafe { run(&wayx::bridge::API, &mut handle, argc, pointers.as_mut_ptr()) })
            });
            wayx::desktop::run(
                backend,
                false,
                rootless,
                Some(worker),
                selection_config,
                wayx::scale::DisplayOptions {
                    frame_limit: frames,
                    scale,
                },
                rootless.then_some(ready_send),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    #[test]
    fn ssh_is_an_external_tool() {
        assert!(Cli::try_parse_from(["wayx", "ssh"]).is_err());
    }
    #[cfg(feature = "desktop")]
    #[test]
    fn scale_defaults_to_auto_and_accepts_only_supported_values() {
        let cli = Cli::try_parse_from(["wayx", "serve", "--core", "core"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Serve {
                scale: wayx::scale::ScalePolicy::Auto,
                ..
            }
        ));
        for value in ["auto", "0.5", "1", "1.25", "2", "4"] {
            assert!(
                Cli::try_parse_from(["wayx", "serve", "--core", "core", "--scale", value]).is_ok()
            );
        }
        for value in ["0", "NaN", "inf", "4.01", "fit"] {
            assert!(
                Cli::try_parse_from(["wayx", "serve", "--core", "core", "--scale", value]).is_err()
            );
        }
    }
}
