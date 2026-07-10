# Evolve Calc Developer Guide

This document explains the current calculator application from architecture through runtime flow, UI behavior, native-module boundaries, testing, and planned BLE/security work.

## Product Summary

The app is a cross-platform desktop calculator prototype built with:

- Electron for desktop runtime.
- React + TypeScript for UI.
- Rust + `napi-rs` for the future native core.
- A TypeScript mock adapter while the native Rust module is still being connected.

The product concept is a shared calculator session where one desktop can host a room and other desktops can join. The host is the BLE central. Guests are BLE peripherals/advertisers. Once connected, calculator events can be shared across desktop environments.

Current implementation status:

- The UI is functional.
- Host/guest/session behavior is mocked.
- The Rust crate exists and mirrors the high-level API.
- Real BLE, SQLite, keychain, JWE/JWS/JWT/SD-JWT, issuer trust, and holder binding are planned native responsibilities.

## Language and Framework Choices

This project intentionally splits UI, desktop runtime, and native session logic:

| Layer | Language | Main libraries | Current purpose |
| --- | --- | --- | --- |
| Renderer UI | TypeScript + TSX | React, Vite, lucide-react | Draw the calculator, drawers, peers, history, and call high-level commands. |
| Electron preload | TypeScript | Electron `contextBridge`, `ipcRenderer` | Expose the narrow `window.calculator` bridge. |
| Electron main | TypeScript | Electron `BrowserWindow`, `ipcMain` | Own the app window, security settings, IPC handlers, and native adapter selection. |
| Shared contract | TypeScript | none | Define the API and state objects used by every TypeScript layer. |
| Native core | Rust | `napi-rs`, `serde`, `uuid`, `time` | Scaffold for the future BLE/security/session engine. |
| Tests | TypeScript, Rust | Vitest, Rust unit tests | Verify expression evaluation and native parser behavior. |

Electron is used because the target app needs the same desktop shell on Linux, macOS, and Windows across x64 and arm64. React and TypeScript are used for a strongly typed UI layer. Rust is used for the native engine because it is better suited for BLE, keychain integration, SQLite, cryptography, chunking, and trust validation than renderer JavaScript.

## Repository Map

```text
.
├── src/
│   ├── electron/
│   │   ├── main.ts
│   │   ├── preload.ts
│   │   └── native-calculator.ts
│   ├── renderer/
│   │   ├── main.tsx
│   │   ├── styles.css
│   │   ├── browser-calculator.ts
│   │   ├── global.d.ts
│   │   └── index.html
│   └── shared/
│       ├── calculator-api.ts
│       ├── expression.ts
│       └── expression.test.ts
├── crates/
│   └── native/
│       ├── Cargo.toml
│       ├── build.rs
│       ├── README.md
│       └── src/lib.rs
├── scripts/
│   ├── build-native.mjs
│   ├── build-native-optional.mjs
│   ├── browser-visual-ui-pass.mjs
│   ├── visual-ui-pass.mjs
│   └── visual-ui-screenshot-app.cjs
├── docs/
│   └── calculator-architecture.md
├── evolve-calculator-theme.md
├── package.json
├── vite.config.mts
├── vitest.config.mts
└── tsconfig.json
```

## Runtime Architecture

```text
Electron BrowserWindow
  loads Vite renderer in dev
  loads dist/renderer in production
        |
        v
Renderer React UI
  calculator surface
  host drawer
  history drawer
  browser-only mock fallback for visual review
        |
        v
window.calculator preload API
  available only in Electron
  exposes high-level commands
        |
        v
Electron Main IPC Handlers
  routes commands to calculator API implementation
        |
        v
Native API Adapter
  tries Rust napi-rs module first
  falls back to TypeScript mock adapter
        |
        v
Rust napi-rs Core
  scaffolded current state
  planned owner of BLE, crypto, storage, trust, keys
```

The UI never talks directly to Node.js, filesystem APIs, BLE APIs, private keys, SQLite, or raw crypto material. It receives final state/result objects only.

## Process and State Ownership

Current state ownership is deliberately simple:

