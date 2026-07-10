// Wraps a NativeCalculatorApi so every command emits detailed real-time logs:
// the action + arguments, its duration, a compact result summary, and the
// native diagnostics carried in RoomState (capabilities, warnings, BLE errors,
// validation). This is the primary signal for debugging cross-platform BLE
// behaviour between macOS, Linux, and Windows, because each native peripheral
// backend surfaces distinctive warnings / lastBleError strings.

import type {
  ConnectGuestRequest,
  CreateRoomRequest,
  JoinRoomRequest,
  NativeCalculatorApi,
  RoomState,
  StartAdvertisingRequest,
  SubmitCalculationRequest
} from "../shared/calculator-api";
import { logStore } from "./log-store";

function shorten(value: string, keep = 12): string {
  return value.length > keep ? `${value.slice(0, keep)}…` : value;
}

function summarizeState(state: RoomState): string {
  const connected = state.peers.filter((peer) => peer.connected).length;
  const lines = [
    `role=${state.sessionRole ?? "-"}/${state.bleRole ?? "-"} room=${state.roomId ?? "-"}`,
    `scanning=${state.scanning} advertising=${state.advertising}`,
    `peers=${state.peers.length} (${connected} connected) rooms=${state.rooms?.length ?? 0} history=${state.history.length}`
  ];

  if (state.nativeCapabilities) {
    lines.push(
      `blePeripheralAdvertising=${state.nativeCapabilities.blePeripheralAdvertising} bleCentralScanning=${state.nativeCapabilities.bleCentralScanning}`
    );
  }
  if (state.nativeStatus) {
    lines.push(
      `keychainBacked=${state.nativeStatus.keychainBacked} pendingOutbox=${state.nativeStatus.pendingOutboxEvents ?? 0} fingerprint=${shorten(state.nativeStatus.publicKeyFingerprint)}`
    );
  }
  return lines.join("\n");
}

function describeArgs(args: unknown): string {
  if (args === undefined) {
    return "";
  }
  try {
    return ` ${JSON.stringify(args)}`;
  } catch {
    return " [unserializable args]";
  }
}

/**
 * Log a one-time snapshot of the runtime environment. Captures the OS/browser
 * (via the user agent) and whether the real Electron native bridge is present
 * vs. the browser mock — both essential context when comparing logs collected
 * on different machines.
 */
export function logStartupEnvironment(hasNativeBridge: boolean): void {
  const detailLines = [
    `bridge=${hasNativeBridge ? "electron-native" : "browser-mock"}`,
    `userAgent=${navigator.userAgent}`,
    `platform=${navigator.platform}`,
    `language=${navigator.language}`,
    `viewport=${window.innerWidth}x${window.innerHeight}`
  ];
  logStore.push(
    "info",
    "env",
    `Session started (${hasNativeBridge ? "native bridge" : "browser mock"})`,
    detailLines.join("\n")
  );
}

export type LoggingCalculatorApi = NativeCalculatorApi & {
  /**
   * `getState` for the background receive pump. Unlike the user-initiated
   * commands it does not log the call itself — only diagnostics that changed
   * (new warnings, BLE error transitions) — so a ~1s poll stays quiet.
   */
  pollState(): Promise<RoomState>;
  /**
   * `startScanning` for the background discovery rescan pump. Like `pollState`
   * it does not log the call itself — only diagnostics that changed — so the
   * repeated scans stay quiet while waiting for a guest to appear.
   */
  rescanGuests(): Promise<RoomState>;
  /**
   * `scanRooms` for the background discovery rescan pump — the guest-side
   * mirror of `rescanGuests`, equally quiet.
   */
  rescanRooms(): Promise<RoomState>;
};

