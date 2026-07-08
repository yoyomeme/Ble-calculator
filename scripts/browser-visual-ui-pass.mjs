import { spawn } from "node:child_process";
import { Buffer } from "node:buffer";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(__dirname, "..");
const outputDir = path.join(rootDir, "artifacts", "browser-visual-ui-pass");
const serverUrl = "http://127.0.0.1:5173/";
const chromePath = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const remoteDebuggingPort = 9333;

const scenarios = [
  { name: "desktop-closed", width: 1180, height: 760, action: "closed" },
  { name: "desktop-left-open", width: 1180, height: 760, action: "left" },
  { name: "desktop-right-open", width: 1180, height: 760, action: "right" },
  { name: "desktop-both-open", width: 1180, height: 760, action: "both" },
  { name: "compact-closed", width: 520, height: 760, action: "closed" },
  { name: "compact-left-open", width: 520, height: 760, action: "left" },
  { name: "compact-right-open", width: 520, height: 760, action: "right" },
  { name: "compact-both-attempt", width: 520, height: 760, action: "both" },
  { name: "short-height-right-open", width: 850, height: 560, action: "right" }
];

async function main() {
  await mkdir(outputDir, { recursive: true });
  await waitForHttp(serverUrl, 30_000);

  const userDataDir = await mkdtemp(path.join(os.tmpdir(), "evolve-calc-chrome-"));
  const chrome = spawn(
    chromePath,
    [
      "--headless=new",
      "--hide-scrollbars=false",
      "--disable-gpu",
      "--disable-background-networking",
      "--disable-default-apps",
      "--disable-extensions",
      "--disable-sync",
      "--no-first-run",
      "--no-default-browser-check",
      `--remote-debugging-port=${remoteDebuggingPort}`,
      `--user-data-dir=${userDataDir}`,
      "about:blank"
    ],
    { stdio: ["ignore", "pipe", "pipe"] }
  );

  let chromeStdout = "";
  let chromeStderr = "";
  chrome.stdout.on("data", (chunk) => {
    chromeStdout += chunk.toString();
  });
  chrome.stderr.on("data", (chunk) => {
    chromeStderr += chunk.toString();
  });

  try {
    await waitForHttp(`http://127.0.0.1:${remoteDebuggingPort}/json/version`, 15_000);
    const reports = [];

    for (const scenario of scenarios) {
      process.stdout.write(`capturing ${scenario.name}\n`);
      const target = await createTarget();
      const client = await CdpClient.connect(target.webSocketDebuggerUrl);

      await client.send("Page.enable");
      await client.send("Runtime.enable");
      await client.send("Emulation.setDeviceMetricsOverride", {
        width: scenario.width,
        height: scenario.height,
        deviceScaleFactor: 1,
        mobile: false
      });
      const loaded = waitForLoad(client);
      await client.send("Page.navigate", { url: serverUrl });
      await loaded;
      await client.send("Runtime.evaluate", {
        expression: "document.fonts.ready.then(() => true)",
        awaitPromise: true
      });
      await client.send("Runtime.evaluate", {
        expression: scenarioScript(scenario.action),
        awaitPromise: true
      });
      await delay(250);

      const metricsResult = await client.send("Runtime.evaluate", {
        expression: metricsScript(),
        returnByValue: true
      });
      const metrics = metricsResult.result.value;
      const screenshot = await client.send("Page.captureScreenshot", {
        format: "png",
        fromSurface: true,
        captureBeyondViewport: false
      });
      const imagePath = path.join(outputDir, `${scenario.name}.png`);
      await writeFile(imagePath, Buffer.from(screenshot.data, "base64"));

      const report = {
        ...scenario,
        imagePath,
        metrics,
        overlaps: getOverlaps(metrics.rects)
      };
      reports.push(report);

      client.close();
      await closeTarget(target.id);
    }

    await writeFile(path.join(outputDir, "report.json"), JSON.stringify(reports, null, 2));
    await writeFile(path.join(outputDir, "chrome-stdout.log"), chromeStdout);
    await writeFile(path.join(outputDir, "chrome-stderr.log"), chromeStderr);

    for (const report of reports) {
      process.stdout.write(
        `${report.name}: screenshot=${path.relative(rootDir, report.imagePath)} overlaps=${report.overlaps.length} scroll=${report.metrics.document.scrollWidth}x${report.metrics.document.scrollHeight}\n`
      );
      for (const overlap of report.overlaps) {
        process.stdout.write(
          `  overlap ${overlap.a}/${overlap.b}: ${overlap.width}x${overlap.height}\n`
        );
      }
    }
  } finally {
    chrome.kill("SIGTERM");
    await rm(userDataDir, { recursive: true, force: true });
  }
}

async function createTarget() {
  const response = await fetchJson(
    `http://127.0.0.1:${remoteDebuggingPort}/json/new?${encodeURIComponent("about:blank")}`,
    { method: "PUT" }
  );

  return response;
}

