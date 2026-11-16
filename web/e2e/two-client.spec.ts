// Two browser clients against the real rooms Worker (scripts/web-e2e.sh starts
// it and the page server, and sets E2E_SITE / E2E_API).
//
// - Desktop: A creates a 12-piece room through the create form, B opens A's
//   invite link, both list each other, B drags a piece home and it snaps (A
//   sees it held, moved and placed), chat both ways, B reloads and resumes as
//   the same name, host controls only on A, then 25 pick-up/drop rounds each
//   followed by "nobody holds a piece" on A's copy of the server state within
//   3 s, and last A kicks B (B gets the kicked screen and goes back to the lobby).
//   Interruptions really covered there: pointercancel (piece goes back), window
//   blur with the button still down (the lease must be gone before the mouse
//   comes up; piece goes back), reload mid-drag, socket cut mid-drag (Playwright
//   closes B's routed socket on both sides while offline). The second-finger
//   cancel is covered by the phone test. Not covered: a real OS-level focus
//   loss (headless has none; the blur event is synthetic), and a true transport
//   outage: Chromium's offline emulation leaves an open loopback WebSocket
//   working, so the 'network-flap-then-drop' round is a plain drop made while
//   the context is flagged offline (it only checks the flap does no harm).
// - Phone: B as an iPhone 13 (Chromium, touch) joins by typing the code:
//   touch drag, a second finger mid-drag (cancel: the piece goes back and is
//   let go), long-press select, pinch zoom.
//
// State is read through the board's read-only test probe
// (`window.__pieceoffTest`, config `testHooks: true`). Turnstile cannot run
// here: an init script installs a stand-in `window.turnstile` whose token the
// Worker's loopback verifier accepts (`good-create-room`); nothing is fetched
// from challenges.cloudflare.com. No fixed sleeps: every wait polls for state.
import { expect, test, devices, type Browser, type BrowserContext, type CDPSession, type Page, type WebSocketRoute } from '@playwright/test';

declare const process: { env: Record<string, string | undefined> };
const SITE = process.env.E2E_SITE ?? '';
const API = process.env.E2E_API ?? '';

interface Piece {
  id: number;
  group_id: number;
  row: number;
  column: number;
  x: number;
  y: number;
  placed: boolean;
  held_by: string | null;
}
interface Snap {
  invite_code: string;
  width: number;
  height: number;
  rows: number;
  columns: number;
  host_id: string;
  revision: number;
  pieces: Piece[];
}
type Pt = { x: number; y: number };

test.beforeAll(() => {
  if (!SITE || !API) throw new Error('E2E_SITE and E2E_API are not set: run scripts/web-e2e.sh');
});

async function newClient(browser: Browser, opts: Parameters<Browser['newContext']>[0], tag: string): Promise<{ ctx: BrowserContext; page: Page; log: string[] }> {
  const ctx = await browser.newContext(opts);
  await ctx.addInitScript(
    ({ api, site }) => {
      (window as unknown as Record<string, unknown>).PIECEOFF_WEB_CONFIG = { api, site, assetBase: '/play-app/', turnstileSiteKey: 'e2e-site-key', testHooks: true };
      // Turnstile stand-in: hands out the token the loopback verifier accepts for the widget's action.
      (window as unknown as Record<string, unknown>).turnstile = {
        render(_el: unknown, o: { action: string; callback: (t: string) => void }) {
          setTimeout(() => o.callback(`good-${o.action.replace(/_/g, '-')}`), 0);
          return 'e2e-widget';
        },
        reset() {},
        remove() {},
      };
    },
    { api: API, site: SITE },
  );
  await ctx.route('https://challenges.cloudflare.com/**', (r) => r.abort());
  const page = await ctx.newPage();
  const log: string[] = [];
  page.on('console', (m) => {
    if (m.type() === 'error' || m.type() === 'warning') log.push(`${tag} ${m.type()}: ${m.text()}`);
  });
  page.on('pageerror', (e) => log.push(`${tag} pageerror: ${e.message}`));
  page.on('websocket', (ws) => {
    ws.on('framereceived', (f) => {
      const s = typeof f.payload === 'string' ? f.payload : '';
      if (s.includes('"type":"error"')) log.push(`${tag} server error: ${s.slice(0, 300)}`);
    });
    ws.on('close', () => log.push(`${tag} socket closed`));
    if (process.env.E2E_DEBUG) ws.on('framesent', (f) => log.push(`${tag} sent ${String(f.payload).slice(0, 120)}`));
  });
  return { ctx, page, log };
}

