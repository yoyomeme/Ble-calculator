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
//! The wire contract (service UUID, `EVC:J:<room>` advertisement name, and
//! chunk framing) is identical on every platform, which is what lets a
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
///
/// Deliberately terse: a BLE scan-response local name has at most 29 usable
/// bytes (31-byte PDU minus the 2-byte AD header), and CoreBluetooth truncates
/// anything longer. The whole `EVC:<kind>:<room>` name must survive intact or
/// the receiving parser sees a corrupted room id, so the wire format budgets
/// well under that limit (`advertisement_names_fit_scan_response_budget`).
const ADVERTISEMENT_PREFIX: &str = "EVC";
const JOIN_ADVERTISEMENT_KIND: &str = "J";
const ROOM_ADVERTISEMENT_KIND: &str = "R";

/// Hard ceiling every built advertisement name must stay under, with margin
/// below the 29-byte radio budget. Room ids are short (`r-xxxxxx`) so this
/// holds; the unit test keeps it honest.
pub const MAX_ADVERTISEMENT_NAME_BYTES: usize = 26;

/// Longest room id that still fits an advertisement name: the budget minus the
/// `EVC:<kind>:` framing. Enforced at runtime by `join_room` because the join
/// code is typed by hand and would otherwise be truncated on air, corrupting
/// the id exactly like the bug this format exists to prevent.
pub const MAX_ROOM_ID_BYTES: usize =
    MAX_ADVERTISEMENT_NAME_BYTES - ADVERTISEMENT_PREFIX.len() - JOIN_ADVERTISEMENT_KIND.len() - 2;

/// Everything a backend needs to start advertising as a joinable guest.
#[derive(Debug, Clone)]
pub struct PeripheralConfig {
    pub service_uuid: String,
    pub rx_characteristic_uuid: String,
    pub tx_characteristic_uuid: String,
    /// Fully-formatted BLE `local_name`, e.g. `EVC:J:r-abc123`.
    pub local_name: String,
}

impl PeripheralConfig {
    /// Build a config for a guest joining `room_id` (`EVC:J:<room>`). The host
    /// scans for these and connects.
    pub fn join(room_id: &str) -> Self {
        Self::for_local_name(build_join_local_name(room_id))
    }

    /// Build a config for a host advertising `room_id` as a discoverable room
    /// (`EVC:R:<room>`). This is a discovery beacon only — the host stays the
    /// BLE *central* for the data link — so guests scanning for rooms
    /// (`scan_rooms`) can find it.
    pub fn room(room_id: &str) -> Self {
        Self::for_local_name(build_room_local_name(room_id))
    }

    fn for_local_name(local_name: String) -> Self {
        Self {
            service_uuid: CALCULATOR_SERVICE_UUID.to_string(),
            rx_characteristic_uuid: CALCULATOR_RX_CHARACTERISTIC_UUID.to_string(),
            tx_characteristic_uuid: CALCULATOR_TX_CHARACTERISTIC_UUID.to_string(),
            local_name,
        }
    }
}

/// Build the `local_name` a guest advertises so a host scan can parse it with
/// `parse_local_name_advertisement`. Colons are stripped from the room id so
/// they cannot break the `prefix:kind:room` framing. No human-readable label
/// travels on the wire — it would push the name past the scan-response budget
/// and be truncated anyway; scanners synthesize display labels locally.
pub fn build_join_local_name(room_id: &str) -> String {
    build_local_name(JOIN_ADVERTISEMENT_KIND, room_id)
}

/// Build the `local_name` a host advertises as a room discovery beacon. Same
/// `prefix:kind:room` framing as [`build_join_local_name`], so the host scan
/// parser reads it with `parse_local_name_advertisement`.
pub fn build_room_local_name(room_id: &str) -> String {
    build_local_name(ROOM_ADVERTISEMENT_KIND, room_id)
}

