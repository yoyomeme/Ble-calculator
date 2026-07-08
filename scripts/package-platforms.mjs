import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, renameSync, rmSync } from "node:fs";
import path from "node:path";

const rootDir = path.resolve(import.meta.dirname, "..");

const targetMatrix = {
  "mac-arm64": {
    platform: "mac",
    arch: "arm64",
    rustTarget: "aarch64-apple-darwin",
    nodeArtifact: "index.darwin-arm64.node"
  },
  "mac-x64": {
    platform: "mac",
    arch: "x64",
    rustTarget: "x86_64-apple-darwin",
    nodeArtifact: "index.darwin-x64.node"
  },
  "win-x64": {
    platform: "win",
    arch: "x64",
    rustTarget: "x86_64-pc-windows-msvc",
    nodeArtifact: "index.win32-x64-msvc.node"
  },
  "win-arm64": {
    platform: "win",
    arch: "arm64",
    rustTarget: "aarch64-pc-windows-msvc",
    nodeArtifact: "index.win32-arm64-msvc.node"
  },
  "linux-x64": {
    platform: "linux",
    arch: "x64",
    rustTarget: "x86_64-unknown-linux-gnu",
    nodeArtifact: "index.linux-x64-gnu.node"
  },
  "linux-arm64": {
    platform: "linux",
    arch: "arm64",
    rustTarget: "aarch64-unknown-linux-gnu",
    nodeArtifact: "index.linux-arm64-gnu.node"
  }
};

const aliases = {
  current: currentHostTargets(),
  mac: ["mac-arm64", "mac-x64"],
  darwin: ["mac-arm64", "mac-x64"],
  win: ["win-x64", "win-arm64"],
  windows: ["win-x64", "win-arm64"],
  linux: ["linux-x64", "linux-arm64"],
  all: Object.keys(targetMatrix)
};

const args = process.argv.slice(2);
const skipNative = takeFlag(args, "--skip-native");
const dryRun = takeFlag(args, "--dry-run");
const requestedTargets = expandTargets(args.length > 0 ? args : ["current"]);

if (requestedTargets.length === 0) {
  printUsageAndExit(1);
}

const env = buildEnvWithRustupCargo();
const electronBuilderTargets = groupElectronTargets(requestedTargets);

if (dryRun) {
  printPlan(requestedTargets, electronBuilderTargets, skipNative);
  process.exit(0);
}

const stashedNativeArtifacts = skipNative ? stashNativeArtifacts() : [];

try {
  run("npm", ["run", "typecheck"], { env });
  run("npm", ["run", "lint"], { env });
  run("npm", ["run", "test"], { env });

  if (!skipNative) {
    for (const key of requestedTargets) {
      ensureRustTargetInstalled(key, env);
      buildNativeForTarget(key, env);
    }
  } else {
    console.warn("Skipping native module builds. Packaged apps will use the TypeScript mock if no compatible .node artifact is included.");
  }

  run("npm", ["run", skipNative ? "build:app" : "build"], { env });

  const builderArgs = ["electron-builder", "--config", "electron-builder.yml"];
  for (const [platform, arches] of electronBuilderTargets.entries()) {
    builderArgs.push(`--${platform}`, ...[...arches].map((arch) => `--${arch}`));
  }
  run("npx", builderArgs, { env });
} finally {
  restoreNativeArtifacts(stashedNativeArtifacts);
}

function buildNativeForTarget(key, env) {
  const target = targetMatrix[key];
  const artifactPath = path.join(rootDir, target.nodeArtifact);
  const args = [
    "napi",
    "build",
    "--cargo-cwd",
    "crates/native",
    "--platform",
    "--release",
    "--target",
    target.rustTarget
  ];

  console.log(`\nBuilding native module for ${key} (${target.rustTarget})`);
  run("npx", args, { env });

  if (!existsSync(artifactPath)) {
    throw new Error(`Expected native artifact was not produced: ${target.nodeArtifact}`);
  }
}

function expandTargets(values) {
  const expanded = [];
  for (const value of values) {
    const normalized = value.toLowerCase();
    const targets = aliases[normalized] ?? [normalized];
    for (const target of targets) {
      if (!targetMatrix[target]) {
        console.error(`Unknown package target: ${value}`);
        printUsageAndExit(1);
      }
      if (!expanded.includes(target)) {
        expanded.push(target);
      }
    }
  }
  return expanded;
}