| State or resource | Current owner | Future owner | Notes |
| --- | --- | --- | --- |
| Text expression in the input | Renderer React state | Renderer React state | This is view state only. |
| Room/session state | Mock adapter or Rust scaffold | Rust core | Returned as full `RoomState` after every command. |
| Peer discovery list | Mock adapter or Rust scaffold | Rust BLE core | Mock inserts fake Linux/Mac peers. |
| Calculation history | Mock adapter or Rust scaffold memory | Rust + SQLite | UI only renders the history it receives. |
| BLE scanning/advertising | Mock booleans | Rust with `btleplug` or platform-specific BLE code | Host is central; guests advertise as peripherals. |
| Device keypair | Not implemented | Rust + OS keychain | Renderer must never receive private keys. |
| Trust and credential validation | Mock trusted/pending flags | Rust, optionally backend | UI must not make final trust decisions. |

The central rule is that the renderer may optimistically calculate display previews, but it does not own final synced calculation state. The final state shown in history comes from `submitCalculation()`.

## Electron Main Process

File: `src/electron/main.ts`

Responsibilities:

- Creates the `BrowserWindow`.
- Enables `contextIsolation`.
- Disables `nodeIntegration`.
- Enables renderer sandboxing.
- Registers IPC handlers for calculator commands.
- Loads Vite dev server in development through `VITE_DEV_SERVER_URL`.
- Loads `dist/renderer/index.html` in production.

Important settings:

```ts
webPreferences: {
  preload: path.join(__dirname, "preload.js"),
  contextIsolation: true,
  nodeIntegration: false,
  sandbox: true
}
```

The main process calls `getCalculatorApi()` from `src/electron/native-calculator.ts`, then wires each command to IPC:

- `calculator:get-state`
- `calculator:create-room`
- `calculator:start-scanning`
- `calculator:connect-guest`
- `calculator:start-advertising`
- `calculator:accept-host-connection`
- `calculator:submit-calculation`

## Preload Bridge

File: `src/electron/preload.ts`

The preload script exposes exactly one API:

```ts
window.calculator
```

The exposed API implements `NativeCalculatorApi` from `src/shared/calculator-api.ts`.

It wraps `ipcRenderer.invoke()` calls and does not expose:

- Node.js APIs.
- Filesystem access.
- BLE access.
- Raw native module handles.
- Secrets.
- Raw decrypted payloads.

This keeps the renderer restricted to high-level commands.

## Shared API Contract

File: `src/shared/calculator-api.ts`

This is the contract between:

- Renderer.
- Preload.
- Electron main.
- TypeScript mock adapter.
- Rust native module.

Core types:

```ts
type SessionRole = "host" | "guest";
type BleRole = "central" | "peripheral";
type TrustStatus = "trusted" | "untrusted" | "pending";
```

The main state object is `RoomState`:

```ts
interface RoomState {
  localDeviceId: string;
  roomId: string | null;
  roomName: string | null;
  sessionRole: SessionRole | null;
  bleRole: BleRole | null;
  scanning: boolean;
  advertising: boolean;
  peers: PeerSummary[];
  history: CalculationEntry[];
}
```

The high-level API is:

```ts
interface NativeCalculatorApi {
  getState(): Promise<RoomState>;
  createRoom(request: CreateRoomRequest): Promise<RoomState>;
  startScanning(): Promise<RoomState>;
  connectGuest(request: ConnectGuestRequest): Promise<RoomState>;
  startAdvertising(request: StartAdvertisingRequest): Promise<RoomState>;
  acceptHostConnection(): Promise<RoomState>;
  submitCalculation(request: SubmitCalculationRequest): Promise<RoomState>;
}
```

Every command returns the full `RoomState`. This keeps the renderer simple and avoids partial client-side trust decisions.

### Command Behavior Table

