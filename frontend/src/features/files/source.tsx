/** Authorized file sources adapt attachments to the workspace's existing viewers without copying their rendering. */
import { queryOptions } from '@tanstack/react-query';
import { createContext, useContext } from 'react';
import { documentFileArchivePath, documentFilePath } from '@/api/documents';
import { traceRequest } from '@/api/trace';
import { bringFile, headQuery, type FileHead } from './content';
import { type Door, readFileBytes, type FileBytes } from '@/api/workspace';
import type { ArchiveListing, Author, ChatPart, ContentPart, DocumentFile, ReferencePreview } from '@/generated/api-types';
import { attachmentFileReference, environmentFileReference, markdownReference } from '@/features/people/references';
import { extensionOf, languageOf } from './languages';

export interface FileSource {
  /** An owner-specific API path, without the /api prefix. */
  url: string;
  name: string;
  size?: number | undefined;
  mediaType?: string | undefined;
  hash?: string | undefined;
  /** The path and door are available only for workspace files. */
  workspace?: { door: Door; path: string } | undefined;
  /** An attachment identifier has stable reference grammar. */
  fileId?: string | undefined;
  /** The materialized path as the receiving thread knows it. */
  deliveredPath?: string | undefined;
  reference?: string | undefined;
  archiveUrl?: string | undefined;
  line?: number | undefined;
  owner?: Author | undefined;
  updatedAt?: string | undefined;
  description?: string | undefined;
  href?: string | undefined;
  /** Immutable blob identity or the owner's current file revision. */
  version?: string | undefined;
}

/** A single bounded reply carries the PNG or inert excerpt; no original is fetched for a thumbnail. */
export interface FilePreview {
  file_id?: string;
  path?: string;
  state: 'ready' | 'unavailable' | 'too_large';
  hash: string | null;
  size: number;
  media_type: string;
  is_text: boolean;
  image: string | null;
  text: string | null;
  width: number | null;
  height: number | null;
  duration_seconds: number | null;
  entry_count: number | null;
  viewer_bytes?: number;
}

const append = (url: string, values: Record<string, string>) => `${url}${url.includes('?') ? '&' : '?'}${new URLSearchParams(values)}`;
export const FileSourceContext = createContext<FileSource | null>(null);
export const useFileSource = () => useContext(FileSourceContext);

export function blobSource(kind: 'chat' | 'thread' | 'upload', owner: string, blob: string, name: string, size?: number, mediaType?: string): FileSource {
  const url = kind === 'upload' ? `/uploads/${blob}?${new URLSearchParams({ name })}` : `/${kind}s/${owner}/blobs/${blob}`;
  return { url, name, size, mediaType, hash: blob, archiveUrl: `/uploads/${blob}?${new URLSearchParams({ name, archive: 'true' })}` };
}

export function documentSource(id: string, file: DocumentFile, inspect = false): FileSource {
  const url = documentFilePath(id, file.id, inspect);
  return { url, name: file.name, size: file.size, mediaType: file.media_type, fileId: file.id, version: file.added_at, updatedAt: file.added_at, owner: file.added_by ?? undefined, archiveUrl: documentFileArchivePath(id, file.id, inspect) };
}

export function workspaceSource(door: Door, path: string, name = path.split('/').at(-1) ?? path, size?: number): FileSource {
  return { url: `/environments/${door.environmentId}/files/preview?${new URLSearchParams({ path, ...(door.threadId === null ? {} : { thread: door.threadId }) })}`, name, size, workspace: { door, path } };
}

/** A reference already checked by its card reaches the same owner-specific source as an attachment. */
export function referenceSource(preview: ReferencePreview): FileSource | null {
  const file = preview.file;
  if (file === undefined || file === null || file.missing) return null;
  const line = preview.part !== null && 'line' in preview.part.part ? preview.part.number : undefined;
  if (preview.kind === 'environment_file' && file.path !== null) return { ...workspaceSource({ environmentId: preview.id, threadId: null }, file.path, preview.title, file.size ?? undefined), line };
  if (preview.kind === 'file' && file.owner_kind === 'chat' && file.owner_id !== null && typeof file.blob === 'string') return { ...blobSource('chat', file.owner_id, file.blob, preview.title, file.size ?? undefined, file.media_type ?? undefined), fileId: preview.id, line };
  if (preview.kind !== 'file' || file.owner_kind !== 'document' || file.owner_id === null) return null;
  return { url: documentFilePath(file.owner_id, preview.id), name: preview.title, size: file.size ?? undefined, mediaType: file.media_type ?? undefined, fileId: preview.id, version: preview.updated_at ?? undefined, archiveUrl: documentFileArchivePath(file.owner_id, preview.id), line };
}

