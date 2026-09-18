use log::debug;
use std::{
    io::{Read, Write},
    net::ToSocketAddrs,
    sync::Mutex,
};
use std::{net::TcpStream, time::Duration};

use crate::{MfiAuthDigest, MfiDevice, MfiI2cError, MfiResult};

fn send_command(stream: &mut TcpStream, cmd: u8, payload: &[u8]) -> MfiResult<Vec<u8>> {
    let len = (payload.len() as u16).to_be_bytes();
    stream.write_all(&[cmd])?;
    stream.write_all(&len)?;
    stream.write_all(payload)?;

    let mut status_buf = [0u8; 1];
    stream.read_exact(&mut status_buf)?;
    let ok = status_buf[0] == 0;

    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf)?;
    let resp_len = u16::from_be_bytes(len_buf) as usize;

    let mut resp = vec![0u8; resp_len];
    stream.read_exact(&mut resp)?;

    if !ok {
        let msg = unsafe { String::from_utf8_unchecked(resp) };
        return Err(MfiI2cError::Remote(msg));
    }

    Ok(resp)
}

fn connect(addr: &str) -> MfiResult<TcpStream> {
    let socket_addr = addr.to_socket_addrs()?.next().unwrap();
    let stream = TcpStream::connect_timeout(&socket_addr, Duration::from_millis(1000))?;
    stream.set_write_timeout(Some(Duration::from_millis(1000)))?;
    stream.set_read_timeout(Some(Duration::from_millis(5000)))?;
    Ok(stream)
}

pub struct MfiDeviceRemoteClient {
    addr: String,
    certificate: Mutex<Vec<u8>>,
}

impl MfiDeviceRemoteClient {
    pub fn new(addr: String) -> MfiResult<Self> {
        let s = Self {
            addr,
            certificate: Mutex::new(Vec::new()),
        };

        /*debug!("Reading MFI certificate...");
        s.certificate = Some(s.read_certificate()?); */
        Ok(s)
    }
}

impl MfiDevice for MfiDeviceRemoteClient {
    fn read_certificate(&self) -> MfiResult<Vec<u8>> {
        let mut cached_cert = self.certificate.lock().unwrap();
        if !cached_cert.is_empty() {
            return Ok(cached_cert.clone());
        }

        let mut stream = connect(&self.addr)?;

        debug!("Sending MFI remote command: read certificate");
        let ret = send_command(&mut stream, 0x01, &[])?;
        debug!("Sending MFI remote command: read certificate: OK");

        *cached_cert = ret.clone();
        Ok(ret)
    }

    fn generate_challenge_response(&self, challenge: &[u8]) -> MfiResult<Vec<u8>> {
        debug!("Sending MFI remote command: generate challenge response");
        let mut stream = connect(&self.addr)?;

        let ret = send_command(&mut stream, 0x02, challenge)?;
        debug!("Sending MFI remote command: generate challenge response: OK");
        Ok(ret)
    }

    /// Ask the remote server which digest its chip wants.
    ///
    /// This deliberately does not fall back to inspecting the certificate
    /// length: a server too old to answer command 0x03 makes auth-setup fail
    /// here instead of silently signing with the wrong digest later.
    fn authentication_digest(&self) -> MfiResult<MfiAuthDigest> {
        debug!("Sending MFI remote command: authentication digest");
        let mut stream = connect(&self.addr)?;
        let response = send_command(&mut stream, 0x03, &[])?;
        let code = *response
            .first()
            .ok_or_else(|| MfiI2cError::Remote("MFI server returned an empty authentication digest response".into()))?;
        MfiAuthDigest::from_wire(code)
            .ok_or_else(|| MfiI2cError::Remote(format!("MFI server returned unknown authentication digest 0x{code:02x}")))
    }
}
