// Full-screen end states. Each has a plain explanation and a way out.
import type { FatalReason } from '../net/socket';
import { button, h } from './dom';

export type EndState = FatalReason | 'room-not-found' | 'invalid-code';

export interface EndCopy {
  title: string;
  body: string;
  /** The main way out. */
  primary: 'reload' | 'retry' | 'lobby';
}

export const END_COPY: Record<EndState, EndCopy> = {
  'update-client': {
    title: 'PieceOff has been updated',
    body: 'The rooms server runs a newer version than this page. Reload to update, then join again.',
    primary: 'reload',
  },
  'update-server': {
    title: 'This page is newer than the server',
    body: 'The rooms server has not caught up with this version yet. Try again in a few minutes.',
    primary: 'lobby',
  },
  kicked: {
    title: 'You were removed from the room',
    body: 'The host removed you from this room.',
    primary: 'lobby',
  },
  banned: {
    title: 'You can’t rejoin this room',
    body: 'The host removed you from this room, so it will not let you back in.',
    primary: 'lobby',
  },
  'room-gone': {
    title: 'This room has closed',
    body: 'The room is no longer on the server. Rooms close after everyone has been gone for a while.',
    primary: 'lobby',
  },
  'room-not-found': {
    title: 'Room not found',
    body: 'No room has that code. It may have closed, or the code has a typo.',
    primary: 'lobby',
  },
  'invalid-code': {
    title: 'That is not a room code',
    body: 'Room codes are 8 letters and digits, like ABCD2345 (no I, O, 0 or 1).',
    primary: 'lobby',
  },
  paused: {
    title: 'PieceOff Online is paused',
    body: 'Online play is switched off for a while. Please try again later.',
    primary: 'retry',
  },
  'rate-limited': {
    title: 'Too many tries',
    body: 'This connection asked the server too often. Wait a minute, then try again.',
    primary: 'lobby',
  },
  'opened-elsewhere': {
    title: 'Opened in another window',
    body: 'You joined this room from another tab or device with the same name, so this window let go.',
    primary: 'retry',
  },
  'name-taken': {
    title: 'That name is taken',
    body: 'Someone in this room already uses that name.',
    primary: 'lobby',
  },
};

export interface EndActions {
  reload(): void;
  retry(): void;
  lobby(): void;
}

export function renderEnd(state: EndState, a: EndActions, code?: string): HTMLElement {
  const copy = END_COPY[state];
  const actions = h('div', { class: 'actions' });
  const lobby = button('Back to the lobby', a.lobby, { class: copy.primary === 'lobby' ? 'btn btn--primary' : 'btn' });
  if (copy.primary === 'reload') actions.append(button('Reload to update', a.reload, { class: 'btn btn--primary' }), lobby);
  else if (copy.primary === 'retry') actions.append(button('Try again', a.retry, { class: 'btn btn--primary' }), lobby);
  else actions.append(lobby);
  return h(
    'section',
    { class: 'screen screen--end', 'aria-labelledby': 'po-end-title', 'data-end': state },
    h('h1', { id: 'po-end-title' }, copy.title),
    h('p', {}, copy.body),
    code ? h('p', { class: 'muted' }, 'Room ', h('code', {}, code)) : null,
    actions,
  );
}
