"use strict";

const { execFileSync, spawn, spawnSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { fileURLToPath } = require("node:url");

const DEFAULT_BASE = "https://github.com/bcmyguest/oncebitten/releases";
const FETCH_ENTRY = path.join(__dirname, "fetch.js");
const MAX_REDIRECTS = 10;
const TRIPLES = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
};

function version() {
  return require("../package.json").version;
}

function triple(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  const found = TRIPLES[key];
  if (!found) {
    throw new Error(`oncebitten has no build for ${key}`);
  }
  return found;
}

function releaseUrls(ver, target, base = DEFAULT_BASE) {
  const url = `${base.replace(/\/+$/, "")}/download/v${ver}/oncebitten-${target}.tar.gz`;
  return { archive: url, checksum: `${url}.sha256` };
}

function cacheRoot(env = process.env, platform = process.platform, home = os.homedir()) {
  if (env.ONCEBITTEN_CACHE_DIR) return env.ONCEBITTEN_CACHE_DIR;
  if (platform === "win32") {
    return path.join(env.LOCALAPPDATA || path.join(home, "AppData", "Local"), "oncebitten", "cache");
  }
  return path.join(env.XDG_CACHE_HOME || path.join(home, ".cache"), "oncebitten");
}

function binaryPath(name, platform = process.platform, env = process.env) {
  return path.join(cacheRoot(env, platform), version(), platform === "win32" ? `${name}.exe` : name);
}

function download(url, redirects = 0) {
  if (url.startsWith("file:")) {
    return Promise.resolve(fs.readFileSync(fileURLToPath(url)));
  }
  return new Promise((resolve, reject) => {
    https
      .get(url, (res) => {
        const { statusCode, headers } = res;
        if (statusCode >= 300 && statusCode < 400 && headers.location) {
          res.resume();
          if (redirects >= MAX_REDIRECTS) return reject(new Error(`too many redirects for ${url}`));
          return resolve(download(new URL(headers.location, url).href, redirects + 1));
        }
        if (statusCode !== 200) {
          res.resume();
          return reject(new Error(`cannot download ${url}: HTTP ${statusCode}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", (error) => reject(new Error(`cannot download ${url}: ${error.message}`)));
  });
}

async function install(env = process.env, platform = process.platform, arch = process.arch) {
  const ver = version();
  const target = triple(platform, arch);
  const urls = releaseUrls(ver, target, env.ONCEBITTEN_RELEASE_URL || DEFAULT_BASE);
  const root = cacheRoot(env, platform);
  const dest = path.join(root, ver);
  fs.mkdirSync(root, { recursive: true });
  const tmp = fs.mkdtempSync(path.join(root, ".tmp-"));
  try {
    const archive = await download(urls.archive);
    const expected = (await download(urls.checksum)).toString().trim().split(/\s+/)[0].toLowerCase();
    const actual = createHash("sha256").update(archive).digest("hex");
    if (actual !== expected) {
      throw new Error(`checksum mismatch for ${urls.archive}: want ${expected}, got ${actual}`);
    }
    fs.writeFileSync(path.join(tmp, "archive.tar.gz"), archive);
    fs.mkdirSync(path.join(tmp, "pkg"));
    execFileSync("tar", ["-xzf", "archive.tar.gz", "-C", "pkg"], { cwd: tmp, stdio: "ignore" });
    for (const file of fs.readdirSync(path.join(tmp, "pkg"))) {
      fs.chmodSync(path.join(tmp, "pkg", file), 0o755);
    }
    try {
      fs.renameSync(path.join(tmp, "pkg"), dest);
    } catch (error) {
      // A concurrent run won the race; its copy is complete.
      if (!fs.existsSync(dest)) throw error;
    }
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

function drainStdin(done) {
  process.stdin.on("data", () => {}).on("end", done).on("error", done);
}

function exec(exe) {
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

function run(name) {
  let exe;
  try {
    triple();
    exe = binaryPath(name);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
  if (fs.existsSync(exe)) exec(exe);
  if (["capture", "steer"].includes(process.argv[2])) {
    // A hook must not wait on a download: drop this event, fetch in the background.
    drainStdin(() => {
      spawn(process.execPath, [FETCH_ENTRY], { detached: true, stdio: "ignore" }).unref();
      process.exit(0);
    });
    return;
  }
  install().then(
    () => exec(exe),
    (error) => {
      console.error(`oncebitten: ${error.message}`);
      process.exit(2);
    },
  );
}

module.exports = { TRIPLES, binaryPath, cacheRoot, install, releaseUrls, run, triple };
