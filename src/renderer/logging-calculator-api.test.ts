// Verifies the logging contract of the background rescan methods: unlike
// user-initiated actions they must not log the call itself (a 3s rescan loop
// would flood the Activity Log), but real BLE error transitions and repeated
// failures must still surface — each distinct failure exactly once.

import { beforeEach, describe, expect, it } from "vitest";
import type { NativeCalculatorApi, RoomState } from "../shared/calculator-api";
import { logStore } from "./log-store";
import { createLoggingCalculatorApi } from "./logging-calculator-api";

function makeState(overrides: Partial<RoomState> = {}): RoomState {
  return {
    localDeviceId: "test-device",
    roomId: "room-test",
    roomName: "Test Room",
    sessionRole: "host",
    bleRole: "central",
    scanning: true,
    advertising: false,
    peers: [],
    rooms: [],
    history: [],
    ...overrides
  };
}

function withBleError(error: string | null): RoomState {
  return makeState({
    nativeStatus: {
      sqlitePath: null,
      keychainBacked: false,
      publicKeyFingerprint: "test-fingerprint",
      lastBleError: error,
      lastValidation: null
    }
  });
}

function makeApi(overrides: Partial<NativeCalculatorApi> = {}): NativeCalculatorApi {
  const respond = () => Promise.resolve(makeState());
  return {
    getState: respond,
    createRoom: respond,
    startScanning: respond,
    connectGuest: respond,
    scanRooms: respond,
    joinRoom: respond,
    startAdvertising: respond,
    acceptHostConnection: respond,
    resetBleSession: respond,
    submitCalculation: respond,
    ...overrides
  };
}

describe("background rescan logging contract", () => {
  beforeEach(() => {
    logStore.clear();
  });

  it("rescanGuests and rescanRooms stay quiet on uneventful scans", async () => {
    const wrapper = createLoggingCalculatorApi(makeApi());

    // Prime the diagnostics tracker: the first observed state always logs the
    // initial lastBleError value (by design).
    await wrapper.pollState();
    const baseline = logStore.getSnapshot().length;

    await wrapper.rescanGuests();
    await wrapper.rescanGuests();
    await wrapper.rescanRooms();

    expect(logStore.getSnapshot()).toHaveLength(baseline);
  });

  it("user-initiated startScanning still logs the call itself", async () => {
    const wrapper = createLoggingCalculatorApi(makeApi());
    await wrapper.pollState();
    const baseline = logStore.getSnapshot().length;

    await wrapper.startScanning();

    const added = logStore.getSnapshot().slice(baseline);
    expect(added.some((entry) => entry.message.startsWith("→ startScanning"))).toBe(true);
    expect(added.some((entry) => entry.message.startsWith("✓ startScanning"))).toBe(true);
  });

  it("surfaces a BLE error transition once, then logs its clearing", async () => {
    const responses = [withBleError(null), withBleError("adapter off"), withBleError("adapter off"), withBleError(null)];
    const wrapper = createLoggingCalculatorApi(
      makeApi({ startScanning: () => Promise.resolve(responses.shift()!) })
    );

    await wrapper.rescanGuests(); // primes lastBleError = null
    const baseline = logStore.getSnapshot().length;

    await wrapper.rescanGuests(); // error appears
    await wrapper.rescanGuests(); // same error again — no new entry
    const afterError = logStore.getSnapshot().slice(baseline);
    expect(afterError).toHaveLength(1);
    expect(afterError[0]?.level).toBe("error");
    expect(afterError[0]?.scope).toBe("ble");
    expect(afterError[0]?.message).toBe("adapter off");

    await wrapper.rescanGuests(); // error clears
    const afterClear = logStore.getSnapshot().slice(baseline);
    expect(afterClear).toHaveLength(2);
    expect(afterClear[1]?.message).toBe("BLE error cleared");
  });

  it("logs a rescan bridge failure once, not on every tick", async () => {
    const wrapper = createLoggingCalculatorApi(
      makeApi({ startScanning: () => Promise.reject(new Error("bridge gone")) })
    );
    const baseline = logStore.getSnapshot().length;

    await expect(wrapper.rescanGuests()).rejects.toThrow("bridge gone");
    await expect(wrapper.rescanGuests()).rejects.toThrow("bridge gone");

    const added = logStore.getSnapshot().slice(baseline);
    expect(added).toHaveLength(1);
    expect(added[0]?.message).toContain("background rescan failed: bridge gone");
  });
});
