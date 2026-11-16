// Everything on the table that is not a piece, drawn with Canvas 2D on two
// layers around the WebGL canvas:
//
//   under (below the GL canvas, which clears to transparent): the table, the
//     board frame, the tray area and the holders, so holders lie under the
//     pieces like on desktop;
//   over (above it, pointer-events: none): rubber-band, pings and remote cursors.
//
// Why 2D canvases and not more draws in the GL pass: these carry text (player
// names, holder labels), which WebGL would need a glyph atlas for; they are a
// handful of rectangles and arrows, cheap to redraw on demand; and they keep
// working while the GL context is lost, so a player still sees the table,
// cursors and pings (and can keep dragging) until it is restored. The pieces
// themselves stay one fill draw and one outline draw in the GL renderer.
//
// Colours follow the desktop (cursors.rs `player_color`, board.rs table
// colours): a player's colour comes from an FNV-1a hash of the display name,
// which survives a reconnect (client ids do not).
import type { PieceHolder } from '../protocol';
import type { Camera } from './camera';
import { MAX_DPR } from './renderer';

/** Remote cursors are forgotten after this long without an update (ARCHITECTURE.md). */
export const CURSOR_TTL_MS = 3000;
/** Pings show this long. */
export const PING_TTL_MS = 3000;
/** The last part of a cursor's life is a fade (desktop FADE_SECONDS). */
const CURSOR_FADE_MS = 500;

/** Board geometry from core (`board_layout` in the web wasm), board coordinates. */
export interface BoardLayout {
  tray: { x: number; y: number; width: number; height: number };
  tray_pad: number;
  scene: { min_x: number; min_y: number; max_x: number; max_y: number };
}

export interface CursorMark {
  name: string;
  x: number;
  y: number;
  /** Local ms of the last update. */
  at: number;
}

export interface PingMark {
  name: string;
  x: number;
  y: number;
  at: number;
}

