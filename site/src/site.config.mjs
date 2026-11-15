// Single source of truth for branding and URLs. Plain .mjs so build scripts,
// the Astro config, and pages all read the same values.
//
// SITE_URL and BASE_PATH override siteUrl and basePath at build time. Use
// BASE_PATH=/ for a custom domain at the root.

export const site = {
  name: 'oncebitten',
  tagline: 'Turns repeated agent mistakes into rules that git enforces.',
  description:
    'oncebitten turns repeated agent mistakes into rules that git enforces: checks that run at pre-push, in CI and at PR creation.',
  repoUrl: 'https://github.com/bcmyguest/oncebitten',
  /** Branch that source links in the docs point at. */
  branch: 'main',
  siteUrl: 'https://bcmyguest.github.io',
  basePath: '/oncebitten/',
};
