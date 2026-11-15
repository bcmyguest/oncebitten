import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { test } from 'node:test';
import { diagramProblems, isMermaidSvg, mermaidFenceLines } from './lib/diagrams.mjs';

const docs = new URL('../../docs/', import.meta.url);
const list = (dir, ext) => readdirSync(new URL(dir, docs)).filter((f) => f.endsWith(ext));

test('mermaidFenceLines finds backtick and tilde fences only', () => {
  assert.deepEqual(mermaidFenceLines('a\n```mermaid\nflowchart LR\n```\n~~~ mermaid\n```js\n'), [2, 5]);
  assert.deepEqual(mermaidFenceLines('`mermaid` in prose\n```text\n```'), []);
});

test('diagramProblems reports missing and orphaned renders', () => {
  assert.deepEqual(diagramProblems(['a'], ['a.svg', 'a-dark.svg']), []);
  assert.deepEqual(diagramProblems(['a', 'b'], ['a.svg', 'b-dark.svg', 'c.svg']), [
    'docs/diagrams/a.mmd has no docs/media/a-dark.svg',
    'docs/diagrams/b.mmd has no docs/media/b.svg',
    'docs/media/c.svg has no docs/diagrams/c.mmd',
  ]);
});

test('docs embed rendered diagrams, not mermaid fences', () => {
  const found = list('.', '.md').flatMap((f) => mermaidFenceLines(readFileSync(new URL(f, docs), 'utf8')).map((n) => `docs/${f}:${n}`));
  assert.deepEqual(found, [], 'move each block to docs/diagrams/<name>.mmd, run `pnpm diagrams`, embed the SVG pair');
});

test('every docs/diagrams/*.mmd has both SVGs and every rendered SVG has its source', () => {
  const sources = list('diagrams/', '.mmd').map((f) => f.slice(0, -4));
  const renders = list('media/', '.svg').filter((f) => isMermaidSvg(readFileSync(new URL(`media/${f}`, docs), 'utf8')));
  assert.deepEqual(diagramProblems(sources, renders), [], 'run `pnpm diagrams` in site/');
});