function groupElectronTargets(targets) {
  const grouped = new Map();
  for (const key of targets) {
    const target = targetMatrix[key];
    const arches = grouped.get(target.platform) ?? new Set();
    arches.add(target.arch);
    grouped.set(target.platform, arches);
  }
  return grouped;
}

function currentTargetKey() {
  const platformMap = {
    darwin: "mac",
    win32: "win",
    linux: "linux"
  };
  const archMap = {
    arm64: "arm64",
    x64: "x64"
  };
  const platform = platformMap[process.platform];
  const arch = archMap[process.arch];
  if (!platform || !arch) {
    throw new Error(`Unsupported current package host: ${process.platform}/${process.arch}`);
  }
  return `${platform}-${arch}`;
}

function currentHostTargets() {
  if (process.platform === "darwin") {
    return ["mac-arm64", "mac-x64"];
  }
  return [currentTargetKey()];
}

function ensureRustTargetInstalled(key, env) {
  const target = targetMatrix[key];
  const installed = spawnSync("rustup", ["target", "list", "--installed"], {
    cwd: rootDir,
    env,
    encoding: "utf8",
    shell: process.platform === "win32"
  });

  if (installed.status !== 0) {
    console.warn(`Could not list installed Rust targets. Continuing and letting Cargo report target errors for ${target.rustTarget}.`);
    return;
  }

  if (installed.stdout.split(/\r?\n/).includes(target.rustTarget)) {
    return;
  }

  console.log(`\nInstalling Rust target ${target.rustTarget}`);
  run("rustup", ["target", "add", target.rustTarget], { env });
}

function takeFlag(values, flag) {
  const index = values.indexOf(flag);
  if (index === -1) {
    return false;
  }
  values.splice(index, 1);
  return true;
}

function buildEnvWithRustupCargo() {
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

  return env;
}

function run(command, args, { env }) {
  console.log(`\n$ ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: rootDir,
    env,
    stdio: "inherit",
    shell: process.platform === "win32"
  });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    throw new Error(`Command failed with exit code ${result.status ?? 1}: ${command} ${args.join(" ")}`);
  }
}

function stashNativeArtifacts() {
  const stashDir = path.join(rootDir, ".native-artifact-stash");
  rmSync(stashDir, { recursive: true, force: true });
  mkdirSync(stashDir, { recursive: true });

  const artifacts = readdirSync(rootDir)
    .filter((name) => /^index\..+\.node$/.test(name))
    .map((name) => ({
      name,
      from: path.join(rootDir, name),
      to: path.join(stashDir, name)
    }));

  for (const artifact of artifacts) {
    renameSync(artifact.from, artifact.to);
  }

  if (artifacts.length > 0) {
    console.warn(`Temporarily stashed ${artifacts.length} native artifact(s) for --skip-native packaging.`);
  }

  return artifacts;
}

function restoreNativeArtifacts(artifacts) {
  for (const artifact of artifacts) {
    if (existsSync(artifact.to)) {
      renameSync(artifact.to, artifact.from);
    }
  }
  rmSync(path.join(rootDir, ".native-artifact-stash"), { recursive: true, force: true });
}

function printPlan(targets, grouped, skipNativeBuild) {
  console.log("Package plan:");
  for (const target of targets) {
    const item = targetMatrix[target];
    console.log(`- ${target}: rust=${item.rustTarget}, artifact=${item.nodeArtifact}`);
  }
  console.log("electron-builder targets:");
  for (const [platform, arches] of grouped.entries()) {
    console.log(`- ${platform}: ${[...arches].join(", ")}`);
  }
  console.log(`native build: ${skipNativeBuild ? "skipped" : "enabled"}`);
}

function printUsageAndExit(code) {
  console.log(`
Usage:
  node scripts/package-platforms.mjs [target...] [--skip-native] [--dry-run]

Targets:
  current
  mac | mac-arm64 | mac-x64
  win | win-x64 | win-arm64
  linux | linux-x64 | linux-arm64
  all

Examples:
  npm run package
  npm run package -- current
  npm run package -- mac-arm64
  npm run package -- mac
  npm run package -- all
  npm run package -- linux-x64 --skip-native

Notes:
  On macOS, current builds both mac-arm64 and mac-x64 when the Rust target/toolchain is available.
`);
  process.exit(code);
}
