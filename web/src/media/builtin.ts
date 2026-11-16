import { builtin_media_json } from '../../wasm/pieceoff_web_wasm.js';
import manifest from '../../public/builtin/manifest.json';

export interface BuiltinAssets {
  mp4: string;
  poster: string;
  title: string;
}

const files: Record<string, { mp4: string; poster: string }> = manifest;
let titles: Map<string, string> | null = null;

// Titles come from the core (the one source of truth for the built-in list), so
// the WASM module must be initialised before the first call; the manifest only
// carries the generated file names.
function titleOf(sha: string): string | undefined {
  titles ??= new Map(
    (JSON.parse(builtin_media_json()) as { sha256: string; title: string }[]).map((m) => [
      m.sha256,
      m.title,
    ]),
  );
  return titles.get(sha);
}

/** Playable assets for a bundled loop, or null when `sha256` is not built-in.
 *  Hex is case-insensitive; `base` is the URL prefix the bundle serves
 *  `public/builtin/` from. */
export function builtinAssets(sha256: string, base = '/builtin/'): BuiltinAssets | null {
  const sha = sha256.toLowerCase();
  const entry = Object.hasOwn(files, sha) ? files[sha] : undefined;
  const title = titleOf(sha);
  if (!entry || title === undefined) return null;
  return { mp4: base + entry.mp4, poster: base + entry.poster, title };
}