export function createLoggingCalculatorApi(
  api: NativeCalculatorApi
): LoggingCalculatorApi {
  const loggedWarnings = new Set<string>();
  const loggedPollErrors = new Set<string>();
  let loggedCapabilities = false;
  // `undefined` = not yet observed, so the first observed value is logged too.
  let lastBleError: string | null | undefined = undefined;

  function processResult(action: string, state: RoomState, elapsedMs: number): void {
    logStore.push(
      "success",
      "action",
      `✓ ${action} (${Math.round(elapsedMs)}ms)`,
      summarizeState(state)
    );
    processDiagnostics(state);
  }

  function processDiagnostics(state: RoomState): void {
    if (!loggedCapabilities && state.nativeCapabilities) {
      loggedCapabilities = true;
      logStore.push(
        "info",
        "native",
        "Native capabilities",
        JSON.stringify(state.nativeCapabilities, null, 2)
      );
    }

    for (const warning of state.nativeWarnings ?? []) {
      if (!loggedWarnings.has(warning)) {
        loggedWarnings.add(warning);
        logStore.push("warn", "native", warning);
      }
    }

    const bleError = state.nativeStatus?.lastBleError ?? null;
    if (bleError !== lastBleError) {
      lastBleError = bleError;
      if (bleError) {
        logStore.push("error", "ble", bleError);
      } else {
        logStore.push("info", "ble", "BLE error cleared");
      }
    }
  }

  async function logged(
    action: string,
    args: unknown,
    run: () => Promise<RoomState>
  ): Promise<RoomState> {
    const startedAt = performance.now();
    logStore.push("info", "action", `→ ${action}${describeArgs(args)}`);
    try {
      const state = await run();
      processResult(action, state, performance.now() - startedAt);
      return state;
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : "Unexpected native bridge error";
      logStore.push(
        "error",
        "action",
        `✗ ${action} failed (${Math.round(performance.now() - startedAt)}ms): ${message}`
      );
      throw caught;
    }
  }

  async function pollState(): Promise<RoomState> {
    try {
      const state = await api.getState();
      processDiagnostics(state);
      return state;
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : "Unexpected native bridge error";
      // Log each distinct poll failure once, not on every tick.
      if (!loggedPollErrors.has(message)) {
        loggedPollErrors.add(message);
        logStore.push("error", "action", `✗ background poll failed: ${message}`);
      }
      throw caught;
    }
  }

  async function quietRescan(run: () => Promise<RoomState>): Promise<RoomState> {
    try {
      const state = await run();
      processDiagnostics(state);
      return state;
    } catch (caught) {
      const message =
        caught instanceof Error ? caught.message : "Unexpected native bridge error";
      // Log each distinct rescan failure once, not on every tick.
      if (!loggedPollErrors.has(message)) {
        loggedPollErrors.add(message);
        logStore.push("error", "action", `✗ background rescan failed: ${message}`);
      }
      throw caught;
    }
  }

  return {
    getState: () => logged("getState", undefined, () => api.getState()),
    pollState,
    rescanGuests: () => quietRescan(() => api.startScanning()),
    rescanRooms: () => quietRescan(() => api.scanRooms()),
    createRoom: (request: CreateRoomRequest) =>
      logged("createRoom", request, () => api.createRoom(request)),
    startScanning: () => logged("startScanning", undefined, () => api.startScanning()),
    connectGuest: (request: ConnectGuestRequest) =>
      logged("connectGuest", request, () => api.connectGuest(request)),
    scanRooms: () => logged("scanRooms", undefined, () => api.scanRooms()),
    joinRoom: (request: JoinRoomRequest) =>
      logged("joinRoom", request, () => api.joinRoom(request)),
    startAdvertising: (request: StartAdvertisingRequest) =>
      logged("startAdvertising", request, () => api.startAdvertising(request)),
    acceptHostConnection: () =>
      logged("acceptHostConnection", undefined, () => api.acceptHostConnection()),
    resetBleSession: () =>
      logged("resetBleSession", undefined, () => api.resetBleSession()),
    submitCalculation: (request: SubmitCalculationRequest) =>
      logged("submitCalculation", request, () => api.submitCalculation(request))
  };
}
