use std::{error::Error, sync::Arc, thread::spawn, time::Duration};

use catplay_mfi::{
    MfiDeficeRef, MfiDevice, MfiDeviceI2C,
    server::{MfiDeviceRemoteClient, MfiDeviceServer},
};
use log::info;
use uuid::Uuid;

use crate::AppConfig;

pub struct MfiManager {
    device: Option<Arc<dyn MfiDevice>>,
}

impl MfiManager {
    pub fn new() -> Self {
        Self { device: None }
    }

    fn cert_hash(&self, mfi: &Arc<dyn MfiDevice>) -> Result<Uuid, Box<dyn Error>> {
        let mfi_cert: Vec<u8> = mfi.read_certificate()?;
        Ok(Uuid::new_v5(&Uuid::NAMESPACE_DNS, &mfi_cert))
    }

    pub fn get_device(&self) -> Arc<dyn MfiDevice> {
        self.device.as_ref().unwrap().clone()
    }

    pub fn identity_uuid(&self) -> Result<Uuid, Box<dyn Error>> {
        let device = self.device.as_ref().ok_or("MFi manager has not been started")?;
        self.cert_hash(device)
    }

    pub fn start(&mut self, config: &AppConfig) -> Result<(), Box<dyn Error>> {
        let mfi: MfiDeficeRef = if let Some(mfi_client) = &config.mfi.client {
            Arc::new(MfiDeviceRemoteClient::new(mfi_client.remote.clone())?)
        } else if let Some(mfi_i2c) = &config.mfi.i2c {
            let path = mfi_i2c.device_path()?;
            let address = mfi_i2c.address()?;
            if mfi_i2c.device.is_none() {
                log::warn!("mfi.i2c.bus_offset is deprecated; use device = {:?}", path);
            }
            if mfi_i2c.address.is_none() {
                log::warn!("mfi.i2c.dev_addr is deprecated; use address = 0x{address:02x}");
            }
            Arc::new(MfiDeviceI2C::open(
                path,
                address,
                Duration::from_millis(mfi_i2c.timeout_ms),
                Duration::from_millis(mfi_i2c.retry_delay_ms),
            )?)
        } else {
            return Err("mfi backend not defined".into());
        };

        if config.mfi.selftest {
            info!("Performing MFi self-test");
            let report = mfi.self_test()?;
            let uuid = self.cert_hash(&mfi)?;
            let fingerprint = report.certificate_sha256.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            info!(
                "MFI self-test passed: device={:?}, cert_size={}, cert_sha256={}, signature_size={}, verified={}, identity={}",
                report.device_info, report.certificate_len, fingerprint, report.signature_len, report.signature_verified, uuid
            );
        }

        self.device.replace(mfi.clone());

        if let Some(mfi_server) = &config.mfi.server
            && mfi_server.enabled
        {
            info!("Starting MFI server on {}", mfi_server.bind.clone());

            let mut server = MfiDeviceServer::new(mfi_server.bind.clone(), mfi.clone());
            server.bind().map_err(|err| format!("failed to bind mfi server: {}", err))?;
            spawn(move || server.listen());
            info!("MFI server is now running in background")
        }

        Ok(())
    }
}
