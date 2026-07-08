import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import electronPath from "electron";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(__dirname, "..");
const outputDir = path.join(rootDir, "artifacts", "visual-ui-pass");
const serverUrl = "http://127.0.0.1:5173/";

const scenarios = [
  { name: "desktop-closed", width: 1180, height: 760, action: "closed" },
  { name: "desktop-left-open", width: 1180, height: 760, action: "left" },
  { name: "desktop-right-open", width: 1180, height: 760, action: "right" },
  { name: "compact-closed", width: 520, height: 760, action: "closed" },
  { name: "compact-left-open", width: 820, height: 760, action: "left" },
  { name: "compact-right-open", width: 850, height: 760, action: "right" },
  { name: "short-height-right-open", width: 850, height: 560, action: "right" }
];

await mkdir(outputDir, { recursive: true });
await waitForServer(serverUrl);

const app = spawn(
  electronPath,
  [
    "--disable-gpu",
    "--no-sandbox",
    "--js-flags=--expose-gc",
    path.join(rootDir, "scripts", "visual-ui-screenshot-app.cjs")
  ],
  {
    cwd: rootDir,
    env: {
      ...process.env,
      VITE_DEV_SERVER_URL: serverUrl,
      VISUAL_PASS_SCENARIOS: JSON.stringify(scenarios),
      VISUAL_PASS_OUTPUT_DIR: outputDir
    },
    stdio: ["ignore", "pipe", "pipe"]
  }
);

let stdout = "";
let stderr = "";

app.stdout.on("data", (chunk) => {
  stdout += chunk.toString();
});

app.stderr.on("data", (chunk) => {
  stderr += chunk.toString();
});

const code = await new Promise((resolve) => {
  app.on("close", resolve);
});

await writeFile(path.join(outputDir, "electron-stdout.log"), stdout);
await writeFile(path.join(outputDir, "electron-stderr.log"), stderr);

if (stdout.trim()) {
  process.stdout.write(stdout);
}

if (stderr.trim()) {
  process.stderr.write(stderr);
}

if (code !== 0) {
  throw new Error(`Visual UI pass failed with exit code ${code}`);
}

async function waitForServer(url) {
  const timeoutAt = Date.now() + 30_000;

  while (Date.now() < timeoutAt) {
    if (await canReach(url)) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 350));
  }

  throw new Error(`Timed out waiting for ${url}`);
}

function canReach(url) {
  return new Promise((resolve) => {
    const request = http.get(url, (response) => {
      response.resume();
      resolve(response.statusCode && response.statusCode < 500);
    });

    request.on("error", () => resolve(false));
    request.setTimeout(1500, () => {
      request.destroy();
      resolve(false);
    });
  });
}
