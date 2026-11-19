/** API client for Files with rename, copy and the search inside them, what an archive holds, snapshots with their archives, Git and attached mounts of an environment.
 *
 * What the files, the changes and the Git views of a thread ask of the API. Every request names the
 * environment and, as the way in, the thread the person works from: a thread is a door into the
 * workspace of its environment, and what it lets its reader do is on its own page. A file is read in
 * two steps (the environment is asked to bring that version to the store, then the bytes are read
 * from it by the hash) and written on the version that was read, so that a file that moved
 * meanwhile is shown to the writer and never overwritten.
 *
 * So far: listing, describing, reading, writing, making, renaming, copying and deleting, the search
 * by name and inside files, what an archive holds, the changes of the threads of an environment,
 * Git status and differences, and the environment itself. Snapshots and mounts are not asked for here.
 */
import { queryOptions } from '@tanstack/react-query';

import { ApiError, api, client, done, ok, repeatable } from '@/api/client';
import { liveValue } from '@/api/liveValue';
import { queryClient } from '@/api/query';
import type { UploadWork } from '@/lib/useUploads';
import { waitForJob } from '@/realtime/queryFeeds';
import type {
  ArchiveListing,
  DirListing,
  Environment,
  EnvironmentChange,
  EnvironmentUploadFinishBody,
  FetchState,
  FileListSource,
  FileMatch,
  FileStat,
  GitStatus,
  Job,
} from '@/generated/api-types';

/** The way into an environment's workspace: the environment, and the thread that lets the person in. */
export interface Door {
  environmentId: string;
  /** The thread the request is made through; `null` for somebody who manages the environment itself. */
  threadId: string | null;
}

/** The answer of a request whose body is bytes or text: a refusal is thrown as the problem it carries. */
async function bodyOf(answer: Promise<{ data?: unknown; error?: unknown; response: Response }>): Promise<{ data: unknown; response: Response }> {
  const result = await answer;
  await done(Promise.resolve(result));
  return { data: result.data, response: result.response };
}

const through = (door: Door): { thread?: string } => (door.threadId === null ? {} : { thread: door.threadId });
const pathOf = (door: Door) => ({ id: door.environmentId });

export const workspaceKeys = {
  environment: (id: string) => ['environment', id] as const,
  stat: (door: Door, path: string) => ['workspace-stat', door.environmentId, path] as const,
  changes: (door: Door, all: boolean) => ['workspace-changes', door.environmentId, door.threadId, all] as const,
  git: (door: Door) => ['workspace-git', door.environmentId, door.threadId] as const,
  gitDiff: (door: Door, staged: boolean) => ['workspace-git-diff', door.environmentId, door.threadId, staged] as const,
  archive: (door: Door, path: string, inside: string) => ['workspace-archive', door.environmentId, path, inside] as const,
  search: (door: Door, text: string, where: string) => ['workspace-search', door.environmentId, text, where] as const,
};

// ---------------------------------------------------------------- the environment

/** The environment as its caller may see it: where it runs, its state, its switches. */
export { environmentQuery } from '@/api/environments';

/** Starts a stopped environment that Orch made; the job that does it is the answer. */
export function startEnvironment(id: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/start', { params: { path: { id } } }));
}

/** Stops an environment that Orch made, ending the turns that run in it only when asked to. */
export function stopEnvironment(id: string, interrupt: boolean): Promise<Environment> {
  return ok(api.POST('/environments/{id}/stop', { params: { path: { id } }, body: { interrupt } }));
}

/** Asks that the environment not be stopped for being idle: whoever manages the environment may. The door is kept
 * for the callers' sake; keeping an environment awake is managing it, whichever thread it is reached from. */