async function closeTarget(id) {
  await fetchJson(`http://127.0.0.1:${remoteDebuggingPort}/json/close/${id}`).catch(() => null);
}

function scenarioScript(action) {
  return `
    (async () => {
      const left = document.querySelector('.drawer-toggle--left');
      const right = document.querySelector('.drawer-toggle--right');
      const wait = () => new Promise((resolve) => setTimeout(resolve, 280));
      const isOpen = (button) => button?.getAttribute('aria-expanded') === 'true';
      const setOpen = async (button, open) => {
        if (button && isOpen(button) !== open) {
          button.click();
          await wait();
        }
      };
      const action = ${JSON.stringify(action)};
      if (action === 'closed') {
        await setOpen(left, false);
        await setOpen(right, false);
      }
      if (action === 'left') {
        await setOpen(right, false);
        await setOpen(left, true);
      }
      if (action === 'right') {
        await setOpen(left, false);
        await setOpen(right, true);
      }
      if (action === 'both') {
        await setOpen(left, true);
        await setOpen(right, true);
      }
      await new Promise((resolve) => setTimeout(resolve, 260));
      window.scrollTo(0, 0);
      return true;
    })()
  `;
}

function metricsScript() {
  return `
    (() => {
      const selectors = {
        left: '.left-panel',
        center: '.calculator-panel',
        right: '.right-panel',
        display: '.display',
        keypad: '.keypad',
        historyList: '.history-list',
        peerList: '.peer-list',
        statusRail: '.status-rail'
      };
      const rects = {};
      for (const [name, selector] of Object.entries(selectors)) {
        const element = document.querySelector(selector);
        const rect = element?.getBoundingClientRect();
        rects[name] = rect
          ? {
              x: Math.round(rect.x),
              y: Math.round(rect.y),
              width: Math.round(rect.width),
              height: Math.round(rect.height),
              right: Math.round(rect.right),
              bottom: Math.round(rect.bottom),
              scrollHeight: element.scrollHeight,
              clientHeight: element.clientHeight,
              scrollWidth: element.scrollWidth,
              clientWidth: element.clientWidth
            }
          : null;
      }
      return {
        viewport: { width: window.innerWidth, height: window.innerHeight },
        document: {
          scrollWidth: document.documentElement.scrollWidth,
          scrollHeight: document.documentElement.scrollHeight
        },
        rects
      };
    })()
  `;
}

function getOverlaps(rects) {
  const pairs = [
    ["left", "center"],
    ["center", "right"],
    ["left", "right"]
  ];

  return pairs.flatMap(([a, b]) => {
    const first = rects[a];
    const second = rects[b];
    if (!first || !second) {
      return [];
    }

    const width = Math.min(first.right, second.right) - Math.max(first.x, second.x);
    const height = Math.min(first.bottom, second.bottom) - Math.max(first.y, second.y);
    return width > 0 && height > 0 ? [{ a, b, width, height }] : [];
  });
}

async function waitForLoad(client) {
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      cleanup();
      reject(new Error("Timed out waiting for Page.loadEventFired"));
    }, 10_000);

    const cleanup = client.on("Page.loadEventFired", () => {
      clearTimeout(timeout);
      resolve();
    });
  });
}

async function waitForHttp(url, timeoutMs) {
  const timeoutAt = Date.now() + timeoutMs;
  while (Date.now() < timeoutAt) {
    try {
      await fetchText(url);
      return;
    } catch {
      await delay(250);
    }
  }
  throw new Error(`Timed out waiting for ${url}`);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function fetchJson(url, options) {
  return fetchText(url, options).then((text) => JSON.parse(text));
}

function fetchText(url, options = {}) {
  return new Promise((resolve, reject) => {
    const request = http.request(url, options, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        body += chunk;
      });
      response.on("end", () => {
        if (response.statusCode && response.statusCode >= 200 && response.statusCode < 300) {
          resolve(body);
        } else {
          reject(new Error(`HTTP ${response.statusCode}: ${body}`));
        }
      });
    });
    request.on("error", reject);
    request.end();
  });
}

class CdpClient {
  static async connect(url) {
    const ws = new globalThis.WebSocket(url);
    const client = new CdpClient(ws);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", reject, { once: true });
    });
    return client;
  }

  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
    ws.addEventListener("message", (event) => this.handleMessage(event));
  }

  send(method, params = {}) {
    const id = this.nextId++;
    const payload = JSON.stringify({ id, method, params });
    this.ws.send(payload);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
  }

  on(method, callback) {
    const listeners = this.listeners.get(method) ?? new Set();
    listeners.add(callback);
    this.listeners.set(method, listeners);
    return () => {
      listeners.delete(callback);
    };
  }

  close() {
    this.ws.close();
  }

  handleMessage(event) {
    const message = JSON.parse(event.data);
    if (message.id) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(new Error(message.error.message));
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    const listeners = this.listeners.get(message.method);
    if (listeners) {
      for (const listener of listeners) {
        listener(message.params);
      }
    }
  }
}

await main();