| Command | UI trigger | Current TypeScript/Rust behavior | Future native behavior |
| --- | --- | --- | --- |
| `getState()` | App load | Returns the current in-memory state. | Load durable session/device state from Rust and SQLite where appropriate. |
| `createRoom({ roomName })` | Host Bench `Start` | Sets `sessionRole: "host"`, `bleRole: "central"`, creates `roomId`, clears advertising. | Create a room, create/restore device identity, prepare host-central BLE scan/connect flow. |
| `startScanning()` | Host Bench `Scan` | Sets `scanning: true`, inserts one fake guest peer when none exist. | Start BLE central scanning for guest advertisements that match the room/session schema. |
| `connectGuest({ peerId })` | Peer row `Connect` | Marks a matching peer connected and trusted. | Connect to the peripheral, establish session transport, validate identity/trust, return updated peer state. |
| `startAdvertising({ roomCode })` | Host Bench `Signal` | Sets `sessionRole: "guest"`, `bleRole: "peripheral"`, `advertising: true`. | Start guest BLE advertising/GATT server for a host to scan and connect. |
| `acceptHostConnection()` | Host Bench `Accept` | Stops advertising and inserts a fake connected Mac host. | Complete peripheral-side host connection acceptance and trust/session setup. |
| `submitCalculation({ expression })` | `Sync`, `Enter`, `=` | Evaluates expression, prepends a trusted history item. | Package calculation event, sign/encrypt as required, sync to peers, persist to SQLite, return final validated state. |

The API is intentionally command-oriented. The renderer should not mutate `RoomState` locally except by replacing it with the state returned from these commands.

## Renderer UI

Main file: `src/renderer/main.tsx`

Styling: `src/renderer/styles.css`

Browser-review fallback: `src/renderer/browser-calculator.ts`

### Main UI Areas

The UI has three primary regions:

```text
left drawer     center calculator     right drawer
Host Bench      Evolve Calc           Peers + History
```

The calculator is always the central primary surface. The side areas are collapsible drawers:

- Host Bench expands from the left.
- Peers/History expands from the right.
- Opening one drawer closes the other.
- Compact browser widths use horizontal scrolling rather than overlapping panels.

This is intentional. The design prioritizes non-overlap and stable component sizes over shrinking controls until they become unreadable.

### React Component Map

All renderer components currently live in `src/renderer/main.tsx`.

| Component/function | Responsibility |
| --- | --- |
| `App` | Owns React state, calls calculator API, renders the shell, drawers, calculator, history, and status rail. |
| `PanelHeader` | Reusable drawer header with eyebrow and title. |
| `StatusPill` | Bottom status rail pill. |
| `SegmentedRole` | Visual role selector/status. It displays current role but does not currently switch role directly. |
| `SessionFacts` | Shows device ID, room ID, pending action, and trust state. |
| `StatusBeacon` | Shows `Idle`, `Scanning`, or `Signal` in the peer drawer. |
| `PeerRow` | Renders one peer and its `Connect`/`Connected` action. |
| `shortId` | Truncates long device IDs for compact display. |

Important React state:

| State | Purpose |
| --- | --- |
| `state` | Latest `RoomState` returned by the calculator API. |
| `roomName` | Host room name input. |
| `roomCode` | Guest advertising room code input. |
| `expression` | Editable calculator expression. |
| `pendingAction` | Disables actions and shows what async command is in flight. |
| `error` | Stores bridge/native command errors for the Host Bench error box. |
| `hostOpen` | Controls the left drawer expansion. |
| `historyOpen` | Controls the right drawer expansion. |

Both drawers open by default when the viewport can fit the full layout. `toggleHostBench()` and `toggleHistory()` allow both drawers to stay open at those widths. When the viewport is too narrow, opening one drawer closes the other. If the window is resized smaller while both drawers are open, the most recently opened drawer stays open.

### Host Bench

Host Bench controls the room/session mock flow.

Fields and actions:

- `Host room`: room name input.
- `Start`: calls `createRoom({ roomName })`.
- `Guest advertising`: room code input.
- `Signal`: calls `startAdvertising({ roomCode })`.
- `Scan`: calls `startScanning()`.
- `Accept`: calls `acceptHostConnection()`.

Session facts:

- Device ID.
- Room ID.
- Current action.
- Trust status.

### Calculator Surface

The center calculator has:

- Header: `Evolve Calc`, mode chip `Standard`.
- Display: expression and live preview result.
- Text input: editable expression.
- `Sync`: sends the calculation event.
- Keypad: number, operator, utility, destructive, and equals keys.

