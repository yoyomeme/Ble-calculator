import { contextBridge, ipcRenderer } from "electron";
import type {
  ConnectGuestRequest,
  CreateRoomRequest,
  JoinRoomRequest,
  NativeCalculatorApi,
  StartAdvertisingRequest,
  SubmitCalculationRequest
} from "../shared/calculator-api";

const calculatorChannels = {
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

const calculatorApi: NativeCalculatorApi = {
  getState: () => ipcRenderer.invoke(calculatorChannels.getState),
  createRoom: (request: CreateRoomRequest) =>
    ipcRenderer.invoke(calculatorChannels.createRoom, request),
  startScanning: () => ipcRenderer.invoke(calculatorChannels.startScanning),
  connectGuest: (request: ConnectGuestRequest) =>
    ipcRenderer.invoke(calculatorChannels.connectGuest, request),
  scanRooms: () => ipcRenderer.invoke(calculatorChannels.scanRooms),
  joinRoom: (request: JoinRoomRequest) => ipcRenderer.invoke(calculatorChannels.joinRoom, request),
  startAdvertising: (request: StartAdvertisingRequest) =>
    ipcRenderer.invoke(calculatorChannels.startAdvertising, request),
  acceptHostConnection: () => ipcRenderer.invoke(calculatorChannels.acceptHostConnection),
  resetBleSession: () => ipcRenderer.invoke(calculatorChannels.resetBleSession),
  submitCalculation: (request: SubmitCalculationRequest) =>
    ipcRenderer.invoke(calculatorChannels.submitCalculation, request)
};

contextBridge.exposeInMainWorld("calculator", calculatorApi);
