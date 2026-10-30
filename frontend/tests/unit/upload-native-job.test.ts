/** A Files write follows its durable native job; cancelling HTTP never manufactures a stopped receipt. */
import { waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { writeUploadedFile } from '@/api/workspace';
import type { FileStat, Job } from '@/generated/api-types';
import type { UploadWork } from '@/lib/useUploads';

const id = '10000000-0000-0000-0000-000000000001';
const batch = '10000000-0000-0000-0000-000000000002';
const blob = 'a'.repeat(64);
const door = { environmentId: 'workspace', threadId: null };
const before = client.session.getState();
const release: (() => void)[] = [];

function job(state: Job['state'] = 'running'): Job {
  const now = new Date().toISOString();
  return { id, kind: 'delivery', state, node_id: null, environment_id: 'workspace', provider_id: null, batch_id: null,
    result: null, error: null, log: null, created_by: null, created_at: now, started_at: now,
    finished_at: state === 'cancelled' || state === 'succeeded' ? now : null,
    progress: { phase: 'files', phases: [{ name: 'files', state: 'running', weight: 100 }], percent: 12,
      files_done: 0, files_total: 1, bytes_done: 512, bytes_total: 4096, rate_bps: null, eta_seconds: null, message: null } };
}
const stat: FileStat = { type: 'file', path: 'folder/report.bin', name: 'report.bin', size: 4096,
  mtime: new Date().toISOString(), mode: 0o644, link_target: null, cached: true, hash: blob, media_type: 'application/octet-stream', is_text: false };

function fixture(holdCreation = false, refused = false, retainedWrite = false, lostCreation = false) {
  const requests: Request[] = [];
  let missingLookup = false;
  let open: () => void = () => undefined;
  const creation = new Promise<void>((resolve) => { open = resolve; });
  release.push(open);
  const fetch = vi.fn(async (request: Request): Promise<Response> => {
    requests.push(request.clone());
    const path = new URL(request.url).pathname;
    if (request.method === 'POST' && path.endsWith('/write')) {
      if (holdCreation) await creation;
      if (refused) return Response.json({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Denied', detail: 'Denied', request_id: 'req' }, { status: 403 });
      if (lostCreation) throw new TypeError('The admitted write reply was lost');
      return Response.json(job(), { status: 202 });
    }
    if (request.method === 'GET' && path === `/api/v1/jobs/${id}` && lostCreation) {
      if (missingLookup) {
        missingLookup = false;
        return Response.json({ type: 'orch:error:not_found', code: 'not_found', status: 404, title: 'Not found', detail: 'Not found', request_id: 'req' }, { status: 404 });
      }
      return Response.json(job());
    }
    if (path === `/api/v1/jobs/${id}/cancel`) return Response.json({ ...job(), progress: { ...job().progress, activity: { stopping: true, updated_at: new Date().toISOString(), stall_after_seconds: 30, directories_done: null } } });
    if (request.method === 'GET' && path.endsWith('/write')) {
      if (!retainedWrite && queryClient.getQueryData<Job>(['job', id])?.state === 'cancelled') return Response.json({ type: 'orch:error:plan.blocked', code: 'plan.blocked', status: 409, title: 'Stopped', detail: 'Stopped', request_id: 'req' }, { status: 409 });
      return Response.json(stat);
    }
    throw new Error(`Unexpected ${request.method} ${path}`);
  });
  vi.stubGlobal('fetch', fetch);
  return { requests, open, missLookup: () => { missingLookup = true; }, writes: () => requests.filter((r) => r.method === 'POST' && new URL(r.url).pathname.endsWith('/write')),
    stops: () => requests.filter((r) => new URL(r.url).pathname.endsWith('/cancel')),
    receipts: () => requests.filter((r) => r.method === 'GET' && new URL(r.url).pathname.endsWith('/write')) };
}

afterEach(() => {
  for (const done of release.splice(0)) done();
  client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 });
  queryClient.clear();
  client.session.setState(before);
  vi.unstubAllGlobals();
});

it('waits for the job stop proof after a dispatched write, with one stop and no HTTP polling', async () => {
  const server = fixture(), abort = new AbortController(), reports: UploadWork[] = [];
  let settled = false;
  const result = writeUploadedFile(door, stat.path, blob, 'none', abort.signal, batch, (work) => reports.push(work), { id });
  const caught = result.then((value) => { settled = true; return value; }, (error: unknown) => { settled = true; return error; });
  await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(1));
  abort.abort();
  const handle = reports.find((work) => work.stop !== undefined)?.stop;
  if (handle === undefined) throw new Error('No native stop handle');
  const stopped = handle().catch((error: unknown) => error);
  await waitFor(() => expect(server.stops()).toHaveLength(1));
  await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(2));
  expect(reports.filter((work) => work.job !== undefined).at(-1)?.job?.progress.activity?.stopping).toBe(true);
  expect(settled).toBe(false);
  expect(server.receipts()).toHaveLength(0);
  queryClient.setQueryData(['job', id], job('cancelled'));
  expect(await stopped).toBeUndefined();
  expect(await caught).toMatchObject({ name: 'AbortError' });
  expect(server.stops()).toHaveLength(1);
  expect(server.receipts()).toHaveLength(1);
  expect(server.requests.some((r) => new URL(r.url).pathname === `/api/v1/jobs/${id}`)).toBe(false);
  expect(await server.writes()[0]!.json()).toEqual({ path: stat.path, blob, expected_hash: 'none', upload_batch: batch });
});