Key model:

```ts
type CalculatorKey =
  | { label: string; value: string; role?: "number" | "operator"; ariaLabel?: string }
  | { label: string; action: "clear" | "delete" | "equals"; role: "danger" | "utility" | "equals"; ariaLabel?: string };
```

Current keys:

```text
AC DEL % ÷
7  8   9 ×
4  5   6 -
1  2   3 +
0  .   =
```

Actions:

- `AC`: clears the expression.
- `DEL`: removes the last character.
- `=`: submits the current expression through `submitCalculation`.
- Operators insert expression tokens.
- `Enter` inside the expression input also submits.

Expression preview:

- The display preview is computed locally with `calculateExpression(expression)`.
- Invalid local preview renders as `Waiting`.
- Final history is not written locally; it is written only after `submitCalculation()` returns a new `RoomState`.

This means the preview is a convenience feature, not the trusted synced result.

### Right Drawer

The right drawer contains:

- Peer discovery/connect state.
- History event list.

The history list is internally scrollable. It should not expand the panel or push the calculator around.

### Browser Mock API

File: `src/renderer/browser-calculator.ts`

When the app is opened directly in a browser at `http://127.0.0.1:5173`, Electron preload does not exist, so `window.calculator` is undefined. For visual review and browser screenshot automation, the renderer falls back to a local browser mock:

```ts
const calculatorApi = useMemo(() => window.calculator ?? createBrowserCalculatorApi(), []);
```

This fallback is renderer-only. Electron still uses the preload bridge and main-process adapter.

## Styling and Responsive Rules

The theme is based on `evolve-calculator-theme.md`.

Core visual direction:

- Dark charcoal workbench.
- Cream calculator display.
- Orange operators.
- Yolk equals key.
- Brick `AC`.
- Leaf green sync/action.
- Compact utility proportions.

Important layout constraints:

- Calculator is fixed at `420px` wide.
- Side collapsed columns are `52px`.
- Host drawer expands to `300px`.
- History drawer expands to `330px`.
- Components use `overflow: hidden`, `text-overflow: ellipsis`, or internal scrolling to avoid text overlap.
- Compact widths intentionally allow horizontal page scrolling instead of shrinking the calculator below its usable size.

Visual regression screenshots are generated by `scripts/browser-visual-ui-pass.mjs`.

### Layout Invariants

These values are intentional and should be preserved unless the visual pass is updated at the same time:

| Selector | Constraint | Why it exists |
| --- | --- | --- |
| `.workspace` | `grid-template-columns: 52px 420px 52px` closed | Keeps drawer toggles visible without competing with the calculator. |
| `.workspace.host-open` | `300px 420px 52px` | Opens Host Bench while calculator remains fixed. |
| `.workspace.history-open` | `52px 420px 330px` | Opens Peers/History while calculator remains fixed. |
| `.workspace.host-open.history-open` | `300px 420px 330px` | Opens both side panels when the viewport can fit the full workbench. |
| `.calculator-panel` | `width/min-width: 420px`, `height: 620px` | Prevents keypad, display, and input controls from collapsing into overlap. |
| `.left-panel`, `.right-panel` | `height: 620px`, `overflow: hidden` | Keeps drawers aligned with calculator and avoids vertical growth. |
| `.peer-list`, `.history-list` | `overflow: auto` | Lists scroll internally instead of stretching the drawer. |
| `body` and `.app-shell` | page overflow is allowed | Very narrow windows scroll horizontally instead of destroying component geometry. |

Text overflow rules are part of the design:

- Long IDs use `shortId()` or CSS ellipsis.
- Peer labels, role text, and history results use `minmax(0, 1fr)` with `text-overflow: ellipsis`.
- Display expressions use `overflow-wrap: anywhere` so long user input wraps inside the fixed display.
- Buttons and compact pills have fixed or bounded widths so labels cannot resize surrounding layout.

## Calculator Expression Logic

File: `src/shared/expression.ts`

The TypeScript evaluator supports:

- Addition: `+`
- Subtraction: `-`
- Multiplication: `*`
- Division: `/`
- Modulo: `%`
- Decimal values.
- Unary negative numbers in simple expressions.

It does not currently support:

- Parentheses.
- Functions.
- Scientific notation.
- Variables.

High-level flow:

1. Remove whitespace.
2. Validate the expression with a conservative regex.
3. Tokenize numbers and operators.
4. Evaluate using two stacks:
   - `values`
   - `ops`
5. Apply precedence:
   - `+` and `-`: precedence 1.
   - `*`, `/`, `%`: precedence 2.
6. Return a rounded finite result or `Invalid expression`.

The Rust crate includes a parallel implementation in `crates/native/src/lib.rs` for the native module scaffold.

### TypeScript Evaluator Details

The TypeScript evaluator uses this validation regex after removing whitespace:

```ts
/^-?\d+(\.\d+)?([+\-*/%]-?\d+(\.\d+)?)*$/
```

That means accepted examples include:

```text
7 + 5 * 2
-4+2
10/4
10%4
```

Rejected examples include:

```text
7 + nope
1+
(1+2)
.5+1
1e3+2
```

After validation, the evaluator tokenizes with:

```ts
/-?\d+(?:\.\d+)?|[+\-*/%]/g
```

Then it applies a standard two-stack algorithm. Operators with greater or equal precedence already on the stack are applied before pushing the next operator. Division or modulo by zero produces `Number.NaN`, which is returned to the UI as `Invalid expression`.

The final numeric value is rounded to 8 decimal places:

```ts
String(Number(result.toFixed(8)))
```

This removes floating-point noise and trailing zeros.

## TypeScript Mock Native Adapter

File: `src/electron/native-calculator.ts`

This adapter does two things:

1. Attempts to load a real native module.
2. Falls back to an in-memory TypeScript mock when the native module is missing.

Native candidates:

```ts
const nativeCandidates = [
  "../../../index.js",
  "../../../crates/native/index.js",
  "../../../crates/native/ble_calculator_native.node",
  "../../../crates/native/ble-calculator-native.node"
];
```

Mock state behavior:

- `createRoom` marks the app as host/central.
- `startScanning` inserts a fake Linux guest.
- `connectGuest` marks the peer trusted and connected.
- `startAdvertising` marks the app as guest/peripheral.
- `acceptHostConnection` inserts a fake Mac host.
- `submitCalculation` evaluates and inserts a history entry.

This lets the Electron app run before the Rust module is compiled.

### Adapter Loading Rules

`getCalculatorApi()` caches the first working adapter in module scope. Loading order:

1. Try `../../../index.js`.
2. Try `../../../crates/native/index.js`.
3. Try `../../../crates/native/ble_calculator_native.node`.
4. Try `../../../crates/native/ble-calculator-native.node`.
5. If none load and the error is only `MODULE_NOT_FOUND`, use the TypeScript mock silently.
6. If a candidate exists but fails for another reason, log a warning and continue to the next candidate.

The loaded module must expose every function in `NativeCalculatorApi`. Partial native modules are rejected and the mock is used.

## Rust Native Module

Files:

- `crates/native/Cargo.toml`
- `crates/native/build.rs`
- `crates/native/src/lib.rs`

The Rust crate is a `cdylib` built through `napi-rs`.

Current dependencies:

- `napi`
- `napi-derive`
- `once_cell`
- `serde`
- `serde_json`
- `time`
- `uuid`

Current exported functions mirror the TypeScript API:

- `get_state`
- `create_room`
- `start_scanning`
- `connect_guest`
- `start_advertising`
- `accept_host_connection`
- `submit_calculation`
- `get_native_runtime_status`
- `validate_credential_bundle`

Current Rust UI/session state is held in memory, while calculation history and sync outbox records are persisted to SQLite:

```rust
static APP_STATE: Lazy<Mutex<RoomState>> = Lazy::new(|| Mutex::new(RoomState::new()));
```

The Rust module currently returns serialized JSON-compatible objects to Node.

### Rust State and Command Logic

Rust mirrors the TypeScript API with snake_case exported function names. `napi-rs` maps these into JavaScript-callable functions.

