"use strict";

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const PLATFORM_PACKAGES = {
  "linux-x64": "oncebitten-linux-x64",
  "linux-arm64": "oncebitten-linux-arm64",
  "darwin-x64": "oncebitten-darwin-x64",
  "darwin-arm64": "oncebitten-darwin-arm64",
  "win32-x64": "oncebitten-win32-x64",
};

function binaryPath(name, platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  const pkg = PLATFORM_PACKAGES[key];
  if (!pkg) {
    throw new Error(`oncebitten has no build for ${key}`);
  }
  let manifest;
  try {
    manifest = require.resolve(`${pkg}/package.json`);
  } catch {
    throw new Error(
      `${pkg} is not installed. Reinstall without --no-optional / --omit=optional.`,
    );
  }
  return path.join(path.dirname(manifest), "bin", platform === "win32" ? `${name}.exe` : name);
}

function run(name) {
  let exe;
  try {
    exe = binaryPath(name);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
  const result = spawnSync(exe, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    console.error(`cannot run ${exe}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.signal) {
    process.kill(process.pid, result.signal);
  }
  process.exit(result.status ?? 1);
}

module.exports = { PLATFORM_PACKAGES, binaryPath, run };