it('keeps creation alive when Cancel precedes its reply, then stops that exact admitted job', async () => {
  const server = fixture(true), abort = new AbortController();
  let settled = false;
  const result = writeUploadedFile(door, stat.path, blob, 'none', abort.signal, batch, undefined, { id });
  const caught = result.then((value) => { settled = true; return value; }, (error: unknown) => { settled = true; return error; });
  await waitFor(() => expect(server.writes()).toHaveLength(1));
  abort.abort();
  expect(server.writes()[0]!.signal.aborted).toBe(false);
  expect(server.stops()).toHaveLength(0);
  expect(settled).toBe(false);
  server.open();
  await waitFor(() => expect(server.stops()).toHaveLength(1));
  expect(settled).toBe(false);
  queryClient.setQueryData(['job', id], job('cancelled'));
  expect(await caught).toMatchObject({ name: 'AbortError' });
  expect(server.writes()).toHaveLength(1);
});

it('returns a verified result only after completion and streams progress from the shared job query', async () => {
  const server = fixture(), reports: UploadWork[] = [];
  const result = writeUploadedFile(door, stat.path, blob, 'none', new AbortController().signal, batch, (work) => reports.push(work), { id });
  await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(1));
  expect(server.receipts()).toHaveLength(0);
  queryClient.setQueryData(['job', id], { ...job(), progress: { ...job().progress, bytes_done: 1024 } });
  await waitFor(() => expect(reports.some((work) => work.job?.progress.bytes_done === 1024)).toBe(true));
  queryClient.setQueryData(['job', id], job('succeeded'));
  expect(await result).toEqual(stat);
  expect(server.stops()).toHaveLength(0);
  expect(server.receipts()).toHaveLength(1);
});

it('keeps the actual native file receipt when completion crossed Cancel before its reply', async () => {
  const server = fixture(false, false, true), abort = new AbortController();
  const result = writeUploadedFile(door, stat.path, blob, 'none', abort.signal, batch, undefined, { id });
  const caught = result.catch((error: unknown) => error);
  await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(1));
  abort.abort();
  await waitFor(() => expect(server.stops()).toHaveLength(1));
  expect(server.receipts()).toHaveLength(0);
  queryClient.setQueryData(['job', id], job('cancelled'));
  expect(await caught).toEqual(stat);
  expect(server.writes()).toHaveLength(1);
  expect(server.stops()).toHaveLength(1);
  expect(server.receipts()).toHaveLength(1);
});

it('a known completed native job needs no new cancellation request when Cancel crosses its live proof', async () => {
  const server = fixture(), abort = new AbortController();
  const result = writeUploadedFile(door, stat.path, blob, 'none', abort.signal, batch, undefined, { id });
  const caught = result.catch((error: unknown) => error);
  await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(1));
  queryClient.setQueryData(['job', id], job('succeeded'));
  abort.abort();
  expect(await caught).toEqual(stat);
  expect(server.stops()).toHaveLength(0);
  expect(server.writes()).toHaveLength(1);
  expect(server.receipts()).toHaveLength(1);
});

