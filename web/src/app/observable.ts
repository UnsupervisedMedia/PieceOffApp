/** A current value plus change callbacks. `subscribe` calls back at once with the current value. */
export interface Observable<T> {
  get(): T;
  subscribe(cb: (v: T) => void): () => void;
}

export class Store<T> implements Observable<T> {
  private readonly cbs: Array<(v: T) => void> = [];
  constructor(private value: T) {}

  get(): T {
    return this.value;
  }

  set(v: T): void {
    this.value = v;
    for (const cb of [...this.cbs]) cb(v);
  }

  subscribe(cb: (v: T) => void): () => void {
    this.cbs.push(cb);
    cb(this.value);
    return () => {
      const i = this.cbs.indexOf(cb);
      if (i >= 0) this.cbs.splice(i, 1);
    };
  }
}
