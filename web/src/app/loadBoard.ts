// The board (WebGL renderer, gestures, overlays) is a separate chunk so the
// lobby entry stays small. Task 9 replaces `./boardStub` with the real board
// module and turns on esbuild code splitting; until then this placeholder is
// NOT IMPLEMENTED: it shows who joined and a way out, nothing more.
import type { Joined } from './join';

export interface BoardModule {
  mountBoard(root: HTMLElement, joined: Joined & { spectatorBecauseFull: boolean }, leave: () => void): { destroy(): void };
}

export function loadBoard(): Promise<BoardModule> {
  return import('./boardStub');
}
