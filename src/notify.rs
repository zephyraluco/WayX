//! Coalesced UI notifications and a socket readable by the embedded X server.
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::Arc,
};

pub type UiWake = Arc<dyn Fn() + Send + Sync>;

pub struct InputWake {
    reader: TcpStream,
    writer: TcpStream,
    pending: bool,
}
impl InputWake {
    pub fn new() -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let writer = TcpStream::connect(listener.local_addr()?)?;
        let (reader, _) = listener.accept()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        writer.set_nodelay(true)?;
        Ok(Self {
            reader,
            writer,
            pending: false,
        })
    }
    pub fn descriptor(&self) -> io::Result<i32> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            Ok(self.reader.as_raw_fd())
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket;
            i32::try_from(self.reader.as_raw_socket())
                .map_err(|_| io::Error::other("X server socket handle exceeds int"))
        }
    }
    pub fn signal(&mut self) -> io::Result<()> {
        if !self.pending {
            match self.writer.write(&[1]) {
                Ok(1) => self.pending = true,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.pending = true,
                Err(e) => return Err(e),
                _ => return Err(io::Error::other("Input notification socket closed")),
            }
        }
        Ok(())
    }
    pub fn drain(&mut self) -> io::Result<()> {
        let mut bytes = [0; 64];
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => return Err(io::Error::other("Input notification socket closed")),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        self.pending = false;
        Ok(())
    }
}

/// Publishes completion even when loading the core or the worker itself fails.
pub struct WorkerExit(pub crate::bridge::SharedBackend);
impl Drop for WorkerExit {
    fn drop(&mut self) {
        if let Ok(mut backend) = self.0.lock() {
            backend.worker_exited = true;
            backend.notify_ui();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_wake_coalesces_and_can_be_rearmed_after_drain() -> io::Result<()> {
        let mut wake = InputWake::new()?;
        for _ in 0..1000 {
            wake.signal()?;
        }
        let mut byte = [0];
        wake.reader.set_nonblocking(false)?;
        wake.reader
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        assert_eq!(wake.reader.peek(&mut byte)?, 1);
        wake.reader.set_nonblocking(true)?;
        wake.drain()?;
        assert_eq!(
            wake.reader.peek(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        wake.signal()?;
        wake.reader.set_nonblocking(false)?;
        assert_eq!(wake.reader.peek(&mut byte)?, 1);
        Ok(())
    }
}
