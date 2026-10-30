/** A queue of resumable files with progress, Retry and Remove, shared by composers and documents. */
import { useCallback, useEffect, useRef, useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';

import type { Job, UploadedBlob } from '@/generated/api-types';
import { UploadTask } from '@/lib/uploads';
import { useViewWork } from '@/features/workbench/viewOpening';

export interface QueuedUpload {
  id: string;
  file: File;
  received: number;
  phase: 'waiting' | 'uploading' | 'writing' | 'stopping' | 'failed';
  work?: Job;
  error?: unknown;
  uploadId?: string | null;
  kind?: 'pasted_text';
  characters?: number;
  replaces?: string;
}

/** Native writes keep their own measured progress until the machine has stopped. */
export interface UploadWork { job?: Job; stop?: () => Promise<void> }
interface Work<T> { task: UploadTask; abort: AbortController; running: boolean; pending: boolean; done?: Promise<void>; finish?: () => void; stop?: () => Promise<void>; cancelling?: Promise<boolean>; attach: (blob: UploadedBlob, signal: AbortSignal, report: (work: UploadWork) => void) => Promise<T>; publish: (part: T) => void }

/** Keeps failed tasks for Retry, and calls publish only after the whole file is attached. */
export function useUploads<T>(source: string, attach: (blob: UploadedBlob, signal: AbortSignal) => Promise<T>, publish: (part: T) => void) {
  const reader = useStore(client.session, (state) => state.readerVersion);
  const scope = `${source}:${reader ?? 0}`;
  const [state, setState] = useState<{ scope: string; rows: QueuedUpload[]; completed: number; bytes: number }>({ scope, rows: [], completed: 0, bytes: 0 });
  const work = useRef(new Map<string, Work<T>>());
  const callbacks = useRef({ attach, publish });
  const next = useRef<() => void>(() => undefined);
  useEffect(() => { callbacks.current = { attach, publish }; }, [attach, publish]);
  const change = useCallback((update: (rows: QueuedUpload[]) => QueuedUpload[], completed?: File) => {
    if (client.session.getState().readerVersion !== reader) return;
    setState((held) => {
      const sameBatch = held.scope === scope && held.rows.length > 0;
      return { scope, rows: update(held.scope === scope ? held.rows : []), completed: (sameBatch ? held.completed : 0) + (completed === undefined ? 0 : 1), bytes: (sameBatch ? held.bytes : 0) + (completed?.size ?? 0) };
    });
  }, [reader, scope]);
  const run = useCallback(async (id: string) => {
    const job = work.current.get(id);
    if (job === undefined || job.running) return;
    job.running = true;
    job.pending = false;
    job.done = new Promise<void>((resolve) => { job.finish = resolve; });
    const abort = job.abort;
    change((rows) => rows.map((row) => row.id === id ? { ...row, phase: 'uploading', error: undefined } : row));
    try {
      const blob = await job.task.run(abort.signal, (received) => change((rows) => rows.map((row) => row.id === id ? { ...row, received, uploadId: job.task.uploadId } : row)));
      if (client.session.getState().readerVersion !== reader) return;
      const part = await job.attach(blob, abort.signal, (work) => {
        if (work.stop !== undefined) job.stop = work.stop;
        change((rows) => rows.map((row) => row.id === id ? { ...row, phase: abort.signal.aborted ? 'stopping' : 'writing', work: work.job ?? (work.stop === undefined ? row.work : undefined) } : row));
      });
      abort.signal.throwIfAborted();
      if (client.session.getState().readerVersion !== reader) return;
      job.publish(part);
      work.current.delete(id);
      change((rows) => rows.filter((row) => row.id !== id), job.task.file);
    } catch (error) {
      if (abort.signal.aborted) return;
      change((rows) => rows.map((row) => row.id === id ? { ...row, phase: 'failed', error } : row));
    } finally {
      job.running = false;
      job.finish?.();
      next.current();
    }
  }, [change, reader]);
  // One adaptive stream per destination bounds memory for folder drops without bounding their file count.
  const startNext = useCallback(() => {
    if ([...work.current.values()].some((job) => job.running || job.cancelling !== undefined || job.abort.signal.aborted && job.stop !== undefined)) return;
    const queued = [...work.current].find(([, job]) => job.pending);
    if (queued !== undefined) void run(queued[0]);
  }, [run]);
  useEffect(() => { next.current = startNext; }, [startNext]);
  const add = useCallback((files: Iterable<File>, publishOne?: (part: T) => void, attachOne?: (blob: UploadedBlob, signal: AbortSignal, file: File, report: (work: UploadWork) => void) => Promise<T>, metadata?: Pick<QueuedUpload, 'kind' | 'characters' | 'replaces'>) => {
    const ids: string[] = [];
    for (const file of files) {
      const id = crypto.randomUUID();
      ids.push(id);
      work.current.set(id, { task: new UploadTask(file), abort: new AbortController(), running: false, pending: true, attach: attachOne === undefined ? callbacks.current.attach : (blob, signal, report) => attachOne(blob, signal, file, report), publish: publishOne ?? callbacks.current.publish });
      change((rows) => [...rows, { id, file, received: 0, phase: 'waiting', ...metadata }]);
    }
    startNext();
    return ids;
  }, [change, startNext]);
  const remove = useCallback((id: string) => {
    const job = work.current.get(id);
    if (job === undefined) return Promise.resolve(true);
    if (job.cancelling !== undefined) return job.cancelling;
    job.pending = false;
    job.abort.abort();
    change((rows) => rows.map((row) => row.id === id ? { ...row, phase: 'stopping' } : row));
    job.cancelling = (async () => {
      try {
        // A settled HTTP request is not a native stop receipt. Keep the row, its content and the
        // next transfer behind the explicit native proof, then join the attachment callback.
        await job.stop?.();
        await job.done;
        await job.task.cancel();
        if (work.current.get(id) === job) work.current.delete(id);
        change((rows) => rows.filter((row) => row.id !== id));
        next.current();
        return true;
      } catch (error) {
        job.cancelling = undefined;
        change((rows) => rows.map((row) => row.id === id ? { ...row, phase: 'stopping', error } : row));
        return false;
      }
    })();
    return job.cancelling;
  }, [change]);
  const pause = useCallback((id: string) => {
    const job = work.current.get(id);
    if (job === undefined || job.abort.signal.aborted) return;
    job.pending = false;
    job.abort.abort();
    void job.task.cancel().catch(() => undefined);
    change((rows) => rows.map((row) => row.id === id ? { ...row, phase: 'waiting', received: 0 } : row));
  }, [change]);
  const revise = useCallback((id: string, file: File, attachOne: Work<T>['attach'], metadata?: Pick<QueuedUpload, 'kind' | 'characters'>) => {
    const previous = work.current.get(id);
    if (previous === undefined) return;
    previous.abort.abort();
    void previous.task.cancel().catch(() => undefined);
    work.current.set(id, { task: new UploadTask(file), abort: new AbortController(), running: false, pending: true, attach: attachOne, publish: previous.publish });
    change((rows) => rows.map((row) => row.id === id ? { ...row, file, phase: 'waiting', received: 0, uploadId: null, error: undefined, ...metadata } : row));
    startNext();
  }, [change, startNext]);
  useEffect(() => {
    const held = work.current;
    return () => {
      for (const job of held.values()) {
        job.abort.abort();
        void (async () => { await job.stop?.(); await job.done; await job.task.cancel(); })().catch(() => undefined);
      }
      held.clear();
    };
  }, [scope]);
  const rows = state.scope === scope ? state.rows : [];
  const progress = { completed: state.scope === scope ? state.completed : 0, total: rows.length + (state.scope === scope ? state.completed : 0), received: rows.reduce((bytes, row) => bytes + row.received, state.scope === scope ? state.bytes : 0), size: rows.reduce((bytes, row) => bytes + row.file.size, state.scope === scope ? state.bytes : 0) };
  useViewWork(rows.length > 0);
  useEffect(() => {
    if (rows.length === 0) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [rows.length]);
  return { rows, progress, add, remove, pause, revise, retry: (id: string) => { const job = work.current.get(id); if (job !== undefined && !job.running) { job.pending = true; change((rows) => rows.map((row) => row.id === id ? { ...row, phase: 'waiting' } : row)); startNext(); } }, blocked: rows.length > 0 };
}
