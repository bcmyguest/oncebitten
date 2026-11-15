// Assembles the npm packages from the release tarballs and packs them.
//   node scripts/npm-package.mjs <version> <tarball-dir> <out-dir> [--all]
// <tarball-dir> holds oncebitten-<target>.tar.gz, as release.yml builds them.
// <out-dir> receives one .tgz per package. Without --all, targets whose
// tarball is missing are skipped (local runs build only the host target).
import { execFileSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(fileURLToPath(import.meta.url), '../..');
const MAIN = join(ROOT, 'packaging/npm/oncebitten');

export const TARGETS = [
  { target: 'x86_64-unknown-linux-musl', os: 'linux', cpu: 'x64' },
  { target: 'aarch64-unknown-linux-musl', os: 'linux', cpu: 'arm64' },
  { target: 'x86_64-apple-darwin', os: 'darwin', cpu: 'x64' },
  { target: 'aarch64-apple-darwin', os: 'darwin', cpu: 'arm64' },
  { target: 'x86_64-pc-windows-msvc', os: 'win32', cpu: 'x64' },
];
const BINS = ['oncebitten', 'oncebitten-checker'];

export function platformManifest({ os, cpu }, version) {
  return {
    name: `oncebitten-${os}-${cpu}`,
    version,
    description: `oncebitten binaries for ${os} ${cpu}`,
    license: 'Apache-2.0',
    repository: {
      type: 'git',
      url: 'git+https://github.com/bcmyguest/oncebitten.git',
    },
    homepage: 'https://bcmyguest.github.io/oncebitten/',
    os: [os],
    cpu: [cpu],
    files: ['bin'],
  };
}

export function mainManifest(template, version) {
  const manifest = { ...template, version };
  manifest.optionalDependencies = Object.fromEntries(
    Object.keys(template.optionalDependencies).map((name) => [name, version]),
  );
  return manifest;
}

function pack(dir, out) {
  execFileSync('npm', ['pack', '--pack-destination', out, '--silent'], {
    cwd: dir,
    stdio: ['ignore', 'inherit', 'inherit'],
    shell: process.platform === 'win32',
  });
}

function main(argv) {
  const [version, tarballs, outArg, ...flags] = argv;
  if (!version || !tarballs || !outArg) {
    throw new Error('usage: npm-package.mjs <version> <tarball-dir> <out-dir> [--all]');
  }
  const out = resolve(outArg);
  mkdirSync(out, { recursive: true });
  const work = mkdtempSync(join(tmpdir(), 'oncebitten-npm-'));
  const exeSuffix = (os) => (os === 'win32' ? '.exe' : '');
  let packed = 0;
  try {
    for (const entry of TARGETS) {
      const archive = join(resolve(tarballs), `oncebitten-${entry.target}.tar.gz`);
      if (!existsSync(archive)) {
        if (flags.includes('--all')) throw new Error(`missing ${archive}`);
        continue;
      }
      const dir = join(work, `${entry.os}-${entry.cpu}`);
      mkdirSync(join(dir, 'bin'), { recursive: true });
      execFileSync('tar', ['-xzf', archive, '-C', join(dir, 'bin')]);
      for (const bin of BINS) chmodSync(join(dir, 'bin', bin + exeSuffix(entry.os)), 0o755);
      writeFileSync(join(dir, 'package.json'), JSON.stringify(platformManifest(entry, version), null, 2) + '\n');
      pack(dir, out);
      packed += 1;
    }
    if (packed === 0) throw new Error(`no oncebitten-<target>.tar.gz in ${tarballs}`);
    const dir = join(work, 'main');
    cpSync(MAIN, dir, { recursive: true });
    const template = JSON.parse(readFileSync(join(MAIN, 'package.json'), 'utf8'));
    writeFileSync(join(dir, 'package.json'), JSON.stringify(mainManifest(template, version), null, 2) + '\n');
    pack(dir, out);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}
