//! Persistent BLE **central** (host) side, built on `btleplug`.
//!
//! The previous implementation built a fresh Tokio runtime per call, connected,
//! and then dropped every handle — so the connection was never retained and no
//! GATT read/write/notify could follow (see the Core Bluetooth review gaps #1
//! and #2). This module keeps a single long-lived runtime plus the connected
//! `Peripheral` and its RX `Characteristic` in a process-global, so a host can
//! actually stream calculation events to a connected guest and observe the link
//! dropping.
//!
//! Concurrency: every operation locks one `Mutex`, so central BLE work is
//! serialized. That is desirable — one radio, one connection at a time — and it
//! keeps the non-reentrant `btleplug` backend calls from overlapping.
//!
//! Note on MTU: `btleplug` 0.11 exposes no `maximumWriteValueLength` / MTU
//! query. We therefore prefer write-**with-response** (acknowledged, ordered,
//! and supported up to 512 bytes by Core Bluetooth's automatic long-write),
//! which also gives natural per-write backpressure, and cap each frame with a
//! conservative constant in `lib.rs`.

use std::sync::Mutex;
use std::time::Duration;

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, PeripheralProperties,
    ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use once_cell::sync::Lazy;
use uuid::Uuid;

/// One long-lived Tokio runtime shared by every central operation. Kept alive
/// for the whole process so the `btleplug` connection is not torn down when a
/// single call returns. `None` if the runtime could not be built.
static RT: Lazy<Option<tokio::runtime::Runtime>> = Lazy::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .ok()
});

struct CentralInner {
    /// Created lazily on first use and reused so the connection stays bound to
    /// the same adapter/backend instance.
    adapter: Option<Adapter>,
    /// The currently connected guest peripheral and the RX characteristic the
    /// host writes calculation events into. `None` when disconnected.
    connected: Option<(Peripheral, Characteristic)>,
}

static CENTRAL: Lazy<Mutex<CentralInner>> = Lazy::new(|| {
    Mutex::new(CentralInner {
        adapter: None,
        connected: None,
    })
});

fn runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    RT.as_ref()
        .ok_or_else(|| "Tokio runtime for the BLE central is unavailable".to_string())
}

fn lock() -> Result<std::sync::MutexGuard<'static, CentralInner>, String> {
    CENTRAL
        .lock()
        .map_err(|_| "BLE central lock was poisoned".to_string())
}

/// Create (once) and return the first BLE adapter.
fn ensure_adapter(
    rt: &tokio::runtime::Runtime,
    inner: &mut CentralInner,
) -> Result<Adapter, String> {
    if let Some(adapter) = &inner.adapter {
        return Ok(adapter.clone());
    }

    let adapter = rt.block_on(async {
        let manager = Manager::new()
            .await
            .map_err(|error| format!("BLE manager unavailable: {error}"))?;
        let adapters = manager
            .adapters()
            .await
            .map_err(|error| format!("BLE adapter list unavailable: {error}"))?;
        adapters
            .into_iter()
            .next()
            .ok_or_else(|| "No BLE adapter found".to_string())
    })?;

    inner.adapter = Some(adapter.clone());
    Ok(adapter)
}

/// Scan for peripherals advertising `service_uuid` for `duration_ms` and return
/// their advertisement properties (address, local name, and RSSI). Filtering by
/// service UUID at the radio level is the professional default (review gap #4);
/// the returned `rssi` closes gap #5.
pub fn scan(service_uuid: Uuid, duration_ms: u64) -> Result<Vec<PeripheralProperties>, String> {
    let rt = runtime()?;
    let mut inner = lock()?;
    let adapter = ensure_adapter(rt, &mut inner)?;

    rt.block_on(async {
        adapter
            .start_scan(ScanFilter {
                services: vec![service_uuid],
            })
            .await
            .map_err(|error| format!("BLE scan failed to start: {error}"))?;
        tokio::time::sleep(Duration::from_millis(duration_ms)).await;

        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|error| format!("BLE peripheral list unavailable: {error}"))?;
        let _ = adapter.stop_scan().await;

        let mut discovered = Vec::new();
        for peripheral in peripherals {
            if let Ok(Some(properties)) = peripheral.properties().await {
                discovered.push(properties);
            }
        }
        Ok(discovered)
    })
}

