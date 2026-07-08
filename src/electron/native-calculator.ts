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
import { randomUUID } from "node:crypto";
import { createRequire } from "node:module";

type NativeModule = NativeCalculatorApi;
const requireNative = createRequire(__filename);

const nativeCandidates = [
  "../../../index.js",
  "../../../crates/native/index.js",
  "../../../crates/native/ble_calculator_native.node",
  "../../../crates/native/ble-calculator-native.node"
] as const;

let nativeApi: NativeCalculatorApi | null = null;

export function getCalculatorApi(): NativeCalculatorApi {
  if (nativeApi) {
    return nativeApi;
  }

  nativeApi = loadNativeApi() ?? createMockCalculatorApi();
  return nativeApi;
}

function loadNativeApi(): NativeCalculatorApi | null {
  for (const candidate of nativeCandidates) {
    try {
      const loaded = requireNative(candidate) as Partial<NativeModule>;

      if (isNativeCalculatorApi(loaded)) {
        return loaded;
      }
    } catch (error) {
      if (!isMissingModuleError(error)) {
        console.warn(`Failed to load native calculator module ${candidate}`, error);
      }
    }
  }

  return null;
}

function isNativeCalculatorApi(value: Partial<NativeModule>): value is NativeCalculatorApi {
  return (
    typeof value.getState === "function" &&
    typeof value.createRoom === "function" &&
    typeof value.startScanning === "function" &&
    typeof value.connectGuest === "function" &&
    typeof value.scanRooms === "function" &&
    typeof value.joinRoom === "function" &&
    typeof value.startAdvertising === "function" &&
    typeof value.acceptHostConnection === "function" &&
    typeof value.resetBleSession === "function" &&
    typeof value.submitCalculation === "function"
  );
}

function isMissingModuleError(error: unknown): boolean {
  return (
    error instanceof Error &&
    "code" in error &&
    (error as Error & { code?: string }).code === "MODULE_NOT_FOUND"
  );
}

function createMockCalculatorApi(): NativeCalculatorApi {
  const localDeviceId = `mock-${randomUUID()}`;
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
      state.roomId = `room-${randomUUID().slice(0, 8)}`;
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
          id: "guest-mock-linux",
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
    async scanRooms() {
      state.sessionRole = "guest";
      state.bleRole = "central";
      state.scanning = true;
      state.advertising = false;
      if (!state.rooms || state.rooms.length === 0) {
        state.rooms = [
          {
            id: "room-mock-desk",
            name: "Desk Calculator",
            hostDeviceId: "mock-host-mac",
            trustStatus: "pending",
            joinable: true,
            lastSeenIso: new Date().toISOString()
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
          id: room?.hostDeviceId ?? "host-mock-pending",
          label: room?.name ?? "Host pending",
          sessionRole: "host",
          bleRole: "central",
          trustStatus: "pending",
          connected: false,
          lastSeenIso: new Date().toISOString()
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
          id: "host-mock-mac",
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
      if (expression.length === 0) {
        return cloneState(state);
      }

      state.history.unshift({
        id: randomUUID(),
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
