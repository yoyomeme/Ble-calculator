//! Windows guest BLE peripheral backend built on WinRT
//! `GattServiceProvider` (`Windows.Devices.Bluetooth.GenericAttributeProfile`).
//!
//! Exposes the same calculator GATT service (RX write + TX notify) as the macOS
//! and Linux backends and advertises it as a connectable peripheral, so a host
//! can discover and connect to a Windows guest.
//!
//! WinRT objects are agile (`Send + Sync` in windows-rs), and its GATT async
//! operations can be `.get()`-blocked, so this backend does not need a
//! dedicated runtime thread the way the Linux `bluer` backend does. The
//! `WriteRequested` event fires on a WinRT thread-pool thread; its handler
//! pushes received bytes into the shared inbound buffer.
//!
//! Verification note: the `windows` crate is Windows-only, so this module is
//! `cfg`-gated to `target_os = "windows"` and cannot be compiled or exercised
//! on macOS/Linux. It is validated on a Windows host separately.
//!
//! Known limitation: `GattServiceProviderAdvertisingParameters` only controls
//! `IsConnectable` / `IsDiscoverable` — it advertises the service UUID but
//! cannot carry the custom `EvolveCalc:JOIN:<room>:<label>` local name that the
//! host scan parser reads for room/label metadata. Carrying that metadata needs
//! a separate `BluetoothLEAdvertisementPublisher`, or a host-side rule that
//! treats a bare calculator-service-UUID match as a discoverable guest. Until
//! then a Windows guest is connectable but not fully self-describing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::GUID;
use windows::Devices::Bluetooth::BluetoothError;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattLocalCharacteristic, GattLocalCharacteristicParameters,
    GattProtectionLevel, GattServiceProvider, GattServiceProviderAdvertisingParameters,
    GattWriteOption, GattWriteRequestedEventArgs,
};
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};

use super::{BlePeripheral, PeripheralConfig};

struct WindowsShared {
    advertising: AtomicBool,
    inbound: Mutex<Vec<Vec<u8>>>,
}

/// Holds the objects that must stay alive for advertising to continue. Dropping
/// it (via `stop`/replacement) stops advertising and unregisters the handler.
struct ActiveSession {
    provider: GattServiceProvider,
    rx: GattLocalCharacteristic,
    write_token: EventRegistrationToken,
    /// TX notify characteristic (guest -> host). Retained so `notify` can push
    /// events and `has_subscriber` can read the subscribed-client list.
    tx: GattLocalCharacteristic,
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        let _ = self.provider.StopAdvertising();
        let _ = self.rx.RemoveWriteRequested(self.write_token);
    }
}

pub struct WindowsPeripheral {
    shared: Arc<WindowsShared>,
    active: Option<ActiveSession>,
}

impl WindowsPeripheral {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(WindowsShared {
                advertising: AtomicBool::new(false),
                inbound: Mutex::new(Vec::new()),
            }),
            active: None,
        }
    }
}

impl BlePeripheral for WindowsPeripheral {
    fn platform(&self) -> &'static str {
        "windows-gatt"
    }

    fn is_supported(&self) -> bool {
        true
    }

    fn start_advertising(&mut self, config: &PeripheralConfig) -> Result<(), String> {
        // Drop any previous session first so we do not double-advertise.
        self.active = None;
        self.shared.advertising.store(false, Ordering::SeqCst);

        match build_session(config, &self.shared) {
            Ok(session) => {
                self.active = Some(session);
                self.shared.advertising.store(true, Ordering::SeqCst);
                Ok(())
            }
            Err(error) => Err(format!("Windows GATT advertising failed: {error}")),
        }
    }

    fn stop(&mut self) -> Result<(), String> {
        // Dropping ActiveSession stops advertising + removes the handler.
        self.active = None;
        self.shared.advertising.store(false, Ordering::SeqCst);
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
        let Some(active) = self.active.as_ref() else {
            return Err("Windows GATT peripheral is not advertising".to_string());
        };
        let mut sent = 0usize;
        for frame in frames {
            notify_frame(&active.tx, frame)
                .map_err(|error| format!("Windows GATT notify failed: {error}"))?;
            sent += 1;
        }
        Ok(sent)
    }

    fn has_subscriber(&self) -> bool {
        self.active
            .as_ref()
            .map(|active| subscribed_count(&active.tx) > 0)
            .unwrap_or(false)
    }
}