export function keepEnvironmentAwake(id: string, _door?: Door): Promise<Environment> {
  return ok(api.POST('/environments/{id}/keep-awake', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- looking

/** What lies directly in a directory: live when the environment is online, else from the last list of its tree. */
export function listDirectory(door: Door, path: string, options: { source?: FileListSource; cursor?: string | null; limit?: number; signal?: AbortSignal } = {}): Promise<DirListing> {
  return ok(
    api.GET('/environments/{id}/files', {
      params: {
        path: pathOf(door),
        query: { path, ...through(door), ...(options.source === undefined ? {} : { source: options.source }),
          ...(options.cursor === undefined ? {} : { cursor: options.cursor }), ...(options.limit === undefined ? {} : { limit: options.limit }) },
      },
      ...(options.signal === undefined ? {} : { signal: options.signal }),
    }),
  );
}

/** What one path is: its kind, size, time, media type and, where it is known, its hash. */
export function statFile(door: Door, path: string, signal?: AbortSignal): Promise<FileStat> {
  return ok(
    api.GET('/environments/{id}/files/stat', {
      params: { path: pathOf(door), query: { path, ...through(door) } },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** The longest the API is asked to hold an answer while the environment brings a file: the most it allows. */
export const FETCH_WAIT_SECS = 25;

/** Asks that the exact version of a file be in the store, and says whether it is, with the hash that names it. */
export function fetchFile(door: Door, path: string, signal?: AbortSignal): Promise<FetchState> {
  return ok(
    api.POST('/environments/{id}/files/fetch', {
      params: { path: pathOf(door), query: { ...through(door), wait: FETCH_WAIT_SECS } },
      body: { path },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** A reading of a file's bytes: how many came, and how many there are in all. */
export interface FileBytes {
  bytes: Uint8Array;
  /** The size of the whole file when the answer was cut to a range, else the length of what came. */
  total: number;
}

/** The bytes of a version of a file, all of them or the first `limit`. */
export async function readFileBytes(door: Door, path: string, hash: string, limit?: number, signal?: AbortSignal): Promise<FileBytes> {
  const result = await api.GET('/environments/{id}/files/content', {
    params: { path: pathOf(door), query: { path, hash, ...through(door) } },
    ...(limit === undefined ? {} : { headers: { Range: `bytes=0-${limit - 1}` } }),
    parseAs: 'arrayBuffer',
    ...(signal === undefined ? {} : { signal }),
  });
  // No range of a file with nothing in it can be had: the server says so with `416` and `bytes */0`, and that is the whole answer.
  if (result.response.status === 416 && /\/0\s*$/.test(result.response.headers.get('Content-Range') ?? '')) return { bytes: new Uint8Array(0), total: 0 };
  const answer = await bodyOf(Promise.resolve(result));
  const bytes = new Uint8Array(answer.data as ArrayBuffer);
  const range = /\/(\d+)\s*$/.exec(answer.response.headers.get('Content-Range') ?? '');
  return { bytes, total: range?.[1] === undefined ? bytes.length : Number(range[1]) };
}

/** Writes bytes (or text) into a file on the version that was read; `'none'` says the file must not exist yet. */
export function writeFile(door: Door, path: string, content: string | Uint8Array, expectedHash: string | null): Promise<FileStat> {
  return ok(
    api.PUT('/environments/{id}/files/content', {
      params: {
        path: pathOf(door),
        query: { path, ...through(door), ...(expectedHash === null ? {} : { expected_hash: expectedHash }) },
      },
      body: content as unknown as string,
      // The bytes go as they are: the default would write them as JSON.
      bodySerializer: (body: unknown) => body as BodyInit,
      headers: { 'Content-Type': 'application/octet-stream' },
    }),
  );
}

/** One write identity survives lost replies; only refusal or an ended write without a saved file permits a fresh retry. */
export interface UploadWriteAttempt { id: string; sent?: boolean; terminal?: boolean }

/** A completed upload is a durable native job; an HTTP abort never substitutes for its stopped result. */
export async function writeUploadedFile(door: Door, path: string, blob: string, expectedHash: string | null, signal: AbortSignal, uploadBatch?: string, report?: (work: UploadWork) => void, attempt: UploadWriteAttempt = { id: crypto.randomUUID() }): Promise<FileStat> {
  signal.throwIfAborted();
  const reader = client.session.getState().readerVersion;
  const current = () => { if (client.session.getState().readerVersion !== reader) throw new DOMException('Upload reader changed', 'AbortError'); };
  const fresh = attempt.sent !== true;
  attempt.sent = true;
  // Creation must settle even when Cancel arrives before its reply; the durable identity is already known.
  const creation = ok(api.POST('/environments/{id}/files/uploads/{upload_id}/write', {
    params: { path: { ...pathOf(door), upload_id: attempt.id }, query: through(door) },
    body: { path, blob, upload_batch: uploadBatch ?? attempt.id, ...(expectedHash === null ? {} : { expected_hash: expectedHash }) },
    ...repeatable(attempt.id),
  }));
  let admissionRefused = false;
  let recovered: Promise<Job> | undefined;
  const admission = () => creation.catch((error: unknown) => {
    current();
    if (fresh && error instanceof ApiError && error.problem.status >= 400 && error.problem.status < 500 && error.problem.status !== 408) {
      admissionRefused = true;
      attempt.terminal = true;
      throw error;
    }
    // Keep a recovered admission, but allow a failed lookup to be tried by the next explicit Stop or Retry.
    return recovered ??= ok(api.GET('/jobs/{id}', { params: { path: { id: attempt.id } } }))
      .catch((error: unknown) => { recovered = undefined; throw error; });
  });
  let stoppingActivity: Job['progress']['activity'];
  const changed = (job: Job) => {
    if (client.session.getState().readerVersion !== reader) return;
    // The accepted stop is irreversible. An older cached snapshot must not hide its wait allowance.
    const shown = stoppingActivity !== undefined && stoppingActivity !== null && (job.state === 'queued' || job.state === 'running')
      ? { ...job, progress: { ...job.progress, activity: stoppingActivity } } : job;
    report?.({ job: shown });
  };
  const follow = async (job: Job) => {
    current();
    const left = new AbortController();
    const unlisten = client.session.subscribe((state) => { if (state.readerVersion !== reader) left.abort(); });
    try { return await waitForJob(queryClient, job, changed, left.signal); }
    finally { unlisten(); left.abort(); }
  };
  let stopping: Promise<void> | null = null;
  const stop = async (): Promise<void> => {
    current();
    stopping ??= (async () => {
      const admitted = await admission().catch((error: unknown) => { if (admissionRefused) return null; throw error; });
      if (admitted === null) return;
      current();
      const terminal = (job: Job) => job.state === 'succeeded' || job.state === 'failed' || job.state === 'cancelled';
      const cached = queryClient.getQueryData<Job>(['job', admitted.id]);
      const known = terminal(admitted) ? admitted : cached !== undefined && terminal(cached) ? cached : admitted;
      const asked = terminal(known) ? known : await ok(api.POST('/jobs/{id}/cancel', { params: { path: { id: admitted.id } } }));
      if (asked.progress.activity?.stopping === true) stoppingActivity = asked.progress.activity;
      const ended = await follow(asked);
      attempt.terminal = (ended.state === 'failed' || ended.state === 'cancelled') && ended.result === null;
    })().catch((error: unknown) => { stopping = null; throw error; });
    return stopping;
  };
  report?.({ stop });
  const abort = () => { void stop().catch(() => undefined); };
  signal.addEventListener('abort', abort, { once: true });
  if (signal.aborted) abort();
  try {
    const admitted = await admission();
    current();
    const ended = await follow(admitted);
    current();
    attempt.terminal = (ended.state === 'failed' || ended.state === 'cancelled') && ended.result === null;
    if (signal.aborted) await stop();
    try {
      return await ok(api.GET('/environments/{id}/files/uploads/{upload_id}/write', { params: { path: { ...pathOf(door), upload_id: ended.id }, query: through(door) } }));
    } catch (error) {
      if (ended.state === 'cancelled' && error instanceof ApiError && error.is('plan.blocked')) throw new DOMException('Upload cancelled', 'AbortError');
      throw error;
    }
  } catch (error) {
    if (signal.aborted) await stop();
    throw error;
  } finally {
    signal.removeEventListener('abort', abort);
  }
}

/** Records one batch outcome, including retained receipts for cancelled partial uploads. */
export async function finishWorkspaceUpload(door: Door, id: string, body: EnvironmentUploadFinishBody): Promise<void> {
  await ok(api.POST('/environments/{id}/files/uploads/{upload_id}/finish', { params: { path: { ...pathOf(door), upload_id: id }, query: through(door) }, body, ...repeatable(id) }));
}

/** The file as it stands, which a refused write carries so that the writer is shown the newer version. */
export function currentOf(error: unknown): FileStat | null {
  if (!(error instanceof ApiError) || !error.is('conflict.stale_revision')) return null;
  const current: unknown = (error.problem as unknown as Record<string, unknown>).current;
  return typeof current === 'object' && current !== null ? (current as FileStat) : null;
}

export function makeDirectory(door: Door, path: string): Promise<FileStat> {
  return ok(api.POST('/environments/{id}/files/mkdir', { params: { path: pathOf(door), query: through(door) }, body: { path } }));
}

/** Moves a file or a directory; it is also how one is renamed. */
export function renameFile(door: Door, from: string, to: string): Promise<FileStat> {
  return ok(api.POST('/environments/{id}/files/rename', { params: { path: pathOf(door), query: through(door) }, body: { from, to } }));
}

export function copyFile(door: Door, from: string, to: string): Promise<FileStat> {
  return ok(api.POST('/environments/{id}/files/copy', { params: { path: pathOf(door), query: through(door) }, body: { from, to } }));
}

export function deleteFile(door: Door, path: string, recursive: boolean): Promise<void> {
  return done(
    api.DELETE('/environments/{id}/files', { params: { path: pathOf(door), query: { path, recursive, ...through(door) } } }),
  );
}

// ---------------------------------------------------------------- searching

/** How many names the search by name lists at most: what the API holds to. */
export const NAME_SEARCH_MAX = 200;
/** How many lines the search inside files asks for: the API's own default. */
export const CONTENT_SEARCH_LIMIT = 200;

export function searchNames(door: Door, text: string, signal?: AbortSignal) {
  return ok(
    api.GET('/environments/{id}/files/search', {
      params: { path: pathOf(door), query: { q: text, in: 'names', ...through(door) } },
      ...(signal === undefined ? {} : { signal }),
    }),
  ) as Promise<DirListing['entries']>;
}

export interface ContentSearchOptions {
  /** A directory to stay in; the thread's own unless given. */
  under?: string;
  /** Which file names. */
  glob?: string;
  regex?: boolean;
  caseSensitive?: boolean;
}

export interface ContentMatches {
  matches: FileMatch[];
  truncated: boolean;
  searched: number;
}

/** The lines that hold a word or a pattern, file by file. The environment has to be online. */
export function searchContent(door: Door, text: string, options: ContentSearchOptions = {}, signal?: AbortSignal): Promise<ContentMatches> {
  return ok(
    api.GET('/environments/{id}/files/search', {
      params: {
        path: pathOf(door),
        query: {
          q: text,
          in: 'content',
          limit: CONTENT_SEARCH_LIMIT,
          ...through(door),
          ...(options.under === undefined ? {} : { path: options.under }),
          ...(options.glob === undefined || options.glob === '' ? {} : { glob: options.glob }),
          ...(options.regex === true ? { regex: true } : {}),
          ...(options.caseSensitive === true ? { case: true } : {}),
        },
      },
      ...(signal === undefined ? {} : { signal }),
    }),
  ) as Promise<ContentMatches>;
}

// ---------------------------------------------------------------- archives

/** What an archive holds, read where the file lies and without unpacking it. */
export const archiveQuery = (door: Door, path: string, inside: string) =>
  queryOptions({
    queryKey: workspaceKeys.archive(door, path, inside),
    queryFn: ({ signal }): Promise<ArchiveListing> =>
      ok(
        api.GET('/environments/{id}/files/archive', {
          params: { path: pathOf(door), query: { path, ...(inside === '' ? {} : { inside }), ...through(door) } },
          signal,
        }),
      ),
  });

/** One entry of an archive, for looking at it: at most the preview limit. */
export async function readArchiveEntry(door: Door, path: string, entry: string, signal?: AbortSignal): Promise<Uint8Array> {
  const answer = await bodyOf(
    api.GET('/environments/{id}/files/archive/entry', {
      params: { path: pathOf(door), query: { path, entry, ...through(door) } },
      parseAs: 'arrayBuffer',
      ...(signal === undefined ? {} : { signal }),
    }),
  );
  return new Uint8Array(answer.data as ArrayBuffer);
}

// ---------------------------------------------------------------- what changed

/** The files the threads' tool calls changed, the latest change of each path, newest first: this thread's, or every thread's of the environment. */
export const changesQuery = (door: Door, all: boolean) =>
  queryOptions({
    queryKey: workspaceKeys.changes(door, all),
    queryFn: async ({ signal }): Promise<EnvironmentChange[]> => {
      const reply = await ok(
        api.GET('/environments/{id}/changes', {
          params: { path: pathOf(door), query: { ...through(door), ...(all ? { all: true } : {}) } },
          signal,
        }),
      );
      return [...reply.changes].sort((one, other) => Date.parse(other.at) - Date.parse(one.at));
    },
  });

/** Git status of the repository at `path` (the root when empty). Best effort: the reason is in the answer. */
export const gitStatusQuery = (door: Door) =>
  queryOptions({
    queryKey: workspaceKeys.git(door),
    // The server reads the one repository at the root of the workspace and refuses a path that is not the root.
    queryFn: ({ signal }): Promise<GitStatus> => liveValue(workspaceKeys.git(door), signal),
  });

/** The unified difference of the repository at the root of the workspace (what is staged, or what is not), and whether it was cut at its limit. */
export async function gitDiff(door: Door, staged: boolean, signal?: AbortSignal): Promise<{ text: string; truncated: boolean }> {
  const answer = await bodyOf(
    api.GET('/environments/{id}/git/diff', {
      params: { path: pathOf(door), query: { ...through(door), ...(staged ? { staged: true } : {}) } },
      parseAs: 'text',
      ...(signal === undefined ? {} : { signal }),
    }),
  );
  return { text: answer.data as string, truncated: answer.response.headers.get('X-Truncated') === '1' };
}

/** What the difference of the whole repository says of one file: its sections, in the order Git wrote them (a renamed file is in the section of its old and of its new name). */
export function partOfDiff(text: string, path: string): string {
  return text
    .split(/^(?=diff --git )/m)
    .filter((section) => {
      const head = section.split('\n', 1)[0] ?? '';
      return head.endsWith(` b/${path}`) || head.startsWith(`diff --git a/${path} `);
    })
    .join('');
}

// ---------------------------------------------------------------- the thread's own settings

/** Gives a value set on the thread back to what it inherits; the parameters panel's way back. */
export function clearThreadSetting(threadId: string, key: string, expectedRevision?: number): Promise<void> {
  return ok(api.DELETE('/nodes/{id}/settings/{key}', { params: { path: { id: threadId, key }, ...(expectedRevision === undefined ? {} : { query: { expected_revision: expectedRevision } }) } })).then(() => undefined);
}
