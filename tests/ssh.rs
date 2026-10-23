#![cfg(feature = "ssh")]
//! Loopback integration: a real encrypted russh session opens an RFC4254 X11
//! channel; the destination verifies it receives the real, not the spoof cookie.
use anyhow::{Result, ensure};
use russh::{
    Channel, ChannelId,
    keys::{Algorithm, PrivateKey, PublicKey, ssh_key::LineEnding},
    server,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
#[derive(Clone, Copy)]
enum Scenario {
    PublicKey,
    Password,
    WrongPassword,
    RejectedCommand,
}
struct Remote {
    authorized: PublicKey,
    cookie: String,
    password_only: bool,
    reject_exec: bool,
}
impl server::Handler for Remote {
    type Error = anyhow::Error;
    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<server::Auth> {
        Ok(
            if !self.password_only && user == "test" && key == &self.authorized {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            },
        )
    }
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<server::Auth> {
        Ok(
            if self.password_only && user == "test" && password == "loopback-password" {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            },
        )
    }
    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<()> {
        reply.accept().await;
        Ok(())
    }
    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _single: bool,
        protocol: &str,
        cookie: &str,
        screen: u32,
        session: &mut server::Session,
    ) -> Result<()> {
        ensure!(
            protocol == "MIT-MAGIC-COOKIE-1" && screen == 0 && cookie.len() == 32,
            "Invalid X11 request"
        );
        self.cookie = cookie.into();
        session.channel_success(channel)?;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<()> {
        ensure!(data == b"test-x11", "Unexpected command");
        // Exercise real encrypted output arriving before the exec acknowledgement.
        session.data(channel, b"Remote startup output\n".to_vec())?;
        session.extended_data(channel, 1, b"Remote startup diagnostic\n".to_vec())?;
        if self.reject_exec {
            session.channel_failure(channel)?;
            return Ok(());
        }
        session.channel_success(channel)?;
        let handle = session.handle();
        let cookie = self.cookie.clone();
        tokio::spawn(async move {
            let result: Result<()> = async {
                let channel = handle.channel_open_x11("127.0.0.1", 12345).await?;
                let mut stream = channel.into_stream();
                let mut setup = vec![b'l', 0, 11, 0, 0, 0, 18, 0, 16, 0, 0, 0];
                setup.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
                setup.extend([0, 0]);
                for i in (0..32).step_by(2) {
                    setup.push(u8::from_str_radix(&cookie[i..i + 2], 16)?);
                }
                stream.write_all(&setup).await?;
                let mut response = [0; 12];
                stream.read_exact(&mut response).await?;
                ensure!(&response == b"X11 response", "X11 response mismatch");
                let payload = compression_payload();
                stream.write_all(&payload).await?;
                let mut echoed = vec![0; payload.len()];
                stream.read_exact(&mut echoed).await?;
                ensure!(echoed == payload, "Large compressed X11 payload mismatch");
                Ok(())
            }
            .await;
            let code = if let Err(error) = result {
                eprintln!("Forwarding integration failed: {error}");
                1
            } else {
                0
            };
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }
}
#[test]
fn encrypted_x11_cookie_substitution_and_exit_status() -> Result<()> {
    // Repeatedly exercise real TCP teardown; Windows exposed a shutdown race
    // which a single fast loopback session often misses.
    for _ in 0..10 {
        loopback_roundtrip(Scenario::PublicKey)?;
    }
    Ok(())
}
#[test]
fn password_authentication_should_forward_x11_and_preserve_exit_status() -> Result<()> {
    for _ in 0..5 {
        loopback_roundtrip(Scenario::Password)?;
    }
    Ok(())
}
#[test]
fn incorrect_password_should_fail_without_forwarding_x11() -> Result<()> {
    loopback_roundtrip(Scenario::WrongPassword)
}
#[test]
fn explicit_exec_failure_should_still_fail_after_early_output() -> Result<()> {
    loopback_roundtrip(Scenario::RejectedCommand)
}
fn loopback_roundtrip(scenario: Scenario) -> Result<()> {
    compressed_roundtrip(scenario, None, None)
}
fn compressed_roundtrip(
    scenario: Scenario,
    compression: Option<wayx::ssh::Compression>,
    server_algorithm: Option<russh::compression::Name>,
) -> Result<()> {
    tokio::runtime::Runtime::new()?.block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let dir = tempfile::tempdir()?;
            let host_key = PrivateKey::random(&mut rand_10::rng(), Algorithm::Ed25519)?;
            let identity = PrivateKey::random(&mut rand_10::rng(), Algorithm::Ed25519)?;
            let key_path = dir.path().join("identity");
            identity.write_openssh_file(&key_path, LineEnding::LF)?;
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            let port = listener.local_addr()?.port();
            let known_hosts = dir.path().join("known_hosts");
            std::fs::write(
                &known_hosts,
                format!(
                    "[127.0.0.1]:{port} {}\n",
                    host_key.public_key().to_openssh()?
                ),
            )?;
            let local = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            let display = local
                .local_addr()?
                .port()
                .checked_sub(6000)
                .context("Ephemeral port below X11 base")?;
            let xauthority = dir.path().join("Xauthority");
            let mut auth = vec![1, 0];
            for field in [
                &b"localhost"[..],
                display.to_string().as_bytes(),
                b"MIT-MAGIC-COOKIE-1",
                &[77u8; 16],
            ] {
                auth.extend_from_slice(&(field.len() as u16).to_be_bytes());
                auth.extend_from_slice(field);
            }
            std::fs::write(&xauthority, auth)?;
            let (connected, mut connection) = tokio::sync::oneshot::channel();
            let destination = tokio::spawn(async move {
                let (mut stream, _) = local.accept().await?;
                let _ = connected.send(());
                let mut setup = [0; 48];
                stream.read_exact(&mut setup).await?;
                ensure!(
                    setup[32..] == [77; 16],
                    "Destination did not receive real cookie"
                );
                stream.write_all(b"X11 response").await?;
                let expected = compression_payload();
                let mut payload = vec![0; expected.len()];
                stream.read_exact(&mut payload).await?;
                ensure!(payload == expected, "Forwarded X11 payload mismatch");
                stream.write_all(&payload).await?;
                Ok::<_, anyhow::Error>(())
            });
            let config = Arc::new(server::Config {
                keys: vec![host_key],
                preferred: russh::Preferred {
                    compression: server_algorithm
                        .map(|a| vec![a].into())
                        .unwrap_or_else(|| russh::Preferred::default().compression),
                    ..Default::default()
                },
                ..Default::default()
            });
            let authorized = identity.public_key().clone();
            let remote = tokio::spawn(async move {
                let (stream, _) = listener.accept().await?;
                server::run_stream(
                    config,
                    stream,
                    Remote {
                        authorized,
                        cookie: String::new(),
                        password_only: matches!(
                            scenario,
                            Scenario::Password | Scenario::WrongPassword
                        ),
                        reject_exec: matches!(scenario, Scenario::RejectedCommand),
                    },
                )
                .await?
                .await?;
                Ok::<_, anyhow::Error>(())
            });
            let result = wayx::ssh::connect(wayx::ssh::Options {
                compression,
                host: "127.0.0.1".into(),
                port,
                user: "test".into(),
                authentication: match scenario {
                    Scenario::Password => wayx::ssh::Authentication::Password {
                        password: Some("loopback-password".into()),
                    },
                    Scenario::WrongPassword => wayx::ssh::Authentication::Password {
                        password: Some("not-the-loopback-password".into()),
                    },
                    _ => wayx::ssh::Authentication::PublicKey {
                        identity: key_path,
                        passphrase_env: None,
                    },
                },
                known_hosts,
                xauthority,
                display,
                command: "test-x11".into(),
            })
            .await;
            if compression.is_some()
                && server_algorithm == Some(russh::compression::NONE)
                && compression != Some(wayx::ssh::Compression::None)
            {
                let error = result.expect_err("Explicit compression must not fall back");
                ensure!(
                    error.chain().any(|cause| matches!(
                        cause.downcast_ref::<russh::Error>(),
                        Some(russh::Error::NoCommonAlgo { .. })
                    )),
                    "Unexpected error: {error:#}"
                );
                destination.abort();
                remote.abort();
                return Ok(());
            }
            if matches!(
                scenario,
                Scenario::WrongPassword | Scenario::RejectedCommand
            ) {
                let error = result.expect_err("The request must be rejected");
                let expected = if matches!(scenario, Scenario::WrongPassword) {
                    "SSH password authentication failed"
                } else {
                    "SSH server rejected remote command"
                };
                ensure!(
                    error.to_string().contains(expected),
                    "Unexpected failure: {error}"
                );
                ensure!(!error.to_string().contains("loopback-password"));
                ensure!(
                    tokio::time::timeout(Duration::from_millis(50), &mut connection)
                        .await
                        .is_err(),
                    "Rejected request reached local X11"
                );
                destination.abort();
                remote.abort();
                return Ok(());
            }
            let status = result.context("SSH client session")?;
            ensure!(status == 0, "Remote command failed");
            destination.await?.context("Local X11 destination")?;
            // All application bytes, cookie substitution and exit status were
            // verified above. TCP may reset on peer teardown when SSH channel
            // bookkeeping is still in flight (observed on Windows and macOS).
            if let Err(error) = remote.await?
                && !peer_reset_after_completion(&error)
            {
                return Err(error.context("SSH server session teardown"));
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    })
}
use anyhow::Context;

// russh's transparent IO variant does not expose std::io::Error itself through
// Error::source(), so inspect the typed wrapper as well as ordinary IO errors.
fn peer_reset_after_completion(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let io = cause.downcast_ref::<std::io::Error>().or_else(|| {
            match cause.downcast_ref::<russh::Error>() {
                Some(russh::Error::IO(error)) => Some(error),
                _ => None,
            }
        });
        io.is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            )
        })
    })
}
#[test]
fn teardown_should_accept_typed_peer_reset_but_reject_truncated_data() {
    let reset = russh::Error::IO(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
    assert!(peer_reset_after_completion(
        &anyhow::Error::new(reset).context("server teardown")
    ));
    let aborted = russh::Error::IO(std::io::Error::from(std::io::ErrorKind::ConnectionAborted));
    assert!(peer_reset_after_completion(&aborted.into()));
    let truncated = russh::Error::IO(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    assert!(!peer_reset_after_completion(&truncated.into()));
}

#[test]
fn compression_algorithms_forward_real_x11() -> Result<()> {
    use russh::compression::{NONE, ZLIB, ZLIB_LEGACY};
    use wayx::ssh::Compression;
    for (selection, algorithm) in [
        (None, ZLIB),
        (None, ZLIB_LEGACY),
        (None, NONE),
        (Some(Compression::ZlibOpenSsh), ZLIB_LEGACY),
        (Some(Compression::Zlib), ZLIB),
        (Some(Compression::None), NONE),
    ] {
        compressed_roundtrip(Scenario::Password, selection, Some(algorithm))?;
    }
    Ok(())
}

#[test]
fn explicit_compression_must_not_fall_back() -> Result<()> {
    for algorithm in [
        wayx::ssh::Compression::ZlibOpenSsh,
        wayx::ssh::Compression::Zlib,
    ] {
        compressed_roundtrip(
            Scenario::Password,
            Some(algorithm),
            Some(russh::compression::NONE),
        )?;
    }
    Ok(())
}

// Exercise multiple SSH packets with both repetitive pixels and less compressible bytes.
fn compression_payload() -> Vec<u8> {
    let mut payload = vec![0x55; 64 * 1024];
    let mut state = 0x12345678u32;
    for _ in 0..64 * 1024 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        payload.push(state as u8);
    }
    payload
}
