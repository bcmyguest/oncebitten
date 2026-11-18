// Packs the single npm package, a launcher that fetches the release binary on first run.
//   node scripts/npm-package.mjs <version> <tarball-dir> <out-dir> [--all]
// <tarball-dir> and --all are accepted for older callers and ignored: the package holds no binaries.
// <out-dir> receives oncebitten-<version>.tgz.
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(fileURLToPath(import.meta.url), '../..');
const MAIN = join(ROOT, 'packaging/npm/oncebitten');

export function mainManifest(template, version) {
  return { ...template, version };
}

function pack(dir, out) {
  execFileSync('npm', ['pack', '--pack-destination', out, '--silent'], {
    cwd: dir,
    stdio: ['ignore', 'inherit', 'inherit'],
    shell: process.platform === 'win32',
  });
}

function main(argv) {
  const [version, tarballs, outArg] = argv;
  if (!version || !tarballs || !outArg) {
    throw new Error('usage: npm-package.mjs <version> <tarball-dir> <out-dir> [--all]');
  }
  const out = resolve(outArg);
  mkdirSync(out, { recursive: true });
  const work = mkdtempSync(join(tmpdir(), 'oncebitten-npm-'));
  try {
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
