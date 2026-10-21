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
struct Remote {
    authorized: PublicKey,
    cookie: String,
}
impl server::Handler for Remote {
    type Error = anyhow::Error;
    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<server::Auth> {
        Ok(if user == "test" && key == &self.authorized {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
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
            let destination = tokio::spawn(async move {
                let (mut stream, _) = local.accept().await?;
                let mut setup = [0; 48];
                stream.read_exact(&mut setup).await?;
                ensure!(
                    setup[32..] == [77; 16],
                    "Destination did not receive real cookie"
                );
                stream.write_all(b"X11 response").await?;
                Ok::<_, anyhow::Error>(())
            });
            let config = Arc::new(server::Config {
                keys: vec![host_key],
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
                    },
                )
                .await?
                .await?;
                Ok::<_, anyhow::Error>(())
            });
            let status = wayx::ssh::connect(wayx::ssh::Options {
                host: "127.0.0.1".into(),
                port,
                user: "test".into(),
                identity: key_path,
                passphrase_env: None,
                known_hosts,
                xauthority,
                display,
                command: "test-x11".into(),
            })
            .await?;
            ensure!(status == 0, "Remote command failed");
            destination.await??;
            remote.await??;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    })
}
use anyhow::Context;
