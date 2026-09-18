use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use catplay_carplay::{
    carplay_rx::sink::AirPlayServerShared,
    carplay_tx::{AirPlayTransmitterImpl, AirPlayTransmitterSessionError, TeardownGuard},
};
use catplay_carplay_rx_gadget::{CarPlayWirelessGadget, CarPlayWirelessGadgetError, CarPlayWirelessGadgetState};
use catplay_carplay_tx_gadget::client::{CarPlayUsbClientGadget, CarPlayUsbClientGadgetError, CarPlayUsbClientGadgetStatus};
use catplay_hap::HomekitStorageRef;
use catplay_iap2_usb::GadgetError;
use catplay_mfi::MfiDeficeRef;
use catplay_util::{
    AsyncShutdown, EventReconciler, EventSleeper, EventToken, Reconcilable, Reconciler, deadline_after, event_select, mpsc,
};
use log::{debug, error, info};
use tokio::sync::Mutex as TokioMutex;

use crate::proxy_rpc::{CarManager, CarPlayRxSession, TxAdapter, TxAdapterOp};

pub struct ProdGadgetConfig {
    pub homekit_rx: HomekitStorageRef,
    pub homekit_tx: HomekitStorageRef,
    pub mfi: Option<MfiDeficeRef>,

    // Car side UDC. None = autodiscover only UDC, Some = UDC name
    pub udc_tx: Option<String>,
    // Extra UDC for optional USB receive. None = disabled, Some = UDC name
    pub udc_rx: Option<String>,

    pub bt_name: String,
    pub hci: String,
    pub iface: String,
    pub wpa: bool,
    pub ssid: String,
    pub channel: Option<u8>,
    pub passphrase: String,

    pub pinned: bool,
    pub iphone_instance: String,
    pub iphone_serial: String,
    pub bonjour_id: String,

    pub bt_cache_file: String,
    pub persist_dir: Option<PathBuf>,
}

pub struct ProdGadget {
    cfg: ProdGadgetConfig,

    rx: Option<Reconciler<CarPlayWirelessGadget<CarPlayRxSession>>>,
    // rx_usb: Option<Reconciler<CarPlayUsbGadget<CarPlayRxSession>>>,
    tx: Option<Reconciler<CarPlayUsbClientGadget>>,
    pending_transmitter: Arc<TokioMutex<Option<TeardownGuard<AirPlayTransmitterImpl>>>>,

    last_tx: Arc<Mutex<Option<ProdGadgetSession>>>,
    burst_wakeups: bool,
    session_seq: u64,
}

