use std::{error::Error, fs, io::ErrorKind};

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
        match fs::read_to_string(&identity_file) {
            Ok(saved) if saved.trim() != identity => {
                return Err(format!(
                    "MFi identity changed from {} to {}; remove {} only when intentionally reprovisioning the device",
                    saved.trim(),
                    identity,
                    identity_file.display()
                )
                .into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                fs::write(&identity_file, format!("{identity}\n"))?;
            }
            Err(error) => return Err(error.into()),
        }
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