it('recovers an admitted write after its creation reply is lost and retains the actual file after Cancel', async () => {
  const server = fixture(false, false, true, true), abort = new AbortController(), reports: UploadWork[] = [];
  let settled = false;
  const result = writeUploadedFile(door, stat.path, blob, 'none', abort.signal, batch, (work) => reports.push(work), { id });
  const caught = result.then((value) => { settled = true; return value; }, (error: unknown) => { settled = true; return error; });
  await waitFor(() => expect(server.writes()).toHaveLength(1));
  abort.abort();
  const stop = reports.find((work) => work.stop !== undefined)?.stop;
  if (stop === undefined) throw new Error('No native stop handle');
  const stopped = stop().catch((error: unknown) => error);
  try {
    await waitFor(() => expect(server.stops()).toHaveLength(1));
    expect(settled).toBe(false);
    expect(server.receipts()).toHaveLength(0);
    queryClient.setQueryData(['job', id], { ...job('cancelled'), result: { ...stat } });
    expect(await stopped).toBeUndefined();
    expect(await caught).toEqual(stat);
    expect(server.writes()).toHaveLength(1);
    expect(server.receipts()).toHaveLength(1);
    expect(server.requests.filter((request) => new URL(request.url).pathname === `/api/v1/jobs/${id}`)).toHaveLength(1);
  } finally {
    queryClient.setQueryData(['job', id], { ...job('cancelled'), result: { ...stat } });
    await stopped;
    await caught;
  }
});

it('an unconfirmed admission lookup stays retryable and never substitutes for the machine stop proof', async () => {
  const server = fixture(false, false, false, true), reports: UploadWork[] = [];
  server.missLookup();
  const result = writeUploadedFile(door, stat.path, blob, 'none', new AbortController().signal, batch, (work) => reports.push(work), { id });
  await expect(result).rejects.toMatchObject({ problem: { status: 404 } });
  const stop = reports.find((work) => work.stop !== undefined)?.stop;
  if (stop === undefined) throw new Error('No native stop handle');
  let settled = false;
  const stopped = stop().then(() => { settled = true; }).catch((error: unknown) => error);
  try {
    await waitFor(() => expect(server.stops()).toHaveLength(1));
    expect(settled).toBe(false);
    expect(server.requests.filter((request) => new URL(request.url).pathname === `/api/v1/jobs/${id}`)).toHaveLength(2);
    expect(server.writes()).toHaveLength(1);
    queryClient.setQueryData(['job', id], job('cancelled'));
    expect(await stopped).toBeUndefined();
    expect(settled).toBe(true);
  } finally { queryClient.setQueryData(['job', id], job('cancelled')); await stopped; }
});

it('a requesting reader leaving detaches the observer without stopping or reading as the next person', async () => {
  const server = fixture();
  const result = writeUploadedFile(door, stat.path, blob, 'none', new AbortController().signal, batch, undefined, { id });
  const caught = result.catch((error: unknown) => error);
  await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(1));
  client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1, userId: 'next-person' });
  expect(await caught).toMatchObject({ name: 'AbortError' });
  expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(0);
  expect(server.stops()).toHaveLength(0);
  expect(server.receipts()).toHaveLength(0);
});

it('a fresh explicit admission refusal needs no invented machine stop', async () => {
  const server = fixture(false, true), reports: UploadWork[] = [];
  const result = writeUploadedFile(door, stat.path, blob, 'none', new AbortController().signal, batch, (work) => reports.push(work), { id });
  await expect(result).rejects.toMatchObject({ problem: { code: 'authz.denied' } });
  const stop = reports.find((work) => work.stop !== undefined)?.stop;
  if (stop === undefined) throw new Error('No native stop handle');
  await stop();
  expect(server.writes()).toHaveLength(1);
  expect(server.stops()).toHaveLength(0);
  expect(server.receipts()).toHaveLength(0);
});
