// Outline level of detail: how many contour samples a piece gets for its size on
// screen, and when a zoom change is worth rebuilding the mesh. Moved out of the
// renderer spike (dev/spike.ts) so the board and the spike share one policy.
import { strideFor } from './mesh';

/** Wait this long after the last zoom step before rebuilding (a pinch is many steps). */
export const LOD_DEBOUNCE_MS = 150;

/** Samples worth drawing for a cell of cellW x cellH source px at `scale` CSS px
 *  per source px: four per screen px of the larger side, over 2.2 (the measured
 *  point below which thinning is invisible). A fixed `cap` wins. */
export function wantedSamples(cellW: number, cellH: number, scale: number, cap: number | null): number {
  return cap ?? (Math.max(cellW, cellH) * scale * 4) / 2.2;
}

export interface LodTimers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface LodOptions {
  /** Full sample count of a representative piece (the first outline). */
  samples0: number;
  cellW: number;
  cellH: number;
  /** Fixed max samples (no rebuilds), or null/undefined for zoom-driven. */
  cap?: number | null;
  timers?: LodTimers;
  /** Build the mesh again with this max sample count. */
  rebuild(maxSamples: number): void;
}

const realTimers: LodTimers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

/** Decides when to rebuild: only when the stride for the representative piece
 *  changes, debounced so a pinch rebuilds once at its end. */
export class LodPolicy {
  private stride = 0;
  private timer: unknown = null;
  private scale = 1;
  private disposed = false;
  private readonly cap: number | null;
  private readonly timers: LodTimers;

  constructor(private readonly o: LodOptions) {
    this.cap = o.cap ?? null;
    this.timers = o.timers ?? realTimers;
  }

  samplesAt(scale: number): number {
    return wantedSamples(this.o.cellW, this.o.cellH, scale, this.cap);
  }

  /** The mesh now in use was built with `stride` for the representative piece. */
  built(stride: number): void {
    this.stride = stride;
  }

  /** The camera scale changed. */
  viewChanged(scale: number): void {
    if (this.disposed || this.cap !== null) return;
    this.scale = scale;
    const want = strideFor(this.o.samples0, this.samplesAt(scale));
    if (want === this.stride) {
      this.cancel();
      return;
    }
    this.cancel();
    this.timer = this.timers.set(() => {
      this.timer = null;
      this.o.rebuild(this.samplesAt(this.scale));
    }, LOD_DEBOUNCE_MS);
  }

  dispose(): void {
    this.cancel();
    this.disposed = true;
  }

  private cancel(): void {
    if (this.timer !== null) this.timers.clear(this.timer);
    this.timer = null;
  }
}
