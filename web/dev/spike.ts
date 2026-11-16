// Renderer spike: a fixed local puzzle on the WebGL2 board renderer, with a
// frame-time readout. Not part of play.js. Query:
//   ?pieces=400|1000  &cut=jigsaw|hexagon|triangle  &art=0..7  &cap=auto|<max samples>  &bench=1|0
import init, { builtin_media_json, piece_outlines, protocol_version } from '../wasm/pieceoff_web_wasm.js';
import { builtinAssets } from '../src/media/builtin';
import type { CutStyle, RoomSnapshot } from '../src/protocol';
import { buildMesh, strideFor, type Mesh } from '../src/gl/mesh';
import { BoardRenderer, type FrameInfo } from '../src/gl/renderer';
import { FLAG_HELD, FLAG_PLACED, FLAG_SELECTED } from '../src/gl/shaders';
import { devSnapshot, tableBounds } from './fixture';

const q = new URLSearchParams(location.search);
const PIECES = Math.max(4, Math.min(2000, Number(q.get('pieces') ?? 400) || 400));
const CUT = (['jigsaw', 'hexagon', 'triangle'].includes(q.get('cut') ?? '') ? q.get('cut') : 'jigsaw') as CutStyle;
const ART = Math.max(0, Math.min(7, Number(q.get('art') ?? 0) || 0));
const CAP_PARAM = q.get('cap') ?? 'auto';
const CAP = CAP_PARAM === 'auto' ? null : Math.max(3, Number(CAP_PARAM) || 1e9);
let bench = q.get('bench') !== '0';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>('board');
const poster = $<HTMLImageElement>('poster');
const video = $<HTMLVideoElement>('video');
const hud = $<HTMLDivElement>('hud');
const lostMsg = $<HTMLDivElement>('lostmsg');

interface Stats {
  frames: number;
  avgMs: number;
  p95Ms: number;
  pctOver20: number;
  jsMs: number;
  rendersPerSec: number;
  videoFps: number;
  glRenderer: string;
  dpr: number;
  pieces: number;
  cut: CutStyle;
  cap: string;
  stride: number;
  vertices: number;
  triangles: number;
  meshBuildMs: number;
  contextLost: boolean;
  videoPaused: boolean;
  videoReadyState: number;
  bench: boolean;
  errors: string[];
}

const stats: Stats = {
  frames: 0, avgMs: 0, p95Ms: 0, pctOver20: 0, jsMs: 0, rendersPerSec: 0, videoFps: 0,
  glRenderer: '', dpr: 1, pieces: 0, cut: CUT, cap: CAP_PARAM, stride: 1, vertices: 0, triangles: 0,
  meshBuildMs: 0, contextLost: false, videoPaused: true, videoReadyState: 0, bench, errors: [],
};
addEventListener('error', (e) => stats.errors.push(String(e.message)));
addEventListener('unhandledrejection', (e) => stats.errors.push(String(e.reason)));

