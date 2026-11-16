import { readFileSync } from 'node:fs';
import init from '../wasm/pieceoff_web_wasm.js';

let ready: Promise<unknown> | null = null;

/** Initialise the core WASM module under node (wasm-bindgen's `--target web`
 *  glue fetches by URL; hand it the bytes instead). Idempotent. */
export function initWasm(): Promise<unknown> {
  ready ??= init({
    module_or_path: readFileSync(new URL('../wasm/pieceoff_web_wasm_bg.wasm', import.meta.url)),
  });
  return ready;
}