const snap = (p: Page): Promise<Snap | null> =>
  p.evaluate(() => ((window as unknown as { __pieceoffTest?: { snapshot(): unknown } }).__pieceoffTest?.snapshot() as Snap | null) ?? null);
const clientId = (p: Page): Promise<string | null> =>
  p.evaluate(() => (window as unknown as { __pieceoffTest?: { clientId(): string | null } }).__pieceoffTest?.clientId() ?? null);
const toPage = (p: Page, t: Pt): Promise<Pt> =>
  p.evaluate(([x, y]) => {
    const [px, py] = (window as unknown as { __pieceoffTest: { toPage(x: number, y: number): [number, number] } }).__pieceoffTest.toPage(x, y);
    return { x: px, y: py };
  }, [t.x, t.y] as const);
const cameraScale = (p: Page): Promise<number> =>
  p.evaluate(() => (window as unknown as { __pieceoffTest: { camera(): { scale: number } } }).__pieceoffTest.camera().scale);
const selection = (p: Page): Promise<number[]> =>
  p.evaluate(() => (window as unknown as { __pieceoffTest: { selection(): number[] } }).__pieceoffTest.selection());

/** Wait until the board is up with a server snapshot and the client knows its id. */
async function boardReady(p: Page): Promise<Snap> {
  await expect(p.locator('canvas.po-board-gl')).toBeVisible({ timeout: 60_000 });
  await expect.poll(async () => (await snap(p)) !== null && (await clientId(p)) !== null, { timeout: 60_000 }).toBe(true);
  return (await snap(p))!;
}

const cell = (s: Snap) => ({ w: s.width / s.columns, h: s.height / s.rows });
const piece = (s: Snap, id: number) => s.pieces.find((q) => q.id === id)!;
const held = (s: Snap | null) => (s?.pieces ?? []).filter((q) => q.held_by !== null).map((q) => `${q.id}:${q.held_by}`);

/** A loose piece whose centre is clear of every other piece's cell, if there is one. */
function freePiece(s: Snap): Piece {
  const { w, h } = cell(s);
  const loose = s.pieces.filter((q) => !q.placed && q.held_by === null);
  const clear = (q: Piece) => s.pieces.every((o) => o.id === q.id || Math.abs(o.x - q.x) > w * 1.45 || Math.abs(o.y - q.y) > h * 1.45);
  const best = loose.find(clear) ?? loose[0];
  if (!best) throw new Error('no loose piece left');
  return best;
}
const centre = (q: Piece, s: Snap): Pt => ({ x: q.x + cell(s).w / 2, y: q.y + cell(s).h / 2 });

/** Nobody holds a piece in A's copy of the server state, within 3 s. */
async function noLease(a: Page, what: string): Promise<void> {
  const t0 = Date.now();
  try {
    await expect.poll(async () => held(await snap(a)), { timeout: 3_000 }).toEqual([]);
  } catch {
    // Say how long it really stays held (up to 100 s, past the Worker's 90 s idle-socket cut).
    const still = held(await snap(a));
    let released = -1;
    try {
      await expect.poll(async () => held(await snap(a)), { timeout: 100_000, intervals: [250] }).toEqual([]);
      released = Date.now() - t0;
    } catch {
      // still held
    }
    throw new Error(`a piece stayed held for more than 3 s after: ${what} (${still.join(', ')}); ` +
      (released >= 0 ? `released after ${released} ms` : 'still held after 100 s'));
  }
}

