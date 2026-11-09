# Manual site

The short, picture-first manual, built with [Astro Starlight](https://starlight.astro.build) and published to GitHub
Pages by `.github/workflows/manual.yml`.

```sh
cd docs/manual-site
pnpm install
pnpm dev        # http://localhost:4321
pnpm build      # static site in dist/
```

- Pages are `.md` / `.mdx` under `src/content/docs/`; the sidebar groups follow the folders (`astro.config.mjs`).
- `<Shot name="develop" alt="…" />` shows a screenshot as Classic / Modern / Compact tabs. The reader's choice is shared
  by every shot on the site.
- Screenshots are committed (`src/assets/shots/<layout>/<name>.jpg`) and rendered headless from the procedural demo
  library: `node scripts/shots.mjs --cli path/to/app-cli [name …]`. Add a shot by adding an entry to
  `scripts/shots.json` (control-protocol steps; the script adds the layout switch, settle and screenshot).
- The product name comes from `brand.toml` (`astro.config.mjs`); pages may write the display name.
