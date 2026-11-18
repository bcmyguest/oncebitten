import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { mainManifest } from './npm-package.mjs';

const template = JSON.parse(readFileSync(new URL('../packaging/npm/oncebitten/package.json', import.meta.url), 'utf8'));

test('the manifest has no dependencies of any kind', () => {
  assert.deepEqual(Object.keys(template).filter((key) => /dependencies$/i.test(key)), []);
});

test('the main manifest takes the release version', () => {
  assert.equal(mainManifest(template, '1.2.3').version, '1.2.3');
});

test('packing yields one tarball without the launcher tests', () => {
  const out = mkdtempSync(join(tmpdir(), 'oncebitten-pack-'));
  try {
    const script = fileURLToPath(new URL('./npm-package.mjs', import.meta.url));
    execFileSync(process.execPath, [script, '1.2.3', 'unused', out, '--all'], { stdio: 'ignore' });
    assert.deepEqual(readdirSync(out), ['oncebitten-1.2.3.tgz']);
    const files = execFileSync('tar', ['-tzf', join(out, 'oncebitten-1.2.3.tgz')]).toString();
    assert.ok(files.includes('package/lib/launch.js') && files.includes('package/lib/fetch.js'));
    assert.ok(!files.includes('launch.test.js'));
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});
