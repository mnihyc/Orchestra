/** Shared directory observations. Each pane selects its own complete page; native demand,
 * payloads, selectors and unfinished reads have one bounded owner. No timer or implicit page walk. */
import { useSyncExternalStore } from 'react';
import { create } from 'zustand';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { installWorkspaceBindings } from '@/api/workspaceBinding';
import { listDirectory } from '@/api/workspace';
import type { Door } from '@/api/workspace';
import { socket } from '@/realtime/live';
import { DirectoryWindows } from './directoryWindows';
import { EMPTY, directoryKey } from './directoryTypes';
import type { DirectoryLease, DirectoryOpen, DirectoryOpened, DirectoryState, DirectoryWindow } from './directoryTypes';

export type { DirectoryAdmission, DirectoryCapacity, DirectoryCapacityReason, DirectoryLease, DirectoryNavigation,
  DirectoryOpen, DirectoryOpened, DirectoryState, DirectoryWindow } from './directoryTypes';
export { directoryKey, directoryFeed, feedPath, sortEntries } from './directoryTypes';
export { MOST_WATCHED } from './directoryWindows';
export { DIRECTORY_READ_CONCURRENCY } from './directoryReads';

interface DirectoryStore { dirs: ReadonlyMap<string, DirectoryState> }
export const useDirectories = create<DirectoryStore>(() => ({ dirs: new Map() }));
const windows = new DirectoryWindows({
  reader: () => client.session.getState().readerVersion,
  fetch: listDirectory,
  subscribe: (feed, options) => socket.subscribe(feed, options),
  publish: (dirs) => useDirectories.setState({ dirs }),
});
installWorkspaceBindings(queryClient, (environmentId) => windows.forget(environmentId));

/** Compatibility head for non-paged consumers. It never changes another pane's selection. */
export const directoryWriter = {
  load: windows.load.bind(windows),
  retry: windows.retry.bind(windows),
  apply: windows.apply.bind(windows),
  forget: windows.forget.bind(windows),
  clear: windows.clear.bind(windows),
};
export function useDirectory(environmentId: string, path: string): DirectoryState | undefined {
  return useDirectories((state) => state.dirs.get(directoryKey(environmentId, path)));
}

/** Acquire in an effect. A refused admission is an explicit state, never a loading-loop trigger. */
export function openDirectory(door: Door, path: string, options: DirectoryOpen): DirectoryOpened {
  listenToTransport();
  return windows.open(door, path, options);
}
const noSubscription = () => () => undefined;
const emptyWindow = () => EMPTY;
/** One observer per lease. Share its snapshot with child controls, or acquire another bounded lease. */
export function useDirectoryWindow(lease: DirectoryLease | null): DirectoryWindow {
  return useSyncExternalStore(lease?.subscribe ?? noSubscription, lease?.read ?? emptyWindow, emptyWindow);
}
export function watchDirectory(door: Door, path: string): () => void { listenToTransport(); return windows.watch(door, path); }
export function stopWatching(): void { windows.clear(); }

// One lifetime listener, not a per-directory listener/queue, installed when the first directory is opened or
// watched: before that a change of the connection has nothing to touch, and importing this module starts nothing.
// Read-only raw observations belong to the current reader just as QueryClient rows do. Reconnect acknowledgements are
// not freshness.
let listening = false;
function listenToTransport(): void {
  if (listening) return;
  listening = true;
  socket.status.subscribe((next, previous) => {
    if (next.phase !== previous.phase) windows.transport(next.phase === 'ready');
  });
}
client.onSession((event) => { if (event.type === 'signed_out') stopWatching(); });
client.session.subscribe((next, previous) => {
  if (next.readerVersion !== previous.readerVersion) stopWatching();
});
