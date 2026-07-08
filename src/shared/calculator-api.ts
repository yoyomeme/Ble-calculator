export type SessionRole = "host" | "guest";

export type BleRole = "central" | "peripheral" | "dual";

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

export interface RoomSummary {
  id: string;
  name: string;
  hostDeviceId: string;
  trustStatus: TrustStatus;
  joinable: boolean;
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

export interface NativeCapabilities {
  bleCentralScanning: boolean;
  blePeripheralAdvertising: boolean;
  sqlitePersistence: boolean;
  keychainStorage: boolean;
  localJwsSigning: boolean;
  jweDecryption: boolean;
  jwtSdJwtVerification: boolean;
  issuerTrustValidation: boolean;
  holderKeyBinding: boolean;
  crossDeviceSync: boolean;
}

export interface ValidationSummary {
  valid: boolean;
  kind: string;
  issuerTrusted: boolean;
  holderBound: boolean;
  reason: string;
}

export interface NativeRuntimeStatus {
  sqlitePath: string | null;
  keychainBacked: boolean;
  publicKeyFingerprint: string;
  lastBleError: string | null;
  lastValidation: ValidationSummary | null;
  pendingOutboxEvents?: number;
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
  rooms?: RoomSummary[];
  history: CalculationEntry[];
  nativeCapabilities?: NativeCapabilities;
  nativeStatus?: NativeRuntimeStatus;
  nativeWarnings?: string[];
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

export interface JoinRoomRequest {
  roomId: string;
}

export interface SubmitCalculationRequest {
  expression: string;
}

export interface NativeCalculatorApi {
  getState(): Promise<RoomState>;
  createRoom(request: CreateRoomRequest): Promise<RoomState>;
  startScanning(): Promise<RoomState>;
  connectGuest(request: ConnectGuestRequest): Promise<RoomState>;
  scanRooms(): Promise<RoomState>;
  joinRoom(request: JoinRoomRequest): Promise<RoomState>;
  startAdvertising(request: StartAdvertisingRequest): Promise<RoomState>;
  acceptHostConnection(): Promise<RoomState>;
  resetBleSession(): Promise<RoomState>;
  submitCalculation(request: SubmitCalculationRequest): Promise<RoomState>;
}

export const calculatorChannels = {
  getState: "calculator:get-state",
  createRoom: "calculator:create-room",
  startScanning: "calculator:start-scanning",
  connectGuest: "calculator:connect-guest",
  scanRooms: "calculator:scan-rooms",
  joinRoom: "calculator:join-room",
  startAdvertising: "calculator:start-advertising",
  acceptHostConnection: "calculator:accept-host-connection",
  resetBleSession: "calculator:reset-ble-session",
  submitCalculation: "calculator:submit-calculation"
} as const;
