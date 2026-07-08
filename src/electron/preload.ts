import { contextBridge, ipcRenderer } from "electron";
import type {
  ConnectGuestRequest,
  CreateRoomRequest,
  NativeCalculatorApi,
  StartAdvertisingRequest,
  SubmitCalculationRequest
} from "../shared/calculator-api";

const calculatorChannels = {
  getState: "calculator:get-state",
  createRoom: "calculator:create-room",
  startScanning: "calculator:start-scanning",
  connectGuest: "calculator:connect-guest",
  startAdvertising: "calculator:start-advertising",
  acceptHostConnection: "calculator:accept-host-connection",
  submitCalculation: "calculator:submit-calculation"
} as const;

const calculatorApi: NativeCalculatorApi = {
  getState: () => ipcRenderer.invoke(calculatorChannels.getState),
  createRoom: (request: CreateRoomRequest) =>
    ipcRenderer.invoke(calculatorChannels.createRoom, request),
  startScanning: () => ipcRenderer.invoke(calculatorChannels.startScanning),
  connectGuest: (request: ConnectGuestRequest) =>
    ipcRenderer.invoke(calculatorChannels.connectGuest, request),
  startAdvertising: (request: StartAdvertisingRequest) =>
    ipcRenderer.invoke(calculatorChannels.startAdvertising, request),
  acceptHostConnection: () => ipcRenderer.invoke(calculatorChannels.acceptHostConnection),
  submitCalculation: (request: SubmitCalculationRequest) =>
    ipcRenderer.invoke(calculatorChannels.submitCalculation, request)
};

contextBridge.exposeInMainWorld("calculator", calculatorApi);
