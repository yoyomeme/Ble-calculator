//! Linux guest BLE peripheral backend built on BlueZ via the `bluer` crate
//! (D-Bus). Provides the same calculator GATT service + advertisement as the
//! macOS CoreBluetooth backend, so a macOS host can discover and connect to a
//! Linux guest and vice versa.
//!
//! Threading model: `bluer` is async (tokio) and its `ApplicationHandle` /
//! `AdvertisementHandle` must stay alive for advertising to continue (dropping
//! them unregisters the GATT app / stops advertising). We therefore own a
//! dedicated thread running a current-thread tokio runtime that holds those
//! handles and is driven by an unbounded command channel. `start`/`stop` from
//! the calculator thread send commands; `start` waits briefly for a real
//! success/error so the UI's `lastBleError` reflects BlueZ setup failures.
//!
//! Verification note: this backend requires a Linux host with a running BlueZ
//! `bluetoothd` and D-Bus. It cannot be compiled or exercised on macOS (the
//! module is `cfg`-gated to `target_os = "linux"`), so it is validated on a
//! Linux machine separately.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bluer::adv::{Advertisement, AdvertisementHandle, Type as AdvertisementType};
use bluer::gatt::local::{
    Application, ApplicationHandle, Characteristic, CharacteristicNotify,
    CharacteristicNotifyMethod, CharacteristicNotifier, CharacteristicWrite,
    CharacteristicWriteMethod, Service,
};
use bluer::{Adapter, Session};
use futures::FutureExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use super::{BlePeripheral, PeripheralConfig};

/// How long `start_advertising` waits for the BlueZ setup to succeed or fail
/// before returning control to the UI.
const START_TIMEOUT: Duration = Duration::from_secs(10);

struct LinuxShared {
    advertising: AtomicBool,
    /// True while a host is subscribed to the TX characteristic.
    subscribed: AtomicBool,
    inbound: Mutex<Vec<Vec<u8>>>,
    /// The active TX notifier, captured when a host subscribes. Used by the
    /// worker to push guest -> host events. `None` when no host is subscribed.
    notifier: AsyncMutex<Option<CharacteristicNotifier>>,
}

enum Command {
    Start(PeripheralConfig, Sender<Result<(), String>>),
    Notify(Vec<Vec<u8>>, Sender<Result<usize, String>>),
    Stop,
}

pub struct LinuxPeripheral {
    shared: Arc<LinuxShared>,
    commands: UnboundedSender<Command>,
    _worker: std::thread::JoinHandle<()>,
}

impl LinuxPeripheral {
    pub fn new() -> Self {
        let shared = Arc::new(LinuxShared {
            advertising: AtomicBool::new(false),
            subscribed: AtomicBool::new(false),
            inbound: Mutex::new(Vec::new()),
            notifier: AsyncMutex::new(None),
        });
        let (commands, receiver) = unbounded_channel::<Command>();

        let worker_shared = shared.clone();
        let worker = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(run_worker(receiver, worker_shared));
        });

        Self {
            shared,
            commands,
            _worker: worker,
        }
    }
}

impl BlePeripheral for LinuxPeripheral {
    fn platform(&self) -> &'static str {
        "linux-bluez"
    }

    fn is_supported(&self) -> bool {
        true
    }

    fn start_advertising(&mut self, config: &PeripheralConfig) -> Result<(), String> {
        let (reply, reply_rx) = mpsc::channel();
        self.commands
            .send(Command::Start(config.clone(), reply))
            .map_err(|_| "Linux BLE peripheral worker is not running".to_string())?;

        match reply_rx.recv_timeout(START_TIMEOUT) {
            Ok(result) => {
                self.shared
                    .advertising
                    .store(result.is_ok(), Ordering::SeqCst);
                result
            }
            Err(_) => Err("Timed out waiting for BlueZ to start advertising".to_string()),
        }
    }

    fn stop(&mut self) -> Result<(), String> {
        let _ = self.commands.send(Command::Stop);
        self.shared.advertising.store(false, Ordering::SeqCst);
        self.shared.subscribed.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_advertising(&self) -> bool {
        self.shared.advertising.load(Ordering::SeqCst)
    }

    fn take_inbound(&mut self) -> Vec<Vec<u8>> {
        match self.shared.inbound.lock() {
            Ok(mut inbound) => std::mem::take(&mut *inbound),
            Err(_) => Vec::new(),
        }
    }

    fn notify(&mut self, frames: &[Vec<u8>]) -> Result<usize, String> {
        if frames.is_empty() {
            return Ok(0);
        }
        let (reply, reply_rx) = mpsc::channel();
        self.commands
            .send(Command::Notify(frames.to_vec(), reply))
            .map_err(|_| "Linux BLE peripheral worker is not running".to_string())?;
        reply_rx
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| "Timed out waiting for BlueZ notify".to_string())?
    }

    fn has_subscriber(&self) -> bool {
        self.shared.subscribed.load(Ordering::SeqCst)
    }
}

