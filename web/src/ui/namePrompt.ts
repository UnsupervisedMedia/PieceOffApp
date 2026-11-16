// "Who are you?" before joining. Re-shown with the server's text when a name is taken.
import { MAX_NAME_CHARS } from '../app/names';
import { button, h, uid } from './dom';

export interface NamePromptOptions {
  code: string;
  spectator: boolean;
  value: string;
  /** Verbatim refusal (name taken / public filter), if this is a re-prompt. */
  problem: string | null;
  submit(name: string): void;
  back(): void;
}

export function renderNamePrompt(o: NamePromptOptions): HTMLElement {
  const id = uid('name');
  const errId = uid('name-err');
  const input = h('input', {
    id,
    name: 'name',
    type: 'text',
    class: 'input',
    autocomplete: 'nickname',
    autocapitalize: 'words',
    spellcheck: 'false',
    enterkeyhint: 'go',
    maxlength: MAX_NAME_CHARS,
    required: true,
    autofocus: true,
    'aria-describedby': errId,
    'aria-invalid': o.problem ? 'true' : 'false',
  });
  input.value = o.value;
  const err = h('p', { id: errId, class: 'field-error', role: 'alert' }, o.problem ?? '');
  const form = h(
    'form',
    { class: 'screen screen--name', 'aria-labelledby': 'po-name-title', novalidate: true },
    h('h1', { id: 'po-name-title' }, o.spectator ? 'Watch room ' : 'Join room ', h('code', {}, o.code)),
    h('label', { for: id }, 'Your name'),
    input,
    err,
    h(
      'div',
      { class: 'actions' },
      h('button', { type: 'submit', class: 'btn btn--primary' }, o.spectator ? 'Watch' : 'Join'),
      button('Back', o.back),
    ),
  );
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    if (input.value.trim() === '') {
      err.textContent = 'Type a name first.';
      input.setAttribute('aria-invalid', 'true');
      input.focus();
      return;
    }
    o.submit(input.value);
  });
  if (o.problem) queueMicrotask(() => input.select());
  return form;
}
