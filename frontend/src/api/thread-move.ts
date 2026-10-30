/** Stored thread reparenting reviews and their guarded confirmation. */
import { api, ok, repeatable } from '@/api/client';
import type { Thread, ThreadMovePlan } from '@/generated/api-types';

export function previewThreadMove(id: string, parentId: string, signal?: AbortSignal): Promise<ThreadMovePlan> {
  return ok(api.POST('/threads/{id}/move/preview', { params: { path: { id } }, body: { parent_id: parentId }, ...(signal === undefined ? {} : { signal }) }));
}

export function moveThread(id: string, plan: ThreadMovePlan, key: string): Promise<Thread> {
  return ok(api.POST('/threads/{id}/move', { params: { path: { id } }, body: { parent_id: plan.to.id, expected_revision: plan.expected_revision, preflight_id: plan.preflight_id }, ...repeatable(key) }));
}
