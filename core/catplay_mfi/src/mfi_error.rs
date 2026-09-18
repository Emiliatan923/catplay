use std::io;

#[derive(thiserror::Error, Debug)]
pub enum MfiI2cError {
    #[error("MFI i2c read timeout (reg=0x{reg:02X}, n={n}, tries={tries}, status={status})")]
    ReadTimeout {
        reg: u8,
        n: usize,
        tries: usize,
        status: io::Error,
    },
    #[error("MFI i2c write timeout (reg=0x{reg:02X}, n={n} tries={tries}, status={status})")]
    WriteTimeout {
        reg: u8,
        n: usize,
        tries: usize,
        status: io::Error,
    },

    #[error("MFI I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("MFI signing status: status 0x{status:02X}, error 0x{code:02X}")]
    SigningError { status: u8, code: u8 },

    #[error("MFI operation did not reach status 0x10 before timeout (last status 0x{status:02X}, error 0x{code:02X})")]
    StatusTimeout { status: u8, code: u8 },

    #[error("MFI chip protocol error: {0}")]
    ChipProtocol(String),

    #[error("MFI certificate error: {0}")]
    Certificate(String),

    #[error("MFI signature verification failed: {0}")]
    SignatureVerification(String),

    #[error("MFI challenge length {actual} is invalid for protocol {protocol_major}; expected {expected}")]
    InvalidChallengeLength {
        protocol_major: u8,
        expected: usize,
        actual: usize,
    },

    #[error("unsupported MFI device version 0x{device_version:02X} / protocol {protocol_major}.{protocol_minor}")]
    UnsupportedDevice {
        device_version: u8,
        protocol_major: u8,
        protocol_minor: u8,
    },

    #[error("MFI internal lock was poisoned: {0}")]
    LockPoisoned(&'static str),

    #[error("MFI unexpected data size: {0}")]
    UnexpectedSize(usize),

    #[error("Other MFI error: {0}")]
    Other(String),

    #[error("{0}")]
    Remote(String),
}

impl From<&str> for MfiI2cError {
    fn from(s: &str) -> Self {
        MfiI2cError::Other(s.to_string())
    }
}

impl From<String> for MfiI2cError {
    fn from(s: String) -> Self {
        MfiI2cError::Other(s)
    }
}

pub type MfiResult<T> = Result<T, MfiI2cError>;
