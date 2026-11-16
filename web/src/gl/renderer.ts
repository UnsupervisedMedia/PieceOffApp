import type { RoomSnapshot } from '../protocol';
import { Camera } from './camera';
import type { Mesh } from './mesh';
import {
  FLAG_BACKDROP,
  FLAG_HELD,
  FLAG_PLACED,
  FRAG,
  STATE_PER_ROW,
  VERT,
} from './shaders';

export interface BoardMedia {
  video: HTMLVideoElement;
  poster: HTMLImageElement;
}

export interface FrameInfo {
  /** rAF timestamp of this frame. */
  time: number;
  /** JS time spent in this frame's upload + draw calls (ms). */
  cpuMs: number;
  /** A new video frame was uploaded this frame. */
  videoUploaded: boolean;
}

/** Device pixel ratio cap. */
export const MAX_DPR = 2;
/** Pieces smaller than this on screen (CSS px) get no outline stroke (desktop: 7 points). */
const MIN_OUTLINE_PX = 7;

/** UV rect that makes media of the given size cover the board (centre crop, no stretch). */
export function coverUv(boardW: number, boardH: number, mediaW: number, mediaH: number): [number, number, number, number] {
  if (boardW <= 0 || boardH <= 0 || mediaW <= 0 || mediaH <= 0) return [0, 0, 1, 1];
  const b = boardW / boardH;
  const m = mediaW / mediaH;
  if (Math.abs(b - m) < 1e-4) return [0, 0, 1, 1];
  if (m > b) {
    const inset = (1 - b / m) / 2;
    return [inset, 0, 1 - inset, 1];
  }
  const inset = (1 - m / b) / 2;
  return [0, inset, 1, 1 - inset];
}

type VideoWithRvfc = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: (now: number, meta: unknown) => void) => number;
  cancelVideoFrameCallback?: (handle: number) => void;
};

interface Gl {
  gl: WebGL2RenderingContext;
  program: WebGLProgram;
  vao: WebGLVertexArrayObject;
  buffers: WebGLBuffer[];
  fillEbo: WebGLBuffer;
  lineEbo: WebGLBuffer;
  fillCount: number;
  lineCount: number;
  state: WebGLTexture;
  media: WebGLTexture;
  mediaW: number;
  mediaH: number;
  u: Record<string, WebGLUniformLocation | null>;
}

/** WebGL2 board: one program, one batched mesh for every piece, per-piece state
 *  in a float texture (a drag re-uploads two texels). Draws on demand: after a
 *  state/camera change or when the video delivers a new frame. */
