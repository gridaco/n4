import type { ExportStatus } from '@n4/host-kit/types';

export interface ExportProject {
  name: string;
  size: [width: number, height: number];
  fps: [numerator: number, denominator: number];
  duration: number;
  revision: number;
}

export interface ExportDialogBridge {
  inspect(): Promise<ExportProject>;
  startExport(revision: number): Promise<ExportStatus | null>;
  exportStatus(): Promise<ExportStatus>;
  cancelExport(): Promise<ExportStatus>;
  requestClose(): Promise<void>;
}