/// Connect to `address`, discover services, locate the calculator RX
/// characteristic, and **retain** the peripheral + characteristic so subsequent
/// writes can reuse the live connection (review gap #2).
pub fn connect(address: &str, service_uuid: Uuid, rx_uuid: Uuid) -> Result<(), String> {
    let rt = runtime()?;
    let mut inner = lock()?;
    let adapter = ensure_adapter(rt, &mut inner)?;
    let address = address.to_string();

    let connected = rt.block_on(async {
        adapter
            .start_scan(ScanFilter {
                services: vec![service_uuid],
            })
            .await
            .map_err(|error| format!("BLE scan failed before connect: {error}"))?;
        tokio::time::sleep(Duration::from_millis(900)).await;

        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|error| format!("BLE peripheral list unavailable before connect: {error}"))?;
        let _ = adapter.stop_scan().await;

        for peripheral in peripherals {
            if peripheral.address().to_string() != address {
                continue;
            }

            peripheral
                .connect()
                .await
                .map_err(|error| format!("BLE peripheral connect failed: {error}"))?;
            peripheral
                .discover_services()
                .await
                .map_err(|error| format!("BLE service discovery failed: {error}"))?;

            let rx = peripheral
                .characteristics()
                .into_iter()
                .find(|characteristic| characteristic.uuid == rx_uuid)
                .ok_or_else(|| {
                    "Connected peer does not expose the calculator RX characteristic".to_string()
                })?;

            return Ok((peripheral, rx));
        }

        Err(format!("BLE peer {address} was not found for connect"))
    })?;

    inner.connected = Some(connected);
    Ok(())
}

/// True while the retained peripheral reports an active connection. Used to keep
/// `peer.connected` honest and detect drops (review gap #3). Returns `false`
/// when nothing is connected.
pub fn is_connected() -> bool {
    let Ok(rt) = runtime() else {
        return false;
    };
    let Ok(inner) = lock() else {
        return false;
    };
    let Some((peripheral, _)) = inner.connected.as_ref() else {
        return false;
    };
    let peripheral = peripheral.clone();
    rt.block_on(async move { peripheral.is_connected().await.unwrap_or(false) })
}

/// Write each frame to the connected guest's RX characteristic as a real GATT
/// write (review gap #1). Frames are sent sequentially and each write is
/// awaited, which — with write-with-response — provides ordered delivery and
/// backpressure (review gap #6). Returns the number of frames written.
pub fn write_frames(frames: &[Vec<u8>]) -> Result<usize, String> {
    let rt = runtime()?;
    let inner = lock()?;
    let (peripheral, rx) = inner
        .connected
        .as_ref()
        .ok_or_else(|| "No connected guest to deliver the calculation to".to_string())?;

    let write_type = if rx.properties.contains(CharPropFlags::WRITE) {
        WriteType::WithResponse
    } else if rx.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        WriteType::WithoutResponse
    } else {
        return Err("Calculator RX characteristic is not writable".to_string());
    };

    let peripheral = peripheral.clone();
    let rx = rx.clone();

    rt.block_on(async move {
        if !peripheral.is_connected().await.unwrap_or(false) {
            return Err("Guest peer is no longer connected".to_string());
        }

        let mut written = 0usize;
        for frame in frames {
            peripheral
                .write(&rx, frame, write_type)
                .await
                .map_err(|error| format!("BLE characteristic write failed: {error}"))?;
            written += 1;
        }
        Ok(written)
    })
}

/// Tear down the retained connection, if any. Best-effort — errors are ignored
/// because the caller is resetting session state regardless.
pub fn disconnect() {
    let Ok(rt) = runtime() else {
        return;
    };
    let Ok(mut inner) = lock() else {
        return;
    };
    if let Some((peripheral, _)) = inner.connected.take() {
        let _ = rt.block_on(async move { peripheral.disconnect().await });
    }
}
