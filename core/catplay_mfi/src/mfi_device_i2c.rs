use log::debug;
use openssl::{pkcs7::Pkcs7, rand::rand_bytes, rsa::Padding, sha::sha256, x509::X509};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::{MfiAuthDigest, MfiDevice, MfiDeviceInfo, MfiI2cError, MfiResult, MfiSelfTestReport};

const I2C_RDWR: libc::c_ulong = 0x0707;
const I2C_M_RD: u16 = 0x0001;

const REG_DEVICE_VERSION: u8 = 0x00;
const REG_AUTH_REVISION: u8 = 0x01;
const REG_PROTOCOL_MAJOR: u8 = 0x02;
const REG_PROTOCOL_MINOR: u8 = 0x03;
const REG_ERROR_CODE: u8 = 0x05;
const REG_AUTH_CONTROL_STATUS: u8 = 0x10;
const REG_RESPONSE_LENGTH: u8 = 0x11;
const REG_RESPONSE_DATA: u8 = 0x12;
const REG_CHALLENGE_LENGTH: u8 = 0x20;
const REG_CHALLENGE_DATA: u8 = 0x21;
const REG_CERTIFICATE_LENGTH: u8 = 0x30;
const REG_CERTIFICATE_PAGE_1: u8 = 0x31;
const REG_SELF_TEST: u8 = 0x40;

const DEVICE_VERSION_2_0C: u8 = 0x05;
const PROTOCOL_2: u8 = 2;
const PROTOCOL_2_CHALLENGE_LEN: usize = 20;
const PROTOCOL_2_CERT_MAX: usize = 1280;
const CERT_PAGE_LEN: usize = 128;
const RESPONSE_MAX: usize = 128;
const AUTH_STATUS_SUCCESS: u8 = 0x10;

#[repr(C)]
struct I2cMessage {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

#[repr(C)]
struct I2cRdwrData {
    msgs: *mut I2cMessage,
    nmsgs: u32,
}

fn checked_i2c_len(len: usize) -> io::Result<u16> {
    u16::try_from(len).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "I2C message exceeds 65535 bytes"))
}

