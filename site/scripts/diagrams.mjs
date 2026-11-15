// Renders docs/diagrams/<name>.mmd to docs/media/<name>.svg and <name>-dark.svg,
// colored from src/theme.yml with DM Sans embedded. Run by hand (`pnpm diagrams`)
// and commit the SVGs; the build does not render, so CI needs no browser.
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '@mermaid-js/mermaid-cli';
import { parse } from 'yaml';
import { mermaidConfig } from './lib/diagrams.mjs';

const site = new URL('..', import.meta.url);
const sources = new URL('../docs/diagrams/', site);
const media = new URL('../docs/media/', site);
const semantic = parse(readFileSync(new URL('src/theme.yml', site), 'utf8')).theme.semantic;
const font = new URL(import.meta.resolve('@fontsource/dm-sans/400.css'));

// Puppeteer's Chrome download is a build script pnpm skips; fetch the pinned
// headless shell into ~/.cache/puppeteer (a no-op once present).
const puppeteer = createRequire(import.meta.resolve('@mermaid-js/mermaid-cli')).resolve('puppeteer');
execFileSync(process.execPath, [join(dirname(puppeteer), 'node', 'cli.js'), 'browsers', 'install', 'chrome-headless-shell'], { stdio: 'ignore' });

// Ubuntu 23.10+ blocks Chrome's user-namespace sandbox. The page renders only
// this repo's .mmd files, so run without it.
const puppeteerConfig = { headless: 'shell', args: ['--no-sandbox'] };

const names = readdirSync(sources)
  .filter((f) => f.endsWith('.mmd'))
  .map((f) => f.slice(0, -4));
for (const name of names) {
  for (const [mode, suffix] of [['light', ''], ['dark', '-dark']]) {
    const tokens = semantic[mode];
    await run(fileURLToPath(new URL(`${name}.mmd`, sources)), fileURLToPath(new URL(`${name}${suffix}.svg`, media)), {
      quiet: true,
      puppeteerConfig,
      parseMMDOptions: {
        mermaidConfig: mermaidConfig(tokens, mode === 'dark'),
        backgroundColor: tokens.panel,
        customFontCSS: [{ cssUrl: font }],
      },
    });
    console.log(`diagrams: docs/media/${name}${suffix}.svg`);
  }
}
