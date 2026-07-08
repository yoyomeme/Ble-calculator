# BLE Calculator Desktop

Cross-platform Electron calculator sync app with a Rust `napi-rs` core.

Developer documentation:

- [Developer onboarding](docs/developer-onboarding.md)
- [Architecture guide](docs/calculator-architecture.md)
- [UI layout and visual pass](docs/ui-layout-and-visual-pass.md)
- [Runtime, native core, and security notes](docs/runtime-native-and-security.md)

Current state:

- Electron + React + TypeScript desktop UI.
- Secure preload bridge exposing only high-level calculator commands.
- Rust `napi-rs` native module scaffold in `crates/native`.
- Development fallback adapter so the UI runs before the Rust module is built.
- Browser-only mock adapter for visual UI review at `http://127.0.0.1:5173`.
- Mock host/guest flow for the planned BLE model:
  - host desktop: session host, BLE central, scans/connects to guests
  - guest desktop: session guest, BLE peripheral/advertiser

## Commands

```sh
npm install
npm run dev
npm run typecheck
npm run lint
npm run test
npm run build
npm run build:native
```

`npm run build` skips the native module when `cargo` is unavailable and uses the mock adapter. `npm run build:native` is strict and fails if the Rust native module cannot compile.

## Visual UI Pass

With Vite running:

```sh
node scripts/browser-visual-ui-pass.mjs
```

Screenshots and overlap metrics are written to:

```text
artifacts/browser-visual-ui-pass/
```

## Native Core

Install Rust before building the native module:

```sh
rustup default stable
npm run build:native
```

The native build scripts also try `rustup which cargo` and prepend the discovered toolchain bin directory to `PATH`, because this machine has `rustup` available even when `cargo` is not on the default shell path.

Planned Rust responsibilities:

- BLE central scanning and guest connection flow.
- BLE peripheral advertising/GATT server flow.
- BLE chunk reassembly.
- OS keychain-backed private key storage.
- SQLite event/session persistence.
- JWE decrypt and JWS/JWT/SD-JWT verification.
- Issuer trust and holder key binding validation.