State is stored in:

```rust
static APP_STATE: Lazy<Mutex<RoomState>> = Lazy::new(|| Mutex::new(RoomState::new()));
```

Every exported command calls `with_state_json()`, which:

1. Locks the global `RoomState`.
2. Runs the command closure.
3. Serializes the returned value with `serde_json`.
4. Returns `serde_json::Value`, which `napi-rs` converts into a JavaScript value.

The Rust structs use `#[serde(rename_all = "camelCase")]` so the JSON shape matches TypeScript's `RoomState`, `PeerSummary`, and `CalculationEntry`.

Rust expression evaluation is similar to the TypeScript evaluator but implemented as a parser:

1. Remove whitespace.
2. Convert the string into `Token::Number(f64)` and `Token::Op(char)`.
3. Track whether the parser expects a number so unary negative numbers are accepted only in number positions.
4. Apply operator precedence with a value stack and operator stack.
5. Reject incomplete expressions or unknown tokens by returning `None`.
6. Return `Invalid expression` for invalid or non-finite results.

Rust currently has unit tests for operator precedence, invalid expressions, and modulo.

### Native Implementation Status

Implemented now:

- SQLite local calculation history.
- SQLite sync outbox for signed calculation events.
- OS keychain-backed local signing key loading/creation where supported by the `keyring` crate.
- Ed25519 signing for local calculation events.
- Holder key binding for local signed calculation events.
- Host central scan attempts through `btleplug`.
- Host scanning filters for Evolve Calc room/join advertisement metadata rather than showing every nearby BLE peripheral.
- Guest room discovery, room join request state, and BLE reset commands are part of the shared native API.
- Guest peripheral advertising via a per-OS backend behind the `ble::BlePeripheral` trait; macOS uses CoreBluetooth `CBPeripheralManager`, Linux uses BlueZ via `bluer`, and Windows uses WinRT `GattServiceProvider` (each advertising the calculator GATT service). See `docs/runtime-native-and-security.md` → "Cross-Platform BLE Peripheral Backends".
- BLE chunk framing and reassembly helpers for signed event payloads.
- Native runtime status and warnings on `RoomState`.
- Fail-closed credential validation placeholder through `validate_credential_bundle()`.

Still pending (the authoritative, consolidated roadmap lives in
`docs/runtime-native-and-security.md` → "Outstanding Work (TODO)"; the summary
below mirrors it):

- Windows guest advertising carries only the service UUID: WinRT `GattServiceProvider` cannot set the custom `EVC:J:<room>` local name, so a Windows guest is connectable but not fully self-describing until a `BluetoothLEAdvertisementPublisher` (or a host-side service-UUID-only rule) is added.
- Full receive-side GATT transport: the macOS/Linux/Windows peripherals buffer inbound host writes, but reassembly + signature/holder verification of received events and TX notify delivery are not wired end-to-end yet.
- On-device validation of all three peripheral backends: macOS CoreBluetooth (two machines + Bluetooth permission), Linux `bluer` (a Linux host with `bluetoothd`), and Windows `GattServiceProvider` (a Windows host). None can be compiled on the other OSes.
- Marking SQLite outbox rows delivered after real cross-device transport succeeds.
- JWE decryption.
- JWT/JWS/SD-JWT issuer/key resolution and verification.
- Issuer trust policy and revocation checks.

### Planned Rust Responsibilities

The Rust core should continue growing to own:

- BLE session lifecycle.
- Host central scanning and guest connection flow.
- Guest peripheral advertising/GATT server flow.
- BLE chunking and reassembly.
- Nonce generation.
- Session cryptography.
- OS keychain-backed private key storage.
- SQLite event/session persistence.
- JWE decryption.
- JWS/JWT/SD-JWT verification.
- Issuer trust validation.
- Holder key binding validation.
- Final validation result returned to UI.

The renderer should never receive raw secrets or make final trust decisions.

## BLE Role Model

Product terminology:

```text
Session host = user who creates the room.
Session guest = user who joins the room.
```

BLE terminology:

```text
Central = scanner / initiator / connector.
Peripheral = advertiser / connectable target.
```

