// Verifies the browser mock's delayed-discovery simulation: devices only
// appear after repeated scan passes, mirroring a real peer that starts
// advertising after discovery began. This is what the background rescan pump
// in main.tsx relies on — a single one-shot scan must NOT be enough.

import { describe, expect, it } from "vitest";
import { createBrowserCalculatorApi } from "./browser-calculator";

describe("browser mock discovery simulation", () => {
  it("host scan discovers a late-arriving guest only after repeated passes", async () => {
    const api = createBrowserCalculatorApi();
    await api.createRoom({ roomName: "Test Room" });

    const first = await api.startScanning();
    expect(first.scanning).toBe(true);
    expect(first.peers).toHaveLength(0);

    const second = await api.startScanning();
    expect(second.peers).toHaveLength(0);

    const third = await api.startScanning();
    expect(third.peers).toHaveLength(1);
    expect(third.peers[0]?.sessionRole).toBe("guest");
    expect(third.peers[0]?.connected).toBe(false);
  });

  it("guest scan discovers a late-arriving host room only after repeated passes", async () => {
    const api = createBrowserCalculatorApi();

    const first = await api.scanRooms();
    expect(first.scanning).toBe(true);
    expect(first.rooms ?? []).toHaveLength(0);

    const second = await api.scanRooms();
    expect(second.rooms ?? []).toHaveLength(0);

    const third = await api.scanRooms();
    expect(third.rooms).toHaveLength(1);
    expect(third.rooms?.[0]?.joinable).toBe(true);
  });

  it("connecting a discovered guest stops the scan loop", async () => {
    const api = createBrowserCalculatorApi();
    await api.createRoom({ roomName: "Test Room" });

    let state = await api.startScanning();
    while (state.peers.length === 0) {
      state = await api.startScanning();
    }

    const connected = await api.connectGuest({ peerId: state.peers[0]!.id });
    expect(connected.scanning).toBe(false);
    expect(connected.peers[0]?.connected).toBe(true);
  });

  it("joining a discovered room stops scanning and starts advertising", async () => {
    const api = createBrowserCalculatorApi();

    let state = await api.scanRooms();
    while ((state.rooms ?? []).length === 0) {
      state = await api.scanRooms();
    }

    const joined = await api.joinRoom({ roomId: state.rooms![0]!.id });
    expect(joined.scanning).toBe(false);
    expect(joined.advertising).toBe(true);
  });

  it("resetBleSession restarts the delayed-discovery simulation", async () => {
    const api = createBrowserCalculatorApi();
    await api.createRoom({ roomName: "Test Room" });

    let state = await api.startScanning();
    while (state.peers.length === 0) {
      state = await api.startScanning();
    }

    await api.resetBleSession();
    const afterReset = await api.startScanning();
    expect(afterReset.peers).toHaveLength(0);
  });
});
