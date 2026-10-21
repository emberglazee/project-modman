#!/usr/bin/env node
// Builds dist/modman-merge.html: one self-contained page that runs straight
// from file:// — no server, no network. Inlines CSS + JS and embeds the wasm
// as base64; the page initializes synchronously from the embedded bytes.
//
// Usage: node web/build-single.mjs   (requires web/pkg from wasm-pack
//        --target no-modules, i.e. the glue exposes a `wasm_bindgen` global)

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const web = dirname(fileURLToPath(import.meta.url));
const pkg = join(web, 'pkg');

const html = readFileSync(join(web, 'index.html'), 'utf8');
const css = readFileSync(join(web, 'style.css'), 'utf8');
const app = readFileSync(join(web, 'app.js'), 'utf8');
const glue = readFileSync(join(pkg, 'modman_wasm.js'), 'utf8');
const wasm = readFileSync(join(pkg, 'modman_wasm_bg.wasm'));
const wasmB64 = wasm.toString('base64');

for (const [name, text] of [['glue', glue], ['app', app]]) {
  if (text.includes('</script')) throw new Error(`${name} contains </script`);
}

// Tolerant of a cache-busting query on the script tags (`app.js?v=0.5.0`):
// the hosted page uses one, and a literal match here silently stopped matching.
const SCRIPT_WASM = /<script src="pkg\/modman_wasm\.js(?:\?[^"]*)?"><\/script>/;
const SCRIPT_APP = /<script src="app\.js(?:\?[^"]*)?"><\/script>/;
const LINK_CSS = /<link rel="stylesheet" href="style\.css(?:\?[^"]*)?">/;

let out = html
  .replace(LINK_CSS, () => `<style>\n${css}\n</style>`)
  .replace(SCRIPT_WASM, '')
  .replace(
    SCRIPT_APP,
    () =>
      `<script>window.MODMAN_WASM_BASE64 = "${wasmB64}";</script>\n` +
      `<script>\n${glue}\n</script>\n` +
      `<script>\n${app}\n</script>`,
  );

if (!out.includes('MODMAN_WASM_BASE64')) throw new Error('inline replacement failed');
if (SCRIPT_APP.test(out) || SCRIPT_WASM.test(out)) {
  throw new Error('a script tag was left un-inlined; index.html changed shape');
}

mkdirSync(join(web, 'dist'), { recursive: true });
const outPath = join(web, 'dist', 'modman-merge.html');
writeFileSync(outPath, out);
console.log(`Wrote ${outPath} (${(Buffer.byteLength(out) / 1024 / 1024).toFixed(2)} MB)`);
