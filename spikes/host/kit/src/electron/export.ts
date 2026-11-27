import { link, lstat, mkdtemp, rm } from 'node:fs/promises';
import { dirname, isAbsolute, join } from 'node:path';
import { setTimeout as wait } from 'node:timers/promises';
import type { NativeSession } from '../client/index.ts';
import type { ExportStatus } from '../client/types.ts';

export type ChooseExportPath = (suggestedName: string) => Promise<string | null>;

function suggestedName(name: string) {
  const basename = name.replace(/[/\\:*?<>|"\u0000-\u001f]/g, '-').replace(/[. ]+$/g, '').trim();
  return (basename || 'Untitled') + '.mp4';
}

/** Each renderer grant owns one export and its private staging directory. */
export class ViewerExport {
  status: ExportStatus = { state: 'idle' };
  private revoked = false;
  private choosing = false;
  private cancelBeforeStart = false;
  private session?: NativeSession;
  private completion?: Promise<void>;
  private readonly choosePath?: ChooseExportPath;

  constructor(choosePath?: ChooseExportPath) {
    this.choosePath = choosePath;
  }

  async start(session: NativeSession, revision: number): Promise<ExportStatus | null> {
    if (!this.choosePath) throw new Error('Export is unavailable in this window.');
    if (this.revoked || revision !== session.document.revision) throw new Error('The selected project revision is no longer open.');
    if (this.choosing || this.completion) throw new Error('An export is already in progress.');
    this.choosing = true;
    this.cancelBeforeStart = false;
    try {
      const selected = await this.choosePath(suggestedName(session.document.snapshot.name));
      if (this.revoked) throw new Error('Viewer document closed.');
      if (!selected || this.cancelBeforeStart) return null;
      if (!isAbsolute(selected)) throw new Error('Choose an absolute path for the MP4 export.');
      const destination = /\.mp4$/i.test(selected) ? selected : selected + '.mp4';
      const started = Promise.withResolvers<ExportStatus>();
      this.completion = Promise.resolve().then(() => this.run(session, revision, destination, started));
      return await started.promise;
    } finally {
      this.choosing = false;
    }
  }

  private async run(
    session: NativeSession,
    revision: number,
    destination: string,
    started: ReturnType<typeof Promise.withResolvers<ExportStatus>>,
  ) {
    let directory: string | undefined;
    let terminal: ExportStatus = { state: 'cancelled' };
    try {
      const exists = await lstat(destination).then(() => true, (error: NodeJS.ErrnoException) => {
        if (error.code === 'ENOENT') return false;
        throw error;
      });
      if (this.revoked || this.cancelBeforeStart) return;
      if (exists) throw new Error('That file already exists. Choose a new filename.');
      directory = await mkdtemp(join(dirname(destination), '.n4-export-'));
      if (this.revoked || this.cancelBeforeStart) return;
      const staged = join(directory, 'video.mp4');
      this.session = session;
      let native = await session.startExport(staged, revision);
      while (!this.revoked && (native.state === 'running' || native.state === 'cancelling')) {
        if (native.state === 'cancelling' || this.status.state !== 'cancelling') this.status = native;
        started.resolve(this.status);
        await wait(100);
        if (!this.revoked) native = await session.exportStatus();
      }
      if (!this.revoked) {
        switch (native.state) {
          case 'completed':
            // The staging directory lives beside the destination so this is
            // atomic and another process cannot be overwritten during publication.
            await link(staged, destination).catch((error: NodeJS.ErrnoException) => {
              if (error.code === 'EEXIST') throw new Error('That file already exists. Choose a new filename.');
              throw error;
            });
            terminal = { ...native, path: destination };
            break;
          case 'failed':
          case 'cancelled':
            terminal = native;
            break;
          default:
            throw new Error('The native export ended without a movie.');
        }
      }
    } catch (error) {
      terminal = { state: 'failed', error: error instanceof Error ? error.message : String(error) };
    } finally {
      if (directory) {
        await rm(directory, { recursive: true, force: true }).catch(error => {
          console.warn('Could not remove export staging directory:', error);
        });
      }
      this.status = terminal;
      this.session = undefined;
      this.completion = undefined;
      started.resolve(this.status);
    }
  }

  async cancel(): Promise<ExportStatus> {
    const { session, completion } = this;
    if (!session) {
      if (this.choosing) {
        this.cancelBeforeStart = true;
        this.status = { state: 'cancelled' };
        await completion;
      }
      return this.status;
    }
    const native = await session.cancelExport();
    if (native.state === 'cancelling' && completion === this.completion) this.status = native;
    else await completion;
    return this.status;
  }

  revoke() {
    this.revoked = true;
    return this.completion;
  }
}