export const previewQuery = (source: FileSource) => queryOptions({
  queryKey: ['file-preview', source.url, source.version ?? null] as const,
  meta: { fileEnvironment: source.workspace?.door.environmentId ?? null, filePath: source.workspace?.path ?? null },
  queryFn: async ({ signal }): Promise<FilePreview> => {
    const response = await traceRequest(source.workspace === undefined ? append(source.url, { preview: 'true' }) : source.url, { signal });
    const preview = await response.json() as FilePreview;
    const file = response.headers.get('x-orch-file-id');
    const path = response.headers.get('x-orch-file-path');
    return { ...preview, ...(file === null ? {} : { file_id: file }), ...(path === null ? {} : { path: decodeURIComponent(path.replace(/\+/g, ' ')) }) };
  },
  staleTime: Number.POSITIVE_INFINITY, gcTime: 60_000, retry: false,
  refetchOnWindowFocus: false, refetchOnReconnect: false,
});

/** Only an explicit viewer requests the original; text is read to its existing preview bound. */
export async function sourceBytes(source: FileSource, hash: string, limit?: number, signal?: AbortSignal): Promise<FileBytes> {
  if (source.workspace !== undefined) return readFileBytes(source.workspace.door, source.workspace.path, hash, limit, signal);
  const response = await traceRequest(source.url, { ...(signal === undefined ? {} : { signal }), ...(limit === undefined ? {} : { headers: { Range: `bytes=0-${limit - 1}` } }) });
  const bytes = new Uint8Array(await response.arrayBuffer());
  const total = /\/(\d+)\s*$/.exec(response.headers.get('Content-Range') ?? '')?.[1];
  return { bytes, total: total === undefined ? bytes.length : Number(total) };
}

/** An explicit download lets the browser spool a Blob without allocating a second whole-file JavaScript buffer. */
export async function sourceDownload(source: FileSource): Promise<void> {
  let url = `${source.url}${source.url.includes('?') ? '&' : '?'}download=true`;
  if (source.workspace !== undefined) {
    const { door, path } = source.workspace;
    const brought = await bringFile(door, path, new AbortController().signal);
    if (brought.state !== 'ready') throw new Error(brought.reason);
    url = `/environments/${door.environmentId}/files/content?${new URLSearchParams({ path, hash: brought.hash, download: 'true', ...(door.threadId === null ? {} : { thread: door.threadId }) })}`;
  }
  const response = await traceRequest(url);
  const blob = await response.blob();
  const address = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = address; link.download = source.name; link.click();
  setTimeout(() => URL.revokeObjectURL(address), 0);
}

export function sourceHasReference(source: FileSource): boolean {
  return source.fileId !== undefined || source.workspace !== undefined || source.reference !== undefined || source.deliveredPath !== undefined;
}

/** The common viewer receives its normal head even when the file belongs to a chat or document. */
export const sourceHeadQuery = (source: FileSource) => queryOptions({
  queryKey: ['file-source-head', source.url, source.version ?? null] as const,
  queryFn: async ({ client }): Promise<FileHead> => {
    if (source.workspace !== undefined) return client.query({ ...headQuery(source.workspace.door, source.workspace.path), staleTime: 0 });
    // Opening rechecks authority; a cached thumbnail alone never authorizes a new original view.
    const preview = await client.query({ ...previewQuery(source), staleTime: 0 });
    return {
      stat: { name: source.name, path: source.url, type: 'file', size: preview.size, mtime: '', mode: 0, link_target: null, cached: true,
        hash: preview.hash, media_type: preview.media_type, is_text: preview.is_text },
      brought: { state: 'ready', hash: preview.hash ?? source.hash ?? source.url, size: preview.size },
    };
  }, retry: false, staleTime: 0, gcTime: 0,
});

