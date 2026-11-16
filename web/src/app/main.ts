// The lobby app: landing/lobby, name prompt, create form, joining, end states.
// It does not import the board (WebGL); that chunk comes through loadBoard().
import init from '../../wasm/pieceoff_web_wasm.js';
import { Banner } from '../ui/banner';
import { CreateForm } from '../ui/createForm';
import { focusFirst, h, button } from '../ui/dom';
import { renderEnd } from '../ui/fatal';
import { LandingScreen } from '../ui/landing';
import { renderNamePrompt } from '../ui/namePrompt';
import { readConfig, type WebConfig } from './config';
import { BUILTIN_MEDIA, CreateError, createRoom, gridForPieceCount } from './create';
import { codeFromSearch, joinFlow, parseCode, type JoinDeps, type JoinFlow, type JoinState, type StartOptions } from './join';
import { loadBoard as defaultLoadBoard, type BoardModule } from './loadBoard';
import { LobbyPoller, fetchLobby, quickMatch, type LobbyRoom } from './lobby';
import { browserStorage, cleanName, loadName, playerKey, saveName, saveResume, type AppStorage } from './names';
import { loadTurnstile, renderTurnstile, type TurnstileApi, type TurnstileWidget } from './turnstile';
import './play.css';

/** After a 429 from room creation, Create stays disabled this long. */
export const CREATE_COOLDOWN_MS = 60_000;

export interface AppDeps {
  root: HTMLElement;
  config: WebConfig;
  storage?: AppStorage;
  fetch?: typeof fetch;
  location?: { search: string; pathname: string };
  history?: { replaceState(data: unknown, unused: string, url?: string | URL | null): void };
  reload?: () => void;
  turnstile?: () => Promise<TurnstileApi>;
  socket?: JoinDeps['socket'];
  visibility?: EventTarget & { visibilityState?: string };
  random?: () => number;
  loadBoard?: () => Promise<BoardModule>;
}

export interface App {
  readonly join: JoinFlow;
  destroy(): void;
}

