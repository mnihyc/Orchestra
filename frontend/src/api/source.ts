/** API client for the source of a thread's work: a recorded state of its files, a restore planned and then confirmed, a commit and a push in Git, the thread's links, a source prepared again, and the comments on lines kept in the private draft.
 *
 * Every act that writes is one request made safe to repeat with the key the person's press made: a
 * plan is confirmed by its own word (`preflight_id`), and a job that was accepted is followed, never
 * asked for twice. What the server refuses comes back as the problem it says.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import type { Door } from '@/api/workspace';
import type { ChangeOptions, ContentPart, Delivery, Draft, EnvironmentGitCommitBody, EnvironmentGitPushBody, Job, RepositorySource, RestorePlan, ReviewComment, Snapshot, ThreadLink } from '@/generated/api-types';

const through = (door: Door): { thread?: string } => (door.threadId === null ? {} : { thread: door.threadId });

export const sourceKeys = {
  snapshot: (id: string) => ['source-snapshot', id] as const,
};

/** One recorded state of files: whether it is complete, where it was taken, and why it may not be had. */
export const snapshotQuery = (id: string) =>
  queryOptions({
    queryKey: sourceKeys.snapshot(id),
    queryFn: (): Promise<Snapshot> => ok(api.GET('/snapshots/{id}', { params: { path: { id } } })),
    staleTime: Infinity,
  });

/** Asks for the plan of putting a recorded state back: what each path would undergo, who wrote since, and what stops it. Nothing is written. */
export function planRestore(environmentId: string, snapshotId: string, options: ChangeOptions): Promise<RestorePlan> {
  return ok(api.POST('/environments/{id}/restores/preflight', { params: { path: { id: environmentId } }, body: { snapshot_id: snapshotId, options } }));
}

/** Confirms one plan as it was shown: the job that writes it, keeping a complete state to undo it with. */
export function restoreFiles(environmentId: string, preflightId: string, key: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/restores', { params: { path: { id: environmentId } }, body: { preflight_id: preflightId }, ...repeatable(key) }));
}

/** Commits the chosen paths of one repository as the caller, against the head and status that were read. */
export function commitFiles(door: Door, body: EnvironmentGitCommitBody, key: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/git/commit', { params: { path: { id: door.environmentId }, query: through(door) }, body, ...repeatable(key) }));
}

/** Pushes one branch to one remote, never by force and never pulling first. */
export function pushBranch(door: Door, body: EnvironmentGitPushBody, key: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/git/push', { params: { path: { id: door.environmentId }, query: through(door) }, body, ...repeatable(key) }));
}

/** Replaces the thread's links in one act. */
export function saveThreadLinks(threadId: string, links: ThreadLink[]): Promise<ThreadLink[]> {
  return ok(api.PUT('/threads/{id}/links', { params: { path: { id: threadId } }, body: { links } }));
}

/** Prepares the thread's source again after it failed; with `source`, accepts the one that resolves now instead of the one it was made with. */
export function prepareSource(environmentId: string, threadId: string, key: string, source?: RepositorySource): Promise<Job> {
  return ok(api.POST('/environments/{id}/source/prepare', { params: { path: { id: environmentId } }, body: { thread_id: threadId, ...(source === undefined ? {} : { source }) }, ...repeatable(key) }));
}

/**
 * Empties the draft once what it held was sent or erased, keeping the comments on lines that were not sent with
 * it: a draft with none is deleted, one with some keeps them under empty text (omitted comments stay as they were).
 */
export async function clearDraftText(threadId: string): Promise<void> {
  const draft = await ok(api.GET('/threads/{id}/draft', { params: { path: { id: threadId } } }));
  if (draft !== null && draft.review_comments.length > 0) {
    await ok(api.PUT('/threads/{id}/draft', { params: { path: { id: threadId } }, body: { content: [], delivery: draft.delivery } }));
    return;
  }
  await done(api.DELETE('/threads/{id}/draft', { params: { path: { id: threadId } } }));
}

/** Writes the comments on lines into the private draft, keeping its text as it stands. */
export function saveReviewComments(threadId: string, content: ContentPart[], delivery: Delivery, comments: ReviewComment[]): Promise<Draft> {
  return ok(api.PUT('/threads/{id}/draft', { params: { path: { id: threadId } }, body: { content, delivery, review_comments: comments } }));
}
