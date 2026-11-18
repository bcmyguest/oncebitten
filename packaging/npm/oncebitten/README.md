# oncebitten

Turns repeated agent mistakes into rules that git enforces.

```sh
npx oncebitten --version
npm install --global oncebitten
```

This package is a launcher. On first run it downloads the release binaries for your platform from the
GitHub Release, verifies their SHA-256, caches them and runs them. Nothing runs at install time, so
`--ignore-scripts` works; the first run needs network access.

- `ONCEBITTEN_RELEASE_URL`: releases base URL (default: `https://github.com/bcmyguest/oncebitten/releases`).
- `ONCEBITTEN_CACHE_DIR`: cache directory (default: `$XDG_CACHE_HOME/oncebitten` or `~/.cache/oncebitten`; `%LOCALAPPDATA%\oncebitten\cache` on Windows).

Docs: https://bcmyguest.github.io/oncebitten/ · Source: https://github.com/bcmyguest/oncebitten
