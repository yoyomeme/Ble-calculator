import type {
  ConnectGuestRequest,
  CreateRoomRequest,
  JoinRoomRequest,
  NativeCalculatorApi,
  RoomState,
  StartAdvertisingRequest,
  SubmitCalculationRequest
} from "../shared/calculator-api";
import { calculateExpression } from "../shared/expression";

export function createBrowserCalculatorApi(): NativeCalculatorApi {
  const localDeviceId = `browser-${randomId()}`;
  const state: RoomState = {
    localDeviceId,
    roomId: null,
    roomName: null,
    sessionRole: null,
    bleRole: null,
    scanning: false,
    advertising: false,
    peers: [],
    rooms: [],
    history: []
  };

  return {
    async getState() {
      return cloneState(state);
    },
    async createRoom(request: CreateRoomRequest) {
      state.roomId = `room-${randomId().slice(0, 8)}`;
      state.roomName = request.roomName.trim() || "Calculator Room";
      state.sessionRole = "host";
      state.bleRole = "central";
      state.advertising = false;
      state.scanning = false;
      state.rooms = [];
      return cloneState(state);
    },
    async startScanning() {
      state.sessionRole = "host";
      state.bleRole = "central";
      state.scanning = true;
      if (state.peers.length === 0) {
        state.peers.push({
          id: "guest-browser-linux",
          label: "Linux Calculator",
          sessionRole: "guest",
          bleRole: "peripheral",
          trustStatus: "pending",
          connected: false,
          lastSeenIso: new Date().toISOString(),
          rssi: null
        });
      }
      return cloneState(state);
    },
    async connectGuest(request: ConnectGuestRequest) {
      state.peers = state.peers.map((peer) =>
        peer.id === request.peerId ? { ...peer, connected: true, trustStatus: "trusted" } : peer
      );
      // A successful connection stops the scan — the Discovery tab switches to
      // the Connection Card, so scanning must not linger.
      state.scanning = false;
      return cloneState(state);
    },
    async scanRooms() {
      state.sessionRole = "guest";
      state.bleRole = "central";
      state.scanning = true;
      state.advertising = false;
      if (!state.rooms || state.rooms.length === 0) {
        state.rooms = [
          {
            id: "room-browser-desk",
            name: "Desk Calculator",
            hostDeviceId: "browser-host-mac",
            trustStatus: "pending",
            joinable: true,
            lastSeenIso: new Date().toISOString(),
            rssi: null
          }
        ];
      }
      return cloneState(state);
    },
    async joinRoom(request: JoinRoomRequest) {
      const room = state.rooms?.find((candidate) => candidate.id === request.roomId);
      state.roomId = request.roomId;
      state.roomName = room?.name ?? `Join ${request.roomId}`;
      state.sessionRole = "guest";
      state.bleRole = "peripheral";
      state.advertising = true;
      state.scanning = false;
      state.peers = [
        {
          id: room?.hostDeviceId ?? "host-browser-pending",
          label: room?.name ?? "Host pending",
          sessionRole: "host",
          bleRole: "central",
          trustStatus: "pending",
          connected: false,
          lastSeenIso: new Date().toISOString(),
          rssi: null
        }
      ];
      return cloneState(state);
    },
    async startAdvertising(request: StartAdvertisingRequest) {
      state.roomId = request.roomCode.trim() || null;
      state.roomName = state.roomId ? `Join ${state.roomId}` : null;
      state.sessionRole = "guest";
      state.bleRole = "peripheral";
      state.advertising = true;
      state.scanning = false;
      return cloneState(state);
    },
    async acceptHostConnection() {
      state.advertising = false;
      state.peers = [
        {
          id: "host-browser-mac",
          label: "Mac Host",
          sessionRole: "host",
          bleRole: "central",
          trustStatus: "trusted",
          connected: true,
          lastSeenIso: new Date().toISOString()
        }
      ];
      return cloneState(state);
    },
    async resetBleSession() {
      state.roomId = null;
      state.roomName = null;
      state.sessionRole = null;
      state.bleRole = null;
      state.scanning = false;
      state.advertising = false;
      state.peers = [];
      state.rooms = [];
      return cloneState(state);
    },
    async submitCalculation(request: SubmitCalculationRequest) {
      const expression = request.expression.trim();
      if (!expression) {
        return cloneState(state);
      }

      state.history.unshift({
        id: randomId(),
        originDeviceId: localDeviceId,
        expression,
        result: calculateExpression(expression),
        trusted: true,
        createdAtIso: new Date().toISOString()
      });

      return cloneState(state);
    }
  };
}

function cloneState(state: RoomState): RoomState {
  return JSON.parse(JSON.stringify(state)) as RoomState;
}

function randomId(): string {
  return globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
}
