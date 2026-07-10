# Manual two-Mac BLE E2E verification

End-to-end check of native BLE discovery, connection, and calculation sync
between two physical Macs. This cannot be automated: macOS only exposes real
`CBCentralManager` / `CBPeripheralManager` behavior on hardware, and the two
roles must run on different machines.

## Preconditions (both Macs)

1. **Bluetooth is on** — System Settings > Bluetooth.
2. **The app has Bluetooth permission** — System Settings > Privacy & Security
   > Bluetooth must list the app (or, for `npm run dev`, the app that launched
   it: the terminal/IDE — macOS attributes the permission to the responsible
   launching process). Expect a permission prompt on the first BLE action;
   grant it. If the prompt was ever declined, re-enable the toggle manually.
3. **Both app windows stay in the foreground** — CoreBluetooth throttles
   advertising and scan responses for backgrounded apps.
4. The two Macs are within a few meters of each other.
5. Both run the same build (or any pair of builds ≥ the `EVC:` short
   advertisement format; older `EvolveCalc:` builds are still parsed, but their
   long names can be truncated on air — prefer current builds on both ends).

## Procedure

Call one Mac **Host** and the other **Guest**.

| # | Where | Action | Expected result |
|---|-------|--------|-----------------|
| 1 | Host | Select the **Host** role, enter a room name, click **Create Room** | "Room Created"; a **Room code** (`r-xxxxxx`) appears under the button. The log shows no `lastBleError`. |
| 2 | Guest | Select the **Guest** role, click **Find Hosts** | Within ~3–6 s (the renderer rescans every 3 s) the host's room appears in the Discovery list. |
| 3 | Guest | Click the discovered room (or type the host's room code into **Or join by code** and click **Join**) | Guest switches to advertising ("Signal" status). No `lastBleError`. |
| 4 | Host | Click **Find Guests** | Within ~3–6 s the guest appears in the Discovery list. |
| 5 | Host | Click the guest to **Connect** | Both sides show Connected; discovery stops. |
| 6 | Host | Submit a calculation (e.g. `7 + 5 * 2`) | The result appears in the guest's history, marked trusted. |
| 7 | Guest | Submit a calculation | The result appears in the host's history, marked trusted. |
| 8 | Either | Quit one app | The other side reports the disconnect. |

## Troubleshooting

Errors now surface in the UI (`lastBleError` in the session facts / log pane)
instead of failing silently. What each one means:

| Symptom / message | Cause | Fix |
|---|---|---|
| `Bluetooth is unavailable — the app may have been denied Bluetooth permission…` | macOS TCC denied Bluetooth for the app (scan side; denied permission reports as an "unknown" adapter state) | Enable the app in System Settings > Privacy & Security > Bluetooth, then retry |
| `Bluetooth permission denied. Allow Bluetooth for Evolve Calc…` | Same, on the advertising side (`CBPeripheralManager` unauthorized) | Same as above |
| `Bluetooth is turned off…` | Radio off | Turn Bluetooth on |
| `Room discovery beacon could not start / beacon error: …` (host) | The host's ROOM advertisement failed, so guests cannot discover the room by scanning | Fix the reported cause; guests can still join by typing the room code |
| `Guest BLE advertising could not start: …` (guest) | The guest's JOIN advertisement failed, so the host cannot discover the guest | Fix the reported cause and re-join |
| `BLE scan saw N device(s) advertising the calculator service, but none …` | Something nearby advertises the calculator service UUID but its name did not parse as a room/guest for this session — usually a stale/old-format build or a room-code mismatch | Use current builds on both Macs; verify the guest typed the exact room code shown on the host |
| Empty list, no warnings at all | Nothing matching is on air: the other side isn't advertising yet, is backgrounded, or is out of range | Check the other Mac's step completed without errors and its window is foreground |

## Why two Macs

Documented in `docs/runtime-native-and-security.md`: guest-to-host discovery
needs one machine advertising via `CBPeripheralManager` while the other scans
via `CBCentralManager`; a single machine cannot do both against itself.
