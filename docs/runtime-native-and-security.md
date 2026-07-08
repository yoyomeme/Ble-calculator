# Runtime, Native Core, and Security Notes

This document explains how commands move from the UI into Electron and the Rust native scaffold, plus the security boundary expected for the future BLE implementation.

## Runtime Path

Current command path:

```text
React UI
  calls calculatorApi.createRoom(...)
        |
        v
window.calculator from preload
  invokes calculator:create-room
        |
        v
Electron main ipcMain handler
  calls getCalculatorApi().createRoom(...)
        |
        v
Native adapter
  loads Rust napi-rs module if available
  otherwise uses TypeScript mock
        |
        v
RoomState returned to renderer
```

Every public command returns a complete `RoomState`.

## Electron Security Settings

`src/electron/main.ts` creates the window with:

```ts
contextIsolation: true,
nodeIntegration: false,
sandbox: true
```

This means the renderer cannot use Node APIs directly. The preload file is the only bridge.

`src/electron/preload.ts` exposes:

```ts
contextBridge.exposeInMainWorld("calculator", calculatorApi);
```

The exposed API contains only high-level calculator commands:

- `getState`
- `createRoom`
- `startScanning`
- `connectGuest`
- `startAdvertising`
- `acceptHostConnection`
- `submitCalculation`

Do not expose raw native handles, filesystem APIs, BLE packets, private keys, decrypted credentials, or SQLite access through preload.

## IPC Channels

The shared TypeScript contract defines channel names in `src/shared/calculator-api.ts`:

```ts
calculator:get-state
calculator:create-room
calculator:start-scanning
calculator:connect-guest
calculator:start-advertising
calculator:accept-host-connection
calculator:submit-calculation
```

The main process registers one `ipcMain.handle()` per channel. The preload calls the same channels with `ipcRenderer.invoke()`.

The current code trusts TypeScript types at compile time. Before this becomes a real security-sensitive app, runtime validation should be added in main or native code for every request object.

## Native Adapter Loading

`src/electron/native-calculator.ts` tries native module candidates first:

```text
../../../index.js
../../../crates/native/index.js
../../../crates/native/ble_calculator_native.node
../../../crates/native/ble-calculator-native.node
```

If a candidate cannot be found, loading continues. If a candidate exists but throws another error, the error is logged as a warning and loading continues.

The module must implement the full `NativeCalculatorApi`. If any command is missing, the module is rejected.

When no valid native module loads, the app uses an in-memory TypeScript mock. This keeps UI development unblocked while Rust is being built.

## Mock Adapter Behavior

The TypeScript mock stores one mutable `RoomState` object in memory.

| Command | Mock behavior |
| --- | --- |
| `getState` | Returns a cloned state. |
| `createRoom` | Creates a `room-*` ID, sets role to host/central, clears advertising. |
| `startScanning` | Enables scanning and inserts fake `Linux Calculator` guest if no peers exist. |
| `connectGuest` | Marks matching peer connected and trusted. |
| `startAdvertising` | Sets role to guest/peripheral, sets room from code, enables advertising, stops scanning. |
| `acceptHostConnection` | Stops advertising and inserts fake connected `Mac Host`. |
| `submitCalculation` | Trims expression, evaluates it, prepends a trusted history entry. |

The mock returns `cloneState(state)` through `JSON.parse(JSON.stringify(state))` to avoid leaking mutable internal state to callers.

## Rust Native Scaffold

The Rust crate lives in `crates/native`.

Important files:

| File | Purpose |
| --- | --- |
| `Cargo.toml` | Rust package metadata, `cdylib`, `napi` dependencies. |
| `build.rs` | Runs `napi_build::setup()`. |
| `src/lib.rs` | Exported commands, in-memory state, expression parser, Rust tests. |

Current dependencies:

- `napi`
- `napi-derive`
- `once_cell`
- `serde`
- `serde_json`
- `time`
- `uuid`

Current state:

```rust
static APP_STATE: Lazy<Mutex<RoomState>> = Lazy::new(|| Mutex::new(RoomState::new()));
```

