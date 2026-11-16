// Placeholder for the board chunk (Task 9). Not the board: it only confirms the
// join worked and offers Leave.
import { button, h } from '../ui/dom';
import type { BoardModule } from './loadBoard';

export const mountBoard: BoardModule['mountBoard'] = (root, joined, leave) => {
  const el = h(
    'section',
    { class: 'screen screen--joined', 'aria-labelledby': 'po-joined-title' },
    h('h1', { id: 'po-joined-title' }, joined.welcome.spectator ? 'Watching room ' : 'In room ', h('code', {}, joined.code)),
    h('p', {}, 'Joined as ', h('strong', {}, joined.name), '. ', joined.welcome.snapshot.title),
    h('p', { class: 'muted' }, 'Board not wired yet (development build).'),
    h('div', { class: 'actions' }, button('Leave room', leave, { class: 'btn' })),
  );
  root.replaceChildren(el);
  return { destroy: () => el.remove() };
};
