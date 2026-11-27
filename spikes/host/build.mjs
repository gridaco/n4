import { execFile } from 'node:child_process';
import { copyFile, mkdir } from 'node:fs/promises';
import { promisify } from 'node:util';
import { build } from 'esbuild';

const src = 'harness/src';
const out = 'harness/dist';
await mkdir(out, { recursive: true });

const bundles = [
  ['main/index.ts', 'main.cjs', 'node', 'cjs'],
  ['preload/index.ts', 'preload.cjs', 'node', 'cjs'],
  ['renderer/index.ts', 'renderer.js', 'browser', 'esm'],
  ['export/preload.ts', 'export-preload.cjs', 'node', 'cjs'],
  ['export/index.ts', 'export.js', 'browser', 'esm'],
].map(([entry, file, platform, format]) => build({
  entryPoints: [`${src}/${entry}`],
  outfile: `${out}/${file}`,
  platform,
  format,
  bundle: true,
  target: 'es2024',
  external: ['electron'],
}));

await Promise.all([
  ...bundles,
  copyFile(`${src}/renderer/index.html`, `${out}/index.html`),
  copyFile(`${src}/export/index.html`, `${out}/export.html`),
  promisify(execFile)('tailwindcss', ['-i', `${src}/renderer/viewer.css`, '-o', `${out}/viewer.css`, '--minify']),
]);