For this app:

```text
Host desktop
  SessionRole: host
  BleRole: central
  Behavior:
    creates room
    scans for advertising guests
    connects to guests
    owns shared calculator session

Guest desktop
  SessionRole: guest
  BleRole: peripheral
  Behavior:
    advertises room join capability
    accepts connection from host
    sends/receives calculator state
```

Keep `SessionRole` and `BleRole` separate in code. They are related but not the same concept.

### Planned BLE Message Flow

The current app does not send real BLE packets yet, but the intended direction is:

1. Guest enters a room code and starts advertising.
2. Host creates a room and starts scanning as BLE central.
3. Host filters guest advertisements by service UUID and room/session metadata.
4. Host connects to selected guest peripheral.
5. Rust establishes or restores a secure session.
6. Calculation events are chunked if needed and sent over the BLE characteristic protocol.
7. Receiver reassembles chunks before verification/decryption.
8. Rust validates payload integrity, issuer/holder trust, and session binding.
9. Rust appends the validated calculation event to SQLite and returns final `RoomState`.

Future BLE message design should include:

- Protocol version.
- Room/session ID.
- Message ID.
- Chunk index and total chunks.
- Nonce or session sequence number.
- Payload type.
- Payload bytes.
- Authentication/signature metadata.

The UI should never parse this protocol directly.

## End-to-End User Flows

### Load App

1. Electron main creates a secure browser window.
2. Preload exposes `window.calculator`.
3. Renderer chooses `window.calculator`.
4. If opened in a browser, renderer chooses `createBrowserCalculatorApi()`.
5. Renderer calls `getState()`.
6. State is displayed in drawers/status rail.

### Host Creates Room

1. User enters a host room name.
2. User clicks `Start`.
3. Renderer calls `createRoom({ roomName })`.
4. Main process forwards through IPC.
5. Native/mock API sets:
   - `sessionRole = "host"`
   - `bleRole = "central"`
   - `roomId`
   - `roomName`
6. Full `RoomState` returns to UI.

### Host Finds and Approves Guest

1. User clicks `Find`.
2. Renderer calls `startScanning()`.
3. Native host scanning uses `btleplug` as central and filters for Evolve Calc join-request advertisements for the active room.
4. Browser/mock adapters insert one deterministic guest request for UI review.
5. User clicks `Approve`.
5. Renderer calls `connectGuest({ peerId })`.
6. Native attempts to connect to the discovered BLE peripheral and discover services.
7. If connect succeeds, the peer is marked connected/trusted in the local session state.
8. Status rail updates connected count and trust label.

The current native code does not yet write or subscribe to a calculator GATT characteristic after connection.

### Guest Finds and Joins Room

1. User clicks `Rooms`.
2. Renderer calls `scanRooms()`.
3. Native guest room discovery uses the same central scan machinery and filters for Evolve Calc room advertisements.
4. Browser/mock adapters insert one deterministic room for UI review.
5. User clicks `Join` on a room, or enters a manual room id and clicks `Join`.
6. Renderer calls `joinRoom({ roomId })`.
7. Native/mock sets:
   - `sessionRole = "guest"`
   - `bleRole = "peripheral"`
   - `advertising = true`
   - `roomId`
8. UI reflects guest/peripheral request state.

Real native implementation still needs a platform-specific peripheral advertiser/GATT server so the host Mac can actually discover and connect to the guest Mac.

### Reset BLE Session

1. User clicks `Reset` in the host or guest section.
2. Renderer calls `resetBleSession()`.
3. Native/mock clears:
   - `roomId`
   - `roomName`
   - `sessionRole`
   - `bleRole`
   - `scanning`
   - `advertising`
   - `peers`
   - `rooms`
4. Calculation history remains intact.

### Submit Calculation

1. User types or taps keys.
2. UI previews result locally with `calculateExpression`.
3. User clicks `Sync`, presses `Enter`, or taps `=`.
4. Renderer calls `submitCalculation({ expression })`.
5. Native/mock evaluates expression and appends `CalculationEntry`.
6. UI updates history list.

