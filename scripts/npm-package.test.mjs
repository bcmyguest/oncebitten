import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { TARGETS, mainManifest, platformManifest } from './npm-package.mjs';

const template = JSON.parse(readFileSync(new URL('../packaging/npm/oncebitten/package.json', import.meta.url), 'utf8'));

test('every platform package is an optional dependency of the main package', () => {
  const names = TARGETS.map((entry) => platformManifest(entry, '1.2.3').name).sort();
  assert.deepEqual(Object.keys(template.optionalDependencies).sort(), names);
});

test('the main manifest pins every platform package to its own version', () => {
  const manifest = mainManifest(template, '1.2.3');
  assert.equal(manifest.version, '1.2.3');
  assert.ok(Object.values(manifest.optionalDependencies).every((v) => v === '1.2.3'));
});

test('a platform manifest restricts os and cpu and ships only bin', () => {
  const manifest = platformManifest(TARGETS[4], '1.2.3');
  assert.deepEqual([manifest.os, manifest.cpu, manifest.files], [['win32'], ['x64'], ['bin']]);
});