async function main(): Promise<void> {
  await init({ module_or_path: new URL('./pieceoff_web_wasm_bg.wasm', import.meta.url) });
  const list = JSON.parse(builtin_media_json()) as { sha256: string; title: string }[];
  const sha = list[ART].sha256;
  const assets = builtinAssets(sha, '../builtin/');
  if (!assets) throw new Error('no built-in asset for ' + sha);

  poster.src = assets.poster;
  await poster.decode().catch(() => undefined);
  const width = poster.naturalWidth || 480;
  const height = poster.naturalHeight || 320;
  video.src = assets.mp4;
  video.muted = true;
  video.play().catch(() => undefined);

  const [rows, cols] = grid(PIECES, width / height);
  const snap: RoomSnapshot = devSnapshot({ rows, cols, sha, cut: CUT, width, height, protocolVersion: protocol_version() });
  const out = piece_outlines(JSON.stringify(snap));
  // Full-detail outlines stay on the CPU for picking.
  const outline = { ids: out.ids, offsets: out.offsets, points: out.points };
  out.free();
  const n = snap.pieces.length;
  const cw = width / cols;
  const ch = height / rows;
  const samples0 = (outline.offsets[1] - outline.offsets[0]) / 2;

  const px = new Float32Array(n);
  const py = new Float32Array(n);
  const rot = new Uint8Array(n);
  const flags = new Uint8Array(n);
  snap.pieces.forEach((p, i) => ((px[i] = p.x), (py[i] = p.y)));

  const renderer = new BoardRenderer(canvas, { video, poster });
  stats.glRenderer = renderer.glRenderer;
  const cam = renderer.camera;
  const bounds = tableBounds(snap);
  const fit = () => {
    cam.fit(bounds.w, bounds.h, bounds.minX, bounds.minY);
    renderer.requestRender();
  };
  fit();

  let mesh: Mesh | null = null;
  const wantedSamples = () => CAP ?? (Math.max(cw, ch) * cam.scale * 4) / 2.2;
  const rebuild = () => {
    const t0 = performance.now();
    mesh = buildMesh(outline, snap, { maxSamples: wantedSamples() });
    stats.meshBuildMs = performance.now() - t0;
    stats.stride = mesh.strides[0];
    stats.vertices = mesh.pieceOfVertex.length;
    stats.triangles = mesh.fillIndices.length / 3;
    renderer.setPuzzle(mesh, snap);
    for (let i = 0; i < n; i++) renderer.setPiece(snap.pieces[i].id, px[i], py[i], rot[i], flags[i]);
  };
  rebuild();
  let lodTimer = 0;
  const maybeRebuild = () => {
    if (CAP !== null || !mesh) return;
    if (strideFor(samples0, wantedSamples()) === mesh.strides[0]) return;
    clearTimeout(lodTimer);
    lodTimer = window.setTimeout(rebuild, 150);
  };

  renderer.onContextLost((lost) => {
    lostMsg.hidden = !lost;
    stats.contextLost = lost;
  });

  // ---- stats ----
  const intervals: number[] = [];
  const jsTimes: number[] = [];
  const renderTimes: number[] = [];
  const videoTimes: number[] = [];
  let lastFrame = 0;
  renderer.onFrame = (f: FrameInfo) => {
    stats.frames++;
    if (lastFrame) {
      intervals.push(f.time - lastFrame);
      if (intervals.length > 120) intervals.shift();
    }
    lastFrame = f.time;
    jsTimes.push(f.cpuMs);
    if (jsTimes.length > 120) jsTimes.shift();
    renderTimes.push(f.time);
    if (f.videoUploaded) videoTimes.push(f.time);
    if (bench) renderer.requestRender();
  };
  const tick = () => {
    const now = performance.now();
    while (renderTimes.length && now - renderTimes[0] > 1000) renderTimes.shift();
    while (videoTimes.length && now - videoTimes[0] > 1000) videoTimes.shift();
    if (now - lastFrame > 250) lastFrame = 0; // idle gap: not a frame time
    const sorted = [...intervals].sort((a, b) => a - b);
    stats.avgMs = sorted.length ? sorted.reduce((a, b) => a + b, 0) / sorted.length : 0;
    stats.p95Ms = sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] : 0;
    stats.pctOver20 = sorted.length ? (100 * sorted.filter((x) => x > 20).length) / sorted.length : 0;
    stats.jsMs = jsTimes.length ? jsTimes.reduce((a, b) => a + b, 0) / jsTimes.length : 0;
    stats.rendersPerSec = renderTimes.length;
    stats.videoFps = videoTimes.length;
    stats.dpr = renderer.devicePixelRatio;
    stats.pieces = n;
    stats.videoPaused = video.paused;
    stats.videoReadyState = video.readyState;
    stats.bench = bench;
    hud.textContent =
      `pieces ${n} (${rows}x${cols} ${CUT})  art ${ART}  cap ${CAP_PARAM} stride ${stats.stride}\n` +
      `frame avg ${stats.avgMs.toFixed(1)} ms  p95 ${stats.p95Ms.toFixed(1)} ms  >20ms ${stats.pctOver20.toFixed(0)}%  (last ${intervals.length})\n` +
      `JS/frame ${stats.jsMs.toFixed(2)} ms  renders/s ${stats.rendersPerSec}  video fps ${stats.videoFps}${video.paused ? ' (paused)' : ''}\n` +
      `verts ${stats.vertices}  tris ${stats.triangles}  mesh build ${stats.meshBuildMs.toFixed(1)} ms\n` +
      `DPR ${stats.dpr}  ${canvas.width}x${canvas.height}  ${bench ? 'bench (continuous)' : 'on-demand'}${stats.contextLost ? '  CONTEXT LOST' : ''}\n` +
      `GL ${stats.glRenderer}`;
    setTimeout(tick, 250);
  };
  tick();

  // ---- input (simple: the full gesture machine is Task 6) ----
  const pointers = new Map<number, { x: number; y: number }>();
  let drag: { i: number; dx: number; dy: number; pointer: number } | null = null;
  let pinch: { dist: number; cx: number; cy: number } | null = null;
  let lastTouched = -1;

  const pick = (sx: number, sy: number): number => {
    const [tx, ty] = cam.toTable(sx, sy);
    let best = -1;
    let bestDepth = -Infinity;
    const hx = cw / 2;
    const hy = ch / 2;
    const pts = outline.points;
    for (let i = 0; i < n; i++) {
      // Into piece-local space: undo position, then the quarter turn about the cell centre.
      let lx = tx - px[i] - hx;
      let ly = ty - py[i] - hy;
      for (let r = 0; r < rot[i]; r++) [lx, ly] = [ly, -lx];
      lx += hx;
      ly += hy;
      if (lx < -cw || ly < -ch || lx > 2 * cw || ly > 2 * ch) continue;
      let inside = false;
      const a = outline.offsets[i];
      const b = outline.offsets[i + 1];
      for (let k = a, j = b - 2; k < b; j = k, k += 2) {
        const xi = pts[k], yi = pts[k + 1], xj = pts[j], yj = pts[j + 1];
        if (yi > ly !== yj > ly && lx < ((xj - xi) * (ly - yi)) / (yj - yi) + xi) inside = !inside;
      }
      if (!inside) continue;
      const d = renderer.pieceDepth(snap.pieces[i].id);
      if (d > bestDepth) ((bestDepth = d), (best = i));
    }
    return best;
  };
  const push = (i: number) => renderer.setPiece(snap.pieces[i].id, px[i], py[i], rot[i], flags[i]);

  canvas.addEventListener('pointerdown', (e) => {
    if (video.paused && !userPaused) video.play().catch(() => undefined);
    canvas.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 1) {
      const i = pick(e.clientX, e.clientY);
      if (i >= 0) {
        const [tx, ty] = cam.toTable(e.clientX, e.clientY);
        drag = { i, dx: tx - px[i], dy: ty - py[i], pointer: e.pointerId };
        if (lastTouched >= 0 && lastTouched !== i) ((flags[lastTouched] &= ~FLAG_SELECTED), push(lastTouched));
        lastTouched = i;
        flags[i] = (flags[i] | FLAG_HELD | FLAG_SELECTED) & ~FLAG_PLACED;
        push(i);
      }
    } else if (pointers.size === 2) {
      if (drag) endDrag();
      const [p1, p2] = [...pointers.values()];
      pinch = { dist: Math.hypot(p1.x - p2.x, p1.y - p2.y), cx: (p1.x + p2.x) / 2, cy: (p1.y + p2.y) / 2 };
    }
  });
  canvas.addEventListener('pointermove', (e) => {
    const prev = pointers.get(e.pointerId);
    if (!prev) return;
    const cur = { x: e.clientX, y: e.clientY };
    pointers.set(e.pointerId, cur);
    if (pinch && pointers.size === 2) {
      const [p1, p2] = [...pointers.values()];
      const dist = Math.hypot(p1.x - p2.x, p1.y - p2.y);
      const cx = (p1.x + p2.x) / 2;
      const cy = (p1.y + p2.y) / 2;
      cam.panBy(cx - pinch.cx, cy - pinch.cy);
      if (pinch.dist > 0) cam.zoomAt(cx, cy, dist / pinch.dist);
      pinch = { dist, cx, cy };
      maybeRebuild();
      renderer.requestRender();
    } else if (drag && drag.pointer === e.pointerId) {
      const [tx, ty] = cam.toTable(cur.x, cur.y);
      px[drag.i] = tx - drag.dx;
      py[drag.i] = ty - drag.dy;
      push(drag.i);
    } else if (pointers.size === 1) {
      cam.panBy(cur.x - prev.x, cur.y - prev.y);
      renderer.requestRender();
    }
  });
  const endDrag = () => {
    if (!drag) return;
    const i = drag.i;
    const p = snap.pieces[i];
    flags[i] &= ~FLAG_HELD;
    if (rot[i] === 0 && Math.abs(px[i] - p.column * cw) < cw * 0.3 && Math.abs(py[i] - p.row * ch) < ch * 0.3) {
      px[i] = p.column * cw;
      py[i] = p.row * ch;
      flags[i] = FLAG_PLACED;
    }
    push(i);
    drag = null;
  };
  const up = (e: PointerEvent) => {
    pointers.delete(e.pointerId);
    if (drag && drag.pointer === e.pointerId) endDrag();
    if (pointers.size < 2) pinch = null;
  };
  canvas.addEventListener('pointerup', up);
  canvas.addEventListener('pointercancel', up);
  canvas.addEventListener(
    'wheel',
    (e) => {
      e.preventDefault();
      cam.zoomAt(e.clientX, e.clientY, Math.exp(-e.deltaY * 0.0015));
      maybeRebuild();
      renderer.requestRender();
    },
    { passive: false },
  );

  const turn = () => {
    if (lastTouched < 0) return;
    rot[lastTouched] = (rot[lastTouched] + 1) % 4;
    flags[lastTouched] &= ~FLAG_PLACED;
    push(lastTouched);
  };
  let userPaused = false;
  const playBtn = $<HTMLButtonElement>('play');
  const setPaused = (paused: boolean) => {
    userPaused = paused;
    if (paused) video.pause();
    else video.play().catch(() => undefined);
    playBtn.textContent = paused ? 'play video' : 'pause video';
  };
  $<HTMLButtonElement>('turn').onclick = turn;
  playBtn.onclick = () => setPaused(!video.paused);
  const benchBtn = $<HTMLButtonElement>('bench');
  const setBench = (on: boolean) => {
    bench = on;
    benchBtn.textContent = `bench: ${on ? 'on' : 'off'}`;
    intervals.length = 0;
    renderer.requestRender();
  };
  benchBtn.onclick = () => setBench(!bench);
  setBench(bench);
  $<HTMLButtonElement>('fit').onclick = fit;
  addEventListener('keydown', (e) => {
    if (e.key === 'r') turn();
  });

  // Automation hooks (Playwright / console).
  let loseExt: WEBGL_lose_context | null = null;
  Object.assign(window, {
    __spike: Object.assign(stats, {
      loseContext() {
        loseExt = renderer.gl?.getExtension('WEBGL_lose_context') ?? loseExt;
        loseExt?.loseContext();
      },
      restoreContext() {
        loseExt?.restoreContext();
      },
      pauseVideo: () => setPaused(true),
      playVideo: () => setPaused(false),
      setBench,
      /** Screen position of piece `i`'s cell centre (CSS px). */
      pieceScreen(i: number): [number, number] {
        return cam.toScreen(px[i] + cw / 2, py[i] + ch / 2);
      },
      piecePos(i: number): [number, number, number, number] {
        return [px[i], py[i], rot[i], flags[i]];
      },
      pick,
    }),
  });
}

/** rows x cols == pieces exactly when it factors, with the squarest cells. */
function grid(pieces: number, aspect: number): [number, number] {
  let best: [number, number] = [2, 2];
  let score = Infinity;
  for (let r = 2; r <= pieces / 2; r++) {
    const c = Math.round(pieces / r);
    if (c < 2) break;
    const miss = Math.abs(r * c - pieces) / pieces;
    const sq = Math.abs(Math.log(aspect / c / (1 / r)));
    const sc = miss * 50 + sq;
    if (sc < score) ((score = sc), (best = [r, c]));
  }
  return best;
}

main().catch((e) => {
  stats.errors.push(String(e?.stack ?? e));
  hud.textContent = 'spike failed: ' + String(e?.message ?? e);
  Object.assign(window, { __spike: stats });
});
