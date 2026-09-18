use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

use log::info;

use crate::{AppConfig, HomeKitManager, MfiManager, ProdGadget, ProdGadgetConfig};
use catplay_util::{AsyncShutdown, EventReconciler, EventSleeper, Reconciler};

#[derive(EventSleeper, EventReconciler)]
#[reconcile_error[()]]
pub struct CarPlayOutputManager {
    enabled: bool,
    #[sleep]
    #[reconcile]
    gadget_manager: Option<Reconciler<ProdGadget>>,
}

fn persist_mfi_identity(identity_file: &Path, identity: &str) -> Result<(), Box<dyn Error>> {
    match fs::read_to_string(identity_file) {
        Ok(saved) if saved.trim().is_empty() => {
            // A power loss between create/truncate and write can leave an empty
            // file. It contains no prior identity to protect, so recover it in
            // exactly the same way as a first boot.
        }
        Ok(saved) if saved.trim() != identity => {
            return Err(format!(
                "MFi identity changed from {} to {}; remove {} only when intentionally reprovisioning the device",
                saved.trim(),
                identity,
                identity_file.display()
            )
            .into());
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let temporary_file = identity_file.with_extension("tmp");
    let mut file = OpenOptions::new().create(true).truncate(true).write(true).open(&temporary_file)?;
    file.write_all(format!("{identity}\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary_file, identity_file)?;
    Ok(())
}

impl CarPlayOutputManager {
    pub fn new() -> Self {
        Self {
            enabled: false,
            gadget_manager: None,
        }
    }

    pub fn start(&mut self, config: &AppConfig, _mfi: &MfiManager, homekit: &HomeKitManager) -> Result<(), Box<dyn Error>> {
        let usb = config.resolved_usb_car();
        if !usb.enabled {
            info!("Gadget manager is disabled");
            return Ok(());
        }

        info!("Creating GadgetManager output!!!");

        let config = config.clone();
        let persist_dir = Some(config.state_dir());
        std::fs::create_dir_all(persist_dir.as_ref().unwrap())?;
        let bt_cache_file = persist_dir
            .as_ref()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "persist_dir is required for bluetooth last-connect cache",
                )
            })?
            .join("catplay_last_connect.txt");
        let identity = _mfi.identity_uuid()?.simple().to_string();
        let identity_file = persist_dir.as_ref().unwrap().join("identity");
        persist_mfi_identity(&identity_file, &identity)?;
        let wifi = config.resolved_wifi();
        let bonjour_id = format!(
            "02:{}:{}:{}:{}:{}",
            &identity[0..2],
            &identity[2..4],
            &identity[4..6],
            &identity[6..8],
            &identity[8..10]
        );
        let cfg = ProdGadgetConfig {
            homekit_rx: homekit.homekit_rx.clone().unwrap(),
            homekit_tx: homekit.homekit_tx.clone().unwrap(),
            mfi: Some(_mfi.get_device()),
            udc_tx: usb.udc,
            udc_rx: config.gadget.udc_extra,
            bt_name: config.bluetooth.name,
            hci: config.bluetooth.device,
            iface: wifi.device,
            wpa: wifi.wpa,
            ssid: wifi.ssid,
            channel: wifi.channel,
            passphrase: wifi.password,
            pinned: usb.pinned,
            iphone_instance: "catplay0".into(),
            iphone_serial: format!("CATPLAY{}", identity.to_ascii_uppercase()),
            bonjour_id,
            bt_cache_file: bt_cache_file.to_str().expect("corrupted file path").into(),
            persist_dir,
        };
        let g = ProdGadget::new(cfg);
        info!("GadgetManager output configured");

        self.gadget_manager = Some(g);
        self.enabled = true;
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        if let Some(mut gadget) = self.gadget_manager.take() {
            gadget.shutdown().await;
        }
        self.enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::persist_mfi_identity;
    use std::{fs, time::SystemTime};

    fn test_dir(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("catplay-{name}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn identity_is_created_on_first_boot() {
        let dir = test_dir("identity-new");
        let path = dir.join("identity");
        persist_mfi_identity(&path, "abc123").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "abc123\n");
        assert!(!path.with_extension("tmp").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_identity_is_recovered() {
        let dir = test_dir("identity-empty");
        let path = dir.join("identity");
        fs::write(&path, "").unwrap();
        persist_mfi_identity(&path, "abc123").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "abc123\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn different_nonempty_identity_is_rejected() {
        let dir = test_dir("identity-mismatch");
        let path = dir.join("identity");
        fs::write(&path, "old-id\n").unwrap();
        let error = persist_mfi_identity(&path, "new-id").unwrap_err();
        assert!(error.to_string().contains("MFi identity changed from old-id to new-id"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old-id\n");
        fs::remove_dir_all(dir).unwrap();
    }
}
