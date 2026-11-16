// A non-blocking message strip at the top of the app. Server text is shown
// verbatim (as text). It never takes focus; screen readers hear it through the
// live region.
import { button, h } from './dom';

export type Tone = 'info' | 'error';

export class Banner {
  readonly el: HTMLElement;
  private readonly text: HTMLElement;

  constructor() {
    this.text = h('p', { class: 'banner__text' });
    this.el = h(
      'div',
      { class: 'banner', role: 'status', 'aria-live': 'polite', hidden: true },
      this.text,
      button('Dismiss', () => this.hide(), { class: 'btn btn--quiet banner__close', 'aria-label': 'Dismiss message' }),
    );
  }

  show(message: string, tone: Tone = 'info'): void {
    this.text.textContent = message;
    this.el.dataset.tone = tone;
    this.el.hidden = false;
  }

  hide(): void {
    this.el.hidden = true;
    this.text.textContent = '';
  }

  get message(): string | null {
    return this.el.hidden ? null : this.text.textContent;
  }
}
