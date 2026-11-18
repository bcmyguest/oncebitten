"use strict";

const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { pathToFileURL } = require("node:url");

const { binaryPath, cacheRoot, releaseUrls, triple } = require("./launch.js");
const { version } = require("../package.json");

const BIN = path.join(__dirname, "..", "bin", "oncebitten.js");
const NAME = `oncebitten-${triple()}.tar.gz`;

function tmp() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "oncebitten-launch-"));
}

// Lays out <base>/download/v<version>/ with a tarball of a shell-script oncebitten.
function release(base, { sha } = {}) {
  const dir = path.join(base, "download", `v${version}`);
  const src = path.join(base, "src");
  fs.mkdirSync(dir, { recursive: true });
  fs.mkdirSync(src);
  fs.writeFileSync(path.join(src, "oncebitten"), '#!/bin/sh\necho "fake $*"\nexit 7\n');
  execFileSync("tar", ["-czf", path.join(dir, NAME), "oncebitten"], { cwd: src });
  const digest = sha ?? createHash("sha256").update(fs.readFileSync(path.join(dir, NAME))).digest("hex");
  fs.writeFileSync(path.join(dir, `${NAME}.sha256`), `${digest}  ${NAME}\n`);
}

function launch(base, cache, args, extra = {}) {
  return spawnSync(process.execPath, [BIN, ...args], {
    env: {
      ...process.env,
      ONCEBITTEN_RELEASE_URL: pathToFileURL(base).href,
      ONCEBITTEN_CACHE_DIR: cache,
    },
    encoding: "utf8",
    ...extra,
  });
}

test("platforms map to release triples", () => {
  assert.equal(triple("linux", "x64"), "x86_64-unknown-linux-musl");
  assert.equal(triple("linux", "arm64"), "aarch64-unknown-linux-musl");
  assert.equal(triple("darwin", "x64"), "x86_64-apple-darwin");
  assert.equal(triple("darwin", "arm64"), "aarch64-apple-darwin");
  assert.equal(triple("win32", "x64"), "x86_64-pc-windows-msvc");
  assert.throws(() => triple("freebsd", "x64"), /oncebitten has no build for freebsd-x64/);
});

test("release urls follow install.sh, with the default and an override base", () => {
  const archive = "https://github.com/bcmyguest/oncebitten/releases/download/v1.2.3/oncebitten-x86_64-apple-darwin.tar.gz";
  assert.deepEqual(releaseUrls("1.2.3", "x86_64-apple-darwin"), { archive, checksum: `${archive}.sha256` });
  assert.equal(
    releaseUrls("1.2.3", "t", "file:///mirror/").archive,
    "file:///mirror/download/v1.2.3/oncebitten-t.tar.gz",
  );
});

test("the cache dir honours the override, XDG and the home default", () => {
  assert.equal(cacheRoot({ ONCEBITTEN_CACHE_DIR: "/c" }, "linux", "/h"), "/c");
  assert.equal(cacheRoot({ XDG_CACHE_HOME: "/x" }, "linux", "/h"), path.join("/x", "oncebitten"));
  assert.equal(cacheRoot({}, "linux", "/h"), path.join("/h", ".cache", "oncebitten"));
  assert.equal(cacheRoot({ LOCALAPPDATA: "/l" }, "win32", "/h"), path.join("/l", "oncebitten", "cache"));
});

test("first run downloads, caches and runs the binary", { skip: process.platform === "win32" }, () => {
  const base = tmp();
  const cache = tmp();
  try {
    release(base);
    const first = launch(base, cache, ["--version"]);
    assert.equal(first.stdout, "fake --version\n");
    assert.equal(first.status, 7);
    const cached = binaryPath("oncebitten", process.platform, { ONCEBITTEN_CACHE_DIR: cache });
    assert.ok(fs.existsSync(cached));
    fs.rmSync(path.join(base, "download"), { recursive: true });
    assert.equal(launch(base, cache, ["again"]).stdout, "fake again\n");
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
    fs.rmSync(cache, { recursive: true, force: true });
  }
});

test("a checksum mismatch exits 2 and leaves nothing cached", () => {
  const base = tmp();
  const cache = tmp();
  try {
    release(base, { sha: "0".repeat(64) });
    const result = launch(base, cache, ["--version"]);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /^oncebitten: checksum mismatch/);
    assert.deepEqual(fs.readdirSync(cache), []);
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
    fs.rmSync(cache, { recursive: true, force: true });
  }
});

test("a failed download exits 2 with one oncebitten line", () => {
  const base = tmp();
  const cache = tmp();
  try {
    const result = launch(base, cache, ["--version"]);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /^oncebitten: cannot|^oncebitten: ENOENT/);
    assert.equal(result.stderr.trim().split("\n").length, 1);
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
    fs.rmSync(cache, { recursive: true, force: true });
  }
});

test("capture and steer drain stdin and exit 0 without waiting for a download", () => {
  for (const hook of ["capture", "steer"]) {
    const base = tmp();
    const cache = tmp();
    try {
      const result = launch(base, cache, [hook, "--host", "claude"], { input: '{"x":1}', timeout: 5000 });
      assert.equal(result.status, 0);
      assert.equal(result.stdout, "");
      assert.ok(!fs.existsSync(path.join(cache, version)));
    } finally {
      fs.rmSync(base, { recursive: true, force: true });
      fs.rmSync(cache, { recursive: true, force: true });
    }
  }
});
