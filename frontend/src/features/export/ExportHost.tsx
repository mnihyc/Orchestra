/** The one entry point of Export…: `openExport(target)` from any menu, and the one host in the frame of the application that draws what it opens and follows the jobs that write archives.
 *
 * A menu does not know which dialog its thing needs. It names the thing, and the host opens the small dialog of a
 * thread, a chat or a document, or the dialog of a place with its tree, its options and its plan. The host also
 * keeps the watch over the archives being written, wherever the person is: it tells when one is ready.
 */
import { lazy, Suspense } from 'react';
import { create } from 'zustand';

import { useExportWatch } from './jobs';
import { isPlace, type ExportTarget } from './target';

const ThreadFile = lazy(() => import('./ThreadFile').then((module) => ({ default: module.ThreadFile })));
const ChatFile = lazy(() => import('./ChatFile').then((module) => ({ default: module.ChatFile })));
const DocumentFile = lazy(() => import('./DocumentFile').then((module) => ({ default: module.DocumentFile })));
const PlaceExport = lazy(() => import('./PlaceExport').then((module) => ({ default: module.PlaceExport })));
const RequestExport = lazy(() => import('./RequestExport').then((module) => ({ default: module.RequestExport })));

/** How long a dialog that was closed stays drawn, for its exit: the slow step of the motion tokens. */
const EXIT_MS = 200;

type Way = 'export' | 'request';

interface HostState {
  target: ExportTarget | null;
  way: Way;
  open: boolean;
  show: (target: ExportTarget, way: Way) => void;
  hide: () => void;
}

const useHost = create<HostState>((set, get) => ({
  target: null,
  way: 'export',
  open: false,
  show: (target, way) => set({ target, way, open: true }),
  hide: () => {
    set({ open: false });
    // The dialog leaves with its exit, and is forgotten once it has.
    setTimeout(() => {
      if (!get().open) set({ target: null });
    }, EXIT_MS);
  },
}));

/**
 * Opens Export… for a thing: a thread, a chat or a document gives one file at once, an org, a project, a
 * folder or a topic an archive of what the caller may read in it.
 */
export function openExport(target: ExportTarget): void {
  useHost.getState().show(target, 'export');
}

/** Opens the form that asks for the right to take a thing away as a file, where the caller holds none. */
export function requestExportAccess(target: ExportTarget): void {
  useHost.getState().show(target, 'request');
}

/** Draws the dialog that was asked for and follows the archives being written. Stands once in the frame of the application. */
export function ExportHost() {
  const target = useHost((state) => state.target);
  const way = useHost((state) => state.way);
  const open = useHost((state) => state.open);
  const hide = useHost((state) => state.hide);
  useExportWatch();
  if (target === null) return null;
  const key = `${way}:${target.kind}:${target.id}`;
  const props = { target, open, onClose: hide };
  return (
    <Suspense fallback={null}>
      {way === 'request' ? (
        <RequestExport key={key} {...props} />
      ) : isPlace(target) ? (
        <PlaceExport key={key} {...props} />
      ) : target.kind === 'thread' ? (
        <ThreadFile key={key} {...props} />
      ) : target.kind === 'chat' ? (
        <ChatFile key={key} {...props} />
      ) : (
        <DocumentFile key={key} {...props} />
      )}
    </Suspense>
  );
}
