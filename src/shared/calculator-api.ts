export type SessionRole = "host" | "guest";

export type BleRole = "central" | "peripheral";

export type TrustStatus = "trusted" | "untrusted" | "pending";

export interface PeerSummary {
  id: string;
  label: string;
  sessionRole: SessionRole;
  bleRole: BleRole;
  trustStatus: TrustStatus;
  connected: boolean;
  lastSeenIso: string;
}

export interface CalculationEntry {
  id: string;
  originDeviceId: string;
  expression: string;
  result: string;
  trusted: boolean;
  createdAtIso: string;
}

export interface RoomState {
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

export interface CreateRoomRequest {
  roomName: string;
}

export interface StartAdvertisingRequest {
  roomCode: string;
}

export interface ConnectGuestRequest {
  peerId: string;
}

export interface SubmitCalculationRequest {
  expression: string;
}

export interface NativeCalculatorApi {
  getState(): Promise<RoomState>;
  createRoom(request: CreateRoomRequest): Promise<RoomState>;
  startScanning(): Promise<RoomState>;
  connectGuest(request: ConnectGuestRequest): Promise<RoomState>;
  startAdvertising(request: StartAdvertisingRequest): Promise<RoomState>;
  acceptHostConnection(): Promise<RoomState>;
  submitCalculation(request: SubmitCalculationRequest): Promise<RoomState>;
}

export const calculatorChannels = {
  getState: "calculator:get-state",
  createRoom: "calculator:create-room",
  startScanning: "calculator:start-scanning",
  connectGuest: "calculator:connect-guest",
  startAdvertising: "calculator:start-advertising",
  acceptHostConnection: "calculator:accept-host-connection",
  submitCalculation: "calculator:submit-calculation"
} as const;
