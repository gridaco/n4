import type { WebContents, IpcMainInvokeEvent } from 'electron';
import { NativeSession } from '../client/index.ts';
import type { EditorCommand, ExportStatus } from '../client/types.ts';
import { ViewerExport, type ChooseExportPath } from './export.ts';

export interface BindingOptions {
  executable: string;
  project: string;
  rendererURL: string;
  editor?: boolean;
  chooseExportPath?: ChooseExportPath;
}

/** Bind the one document granted by the parent. Each window owns its own IPC handlers. */
export function bindViewer(contents: WebContents, options: BindingOptions) {
  let opening: Promise<NativeSession> | undefined;
  let activeSession: NativeSession | undefined;
  let childLifetime: AbortController | undefined;
  let exporting = new ViewerExport(options.chooseExportPath);
  let shutdown = Promise.resolve();
  let disposal: Promise<void> | undefined;
  function release() {
    const active = activeSession;
    const lifetime = childLifetime;
    const completion = exporting.revoke();
    opening = undefined;
    activeSession = undefined;
    childLifetime = undefined;
    exporting = new ViewerExport(options.chooseExportPath);
    const cleanup = (async () => {
      if (active) {
        // A graceful close releases the accepted mix and cancels any encoder.
        // NativeSession times out and terminates a worker that fails to respond.
        await active.close().catch(() => {});
      }
      lifetime?.abort();
      await completion;
    })();
    shutdown = Promise.all([shutdown, cleanup]).then(() => {});
    return shutdown;
  }
  function reset() { void release(); }
  contents.on('render-process-gone', reset);
  // Full-document navigation invalidates the old renderer's outstanding work.
  contents.on('did-navigate', reset);
  async function currentSession() {
    const grant = opening;
    if (!grant) throw new Error('Viewer process stopped. Reopen the project.');
    const session = await grant;
    if (disposal || grant !== opening) throw new Error('Viewer document closed');
    return session;
  }

  const operations = ['inspect', 'seek', 'thumbnail', 'play', 'next', 'pause', 'presented', 'reopen', 'editor', 'hud'] as const;
  for (const op of operations) {
    contents.ipc.handle('n4.viewer.' + op, async (event: IpcMainInvokeEvent, ...args: unknown[]) => {
      if (!event.senderFrame || event.senderFrame !== contents.mainFrame ||
          event.senderFrame.url !== options.rendererURL) throw new Error('Unknown viewer origin');
      if ((op === 'editor' || op === 'hud') && !options.editor) throw new Error('Editor session is unavailable');
      const last = args[2];
      const placement = op === 'thumbnail' && args.length === 3 && typeof last === 'string' && last.length > 0
        ? last : undefined;
      const previewOnly = op === 'seek' && args.length === 3 && typeof last === 'boolean' ? last : undefined;
      const numbers = placement === undefined && previewOnly === undefined ? args : args.slice(0, 2);
      const arity = op === 'seek' || op === 'play' || op === 'presented' || op === 'thumbnail' || op === 'hud' ? 2 :
        op === 'next' || op === 'pause' ? 1 : 0;
      if (op === 'editor') {
        const command = args[0];
        if (args.length !== 1 || typeof command !== 'object' || command === null ||
            typeof (command as Record<string, unknown>).op !== 'string')
          throw new Error('Invalid editor command');
      } else if ((op === 'reopen' ? numbers.length > 1 : numbers.length !== arity) ||
          numbers.some(v => !Number.isSafeInteger(v) || Number(v) < 0)) {
        throw new Error('Invalid viewer arguments');
      }
      if (disposal) throw new Error('Viewer document closed');
      if (!opening) {
        if (op !== 'inspect' && op !== 'reopen') throw new Error('Viewer process stopped. Reopen the project.');
        childLifetime = new AbortController();
        const { signal } = childLifetime;
        const pending = shutdown.then(() => {
          signal.throwIfAborted();
          return NativeSession.open(options.executable, options.project, signal, { editor: options.editor });
        });
        opening = pending;
        void pending.then(session => {
          if (opening === pending) {
            activeSession = session;
            session.onProjectChange(() => {
              if (opening === pending && !contents.isDestroyed() && contents.mainFrame.url === options.rendererURL)
                contents.send('n4.viewer.inputsChanged');
            });
          }
          return session.closed;
        }, () => {}).then(() => {
          if (opening === pending) {
            opening = undefined;
            activeSession = undefined;
          }
        });
      }
      const grant = opening, session = await currentSession();
      switch (op) {
        case 'inspect': return session.document;
        case 'editor': return session.editor(args[0] as EditorCommand);
        case 'hud': return session.hud(Number(args[0]), Number(args[1]));
        case 'seek': return session.seek(Number(args[0]), Number(args[1]), previewOnly);
        case 'thumbnail': {
          const lifetime = childLifetime;
          try {
            return await session.thumbnail(Number(args[0]), Number(args[1]), placement);
          } catch (error) {
            if (grant !== opening || lifetime?.signal.aborted ||
                (error instanceof Error && /^(thumbnail superseded|stale revision)/.test(error.message))) return null;
            throw error;
          }
        }
        case 'reopen': return session.reopen(args.length ? Number(args[0]) : undefined);
        case 'next': return session.next(Number(args[0]));
        case 'pause': return session.pause(Number(args[0]));
        default: return session[op](Number(args[0]), Number(args[1]));
      }
    });
  }
  const dispose = ({ closingWindow = false } = {}) => {
    if (disposal) return disposal;
    disposal = release().then(() => {
      if (!closingWindow && !contents.isDestroyed()) {
        for (const op of operations) contents.ipc.removeHandler('n4.viewer.' + op);
      }
    });
    if (!contents.isDestroyed()) {
      contents.off('did-navigate', reset);
      contents.off('render-process-gone', reset);
      contents.off('destroyed', onDestroyed);
    }
    return disposal;
  };
  function onDestroyed() { void dispose({ closingWindow: true }); }
  contents.once('destroyed', onDestroyed);
  return Object.assign(dispose, {
    async inspectForExport() { return (await currentSession()).document; },
    async startExport(revision: number) {
      const owner = exporting, session = await currentSession();
      return owner.start(session, revision);
    },
    exportStatus(): ExportStatus { return disposal ? { state: 'cancelled' } : exporting.status; },
    async cancelExport(): Promise<ExportStatus> {
      return disposal ? { state: 'cancelled' } : exporting.cancel();
    },
  });
}
