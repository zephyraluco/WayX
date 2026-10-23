//! Authenticated TCP connection shared by the native geometry/GPU clients.
use anyhow::Result;
use x11rb::rust_connection::{DefaultStream, RustConnection};

pub fn connect_native() -> Result<RustConnection> {
    let display: u16 = std::env::var("WAYX_GEOMETRY_DISPLAY")?.parse()?;
    let cookie = wayx::auth::xauthority_cookie(
        &std::path::PathBuf::from(std::env::var("WAYX_GEOMETRY_AUTH")?),
        display,
    )?;
    let stream = DefaultStream::from_tcp_stream(std::net::TcpStream::connect((
        std::net::Ipv4Addr::LOCALHOST,
        6000 + display,
    ))?)?
    .0;
    Ok(RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        b"MIT-MAGIC-COOKIE-1".to_vec(),
        cookie.to_vec(),
    )?)
}
