use std::{error::Error, fs, path::Path};

use crate::{AppConfig, CarPlayOutputManager, HomeKitManager, MfiManager};
// use catplay_iap2_usb_gadget::gadget::GadgetHelper;
use catplay_util::{EventReconciler, EventSleeper};
use log::{error, info};
use tokio::signal;

fn parse_config(path: &Path) -> Result<AppConfig, String> {
    let config_str = fs::read_to_string(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let config: AppConfig = toml::from_str(&config_str).map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    config.validate().map_err(|error| format!("invalid {}: {error}", path.display()))?;
    Ok(config)
}

pub struct Main {
    mfi: Option<MfiManager>,
    homekit: Option<HomeKitManager>,
    output: Option<CarPlayOutputManager>,
}

impl Main {
    pub async fn start(config_path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        let config = parse_config(config_path.as_ref())?;
        fs::create_dir_all(config.state_dir())?;

        let mut mfi = MfiManager::new();
        mfi.start(&config)?;

        let mut homekit = HomeKitManager::default();
        homekit.start(&config, &mfi)?;

        let mut output = CarPlayOutputManager::new();
        output.start(&config, &mfi, &homekit)?;

        Ok(Self {
            mfi: Some(mfi),
            homekit: Some(homekit),
            output: Some(output),
        })
    }

    pub async fn do_loop(&mut self) -> Result<(), Box<dyn Error>> {
        let mut ctrl_c = Box::pin(signal::ctrl_c());

        #[cfg(unix)]
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;

        let result: Result<(), Box<dyn Error>> = loop {
            let Some(output) = self.output.as_mut() else {
                break Err("output manager disappeared".into());
            };

            if output.reconcile().await.is_err() {
                break Err("CarPlay output reconcile failed".into());
            }

            #[cfg(unix)]
            tokio::select! {
                signal = &mut ctrl_c => {
                    signal?;
                    info!("Received SIGINT, shutting down");
                    break Ok(());
                }
                _ = terminate.recv() => {
                    info!("Received SIGTERM, shutting down");
                    break Ok(());
                }
                _ = output.sleep() => {}
            }

            #[cfg(not(unix))]
            tokio::select! {
                signal = &mut ctrl_c => {
                    signal?;
                    info!("Received shutdown signal");
                    break Ok(());
                }
                _ = output.sleep() => {}
            }
        };

        self.shutdown().await;
        result
    }

    pub async fn shutdown(&mut self) {
        info!("Stopping CatPlay managers");
        if let Some(output) = self.output.as_mut() {
            output.shutdown().await;
        }
        self.output.take();
        self.homekit.take();
        self.mfi.take();
        info!("CatPlay shutdown complete");
    }
}

impl Drop for Main {
    fn drop(&mut self) {
        if self.output.is_some() {
            error!("Main dropped before asynchronous shutdown completed");
        }
    }
}