fn build_session(
    config: &PeripheralConfig,
    shared: &Arc<WindowsShared>,
) -> windows::core::Result<ActiveSession> {
    let service_guid = to_guid(&config.service_uuid);
    let rx_guid = to_guid(&config.rx_characteristic_uuid);
    let tx_guid = to_guid(&config.tx_characteristic_uuid);

    let provider_result = GattServiceProvider::CreateAsync(service_guid)?.get()?;
    if provider_result.Error()? != BluetoothError::Success {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x8000_4005u32 as i32),
            "GattServiceProvider creation returned a Bluetooth error",
        ));
    }
    let provider = provider_result.ServiceProvider()?;
    let service = provider.Service()?;

    // RX: host writes calculation events into this characteristic.
    let rx_params = GattLocalCharacteristicParameters::new()?;
    rx_params.SetCharacteristicProperties(
        GattCharacteristicProperties::Write | GattCharacteristicProperties::WriteWithoutResponse,
    )?;
    rx_params.SetWriteProtectionLevel(GattProtectionLevel::Plain)?;
    let rx = service
        .CreateCharacteristicAsync(rx_guid, &rx_params)?
        .get()?
        .Characteristic()?;

    // Clone the Arc (not the inner Mutex) so the write handler can reach it.
    let handler_shared = shared.clone();
    let handler = TypedEventHandler::<GattLocalCharacteristic, GattWriteRequestedEventArgs>::new(
        move |_sender, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(request) = args.GetRequestAsync().and_then(|op| op.get()) {
                    if let Ok(buffer) = request.Value() {
                        if let Ok(bytes) = read_buffer(&buffer) {
                            if let Ok(mut buffer) = handler_shared.inbound.lock() {
                                buffer.push(bytes);
                            }
                        }
                    }
                    // A response is required only for write-with-response.
                    if request.Option().unwrap_or(GattWriteOption::WriteWithoutResponse)
                        == GattWriteOption::WriteWithResponse
                    {
                        let _ = request.Respond();
                    }
                }
            }
            Ok(())
        },
    );
    let write_token = rx.WriteRequested(&handler)?;

    // TX: notify characteristic for guest -> host events. Notify delivery is a
    // later step (parity with the macOS/Linux backends); the session is kept.
    let tx_params = GattLocalCharacteristicParameters::new()?;
    tx_params.SetCharacteristicProperties(
        GattCharacteristicProperties::Notify | GattCharacteristicProperties::Read,
    )?;
    tx_params.SetReadProtectionLevel(GattProtectionLevel::Plain)?;
    let tx = service
        .CreateCharacteristicAsync(tx_guid, &tx_params)?
        .get()?
        .Characteristic()?;

    let advertising_parameters = GattServiceProviderAdvertisingParameters::new()?;
    advertising_parameters.SetIsConnectable(true)?;
    advertising_parameters.SetIsDiscoverable(true)?;
    provider.StartAdvertisingWithParameters(&advertising_parameters)?;

    Ok(ActiveSession {
        provider,
        rx,
        write_token,
        tx,
    })
}

/// Notify one frame to every subscribed client over the TX characteristic.
fn notify_frame(tx: &GattLocalCharacteristic, frame: &[u8]) -> windows::core::Result<()> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(frame)?;
    let buffer = writer.DetachBuffer()?;
    // Blocks until WinRT accepts the value for delivery to subscribed clients.
    tx.NotifyValueAsync(&buffer)?.get()?;
    Ok(())
}

/// Number of centrals currently subscribed to the TX characteristic.
fn subscribed_count(tx: &GattLocalCharacteristic) -> u32 {
    tx.SubscribedClients()
        .and_then(|clients| clients.Size())
        .unwrap_or(0)
}

fn read_buffer(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
    let length = buffer.Length()? as usize;
    let mut bytes = vec![0u8; length];
    if length > 0 {
        let reader = DataReader::FromBuffer(buffer)?;
        reader.ReadBytes(&mut bytes)?;
    }
    Ok(bytes)
}

fn to_guid(uuid: &str) -> GUID {
    match uuid::Uuid::parse_str(uuid) {
        Ok(parsed) => GUID::from_u128(parsed.as_u128()),
        Err(_) => GUID::from_u128(0),
    }
}
