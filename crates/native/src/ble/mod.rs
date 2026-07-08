//! Cross-platform BLE peripheral abstraction.
//!
//! `btleplug` (used elsewhere in this crate) only implements the BLE *central*
//! role: scanning and connecting. It has no GATT server / advertiser, so it
//! cannot make this desktop act as a guest peripheral. Real guest advertising
//! therefore needs one native backend per operating system, all hidden behind
//! the single [`BlePeripheral`] trait so calculator code stays platform-neutral.
//!
//! | OS      | Backend                                             | State        |
//! | ------- | --------------------------------------------------- | ------------ |
//! | macOS   | `CoreBluetooth` `CBPeripheralManager` (`objc2`)     | implemented  |
//! | Linux   | BlueZ / D-Bus via `bluer`                           | stub + TODO  |
//! | Windows | `GattServiceProvider` via the `windows` crate       | stub + TODO  |
//!
//! The wire contract (service UUID, `EvolveCalc:JOIN:...` advertisement name,
//! and chunk framing) is identical on every platform, which is what lets a
//! Linux guest be discovered by a macOS host and vice versa.

#[cfg(target_os = "macos")]
mod peripheral_macos;

#[cfg(target_os = "linux")]
mod peripheral_linux;

#[cfg(target_os = "windows")]
mod peripheral_windows;

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod peripheral_stub;

/// Service exposed by a guest peripheral. Matches the host-side scan filter in
/// `lib.rs` (`CALCULATOR_SERVICE_UUID`).
pub const CALCULATOR_SERVICE_UUID: &str = "7c14f94a-77dd-4a65-9f04-6f7ac8d2a601";
/// Characteristic the host writes calculation events into (host -> guest).
pub const CALCULATOR_RX_CHARACTERISTIC_UUID: &str = "7c14f94a-77dd-4a65-9f04-6f7ac8d2a602";
/// Characteristic the guest notifies calculation events on (guest -> host).
pub const CALCULATOR_TX_CHARACTERISTIC_UUID: &str = "7c14f94a-77dd-4a65-9f04-6f7ac8d2a603";

/// Advertisement `local_name` prefix. Kept in sync with the parser in `lib.rs`.
const ADVERTISEMENT_PREFIX: &str = "EvolveCalc";
const JOIN_ADVERTISEMENT_KIND: &str = "JOIN";

/// Everything a backend needs to start advertising as a joinable guest.
#[derive(Debug, Clone)]
pub struct PeripheralConfig {
    pub service_uuid: String,
    pub rx_characteristic_uuid: String,
    pub tx_characteristic_uuid: String,
    /// Fully-formatted BLE `local_name`, e.g. `EvolveCalc:JOIN:room-abc:Label`.
    pub local_name: String,
}

impl PeripheralConfig {
    /// Build a config for joining `room_id` advertised under `label`.
    pub fn join(room_id: &str, label: &str) -> Self {
        Self {
            service_uuid: CALCULATOR_SERVICE_UUID.to_string(),
            rx_characteristic_uuid: CALCULATOR_RX_CHARACTERISTIC_UUID.to_string(),
            tx_characteristic_uuid: CALCULATOR_TX_CHARACTERISTIC_UUID.to_string(),
            local_name: build_join_local_name(room_id, label),
        }
    }
}

/// Build the `local_name` a guest advertises so a host scan can parse it with
/// `parse_local_name_advertisement`. Colons are stripped from user-supplied
/// fields so they cannot break the `prefix:kind:room:label` framing.
pub fn build_join_local_name(room_id: &str, label: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        ADVERTISEMENT_PREFIX,
        JOIN_ADVERTISEMENT_KIND,
        sanitize_field(room_id),
        sanitize_field(label),
    )
}

fn sanitize_field(value: &str) -> String {
    value.trim().replace(':', " ")
}

/// A platform BLE peripheral. Backends are single-owner and drive their own
/// serial execution context internally, so this trait stays synchronous.
pub trait BlePeripheral: Send {
    /// Which native backend is active (`"macos-corebluetooth"`, `"stub-linux"`...).
    fn platform(&self) -> &'static str;

    /// Whether this platform can actually advertise a GATT peripheral.
    fn is_supported(&self) -> bool;

    /// Begin advertising + exposing the calculator GATT service. Idempotent:
    /// calling again replaces the advertised config.
    fn start_advertising(&mut self, config: &PeripheralConfig) -> Result<(), String>;

    /// Stop advertising and tear down the GATT service.
    fn stop(&mut self) -> Result<(), String>;

    fn is_advertising(&self) -> bool;

    /// Drain calculation-event payloads written by a connected host since the
    /// last call. Reassembly of chunk framing is handled by the caller.
    fn take_inbound(&mut self) -> Vec<Vec<u8>>;
}

/// Construct the peripheral backend for the current platform.
pub fn new_peripheral() -> Box<dyn BlePeripheral> {
    #[cfg(target_os = "macos")]
    {
        Box::new(peripheral_macos::MacosPeripheral::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(peripheral_linux::LinuxPeripheral::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(peripheral_windows::WindowsPeripheral::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Box::new(peripheral_stub::StubPeripheral::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_join_local_name_matching_scan_format() {
        let name = build_join_local_name("room-abc", "MacBook Guest");
        assert_eq!(name, "EvolveCalc:JOIN:room-abc:MacBook Guest");
    }

    #[test]
    fn sanitizes_colons_that_would_break_framing() {
        let name = build_join_local_name("room:evil", "lab:el");
        // Exactly four fields must remain so the host parser stays correct.
        assert_eq!(name.split(':').count(), 4);
        assert_eq!(name, "EvolveCalc:JOIN:room evil:lab el");
    }

    #[test]
    fn join_config_uses_calculator_uuids() {
        let config = PeripheralConfig::join("room-1", "Guest");
        assert_eq!(config.service_uuid, CALCULATOR_SERVICE_UUID);
        assert_eq!(config.rx_characteristic_uuid, CALCULATOR_RX_CHARACTERISTIC_UUID);
        assert_eq!(config.tx_characteristic_uuid, CALCULATOR_TX_CHARACTERISTIC_UUID);
        assert_eq!(config.local_name, "EvolveCalc:JOIN:room-1:Guest");
    }
}
