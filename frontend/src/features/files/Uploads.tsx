/** Workspace uploads use resumable sessions and ask once how a batch handles existing names. */
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError, client } from '@/api/client';
import { finishWorkspaceUpload, writeUploadedFile, type Door, type UploadWriteAttempt } from '@/api/workspace';
import { UploadChips } from '@/features/composer/Attachments';
import { parentOf } from '@/features/files/FileTabs';
import type { FileStat, UploadedBlob, WorkspaceUploadAttempt } from '@/generated/api-types';
import { useUploads, type UploadWork } from '@/lib/useUploads';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { toast } from '@/ui/Toast';
import { ErrorAlert } from '@/ui/ErrorState';

export type ConflictChoice = 'replace' | 'keep';

/** A numbered sibling keeps the original extension, as file managers do. */
export function numberedPath(path: string, number: number): string {
  const slash = path.lastIndexOf('/');
  const dot = path.lastIndexOf('.');
  const split = dot > slash + 1 ? dot : path.length;
  return `${path.slice(0, split)} (${number})${path.slice(split)}`;
}

/** Browser-provided relative paths never escape the chosen folder. */
export function uploadPath(directory: string, file: File): string {
  const relative = file.webkitRelativePath || file.name;
  if (relative.startsWith('/') || relative.includes('\\') || relative.split('/').some((part) => part === '' || part === '.' || part === '..' || /\p{Cc}/u.test(part))) throw new Error('Invalid file path');
  return directory === '' ? relative : `${directory}/${relative}`;
}

/** A final absent check catches names created by somebody else while bytes were uploading. */
export async function writeUpload(path: string, signal: AbortSignal, write: (path: string, expected: string | null) => Promise<FileStat>, choose: () => Promise<ConflictChoice>): Promise<FileStat> {
  let candidate = path;
  let suffix = 1;
  for (;;) {
    signal.throwIfAborted();
    try { return await write(candidate, 'none'); }
    catch (error) {
      if (!(error instanceof ApiError) || !error.is('conflict.stale_revision') && !error.is('conflict.exists')) throw error;
      if (await choose() === 'replace') {
        signal.throwIfAborted();
        // Replace is explicit for the whole batch. Ordinary uploads always require absence.
        return write(path, null);
      }
      suffix += 1;
      candidate = numberedPath(path, suffix);
    }
  }
}

interface Batch {
  id: string;
  reader: number | undefined;
  folder: string;
  finished: boolean;
  cancelling: boolean;
  attempts: Map<File, WorkspaceUploadAttempt>;
  writes: Map<File, Map<string, UploadWriteAttempt>>;
  saved: Set<File>;
  choice: Promise<ConflictChoice> | null;
  resolve?: (choice: ConflictChoice) => void;
  reject?: (reason: Error) => void;
  ids: string[];
  path: string;
}

