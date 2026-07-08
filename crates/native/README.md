# ble-calculator-native

Rust `napi-rs` module for the desktop calculator sync core.

Current scaffold:

- Exposes the same high-level commands consumed by Electron.
- Keeps calculator/session state in Rust.
- Uses mock BLE peer data until the platform BLE implementation is added.

Planned native responsibilities:

- Host central scanning/connection flow.
- Guest peripheral advertising/GATT flow.
- BLE chunking and reassembly.
- OS keychain-backed device keypair.
- SQLite event/session persistence.
- JWE decrypt and JWT/JWS/SD-JWT verification.
- Issuer trust and holder key binding checks.
