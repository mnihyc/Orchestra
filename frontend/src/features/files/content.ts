/** Reading a file of the workspace: what it is, its exact version, and its bytes, in the fewest requests that can tell.
 *
 * A file is read in two steps. First it is described and, in the same breath, the environment is asked
 * to bring that version to the store, which answers with the hash that names it. Then the bytes are read by
 * that hash, which can be kept for as long as one likes since a hash names one content only. So the
 * description and the bringing go together, and the reading follows: two requests in a row before
 * a file shows. A file that changes is described again, its hash is another, and its bytes are asked
 * for anew.
 */
import { type QueryClient, queryOptions, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo } from 'react';

import { type Door, fetchFile, readFileBytes, statFile } from '@/api/workspace';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { FetchState, FetchUnavailable, FileStat } from '@/generated/api-types';

import { PREVIEW_BYTES } from './viewers';
import { sourceBytes, sourceDownload, useFileSource, workspaceSource, type FileSource } from './source';

/** How many times the environment is asked again while it brings a file: each ask holds for up to 25 seconds. */
const BRINGING_ASKS = 12;

/** Where the environment stands with a file the person asked to see. */
export type Brought = { state: 'ready'; hash: string; size: number | null } | { state: 'unavailable'; reason: FetchUnavailable };

/** Obtains the verified version of a workspace file before a viewer or an explicit download reads it. */
export async function bringFile(door: Door, path: string, signal: AbortSignal): Promise<Brought> {
  let last: FetchState | null = null;
  for (let ask = 0; ask < BRINGING_ASKS; ask += 1) {
    last = await fetchFile(door, path, signal);
    if (last.state === 'ready' && last.hash !== null) return { state: 'ready', hash: last.hash, size: last.size };
    if (last.state === 'unavailable') return { state: 'unavailable', reason: last.reason ?? 'not_found' };
  }
  return { state: 'unavailable', reason: last?.reason ?? 'offline_not_cached' };
}

/** What a file is, and the version of it that is ready to be read. */
export interface FileHead {
  stat: FileStat;
  brought: Brought;
}

export const headKey = (door: Door, path: string) => ['workspace-head', door.environmentId, path] as const;

export const headQuery = (door: Door, path: string) =>
  queryOptions({
    queryKey: headKey(door, path),
    queryFn: async ({ signal }): Promise<FileHead> => {
      const [stat, brought] = await Promise.all([statFile(door, path, signal), bringFile(door, path, signal)]);
      return { stat, brought };
    },
    // What is shown is what the directory says: it asks again when it is told that the file changed, and not for being looked at.
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 60_000,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });

/** Describes a file and brings its version; `reload` asks again. */
export function useFileHead(door: Door, path: string) {
  const shown = useViewShown();
  const query = useQuery({ ...headQuery(door, path), enabled: shown });
  const client = useQueryClient();
  return {
    head: query.data,
    error: query.error,
    loading: query.isPending,
    reload: () => invalidateHead(client, door, path),
  };
}

/** Says that a file is not what was read: its description is asked for again. */
export async function invalidateHead(client: QueryClient, door: Door, path: string): Promise<void> {
  await Promise.all([
    client.invalidateQueries({ queryKey: headKey(door, path), exact: true }),
    client.invalidateQueries({ predicate: (query) => query.queryKey[0] === 'file-preview'
      && query.meta?.fileEnvironment === door.environmentId && query.meta.filePath === path }),
  ]);
}

/** The bytes of one version of a file, the first `limit` of them. */
export const bytesQuery = (door: Door, path: string, hash: string, limit: number | undefined, source: FileSource | null = null) =>
  queryOptions({
    queryKey: ['workspace-bytes', source?.url ?? door.environmentId, path, hash, limit ?? 0] as const,
    queryFn: ({ signal }) => source === null ? readFileBytes(door, path, hash, limit, signal) : sourceBytes(source, hash, limit, signal),
    // A hash names one content for ever; the copy is kept a short while for going back and forth.
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 30_000,
  });

export interface FileText {
  text: string;
  /** The hash of the version it is. */
  hash: string;
  /** The whole size, when the text shown is only its first part. */
  total: number;
  /** The text is cut at the limit of what is read. */
  cut: boolean;
}

const decoder = new TextDecoder('utf-8', { fatal: false });

/** The text of the version that is ready, up to what is read and shown. */
export function useFileText(door: Door, path: string, brought: Brought | undefined, limit: number = PREVIEW_BYTES) {
  const source = useFileSource();
  const shown = useViewShown();
  const hash = brought?.state === 'ready' ? brought.hash : null;
  // A file with nothing in it has nothing to ask for.
  const empty = brought?.state === 'ready' && brought.size === 0;
  const query = useQuery({ ...bytesQuery(door, path, hash ?? '', limit, source), enabled: shown && hash !== null && !empty });
  const text = useMemo<FileText | null>(() => {
    if (hash === null) return null;
    if (empty) return { text: '', hash, total: 0, cut: false };
    if (query.data === undefined) return null;
    const cut = query.data.total > query.data.bytes.length;
    return { text: decoder.decode(query.data.bytes), hash, total: query.data.total, cut };
  }, [query.data, hash, empty]);
  return { text, error: query.error, loading: hash !== null && !empty && query.isPending };
}

/** The bytes of a version, all of them, for what the page itself shows: a picture, a sound, a film. */
export function useFileBytes(door: Door, path: string, brought: Brought | undefined) {
  const source = useFileSource();
  const shown = useViewShown();
  const hash = brought?.state === 'ready' ? brought.hash : null;
  const empty = brought?.state === 'ready' && brought.size === 0;
  const query = useQuery({ ...bytesQuery(door, path, hash ?? '', undefined, source), enabled: shown && hash !== null && !empty });
  const nothing = useMemo(() => new Uint8Array(0), []);
  return { bytes: empty ? nothing : (query.data?.bytes ?? null), error: query.error, loading: hash !== null && !empty && query.isPending };
}

/** Gives the person a file: its bytes are fetched with their credentials and handed to the browser to save. */
export async function downloadFile(door: Door, path: string, name: string): Promise<void> {
  return sourceDownload(workspaceSource(door, path, name));
}

/** Hands bytes to the browser to be saved under a name. */
export function saveBytes(name: string, bytes: Uint8Array<ArrayBuffer> | Uint8Array, type: string): void {
  const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  // The browser has been handed the file by the time the next turn of the loop comes.
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

/** Why a file cannot be had, as a key of the catalogue. */
export function unavailableKey(reason: FetchUnavailable): 'files.unavailable.offline' | 'files.unavailable.ignored' | 'files.unavailable.tooLarge' | 'files.unavailable.notFound' {
  switch (reason) {
    case 'offline_not_cached':
      return 'files.unavailable.offline';
    case 'ignored':
      return 'files.unavailable.ignored';
    case 'too_large':
      return 'files.unavailable.tooLarge';
    case 'not_found':
      return 'files.unavailable.notFound';
  }
}
