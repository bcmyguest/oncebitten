// Mermaid diagrams: docs/diagrams/<name>.mmd renders to docs/media/<name>.svg
// (light) and docs/media/<name>-dark.svg (dark). See scripts/diagrams.mjs.

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*mermaid\b/;
const MERMAID_SVG = /<svg\b[^>]*\baria-roledescription="/;

/** Mermaid config for one theme mode, colored from src/theme.yml semantic tokens. */
export function mermaidConfig(t, dark) {
  return {
    theme: 'base',
    themeVariables: {
      darkMode: dark,
      fontFamily: "'DM Sans', sans-serif",
      fontSize: '15px',
      background: t.panel,
      mainBkg: t['brand-soft'],
      primaryColor: t['brand-soft'],
      primaryTextColor: t.ink,
      primaryBorderColor: t.brand,
      nodeBorder: t.brand,
      nodeTextColor: t.ink,
      secondaryColor: t.panel,
      tertiaryColor: t.panel,
      textColor: t.ink,
      lineColor: t.muted,
      edgeLabelBackground: t.panel,
      clusterBkg: t.panel,
      clusterBorder: t.line,
      titleColor: t.ink,
    },
    look: 'classic',
    flowchart: { padding: 12, nodeSpacing: 90, wrappingWidth: 260 },
  };
}

/** 1-based line numbers of ```mermaid (or ~~~mermaid) fences in markdown. */
export function mermaidFenceLines(md) {
  return md.split('\n').flatMap((line, i) => (FENCE.test(line) ? [i + 1] : []));
}

/** Whether an SVG file is mermaid output (logos and other art are not). */
export function isMermaidSvg(svg) {
  return MERMAID_SVG.test(svg);
}

/**
 * Missing renders and orphaned renders.
 * @param {string[]} sources diagram names from docs/diagrams/*.mmd
 * @param {string[]} renders mermaid SVG file names in docs/media
 */
export function diagramProblems(sources, renders) {
  const have = new Set(renders);
  const missing = sources.flatMap((n) => [`${n}.svg`, `${n}-dark.svg`].filter((f) => !have.has(f)).map((f) => `docs/diagrams/${n}.mmd has no docs/media/${f}`));
  const names = new Set(sources);
  const orphans = renders
    .filter((f) => !names.has(f.replace(/(-dark)?\.svg$/, '')))
    .map((f) => `docs/media/${f} has no docs/diagrams/${f.replace(/(-dark)?\.svg$/, '')}.mmd`);
  return [...missing, ...orphans];
}
