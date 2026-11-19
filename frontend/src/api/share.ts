/** API client for Sharing with agents: a thing given to the agents of a thread or a place as a copy or as a share that stays, what was shared and with whom, what an agent's reach holds, and taking a share back.
 *
 * A live share is entries on the page of what is shared and a copy is files in an agent's workspace; both
 * are made by one request that is all or nothing over the threads or places named, and both are listed
 * on the page of what was shared. What the agents of a place know beyond their own threads is listed on
 * that place, and an agent's whole reach is read through its thread as the tree its own tools list.
 */
import { queryOptions } from '@tanstack/react-query';

import { type AccessKind, pathOf } from '@/api/access';
import { api, done, ok } from '@/api/client';
import type {
  AgentShare,
  ShareAgentCreateReply,
  ShareAgentUpdateBody,
  ShareWithAgents,
  ThreadSharedGetReply,
  ThreadSharedTreeGetReply,
} from '@/generated/api-types';

/** The kinds of thing whose shares with agents are listed on their page: a node, a document and a chat. */
export type SharedKind = Extract<AccessKind, 'org' | 'project' | 'topic' | 'thread' | 'document' | 'chat'>;

/** Whether a kind of thing can be shared with agents. */
export const canShareWithAgents = (kind: AccessKind): kind is SharedKind =>
  kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread' || kind === 'document' || kind === 'chat';

export const agentSharesKey = (kind: AccessKind, id: string, side: 'of' | 'to') => ['agent-shares', pathOf(kind), id, side] as const;

/**
 * What was shared of a thing (`of`: the live shares and the copies) or, for a place, what was shared with
 * the agents of that place or of a place above it (`to`): what its agents know beyond their own threads.
 */
export const agentSharesQuery = (target: { kind: AccessKind; id: string }, side: 'of' | 'to') =>
  queryOptions({
    queryKey: agentSharesKey(target.kind, target.id, side),
    queryFn: ({ signal }): Promise<AgentShare[]> =>
      ok(api.GET('/{object}/{id}/agent-shares', { params: { path: { object: pathOf(target.kind), id: target.id }, query: { side } }, signal })),
  });

/** Shares a thing with agents; all or nothing over the threads or places named. The body carries the key that makes a repeated request safe. */
export function shareWithAgents(body: ShareWithAgents): Promise<ShareAgentCreateReply> {
  return ok(api.POST('/share/agents', { body }));
}

/** Changes the areas or the end of a live share; what is left out stays. */
export function changeAgentShare(shareId: string, body: ShareAgentUpdateBody): Promise<AgentShare> {
  return ok(api.PATCH('/share/agents/{share_id}', { params: { path: { share_id: shareId } }, body }));
}

/** Takes a share back: a live one ends at the agent's next call, a copy is removed from the workspace. */
export function stopAgentShare(shareId: string): Promise<void> {
  return done(api.DELETE('/share/agents/{share_id}', { params: { path: { share_id: shareId } } }));
}

/** What a thread's agent may read beyond its own thread, and the copies it was given. */
export const threadSharedQuery = (threadId: string) =>
  queryOptions({
    queryKey: ['thread-shared', threadId] as const,
    queryFn: ({ signal }): Promise<ThreadSharedGetReply> => ok(api.GET('/threads/{id}/shared', { params: { path: { id: threadId } }, signal })),
  });

/** An agent's reach as the tree its own tools list, from a path down. */
export const sharedTreeQuery = (threadId: string, path = '/') =>
  queryOptions({
    queryKey: ['thread-shared-tree', threadId, path] as const,
    queryFn: ({ signal }): Promise<ThreadSharedTreeGetReply> =>
      ok(api.GET('/threads/{id}/shared/tree', { params: { path: { id: threadId }, query: { path, depth: 1 } }, signal })),
  });