export class BoardRenderer {
  readonly camera: Camera;
  /** Called after each drawn frame (stats). */
  onFrame: ((f: FrameInfo) => void) | null = null;
  private g: Gl | null = null;
  private lost = false;
  private lostCbs: ((lost: boolean) => void)[] = [];
  private mesh: Mesh | null = null;
  private snapshot: RoomSnapshot | null = null;
  private count = 0;
  private index = new Map<number, number>();
  /** CPU copy of the state texture; the source of truth across context loss. */
  private state = new Float32Array(0);
  private ranks = new Float32Array(0);
  private layers = new Uint8Array(0);
  private rankSpan = 1;
  private nextRank = 0;
  private dirty = new Set<number>();
  private allDirty = true;
  private rafPending = false;
  private videoDirty = false;
  private posterDirty = false;
  private videoFrames = 0;
  private rvfcHandle = 0;
  private pollHandle = 0;
  private lastVideoTime = -1;
  private dpr = 1;
  private observer: ResizeObserver | null = null;
  private disposed = false;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly media: BoardMedia,
  ) {
    this.camera = new Camera(canvas.clientWidth || 1, canvas.clientHeight || 1);
    canvas.addEventListener('webglcontextlost', this.handleLost, false);
    canvas.addEventListener('webglcontextrestored', this.handleRestored, false);
    this.g = this.createGl();
    this.resize();
    if (typeof ResizeObserver !== 'undefined') {
      this.observer = new ResizeObserver(() => this.resize());
      this.observer.observe(canvas);
    }
    const poster = media.poster;
    if (poster.complete && poster.naturalWidth > 0) this.posterDirty = true;
    else poster.addEventListener('load', () => ((this.posterDirty = true), this.requestRender()));
    this.watchVideo();
  }

  /** The GL renderer string (unmasked when the browser allows it). */
  get glRenderer(): string {
    const gl = this.g?.gl;
    if (!gl) return 'none';
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    return String(gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
  }

  get devicePixelRatio(): number {
    return this.dpr;
  }

  get contextLost(): boolean {
    return this.lost;
  }

  /** Video frames uploaded since construction. */
  get videoFramesUploaded(): number {
    return this.videoFrames;
  }

  get gl(): WebGL2RenderingContext | null {
    return this.g?.gl ?? null;
  }

  /** Use a new mesh. Piece state is kept for ids present in both. */
  setPuzzle(mesh: Mesh, snapshot: RoomSnapshot): void {
    const old = this.index;
    const oldState = this.state;
    const oldRanks = this.ranks;
    const oldLayers = this.layers;
    this.mesh = mesh;
    this.snapshot = snapshot;
    this.count = mesh.ids.length;
    this.index = new Map(Array.from(mesh.ids, (id, i) => [id, i]));
    // One extra slot: the board backdrop.
    const slots = this.count + 1;
    const rows = Math.ceil(slots / STATE_PER_ROW);
    this.state = new Float32Array(rows * STATE_PER_ROW * 8);
    this.ranks = new Float32Array(this.count);
    this.layers = new Uint8Array(this.count);
    this.rankSpan = Math.max(64, this.count * 8);
    const byId = new Map(snapshot.pieces.map((p) => [p.id, p]));
    let maxRank = 0;
    for (let i = 0; i < this.count; i++) {
      const id = mesh.ids[i];
      const prev = old.get(id);
      if (prev !== undefined && oldState.length) {
        this.state.set(oldState.subarray(prev * 8, prev * 8 + 8), i * 8);
        this.ranks[i] = oldRanks[prev];
        this.layers[i] = oldLayers[prev];
      } else {
        const p = byId.get(id);
        this.ranks[i] = i;
        const flags = p?.placed ? FLAG_PLACED : 0;
        this.layers[i] = p?.placed ? 0 : 1;
        this.writeState(i, p?.x ?? 0, p?.y ?? 0, 0, flags);
      }
      maxRank = Math.max(maxRank, this.ranks[i]);
    }
    this.nextRank = maxRank + 1;
    if (this.nextRank >= this.rankSpan) this.compactRanks();
    for (let i = 0; i < this.count; i++) this.writeDepth(i);
    // Backdrop: board rectangle at the very back.
    const b = this.count * 8;
    this.state[b + 3] = FLAG_BACKDROP;
    this.state[b + 4] = 0;
    this.allDirty = true;
    if (this.g) this.uploadMesh(this.g);
    this.requestRender();
  }

  /** Move/rotate/flag one piece. Picking a piece up (HELD) raises it to the top. */
  setPiece(id: number, x: number, y: number, rotation: number, flags: number): void {
    const i = this.index.get(id);
    if (i === undefined) return;
    const wasHeld = (this.state[i * 8 + 3] & FLAG_HELD) !== 0;
    this.writeState(i, x, y, ((rotation % 4) + 4) % 4, flags);
    const layer = flags & FLAG_HELD ? 2 : flags & FLAG_PLACED ? 0 : 1;
    if (flags & FLAG_HELD && !wasHeld) {
      if (this.nextRank >= this.rankSpan) this.compactRanks();
      this.ranks[i] = this.nextRank++;
    }
    this.layers[i] = layer;
    this.writeDepth(i);
    this.dirty.add(i);
    this.requestRender();
  }

  /** Stacking order of a piece (higher = drawn on top), for hit-testing. */
  pieceDepth(id: number): number {
    const i = this.index.get(id);
    return i === undefined ? -1 : this.state[i * 8 + 4];
  }

  requestRender(): void {
    if (this.rafPending || this.disposed) return;
    this.rafPending = true;
    requestAnimationFrame(this.frame);
  }

  /** `cb(true)` on context loss (poster shown, board state kept), `cb(false)` once restored. */
  onContextLost(cb: (lost: boolean) => void): void {
    this.lostCbs.push(cb);
  }

  /** Re-read the canvas size (CSS px) and DPR. */
  resize(): void {
    const w = Math.max(1, this.canvas.clientWidth);
    const h = Math.max(1, this.canvas.clientHeight);
    this.dpr = Math.min(MAX_DPR, globalThis.devicePixelRatio || 1);
    const pw = Math.round(w * this.dpr);
    const ph = Math.round(h * this.dpr);
    if (this.canvas.width !== pw) this.canvas.width = pw;
    if (this.canvas.height !== ph) this.canvas.height = ph;
    if (this.camera.viewW !== w || this.camera.viewH !== h) this.camera.resize(w, h);
    this.requestRender();
  }

  dispose(): void {
    this.disposed = true;
    this.observer?.disconnect();
    const v = this.media.video as VideoWithRvfc;
    if (this.rvfcHandle && v.cancelVideoFrameCallback) v.cancelVideoFrameCallback(this.rvfcHandle);
    if (this.pollHandle) cancelAnimationFrame(this.pollHandle);
    this.canvas.removeEventListener('webglcontextlost', this.handleLost);
    this.canvas.removeEventListener('webglcontextrestored', this.handleRestored);
  }

  // ---- internals -------------------------------------------------------------

  private writeState(i: number, x: number, y: number, rot: number, flags: number): void {
    const o = i * 8;
    this.state[o] = x;
    this.state[o + 1] = y;
    this.state[o + 2] = rot;
    this.state[o + 3] = flags;
  }

  private writeDepth(i: number): void {
    this.state[i * 8 + 4] = (this.layers[i] + (this.ranks[i] + 1) / (this.rankSpan + 1)) / 3;
  }

  /** Re-rank 0..n-1 in current order; rare (after ~7n pick-ups). */
  private compactRanks(): void {
    const order = Array.from({ length: this.count }, (_, i) => i).sort((a, b) => this.ranks[a] - this.ranks[b]);
    order.forEach((i, r) => (this.ranks[i] = r));
    this.nextRank = this.count;
    for (let i = 0; i < this.count; i++) this.writeDepth(i);
    this.allDirty = true;
  }

  private watchVideo(): void {
    const v = this.media.video as VideoWithRvfc;
    if (typeof v.requestVideoFrameCallback === 'function') {
      const onFrame = () => {
        if (this.disposed) return;
        this.videoDirty = true;
        this.requestRender();
        this.rvfcHandle = v.requestVideoFrameCallback!(onFrame);
      };
      this.rvfcHandle = v.requestVideoFrameCallback(onFrame);
    } else {
      // Fallback: poll for a new decoded frame once per display frame. Cheap; a
      // paused/stalled video just stops changing currentTime.
      const poll = () => {
        if (this.disposed) return;
        if (v.readyState >= 2 && v.currentTime !== this.lastVideoTime) {
          this.lastVideoTime = v.currentTime;
          this.videoDirty = true;
          this.requestRender();
        }
        this.pollHandle = requestAnimationFrame(poll);
      };
      this.pollHandle = requestAnimationFrame(poll);
    }
  }

  private readonly frame = (time: number): void => {
    this.rafPending = false;
    const g = this.g;
    if (!g || this.lost || !this.mesh || !this.snapshot) return;
    const t0 = performance.now();
    const gl = g.gl;
    const uploaded = this.uploadMedia(g);
    this.uploadState(g);

    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0.16, 0.17, 0.2, 1);
    gl.clearDepth(1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.useProgram(g.program);
    gl.bindVertexArray(g.vao);
    const cam = this.camera;
    gl.uniform3f(g.u.view, cam.scale, cam.ox, cam.oy);
    gl.uniform2f(g.u.viewSize, cam.viewW, cam.viewH);
    const s = this.snapshot;
    const cw = s.width / s.columns;
    const ch = s.height / s.rows;
    gl.uniform2f(g.u.cell, cw, ch);
    const [u0, v0, u1, v1] = coverUv(s.width, s.height, g.mediaW, g.mediaH);
    gl.uniform4f(g.u.crop, u0, v0, u1, v1);
    gl.uniform1i(g.u.hasMedia, g.mediaW > 0 ? 1 : 0);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, g.state);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, g.media);

    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.depthMask(true);
    gl.disable(gl.BLEND);
    gl.uniform1i(g.u.pass, 0);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.fillEbo);
    gl.drawElements(gl.TRIANGLES, g.fillCount, gl.UNSIGNED_INT, 0);

    if (Math.min(cw, ch) * cam.scale >= MIN_OUTLINE_PX) {
      gl.depthMask(false);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      gl.uniform1i(g.u.pass, 1);
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.lineEbo);
      gl.drawElements(gl.LINES, g.lineCount, gl.UNSIGNED_INT, 0);
    }
    gl.bindVertexArray(null);
    this.onFrame?.({ time, cpuMs: performance.now() - t0, videoUploaded: uploaded });
  };

  private uploadMedia(g: Gl): boolean {
    const gl = g.gl;
    const v = this.media.video;
    let source: TexImageSource | null = null;
    let w = 0;
    let h = 0;
    let isVideo = false;
    // HAVE_CURRENT_DATA: a decoded frame exists. Otherwise keep what is in the
    // texture (last frame or poster).
    if (this.videoDirty && v.readyState >= 2 && v.videoWidth > 0) {
      source = v;
      w = v.videoWidth;
      h = v.videoHeight;
      isVideo = true;
    } else if (this.posterDirty && g.mediaW === 0 && this.media.poster.naturalWidth > 0) {
      source = this.media.poster;
      w = this.media.poster.naturalWidth;
      h = this.media.poster.naturalHeight;
    }
    this.videoDirty = false;
    this.posterDirty = false;
    if (!source) return false;
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, g.media);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    if (w === g.mediaW && h === g.mediaH) {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, source);
    } else {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
      g.mediaW = w;
      g.mediaH = h;
    }
    if (isVideo) this.videoFrames++;
    return isVideo;
  }

  private uploadState(g: Gl): void {
    const gl = g.gl;
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, g.state);
    if (this.allDirty || this.dirty.size > 64) {
      const rows = this.state.length / (STATE_PER_ROW * 8);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, STATE_PER_ROW * 2, rows, gl.RGBA, gl.FLOAT, this.state);
    } else {
      for (const i of this.dirty) {
        const x = (i % STATE_PER_ROW) * 2;
        const y = Math.floor(i / STATE_PER_ROW);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, x, y, 2, 1, gl.RGBA, gl.FLOAT, this.state, i * 8);
      }
    }
    this.dirty.clear();
    this.allDirty = false;
  }

  private createGl(): Gl | null {
    const gl = this.canvas.getContext('webgl2', {
      antialias: true,
      alpha: false,
      depth: true,
      premultipliedAlpha: false,
      preserveDrawingBuffer: false,
      powerPreference: 'default',
    });
    if (!gl || gl.isContextLost()) return null;
    const program = link(gl, VERT, FRAG);
    const u: Gl['u'] = {};
    for (const name of ['state', 'view', 'viewSize', 'cell', 'crop', 'pass', 'media', 'hasMedia']) {
      u[name] = gl.getUniformLocation(program, 'u_' + name);
    }
    gl.useProgram(program);
    gl.uniform1i(u.state, 0);
    gl.uniform1i(u.media, 1);
    const state = gl.createTexture()!;
    const media = gl.createTexture()!;
    for (const tex of [state, media]) {
      gl.bindTexture(gl.TEXTURE_2D, tex);
      const filter = tex === state ? gl.NEAREST : gl.LINEAR;
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    }
    const g: Gl = {
      gl,
      program,
      vao: gl.createVertexArray()!,
      buffers: [gl.createBuffer()!, gl.createBuffer()!, gl.createBuffer()!],
      fillEbo: gl.createBuffer()!,
      lineEbo: gl.createBuffer()!,
      fillCount: 0,
      lineCount: 0,
      state,
      media,
      mediaW: 0,
      mediaH: 0,
      u,
    };
    if (this.mesh) this.uploadMesh(g);
    return g;
  }

  /** Mesh + backdrop quad into the vertex buffers; allocates the state texture. */
  private uploadMesh(g: Gl): void {
    const gl = g.gl;
    const m = this.mesh!;
    const s = this.snapshot!;
    const n = m.pieceOfVertex.length;
    // Backdrop quad: slot `count`, position (0, 0), no rotation, so its
    // piece-local corners are the board rectangle itself.
    const pos = new Float32Array(n * 2 + 8);
    pos.set(m.positions);
    pos.set([0, 0, s.width, 0, s.width, s.height, 0, s.height], n * 2);
    const uv = new Float32Array(n * 2 + 8);
    uv.set(m.uvs);
    const piece = new Uint32Array(n + 4);
    piece.set(m.pieceOfVertex);
    piece.fill(this.count, n);
    // Backdrop first so it never wins a depth tie.
    const fill = new Uint32Array(m.fillIndices.length + 6);
    fill.set([n, n + 1, n + 2, n, n + 2, n + 3]);
    fill.set(m.fillIndices, 6);

    gl.bindVertexArray(g.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, g.buffers[0]);
    gl.bufferData(gl.ARRAY_BUFFER, pos, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, g.buffers[1]);
    gl.bufferData(gl.ARRAY_BUFFER, uv, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, g.buffers[2]);
    gl.bufferData(gl.ARRAY_BUFFER, piece, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribIPointer(2, 1, gl.UNSIGNED_INT, 0, 0);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.fillEbo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, fill, gl.STATIC_DRAW);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, g.lineEbo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, m.lineIndices, gl.STATIC_DRAW);
    g.fillCount = fill.length;
    g.lineCount = m.lineIndices.length;

    const rows = this.state.length / (STATE_PER_ROW * 8);
    gl.bindTexture(gl.TEXTURE_2D, g.state);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, STATE_PER_ROW * 2, rows, 0, gl.RGBA, gl.FLOAT, this.state);
    this.allDirty = false;
    this.dirty.clear();
  }

  private readonly handleLost = (e: Event): void => {
    // Without preventDefault the browser never fires `webglcontextrestored`.
    e.preventDefault();
    this.lost = true;
    this.g = null;
    if (this.media.poster.isConnected) this.media.poster.hidden = false;
    for (const cb of this.lostCbs) cb(true);
  };

  private readonly handleRestored = (): void => {
    this.g = this.createGl();
    if (!this.g) return;
    this.lost = false;
    // Media texture is empty again: poster now, next video frame when it comes.
    this.posterDirty = true;
    this.videoDirty = true;
    if (this.media.poster.isConnected) this.media.poster.hidden = true;
    for (const cb of this.lostCbs) cb(false);
    this.requestRender();
  };
}

function link(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const p = gl.createProgram()!;
  for (const [type, src] of [
    [gl.VERTEX_SHADER, vs],
    [gl.FRAGMENT_SHADER, fs],
  ] as const) {
    const sh = gl.createShader(type)!;
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS) && !gl.isContextLost()) {
      throw new Error('shader: ' + gl.getShaderInfoLog(sh));
    }
    gl.attachShader(p, sh);
  }
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS) && !gl.isContextLost()) {
    throw new Error('link: ' + gl.getProgramInfoLog(p));
  }
  return p;
}