export function startApp(d: AppDeps): App {
  const storage = d.storage ?? browserStorage();
  const loc = d.location ?? location;
  const hist = d.history ?? history;
  const f = d.fetch ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const api = d.config.api;
  const join = joinFlow({ api, storage, socket: d.socket });

  const banner = new Banner();
  const main = h('main', { class: 'po-main' });
  d.root.classList.add('po-app');
  d.root.replaceChildren(banner.el, main);

  let lastRooms: LobbyRoom[] | null = null;
  let board: { destroy(): void } | null = null;
  let widget: TurnstileWidget | null = null;
  let form: CreateForm | null = null;
  let back: (() => void) | null = null;
  let shown: JoinState['kind'] | 'create' | null = null;

  const poller = new LobbyPoller({ api, fetch: f, visibility: d.visibility });
  const landing = new LandingScreen({
    quickMatch: () => void quick(),
    create: () => openCreate({ listed: false }),
    enterCode: (raw) => {
      const code = parseCode(raw);
      if (!code) landing.setCodeError('Room codes are 8 letters and digits, like ABCD2345.');
      else startJoin(code, {});
    },
    join: (room, watch) => startJoin(room.code, { listed: true, spectator: watch }),
    refresh: () => poller.refresh(),
  });
  poller.onUpdate((u) => {
    if (u.rooms) {
      lastRooms = u.rooms;
      landing.setRooms(u.rooms);
    } else if (u.error) {
      landing.setError(u.error);
    }
  });

  const setUrl = (code: string | null) => {
    try {
      const q = new URLSearchParams(loc.search);
      if (code) q.set('c', code);
      else q.delete('c');
      const s = q.toString();
      hist.replaceState(null, '', `${loc.pathname}${s ? `?${s}` : ''}`);
    } catch {
      // history unavailable (sandboxed frame): the URL just does not follow
    }
  };

  const clearScreen = () => {
    board?.destroy();
    board = null;
    widget?.remove();
    widget = null;
    form?.destroy();
    form = null;
    back = null;
  };

  const show = (kind: typeof shown, el: HTMLElement, onBack: (() => void) | null = null) => {
    clearScreen();
    if (kind !== 'idle') poller.stop();
    shown = kind;
    back = onBack;
    main.replaceChildren(el);
    focusFirst(el);
  };

  const showLanding = () => {
    show('idle', landing.el);
    landing.setCodeError('');
    poller.start();
  };

  const startJoin = (code: string, o: StartOptions) => {
    banner.hide();
    setUrl(code);
    join.start(code, o);
  };

  const toLobby = () => {
    banner.hide();
    join.leave(); // -> idle -> landing
  };

  async function quick(): Promise<void> {
    let rooms = lastRooms;
    if (!rooms) {
      try {
        rooms = await fetchLobby(api, { fetch: f });
      } catch (e) {
        banner.show(e instanceof Error ? e.message : 'Could not load the room list.', 'error');
        return;
      }
    }
    const pick = quickMatch(rooms);
    if (pick) {
      startJoin(pick.code, { listed: true });
      return;
    }
    const r = d.random ?? Math.random;
    const sha = BUILTIN_MEDIA[Math.floor(r() * BUILTIN_MEDIA.length) % BUILTIN_MEDIA.length]!.sha256;
    openCreate({ listed: true, sha, note: 'No public room has a free seat right now. Start one:' });
  }

  function openCreate(o: { listed: boolean; sha?: string; note?: string }): void {
    banner.hide();
    const cf = new CreateForm({
      assetBase: d.config.assetBase,
      name: loadName(storage),
      listed: o.listed,
      sha: o.sha,
      note: o.note,
      back: () => showLanding(),
      submit: (v) => {
        const media = BUILTIN_MEDIA.find((m) => m.sha256 === v.sha);
        const token = cf.currentToken;
        if (!media || !token) return;
        const grid = gridForPieceCount(v.pieces, media.width / media.height, v.cutStyle);
        cf.setBusy(true);
        cf.setError('');
        createRoom({
          api,
          turnstileToken: token,
          title: media.title,
          rows: grid.rows,
          columns: grid.columns,
          sha: media.sha256,
          name: v.name,
          listed: v.listed,
          cutStyle: v.cutStyle,
          playerKey: playerKey(storage),
          fetch: f,
        }).then(
          (made) => {
            const name = cleanName(v.name);
            saveName(storage, name);
            // The creator's secret, handed over in memory so the first join is
            // the host even without sessionStorage; stored too, so a reload of
            // this tab resumes as host where storage works.
            const resume = { token: made.resumeToken, name, spectator: false };
            saveResume(storage, made.code, resume);
            startJoin(made.code, { listed: v.listed, resume });
          },
          (e: unknown) => {
            if (form !== cf) return;
            cf.setBusy(false);
            // Turnstile tokens are single use: every attempt needs a fresh check.
            widget?.reset();
            cf.setToken(null);
            const err = e instanceof CreateError ? e : new CreateError('network', 'Could not create the room. Please try again.');
            if (err.kind === 'rate-limited') cf.cooldown(CREATE_COOLDOWN_MS);
            cf.setError(err.message);
          },
        );
      },
    });
    show('create', cf.el, () => showLanding());
    form = cf;
    shown = 'create';
    const load = d.turnstile ?? (() => loadTurnstile());
    load().then(
      (ts) => {
        if (form !== cf) return;
        widget = renderTurnstile(ts, cf.turnstileHost, {
          siteKey: d.config.turnstileSiteKey,
          onToken: (t) => cf.setToken(t),
          onExpire: () => cf.setToken(null),
          onError: () => cf.setCheckProblem('The human check did not work. Reload the page and try again.'),
        });
        cf.setToken(null);
      },
      () => {
        if (form === cf) {
          cf.setCheckProblem('The human check did not load. Allow challenges.cloudflare.com to create rooms.');
        }
      },
    );
  }

  // A /join?c=CODE link (or a reload mid-room) goes straight to that room.
  // Read before the first (idle) state clears the URL.
  const linked = codeFromSearch(loc.search);
  const unsubscribe = join.state.subscribe((s) => {
    switch (s.kind) {
      case 'idle':
        if (!linked || shown !== null) setUrl(null);
        showLanding();
        return;
      case 'asking-name':
      case 'name-taken':
        show(
          s.kind,
          renderNamePrompt({
            code: s.code,
            spectator: s.spectator,
            value: s.kind === 'name-taken' ? s.name : s.suggested,
            problem: s.kind === 'name-taken' ? s.message : null,
            submit: (n) => join.submitName(n),
            back: toLobby,
          }),
          toLobby,
        );
        return;
      case 'checking': {
        const note = s.rateLimited
          ? 'The server is busy. Trying again shortly…'
          : (s.message ?? 'Connecting…');
        if (shown === 'checking') {
          const p = main.querySelector<HTMLElement>('[data-checking-note]');
          if (p) p.textContent = note;
          return;
        }
        show(
          'checking',
          h(
            'section',
            { class: 'screen screen--checking', 'aria-labelledby': 'po-checking-title', 'aria-busy': 'true' },
            h('h1', { id: 'po-checking-title' }, s.spectator ? 'Joining to watch ' : 'Joining ', h('code', {}, s.code)),
            h('p', { class: 'muted', role: 'status', 'aria-live': 'polite', 'data-checking-note': true }, note),
            h('div', { class: 'actions' }, button('Cancel', toLobby)),
          ),
          toLobby,
        );
        return;
      }
      case 'joined-player':
      case 'joined-spectator':
      case 'room-full-spectating': {
        const host = h('div', { class: 'board-host' });
        show(s.kind, host);
        if (s.kind === 'room-full-spectating') banner.show('This room already has 8 players, so you are watching.');
        const joined = s;
        (d.loadBoard ?? defaultLoadBoard)().then(
          (m) => {
            if (join.state.get() !== joined) return;
            board = m.mountBoard(host, { ...joined, spectatorBecauseFull: joined.kind === 'room-full-spectating' }, toLobby);
          },
          () => banner.show('Could not load the board. Check your connection and reload.', 'error'),
        );
        return;
      }
      case 'room-not-found':
        show(
          s.kind,
          renderEnd(s.invalid ? 'invalid-code' : 'room-not-found', { reload: () => (d.reload ?? (() => location.reload()))(), retry: toLobby, lobby: toLobby }, s.code || undefined),
          toLobby,
        );
        return;
      case 'fatal':
        show(
          s.kind,
          renderEnd(
            s.reason,
            {
              reload: () => (d.reload ?? (() => location.reload()))(),
              retry: () => startJoin(s.code, {}),
              lobby: toLobby,
            },
            s.code,
          ),
          toLobby,
        );
        return;
    }
  });

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && back && !e.defaultPrevented) {
      e.preventDefault();
      back();
    }
  };
  d.root.addEventListener('keydown', onKey);

  if (linked) startJoin(linked, {});

  return {
    join,
    destroy() {
      unsubscribe();
      poller.stop();
      clearScreen();
      join.leave();
      d.root.removeEventListener('keydown', onKey);
    },
  };
}

