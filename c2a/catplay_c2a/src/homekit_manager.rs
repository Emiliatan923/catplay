use std::error::Error;

use catplay_hap::{HomekitStorageFile, HomekitStorageRef};

use crate::{AppConfig, MfiManager};

#[derive(Default)]
pub struct HomeKitManager {
    pub homekit_tx: Option<HomekitStorageRef>,
    pub homekit_rx: Option<HomekitStorageRef>,
}

impl HomeKitManager {
    pub fn start(&mut self, config: &AppConfig, _mfi: &MfiManager) -> Result<(), Box<dyn Error>> {
        let state_dir = config.state_dir();
        let db_path_tx = state_dir.join("homekit_tx_db.bin");
        let db_path_rx = state_dir.join("homekit_rx_db.bin");

        self.homekit_tx.replace(HomekitStorageFile::file(db_path_tx.to_str().unwrap())?);
        self.homekit_rx.replace(HomekitStorageFile::file(db_path_rx.to_str().unwrap())?);

        Ok(())
    }
}
