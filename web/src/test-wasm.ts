import { readFileSync } from 'node:fs';
import init from '../wasm/pieceoff_web_wasm.js';

let ready: Promise<unknown> | null = null;

/** Initialise the core WASM module under node (wasm-bindgen's `--target web`
 *  glue fetches by URL; hand it the bytes instead). Idempotent. */
export function initWasm(): Promise<unknown> {
  ready ??= init({
    // A plain path, not a URL: under jsdom the global URL is jsdom's, which
    // node:fs rejects (and which resolves file: URLs differently).
    module_or_path: readFileSync(
      decodeURIComponent(import.meta.url.replace(/^file:\/\//, '').replace(/[^/]*$/, '')) + '../wasm/pieceoff_web_wasm_bg.wasm',
    ),
  });
  return ready;
}
