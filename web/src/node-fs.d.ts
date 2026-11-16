// The one node API the vitest suites use (loading the built .wasm bytes),
// declared here so @types/node's globals stay out of the browser code.
declare module 'node:fs' {
  export function readFileSync(path: URL): Uint8Array<ArrayBuffer>;
}
