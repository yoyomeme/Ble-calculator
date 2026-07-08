const { app, BrowserWindow } = require("electron");
const { mkdir, writeFile } = require("node:fs/promises");
const path = require("node:path");

const scenarios = JSON.parse(process.env.VISUAL_PASS_SCENARIOS ?? "[]");
const outputDir = process.env.VISUAL_PASS_OUTPUT_DIR;
const serverUrl = process.env.VITE_DEV_SERVER_URL;

if (!outputDir || !serverUrl) {
  throw new Error("VISUAL_PASS_OUTPUT_DIR and VITE_DEV_SERVER_URL are required");
}

void main();

async function main() {
  await mkdir(outputDir, { recursive: true });
  await app.whenReady();

  const reports = [];

  for (const scenario of scenarios) {
    console.log(`capturing ${scenario.name}`);
    const window = new BrowserWindow({
      width: scenario.width,
      height: scenario.height,
      x: 40,
      y: 40,
      show: true,
      backgroundColor: "#1a1614",
      webPreferences: {
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true
      }
    });

    await withTimeout(window.loadURL(serverUrl), `${scenario.name} loadURL`);
    await withTimeout(
      window.webContents.executeJavaScript("document.fonts.ready.then(() => true)"),
      `${scenario.name} fonts`
    );
    await withTimeout(
      window.webContents.executeJavaScript(applyScenarioScript(scenario.action)),
      `${scenario.name} scenario`
    );
    await new Promise((resolve) => setTimeout(resolve, 250));

    const image = await withTimeout(window.webContents.capturePage(), `${scenario.name} capture`);
    const imagePath = path.join(outputDir, `${scenario.name}.png`);
    await writeFile(imagePath, image.toPNG());

    const metrics = await window.webContents.executeJavaScript(metricsScript());
    const report = {
      ...scenario,
      imagePath,
      metrics,
      overlaps: getOverlaps(metrics.rects)
    };
    reports.push(report);
    window.destroy();
  }

  await writeFile(path.join(outputDir, "report.json"), JSON.stringify(reports, null, 2));

  for (const report of reports) {
    console.log(
      `${report.name}: screenshot=${path.relative(process.cwd(), report.imagePath)} overlaps=${report.overlaps.length}`
    );
    for (const overlap of report.overlaps) {
      console.log(`  overlap ${overlap.a}/${overlap.b}: ${overlap.width}x${overlap.height}`);
    }
  }

  app.quit();
}

function withTimeout(promise, label) {
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      setTimeout(() => reject(new Error(`Timed out during ${label}`)), 10_000);
    })
  ]);
}

function applyScenarioScript(action) {
  return `
    (() => {
      const left = document.querySelector('.drawer-toggle--left');
      const right = document.querySelector('.drawer-toggle--right');
      if (${JSON.stringify(action)} === 'left') left?.click();
      if (${JSON.stringify(action)} === 'right') right?.click();
      window.scrollTo(0, 0);
      return true;
    })();
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
        peerList: '.peer-list'
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
    })();
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
