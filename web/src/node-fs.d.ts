// The few node APIs the vitest suites use (loading the built .wasm bytes,
// checking generated media), declared here so @types/node's globals stay out
// of the browser code.
declare module 'node:fs' {
  export function readFileSync(path: URL | string): Uint8Array<ArrayBuffer>;
  export function readFileSync(path: URL, encoding: 'utf8'): string;
  export function existsSync(path: URL): boolean;
  export function statSync(path: URL): { size: number };
  export function readdirSync(path: URL): string[];
}
declare module 'node:crypto' {
  export function createHash(algorithm: 'sha256'): {
    update(data: Uint8Array): { digest(encoding: 'hex'): string };
  };
}
declare module 'node:child_process' {
  export function execFileSync(file: string, args: string[]): { toString(): string };
  export function spawnSync(file: string, args: string[]): { status: number | null };
}