fn build_local_name(kind: &str, room_id: &str) -> String {
    let name = format!("{}:{}:{}", ADVERTISEMENT_PREFIX, kind, sanitize_field(room_id));
    debug_assert!(
        name.len() <= MAX_ADVERTISEMENT_NAME_BYTES,
        "advertisement name {name:?} exceeds the scan-response budget"
    );
    name
}

fn sanitize_field(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(':', " ")
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

    /// Send each frame to the subscribed host over the TX notify characteristic
    /// (guest -> host). Returns how many frames were handed to the platform for
    /// delivery. Frames that cannot be sent yet (no subscriber, or the notify
    /// queue is full) may be retained and flushed once delivery is possible, so
    /// a return value below `frames.len()` means the remainder is queued.
    fn notify(&mut self, frames: &[Vec<u8>]) -> Result<usize, String>;

    /// Whether a central (host) is currently subscribed to the TX characteristic.
    /// Lets a guest surface an active host link in its UI.
    fn has_subscriber(&self) -> bool;

    /// Drain the most recent asynchronous backend error, if any (failed
    /// advertising confirmation, undeliverable notification, dropped frames).
    /// Take-once semantics: polling callers surface each error a single time.
    fn take_last_error(&mut self) -> Option<String> {
        None
    }

    /// The subscribed central's maximum notification size in bytes (macOS:
    /// `maximumUpdateValueLength`), or `None` while no subscriber has been
    /// observed. Callers size notify frames with this — notifications are never
    /// fragmented by the platform, unlike long writes.
    fn max_notify_frame_len(&self) -> Option<usize> {
        None
    }
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
        let name = build_join_local_name("r-abc123");
        assert_eq!(name, "EVC:J:r-abc123");
    }

    #[test]
    fn sanitizes_colons_that_would_break_framing() {
        let name = build_join_local_name("room:evil");
        // Exactly three fields must remain so the host parser stays correct.
        assert_eq!(name.split(':').count(), 3);
        assert_eq!(name, "EVC:J:room evil");
    }

    #[test]
    fn normalizes_room_ids_to_lowercase() {
        // Join codes are typed by hand on the guest; case must not matter.
        assert_eq!(build_join_local_name(" R-ABC123 "), "EVC:J:r-abc123");
    }

    #[test]
    fn builds_room_local_name_with_room_kind() {
        let name = build_room_local_name("r-abc123");
        assert_eq!(name, "EVC:R:r-abc123");
        assert_eq!(name.split(':').count(), 3);
    }

    #[test]
    fn advertisement_names_fit_scan_response_budget() {
        // A scan-response local name gets at most 29 bytes on air; anything
        // longer is truncated by CoreBluetooth and the room id is corrupted.
        // `r-xxxxxx` is the longest room id `create_room` generates, and
        // `MAX_ROOM_ID_BYTES` is the longest join code `join_room` accepts.
        let longest_code = "x".repeat(MAX_ROOM_ID_BYTES);
        for name in [
            build_join_local_name("r-abc123"),
            build_room_local_name("r-abc123"),
            build_join_local_name(&longest_code),
        ] {
            assert!(
                name.len() <= MAX_ADVERTISEMENT_NAME_BYTES,
                "advertisement name {name:?} ({} bytes) exceeds the {MAX_ADVERTISEMENT_NAME_BYTES}-byte budget",
                name.len(),
            );
        }
    }

    #[test]
    fn join_config_uses_calculator_uuids() {
        let config = PeripheralConfig::join("r-1");
        assert_eq!(config.service_uuid, CALCULATOR_SERVICE_UUID);
        assert_eq!(config.rx_characteristic_uuid, CALCULATOR_RX_CHARACTERISTIC_UUID);
        assert_eq!(config.tx_characteristic_uuid, CALCULATOR_TX_CHARACTERISTIC_UUID);
        assert_eq!(config.local_name, "EVC:J:r-1");
    }
}
