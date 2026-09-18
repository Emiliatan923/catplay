use log::{debug, warn};
use socket2::{Domain, SockAddr, Socket, Type};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::MfiDevice;

fn handle_mfi_server_client(mut stream: TcpStream, mfi_device: Arc<dyn MfiDevice>) {
    let mut buffer = [0; 1024];

    loop {
        if stream.read_exact(&mut buffer[..3]).is_err() {
            debug!("Client disconnected");
            break;
        }

        let cmd = buffer[0];
        let len = u16::from_be_bytes([buffer[1], buffer[2]]) as usize;

        if len > buffer.len() || stream.read_exact(&mut buffer[..len]).is_err() {
            warn!("Failed to parse payload");
            break;
        }

        let payload = &buffer[..len];
        let response = match cmd {
            0x01 => {
                // Read certificate
                debug!("Responding to certificate request");
                mfi_device.read_certificate()
            }
            0x02 => {
                // Generate challenge response
                let start = Instant::now();
                debug!("Generating challenge-response: started");
                let buf = mfi_device.generate_challenge_response(payload);
                debug!("Generating challenge-response: done in {}ms", start.elapsed().as_millis());
                buf
            }
            0x03 => {
                // Authentication digest selected by the chip itself. Clients
                // require this instead of guessing from the certificate length,
                // so a server without it fails loudly rather than silently
                // signing with the wrong digest.
                debug!("Responding to authentication digest request");
                mfi_device.authentication_digest().map(|digest| vec![digest.to_wire()])
            }
            _ => Err("Unknown command".into()),
        };

        match response {
            Ok(data) => {
                let resp_len = (data.len() as u16).to_be_bytes();
                let _ = stream.write_all(&[0u8]);
                let _ = stream.write_all(&resp_len);
                let _ = stream.write_all(&data);
            }
            Err(e) => {
                warn!("Failed to process MFI command: {}", e);
                let e = e.to_string();
                let resp_len = (e.len() as u16).to_be_bytes();
                let _ = stream.write_all(&[1u8]);
                let _ = stream.write_all(&resp_len);
                let _ = stream.write_all(e.as_bytes());
            }
        }
    }
}

pub struct MfiDeviceServer {
    addr: String,
    device: Arc<dyn MfiDevice>,
    listener: Option<TcpListener>,
}

impl MfiDeviceServer {
    pub fn new(addr: String, device: Arc<dyn MfiDevice>) -> Self {
        Self {
            addr,
            device,
            listener: None,
        }
    }

    pub fn bind(&mut self) -> Result<(), io::Error> {
        let mut last_err = None;

        for addr in self.addr.to_socket_addrs()? {
            let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
            socket.set_reuse_address(true)?;

            let sock_addr = SockAddr::from(addr);
            match socket.bind(&sock_addr) {
                Ok(()) => {
                    socket.listen(128)?;
                    self.listener = Some(socket.into());
                    return Ok(());
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "failed to resolve bind address")))
    }

    pub fn listen(&mut self) -> Result<(), io::Error> {
        let listener = self.listener.take().unwrap();
        debug!("Server listening on {}", self.addr);

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let mfi_clone = self.device.clone();
                    thread::spawn(move || {
                        handle_mfi_server_client(stream, mfi_clone);
                    });
                }
                Err(e) => {
                    warn!("Server error: {}", e);
                }
            }
        }

        Ok(())
    }
}
