#[cfg(feature = "desktop")]
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
#[cfg(any(feature = "desktop", feature = "ssh"))]
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
        /// All arguments after -- are passed unchanged to XLibre.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Establish authenticated SSH X11 forwarding to an already running local XLibre server.
    #[cfg(feature = "ssh")]
    Ssh {
        /// Force one compression algorithm; omitted: zlib@openssh.com, zlib, then none.
        #[arg(long, value_enum)]
        compression: Option<wayx::ssh::Compression>,
        #[arg(long)]
        host: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        #[arg(long)]
        user: String,
        /// Use an OpenSSH private key instead of password authentication.
        #[arg(long, conflicts_with = "password")]
        identity: Option<PathBuf>,
        /// Prompt for the SSH account password without echo (default without --identity).
        #[arg(long, conflicts_with = "identity")]
        password: bool,
        /// Name of an environment variable holding the encrypted-key passphrase.
        #[arg(long, requires = "identity")]
        key_passphrase_env: Option<String>,
        #[arg(long)]
        known_hosts: PathBuf,
        #[arg(long)]
        xauthority: PathBuf,
        #[arg(long, default_value_t = 100)]
        display: u16,
        /// Remote shell command, e.g. 'xterm'.
        #[arg(long)]
        command: String,
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
            wayx::desktop::run(
                std::sync::Arc::new(std::sync::Mutex::new(b)),
                true,
                false,
                None,
                None,
                frames,
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
        } => {
            let backend =
                std::sync::Arc::new(std::sync::Mutex::new(wayx::backend::Backend::default()));
            let selection_config = match (
                args.iter()
                    .find_map(|a| a.strip_prefix(':').and_then(|s| s.parse::<u16>().ok())),
                args.windows(2).find(|a| a[0] == "-auth"),
            ) {
                (Some(display), Some(auth)) => Some((display, PathBuf::from(&auth[1]))),
                _ => {
                    tracing::warn!(
                        "Pass :DISPLAY and -auth FILE to enable X11 selection synchronization"
                    );
                    None
                }
            };
            let shared = backend.clone();
            let worker = std::thread::spawn(move || -> Result<i32> {
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
                frames,
            )?;
        }
        #[cfg(feature = "ssh")]
        Command::Ssh {
            compression,
            host,
            port,
            user,
            identity,
            password: _,
            key_passphrase_env,
            known_hosts,
            xauthority,
            display,
            command,
        } => {
            let code = tokio::runtime::Runtime::new()?.block_on(wayx::ssh::connect(
                wayx::ssh::Options {
                    compression,
                    host,
                    port,
                    user,
                    authentication: match identity {
                        Some(identity) => wayx::ssh::Authentication::PublicKey {
                            identity,
                            passphrase_env: key_passphrase_env,
                        },
                        None => wayx::ssh::Authentication::Password { password: None },
                    },
                    known_hosts,
                    xauthority,
                    display,
                    command,
                },
            ))?;
            if code != 0 {
                std::process::exit(i32::try_from(code).unwrap_or(1));
            }
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "ssh"))]
mod cli_tests {
    use super::*;

    fn ssh_args() -> Vec<&'static str> {
        vec![
            "wayx",
            "ssh",
            "--host",
            "127.0.0.1",
            "--user",
            "test",
            "--known-hosts",
            "known_hosts",
            "--xauthority",
            "Xauthority",
            "--command",
            "xterm",
        ]
    }
    #[test]
    fn password_login_should_not_require_an_identity_file() {
        let mut args = ssh_args();
        args.push("--password");
        let cli = Cli::try_parse_from(args).expect("Password mode must parse without identity");
        assert!(matches!(
            cli.command,
            Command::Ssh {
                password: true,
                identity: None,
                ..
            }
        ));
        assert!(Cli::try_parse_from(ssh_args()).is_ok());
    }
    #[test]
    fn password_and_identity_should_be_mutually_exclusive() {
        let mut args = ssh_args();
        args.extend(["--password", "--identity", "id_ed25519"]);
        assert!(Cli::try_parse_from(args).is_err());
    }
    #[test]
    fn key_passphrase_should_require_a_private_key() {
        let mut args = ssh_args();
        args.extend(["--key-passphrase-env", "TEST_PASSPHRASE"]);
        assert!(Cli::try_parse_from(args).is_err());
    }
    #[test]
    fn compression_selection_is_optional_and_accepts_only_protocol_names() {
        let cli = Cli::try_parse_from(ssh_args()).unwrap();
        assert!(matches!(
            cli.command,
            Command::Ssh {
                compression: None,
                ..
            }
        ));
        for (name, expected) in [
            ("zlib@openssh.com", wayx::ssh::Compression::ZlibOpenSsh),
            ("zlib", wayx::ssh::Compression::Zlib),
            ("none", wayx::ssh::Compression::None),
        ] {
            let mut args = ssh_args();
            args.extend(["--compression", name]);
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(
                matches!(cli.command, Command::Ssh { compression: Some(actual), .. } if actual == expected)
            );
        }
        for invalid in ["auto", "off", "gzip", "zlib,none"] {
            let mut args = ssh_args();
            args.extend(["--compression", invalid]);
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