/** Table-space rectangle. */
export interface OverlayRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 32-bit FNV-1a over the UTF-8 bytes (core `fnv1a`). */
export function fnv1a(text: string): number {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(text)) {
    h ^= b;
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** A player's colour: hue from the name hash, saturation 0.7, value 1 (desktop). */
export function playerColor(name: string): string {
  const [r, g, b] = hsv((fnv1a(name) % 360) / 360, 0.7, 1);
  return `rgb(${r}, ${g}, ${b})`;
}

function hsv(h: number, s: number, v: number): [number, number, number] {
  const i = Math.floor(h * 6);
  const f = h * 6 - i;
  const p = v * (1 - s);
  const q = v * (1 - f * s);
  const t = v * (1 - (1 - f) * s);
  const [r, g, b] = [
    [v, t, p],
    [q, v, p],
    [p, v, t],
    [p, q, v],
    [t, p, v],
    [v, p, q],
  ][i % 6];
  return [Math.round(r * 255), Math.round(g * 255), Math.round(b * 255)];
}

function withAlpha(rgb: string, a: number): string {
  return rgb.replace('rgb(', 'rgba(').replace(')', `, ${a})`);
}

/** What is alive on the over layer: pings this client saw, and (owned by
 *  RoomState) remote cursors, which expire here. */
export class OverlayModel {
  private pings: PingMark[] = [];

  addPing(name: string, x: number, y: number, now: number): void {
    this.pings.push({ name, x, y, at: now });
  }

  livePings(now: number): PingMark[] {
    this.pings = this.pings.filter((p) => now - p.at < PING_TTL_MS);
    return this.pings;
  }

  /** Cursors updated within CURSOR_TTL_MS; older ones are deleted from `cursors`. */
  liveCursors(cursors: Map<string, CursorMark>, now: number): CursorMark[] {
    const out: CursorMark[] = [];
    for (const [id, c] of cursors) {
      if (now - c.at >= CURSOR_TTL_MS) cursors.delete(id);
      else out.push(c);
    }
    return out;
  }

  clear(): void {
    this.pings = [];
  }
}

export interface OverlayOptions {
  model: OverlayModel;
  cursors: () => Map<string, CursorMark>;
  now?: () => number;
  raf?: (cb: FrameRequestCallback) => number;
  caf?: (h: number) => void;
  /** 2D context factory (tests inject a recorder; jsdom has none). */
  context2d?: (c: HTMLCanvasElement) => CanvasRenderingContext2D | null;
  /** Names whose cursors and pings are hidden (local mute). */
  muted?: () => { has(name: string): boolean };
}

export interface OverlayScene {
  layout: BoardLayout | null;
  boardW: number;
  boardH: number;
}

type Ctx = CanvasRenderingContext2D;

export class OverlayLayers {
  private scene: OverlayScene = { layout: null, boardW: 0, boardH: 0 };
  private holders: PieceHolder[] = [];
  private rubber: OverlayRect | null = null;
  private underDirty = true;
  private overDirty = true;
  private handle = 0;
  private pending = false;
  private disposed = false;
  private dpr = 1;
  private cssW = 1;
  private cssH = 1;
  private uctx: Ctx | null | undefined;
  private octx: Ctx | null | undefined;
  private readonly now: () => number;
  private readonly raf: (cb: FrameRequestCallback) => number;
  private readonly caf: (h: number) => void;
  private readonly context2d: (c: HTMLCanvasElement) => Ctx | null;

  constructor(
    readonly underCanvas: HTMLCanvasElement,
    readonly overCanvas: HTMLCanvasElement,
    private readonly camera: Camera,
    private readonly o: OverlayOptions,
  ) {
    this.now = o.now ?? (() => Date.now());
    this.raf = o.raf ?? ((cb) => requestAnimationFrame(cb));
    this.caf = o.caf ?? ((h) => cancelAnimationFrame(h));
    this.context2d =
      o.context2d ??
      ((c) => {
        try {
          return c.getContext('2d');
        } catch {
          return null;
        }
      });
  }

  setScene(s: OverlayScene): void {
    this.scene = s;
    this.requestDraw('under');
  }

  setHolders(h: PieceHolder[]): void {
    this.holders = h;
    this.requestDraw('under');
  }

  setRubber(r: OverlayRect | null): void {
    this.rubber = r;
    this.requestDraw('over');
  }

  /** Match both canvases to their CSS size at the current DPR. */
  resize(cssW: number, cssH: number): void {
    this.cssW = Math.max(1, cssW);
    this.cssH = Math.max(1, cssH);
    this.dpr = Math.min(MAX_DPR, globalThis.devicePixelRatio || 1);
    for (const c of [this.underCanvas, this.overCanvas]) {
      const w = Math.round(this.cssW * this.dpr);
      const h = Math.round(this.cssH * this.dpr);
      if (c.width !== w) c.width = w;
      if (c.height !== h) c.height = h;
    }
    this.requestDraw();
  }

  requestDraw(which: 'under' | 'over' | 'both' = 'both'): void {
    if (this.disposed) return;
    if (which !== 'over') this.underDirty = true;
    if (which !== 'under') this.overDirty = true;
    if (this.pending) return;
    this.pending = true;
    this.handle = this.raf(this.frame);
  }

  dispose(): void {
    this.disposed = true;
    if (this.pending) this.caf(this.handle);
    this.pending = false;
  }

  private readonly frame = (): void => {
    this.pending = false;
    if (this.disposed) return;
    if (this.underDirty) {
      this.underDirty = false;
      this.uctx ??= this.context2d(this.underCanvas);
      if (this.uctx) this.drawUnder(this.uctx);
    }
    if (this.overDirty) {
      this.overDirty = false;
      this.octx ??= this.context2d(this.overCanvas);
      const alive = this.drawOver(this.octx ?? null);
      // Fades, ping rings and expiry need frames while anything is alive.
      if (alive) this.requestDraw('over');
    }
  };

  private begin(ctx: Ctx): void {
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.cssW, this.cssH);
  }

  private screenRect(x: number, y: number, w: number, h: number): [number, number, number, number] {
    const [sx, sy] = this.camera.toScreen(x, y);
    return [sx, sy, w * this.camera.scale, h * this.camera.scale];
  }

  private drawUnder(ctx: Ctx): void {
    this.begin(ctx);
    ctx.fillStyle = 'rgb(22, 24, 29)';
    ctx.fillRect(0, 0, this.cssW, this.cssH);
    const { layout, boardW, boardH } = this.scene;
    if (layout) {
      const s = layout.scene;
      roundRect(ctx, ...this.screenRect(s.min_x, s.min_y, s.max_x - s.min_x, s.max_y - s.min_y), 12);
      ctx.fillStyle = 'rgb(35, 38, 44)';
      ctx.fill();
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.11)';
      ctx.lineWidth = 1;
      ctx.stroke();
      const t = layout.tray;
      const p = layout.tray_pad;
      roundRect(ctx, ...this.screenRect(t.x - p, t.y - p, t.width + 2 * p, t.height + 2 * p), 10);
      ctx.fillStyle = 'rgb(27, 31, 39)';
      ctx.fill();
    }
    if (boardW > 0 && boardH > 0) {
      const [x, y, w, h] = this.screenRect(0, 0, boardW, boardH);
      roundRect(ctx, x - 3, y - 3, w + 6, h + 6, 6);
      ctx.fillStyle = 'rgb(17, 19, 24)';
      ctx.fill();
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.09)';
      ctx.stroke();
    }
    ctx.font = '12px system-ui, sans-serif';
    ctx.textBaseline = 'top';
    for (const hd of this.holders) {
      const colour = playerColor(hd.owner);
      const away = hd.owner_away_since_ms != null;
      const [x, y, w, h] = this.screenRect(hd.x, hd.y, hd.width, hd.height);
      roundRect(ctx, x, y, w, h, 8);
      ctx.fillStyle = withAlpha(colour, away ? 0.06 : 0.14);
      ctx.fill();
      ctx.setLineDash?.(away ? [6, 4] : []);
      ctx.strokeStyle = withAlpha(colour, away ? 0.5 : 0.9);
      ctx.lineWidth = 2;
      ctx.stroke();
      ctx.setLineDash?.([]);
      const label = hd.label ? `${hd.label} · ${hd.owner}` : hd.owner;
      ctx.fillStyle = withAlpha(colour, away ? 0.6 : 1);
      ctx.fillText(label, x + 6, y + 5, Math.max(10, w - 12));
    }
  }

  /** Returns whether anything animated is still alive. */
  private drawOver(ctx: Ctx | null): boolean {
    const now = this.now();
    const muted = this.o.muted?.() ?? null;
    const cursors = this.o.model.liveCursors(this.o.cursors(), now).filter((c) => !muted?.has(c.name));
    const pings = this.o.model.livePings(now).filter((p) => !muted?.has(p.name));
    const alive = cursors.length > 0 || pings.length > 0;
    if (!ctx) return alive;
    this.begin(ctx);
    const r = this.rubber;
    if (r) {
      const [x, y, w, h] = this.screenRect(r.x, r.y, r.w, r.h);
      ctx.fillStyle = 'rgba(255, 200, 40, 0.18)';
      ctx.fillRect(x, y, w, h);
      ctx.strokeStyle = 'rgb(255, 200, 40)';
      ctx.lineWidth = 1;
      ctx.strokeRect(x + 0.5, y + 0.5, w, h);
    }
    for (const p of pings) {
      const t = (now - p.at) / PING_TTL_MS;
      const [sx, sy] = this.camera.toScreen(p.x, p.y);
      const colour = playerColor(p.name);
      for (const phase of [0, 0.5]) {
        const k = (t * 3 + phase) % 1;
        ctx.beginPath();
        ctx.arc(sx, sy, 6 + 26 * k, 0, Math.PI * 2);
        ctx.strokeStyle = withAlpha(colour, (1 - k) * (1 - t));
        ctx.lineWidth = 3;
        ctx.stroke();
      }
      ctx.beginPath();
      ctx.arc(sx, sy, 4, 0, Math.PI * 2);
      ctx.fillStyle = withAlpha(colour, 1 - t);
      ctx.fill();
    }
    ctx.font = '12px system-ui, sans-serif';
    ctx.textBaseline = 'middle';
    for (const c of cursors) {
      const left = CURSOR_TTL_MS - (now - c.at);
      const alpha = Math.max(0, Math.min(1, left / CURSOR_FADE_MS));
      const [sx, sy] = this.camera.toScreen(c.x, c.y);
      const colour = playerColor(c.name);
      ctx.globalAlpha = alpha;
      ctx.beginPath();
      ctx.moveTo(sx, sy);
      ctx.lineTo(sx, sy + 16);
      ctx.lineTo(sx + 4.5, sy + 12);
      ctx.lineTo(sx + 11, sy + 11);
      ctx.closePath();
      ctx.fillStyle = colour;
      ctx.fill();
      ctx.strokeStyle = 'rgba(0, 0, 0, 0.7)';
      ctx.lineWidth = 1;
      ctx.stroke();
      const w = ctx.measureText(c.name).width + 10;
      ctx.fillStyle = 'rgba(0, 0, 0, 0.6)';
      ctx.fillRect(sx + 12, sy + 12, w, 18);
      ctx.fillStyle = colour;
      ctx.fillText(c.name, sx + 17, sy + 21);
      ctx.globalAlpha = 1;
    }
    return alive;
  }
}

function roundRect(ctx: Ctx, x: number, y: number, w: number, h: number, r: number): void {
  ctx.beginPath();
  if (typeof ctx.roundRect === 'function') ctx.roundRect(x, y, w, h, Math.min(r, w / 2, h / 2));
  else ctx.rect(x, y, w, h);
}