export function sourceReference(source: FileSource, line?: number): string {
  if (source.reference !== undefined && line === undefined) return source.reference;
  const at = line ?? source.line;
  if (source.workspace !== undefined) return environmentFileReference(source.workspace.door.environmentId, source.workspace.path, at);
  if (source.fileId !== undefined) return attachmentFileReference(source.fileId, source.name, at);
  if (source.deliveredPath !== undefined) return markdownReference(source.name, `${source.deliveredPath}${at === undefined ? '' : `#${at}`}`);
  // A transient draft has no saved reference yet; its attachment link still names the exact original.
  return markdownReference(source.name, source.url);
}

// Arbitrary filename suffixes must not resolve inherited object members as media types.
const mediaTypesByExtension = new Map(Object.entries({ png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml', heic: 'image/heic', heif: 'image/heif', avif: 'image/avif', tif: 'image/tiff', tiff: 'image/tiff', pdf: 'application/pdf', mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg', oga: 'audio/ogg', opus: 'audio/ogg', flac: 'audio/flac', m4a: 'audio/mp4', aac: 'audio/aac', mp4: 'video/mp4', webm: 'video/webm', mov: 'video/quicktime', ogv: 'video/ogg', m4v: 'video/mp4', mkv: 'video/x-matroska' }));

export function sourceFacts(source: FileSource, preview?: Pick<FilePreview, 'media_type' | 'size' | 'is_text'>) {
  const extension = extensionOf(source.name);
  const mediaType = preview?.media_type ?? source.mediaType ?? mediaTypesByExtension.get(extension) ?? '';
  return { name: source.name, extension, mediaType, size: preview?.size ?? source.size ?? 0,
    isText: preview?.is_text ?? (mediaType.startsWith('text/') || languageOf(source.name, null) !== null || ['txt', 'md', 'markdown'].includes(extension)) };
}

/** Messages, forwards, search results and notification projections use the same attachment order. */
export function sourcesOfParts(kind: 'chat' | 'thread', owner: string, parts: readonly (ChatPart | ContentPart)[]): FileSource[] {
  return parts.flatMap((part) => {
    if (part.type === 'forwarded') return [
      ...part.images.map((file) => blobSource(kind, owner, file.blob, file.name, undefined, file.media_type)),
      ...part.attachments.map((file) => blobSource(kind, owner, file.blob, file.name, file.size, file.media_type)),
    ];
    if (part.type === 'image') return [blobSource(kind, owner, part.blob, part.name, undefined, part.media_type)];
    if (part.type === 'attachment') return [blobSource(kind, owner, part.blob, part.name, part.size, part.media_type)];
    if (part.type === 'file' && part.blob !== null) return [{ ...blobSource(kind, owner, part.blob, part.name, part.size), deliveredPath: part.path }];
    return [];
  });
}

export async function sourceArchive(source: FileSource, inside: string, signal?: AbortSignal): Promise<ArchiveListing> {
  const url = source.archiveUrl;
  if (url === undefined) throw new Error('No archive source');
  return (await traceRequest(append(url, { inside }), { ...(signal === undefined ? {} : { signal }) })).json() as Promise<ArchiveListing>;
}

export async function sourceArchiveEntry(source: FileSource, entry: string, signal?: AbortSignal): Promise<Uint8Array> {
  const url = source.archiveUrl;
  if (url === undefined) throw new Error('No archive source');
  if (url.startsWith('/uploads/')) {
    const response = await traceRequest(append(url, { entry }), { ...(signal === undefined ? {} : { signal }) });
    return new Uint8Array(await response.arrayBuffer());
  }
  const split = url.indexOf('?');
  const base = split === -1 ? url : url.slice(0, split);
  const tail = split === -1 ? '' : url.slice(split);
  const response = await traceRequest(append(`${base}/entry${tail}`, { entry }), { ...(signal === undefined ? {} : { signal }) });
  return new Uint8Array(await response.arrayBuffer());
}