/// RAII holder for an active advertising session. Dropping it unregisters the
/// GATT application and stops advertising.
struct ActiveSession {
    _session: Session,
    _adapter: Adapter,
    _app: ApplicationHandle,
    _advertisement: AdvertisementHandle,
}

async fn run_worker(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<Command>,
    shared: Arc<LinuxShared>,
) {
    let mut active: Option<ActiveSession> = None;

    while let Some(command) = receiver.recv().await {
        match command {
            Command::Start(config, reply) => {
                // Drop any previous session first so we do not double-advertise.
                active = None;
                shared.advertising.store(false, Ordering::SeqCst);

                match start_session(&config, &shared).await {
                    Ok(session) => {
                        active = Some(session);
                        shared.advertising.store(true, Ordering::SeqCst);
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(format!("BlueZ advertising failed: {error}")));
                    }
                }
            }
            Command::Notify(frames, reply) => {
                let _ = reply.send(notify_frames(&shared, frames).await);
            }
            Command::Stop => {
                active = None;
                shared.advertising.store(false, Ordering::SeqCst);
                shared.subscribed.store(false, Ordering::SeqCst);
                *shared.notifier.lock().await = None;
            }
        }
    }
}

/// Push each frame to the subscribed host over the captured TX notifier.
async fn notify_frames(shared: &Arc<LinuxShared>, frames: Vec<Vec<u8>>) -> Result<usize, String> {
    let mut slot = shared.notifier.lock().await;
    let Some(notifier) = slot.as_mut() else {
        return Err("No host is subscribed to the guest TX characteristic".to_string());
    };
    if notifier.is_stopped() {
        *slot = None;
        shared.subscribed.store(false, Ordering::SeqCst);
        return Err("Host unsubscribed from the guest TX characteristic".to_string());
    }

    let mut sent = 0usize;
    for frame in frames {
        notifier
            .notify(frame)
            .await
            .map_err(|error| format!("BlueZ notify failed: {error}"))?;
        sent += 1;
    }
    Ok(sent)
}

async fn start_session(
    config: &PeripheralConfig,
    shared: &Arc<LinuxShared>,
) -> bluer::Result<ActiveSession> {
    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    let service_uuid = parse_uuid(&config.service_uuid)?;
    let rx_uuid = parse_uuid(&config.rx_characteristic_uuid)?;
    let tx_uuid = parse_uuid(&config.tx_characteristic_uuid)?;

    // RX: host writes calculation events; push each into the inbound buffer.
    let inbound = shared.inbound.clone();
    let write_fun: bluer::gatt::local::CharacteristicWriteFun = Box::new(move |value, _req| {
        let inbound = inbound.clone();
        async move {
            if let Ok(mut buffer) = inbound.lock() {
                buffer.push(value);
            }
            Ok(())
        }
        .boxed()
    });

    let rx_characteristic = Characteristic {
        uuid: rx_uuid,
        write: Some(CharacteristicWrite {
            write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Fun(write_fun),
            ..Default::default()
        }),
        ..Default::default()
    };

    // TX: notify characteristic for guest -> host events. When a host subscribes
    // BlueZ hands us a notifier; capture it so `notify_frames` can push signed
    // calculation events, and flag the live subscription for the guest UI.
    let notify_shared = shared.clone();
    let tx_characteristic = Characteristic {
        uuid: tx_uuid,
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(Box::new(move |notifier| {
                let notify_shared = notify_shared.clone();
                async move {
                    notify_shared.subscribed.store(true, Ordering::SeqCst);
                    *notify_shared.notifier.lock().await = Some(notifier);
                }
                .boxed()
            })),
            ..Default::default()
        }),
        ..Default::default()
    };

    let service = Service {
        uuid: service_uuid,
        primary: true,
        characteristics: vec![rx_characteristic, tx_characteristic],
        ..Default::default()
    };
    let application = Application {
        services: vec![service],
        ..Default::default()
    };
    let app_handle = adapter.serve_gatt_application(application).await?;

    let advertisement = Advertisement {
        advertisement_type: AdvertisementType::Peripheral,
        service_uuids: [service_uuid].into_iter().collect(),
        local_name: Some(config.local_name.clone()),
        discoverable: Some(true),
        ..Default::default()
    };
    let advertisement_handle = adapter.advertise(advertisement).await?;

    Ok(ActiveSession {
        _session: session,
        _adapter: adapter,
        _app: app_handle,
        _advertisement: advertisement_handle,
    })
}

fn parse_uuid(value: &str) -> bluer::Result<Uuid> {
    Uuid::parse_str(value).map_err(|error| {
        bluer::Error {
            kind: bluer::ErrorKind::Failed,
            message: format!("Invalid UUID {value}: {error}"),
        }
    })
}
