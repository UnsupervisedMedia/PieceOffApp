// The board (WebGL renderer, gestures, overlays) is a separate lazy chunk
// (esbuild code splitting) so the lobby entry stays small: nothing in the
// entry may import ./board statically.
import type { Joined } from './join';

export interface BoardModule {
  mountBoard(
    root: HTMLElement,
    joined: Joined & { spectatorBecauseFull: boolean },
    leave: () => void,
    config: { assetBase: string; site: string },
  ): { destroy(): void };
}

export function loadBoard(): Promise<BoardModule> {
  return import('./board').then((m) => ({ mountBoard: m.mountJoined }));
}
