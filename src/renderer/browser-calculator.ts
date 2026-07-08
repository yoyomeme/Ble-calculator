import type {
  ConnectGuestRequest,
  CreateRoomRequest,
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
      return cloneState(state);
    },
    async startScanning() {
      state.scanning = true;
      if (state.peers.length === 0) {
        state.peers.push({
          id: "guest-browser-linux",
          label: "Linux Calculator",
          sessionRole: "guest",
          bleRole: "peripheral",
          trustStatus: "pending",
          connected: false,
          lastSeenIso: new Date().toISOString()
        });
      }
      return cloneState(state);
    },
    async connectGuest(request: ConnectGuestRequest) {
      state.peers = state.peers.map((peer) =>
        peer.id === request.peerId ? { ...peer, connected: true, trustStatus: "trusted" } : peer
      );
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