Future implementation should sign/encrypt/session-wrap the calculation event before transmission.

### Error Flow

1. Renderer calls `runAction(label, action)`.
2. `pendingAction` is set to `label`.
3. Existing `error` is cleared.
4. If the command resolves, returned `RoomState` replaces local `state`.
5. If the command rejects, `error` is set from the thrown error message.
6. `pendingAction` is cleared in `finally`.

The Host Bench renders `error` inside `.error-box`. Buttons disable while `pendingAction` is non-null to prevent overlapping command calls from the UI.

## Security Boundaries

Current intended boundary:

```text
Renderer:
  UI only
  no private keys
  no raw BLE packets
  no raw decrypted credentials
  no final trust decisions

Preload:
  narrow IPC bridge only

Electron Main:
  process lifecycle
  IPC routing
  native adapter selection

Rust Core:
  BLE
  keychain
  SQLite
  crypto
  issuer trust
  holder binding
  final validation result
```

When implementing real security:

- Do not expose private keys to renderer.
- Do not expose raw JWE/JWT/JWS/SD-JWT payloads to renderer unless explicitly safe.
- Return final validation result objects, not raw trust internals.
- Validate IPC request shapes before native execution.
- Keep renderer sandboxing and context isolation enabled.

### High-Assurance Backend Option

If the product moves into high-assurance credential validation, the backend should perform final issuer/trust/audit validation. In that mode:

1. Rust still owns local BLE, keychain, nonce creation, chunk reassembly, JWE decrypt, signature checks, and holder binding.
2. Rust sends only the minimum validation/audit package required by the backend.
3. Backend performs authoritative issuer policy, revocation, audit logging, and risk decisions.
4. Backend returns a final validation decision.
5. Rust returns a summarized final result to the renderer.

The renderer still displays the result only; it does not become a verifier.

## Build and Development Commands

Install dependencies:

```sh
npm install
```

Run dev app:

```sh
npm run dev
```

Typecheck:

```sh
npm run typecheck
```

Lint:

```sh
npm run lint
```

Tests:

```sh
npm run test
```

Build Electron/renderer:

```sh
npm run build
```

Build Rust native module:

```sh
npm run build:native
```

The native build scripts discover Cargo through:

```sh
rustup which cargo
```

This is needed because some shells may have `rustup` installed but not have Cargo's toolchain bin directory on `PATH`.

## Visual UI Pass

Run the browser visual pass:

```sh
node scripts/browser-visual-ui-pass.mjs
```

Prerequisites:

- Vite dev server running at `http://127.0.0.1:5173`.
- Google Chrome installed at `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

Outputs:

```text
artifacts/browser-visual-ui-pass/
├── desktop-closed.png
├── desktop-left-open.png
├── desktop-right-open.png
├── compact-closed.png
├── compact-left-open.png
├── compact-right-open.png
├── short-height-right-open.png
├── report.json
├── chrome-stdout.log
└── chrome-stderr.log
```

The report includes bounding boxes for left, center, and right components and flags overlap between:

- left vs center
- center vs right
- left vs right

The latest pass reported zero overlaps across all captured states.

## Known Current Limitations

- Real BLE is not implemented yet.
- SQLite is not implemented yet.
- OS keychain integration is not implemented yet.
- JWE/JWS/JWT/SD-JWT validation is not implemented yet.
- Issuer trust and holder binding are not implemented yet.
- The Rust module mirrors the mock behavior and keeps state in memory.
- The TypeScript browser mock exists only for browser visual review.
- The expression evaluator is intentionally simple and does not support parentheses.

## Suggested Next Development Steps

1. Finish native module compilation and make Electron load the `.node` artifact.
2. Add SQLite persistence in Rust.
3. Add keypair creation/loading through OS keychain.
4. Define BLE message schema for calculator events.
5. Implement host central scanning flow.
6. Implement guest peripheral advertising flow.
7. Implement chunking/reassembly.
8. Add session nonce and key agreement.
9. Add signed/encrypted calculator event transport.
10. Add real trust validation result model.
11. Replace mock peer insertion with native BLE discovery events.
12. Add integration tests around native command outputs.