fn transfer(fd: RawFd, messages: &mut [I2cMessage]) -> io::Result<()> {
    let mut data = I2cRdwrData {
        msgs: messages.as_mut_ptr(),
        nmsgs: u32::try_from(messages.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many I2C messages"))?,
    };

    // SAFETY: every message and backing buffer remains alive until ioctl returns.
    let result = unsafe { libc::ioctl(fd, I2C_RDWR, &mut data) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if result as usize != messages.len() {
        return Err(io::Error::other(format!(
            "incomplete I2C transfer: {result}/{} messages",
            messages.len()
        )));
    }
    Ok(())
}

fn write_transaction(file: &File, address: u8, data: &[u8]) -> io::Result<()> {
    let mut message = I2cMessage {
        addr: u16::from(address),
        flags: 0,
        len: checked_i2c_len(data.len())?,
        buf: data.as_ptr().cast_mut(),
    };
    transfer(file.as_raw_fd(), std::slice::from_mut(&mut message))
}

fn read_transaction(file: &File, address: u8, data: &mut [u8]) -> io::Result<()> {
    let mut message = I2cMessage {
        addr: u16::from(address),
        flags: I2C_M_RD,
        len: checked_i2c_len(data.len())?,
        buf: data.as_mut_ptr(),
    };
    transfer(file.as_raw_fd(), std::slice::from_mut(&mut message))
}

#[cfg(target_os = "linux")]
fn is_transient_i2c_error(error: &io::Error) -> bool {
    // The 3959 NAKs regularly - it does not even answer I2C quick-write probing
    // - and the rk3x controller reports that as ETIMEDOUT rather than
    // EREMOTEIO, so every "the slave did not answer this time" errno has to be
    // retried instead of aborting the transfer.
    error.raw_os_error().is_some_and(|code| {
        matches!(
            code,
            libc::EINTR | libc::EAGAIN | libc::EIO | libc::ENXIO | libc::EREMOTEIO | libc::ETIMEDOUT
        )
    })
}

#[cfg(not(target_os = "linux"))]
fn is_transient_i2c_error(_error: &io::Error) -> bool {
    false
}

pub trait MfiI2cTransport: Send {
    fn read_register(&mut self, register: u8, data: &mut [u8]) -> MfiResult<()>;
    fn write_register(&mut self, register: u8, data: &[u8]) -> MfiResult<()>;
}

pub struct LinuxI2cTransport {
    file: File,
    address: u8,
    timeout: Duration,
    retry_delay: Duration,
}

impl LinuxI2cTransport {
    pub fn open(path: impl AsRef<Path>, address: u8, timeout: Duration, retry_delay: Duration) -> MfiResult<Self> {
        if !matches!(address, 0x10 | 0x11) {
            return Err(MfiI2cError::Other(format!(
                "MFi address must be the configured 0x10 or 0x11, got 0x{address:02x}"
            )));
        }
        if timeout.is_zero() || retry_delay.is_zero() {
            return Err(MfiI2cError::Other("I2C timeout and retry delay must be non-zero".into()));
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self {
            file,
            address,
            timeout,
            retry_delay,
        })
    }

    fn retry_io<F>(&self, register: u8, size: usize, read: bool, mut operation: F) -> MfiResult<()>
    where
        F: FnMut() -> io::Result<()>,
    {
        let deadline = Instant::now() + self.timeout;
        let mut tries = 0;
        loop {
            tries += 1;
            match operation() {
                Ok(()) => return Ok(()),
                Err(error) if is_transient_i2c_error(&error) && Instant::now() < deadline => sleep(self.retry_delay),
                Err(error) if is_transient_i2c_error(&error) => {
                    return Err(if read {
                        MfiI2cError::ReadTimeout {
                            reg: register,
                            n: size,
                            tries,
                            status: error,
                        }
                    } else {
                        MfiI2cError::WriteTimeout {
                            reg: register,
                            n: size,
                            tries,
                            status: error,
                        }
                    });
                }
                Err(error) => return Err(MfiI2cError::Io(error)),
            }
        }
    }
}

impl MfiI2cTransport for LinuxI2cTransport {
    fn read_register(&mut self, register: u8, data: &mut [u8]) -> MfiResult<()> {
        debug!("MFi read register 0x{register:02X}, {} bytes", data.len());

        // Apple HomeKitADK's Raspberry Pi backend uses a STOP between selecting
        // the register and reading it. Keep the two messages as separate
        // transfers so both bcm2835 I2C and hid-mcp2221 can use this backend.
        self.retry_io(register, 1, false, || write_transaction(&self.file, self.address, &[register]))?;

        // The chip needs a moment after the pointer write before it answers the
        // read; the working smbus2 reference pauses the same amount here and
        // reads without a settle time fail with ETIMEDOUT on this bus.
        sleep(self.retry_delay);

        self.retry_io(register, data.len(), true, || read_transaction(&self.file, self.address, data))
    }

    fn write_register(&mut self, register: u8, data: &[u8]) -> MfiResult<()> {
        debug!("MFi write register 0x{register:02X}, {} bytes", data.len());
        let mut buffer = Vec::with_capacity(data.len() + 1);
        buffer.push(register);
        buffer.extend_from_slice(data);
        self.retry_io(register, data.len(), false, || write_transaction(&self.file, self.address, &buffer))
    }
}

pub struct MfiDeviceI2C {
    transport: Mutex<Box<dyn MfiI2cTransport>>,
    timeout: Duration,
    poll_delay: Duration,
    info: Mutex<Option<MfiDeviceInfo>>,
    certificate: Mutex<Vec<u8>>,
}

impl MfiDeviceI2C {
    /// Legacy constructor retained for existing firmware configurations.
    pub fn new(bus_offset: u32, address: u8) -> MfiResult<Self> {
        Self::open(
            format!("/dev/i2c-{bus_offset}"),
            address,
            Duration::from_secs(2),
            Duration::from_millis(5),
        )
    }

    pub fn open(path: impl AsRef<Path>, address: u8, timeout: Duration, retry_delay: Duration) -> MfiResult<Self> {
        let transport = LinuxI2cTransport::open(path, address, timeout, retry_delay)?;
        Ok(Self::from_transport(Box::new(transport), timeout, retry_delay))
    }

    pub fn from_transport(transport: Box<dyn MfiI2cTransport>, timeout: Duration, poll_delay: Duration) -> Self {
        Self {
            transport: Mutex::new(transport),
            timeout,
            poll_delay,
            info: Mutex::new(None),
            certificate: Mutex::new(Vec::new()),
        }
    }

    fn read_exact(transport: &mut dyn MfiI2cTransport, register: u8, size: usize) -> MfiResult<Vec<u8>> {
        let mut data = vec![0; size];
        transport.read_register(register, &mut data)?;
        Ok(data)
    }

    fn read_error(transport: &mut dyn MfiI2cTransport) -> MfiResult<u8> {
        Ok(Self::read_exact(transport, REG_ERROR_CODE, 1)?[0])
    }

    fn ensure_no_chip_error(transport: &mut dyn MfiI2cTransport, context: &str) -> MfiResult<()> {
        let code = Self::read_error(transport)?;
        if code == 0 {
            Ok(())
        } else {
            Err(MfiI2cError::ChipProtocol(format!("{context}: chip error 0x{code:02x}")))
        }
    }

    fn read_device_info_locked(transport: &mut dyn MfiI2cTransport) -> MfiResult<MfiDeviceInfo> {
        // Reading the error code first also clears any stale error from a
        // previous interrupted transaction on 2.0C.
        let _ = Self::read_error(transport)?;
        let info = MfiDeviceInfo {
            device_version: Self::read_exact(transport, REG_DEVICE_VERSION, 1)?[0],
            authentication_revision: Self::read_exact(transport, REG_AUTH_REVISION, 1)?[0],
            protocol_major: Self::read_exact(transport, REG_PROTOCOL_MAJOR, 1)?[0],
            protocol_minor: Self::read_exact(transport, REG_PROTOCOL_MINOR, 1)?[0],
        };
        Self::ensure_no_chip_error(transport, "reading device information")?;
        if info.device_version != DEVICE_VERSION_2_0C || info.protocol_major != PROTOCOL_2 {
            return Err(MfiI2cError::UnsupportedDevice {
                device_version: info.device_version,
                protocol_major: info.protocol_major,
                protocol_minor: info.protocol_minor,
            });
        }
        Ok(info)
    }

    pub fn i2c_device_info(&self) -> MfiResult<MfiDeviceInfo> {
        if let Some(info) = self.info.lock().map_err(|_| MfiI2cError::LockPoisoned("device info"))?.clone() {
            return Ok(info);
        }
        let mut transport = self.transport.lock().map_err(|_| MfiI2cError::LockPoisoned("I2C transport"))?;
        let info = Self::read_device_info_locked(transport.as_mut())?;
        *self.info.lock().map_err(|_| MfiI2cError::LockPoisoned("device info"))? = Some(info.clone());
        Ok(info)
    }

    pub fn i2c_read_certificate(&self) -> MfiResult<Vec<u8>> {
        let mut cached = self.certificate.lock().map_err(|_| MfiI2cError::LockPoisoned("certificate cache"))?;
        if !cached.is_empty() {
            return Ok(cached.clone());
        }

        let info = self.i2c_device_info()?;
        let mut transport = self.transport.lock().map_err(|_| MfiI2cError::LockPoisoned("I2C transport"))?;
        let length = Self::read_exact(transport.as_mut(), REG_CERTIFICATE_LENGTH, 2)?;
        let size = u16::from_be_bytes([length[0], length[1]]) as usize;
        if info.protocol_major != PROTOCOL_2 || size == 0 || size > PROTOCOL_2_CERT_MAX {
            return Err(MfiI2cError::UnexpectedSize(size));
        }

        let mut certificate = Vec::with_capacity(size);
        let pages = size.div_ceil(CERT_PAGE_LEN);
        for page in 0..pages {
            let chunk_len = (size - certificate.len()).min(CERT_PAGE_LEN);
            let register = REG_CERTIFICATE_PAGE_1
                .checked_add(u8::try_from(page).map_err(|_| MfiI2cError::UnexpectedSize(size))?)
                .ok_or(MfiI2cError::UnexpectedSize(size))?;
            certificate.extend_from_slice(&Self::read_exact(transport.as_mut(), register, chunk_len)?);
        }
        Self::ensure_no_chip_error(transport.as_mut(), "reading certificate")?;
        validate_certificate(&certificate)?;
        *cached = certificate.clone();
        Ok(certificate)
    }

    pub fn i2c_generate_challenge_response(&self, challenge: &[u8]) -> MfiResult<Vec<u8>> {
        let info = self.i2c_device_info()?;
        if challenge.len() != PROTOCOL_2_CHALLENGE_LEN {
            return Err(MfiI2cError::InvalidChallengeLength {
                protocol_major: info.protocol_major,
                expected: PROTOCOL_2_CHALLENGE_LEN,
                actual: challenge.len(),
            });
        }

        let mut transport = self.transport.lock().map_err(|_| MfiI2cError::LockPoisoned("I2C transport"))?;
        let _ = Self::read_error(transport.as_mut())?;
        transport.write_register(REG_CHALLENGE_LENGTH, &(challenge.len() as u16).to_be_bytes())?;
        transport.write_register(REG_CHALLENGE_DATA, challenge)?;
        transport.write_register(REG_RESPONSE_LENGTH, &(RESPONSE_MAX as u16).to_be_bytes())?;
        transport.write_register(REG_AUTH_CONTROL_STATUS, &[0x01])?;

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            let status = Self::read_exact(transport.as_mut(), REG_AUTH_CONTROL_STATUS, 1)?[0];
            if status == AUTH_STATUS_SUCCESS {
                break status;
            }
            if status & 0x80 != 0 {
                let code = Self::read_error(transport.as_mut())?;
                return Err(MfiI2cError::SigningError { status, code });
            }
            if Instant::now() >= deadline {
                let code = Self::read_error(transport.as_mut())?;
                return Err(MfiI2cError::StatusTimeout { status, code });
            }
            sleep(self.poll_delay);
        };

        let length = Self::read_exact(transport.as_mut(), REG_RESPONSE_LENGTH, 2)?;
        let size = u16::from_be_bytes([length[0], length[1]]) as usize;
        if !(1..=RESPONSE_MAX).contains(&size) {
            return Err(MfiI2cError::UnexpectedSize(size));
        }
        let response = Self::read_exact(transport.as_mut(), REG_RESPONSE_DATA, size)?;
        let code = Self::read_error(transport.as_mut())?;
        if code != 0 {
            return Err(MfiI2cError::SigningError { status, code });
        }
        Ok(response)
    }

    fn run_self_test(&self) -> MfiResult<MfiSelfTestReport> {
        let info = self.i2c_device_info()?;
        {
            let mut transport = self.transport.lock().map_err(|_| MfiI2cError::LockPoisoned("I2C transport"))?;
            transport.write_register(REG_SELF_TEST, &[0x01])?;
            let status = Self::read_exact(transport.as_mut(), REG_SELF_TEST, 1)?[0];
            if status & 0xC0 != 0xC0 {
                return Err(MfiI2cError::ChipProtocol(format!(
                    "self-test 0x{status:02x}: certificate/private-key bits are not both set"
                )));
            }
            Self::ensure_no_chip_error(transport.as_mut(), "running self-test")?;
        }

        let certificate = self.i2c_read_certificate()?;
        let mut challenge = [0u8; PROTOCOL_2_CHALLENGE_LEN];
        rand_bytes(&mut challenge).map_err(|error| MfiI2cError::Other(format!("random challenge generation failed: {error}")))?;
        let signature = self.i2c_generate_challenge_response(&challenge)?;
        verify_protocol2_signature(&certificate, &challenge, &signature)?;

        Ok(MfiSelfTestReport {
            device_info: Some(info),
            certificate_len: certificate.len(),
            certificate_sha256: sha256(&certificate),
            signature_len: signature.len(),
            signature_verified: true,
        })
    }
}

impl MfiDevice for MfiDeviceI2C {
    fn read_certificate(&self) -> MfiResult<Vec<u8>> {
        self.i2c_read_certificate()
    }

    fn generate_challenge_response(&self, challenge: &[u8]) -> MfiResult<Vec<u8>> {
        self.i2c_generate_challenge_response(challenge)
    }

    fn device_info(&self) -> MfiResult<Option<MfiDeviceInfo>> {
        self.i2c_device_info().map(Some)
    }

    fn authentication_digest(&self) -> MfiResult<MfiAuthDigest> {
        let info = self.i2c_device_info()?;
        match info.protocol_major {
            PROTOCOL_2 => Ok(MfiAuthDigest::Sha1),
            _ => Ok(MfiAuthDigest::Sha256),
        }
    }

    fn self_test(&self) -> MfiResult<MfiSelfTestReport> {
        self.run_self_test()
    }
}

fn certificate_leaf(certificate: &[u8]) -> MfiResult<X509> {
    if let Ok(certificate) = X509::from_der(certificate) {
        return Ok(certificate);
    }
    let bundle = Pkcs7::from_der(certificate).map_err(|error| MfiI2cError::Certificate(error.to_string()))?;
    let certificates = bundle
        .signed()
        .and_then(|signed| signed.certificates())
        .ok_or_else(|| MfiI2cError::Certificate("PKCS#7 bundle contains no certificates".into()))?;
    certificates
        .get(0)
        .map(ToOwned::to_owned)
        .ok_or_else(|| MfiI2cError::Certificate("PKCS#7 bundle contains an empty certificate stack".into()))
}

fn validate_certificate(certificate: &[u8]) -> MfiResult<()> {
    let _ = certificate_leaf(certificate)?;
    Ok(())
}

fn verify_protocol2_signature(certificate: &[u8], digest: &[u8], signature: &[u8]) -> MfiResult<()> {
    const SHA1_DIGEST_INFO_PREFIX: &[u8] = &[
        0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2B, 0x0E, 0x03, 0x02, 0x1A, 0x05, 0x00, 0x04, 0x14,
    ];

    let key = certificate_leaf(certificate)?
        .public_key()
        .map_err(|error| MfiI2cError::Certificate(error.to_string()))?;
    let rsa = key.rsa().map_err(|error| MfiI2cError::Certificate(error.to_string()))?;
    let mut decoded = vec![0; rsa.size() as usize];
    let size = rsa
        .public_decrypt(signature, &mut decoded, Padding::PKCS1)
        .map_err(|error| MfiI2cError::SignatureVerification(error.to_string()))?;
    decoded.truncate(size);

    let mut expected = Vec::with_capacity(SHA1_DIGEST_INFO_PREFIX.len() + digest.len());
    expected.extend_from_slice(SHA1_DIGEST_INFO_PREFIX);
    expected.extend_from_slice(digest);
    if decoded != expected {
        return Err(MfiI2cError::SignatureVerification(
            "RSA digest info does not match the challenge".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;

    struct MockTransport {
        reads: HashMap<u8, VecDeque<Vec<u8>>>,
        writes: Arc<Mutex<Vec<(u8, Vec<u8>)>>>,
    }

    impl MockTransport {
        fn protocol2() -> (Self, Arc<Mutex<Vec<(u8, Vec<u8>)>>>) {
            let mut reads = HashMap::new();
            reads.insert(REG_ERROR_CODE, VecDeque::from(vec![vec![0]; 8]));
            reads.insert(REG_DEVICE_VERSION, VecDeque::from([vec![DEVICE_VERSION_2_0C]]));
            reads.insert(REG_AUTH_REVISION, VecDeque::from([vec![1]]));
            reads.insert(REG_PROTOCOL_MAJOR, VecDeque::from([vec![PROTOCOL_2]]));
            reads.insert(REG_PROTOCOL_MINOR, VecDeque::from([vec![0]]));
            reads.insert(REG_AUTH_CONTROL_STATUS, VecDeque::from([vec![1], vec![AUTH_STATUS_SUCCESS]]));
            reads.insert(REG_RESPONSE_LENGTH, VecDeque::from([vec![0, 4]]));
            reads.insert(REG_RESPONSE_DATA, VecDeque::from([vec![1, 2, 3, 4]]));
            let writes = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    reads,
                    writes: writes.clone(),
                },
                writes,
            )
        }
    }

    impl MfiI2cTransport for MockTransport {
        fn read_register(&mut self, register: u8, data: &mut [u8]) -> MfiResult<()> {
            let value = self
                .reads
                .get_mut(&register)
                .and_then(VecDeque::pop_front)
                .ok_or_else(|| MfiI2cError::Other(format!("unexpected read 0x{register:02x}")))?;
            if value.len() != data.len() {
                return Err(MfiI2cError::UnexpectedSize(value.len()));
            }
            data.copy_from_slice(&value);
            Ok(())
        }

        fn write_register(&mut self, register: u8, data: &[u8]) -> MfiResult<()> {
            self.writes.lock().unwrap().push((register, data.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn protocol2_signing_polls_and_programs_expected_length() {
        let (transport, writes) = MockTransport::protocol2();
        let device = MfiDeviceI2C::from_transport(Box::new(transport), Duration::from_millis(50), Duration::from_millis(1));
        let response = device.generate_challenge_response(&[0xA5; 20]).unwrap();
        assert_eq!(response, vec![1, 2, 3, 4]);
        assert_eq!(
            *writes.lock().unwrap(),
            vec![
                (REG_CHALLENGE_LENGTH, vec![0, 20]),
                (REG_CHALLENGE_DATA, vec![0xA5; 20]),
                (REG_RESPONSE_LENGTH, vec![0, RESPONSE_MAX as u8]),
                (REG_AUTH_CONTROL_STATUS, vec![1]),
            ]
        );
    }

    #[test]
    fn protocol2_rejects_wrong_digest_length() {
        let (transport, _) = MockTransport::protocol2();
        let device = MfiDeviceI2C::from_transport(Box::new(transport), Duration::from_millis(50), Duration::from_millis(1));
        assert!(matches!(
            device.generate_challenge_response(&[0xA5; 32]),
            Err(MfiI2cError::InvalidChallengeLength {
                expected: 20,
                actual: 32,
                ..
            })
        ));
    }
}