/** The piece `by` holds in A's copy of the server state, once there is one. */
async function heldPiece(a: Page, by: string, what = ''): Promise<number> {
  let id = -1;
  await expect
    .poll(
      async () => {
        const p = (await snap(a))!.pieces.find((q) => q.held_by === by);
        id = p?.id ?? -1;
        return id >= 0;
      },
      { timeout: 15_000, message: `A never saw ${by} hold a piece ${what}` },
    )
    .toBe(true);
  return id;
}

/** The camera has stopped changing: the same mapping across two animation frames
 *  (a panel opening or closing resizes the stage, and the view refits a frame later). */
async function cameraSettled(p: Page): Promise<void> {
  await expect
    .poll(() =>
      p.evaluate(
        () =>
          new Promise<boolean>((done) => {
            const probe = (window as unknown as { __pieceoffTest: { camera(): { scale: number; ox: number; oy: number } } }).__pieceoffTest;
            const before = JSON.stringify(probe.camera());
            requestAnimationFrame(() => requestAnimationFrame(() => done(JSON.stringify(probe.camera()) === before)));
          }),
      ),
    )
    .toBe(true);
}

/** Mouse press at piece `q`'s centre (B's view) and a small move, until A sees B
 *  hold a piece (the one on top there); returns the press point and its id. */
async function mouseGrab(b: Page, a: Page, s: Snap, q: Piece, bId: string, what = ''): Promise<{ at: Pt; id: number }> {
  await cameraSettled(b);
  const at = await toPage(b, centre(q, s));
  await b.mouse.move(at.x, at.y);
  await b.mouse.down();
  await b.mouse.move(at.x + 12, at.y + 8, { steps: 4 });
  return { at, id: await heldPiece(a, bId, what) };
}

// ---- touch through CDP (Input.dispatchTouchEvent) --------------------------------

type Touch = { x: number; y: number; id: number };
async function touch(cdp: CDPSession, type: 'touchStart' | 'touchMove' | 'touchEnd' | 'touchCancel', points: Touch[]): Promise<void> {
  await cdp.send('Input.dispatchTouchEvent', {
    type,
    touchPoints: points.map((p) => ({ x: p.x, y: p.y, id: p.id, radiusX: 1, radiusY: 1, force: 1 })),
  });
}
async function touchPath(cdp: CDPSession, from: Pt, to: Pt, steps: number, id = 0): Promise<void> {
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    await touch(cdp, 'touchMove', [{ x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t, id }]);
  }
}

// ---- UI steps ----------------------------------------------------------------------

async function createRoom(a: Page, name: string): Promise<Snap> {
  await a.goto(`${SITE}/`);
  await a.getByRole('button', { name: 'Create room' }).click();
  await a.locator('label', { hasText: /^Easy · 12$/ }).click();
  await a.fill('form input[name=name]', name);
  const submit = a.locator('form button[type=submit]');
  await expect(submit).toBeEnabled();
  await submit.click();
  return boardReady(a);
}

async function openTab(p: Page, tab: 'people' | 'room' | 'chat' | 'holders'): Promise<void> {
  const tabEl = p.locator(`[role=tab][data-tab=${tab}]`);
  if (!(await tabEl.isVisible())) await p.click(tab === 'chat' ? '[data-open=chat]' : '[data-open=menu]');
  await tabEl.click();
  await expect(tabEl).toHaveAttribute('aria-selected', 'true');
}

async function closePanel(p: Page): Promise<void> {
  if (await p.locator('.po-panel').isVisible()) await p.keyboard.press('Escape');
  await expect(p.locator('.po-panel')).toBeHidden();
}