#[derive(Clone, PartialEq, Debug, thiserror::Error)]
pub enum ProdGadgetError {
    #[error("Rx error: {0}")]
    Rx(#[from] CarPlayWirelessGadgetError),
    #[error("Tx error: {0}")]
    Tx(#[from] CarPlayUsbClientGadgetError),

    #[error("Failed to setup usb client, missing udc? {0}")]
    FailedToSetupUsbClient(GadgetError),

    #[error("Transmitter reached eof before pickup: {0}")]
    UnpickedTransmitterEof(#[from] AirPlayTransmitterSessionError),

    #[error("Failed to initialize car proxy: {0}")]
    FailedToStartProxy(catplay_carplay::rtsp_frame::RtspError),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProdGadgetState {
    Initial,
    WaitingForUdc,
    WaitingForUsbTransmitterGadget,
    WaitingForWirelessCarPlayGadget,

    WaitingForCar,
    WaitingForIPhone,

    Running,
}

type LocalResult<T> = Result<T, LocalError>;
type LocalStatus = LocalResult<LocalState>;
type LocalError = ProdGadgetError;
type LocalState = ProdGadgetState;

impl<T> From<LocalError> for LocalResult<T> {
    fn from(value: LocalError) -> Self {
        Err(value)
    }
}

impl From<LocalState> for LocalStatus {
    fn from(value: LocalState) -> Self {
        Ok(value)
    }
}

impl ProdGadget {
    const RESTART_DELAY: Duration = Duration::from_millis(1000);

    pub fn new(cfg: ProdGadgetConfig) -> Reconciler<Self> {
        Reconciler::new(
            Self {
                cfg,
                // rx_usb: None,
                rx: None,
                tx: None,
                pending_transmitter: Default::default(),
                last_tx: Default::default(),
                burst_wakeups: false,
                session_seq: 0,
            },
            Ok(ProdGadgetState::Initial),
        )
    }
}

impl AsyncShutdown for ProdGadget {
    async fn shutdown(&mut self) {
        self.pending_transmitter.lock().await.take();

        if let Some(mut rx) = self.rx.take() {
            rx.shutdown().await;
        }
        // if let Some(mut rx_usb) = self.rx_usb.take() {
        //     rx_usb.shutdown().await;
        // }
        if let Some(mut tx) = self.tx.take() {
            tx.shutdown().await;
        }
    }
}

#[derive(Clone)]
pub struct ProdGadgetSession {
    pub id: u64,
    pub proxy_tx: mpsc::UnboundedSender<TxAdapterOp>,
}

impl EventSleeper for ProdGadget {
    async fn sleep(&mut self) -> Option<EventToken> {
        let mut pending_transmitter = self.pending_transmitter.lock().await;
        let fallback = if self.burst_wakeups {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(500)
        };

        event_select!(
            pending_transmitter.as_mut(),
            self.tx,
            // self.rx_usb,
            self.rx,
            deadline_after(fallback)
        )
    }
}

impl Reconcilable for ProdGadget {
    type Output = LocalStatus;

    async fn on_update(&mut self, new: LocalStatus) -> LocalStatus {
        match new {
            Ok(ref ok) => info!("Progressing -> {ok:?}"),
            Err(ref err) => error!("Entered error state: {err:?}"),
        };

        if new.is_err() {
            debug!("Cleaning up due to error state");
            self.pending_transmitter.lock().await.take();
            self.last_tx.lock().unwrap().take();
            if let Some(rx) = self.rx.as_mut() {
                rx.child_mut().set_invites_blocked(true)
            };
        }

        self.burst_wakeups = new.is_err()
            || matches!(
                new,
                Ok(ProdGadgetState::Initial)
                    | Ok(ProdGadgetState::WaitingForUdc)
                    | Ok(ProdGadgetState::WaitingForUsbTransmitterGadget)
                    | Ok(ProdGadgetState::WaitingForWirelessCarPlayGadget)
            );

        new
    }

    async fn render(&mut self, prev: LocalStatus, update: Instant) -> LocalStatus {
        let _ = self.rx.reconcile().await;
        // let _ = self.rx_usb.reconcile().await;
        let _ = self.tx.reconcile().await;

        let Ok(_status) = prev.as_ref() else {
            let err = prev.err().unwrap();

            if update.elapsed() > Self::RESTART_DELAY {
                return LocalState::Initial.into();
            }

            return err.into();
        };

        let shared = AirPlayServerShared::new();

        if self.rx.is_none() {
            self.rx.replace(CarPlayWirelessGadget::new(
                self.cfg.homekit_rx.clone(),
                self.cfg.mfi.clone(),
                shared.clone(),
                &self.cfg.bt_name,
                &self.cfg.hci,
                &self.cfg.iface,
                &self.cfg.ssid,
                Some(&self.cfg.passphrase),
                self.cfg.wpa,
                self.cfg.channel,
                Some(&self.cfg.bt_cache_file),
                {
                    let car = self.last_tx.clone();

                    move || {
                        let Some(car) = car.lock().unwrap().clone() else {
                            return CarPlayRxSession::reject();
                        };

                        let sess = TxAdapter::new(car.proxy_tx.clone());
                        CarPlayRxSession::new(sess)
                    }
                },
            ));
        }

        if self.tx.is_none() {
            let gadget = CarPlayUsbClientGadget::new_with_identity(
                &self.cfg.iphone_instance,
                Some(&self.cfg.iphone_serial),
                &self.cfg.bonjour_id,
                self.cfg.udc_tx.as_deref(),
                self.cfg.homekit_tx.clone(),
                self.cfg.pinned,
            )
            .map_err(|err| LocalError::FailedToSetupUsbClient(err))?;
            self.tx.replace(gadget);
        }
        let mut pending_transmitter = self.pending_transmitter.lock().await;

        // Transmitter reached error status after successful handshake, but before being picked up by the receiver.
        if let Err(err) = pending_transmitter.reconcile().await {
            return Err(err)?;
        }

        let tx = self.tx.as_mut().unwrap();
        let rx = self.rx.as_mut().unwrap();
        if pending_transmitter.is_none()
            && let Some(transmitter) = tx.child_mut().pop_transmitter()
        {
            debug!("Got a transmitter");
            // pending_transmitter.replace(transmitter);

            let proxy_chan = mpsc::unbounded();
            let car_state = ProdGadgetSession {
                id: 0,
                proxy_tx: proxy_chan.0,
            };

            let manager = CarManager::new(transmitter, proxy_chan.1, self.cfg.persist_dir.clone(), self.cfg.hci.clone())
                .await
                .map_err(LocalError::FailedToStartProxy)?;
            self.session_seq = self.session_seq.wrapping_add(1);
            let session_id = self.session_seq;
            let session_slot = self.last_tx.clone();
            self.last_tx.lock().unwrap().replace(ProdGadgetSession {
                id: session_id,
                ..car_state
            });
            tokio::spawn(async move {
                manager.worker().await;
                let mut slot = session_slot.lock().unwrap();
                if slot.as_ref().is_some_and(|session| session.id == session_id) {
                    slot.take();
                }
            });
        }

        // The worker completion callback clears last_tx, which blocks new
        // wireless invites until a fresh car session is available.
        rx.child_mut().set_invites_blocked(self.last_tx.lock().unwrap().is_none());

        match _status {
            ProdGadgetState::Initial => ProdGadgetState::WaitingForUdc.into(),
            ProdGadgetState::WaitingForUdc => ProdGadgetState::WaitingForUsbTransmitterGadget.into(),
            ProdGadgetState::WaitingForUsbTransmitterGadget => ProdGadgetState::WaitingForWirelessCarPlayGadget.into(),
            ProdGadgetState::WaitingForWirelessCarPlayGadget => ProdGadgetState::WaitingForCar.into(),
            ProdGadgetState::WaitingForCar => match tx.state() {
                Ok(CarPlayUsbClientGadgetStatus::Transmitting | CarPlayUsbClientGadgetStatus::TransmitterReadyForPickup) => {
                    ProdGadgetState::WaitingForIPhone.into()
                }

                _ if pending_transmitter.is_some() => ProdGadgetState::WaitingForIPhone.into(),
                _ => ProdGadgetState::WaitingForCar.into(),
            },
            ProdGadgetState::WaitingForIPhone => match rx.state() {
                Ok(CarPlayWirelessGadgetState::Receiving) => ProdGadgetState::Running.into(),
                _ => ProdGadgetState::WaitingForIPhone.into(),
            },
            ProdGadgetState::Running => {
                let still_transmitting = matches!(tx.state(), Ok(CarPlayUsbClientGadgetStatus::Transmitting));
                let still_receiving = matches!(rx.state(), Ok(CarPlayWirelessGadgetState::Receiving));

                match still_receiving && still_transmitting {
                    true => ProdGadgetState::Running.into(),
                    false => ProdGadgetState::WaitingForCar.into(),
                }
            }
        }
    }
}
