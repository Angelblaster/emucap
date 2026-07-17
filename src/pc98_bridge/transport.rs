use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use super::{BridgeError, BridgeResult};

pub trait GdbTransport {
    fn send(&mut self, payload: &str) -> BridgeResult<String>;
    fn send_no_reply(&mut self, payload: &str) -> BridgeResult<()>;
    fn interrupt(&mut self) -> BridgeResult<String>;
    /// 직전 write 없이 다음 RSP 패킷을 blocking으로 읽는다. `send_cmd`가 stale async stop을
    /// 실제 응답 앞에서 걷어낸 뒤 진짜 응답을 이어 읽을 때 쓴다.
    fn recv_reply(&mut self) -> BridgeResult<String> {
        Err(BridgeError::Emulator("recv_reply unsupported".into()))
    }
    fn get_timeout(&self) -> BridgeResult<Duration> {
        Ok(Duration::from_secs(5))
    }
    fn set_timeout(&mut self, _timeout: Duration) -> BridgeResult<()> {
        Ok(())
    }
    fn recv_nonblocking(&mut self) -> BridgeResult<Option<String>> {
        Ok(None)
    }
}

pub struct GdbRspClient {
    stream: TcpStream,
    buf: VecDeque<u8>,
}

impl GdbRspClient {
    pub fn connect(
        host: &str,
        port: u16,
        timeout: Duration,
        connect_wait: Duration,
    ) -> std::io::Result<Self> {
        let deadline = Instant::now() + connect_wait;
        loop {
            match TcpStream::connect((host, port)) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(timeout))?;
                    stream.set_write_timeout(Some(timeout))?;
                    return Ok(Self {
                        stream,
                        buf: VecDeque::new(),
                    });
                }
                Err(err) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(300));
                    if err.kind() == std::io::ErrorKind::InvalidInput {
                        return Err(err);
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn checksum(payload: &[u8]) -> u8 {
        payload.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
    }

    fn frame(payload: &str) -> Vec<u8> {
        let data = payload.as_bytes();
        let mut out = Vec::with_capacity(data.len() + 4);
        out.push(b'$');
        out.extend_from_slice(data);
        out.push(b'#');
        out.extend_from_slice(format!("{:02x}", Self::checksum(data)).as_bytes());
        out
    }

    fn read_byte(&mut self) -> std::io::Result<u8> {
        if let Some(b) = self.buf.pop_front() {
            return Ok(b);
        }
        let mut chunk = [0u8; 4096];
        let n = self.stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "GDB connection closed",
            ));
        }
        self.buf.extend(&chunk[..n]);
        Ok(self.buf.pop_front().expect("buffer was just filled"))
    }

    fn write_packet(&mut self, payload: &str) -> std::io::Result<()> {
        let frame = Self::frame(payload);
        self.stream.write_all(&frame)?;
        for _ in 0..8 {
            match self.read_byte()? {
                b'+' => return Ok(()),
                b'-' => self.stream.write_all(&frame)?,
                b'$' => {
                    self.buf.push_front(b'$');
                    return Ok(());
                }
                _ => {}
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "GDB packet was not acknowledged",
        ))
    }

    fn read_packet(&mut self) -> std::io::Result<String> {
        while self.read_byte()? != b'$' {}

        let mut raw = Vec::new();
        loop {
            let b = self.read_byte()?;
            if b == b'#' {
                break;
            }
            raw.push(b);
        }
        let mut checksum = [0u8; 2];
        checksum[0] = self.read_byte()?;
        checksum[1] = self.read_byte()?;
        let expected = std::str::from_utf8(&checksum)
            .ok()
            .and_then(|s| u8::from_str_radix(s, 16).ok());
        if expected != Some(Self::checksum(&raw)) {
            let _ = self.stream.write_all(b"-");
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "GDB packet checksum mismatch",
            ));
        }
        self.stream.write_all(b"+")?;

        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'}' && i + 1 < raw.len() {
                out.push(raw[i + 1] ^ 0x20);
                i += 2;
            } else {
                out.push(raw[i]);
                i += 1;
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

impl GdbTransport for GdbRspClient {
    fn send(&mut self, payload: &str) -> BridgeResult<String> {
        self.write_packet(payload)?;
        Ok(self.read_packet()?)
    }

    fn send_no_reply(&mut self, payload: &str) -> BridgeResult<()> {
        self.write_packet(payload)?;
        Ok(())
    }

    fn recv_reply(&mut self) -> BridgeResult<String> {
        Ok(self.read_packet()?)
    }

    fn interrupt(&mut self) -> BridgeResult<String> {
        self.stream.write_all(&[0x03])?;
        std::thread::sleep(Duration::from_millis(10));
        self.send("?")
    }

    fn get_timeout(&self) -> BridgeResult<Duration> {
        Ok(self
            .stream
            .read_timeout()?
            .unwrap_or(Duration::from_secs(5)))
    }

    fn set_timeout(&mut self, timeout: Duration) -> BridgeResult<()> {
        self.stream.set_read_timeout(Some(timeout))?;
        self.stream.set_write_timeout(Some(timeout))?;
        Ok(())
    }

    fn recv_nonblocking(&mut self) -> BridgeResult<Option<String>> {
        let previous = self.stream.read_timeout()?;
        self.stream.set_nonblocking(true)?;
        let read = {
            let mut chunk = [0u8; 4096];
            match self.stream.read(&mut chunk) {
                Ok(0) => Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "GDB connection closed",
                )),
                Ok(n) => {
                    self.buf.extend(&chunk[..n]);
                    Ok(())
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
                Err(err) => Err(err),
            }
        };
        self.stream.set_nonblocking(false)?;
        self.stream.set_read_timeout(previous)?;
        read?;
        if !self.buf.iter().any(|b| *b == b'$') {
            return Ok(None);
        }
        Ok(Some(self.read_packet()?))
    }
}
