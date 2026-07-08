import { spawnSync } from "node:child_process";
import path from "node:path";

const env = { ...process.env };
const cargo = spawnSync("rustup", ["which", "cargo"], {
  encoding: "utf8",
  shell: process.platform === "win32"
});

if (cargo.status === 0) {
  const cargoPath = cargo.stdout.trim();
  if (cargoPath) {
    env.PATH = `${path.dirname(cargoPath)}${path.delimiter}${env.PATH ?? ""}`;
  }
}

const result = spawnSync(
  "npx",
  ["napi", "build", "--cargo-cwd", "crates/native", "--platform", "--release"],
  {
    stdio: "inherit",
    shell: process.platform === "win32",
    env
  }
);

if (result.error) {
  console.warn(`Native build skipped: ${result.error.message}`);
  process.exit(0);
}

if (result.status !== 0) {
  console.warn("Native build skipped or failed. Electron will use the development mock adapter.");
  process.exit(0);
}
