use std::time::Duration;

use async_trait::async_trait;
use catplay_iap2_usb::{GadgetError, GadgetResult};
use catplay_util::{EventReconciler, EventSleeper};

use crate::{AccessoryData, GadgetStatus, PhoneGadget};

/// Platform boundary for the car-facing dual-role USB link.
///
/// Raspberry Pi uses `g_iphone` for device mode and `iap2_scan` after the
/// kernel role switch.  Tests may implement this trait without either driver.
#[async_trait]
pub trait CarUsbTransport: Send {
    async fn prepare_device(&mut self) -> GadgetResult<()>;
    async fn wait_for_accessory(&mut self, timeout: Duration) -> GadgetResult<AccessoryData>;
    async fn reset_to_device(&mut self) -> GadgetResult<()>;
    async fn shutdown(&mut self) -> GadgetResult<()>;
}

#[async_trait]
impl CarUsbTransport for PhoneGadget {
    async fn prepare_device(&mut self) -> GadgetResult<()> {
        self.bind().await
    }

    async fn wait_for_accessory(&mut self, timeout: Duration) -> GadgetResult<AccessoryData> {
        tokio::time::timeout(timeout, async {
            loop {
                self.reconcile().await?;
                match self.status() {
                    GadgetStatus::Accessory(accessory) => return Ok(accessory),
                    GadgetStatus::RoleSwitchFailed | GadgetStatus::Error(_) => {
                        return Err(GadgetError::OtherString("car USB role switch or enumeration failed".into()));
                    }
                    _ => {
                        self.sleep().await;
                    }
                }
            }
        })
        .await
        .map_err(|_| GadgetError::OtherString("timed out waiting for iAP2/NCM accessory data".into()))?
    }

    async fn reset_to_device(&mut self) -> GadgetResult<()> {
        self.unbind().await?;
        self.bind().await
    }

    async fn shutdown(&mut self) -> GadgetResult<()> {
        self.unbind().await
    }
}
