# Developer Onboarding

This is the shortest path for a new developer to understand the calculator app.

## What This App Is

This is a cross-platform desktop calculator sync prototype. It uses Electron for the desktop shell, React + TypeScript for the UI, and a Rust `napi-rs` crate for the future native BLE/security engine.

The product model is:

```text
Host desktop creates a room.
Guest desktops advertise themselves.
Host scans and connects to guests.
Host is BLE central.
Guests are BLE peripherals.
Calculator events are shared after connection.
```

The current implementation has a working calculator UI and mocked host/guest behavior. Real BLE, secure key storage, SQLite persistence, and credential/trust validation are planned Rust responsibilities.

## Read These First

| Document | Use it for |
| --- | --- |
| `README.md` | Commands and quick project summary. |
| `docs/calculator-architecture.md` | Full architecture, flows, API contract, current limitations, and next steps. |
| `docs/ui-layout-and-visual-pass.md` | UI layout rules, responsive behavior, screenshot pass, and overlap checks. |
| `docs/runtime-native-and-security.md` | Electron IPC, native adapter loading, Rust scaffold, BLE/security plan. |
| `crates/native/README.md` | Short native crate summary. |

## Main Code Paths

| Area | Entry point |
| --- | --- |
| React app | `src/renderer/main.tsx` |
| UI styling | `src/renderer/styles.css` |
| Browser UI mock | `src/renderer/browser-calculator.ts` |
| Shared API contract | `src/shared/calculator-api.ts` |
| Expression evaluator | `src/shared/expression.ts` |
| Electron main process | `src/electron/main.ts` |
| Preload bridge | `src/electron/preload.ts` |
| Native adapter and mock | `src/electron/native-calculator.ts` |
| Rust native module | `crates/native/src/lib.rs` |

## Run Locally

Install dependencies:

```sh
npm install
```

Run the desktop app:

```sh
npm run dev
```

Run the browser-only renderer for visual review:

```sh
vite --host 127.0.0.1
```

Then open:

```text
http://127.0.0.1:5173
```

## Validate Changes

Use these checks for normal TypeScript/UI work:

```sh
npm run typecheck
npm run lint
npm run test
npm run build
```

For native work:

```sh
npm run build:native
```

For UI layout work, also run:

```sh
node scripts/browser-visual-ui-pass.mjs
```

The visual pass requires Vite to already be running at `http://127.0.0.1:5173`.

## Current Feature Set

- Calculator expression entry.
- Keypad for numbers, decimal, operators, clear, delete, and equals.
- Live local result preview.
- Submit/sync action that writes calculation history through the API.
- Host room creation mock.
- Guest advertising mock.
- Host scanning mock.
- Peer connect mock.
- Guest accept-host mock.
- Collapsible Host Bench drawer.
- Collapsible Peers/History drawer.
- Scrollable peer and history lists.
- Bottom status rail for BLE role, connection count, and trust state.
- Browser screenshot pass for desktop, compact, and short-height layouts.

## Important Boundaries

Keep these boundaries intact:

- Renderer shows UI only.
- Renderer calls high-level commands only.
- Renderer does not store private keys.
- Renderer does not receive raw secrets.
- Renderer does not verify final trust alone.
- Preload exposes only `window.calculator`.
- Electron main owns IPC routing and native adapter selection.
- Rust should own BLE, keychain, SQLite, cryptography, trust, and final validation.

## Common Development Tasks

To add a new command:

1. Add request/response types in `src/shared/calculator-api.ts`.
2. Add the method to `NativeCalculatorApi`.
3. Add an IPC channel name.
4. Expose it in `src/electron/preload.ts`.
5. Register it in `src/electron/main.ts`.
6. Implement it in `src/electron/native-calculator.ts` mock.
7. Implement it in `crates/native/src/lib.rs`.
8. Update docs and tests.

To change calculator math:

1. Update `src/shared/expression.ts`.
2. Update `src/shared/expression.test.ts`.
3. Mirror the behavior in `crates/native/src/lib.rs`.
4. Add or update Rust tests.
5. Confirm preview and submitted history still match.

To change layout:

1. Update `src/renderer/main.tsx` or `src/renderer/styles.css`.
2. Preserve the fixed calculator size unless intentionally redesigning the visual pass.
3. Make long text truncate, wrap, or scroll inside its component.
4. Run the browser visual pass.
5. Inspect screenshots in `artifacts/browser-visual-ui-pass/`.

## Prototype Limitations

- No real BLE yet.
- No OS keychain yet.
- No SQLite yet.
- No real cross-device calculator sync yet.
- No JWE/JWS/JWT/SD-JWT verification yet.
- No issuer trust or holder key binding yet.
- Native module is still scaffold behavior.
- Browser mock is only for UI review.
