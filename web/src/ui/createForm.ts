// Create a room: one of the eight built-in puzzles, a piece count, a cut style,
// the v16 difficulty options (turned pieces, no hints), listed or invite-only,
// the host's name, and the Turnstile check.
import { BUILTIN_MEDIA, CUT_STYLES, LARGE_BOARD, PIECE_PRESETS } from '../app/create';
import { MAX_NAME_CHARS } from '../app/names';
import { builtinAssets } from '../media/builtin';
import type { CutStyle, Difficulty } from '../protocol';
import { button, h, uid } from './dom';

export interface CreateValues {
  sha: string;
  title: string;
  pieces: number;
  cutStyle: CutStyle;
  difficulty: Difficulty;
  listed: boolean;
  name: string;
}

export interface CreateFormOptions {
  assetBase: string;
  name: string;
  listed: boolean;
  /** Preselect this puzzle (Quick match with no open rooms picks one at random). */
  sha?: string;
  note?: string;
  submit(v: CreateValues): void;
  back(): void;
}

export class CreateForm {
  readonly el: HTMLFormElement;
  /** Where the Turnstile widget renders. */
  readonly turnstileHost: HTMLElement;
  private readonly submitBtn: HTMLButtonElement;
  private readonly error: HTMLElement;
  private readonly checkNote: HTMLElement;
  private token: string | null = null;
  private busy = false;
  private cooldownUntil = 0;
  private cooldownTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly o: CreateFormOptions) {
    const pick = o.sha && BUILTIN_MEDIA.some((m) => m.sha256 === o.sha) ? o.sha : BUILTIN_MEDIA[0]!.sha256;
    const puzzles = h('fieldset', { class: 'puzzles' }, h('legend', {}, 'Puzzle'));
    for (const m of BUILTIN_MEDIA) {
      const id = uid('puzzle');
      const poster = builtinAssets(m.sha256, `${o.assetBase}builtin/`)?.poster;
      puzzles.append(
        h(
          'div',
          { class: 'puzzle' },
          h('input', { type: 'radio', name: 'puzzle', id, value: m.sha256, checked: m.sha256 === pick, class: 'visually-hidden' }),
          h(
            'label',
            { for: id },
            poster ? h('img', { src: poster, alt: '', width: 120, height: 80, loading: 'lazy', decoding: 'async' }) : null,
            h('span', {}, m.title),
          ),
        ),
      );
    }

    const pieces = h('fieldset', { class: 'chips' }, h('legend', {}, 'Pieces'));
    for (const p of PIECE_PRESETS) {
      const id = uid('pieces');
      pieces.append(
        h(
          'span',
          { class: 'chip' },
          h('input', { type: 'radio', name: 'pieces', id, value: p.count, checked: p.count === 48, class: 'visually-hidden' }),
          h('label', { for: id }, `${p.label} · ${p.count}`),
        ),
      );
    }

    const cuts = h('fieldset', { class: 'chips' }, h('legend', {}, 'Cut'));
    for (const c of CUT_STYLES) {
      const id = uid('cut');
      cuts.append(
        h(
          'span',
          { class: 'chip' },
          h('input', { type: 'radio', name: 'cut', id, value: c.value, checked: c.value === 'jigsaw', class: 'visually-hidden' }),
          h('label', { for: id, title: c.hint }, c.label),
        ),
      );
    }

    // The desktop's "Harder" options (ui.rs), same labels and hover text.
    const check = (name: string, label: string, hint: string) => {
      const id = uid(name);
      return h(
        'div',
        { class: 'check' },
        h('input', { type: 'checkbox', id, name }),
        h('label', { for: id, title: hint }, label),
      );
    };
    const harder = h(
      'fieldset',
      { class: 'harder' },
      h('legend', {}, 'Harder'),
      check('rotation', 'Rotate pieces', 'Pieces start turned. Turn them upright (E / Q) before they fit. Players on older versions cannot join.'),
      check(
        'no_hints',
        'No hints',
        'No reference image, board ghost, still-frame assist, edge filter or edges-first tidy. Played on trust: a modified client could still show them.',
      ),
    );
    const largeNote = h('p', { class: 'muted', hidden: true }, 'Large boards need a fast device, and are slower to join.');
    const syncLarge = () => {
      largeNote.hidden = Number(new FormData(this.el).get('pieces')) <= LARGE_BOARD;
    };

    const listedId = uid('listed');
    const listed = h(
      'div',
      { class: 'check' },
      h('input', { type: 'checkbox', id: listedId, name: 'listed', checked: o.listed }),
      h('label', { for: listedId }, 'List in the public lobby (strangers can join; chat starts off)'),
    );

    const nameId = uid('host-name');
    const nameInput = h('input', {
      id: nameId,
      name: 'name',
      type: 'text',
      class: 'input',
      autocomplete: 'nickname',
      autocapitalize: 'words',
      spellcheck: 'false',
      maxlength: MAX_NAME_CHARS,
      required: true,
    });
    nameInput.value = o.name;

    this.turnstileHost = h('div', { class: 'turnstile' });
    this.checkNote = h('p', { class: 'muted', role: 'status', 'aria-live': 'polite' }, 'Loading the human check…');
    this.error = h('p', { class: 'field-error', role: 'alert' });
    this.submitBtn = h('button', { type: 'submit', class: 'btn btn--primary', disabled: true }, 'Create room');

    this.el = h(
      'form',
      { class: 'screen screen--create', 'aria-labelledby': 'po-create-title', novalidate: true },
      h('h1', { id: 'po-create-title' }, 'Create a room'),
      o.note ? h('p', {}, o.note) : null,
      puzzles,
      pieces,
      largeNote,
      cuts,
      harder,
      listed,
      h('label', { for: nameId }, 'Your name'),
      nameInput,
      this.turnstileHost,
      this.checkNote,
      this.error,
      h('div', { class: 'actions' }, this.submitBtn, button('Back', o.back)),
    );

    this.el.addEventListener('change', syncLarge);

    this.el.addEventListener('submit', (e) => {
      e.preventDefault();
      if (!this.ready()) return;
      const data = new FormData(this.el);
      const sha = String(data.get('puzzle') ?? '');
      const name = nameInput.value;
      if (name.trim() === '') {
        this.setError('Type a name first.');
        nameInput.focus();
        return;
      }
      this.o.submit({
        sha,
        title: BUILTIN_MEDIA.find((m) => m.sha256 === sha)?.title ?? '',
        pieces: Number(data.get('pieces')),
        cutStyle: (String(data.get('cut')) as CutStyle) || 'jigsaw',
        difficulty: { rotation: data.get('rotation') !== null, no_hints: data.get('no_hints') !== null },
        listed: data.get('listed') !== null,
        name,
      });
    });
  }

  get currentToken(): string | null {
    return this.token;
  }

  setToken(t: string | null): void {
    this.token = t;
    this.checkNote.textContent = t ? '' : 'Complete the human check to create a room.';
    this.sync();
  }

  setCheckProblem(message: string): void {
    this.token = null;
    this.checkNote.textContent = message;
    this.sync();
  }

  setBusy(b: boolean): void {
    this.busy = b;
    this.submitBtn.textContent = b ? 'Creating…' : 'Create room';
    this.sync();
  }

  setError(message: string): void {
    this.error.textContent = message;
  }

  /** Keep Create disabled for a while (after a 429), so a refused click cannot be hammered. */
  cooldown(ms: number): void {
    this.cooldownUntil = Date.now() + ms;
    if (this.cooldownTimer) clearTimeout(this.cooldownTimer);
    this.cooldownTimer = setTimeout(() => {
      this.cooldownTimer = null;
      this.sync();
    }, ms);
    this.sync();
  }

  destroy(): void {
    if (this.cooldownTimer) clearTimeout(this.cooldownTimer);
  }

  private ready(): boolean {
    return !this.busy && this.token !== null && Date.now() >= this.cooldownUntil;
  }

  private sync(): void {
    this.submitBtn.disabled = !this.ready();
  }
}