/** Ensure a viewport meta with `viewport-fit=cover` exists (safe-area insets need it). */
export function ensureViewport(doc: Document = document): void {
  if (doc.querySelector('meta[name="viewport"]')) return;
  const m = doc.createElement('meta');
  m.name = 'viewport';
  m.content = 'width=device-width, initial-scale=1, viewport-fit=cover';
  doc.head.append(m);
}

/** Link `<assetBase>play.css` (esbuild emits it next to play.js) unless the page already does. */
export function ensureStylesheet(assetBase: string, doc: Document = document): void {
  const href = `${assetBase}play.css`;
  const have = [...doc.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]')].some(
    (l) => l.getAttribute('href') === href || l.href.endsWith('/play.css'),
  );
  if (have) return;
  const l = doc.createElement('link');
  l.rel = 'stylesheet';
  l.href = href;
  doc.head.append(l);
}

/** Mount the app into `root` (default `#pieceoff-play`), after the core WASM is ready. */
export async function boot(root?: HTMLElement | null): Promise<App | null> {
  const el = root ?? document.getElementById('pieceoff-play');
  if (!el) return null;
  const config = readConfig(window as unknown as { PIECEOFF_WEB_CONFIG?: unknown });
  ensureViewport();
  ensureStylesheet(config.assetBase);
  try {
    await init({ module_or_path: `${config.assetBase}pieceoff_web_wasm_bg.wasm` });
  } catch {
    el.replaceChildren(
      h(
        'section',
        { class: 'screen screen--end' },
        h('h1', {}, 'PieceOff could not start'),
        h('p', {}, 'This browser could not load the game core. Reload the page, or try a current browser.'),
        h('div', { class: 'actions' }, button('Reload', () => location.reload(), { class: 'btn btn--primary' })),
      ),
    );
    return null;
  }
  return startApp({ root: el, config });
}
