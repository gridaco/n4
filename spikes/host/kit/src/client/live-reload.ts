import type { RevisionChange, ViewerDocument } from './types.ts';

type Revision = Pick<ViewerDocument, 'sessionId' | 'revision'>;

/** Revision numbers start over when a native session is replaced. */
export function continuesSession(before: Revision, after: Revision, change: Pick<RevisionChange, 'fromRevision' | 'toRevision'>) {
  return before.sessionId === after.sessionId && change.fromRevision === before.revision &&
    change.toRevision === after.revision && after.revision === before.revision + 1;
}

/** Serializes explicit refreshes and filesystem hints for a single open session. */
export class LiveReload<T> {
  private load: () => Promise<T>;
  private onError: (error: unknown) => void;
  private onBusy?: (busy: boolean) => void;
  private running?: Promise<T>;
  private dirty = false;
  private disposed = false;

  constructor(load: () => Promise<T>, options: {
    onError: (error: unknown) => void;
    onBusy?: (busy: boolean) => void;
  }) {
    this.load = load;
    this.onError = options.onError;
    this.onBusy = options.onBusy;
  }

  get busy() {
    return this.running !== undefined;
  }

  changed() {
    if (this.disposed) return;
    this.dirty = true;
    if (!this.running) void this.refresh().catch(error => {
      if (!this.disposed) this.onError(error);
    });
  }

  refresh(): Promise<T> {
    if (this.disposed) return Promise.reject(new Error('Live reload is disposed'));
    if (!this.running) {
      this.running = Promise.resolve().then(() => this.drain()).finally(() => {
        this.running = undefined;
        if (this.disposed) return;
        this.onBusy?.(false);
        if (this.dirty && !this.running) this.changed();
      });
      this.onBusy?.(true);
    }
    return this.running;
  }

  private async drain() {
    do {
      if (this.disposed) throw new Error('Live reload is disposed');
      this.dirty = false;
      try {
        const value = await this.load();
        if (!this.dirty || this.disposed) return value;
      } catch (error) {
        if (!this.dirty || this.disposed) throw error;
      }
    } while (true);
  }

  dispose() {
    this.disposed = true;
    this.dirty = false;
  }
}
