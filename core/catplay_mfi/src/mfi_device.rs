use std::sync::Arc;

use openssl::sha::sha256;

use crate::{MfiI2cError, MfiResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MfiAuthDigest {
    Sha1,
    Sha256,
}

impl MfiAuthDigest {
    /// Wire encoding used by the remote MFI protocol (command 0x03).
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Sha1 => 0x01,
            Self::Sha256 => 0x02,
        }
    }

    pub const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Sha1),
            0x02 => Some(Self::Sha256),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MfiDeviceInfo {
    pub device_version: u8,
    pub authentication_revision: u8,
    pub protocol_major: u8,
    pub protocol_minor: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MfiSelfTestReport {
    pub device_info: Option<MfiDeviceInfo>,
    pub certificate_len: usize,
    pub certificate_sha256: [u8; 32],
    pub signature_len: usize,
    pub signature_verified: bool,
}

pub trait MfiDevice: Send + Sync {
    fn read_certificate(&self) -> MfiResult<Vec<u8>>;

    fn generate_challenge_response(&self, challenge: &[u8]) -> MfiResult<Vec<u8>>;

    fn device_info(&self) -> MfiResult<Option<MfiDeviceInfo>> {
        Ok(None)
    }

    /// Digest the device wants signed during auth-setup.
    ///
    /// The default matches modern (2.0 and later) Apple coprocessors. Older
    /// 2.0C-era parts must report [`MfiAuthDigest::Sha1`] themselves - never
    /// infer the digest from the certificate length.
    fn authentication_digest(&self) -> MfiResult<MfiAuthDigest> {
        Ok(MfiAuthDigest::Sha256)
    }

    fn self_test(&self) -> MfiResult<MfiSelfTestReport> {
        let certificate = self.read_certificate()?;
        if certificate.is_empty() {
            return Err(MfiI2cError::Certificate("certificate is empty".into()));
        }
        let challenge = [0xA5; 20];
        let signature = self.generate_challenge_response(&challenge)?;
        if signature.is_empty() {
            return Err(MfiI2cError::SignatureVerification("signature is empty".into()));
        }
        Ok(MfiSelfTestReport {
            device_info: self.device_info()?,
            certificate_len: certificate.len(),
            certificate_sha256: sha256(&certificate),
            signature_len: signature.len(),
            signature_verified: false,
        })
    }
}

pub type MfiDeficeRef = Arc<dyn MfiDevice>;