Current exported command names:

- `get_state`
- `create_room`
- `start_scanning`
- `connect_guest`
- `start_advertising`
- `accept_host_connection`
- `submit_calculation`

Rust structs use `#[serde(rename_all = "camelCase")]`, so returned objects match TypeScript field names such as `localDeviceId`, `roomId`, and `createdAtIso`.

## Native Build

Strict native build:

```sh
npm run build:native
```

Optional native build used by `npm run build`:

```sh
npm run build:native:optional
```

Both scripts call:

```text
npx napi build --cargo-cwd crates/native --platform --release
```

Before running `npx napi`, the scripts try:

```text
rustup which cargo
```

If Cargo is found through rustup, the toolchain binary directory is prepended to `PATH`. This matters on machines where `rustup` is installed but `cargo` is not visible in the default shell environment.

The current `napi-rs` build emits generated files at the project root, for example:

```text
index.js
index.d.ts
index.darwin-arm64.node
```

These files are generated build artifacts. ESLint and Git ignore them, and the Electron native adapter tries the generated `index.js` wrapper first.

## Planned Native Architecture

The Rust core should grow from the current in-memory scaffold into the owner of session-critical work:

```text
Rust core
  device identity
  OS keychain access
  SQLite persistence
  BLE scanning/advertising/connection
  BLE message chunking/reassembly
  nonce/session management
  JWE decrypt
  JWS/JWT/SD-JWT verification
  issuer trust checks
  holder key binding
  final validation result
```

The renderer should continue to receive only final UI-safe state.

## BLE Roles

Session role and BLE role are different fields:

| Product role | BLE role | Behavior |
| --- | --- | --- |
| Host | Central | Creates room, scans for guests, connects to guests. |
| Guest | Peripheral | Advertises itself, accepts host connection. |

The requested product behavior is:

```text
Host creates a room.
Guests advertise themselves.
Host scans and connects to guests.
Host is the BLE central.
```

Keep that model intact when replacing the mock with real BLE code.

## Secure Storage Plan

Private keys should be created or loaded by Rust and stored using OS-backed secure storage:

| Platform | Expected secure storage |
| --- | --- |
| macOS | Keychain |
| Windows | Credential Manager or DPAPI-backed storage |
| Linux | Secret Service/libsecret where available, with a documented fallback policy |

The renderer must never receive private keys or raw seed material. If the UI needs identity information, return public identifiers or fingerprints only.

## SQLite Plan

SQLite should be owned by Rust, not the renderer.

Likely tables:

- device identity metadata
- rooms/sessions
- peers
- calculation events
- trust decisions
- audit records

Use SQLite for durable local state and auditability. Keep the UI contract as `RoomState` until there is a clear reason to add paged history or event streaming.

## Credential and Trust Plan

For high assurance, Rust should own local verification steps:

1. Create nonce or session challenge.
2. Reassemble BLE chunks.
3. Decrypt JWE.
4. Verify JWS/JWT/SD-JWT signatures.
5. Check holder key binding.
6. Check local issuer trust policy.
7. Return a final validation result or send an audit package to backend.

If a backend is used, backend should own final issuer/trust/audit validation. The renderer still receives only the final validation result.

## Request Validation Needed

Before shipping beyond prototype, add runtime validation for:

- `roomName` maximum length and allowed characters.
- `roomCode` maximum length and allowed characters.
- `peerId` existence and shape.
- `expression` maximum length.
- command authorization based on current role.
- invalid state transitions, such as scanning while guest/peripheral.

TypeScript interfaces are useful for development but are not a security boundary.

## Failure Handling Needed

The current UI can display a command error, but native work should define structured failures:

- BLE unavailable.
- Bluetooth permission denied.
- No adapter found.
- Peer disappeared.
- Connection failed.
- Keychain unavailable.
- SQLite unavailable.
- Invalid payload.
- Trust validation failed.
- Backend validation unavailable.

Prefer returning typed error codes from Rust and mapping them to concise UI messages in Electron or the renderer.