export function useWorkspaceUploads(door: Door, blocked: string | false, refresh: (...directories: string[]) => Promise<void>) {
  const { t } = useTranslation();
  const [conflicts, setConflicts] = useState<Batch[]>([]);
  const batches = useRef(new Set<Batch>());
  const [historyError, setHistoryError] = useState<{ batch: Batch; error: unknown } | null>(null);
  const { environmentId, threadId } = door;
  const reader = client.session.getState().readerVersion;
  const queue = useUploads<FileStat>(JSON.stringify(door), () => Promise.reject(new Error('Upload destination missing')), (file) => { void refresh(parentOf(file.path)); });
  const finish = useCallback((batch: Batch) => {
    if (batch.finished || batch.cancelling || batch.reader !== reader || client.session.getState().readerVersion !== reader) return;
    batch.finished = true;
    const attempts = [...batch.attempts.values()];
    const outcome = attempts.some((file) => file.outcome === 'failed') ? 'failed' : attempts.some((file) => file.outcome === 'cancelled') ? 'cancelled' : 'complete';
    // Completed writes already have authoritative audit facts; omit them from the browser's receipt list.
    const files = attempts.filter((file) => file.outcome !== 'complete' && (file.upload_id !== null || file.blob !== null));
    void finishWorkspaceUpload({ environmentId, threadId }, batch.id, { folder: batch.folder, files, outcome }).then(() => {
      batches.current.delete(batch); if (client.session.getState().readerVersion === reader) setHistoryError(null);
    }, (error: unknown) => { batch.finished = false; if (client.session.getState().readerVersion === reader) setHistoryError({ batch, error }); });
  }, [environmentId, threadId, reader]);
  const remove = async (id: string) => {
    const batch = [...batches.current].find((batch) => batch.ids.includes(id));
    const row = queue.rows.find((row) => row.id === id);
    if (batch !== undefined) {
      batch.cancelling = true;
      const attempt = row === undefined ? undefined : batch.attempts.get(row.file);
      if (attempt !== undefined && row !== undefined) { attempt.upload_id = row.uploadId ?? null; if (!batch.saved.has(row.file)) attempt.outcome = row.phase === 'failed' ? 'failed' : 'cancelled'; }
    }
    const removed = await queue.remove(id);
    if (batch !== undefined) {
      batch.cancelling = false;
      if (!removed) return;
      if (!queue.rows.some((row) => row.id !== id && batch.ids.includes(row.id))) {
        batch.reject?.(new DOMException('Upload cancelled', 'AbortError'));
        setConflicts((before) => before.filter((held) => held !== batch));
        finish(batch);
      }
    }
  };
  const removeBatch = async (batch: Batch) => {
    batch.cancelling = true;
    for (const row of queue.rows) if (batch.ids.includes(row.id)) {
      const attempt = batch.attempts.get(row.file);
      if (attempt !== undefined) { attempt.upload_id = row.uploadId ?? null; if (!batch.saved.has(row.file)) attempt.outcome = row.phase === 'failed' ? 'failed' : 'cancelled'; }
    }
    batch.reject?.(new DOMException('Upload cancelled', 'AbortError'));
    setConflicts((before) => before.filter((held) => held !== batch));
    const removed = await Promise.all(batch.ids.map(queue.remove));
    batch.cancelling = false;
    if (removed.every(Boolean)) finish(batch);
  };
  const add = (directory: string, files: readonly File[]) => {
    if (blocked !== false) { toast({ tone: 'info', title: blocked }); return; }
    if (files.length === 0) { toast({ tone: 'info', title: t('uploads.noFiles') }); return; }
    const batch: Batch = { id: crypto.randomUUID(), reader, folder: directory, finished: false, cancelling: false, attempts: new Map(files.map((file) => [file, { name: file.webkitRelativePath || file.name, size: file.size, upload_id: null, blob: null, outcome: 'complete' }])), writes: new Map(), saved: new Set(), choice: null, ids: [], path: directory || '/' };
    batches.current.add(batch);
    const choose = () => {
      batch.choice ??= new Promise<ConflictChoice>((resolve, reject) => {
        batch.resolve = resolve; batch.reject = reject;
        setConflicts((before) => [...before, batch]);
      });
      return batch.choice;
    };
    const attach = async (blob: UploadedBlob, signal: AbortSignal, file: File, report: (work: UploadWork) => void) => {
      const attempt = batch.attempts.get(file);
      if (attempt !== undefined) attempt.blob = blob.blob;
      const path = uploadPath(directory, file);
      signal.throwIfAborted();
      // The native writer creates missing parents under this same job's cancellation and cleanup.
      const stat = await writeUpload(path, signal, (candidate, expected) => {
        let writes = batch.writes.get(file);
        if (writes === undefined) { writes = new Map(); batch.writes.set(file, writes); }
        const key = JSON.stringify([candidate, expected, blob.blob]);
        let held = writes.get(key);
        if (held === undefined || held.terminal === true) { held = { id: crypto.randomUUID() }; writes.set(key, held); }
        return writeUploadedFile(door, candidate, blob.blob, expected, signal, batch.id, report, held);
      }, choose);
      batch.saved.add(file);
      if (attempt !== undefined) attempt.outcome = 'complete';
      // A completed native file stays even if Cancel won before its receipt arrived.
      if (signal.aborted && client.session.getState().readerVersion === reader) void refresh(parentOf(stat.path));
      return stat;
    };
    batch.ids = queue.add(files, undefined, attach);
  };
  useEffect(() => {
    for (const batch of batches.current) {
      for (const row of queue.rows) if (batch.ids.includes(row.id)) {
        const attempt = batch.attempts.get(row.file);
        if (attempt !== undefined) { attempt.upload_id = row.uploadId ?? null; if (row.phase === 'failed' && !batch.saved.has(row.file)) attempt.outcome = 'failed'; }
      }
      if (batch.ids.length > 0 && !queue.rows.some((row) => batch.ids.includes(row.id))) finish(batch);
    }
  }, [queue.rows, finish]);
  useEffect(() => {
    const held = batches.current;
    return () => { for (const batch of held) batch.reject?.(new DOMException('Upload cancelled', 'AbortError')); held.clear(); };
  }, [door.environmentId, door.threadId, reader]);
  const conflict = conflicts.find((batch) => batch.reader === reader);
  const decide = (choice: ConflictChoice) => {
    if (conflict === undefined) return;
    conflict.resolve?.(choice);
    setConflicts((before) => before.filter((held) => held !== conflict));
  };
  return { add, progress: <><UploadChips queue={{ ...queue, retryReason: blocked, remove: (id) => { void remove(id); } }} />{historyError !== null && historyError.batch.reader === reader && <ErrorAlert error={historyError.error} action={<Button size="sm" onClick={() => finish(historyError.batch)}>{t('common.tryAgain')}</Button>} />}</>, dialog: conflict === undefined ? null : <Dialog open onOpenChange={(open) => { if (!open) void removeBatch(conflict); }} title={t('files.upload.conflict')} size="sm" footer={<>
    <Button variant="secondary" onClick={() => { void removeBatch(conflict); }}>{t('common.cancel')}</Button>
    <Button variant="secondary" onClick={() => decide('keep')}>{t('files.upload.keepBoth')}</Button>
    <Button variant="primary" onClick={() => decide('replace')}>{t('files.upload.replace')}</Button>
  </>}><p className="text-sm text-text-muted">{t('files.upload.conflictIn', { path: conflict.path })}</p></Dialog> };
}
