import { app, BrowserWindow, ipcMain } from "electron";
import path from "node:path";
import { calculatorChannels } from "../shared/calculator-api";
import type {
  ConnectGuestRequest,
  CreateRoomRequest,
  StartAdvertisingRequest,
  SubmitCalculationRequest
} from "../shared/calculator-api";
import { getCalculatorApi } from "./native-calculator";

let mainWindow: BrowserWindow | null = null;

function registerCalculatorIpc(): void {
  const api = getCalculatorApi();

  ipcMain.handle(calculatorChannels.getState, () => api.getState());
  ipcMain.handle(calculatorChannels.createRoom, (_event, request: CreateRoomRequest) =>
    api.createRoom(request)
  );
  ipcMain.handle(calculatorChannels.startScanning, () => api.startScanning());
  ipcMain.handle(calculatorChannels.connectGuest, (_event, request: ConnectGuestRequest) =>
    api.connectGuest(request)
  );
  ipcMain.handle(calculatorChannels.startAdvertising, (_event, request: StartAdvertisingRequest) =>
    api.startAdvertising(request)
  );
  ipcMain.handle(calculatorChannels.acceptHostConnection, () => api.acceptHostConnection());
  ipcMain.handle(calculatorChannels.submitCalculation, (_event, request: SubmitCalculationRequest) =>
    api.submitCalculation(request)
  );
}

async function createWindow(): Promise<void> {
  mainWindow = new BrowserWindow({
    width: 1180,
    height: 760,
    minWidth: 900,
    minHeight: 620,
    title: "BLE Calculator",
    backgroundColor: "#f6f5f2",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  });

  const devServerUrl = process.env.VITE_DEV_SERVER_URL;
  if (devServerUrl) {
    await mainWindow.loadURL(devServerUrl);
    mainWindow.webContents.openDevTools({ mode: "detach" });
  } else {
    await mainWindow.loadFile(path.join(__dirname, "../../renderer/index.html"));
  }
}

void app.whenReady().then(async () => {
  registerCalculatorIpc();
  await createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      void createWindow();
    }
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});