async function sendChat(p: Page, text: string): Promise<void> {
  await openTab(p, 'chat');
  await p.fill('.po-chat-input', text);
  await p.keyboard.press('Enter');
}

// ---- the tests ---------------------------------------------------------------------

test('desktop: join, drag and snap, chat, resume, host controls, leases, kick', async ({ browser }) => {
  const A = await newClient(browser, { viewport: { width: 1100, height: 720 } }, 'A');
  const B = await newClient(browser, { viewport: { width: 1000, height: 700 } }, 'B');
  const a = A.page;
  const b = B.page;
  // B's room sockets go through Playwright, so a round can cut one mid-drag the
  // way a dead network does (Chromium's offline emulation leaves an open
  // loopback WebSocket alone).
  const sockets: { page: WebSocketRoute; server: WebSocketRoute }[] = [];
  await B.ctx.routeWebSocket(/\/ws\?code=/, (ws) => {
    sockets.push({ page: ws, server: ws.connectToServer() });
  });
  const cutSockets = async () => {
    for (const s of sockets.splice(0)) {
      await s.server.close({ code: 1001, reason: 'e2e network cut' }).catch(() => {});
      await s.page.close({ code: 4001, reason: 'e2e network cut' }).catch(() => {});
    }
  };
  try {
    let code = '';
    await test.step('A creates a 12-piece room', async () => {
      const s0 = await createRoom(a, 'Ann');
      code = s0.invite_code;
      expect(code).toMatch(/^[A-Z0-9]{8}$/);
      expect(s0.pieces.length).toBe(12);
    });

    await test.step('B opens the invite link from A’s Room tab', async () => {
      await openTab(a, 'room');
      const inviteHref = (await a.locator('.po-invite-link').innerText()).trim();
      expect(inviteHref).toBe(`${SITE}/join?c=${code}`);
      await closePanel(a);
      await b.goto(inviteHref);
      await b.fill('form input[name=name]', 'Bo');
      await b.locator('form button[type=submit]').click();
      await boardReady(b);
    });
    const bId = (await clientId(b))!;

    await test.step('both see each other under People', async () => {
      await openTab(a, 'people');
      await expect(a.locator('.po-people li[data-name="Bo"]')).toBeVisible();
      await expect(a.locator('.po-people li[data-name="Ann"]')).toContainText('(you, host)');
      await openTab(b, 'people');
      await expect(b.locator('.po-people li[data-name="Ann"]')).toBeVisible();
      await expect(b.locator('.po-people li[data-name="Bo"]')).toContainText('(you)');
      await closePanel(a);
      await closePanel(b);
    });

    await test.step('B drags a piece home; A sees it held, moving, then placed', async () => {
      const s = (await snap(b))!;
      const pressed = freePiece(s);
      const { w, h } = cell(s);
      const press = centre(pressed, s);
      const grabbed = await mouseGrab(b, a, s, pressed, bId);
      const q = piece(s, grabbed.id);
      const home = { x: q.column * w, y: q.row * h };
      // Offset of the press from the held piece's corner, kept through the drag.
      const grab = { x: press.x - q.x, y: press.y - q.y };
      const mid = await toPage(b, { x: (q.x + home.x) / 2 + grab.x, y: (q.y + home.y) / 2 + grab.y });
      await b.mouse.move(mid.x, mid.y, { steps: 8 });
      // A sees it move while B still holds it.
      await expect
        .poll(async () => {
          const p = piece((await snap(a))!, q.id);
          return p.held_by === bId && Math.hypot(p.x - q.x, p.y - q.y) > 1;
        })
        .toBe(true);
      const target = await toPage(b, { x: home.x + grab.x, y: home.y + grab.y });
      await b.mouse.move(target.x, target.y, { steps: 8 });
      await b.mouse.up();
      await expect
        .poll(async () => {
          const p = piece((await snap(a))!, q.id);
          return { placed: p.placed, held: p.held_by, x: Math.round(p.x), y: Math.round(p.y) };
        })
        .toEqual({ placed: true, held: null, x: Math.round(home.x), y: Math.round(home.y) });
      // B's own copy of the server state agrees.
      await expect.poll(async () => piece((await snap(b))!, q.id).placed).toBe(true);
    });

    await test.step('chat both ways', async () => {
      await sendChat(b, 'hi Ann, taking the corners');
      await openTab(a, 'chat');
      await expect(a.locator('.po-chat-log .po-chat-line', { hasText: 'hi Ann, taking the corners' })).toContainText('Bo');
      await a.fill('.po-chat-input', 'go for it, Bo');
      await a.keyboard.press('Enter');
      await expect(b.locator('.po-chat-log .po-chat-line', { hasText: 'go for it, Bo' })).toContainText('Ann');
      await closePanel(a);
      await closePanel(b);
    });

    await test.step('B reloads and resumes as Bo (no name prompt, no second Bo)', async () => {
      await b.reload();
      await boardReady(b);
      await expect(b.locator('form input[name=name]')).toHaveCount(0);
      await openTab(b, 'people');
      await expect(b.locator('.po-people li[data-name="Bo"]')).toContainText('(you)');
      await openTab(a, 'people');
      await expect(a.locator('.po-people li[data-name="Bo"]')).toHaveCount(1);
      await expect(a.locator('.po-people li')).toHaveCount(2);
      await closePanel(a);
      await closePanel(b);
    });

    await test.step('host controls only on A', async () => {
      await openTab(a, 'room');
      await expect(a.locator('.po-host')).toBeVisible();
      await openTab(b, 'room');
      await expect(b.locator('.po-host')).toHaveCount(0);
      await closePanel(a);
      await closePanel(b);
    });

    await test.step('25 pick-ups and drops, some interrupted: no lease survives', async () => {
      const kinds = ['drop', 'pointercancel', 'blur', 'reload', 'drop', 'network-flap-then-drop', 'socket-cut'] as const;
      for (let i = 0; i < 25; i++) {
        const kind = kinds[i % kinds.length];
        const label = `round ${i + 1} (${kind})`;
        const id = (await clientId(b))!;
        const s = (await snap(b))!;
        const q = freePiece(s);
        const { at, id: grabbed } = await mouseGrab(b, a, s, q, id, `(${label})`);
        const origin = piece(s, grabbed);
        // Back and forth, so the pieces stay on the table.
        const sign = i % 2 === 0 ? 1 : -1;
        await b.mouse.move(at.x + sign * 40, at.y + sign * 25, { steps: 4 });
        switch (kind) {
          case 'drop':
            await b.mouse.up();
            break;
          case 'pointercancel':
            await b.evaluate(() => {
              const c = document.querySelector('canvas.po-board-gl')!;
              c.dispatchEvent(new PointerEvent('pointercancel', { pointerId: 1, pointerType: 'mouse', bubbles: true }));
            });
            await b.mouse.up();
            await noLease(a, label);
            // A cancel puts the piece down where it was picked up.
            await expect
              .poll(async () => {
                const p = piece((await snap(a))!, grabbed);
                return [Math.round(p.x), Math.round(p.y)];
              })
              .toEqual([Math.round(origin.x), Math.round(origin.y)]);
            break;
          case 'blur':
            // The window loses focus mid-drag (alt-tab) and the button-up may
            // never come: the lease must go with the blur alone, the piece back
            // where it was picked up.
            await b.evaluate(() => window.dispatchEvent(new FocusEvent('blur')));
            await noLease(a, `${label}, button still down`);
            await expect
              .poll(async () => {
                const p = piece((await snap(a))!, grabbed);
                return [Math.round(p.x), Math.round(p.y)];
              })
              .toEqual([Math.round(origin.x), Math.round(origin.y)]);
            await b.mouse.up(); // cleanup only
            break;
          case 'reload':
            await b.reload();
            await b.mouse.up();
            await boardReady(b);
            break;
          case 'network-flap-then-drop':
            // The context is flagged offline mid-drag and the drop happens then.
            // Chromium keeps the open loopback socket working, so this is a drop
            // during a flap; it must not disturb the connection.
            await B.ctx.setOffline(true);
            await b.mouse.move(at.x + sign * 70, at.y + sign * 40, { steps: 3 });
            await b.mouse.up();
            await B.ctx.setOffline(false);
            await expect(b.locator('.po-board-net')).toHaveText('');
            break;
          case 'socket-cut':
            // The network goes and the socket dies mid-drag; the drop happens offline.
            await B.ctx.setOffline(true);
            await cutSockets();
            await expect(b.locator('.po-board-net')).toHaveText(/Reconnecting/);
            await b.mouse.move(at.x + sign * 70, at.y + sign * 40, { steps: 3 });
            await b.mouse.up();
            // Whether the server let go while B is still offline depends on it
            // hearing the close (a silent network loss is only noticed by the
            // Worker's 90 s idle-socket cut): recorded, not asserted.
            {
              const t0 = Date.now();
              let ms = -1;
              await expect
                .poll(async () => held(await snap(a)).length === 0, { timeout: 3_000 })
                .toBe(true)
                .then(() => (ms = Date.now() - t0), () => {});
              const note = `${label}: ${ms >= 0 ? `released ${ms} ms after the cut, B still offline` : 'still held 3 s after the cut while B was offline'}`;
              test.info().annotations.push({ type: 'socket-cut', description: note });
              console.log(`[socket-cut] ${note}`);
            }
            await B.ctx.setOffline(false);
            // Back online, B reconnects as itself; that must free the piece.
            await expect
              .poll(async () => (await b.locator('.po-board-net').innerText()) === '', { timeout: 30_000 })
              .toBe(true);
            break;
        }
        await noLease(a, label);
        if (kind === 'socket-cut' || kind === 'reload') {
          // B is back in the room before the next round.
          await expect
            .poll(async () => (await clientId(b)) !== null && (await b.locator('.po-board-net').innerText()) === '', { timeout: 30_000 })
            .toBe(true);
        }
      }
    });

    await test.step('A kicks B: B gets the kicked screen and a way back', async () => {
      await openTab(a, 'people');
      await a.click('.po-people li[data-name="Bo"] [data-act=kick]');
      await a.click('.po-confirm [data-act=confirm]');
      await expect(b.locator('[data-end=kicked]')).toBeVisible();
      await expect(a.locator('.po-people li[data-name="Bo"]')).toHaveCount(0);
      await b.getByRole('button', { name: 'Back to the lobby' }).click();
      await expect(b.getByRole('button', { name: 'Create room' })).toBeVisible();
    });
  } finally {
    const log = [...A.log, ...B.log];
    if (log.length) await test.info().attach('browser-log', { body: log.join('\n'), contentType: 'text/plain' });
    await A.ctx.close();
    await B.ctx.close();
  }
});

