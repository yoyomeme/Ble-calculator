//! Fallback BLE peripheral backend for operating systems without a dedicated
//! implementation. macOS (CoreBluetooth), Linux (BlueZ/`bluer`), and Windows
//! (`GattServiceProvider`) all have real backends, so this covers only other
//! targets.
//!
//! It is intentionally *not* a silent no-op: `start_advertising` fails loud with
//! a typed message so the calculator surfaces "not implemented on this platform"
//! instead of pretending the guest is discoverable.

use super::{BlePeripheral, PeripheralConfig};

pub struct StubPeripheral;

impl StubPeripheral {
    pub fn new() -> Self {
        Self
    }
}

impl BlePeripheral for StubPeripheral {
    fn platform(&self) -> &'static str {
        "stub-unsupported"
    }

    fn is_supported(&self) -> bool {
        false
    }

    fn start_advertising(&mut self, _config: &PeripheralConfig) -> Result<(), String> {
        Err(
            "Guest BLE peripheral advertising is not implemented on this operating system."
                .to_string(),
        )
    }

    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn is_advertising(&self) -> bool {
        false
    }

    fn take_inbound(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }

    fn notify(&mut self, _frames: &[Vec<u8>]) -> Result<usize, String> {
        Err(
            "Guest BLE notify (guest -> host) is not implemented on this operating system."
                .to_string(),
        )
    }

    fn has_subscriber(&self) -> bool {
        false
    }
}
