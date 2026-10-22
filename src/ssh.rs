use crate::auth::xauthority_cookie;

use anyhow::{Context, Result, anyhow, ensure};
use rand::RngCore;
use russh::{
    Channel, ChannelMsg, client,
    keys::{PrivateKeyWithHashAlg, known_hosts::check_known_hosts_path, load_secret_key},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Semaphore,
};

/// SSH user authentication; credentials are never included in debug output.
pub enum Authentication {
    /// Authenticate with an OpenSSH private key and optional passphrase variable.
    PublicKey {
        identity: PathBuf,
        passphrase_env: Option<String>,
    },
    /// Authenticate with a password; None prompts without echo after host verification.
    Password { password: Option<String> },
}
pub struct Options {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub authentication: Authentication,
    pub known_hosts: PathBuf,
    pub xauthority: PathBuf,
    pub display: u16,
    pub command: String,
}
#[derive(Debug, thiserror::Error)]
enum AuthenticationError {
    #[error("SSH {method} authentication failed")]
    Rejected { method: &'static str },
}
#[derive(Debug, thiserror::Error)]
enum RequestError {
    #[error("{0}")]
    Rejected(&'static str),
    #[error("SSH channel closed before request acknowledgement ({0})")]
    Closed(&'static str),
    #[error("Remote command exited with status {0} before request acknowledgement")]
    Exited(u32),
}
struct Client {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    display: u16,
    fake_cookie: [u8; 16],
    real_cookie: [u8; 16],
    permits: Arc<Semaphore>,
}
impl client::Handler for Client {
    type Error = anyhow::Error;
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool> {
        ensure!(
            check_known_hosts_path(&self.host, self.port, &key.public_key(), &self.known_hosts)?,
            "SSH host key is not trusted; verify and add it to the specified known_hosts file first"
        );
        Ok(true)
    }
    async fn server_channel_open_x11(
        &mut self,
        channel: Channel<client::Msg>,
        _origin: &str,
        _port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<()> {
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            tracing::warn!("X11 connection limit reached");
            return Ok(());
        };
        reply.accept().await;
        let fake = self.fake_cookie;
        let real = self.real_cookie;
        let port = 6000 + self.display;
        tokio::spawn(async move {
            let _permit = permit;
            let result: Result<()> = async {
                let mut stream = channel.into_stream();
                let prefix = tokio::time::timeout(
                    Duration::from_secs(10),
                    validated_setup(&mut stream, &fake, &real),
                )
                .await
                .context("X11 setup timeout")??;
                // Unauthenticated channels can never reach the local X server.
                #[cfg(unix)]
                if let Ok(mut local) =
                    tokio::net::UnixStream::connect(format!("/tmp/.X11-unix/X{}", port - 6000))
                        .await
                {
                    local.write_all(&prefix).await?;
                    tokio::io::copy_bidirectional(&mut stream, &mut local).await?;
                    return Ok(());
                }
                let mut local = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                    .await
                    .context("Connect local XLibre listener")?;
                local.write_all(&prefix).await?;
                tokio::io::copy_bidirectional(&mut stream, &mut local).await?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(%error,"X11 forwarding channel closed");
            }
        });
        Ok(())
    }
}
/// Reads and authenticates the initial X11 request, replacing only a valid fake cookie.
async fn validated_setup<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
    fake: &[u8; 16],
    real: &[u8; 16],
) -> Result<Vec<u8>> {
    let mut header = [0u8; 12];
    stream.read_exact(&mut header).await?;
    ensure!(matches!(header[0], b'l' | b'B'), "Invalid X11 byte order");
    let read = |offset: usize| {
        if header[0] == b'l' {
            u16::from_le_bytes([header[offset], header[offset + 1]])
        } else {
            u16::from_be_bytes([header[offset], header[offset + 1]])
        }
    };
    ensure!(
        read(2) == 11 && read(4) == 0,
        "Unsupported X11 protocol version"
    );
    let name_len = read(6) as usize;
    let cookie_len = read(8) as usize;
    ensure!(
        name_len == 18 && cookie_len == 16,
        "Only MIT-MAGIC-COOKIE-1 authentication is accepted"
    );
    let padded_name = (name_len + 3) & !3;
    let mut body = vec![0; padded_name + 16];
    stream.read_exact(&mut body).await?;
    ensure!(
        &body[..name_len] == b"MIT-MAGIC-COOKIE-1",
        "Unsupported X11 authentication protocol"
    );
    let difference = body[padded_name..]
        .iter()
        .zip(fake)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    ensure!(difference == 0, "Invalid X11 forwarding cookie");
    body[padded_name..].copy_from_slice(real);
    let mut output = header.to_vec();
    output.extend_from_slice(&body);
    Ok(output)
}
pub async fn connect(options: Options) -> Result<u32> {
    ensure!(
        options.display <= 59535,
        "Display number exceeds TCP port range"
    );
    let real_cookie = xauthority_cookie(&options.xauthority, options.display)?;
    let mut fake_cookie = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut fake_cookie);
    let hex: String = fake_cookie.iter().map(|b| format!("{b:02x}")).collect();
    let config = client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    };
    let handler = Client {
        host: options.host.clone(),
        port: options.port,
        known_hosts: options.known_hosts,
        display: options.display,
        fake_cookie,
        real_cookie,
        permits: Arc::new(Semaphore::new(64)),
    };
    let mut session = tokio::time::timeout(
        Duration::from_secs(30),
        client::connect(
            Arc::new(config),
            (options.host.as_str(), options.port),
            handler,
        ),
    )
    .await
    .context("SSH connect timeout")??;
    // Host-key verification has completed before any interactive password prompt.
    let (auth, method) = match options.authentication {
        Authentication::PublicKey {
            identity,
            passphrase_env,
        } => {
            let passphrase = passphrase_env
                .as_ref()
                .map(|name| {
                    std::env::var(name)
                        .with_context(|| format!("SSH key passphrase variable {name} is not set"))
                })
                .transpose()?;
            let key = load_secret_key(&identity, passphrase.as_deref())
                .context("Read SSH identity; encrypted keys require --key-passphrase-env")?;
            let hash = session.best_supported_rsa_hash().await?.flatten();
            (
                session
                    .authenticate_publickey(
                        &options.user,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await?,
                "public-key",
            )
        }
        Authentication::Password { password } => {
            let password = match password {
                Some(password) => password,
                None => {
                    let prompt = format!("{}@{} password: ", options.user, options.host);
                    tokio::task::spawn_blocking(move || rpassword::prompt_password(prompt))
                        .await
                        .context("SSH password prompt task failed")?
                        .context("Read SSH password from terminal")?
                }
            };
            (
                session
                    .authenticate_password(&options.user, password)
                    .await?,
                "password",
            )
        }
    };
    if !auth.success() {
        return Err(AuthenticationError::Rejected { method }.into());
    }
    let mut channel = session.channel_open_session().await?;
    channel
        .request_x11(true, false, "MIT-MAGIC-COOKIE-1", hex, 0)
        .await?;
    wait_success(&mut channel, "SSH server rejected X11 forwarding").await?;
    channel.exec(true, options.command).await?;
    wait_success(&mut channel, "SSH server rejected remote command").await?;
    let mut exit = None;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => {
                std::io::Write::write_all(&mut std::io::stdout().lock(), &data)?
            }
            ChannelMsg::ExtendedData { data, .. } => {
                std::io::Write::write_all(&mut std::io::stderr().lock(), &data)?
            }
            ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
            ChannelMsg::ExitSignal { signal_name, .. } => {
                return Err(anyhow!("Remote command terminated by {signal_name:?}"));
            }
            _ => {}
        }
    }
    session
        .disconnect(
            russh::Disconnect::ByApplication,
            "WayX command complete",
            "",
        )
        .await?;
    // disconnect() only enqueues SSH_MSG_DISCONNECT. Wait for the transport task
    // to flush and finish before dropping the session (notably on Windows TCP).
    let shutdown = tokio::time::timeout(Duration::from_secs(5), session)
        .await
        .context("SSH shutdown timeout")?;
    // russh reports locally requested termination as Error::Disconnect, even
    // after flushing successfully. Other completion failures remain errors.
    if let Err(error) = shutdown
        && !matches!(
            error.downcast_ref::<russh::Error>(),
            Some(russh::Error::Disconnect)
        )
    {
        return Err(error.context("SSH transport shutdown"));
    }
    exit.context("Remote command closed without an exit status")
}
async fn wait_success(channel: &mut Channel<client::Msg>, message: &'static str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::Failure) => return Err(RequestError::Rejected(message).into()),
                Some(ChannelMsg::Data { data }) => {
                    std::io::Write::write_all(&mut std::io::stdout().lock(), &data)?;
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    std::io::Write::write_all(&mut std::io::stderr().lock(), &data)?;
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    return Err(RequestError::Exited(exit_status).into());
                }
                Some(ChannelMsg::Close) | None => {
                    return Err(RequestError::Closed(message).into());
                }
                // Window adjustments and EOF are independent of request replies.
                _ => {}
            }
        }
    })
    .await
    .context("SSH request acknowledgement timeout")?
}
#[cfg(test)]
mod tests {
    use super::*;
    fn setup(big: bool, cookie: &[u8]) -> Vec<u8> {
        let mut h = vec![if big { b'B' } else { b'l' }, 0];
        for value in [11u16, 0, 18, 16, 0] {
            h.extend(if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
        h.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
        h.extend([0, 0]);
        h.extend(cookie);
        h
    }
    #[test]
    fn rejects_unknown_and_changed_ssh_host_keys() {
        use client::Handler;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("known_hosts");
        let key =
            russh::keys::PrivateKey::random(&mut rand_10::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        let other =
            russh::keys::PrivateKey::random(&mut rand_10::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        let mut client = Client {
            host: "test.example".into(),
            port: 22,
            known_hosts: path.clone(),
            display: 1,
            fake_cookie: [0; 16],
            real_cookie: [0; 16],
            permits: Arc::new(Semaphore::new(1)),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let presented = key.public_key().clone().into();
            assert!(client.check_server_key(&presented).await.is_err());
            std::fs::write(
                &path,
                format!("test.example {}\n", key.public_key().to_openssh().unwrap()),
            )
            .unwrap();
            assert!(client.check_server_key(&presented).await.unwrap());
            assert!(
                client
                    .check_server_key(&other.public_key().clone().into())
                    .await
                    .is_err()
            );
        });
    }
    #[test]
    fn validates_cookie_in_both_byte_orders_and_rejects_bad_auth() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            for big in [false, true] {
                let input = setup(big, &[42; 16]);
                let out = validated_setup(&mut &input[..], &[42; 16], &[77; 16])
                    .await
                    .unwrap();
                assert_eq!(&out[..32], &input[..32]);
                assert_eq!(&out[32..], &[77; 16]);
                assert!(
                    validated_setup(&mut &input[..], &[43; 16], &[77; 16])
                        .await
                        .is_err()
                );
            }
            assert!(
                validated_setup(&mut &[0u8; 12][..], &[0; 16], &[0; 16])
                    .await
                    .is_err()
            );
        });
    }
    #[test]
    fn rejects_truncated_xauthority() {
        let path = std::env::temp_dir().join(format!("wayx-xauth-test-{}", std::process::id()));
        std::fs::write(&path, [1, 0, 255]).unwrap();
        assert!(xauthority_cookie(&path, 0).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