test('phone: touch join by code, drag, second-finger cancel, long-press select, pinch', async ({ browser }) => {
  const A = await newClient(browser, { viewport: { width: 1100, height: 720 } }, 'A');
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = devices['iPhone 13'];
  const B = await newClient(browser, { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch }, 'B');
  const a = A.page;
  const b = B.page;
  try {
    const s0 = await createRoom(a, 'Ann');
    await test.step('B types the room code in the lobby', async () => {
      await b.goto(`${SITE}/`);
      await b.fill('input[name=code]', s0.invite_code.toLowerCase());
      await b.locator('.code-form button[type=submit]').click();
      await b.fill('form input[name=name]', 'Pat');
      await b.locator('form button[type=submit]').click();
      await boardReady(b);
    });
    const bId = (await clientId(b))!;
    const cdp = await B.ctx.newCDPSession(b);

    await test.step('touch drag moves a piece (A sees it), then lets go', async () => {
      const s = (await snap(b))!;
      const q = freePiece(s);
      await cameraSettled(b);
      const from = await toPage(b, centre(q, s));
      await touch(cdp, 'touchStart', [{ ...from, id: 0 }]);
      await touchPath(cdp, from, { x: from.x + 40, y: from.y + 30 }, 6);
      const id = await heldPiece(a, bId, '(touch drag)');
      const before = piece(s, id);
      await touch(cdp, 'touchEnd', []);
      await noLease(a, 'touch drop');
      await expect
        .poll(async () => {
          const p = piece((await snap(a))!, id);
          return Math.hypot(p.x - before.x, p.y - before.y) > 5;
        })
        .toBe(true);
    });

    await test.step('a second finger mid-drag cancels: the piece goes back and is let go', async () => {
      const s = (await snap(b))!;
      const q = freePiece(s);
      await cameraSettled(b);
      const from = await toPage(b, centre(q, s));
      await touch(cdp, 'touchStart', [{ ...from, id: 0 }]);
      const to = { x: from.x + 50, y: from.y + 20 };
      await touchPath(cdp, from, to, 6);
      const id = await heldPiece(a, bId, '(second finger)');
      const before = piece(s, id);
      const second = { x: Math.min(to.x + 90, viewport.width - 10), y: to.y + 60, id: 1 };
      await touch(cdp, 'touchStart', [{ ...to, id: 0 }, second]);
      await touch(cdp, 'touchEnd', []);
      await noLease(a, 'second finger');
      await expect
        .poll(async () => {
          const p = piece((await snap(a))!, id);
          return { x: Math.round(p.x), y: Math.round(p.y) };
        })
        .toEqual({ x: Math.round(before.x), y: Math.round(before.y) });
    });

    await test.step('long-press selects a piece', async () => {
      const s = (await snap(b))!;
      const q = freePiece(s);
      await cameraSettled(b);
      const at = await toPage(b, centre(q, s));
      await touch(cdp, 'touchStart', [{ ...at, id: 0 }]);
      await expect.poll(async () => (await selection(b)).length, { timeout: 10_000 }).toBe(1);
      const [sel] = await selection(b);
      await touch(cdp, 'touchEnd', []);
      await expect.poll(() => selection(b)).toEqual([sel]);
      await noLease(a, 'long-press');
    });

    await test.step('pinch out zooms in', async () => {
      const before = await cameraScale(b);
      // Two fingers on the empty board (the pieces start in the tray), moving apart.
      const s = (await snap(b))!;
      const mid = { x: s.width / 2, y: s.height / 2 };
      expect(s.pieces.some((q) => Math.abs(centre(q, s).x - mid.x) < cell(s).w * 1.5 && Math.abs(centre(q, s).y - mid.y) < cell(s).h * 1.5)).toBe(false);
      await cameraSettled(b);
      const { x: cx, y: cy } = await toPage(b, mid);
      await touch(cdp, 'touchStart', [{ x: cx - 30, y: cy, id: 0 }, { x: cx + 30, y: cy, id: 1 }]);
      for (let i = 1; i <= 8; i++) {
        await touch(cdp, 'touchMove', [{ x: cx - 30 - i * 12, y: cy, id: 0 }, { x: cx + 30 + i * 12, y: cy, id: 1 }]);
      }
      await touch(cdp, 'touchEnd', []);
      await expect.poll(() => cameraScale(b)).toBeGreaterThan(before * 1.5);
      await noLease(a, 'pinch');
    });
  } finally {
    const log = [...A.log, ...B.log];
    if (log.length) await test.info().attach('browser-log', { body: log.join('\n'), contentType: 'text/plain' });
    await A.ctx.close();
    await B.ctx.close();
  }
});
