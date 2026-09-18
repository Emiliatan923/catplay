use crate::{
    proxy_rpc::{OverlayPeerState, OverlayPolicy},
    ui::UiState,
};
use catplay_bt::BluezManager;
use catplay_util::{EventReconciler, EventSleeper, EventToken, LazyAsync};
use log::{debug, warn};
use std::time::Duration;

pub struct OverlayPolicyDefault {
    adapter: String,
    bluez: LazyAsync<BluezManager>,
    peer: OverlayPeerState,
    overlay: Option<UiState>,
}

impl OverlayPolicyDefault {
    pub fn new(adapter: impl Into<String>) -> Self {
        let adapter = adapter.into();
        Self {
            adapter: adapter.clone(),
            bluez: LazyAsync::new(move || Self::start_pin_agent(adapter)),
            peer: OverlayPeerState::Unconnected,
            overlay: None,
        }
    }
    async fn start_pin_agent(adapter: String) -> BluezManager {
        loop {
            let mut mgr = BluezManager::new();
            match mgr.register_pin_agent().await {
                Ok(()) => {
                    // The iPhone finds the accessory by scanning, so both
                    // properties must stay enabled for wireless pairing.
                    if let Err(err) = mgr.set_discoverable(&adapter, true).await {
                        warn!("Failed to set BlueZ adapter discoverable on {adapter}: {err}");
                    }
                    if let Err(err) = mgr.set_pairable(&adapter, true).await {
                        warn!("Failed to set BlueZ adapter pairable on {adapter}: {err}");
                    }
                    return mgr;
                }
                Err(err) => {
                    debug!("BlueZ agent not available: {err}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
}

impl OverlayPolicy for OverlayPolicyDefault {
    fn overlay(&mut self) -> Option<UiState> {
        if self.peer != OverlayPeerState::Unconnected {
            return None;
        }

        if let Some(req) = self.bluez.as_ref().as_ref().and_then(|bluez| bluez.get_pairing_request()) {
            return Some(UiState::Pairing {
                device: req.remote_name.unwrap_or("unknown".into()),
                pin: format!("{}", req.passkey),
            });
        }

        // return Some(UiState::Connecting {
        //     device: "iPhone".into(),
        //     ticks: 10,
        // });

        Some(UiState::WaitingForConnection)
    }

    fn on_hid_interact(&mut self) -> bool {
        if let Some(req) = self.bluez.as_ref().as_ref().and_then(|bluez| bluez.get_pairing_request()) {
            req.accept();
            return false;
        }

        true
    }

    fn set_peer_state(&mut self, state: OverlayPeerState) {
        self.peer = state;
    }
}

impl EventReconciler for OverlayPolicyDefault {
    type Error = ();

    async fn reconcile(&mut self) -> Result<(), Self::Error> {
        let bluez_failed = if let Some(bluez) = self.bluez.as_ref().as_ref() {
            bluez.get_address(&self.adapter).await.is_err()
        } else {
            false
        };
        if bluez_failed {
            warn!("BlueZ restarted; re-registering the pairing agent");
            let adapter = self.adapter.clone();
            self.bluez = LazyAsync::new(move || Self::start_pin_agent(adapter));
        }
        self.overlay = self.overlay();
        Ok(())
    }
}

impl EventSleeper for OverlayPolicyDefault {
    async fn sleep(&mut self) -> Option<EventToken> {
        self.bluez.sleep().await
    }
}
