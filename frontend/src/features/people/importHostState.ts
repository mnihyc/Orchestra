/** Every import entry opens the same host, including a running job and a retained report. */
import { create } from 'zustand';
import { client } from '@/api/client';
import type { ImportDestinationName } from '@/features/people/ImportDestination';

export type ImportTarget =
  | { kind: 'new'; place: ImportDestinationName | null; folder: ImportDestinationName | null }
  | { kind: 'job'; job: string; place: string | null }
  | { kind: 'report'; report: string };

interface HostState {
  target: ImportTarget | null;
  user: string | null;
  reader: number | null;
}

export const useImportHost = create<HostState>(() => ({ target: null, user: null, reader: null }));

function show(target: ImportTarget): void {
  const session = client.session.getState();
  if (session.userId === null) return;
  useImportHost.setState({ target, user: session.userId, reader: session.readerVersion ?? 0 });
}

/** Opens a new import, optionally at the place and folder the person is viewing. */
export function openImport(place: ImportDestinationName | null = null, folder: ImportDestinationName | null = null): void {
  show({ kind: 'new', place, folder });
}

/** Reopens the progress of a job after its dialog was closed. */
export function openImportJob(job: string, place: string | null): void {
  show({ kind: 'job', job, place });
}

/** Opens a retained report. */
export function openImportReport(report: string): void {
  show({ kind: 'report', report });
}

/** Hides the dialog without stopping a running job. */
export function closeImport(): void { useImportHost.setState({ target: null, user: null, reader: null }); }
