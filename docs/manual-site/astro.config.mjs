import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightImageZoom from 'starlight-image-zoom';
import { readFileSync } from 'node:fs';

// The product name lives in brand.toml only.
const brand = readFileSync(new URL('../../brand.toml', import.meta.url), 'utf8');
const app = /^display_name\s*=\s*"([^"]+)"/m.exec(brand)?.[1];
if (!app) throw new Error('display_name not found in brand.toml');

// Base path is set by the Pages workflow (MANUAL_BASE); `/` for local dev.
export default defineConfig({
  site: process.env.MANUAL_SITE ?? 'http://localhost:4321',
  base: process.env.MANUAL_BASE ?? '/',
  integrations: [
    starlight({
      title: `${app} Manual`,
      description: 'A photo library and non-destructive raw developer. The short manual.',
      plugins: [starlightImageZoom()],
      customCss: ['./src/styles/manual.css'],
      sidebar: [
        { label: 'Start here', items: [{ slug: 'start/install' }, { slug: 'start/layouts' }, { slug: 'start/tour' }] },
        { label: 'Library', autogenerate: { directory: 'library' } },
        { label: 'Develop', autogenerate: { directory: 'develop' } },
        { label: 'Output', autogenerate: { directory: 'output' } },
        { label: 'Automate', autogenerate: { directory: 'automate' } },
      ],
    }),
  ],
});
